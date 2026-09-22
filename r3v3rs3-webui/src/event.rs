use crate::{
    API_ENDPOINT,
    store::{
        AcmeStore, CertStore, ClusterStore, DiscoveryStore, PortStore, ProxyStore, SessionStore,
    },
};
use futures::StreamExt;
use futures::channel::oneshot;
use futures::future::{AbortHandle, abortable};
use gloo_net::eventsource::futures::EventSource;
use gloo_timers::callback::Timeout;
use gloo_utils::format::JsValueSerdeExt;
use r3v3rs3_api::event::ServerEvent;
use wasm_bindgen_futures::spawn_local;
use yew::prelude::*;
use yewdux::prelude::*;

/// The wait before a new connection after the event stream failed.
const RETRY_DELAY_MS: u32 = 5000;

/// The stores that the server events update.
#[derive(Clone)]
struct Dispatchers {
    ports: Dispatch<PortStore>,
    certs: Dispatch<CertStore>,
    acme: Dispatch<AcmeStore>,
    proxies: Dispatch<ProxyStore>,
    discovery: Dispatch<DiscoveryStore>,
    cluster: Dispatch<ClusterStore>,
}

/// Reads the server events while the client has a session. The stream opens when the session
/// loads, closes on logout, and opens again after a failure.
#[hook]
pub fn use_event_subscriber() {
    let (session, _) = use_store::<SessionStore>();
    let dispatchers = Dispatchers {
        ports: use_store::<PortStore>().1,
        certs: use_store::<CertStore>().1,
        acme: use_store::<AcmeStore>().1,
        proxies: use_store::<ProxyStore>().1,
        discovery: use_store::<DiscoveryStore>().1,
        cluster: use_store::<ClusterStore>().1,
    };
    let attempt = use_state(|| 0_u32);
    let signed_in = session.info.is_some();
    use_effect_with((signed_in, *attempt), move |&(signed_in, _)| {
        let handle = signed_in.then(|| subscribe(dispatchers, attempt));
        move || {
            if let Some(handle) = handle {
                handle.abort();
            }
        }
    });
}

/// Reads the event stream until it fails, then starts the next attempt after a delay. Aborting
/// the task closes the stream.
fn subscribe(dispatchers: Dispatchers, attempt: UseStateHandle<u32>) -> AbortHandle {
    let (task, handle) = abortable(async move {
        read_events(&dispatchers).await;
        let (done, wait) = oneshot::channel();
        let _timeout = Timeout::new(RETRY_DELAY_MS, move || {
            let _ = done.send(());
        });
        // The sender lives until the timeout fires, so the wait always ends with `Ok`.
        let _ = wait.await;
        attempt.set(*attempt + 1);
    });
    spawn_local(async move {
        // An aborted task is the closed stream of a logout.
        let _ = task.await;
    });
    handle
}

async fn read_events(dispatchers: &Dispatchers) {
    let mut source = match EventSource::new(&format!("{API_ENDPOINT}/events")) {
        Ok(source) => source,
        Err(err) => {
            web_sys::console::error_1(&format!("the event stream did not open: {err}").into());
            return;
        }
    };
    let mut stream = match source.subscribe("message") {
        Ok(stream) => stream,
        Err(err) => {
            web_sys::console::error_1(&format!("the event stream did not open: {err}").into());
            return;
        }
    };
    while let Some(Ok((_, msg))) = stream.next().await {
        let event = msg
            .data()
            .into_serde::<String>()
            .map_err(|err| err.to_string())
            .and_then(|s| serde_json::from_str::<ServerEvent>(&s).map_err(|err| err.to_string()));
        match event {
            Ok(event) => apply_event(dispatchers, event),
            Err(err) => {
                web_sys::console::error_1(&format!("an unreadable server event: {err}").into())
            }
        }
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
