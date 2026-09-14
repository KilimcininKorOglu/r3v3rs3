//! Publishes the TXT records of the ACME DNS-01 challenge through DNS provider APIs.

mod api;
mod azure;
mod cloudflare;
mod desec;
mod digitalocean;
mod exec;
mod gandi;
mod google;
mod hetzner;
mod linode;
mod ovh;
mod porkbun;
mod rfc2136;
mod route53;
mod rrset;
mod sigv4;
mod vultr;
mod webhook;

pub use exec::check_exec_provider;

use crate::cdn::fetch::HttpClient;
use anyhow::{anyhow, bail};
use async_trait::async_trait;
use hickory_resolver::{
    config::{NameServerConfigGroup, ResolverConfig, ResolverOpts},
    system_conf::read_system_conf,
    TokioAsyncResolver,
};
use r3v3rs3_api::{
    acme::{CloudProvider, DnsProvider, KeyedProvider, LocalProvider, TokenApi, TokenProvider},
    app::AcmeExecConfig,
};
use std::{future::Future, net::SocketAddr, time::Duration};
use tracing::{debug, info, warn};

/// Longest wait for the TXT records to become visible.
pub const PROPAGATION_TIMEOUT: Duration = Duration::from_secs(300);
/// Wait between two lookups of the TXT records.
pub const PROPAGATION_INTERVAL: Duration = Duration::from_secs(5);
/// TTL of the TXT records, in seconds.
const TXT_TTL: u32 = 60;

/// The TXT values that one challenge name needs.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TxtName {
    /// The name without the trailing dot, for example `_acme-challenge.example.com`.
    pub fqdn: String,
    pub values: Vec<String>,
}

/// The records that a provider added, with what it needs to remove them.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TxtRecord {
    pub fqdn: String,
    pub zone: String,
    pub values: Vec<String>,
    /// Record IDs, for providers that address each record separately.
    pub ids: Vec<String>,
}

impl TxtRecord {
    fn new(name: &TxtName, zone: String) -> Self {
        Self {
            fqdn: name.fqdn.clone(),
            zone,
            values: name.values.clone(),
            ids: Vec::new(),
        }
    }
}

#[async_trait]
pub trait DnsClient: Send + Sync {
    /// Adds every value of `name`. On an error, no record of `name` is left behind.
    async fn add_txt(&self, name: &TxtName) -> anyhow::Result<TxtRecord>;

    async fn remove_txt(&self, record: &TxtRecord) -> anyhow::Result<()>;
}

/// A provider API that holds each TXT value as a separate record with its own ID.
#[async_trait]
trait RecordApi: Send + Sync {
    /// The zone that holds `fqdn`, in the form that `create` and `delete` take.
    async fn zone(&self, fqdn: &str) -> anyhow::Result<String>;

    /// Creates one TXT record and returns its ID.
    async fn create(&self, zone: &str, fqdn: &str, value: &str) -> anyhow::Result<String>;

    async fn delete(&self, zone: &str, id: &str) -> anyhow::Result<()>;

    /// Applies the created or deleted records of `zone`, for providers that need a separate step.
    async fn commit(&self, _zone: &str) -> anyhow::Result<()> {
        Ok(())
    }
}

/// A [`DnsClient`] that adds the values of a name one record at a time.
struct PerValue<T>(T);

impl<T: RecordApi> PerValue<T> {
    /// Creates a record for every value of `record` and commits the zone.
    async fn create_all(&self, record: &mut TxtRecord) -> anyhow::Result<()> {
        for index in 0..record.values.len() {
            let value = &record.values[index];
            let id = self.0.create(&record.zone, &record.fqdn, value).await?;
            record.ids.push(id);
        }
        self.0.commit(&record.zone).await
    }
}

#[async_trait]
impl<T: RecordApi> DnsClient for PerValue<T> {
    async fn add_txt(&self, name: &TxtName) -> anyhow::Result<TxtRecord> {
        let zone = self.0.zone(&name.fqdn).await?;
        let mut record = TxtRecord::new(name, zone);
        if let Err(err) = self.create_all(&mut record).await {
            if let Err(remove_err) = self.remove_txt(&record).await {
                warn!(fqdn = name.fqdn, err = %remove_err, "failed to remove TXT record");
            }
            return Err(err);
        }
        Ok(record)
    }

