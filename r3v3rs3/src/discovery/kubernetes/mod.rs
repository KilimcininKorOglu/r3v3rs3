//! The Kubernetes provider. It reads the Ingress resources and the Services, EndpointSlices and TLS
//! secrets that they use, and follows their changes with watch streams.

mod cluster;
mod crd;
mod ingress;

use super::{Built, Reporter, Watch, DEBOUNCE, MIN_BACKOFF};
use anyhow::anyhow;
use cluster::Resources;
use crd::R3v3rs3Proxy;
use futures::stream::{BoxStream, SelectAll, StreamExt, TryStreamExt};
use k8s_openapi::api::core::v1::{Secret, Service};
use k8s_openapi::api::discovery::v1::EndpointSlice;
use k8s_openapi::api::networking::v1::Ingress;
use k8s_openapi::NamespaceResourceScope;
use kube::config::{KubeConfigOptions, Kubeconfig};
use kube::runtime::reflector::{self, Store};
use kube::runtime::watcher;
use kube::{Api, Client, Config, Resource};
use r3v3rs3_api::discovery::{DiscoveryProvider, KubernetesDiscoveryConfig};
use serde::de::DeserializeOwned;
use std::convert::Infallible;
use std::fmt::Debug;
use std::sync::Arc;
use std::time::Duration;
use tokio::time::Instant;

const PROVIDER: DiscoveryProvider = DiscoveryProvider::Kubernetes;

/// The field selector of the secrets that hold a certificate and its private key.
const TLS_SECRET_SELECTOR: &str = "type=kubernetes.io/tls";

/// The events of every watch stream. An event updates the store of its stream.
type Events = SelectAll<BoxStream<'static, Result<(), watcher::Error>>>;

/// The watched resources, with one store for each namespace. A resource that the settings do not
/// read has no store.
struct Stores {
    ingresses: Vec<Store<Ingress>>,
    proxies: Vec<Store<R3v3rs3Proxy>>,
    services: Vec<Store<Service>>,
    slices: Vec<Store<EndpointSlice>>,
    secrets: Vec<Store<Secret>>,
}

impl Stores {
    /// Waits until every store holds its first list.
    async fn wait_until_ready(&self) -> anyhow::Result<()> {
        tokio::try_join!(
            ready(&self.ingresses),
            ready(&self.proxies),
            ready(&self.services),
            ready(&self.slices),
            ready(&self.secrets),
        )?;
        Ok(())
    }

    fn resources(&self) -> Resources {
        Resources {
            ingresses: state(&self.ingresses),
            proxies: state(&self.proxies),
            services: state(&self.services),
            slices: state(&self.slices),
            secrets: state(&self.secrets),
        }
    }
}

/// The proxies of the Ingress and the R3v3rs3Proxy resources, and the certificates of the TLS
/// secrets of the Ingress resources.
fn build(resources: &Resources, settings: &ingress::Settings) -> Built {
    let mut built = ingress::build(resources, settings);
    let custom = crd::build(resources, &settings.ports);
    built.proxies.extend(custom.proxies);
    for issue in custom.issues {
        built.issue(&issue.resource, issue.message);
    }
    built
}

async fn ready<K>(stores: &[Store<K>]) -> anyhow::Result<()>
where
    K: Resource<DynamicType = ()> + Clone + 'static,
{
    for store in stores {
        store
            .wait_until_ready()
            .await
            .map_err(|_| anyhow!("the watch stream stopped before its first list"))?;
    }
    Ok(())
}

fn state<K>(stores: &[Store<K>]) -> Vec<Arc<K>>
where
    K: Resource<DynamicType = ()> + Clone + 'static,
{
    stores.iter().flat_map(Store::state).collect()
}

/// Starts a watch stream for each API, and returns the store that each stream fills.
fn reflect<K>(apis: Vec<Api<K>>, config: &watcher::Config, events: &mut Events) -> Vec<Store<K>>
where
    K: Resource<DynamicType = ()> + Clone + DeserializeOwned + Debug + Send + Sync + 'static,
{
    apis.into_iter()
        .map(|api| {
            let (store, writer) = reflector::store();
            let stream = reflector::reflector(writer, watcher::watcher(api, config.clone()));
            events.push(stream.map_ok(|_| ()).boxed());
            store
        })
        .collect()
}

