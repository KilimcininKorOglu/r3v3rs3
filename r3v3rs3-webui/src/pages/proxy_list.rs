use crate::dialog::{self, Icon};
use std::collections::HashMap;

use crate::API_ENDPOINT;
use crate::auth::use_ensure_auth;
use crate::components::data_list::{
    Column, DANGER_LINK_CLASS, LINK_CLASS, Row, active_toggle, list_card, status_badge,
};
use crate::components::discovery_status::DiscoveryStatusCard;
use crate::i18n::use_locale;
use crate::pages::Route;
use crate::store::{PortStore, ProxyStore, SessionStore};
use gloo_net::http::Request;
use gloo_timers::callback::Interval;
use r3v3rs3_api::i18n::Locale;
use r3v3rs3_api::id::ShortId;
use r3v3rs3_api::port::PortEntry;
use r3v3rs3_api::proxy::{ProxyEntry, ProxyKind, ProxyState, ProxyStatus};
use r3v3rs3_api::upstream::{CircuitState, SrvStatus, UpstreamHealth};
use yew::prelude::*;
use yew_router::prelude::*;
use yewdux::prelude::*;

/// The proxy list reloads the proxies and their statuses at this interval.
const STATUS_REFRESH_INTERVAL_MS: u32 = 10_000;

const COLUMNS: [Column; 5] = [
    Column {
        label: "common.name",
        class: "whitespace-nowrap",
    },
    Column {
        label: "proxies.source",
        class: "whitespace-nowrap",
    },
    Column {
        label: "common.ports",
        class: "",
    },
    Column {
        label: "common.status",
        class: "w-48",
    },
    Column {
        label: "common.active",
        class: "w-0 whitespace-nowrap text-center",
    },
];

#[function_component(ProxyList)]
pub fn proxy_list() -> Html {
    use_ensure_auth();
    let locale = use_locale();

    let (ports, ports_dispatcher) = use_store::<PortStore>();
    let (proxies, proxies_dispatcher) = use_store::<ProxyStore>();
    let (session, _) = use_store::<SessionStore>();
    let can_edit = session.can_edit_proxies();

    use_effect_with((), move |_| {
        let load = move || load_proxies(proxies_dispatcher.clone());
        load();
        let interval = Interval::new(STATUS_REFRESH_INTERVAL_MS, load);
        move || drop(interval)
    });

    let ports_cloned = ports.clone();
    use_effect_with((), move |_| {
        wasm_bindgen_futures::spawn_local(async move {
            if let Ok(res) = get_ports().await {
                ports_dispatcher.set(PortStore {
                    entries: res,
                    loaded: true,
                    ..(*ports_cloned).clone()
                });
            }
        });
    });

    let navigator = use_navigator().unwrap();

    let navigator_cloned = navigator.clone();
    let new_proxy_onclick = Callback::from(move |_| {
        navigator_cloned.push(&Route::NewProxy);
    });

    let rows = proxies
        .entries
        .iter()
        .map(|entry| proxy_row(locale, entry, &proxies, &ports, &navigator, can_edit))
        .collect::<Vec<_>>();
    html! {
        <>
            <DiscoveryStatusCard />
            { list_card(locale, proxies.loaded, "proxies.empty", &COLUMNS, &rows) }
            if can_edit {
                <div class="flex items-center justify-end my-4">
                    <div>
                        <button onclick={new_proxy_onclick} class="inline-flex items-center text-neutral-500 dark:text-neutral-200 bg-white dark:bg-neutral-800 border border-neutral-300 dark:border-neutral-700 focus:outline-none hover:bg-neutral-100 hover:dark:bg-neutral-900 focus:ring-4 focus:ring-neutral-200 dark:focus:ring-neutral-600 font-medium rounded-lg text-sm px-4 py-2" type="button">
                            <img src="/assets/icons/add.svg" class="w-4 h-4 mr-1" />
                            {locale.t("common.add")}
                        </button>
                    </div>
                </div>
            }
        </>
    }
}

