use crate::{
    store::{AcmeStore, CertStore, ClusterStore, DiscoveryStore, PortStore, ProxyStore},
    API_ENDPOINT,
};
use futures::StreamExt;
use gloo_net::eventsource::futures::EventSource;
use gloo_timers::callback::Timeout;
use gloo_utils::format::JsValueSerdeExt;
use r3v3rs3_api::event::ServerEvent;
use serde_derive::{Deserialize, Serialize};
use wasm_bindgen_futures::spawn_local;
use yew::prelude::*;
use yewdux::prelude::*;

#[derive(Default, Clone, PartialEq, Serialize, Deserialize, Store)]
struct EventSession {
    active: bool,
}

/// The stores that the server events update.
struct Dispatchers {
    ports: Dispatch<PortStore>,
    certs: Dispatch<CertStore>,
    acme: Dispatch<AcmeStore>,
    proxies: Dispatch<ProxyStore>,
    discovery: Dispatch<DiscoveryStore>,
    cluster: Dispatch<ClusterStore>,
}

#[hook]
pub fn use_event_subscriber() {
    let (event, dispatcher) = use_store::<EventSession>();
    let dispatchers = Dispatchers {
        ports: use_store::<PortStore>().1,
        certs: use_store::<CertStore>().1,
        acme: use_store::<AcmeStore>().1,
        proxies: use_store::<ProxyStore>().1,
        discovery: use_store::<DiscoveryStore>().1,
        cluster: use_store::<ClusterStore>().1,
    };
    if !event.active {
        let mut es = EventSource::new(&format!("{API_ENDPOINT}/events")).unwrap();
        let mut stream = es.subscribe("message").unwrap();

        dispatcher.set(EventSession { active: true });
        spawn_local(async move {
            let _es = es;
            while let Some(Ok((_, msg))) = stream.next().await {
                let event = msg
                    .data()
                    .into_serde::<String>()
                    .ok()
                    .and_then(|s| serde_json::from_str::<ServerEvent>(&s).ok());
                if let Some(event) = event {
                    apply_event(&dispatchers, event);
                }
            }
            Timeout::new(5000, move || {
                dispatcher.set(EventSession { active: false });
            })
            .forget();
        })
    }
}

fn apply_event(stores: &Dispatchers, event: ServerEvent) {
    match event {
        ServerEvent::PortTableUpdated { entries } => {
            stores.ports.reduce(|state| {
                PortStore {
                    entries,
                    ..(*state).clone()
                }
                .into()
            });
        }
        ServerEvent::CertsUpdated { entries } => {
            stores.certs.set(CertStore {
                entries,
                loaded: true,
            });
        }
        ServerEvent::AcmeUpdated { entries } => {
            stores.acme.set(AcmeStore {
                entries,
                loaded: true,
            });
        }
        ServerEvent::ProxiesUpdated { entries } => {
            stores.proxies.reduce(|state| {
                ProxyStore {
                    entries,
                    loaded: true,
                    ..(*state).clone()
                }
                .into()
            });
        }
        ServerEvent::PortStatusUpdated { id, status } => {
            stores.ports.reduce(|state| {
                let mut cloned = (*state).clone();
                cloned.statuses.insert(id, status);
                cloned.into()
            });
        }
        ServerEvent::ProxyStatusUpdated { id, status } => {
            stores.proxies.reduce(|state| {
                let mut cloned = (*state).clone();
                cloned.statuses.insert(id, status);
                cloned.into()
            });
        }
        ServerEvent::DiscoveryStatusUpdated { entries } => {
            stores.discovery.set(DiscoveryStore { entries });
        }
        ServerEvent::ClusterStatusUpdated { status } => {
            stores.cluster.set(ClusterStore { status });
        }
        _ => (),
    }
}