    async fn remove_txt(&self, record: &TxtRecord) -> anyhow::Result<()> {
        if record.ids.is_empty() {
            return Ok(());
        }
        for id in &record.ids {
            self.0.delete(&record.zone, id).await?;
        }
        self.0.commit(&record.zone).await
    }
}

/// `acme_exec` holds the programs that the exec provider can run.
pub async fn client(
    provider: &DnsProvider,
    acme_exec: &AcmeExecConfig,
) -> anyhow::Result<Box<dyn DnsClient>> {
    let http = crate::cdn::fetch::build_client().await?;
    match provider {
        DnsProvider::Token(provider) => token_client(http, provider),
        DnsProvider::Keyed(provider) => keyed_client(http, provider),
        DnsProvider::Cloud(provider) => cloud_client(http, provider),
        DnsProvider::Local(provider) => local_client(http, provider, acme_exec),
    }
}

fn local_client(
    http: HttpClient,
    provider: &LocalProvider,
    acme_exec: &AcmeExecConfig,
) -> anyhow::Result<Box<dyn DnsClient>> {
    let client: Box<dyn DnsClient> = match provider {
        LocalProvider::Webhook { url, token } => Box::new(webhook::Webhook::new(http, url, token)?),
        LocalProvider::Exec { program } => Box::new(exec::Exec::new(program, acme_exec)?),
        LocalProvider::Rfc2136 {
            server,
            zone,
            key_name,
            key_algorithm,
            key_secret,
        } => Box::new(rfc2136::Rfc2136::new(
            server,
            zone,
            key_name,
            *key_algorithm,
            key_secret,
        )?),
    };
    Ok(client)
}

fn cloud_client(http: HttpClient, provider: &CloudProvider) -> anyhow::Result<Box<dyn DnsClient>> {
    let client: Box<dyn DnsClient> = match provider {
        CloudProvider::Azure {
            tenant_id,
            client_id,
            client_secret,
            subscription_id,
            api_url,
            auth_url,
        } => Box::new(rrset::MergedRrset(azure::Azure::new(
            api::ApiClient::new(http.clone(), api_url.as_deref(), azure::API_URL)?,
            api::ApiClient::new(http, auth_url.as_deref(), azure::AUTH_URL)?,
            azure::Credentials {
                tenant_id: tenant_id.clone(),
                client_id: client_id.clone(),
                client_secret: client_secret.clone(),
                subscription_id: subscription_id.clone(),
            },
        ))),
        CloudProvider::GoogleCloud {
            service_account_key,
            project_id,
            api_url,
            auth_url,
        } => Box::new(rrset::MergedRrset(google::GoogleCloud::new(
            api::ApiClient::new(http.clone(), api_url.as_deref(), google::API_URL)?,
            api::ApiClient::new(http, auth_url.as_deref(), google::AUTH_URL)?,
            google::ServiceAccount::parse(service_account_key, project_id)?,
        ))),
    };
    Ok(client)
}

fn token_client(http: HttpClient, provider: &TokenProvider) -> anyhow::Result<Box<dyn DnsClient>> {
    let url = provider.api_url.as_deref();
    let token = &provider.api_token;
    let client: Box<dyn DnsClient> = match provider.provider {
        TokenApi::Cloudflare => Box::new(PerValue(cloudflare::Cloudflare::new(
            api::ApiClient::new(http, url, cloudflare::API_URL)?,
            token,
        ))),
        TokenApi::DigitalOcean => Box::new(PerValue(digitalocean::DigitalOcean::new(
            api::ApiClient::new(http, url, digitalocean::API_URL)?,
            token,
        ))),
        TokenApi::Hetzner => Box::new(hetzner::Hetzner::new(
            api::ApiClient::new(http, url, hetzner::API_URL)?,
            token,
        )),
        TokenApi::Linode => Box::new(PerValue(linode::Linode::new(
            api::ApiClient::new(http, url, linode::API_URL)?,
            token,
        ))),
        TokenApi::Vultr => Box::new(PerValue(vultr::Vultr::new(
            api::ApiClient::new(http, url, vultr::API_URL)?,
            token,
        ))),
        TokenApi::Gandi => Box::new(rrset::MergedRrset(gandi::Gandi::new(
            api::ApiClient::new(http, url, gandi::API_URL)?,
            token,
        ))),
        TokenApi::Desec => Box::new(rrset::MergedRrset(desec::Desec::new(
            api::ApiClient::new(http, url, desec::API_URL)?,
            token,
        ))),
    };
    Ok(client)
}