/// A proxy row. A discovered proxy, or an account that cannot change the proxies, gets only the
/// read actions.
fn proxy_row(
    locale: Locale,
    entry: &ProxyEntry,
    proxies: &ProxyStore,
    ports: &PortStore,
    navigator: &Navigator,
    can_edit: bool,
) -> Row {
    let id = entry.id;

    let log_onclick = route_onclick(navigator, Route::ProxyLogView { id });
    let config_onclick = route_onclick(navigator, Route::ProxyView { id });
    let delete_onclick = delete_onclick(locale, id);
    let cache_enabled = matches!(&entry.proxy.kind, ProxyKind::Http(http) if http.cache.enabled);
    let purge_onclick = purge_onclick(locale, id);
    let onchange = toggle_onchange(id);
    let port_names = port_names(entry, ports);

    let title = match entry.proxy.name.is_empty() {
        true => id.to_string(),
        false => entry.proxy.name.clone(),
    };

    let status = proxies.statuses.get(&id).cloned().unwrap_or_default();
    let read_only = entry.is_discovered() || !can_edit;
    let config_label = match read_only {
        true => "common.view",
        false => "common.edit",
    };

    Row {
        key: id.to_string(),
        cells: vec![
            html! { <>{title}</> },
            source_cell(locale, entry),
            html! { <>{port_names}</> },
            status_cell(locale, &status),
            active_toggle(entry.proxy.active, read_only, onchange),
        ],
        actions: html! {
            <>
                <a class={LINK_CLASS} onclick={config_onclick}>{locale.t(config_label)}</a>
                <a class={LINK_CLASS} onclick={log_onclick}>{locale.t("common.log")}</a>
                if cache_enabled && can_edit {
                    <a class={LINK_CLASS} onclick={purge_onclick}>{locale.t("proxies.purge")}</a>
                }
                if !read_only {
                    <a class={DANGER_LINK_CLASS} onclick={delete_onclick}>{locale.t("common.delete")}</a>
                }
            </>
        },
    }
}

fn route_onclick(navigator: &Navigator, route: Route) -> Callback<MouseEvent> {
    let navigator = navigator.clone();
    Callback::from(move |_| {
        navigator.push(&route);
    })
}

/// Deletes the proxy after the confirmation of the account.
fn delete_onclick(locale: Locale, id: ShortId) -> Callback<MouseEvent> {
    Callback::from(move |e: MouseEvent| {
        e.prevent_default();
        let question = locale.tf("common.confirm_delete", &[("id", &id.to_string())]);
        dialog::confirm_then(locale, question, async move {
            let _ = delete_site(id).await;
        });
    })
}

/// Purges the cache of the proxy after the confirmation of the account.
fn purge_onclick(locale: Locale, id: ShortId) -> Callback<MouseEvent> {
    Callback::from(move |e: MouseEvent| {
        e.prevent_default();
        let question = locale.tf("proxies.confirm_purge", &[("id", &id.to_string())]);
        dialog::confirm_then(locale, question, async move {
            match purge_cache(id).await {
                Ok(()) => dialog::message(locale, locale.t("proxies.purged"), Icon::Success).await,
                Err(err) => {
                    let text = locale.tf("proxies.purge_failed", &[("error", &err.to_string())]);
                    dialog::message(locale, &text, Icon::Error).await;
                }
            }
        });
    })
}

fn toggle_onchange(id: ShortId) -> Callback<Event> {
    Callback::from(move |_: Event| {
        wasm_bindgen_futures::spawn_local(async move {
            let _ = toggle_proxy(id).await;
        });
    })
}

/// The protocol and the address of every port of the proxy.
fn port_names(entry: &ProxyEntry, ports: &PortStore) -> String {
    entry
        .proxy
        .ports
        .iter()
        .filter_map(|port| ports.entries.iter().find(|p| p.id == *port))
        .map(|entry| {
            let addr = entry
                .port
                .listen
                .socket_addr()
                .map(|addr| addr.to_string())
                .unwrap_or_default();
            format!("{}/{}", entry.port.listen.protocol_name(), addr)
        })
        .collect::<Vec<_>>()
        .join(", ")
}