fn check(event: Option<Result<(), watcher::Error>>) -> anyhow::Result<()> {
    match event {
        Some(Ok(())) => Ok(()),
        Some(Err(err)) => Err(anyhow!("the Kubernetes watch failed: {err}")),
        None => Err(anyhow!("the Kubernetes watch streams ended")),
    }
}

/// Reads the events until every store holds its first list.
async fn wait_until_ready(stores: &Stores, events: &mut Events) -> anyhow::Result<()> {
    let ready = stores.wait_until_ready();
    tokio::pin!(ready);
    loop {
        tokio::select! {
            result = &mut ready => return result,
            event = events.next() => check(event)?,
        }
    }
}

/// Reads the events that arrive within `duration`, so the stores hold every change of the
/// period.
async fn drain(events: &mut Events, duration: Duration) -> anyhow::Result<()> {
    let deadline = Instant::now() + duration;
    while let Ok(event) = tokio::time::timeout_at(deadline, events.next()).await {
        check(event)?;
    }
    Ok(())
}

pub(super) struct Provider {
    kubeconfig: String,
    namespaces: Vec<String>,
    ingress: bool,
    crd: bool,
    settings: ingress::Settings,
    reporter: Reporter,
}

#[async_trait::async_trait]
impl Watch for Provider {
    fn reporter(&self) -> &Reporter {
        &self.reporter
    }

    async fn watch(&self, backoff: &mut Duration) -> anyhow::Result<Infallible> {
        self.follow(backoff).await
    }
}

impl Provider {
    pub(super) fn new(config: &KubernetesDiscoveryConfig, reporter: Reporter) -> Self {
        let namespaces = config
            .namespaces
            .iter()
            .map(|namespace| namespace.trim().to_string())
            .filter(|namespace| !namespace.is_empty())
            .collect();
        Self {
            kubeconfig: config.kubeconfig.trim().to_string(),
            namespaces,
            ingress: config.ingress,
            crd: config.crd,
            settings: ingress::Settings {
                ingress_class: config.ingress_class.trim().to_string(),
                ports: config.ports.clone(),
            },
            reporter,
        }
    }

    /// Lists the resources, then rebuilds the proxies after the changes of each period.
    async fn follow(&self, backoff: &mut Duration) -> anyhow::Result<Infallible> {
        let client = self.client().await?;
        let mut events = SelectAll::new();
        let stores = self.watch_resources(&client, &mut events);
        wait_until_ready(&stores, &mut events).await?;
        loop {
            let built = build(&stores.resources(), &self.settings);
            self.reporter.running(built).await?;
            *backoff = MIN_BACKOFF;
            check(events.next().await)?;
            drain(&mut events, DEBOUNCE).await?;
        }
    }

    /// The errors of kube print their source in their text. The errors here keep only that text,
    /// so the status does not repeat the source.
    async fn client(&self) -> anyhow::Result<Client> {
        let config = if self.kubeconfig.is_empty() {
            Config::infer()
                .await
                .map_err(|err| anyhow!("no kubeconfig file and no service account found: {err}"))?
        } else {
            let kubeconfig =
                Kubeconfig::read_from(&self.kubeconfig).map_err(|err| anyhow!("{err}"))?;
            Config::from_custom_kubeconfig(kubeconfig, &KubeConfigOptions::default())
                .await
                .map_err(|err| anyhow!("invalid kubeconfig file: {err}"))?
        };
        Client::try_from(config)
            .map_err(|err| anyhow!("cannot create the Kubernetes client: {err}"))
    }

    fn watch_resources(&self, client: &Client, events: &mut Events) -> Stores {
        let config = watcher::Config::default();
        let secrets = config.clone().fields(TLS_SECRET_SELECTOR);
        Stores {
            ingresses: reflect(self.apis(self.ingress, client), &config, events),
            proxies: reflect(self.apis(self.crd, client), &config, events),
            services: reflect(self.apis(true, client), &config, events),
            slices: reflect(self.apis(true, client), &config, events),
            secrets: reflect(self.apis(true, client), &secrets, events),
        }
    }

    /// One API for each namespace, or one API for every namespace. A resource that the settings
    /// do not read has no API.
    fn apis<K>(&self, read: bool, client: &Client) -> Vec<Api<K>>
    where
        K: Resource<Scope = NamespaceResourceScope, DynamicType = ()>,
    {
        if !read {
            return Vec::new();
        }
        if self.namespaces.is_empty() {
            return vec![Api::all(client.clone())];
        }
        self.namespaces
            .iter()
            .map(|namespace| Api::namespaced(client.clone(), namespace))
            .collect()
    }
}