fn keyed_client(http: HttpClient, provider: &KeyedProvider) -> anyhow::Result<Box<dyn DnsClient>> {
    let client: Box<dyn DnsClient> = match provider {
        KeyedProvider::Route53 {
            access_key_id,
            secret_access_key,
            api_url,
        } => Box::new(route53::Route53::new(
            api::ApiClient::new(http, api_url.as_deref(), route53::API_URL)?,
            access_key_id,
            secret_access_key,
        )),
        KeyedProvider::Porkbun {
            api_key,
            secret_api_key,
            api_url,
        } => Box::new(PerValue(porkbun::Porkbun::new(
            api::ApiClient::new(http, api_url.as_deref(), porkbun::API_URL)?,
            api_key,
            secret_api_key,
        ))),
        KeyedProvider::Ovh {
            endpoint,
            application_key,
            application_secret,
            consumer_key,
            api_url,
        } => Box::new(PerValue(ovh::Ovh::new(
            api::ApiClient::new(http, api_url.as_deref(), endpoint.url())?,
            application_key,
            application_secret,
            consumer_key,
        ))),
    };
    Ok(client)
}

/// Groups the TXT values by challenge name, because a domain and its wildcard share one name.
pub fn txt_names(values: impl IntoIterator<Item = (String, String)>) -> Vec<TxtName> {
    let mut names: Vec<TxtName> = Vec::new();
    for (domain, value) in values {
        let domain = domain.trim_start_matches("*.").trim_end_matches('.');
        let fqdn = format!("_acme-challenge.{}", domain.to_ascii_lowercase());
        match names.iter_mut().find(|name| name.fqdn == fqdn) {
            Some(name) => name.values.push(value),
            None => names.push(TxtName {
                fqdn,
                values: vec![value],
            }),
        }
    }
    names
}

/// Parent names of `fqdn` that can be its zone, the longest first. A single label is never a zone.
fn zone_candidates(fqdn: &str) -> Vec<&str> {
    let mut candidates = Vec::new();
    let mut rest = fqdn;
    while let Some((_, parent)) = rest.split_once('.') {
        if !parent.contains('.') {
            break;
        }
        candidates.push(parent);
        rest = parent;
    }
    candidates
}

/// `fqdn` relative to `zone`, for example `_acme-challenge.app` in `example.com`.
fn relative_name<'a>(fqdn: &'a str, zone: &str) -> &'a str {
    fqdn.strip_suffix(zone)
        .and_then(|name| name.strip_suffix('.'))
        .unwrap_or(fqdn)
}

/// The longest candidate zone of `fqdn` that `zones` holds, with the matching item.
/// `name` gives the zone name of an item. Names compare without case and without a trailing dot.
fn longest_zone<'a, 'z, T>(
    fqdn: &'a str,
    zones: &'z [T],
    name: impl Fn(&T) -> &str,
) -> Option<(&'a str, &'z T)> {
    zone_candidates(fqdn).into_iter().find_map(|candidate| {
        zones
            .iter()
            .find(|zone| {
                name(zone)
                    .trim_end_matches('.')
                    .eq_ignore_ascii_case(candidate)
            })
            .map(|zone| (candidate, zone))
    })
}