/// The source of the proxy. The title names the resource of a discovered proxy.
fn source_cell(locale: Locale, entry: &ProxyEntry) -> Html {
    let resource = entry
        .source
        .as_ref()
        .map(|source| source.resource.clone())
        .unwrap_or_default();
    html! { <span title={resource}>{source_label(locale, entry)}</span> }
}

/// The name of the discovery provider that created the proxy, or the manual source.
fn source_label(locale: Locale, entry: &ProxyEntry) -> String {
    match &entry.source {
        Some(source) => source.provider.name().to_string(),
        None => locale.t("proxies.source_manual").to_string(),
    }
}

/// The state of a proxy, and the number of healthy upstream servers. The title of the number lists
/// the unhealthy servers with their last errors.
fn status_cell(locale: Locale, status: &ProxyStatus) -> Html {
    let (status_key, color) = match status.state {
        ProxyState::Active => ("state.active", "bg-green-500"),
        ProxyState::Inactive => ("state.inactive", "bg-neutral-500"),
        ProxyState::Unknown => ("state.unknown", "bg-neutral-500"),
    };
    let badge = status_badge(locale.t(status_key), color);
    let srv_errors = srv_errors(locale, &status.srv);
    if status.upstreams.is_empty() && srv_errors.is_empty() {
        return badge;
    }
    let (healthy, unhealthy) = health_summary(locale, &status.upstreams);
    let text = locale.tf(
        "proxies.healthy_servers",
        &[
            ("healthy", &healthy.to_string()),
            ("total", &status.upstreams.len().to_string()),
        ],
    );
    let title = [unhealthy, srv_errors.join("\n")]
        .into_iter()
        .filter(|part| !part.is_empty())
        .collect::<Vec<_>>()
        .join("\n");
    html! {
        <div>
            {badge}
            <span class="block text-xs text-neutral-500 dark:text-neutral-400" title={title}>{text}</span>
            { for srv_errors.iter().map(|line| html! {
                <span class="block text-xs text-red-600 dark:text-red-400">{line}</span>
            }) }
        </div>
    }
}

/// One line for each SRV name whose last lookup failed.
fn srv_errors(locale: Locale, srv: &[SrvStatus]) -> Vec<String> {
    srv.iter()
        .filter_map(|status| {
            let error = status.error.as_deref()?;
            Some(locale.tf(
                "proxies.srv_error",
                &[("name", &status.name), ("error", error)],
            ))
        })
        .collect()
}

/// Returns the number of healthy servers, and one line for each server that is not healthy. A
/// server whose circuit is not closed is not healthy, and its line names the circuit state.
fn health_summary(locale: Locale, upstreams: &[UpstreamHealth]) -> (usize, String) {
    let is_healthy = |server: &&UpstreamHealth| server.healthy && server.circuit.is_closed();
    let healthy = upstreams.iter().filter(is_healthy).count();
    let unhealthy = upstreams
        .iter()
        .filter(|server| !is_healthy(server))
        .map(|server| unhealthy_line(locale, server))
        .collect::<Vec<_>>()
        .join("\n");
    (healthy, unhealthy)
}

fn unhealthy_line(locale: Locale, server: &UpstreamHealth) -> String {
    let circuit = match server.circuit {
        CircuitState::Closed => None,
        CircuitState::Open => Some(locale.t("proxies.circuit_open")),
        CircuitState::HalfOpen => Some(locale.t("proxies.circuit_half_open")),
    };
    let details = circuit
        .into_iter()
        .chain(server.last_error.as_deref())
        .collect::<Vec<_>>();
    if details.is_empty() {
        server.addr.clone()
    } else {
        format!("{}: {}", server.addr, details.join(", "))
    }
}

/// Loads the proxies and their statuses into the store.
fn load_proxies(dispatcher: Dispatch<ProxyStore>) {
    wasm_bindgen_futures::spawn_local(async move {
        let Ok(entries) = get_list().await else {
            return;
        };
        let mut statuses = HashMap::new();
        for entry in &entries {
            if let Ok(status) = get_status(entry.id).await {
                statuses.insert(entry.id, status);
            }
        }
        dispatcher.set(ProxyStore {
            entries,
            statuses,
            loaded: true,
        });
    });
}

async fn get_ports() -> Result<Vec<PortEntry>, gloo_net::Error> {
    Request::get(&format!("{API_ENDPOINT}/ports"))
        .send()
        .await?
        .json()
        .await
}

async fn get_list() -> Result<Vec<ProxyEntry>, gloo_net::Error> {
    Request::get(&format!("{API_ENDPOINT}/proxies"))
        .send()
        .await?
        .json()
        .await
}

async fn get_status(id: ShortId) -> Result<ProxyStatus, gloo_net::Error> {
    Request::get(&format!("{API_ENDPOINT}/proxies/{id}/status"))
        .send()
        .await?
        .json()
        .await
}

async fn purge_cache(id: ShortId) -> Result<(), gloo_net::Error> {
    let res = Request::delete(&format!("{API_ENDPOINT}/proxies/{id}/cache"))
        .send()
        .await?;
    if res.ok() {
        Ok(())
    } else {
        Err(gloo_net::Error::GlooError(format!(
            "HTTP status {}",
            res.status()
        )))
    }
}

async fn delete_site(id: ShortId) -> Result<(), gloo_net::Error> {
    Request::delete(&format!("{API_ENDPOINT}/proxies/{id}"))
        .send()
        .await?;
    Ok(())
}

async fn toggle_proxy(id: ShortId) -> Result<(), gloo_net::Error> {
    let mut entry: ProxyEntry = Request::get(&format!("{API_ENDPOINT}/proxies/{id}"))
        .send()
        .await?
        .json()
        .await?;
    entry.proxy.active = !entry.proxy.active;
    Request::put(&format!("{API_ENDPOINT}/proxies/{id}"))
        .json(&entry)?
        .send()
        .await?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use r3v3rs3_api::discovery::{DiscoveryProvider, DiscoverySource};
    use r3v3rs3_api::proxy::Proxy;

    #[test]
    fn source_label_names_the_provider_or_the_manual_source() {
        let manual = ProxyEntry::from(("abc".parse().unwrap(), Proxy::default()));
        assert_eq!(source_label(Locale::Tr, &manual), "Manuel");
        assert_eq!(source_label(Locale::En, &manual), "Manual");

        let discovered = ProxyEntry {
            source: Some(DiscoverySource {
                provider: DiscoveryProvider::Etcd,
                resource: "r3v3rs3/http/app".into(),
            }),
            ..manual
        };
        assert_eq!(source_label(Locale::Tr, &discovered), "etcd");
    }

    #[test]
    fn health_summary_lists_the_unhealthy_servers() {
        let server = |addr: &str, healthy: bool, last_error: Option<&str>| UpstreamHealth {
            addr: addr.into(),
            weight: 1,
            healthy,
            failures: 0,
            last_error: last_error.map(Into::into),
            circuit: CircuitState::Closed,
        };
        let upstreams = [
            server("http://a", true, Some("old error")),
            server("http://b", false, Some("connection refused")),
            server("http://c", false, None),
            UpstreamHealth {
                circuit: CircuitState::Open,
                ..server("http://d", true, Some("the server answered 503"))
            },
            UpstreamHealth {
                circuit: CircuitState::HalfOpen,
                ..server("http://e", true, None)
            },
        ];
        assert_eq!(
            health_summary(Locale::En, &upstreams),
            (
                1,
                "http://b: connection refused\nhttp://c\n\
                 http://d: circuit breaker is open, the server answered 503\n\
                 http://e: circuit breaker is half-open"
                    .to_string()
            )
        );
    }

    #[test]
    fn srv_errors_name_the_failed_lookups() {
        let status = |name: &str, error: Option<&str>| SrvStatus {
            name: name.into(),
            targets: Vec::new(),
            error: error.map(Into::into),
            refreshed_at: None,
        };
        let srv = [
            status("_http._tcp.a", None),
            status("_http._tcp.b", Some("no records found")),
        ];
        assert_eq!(
            srv_errors(Locale::En, &srv),
            ["SRV _http._tcp.b: no records found"]
        );
        assert!(srv_errors(Locale::Tr, &srv[..1]).is_empty());
    }
}