/// The longest domain of `domains` that holds `fqdn`. `provider` names the provider in the error.
fn domain_zone(fqdn: &str, domains: &[String], provider: &str) -> anyhow::Result<String> {
    longest_zone(fqdn, domains, String::as_str)
        .map(|(zone, _)| zone.to_string())
        .ok_or_else(|| anyhow!("no {provider} domain contains {fqdn}"))
}

/// The name and the ID of the longest zone of `zones` that holds `fqdn`.
/// Each item of `zones` is a zone name and its ID. `provider` names the provider in the error.
fn zone_with_id(
    fqdn: &str,
    zones: &[(String, String)],
    provider: &str,
) -> anyhow::Result<(String, String)> {
    longest_zone(fqdn, zones, |(name, _)| name)
        .map(|(name, (_, id))| (name.to_string(), id.clone()))
        .ok_or_else(|| anyhow!("no {provider} zone contains {fqdn}"))
}

/// Adds the TXT records, runs `task`, and then removes every record that was added,
/// whether `task` succeeds or not.
pub async fn with_txt_records<T>(
    client: &dyn DnsClient,
    names: &[TxtName],
    task: impl Future<Output = anyhow::Result<T>>,
) -> anyhow::Result<T> {
    let (records, added) = add_all(client, names).await;
    let result = match added {
        Ok(()) => task.await,
        Err(err) => Err(err),
    };
    for record in &records {
        match client.remove_txt(record).await {
            Ok(()) => info!(fqdn = record.fqdn, "removed TXT record"),
            Err(err) => warn!(fqdn = record.fqdn, %err, "failed to remove TXT record"),
        }
    }
    result
}

async fn add_all(
    client: &dyn DnsClient,
    names: &[TxtName],
) -> (Vec<TxtRecord>, anyhow::Result<()>) {
    let mut records = Vec::new();
    for name in names {
        match client.add_txt(name).await {
            Ok(record) => {
                info!(fqdn = name.fqdn, "added TXT record");
                records.push(record);
            }
            Err(err) => return (records, Err(err)),
        }
    }
    (records, Ok(()))
}

/// Waits until `resolver`, or the system resolver, returns every value of every name.
pub async fn wait_for_propagation(
    resolver: Option<SocketAddr>,
    names: &[TxtName],
    timeout: Duration,
    interval: Duration,
) -> anyhow::Result<()> {
    let deadline = tokio::time::Instant::now() + timeout;
    loop {
        // A new resolver for each round, so a cached empty answer does not hide a new record.
        let resolver = build_resolver(resolver)?;
        match missing_name(&resolver, names).await {
            None => return Ok(()),
            Some(fqdn) if tokio::time::Instant::now() >= deadline => {
                bail!(
                    "TXT record {fqdn} is not visible after {} seconds",
                    timeout.as_secs()
                )
            }
            Some(_) => tokio::time::sleep(interval).await,
        }
    }
}

fn build_resolver(addr: Option<SocketAddr>) -> anyhow::Result<TokioAsyncResolver> {
    let (config, opts) = match addr {
        Some(addr) => (
            ResolverConfig::from_parts(
                None,
                vec![],
                NameServerConfigGroup::from_ips_clear(&[addr.ip()], addr.port(), true),
            ),
            ResolverOpts::default(),
        ),
        None => read_system_conf()?,
    };
    Ok(TokioAsyncResolver::tokio(config, opts))
}

async fn missing_name<'a>(resolver: &TokioAsyncResolver, names: &'a [TxtName]) -> Option<&'a str> {
    for name in names {
        let visible = visible_values(resolver, &name.fqdn).await;
        if !name.values.iter().all(|value| visible.contains(value)) {
            return Some(&name.fqdn);
        }
    }
    None
}

async fn visible_values(resolver: &TokioAsyncResolver, fqdn: &str) -> Vec<String> {
    match resolver.txt_lookup(format!("{fqdn}.")).await {
        Ok(lookup) => lookup
            .iter()
            .map(|txt| {
                let data = txt
                    .txt_data()
                    .iter()
                    .flat_map(|chunk| chunk.iter().copied());
                String::from_utf8_lossy(&data.collect::<Vec<u8>>()).into_owned()
            })
            .collect(),
        Err(err) => {
            // The name has no record until the provider publishes it, so an error is expected here.
            debug!(fqdn, %err, "TXT lookup failed");
            Vec::new()
        }
    }
}

#[cfg(test)]
mod test {
    use super::*;
    use std::sync::Mutex;

    #[test]
    fn a_domain_and_its_wildcard_share_one_name() {
        let names = txt_names([
            ("Example.com".to_string(), "a".to_string()),
            ("*.example.com".to_string(), "b".to_string()),
            ("app.example.com".to_string(), "c".to_string()),
        ]);
        assert_eq!(
            names,
            vec![
                TxtName {
                    fqdn: "_acme-challenge.example.com".to_string(),
                    values: vec!["a".to_string(), "b".to_string()],
                },
                TxtName {
                    fqdn: "_acme-challenge.app.example.com".to_string(),
                    values: vec!["c".to_string()],
                },
            ]
        );
    }

    #[test]
    fn zone_candidates_start_with_the_longest_parent() {
        assert_eq!(
            zone_candidates("_acme-challenge.a.b.example.com"),
            vec!["a.b.example.com", "b.example.com", "example.com"]
        );
        assert_eq!(
            relative_name("_acme-challenge.a.example.com", "example.com"),
            "_acme-challenge.a"
        );
    }

    #[derive(Default)]
    struct FakeClient {
        fail_on: Option<String>,
        calls: Mutex<Vec<String>>,
    }

    #[async_trait]
    impl DnsClient for FakeClient {
        async fn add_txt(&self, name: &TxtName) -> anyhow::Result<TxtRecord> {
            self.calls
                .lock()
                .unwrap()
                .push(format!("add {}", name.fqdn));
            if self.fail_on.as_deref() == Some(name.fqdn.as_str()) {
                bail!("provider error");
            }
            Ok(TxtRecord::new(name, "zone".to_string()))
        }

        async fn remove_txt(&self, record: &TxtRecord) -> anyhow::Result<()> {
            self.calls
                .lock()
                .unwrap()
                .push(format!("remove {}", record.fqdn));
            Ok(())
        }
    }

    fn names() -> Vec<TxtName> {
        txt_names([
            ("a.test".to_string(), "1".to_string()),
            ("b.test".to_string(), "2".to_string()),
        ])
    }

    /// Runs a task with the records of `names()` through `client`, expects an error,
    /// and returns the calls that the client received.
    async fn failed_run(client: FakeClient, task: anyhow::Result<()>) -> Vec<String> {
        let result = with_txt_records(&client, &names(), async { task }).await;
        assert!(result.is_err());
        client.calls.into_inner().unwrap()
    }

    #[tokio::test]
    async fn records_are_removed_when_the_task_fails() {
        let calls = failed_run(FakeClient::default(), Err(anyhow::anyhow!("order failed"))).await;
        assert_eq!(
            calls,
            vec![
                "add _acme-challenge.a.test",
                "add _acme-challenge.b.test",
                "remove _acme-challenge.a.test",
                "remove _acme-challenge.b.test",
            ]
        );
    }

    #[tokio::test]
    async fn added_records_are_removed_when_a_later_add_fails() {
        let client = FakeClient {
            fail_on: Some("_acme-challenge.b.test".to_string()),
            ..Default::default()
        };
        assert_eq!(
            failed_run(client, Ok(())).await,
            vec![
                "add _acme-challenge.a.test",
                "add _acme-challenge.b.test",
                "remove _acme-challenge.a.test",
            ]
        );
    }

    #[test]
    fn the_longest_zone_wins_without_case_or_trailing_dot() {
        let zones = ["Example.com.".to_string(), "app.example.com".to_string()];
        assert_eq!(
            longest_zone("_acme-challenge.x.app.example.com", &zones, String::as_str),
            Some(("app.example.com", &zones[1]))
        );
        assert_eq!(
            longest_zone("_acme-challenge.example.com", &zones, String::as_str),
            Some(("example.com", &zones[0]))
        );
        assert_eq!(
            longest_zone("_acme-challenge.other.org", &zones, String::as_str),
            None
        );
    }
}
