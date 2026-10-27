use crate::dialog::{self, Icon};
use std::collections::{HashMap, HashSet};

use crate::API_ENDPOINT;
use crate::auth::use_ensure_auth;
use crate::components::action_button::{ActionIconButton, ActionVariant};
use crate::components::data_list::{
    Column, LINK_CLASS, Row, SelectionProps, active_toggle, selectable_list_card, status_badge,
};
use crate::components::discovery_status::DiscoveryStatusCard;
use crate::components::error_page_themes::ErrorPageThemesCard;
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
use serde_derive::Deserialize;
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
        label: "common.domains",
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

#[derive(Clone, Copy, PartialEq, Eq, Default)]
pub enum ProxyTab {
    #[default]
    Proxies,
    ErrorPageThemes,
}

#[derive(Deserialize, Default)]
struct ProxyQuery {
    tab: Option<String>,
}

fn render_tabs(locale: Locale, active_tab: &UseStateHandle<ProxyTab>) -> Html {
    let tabs = [
        (ProxyTab::Proxies, "proxies.tab_proxies"),
        (ProxyTab::ErrorPageThemes, "proxies.tab_error_page_themes"),
    ];
    html! {
        <div class="border-b border-neutral-200 dark:border-neutral-700">
            <nav class="flex space-x-6">
                { for tabs.into_iter().map(|(tab, label)| {
                    let is_active = **active_tab == tab;
                    let active_cls = if is_active {
                        "border-blue-500 text-blue-500 dark:text-blue-400 font-semibold border-b-2"
                    } else {
                        "border-transparent text-neutral-500 hover:text-neutral-700 dark:text-neutral-400 dark:hover:text-neutral-200"
                    };
                    let onclick = {
                        let active_tab = active_tab.clone();
                        Callback::from(move |_| active_tab.set(tab))
                    };
                    html! {
                        <button type="button" {onclick} class={classes!("py-3", "px-1", "text-sm", "transition-colors", active_cls)}>
                            { locale.t(label) }
                        </button>
                    }
                }) }
            </nav>
        </div>
    }
}

#[function_component(ProxyList)]
pub fn proxy_list() -> Html {
    use_ensure_auth();
    let locale = use_locale();
    let location = use_location();
    let initial_tab = location
        .and_then(|loc| loc.query::<ProxyQuery>().ok())
        .and_then(|q| q.tab)
        .map(|t| match t.as_str() {
            "themes" | "error_page_themes" => ProxyTab::ErrorPageThemes,
            _ => ProxyTab::Proxies,
        })
        .unwrap_or_default();
    let active_tab = use_state(|| initial_tab);

    let (ports, ports_dispatcher) = use_store::<PortStore>();
    let (proxies, proxies_dispatcher) = use_store::<ProxyStore>();
    let (session, _) = use_store::<SessionStore>();
    let can_edit = session.can_edit_proxies();
    let selected = use_state(HashSet::<String>::new);
    let active_distribution = use_state(|| Option::<(String, Vec<UpstreamHealth>)>::None);

    let proxies_dispatcher_for_effect = proxies_dispatcher.clone();
    use_effect_with((), move |_| {
        let load = move || load_proxies(proxies_dispatcher_for_effect.clone());
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

    let Some(navigator) = use_navigator() else {
        return html!();
    };

    let navigator_cloned = navigator.clone();
    let new_proxy_onclick = Callback::from(move |_| {
        navigator_cloned.push(&Route::NewProxy);
    });

    let ctx = ProxyRowContext {
        locale,
        proxies: &proxies,
        ports: &ports,
        navigator: &navigator,
        proxies_dispatcher: &proxies_dispatcher,
        can_edit,
        active_distribution: &active_distribution,
    };
    let rows = proxies
        .entries
        .iter()
        .map(|entry| proxy_row(entry, &ctx))
        .collect::<Vec<_>>();
    let selection = build_selection(locale, can_edit, &proxies, &selected, &proxies_dispatcher);
    html! {
        <div class="space-y-6">
            { render_tabs(locale, &active_tab) }

            { match *active_tab {
                ProxyTab::Proxies => html! {
                    <>
                        <DiscoveryStatusCard />
                        { selectable_list_card(locale, proxies.loaded, "proxies.empty", &COLUMNS, &rows, selection.as_ref()) }
                        if let Some((proxy_name, upstreams)) = (*active_distribution).clone() {
                            <crate::components::upstream_distribution_modal::UpstreamDistributionModal
                                {locale}
                                {proxy_name}
                                {upstreams}
                                onclose={{
                                    let active = active_distribution.clone();
                                    Callback::from(move |()| active.set(None))
                                }}
                            />
                        }
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
                },
                ProxyTab::ErrorPageThemes => html! {
                    <ErrorPageThemesCard />
                },
            } }
        </div>
    }
}

fn bulk_delete_callback(
    locale: Locale,
    selected: &UseStateHandle<HashSet<String>>,
    proxies_dispatcher: &Dispatch<ProxyStore>,
) -> Callback<MouseEvent> {
    let (selected, proxies_dispatcher) = (selected.clone(), proxies_dispatcher.clone());
    Callback::from(move |e: MouseEvent| {
        e.prevent_default();
        let selected_ids = (*selected).clone();
        let question = locale.tf(
            "proxies.bulk_confirm_delete",
            &[("count", &selected_ids.len().to_string())],
        );
        let (selected, proxies_dispatcher) = (selected.clone(), proxies_dispatcher.clone());
        dialog::confirm_then(locale, question, async move {
            for id_str in selected_ids {
                if let Ok(id) = id_str.parse::<ShortId>() {
                    let _ = delete_site(id).await;
                }
            }
            selected.set(HashSet::new());
            load_proxies(proxies_dispatcher);
        });
    })
}

fn build_selection(
    locale: Locale,
    can_edit: bool,
    proxies: &ProxyStore,
    selected: &UseStateHandle<HashSet<String>>,
    proxies_dispatcher: &Dispatch<ProxyStore>,
) -> Option<SelectionProps> {
    if !can_edit {
        return None;
    }
    let selectable_keys: HashSet<String> = proxies
        .entries
        .iter()
        .filter(|e| !e.is_discovered())
        .map(|e| e.id.to_string())
        .collect();
    let all_selected =
        !selectable_keys.is_empty() && selectable_keys.iter().all(|k| selected.contains(k));

    let on_toggle = {
        let selected = selected.clone();
        Callback::from(move |key: String| {
            let mut set = (*selected).clone();
            if set.contains(&key) {
                set.remove(&key);
            } else {
                set.insert(key);
            }
            selected.set(set);
        })
    };

    let on_toggle_all = {
        let selected = selected.clone();
        let selectable_keys = selectable_keys.clone();
        Callback::from(move |()| {
            if all_selected {
                selected.set(HashSet::new());
            } else {
                selected.set(selectable_keys.clone());
            }
        })
    };

    let on_clear = {
        let selected = selected.clone();
        Some(Callback::from(move |_: MouseEvent| {
            selected.set(HashSet::new())
        }))
    };

    let bulk_delete = bulk_delete_callback(locale, selected, proxies_dispatcher);

    let bulk_actions = Some(html! {
        <button
            type="button"
            onclick={bulk_delete}
            class="inline-flex items-center text-red-600 dark:text-red-400 bg-white dark:bg-neutral-800 border border-red-300 dark:border-red-700 focus:outline-none hover:bg-red-50 hover:dark:bg-red-950 focus:ring-4 focus:ring-red-200 dark:focus:ring-red-900 font-medium rounded-lg text-xs px-2.5 py-1.5"
        >
            {locale.t("common.bulk_delete")}
        </button>
    });

    Some(SelectionProps {
        selected: (**selected).clone(),
        on_toggle,
        on_toggle_all,
        all_selected,
        selectable_keys: Some(selectable_keys),
        bulk_actions,
        on_clear,
    })
}

struct ProxyRowActions {
    config_onclick: Callback<MouseEvent>,
    config_label: &'static str,
    log_onclick: Callback<MouseEvent>,
    distribution: Option<Callback<MouseEvent>>,
    maintenance: Option<(bool, Callback<MouseEvent>)>,
    purge: Option<Callback<MouseEvent>>,
    delete: Option<Callback<MouseEvent>>,
}

fn config_action_variant(label: &str) -> ActionVariant {
    if label == "common.edit" {
        ActionVariant::Edit
    } else {
        ActionVariant::View
    }
}

fn render_proxy_actions(locale: Locale, actions: ProxyRowActions) -> Html {
    let edit_variant = config_action_variant(actions.config_label);
    html! {
        <>
            <ActionIconButton
                variant={edit_variant}
                title={locale.t(actions.config_label).to_string()}
                onclick={actions.config_onclick}
            />
            if let Some(dist_onclick) = actions.distribution {
                <ActionIconButton
                    variant={ActionVariant::Metrics}
                    title={locale.t("proxies.distribution").to_string()}
                    onclick={dist_onclick}
                />
            }
            <ActionIconButton
                variant={ActionVariant::Logs}
                title={locale.t("common.log").to_string()}
                onclick={actions.log_onclick}
            />
            if let Some((enabled, cb)) = actions.maintenance {
                <ActionIconButton
                    variant={ActionVariant::Pause}
                    title={if enabled { locale.t("proxies.exit_maintenance") } else { locale.t("proxies.enter_maintenance") }.to_string()}
                    onclick={cb}
                />
            }
            if let Some(purge_onclick) = actions.purge {
                <ActionIconButton
                    variant={ActionVariant::Restore}
                    title={locale.t("proxies.purge").to_string()}
                    onclick={purge_onclick}
                />
            }
            if let Some(delete_onclick) = actions.delete {
                <ActionIconButton
                    variant={ActionVariant::Delete}
                    title={locale.t("common.delete").to_string()}
                    onclick={delete_onclick}
                />
            }
        </>
    }
}

fn distribution_callback(
    name: &str,
    upstreams: &[UpstreamHealth],
    active_distribution: &UseStateHandle<Option<(String, Vec<UpstreamHealth>)>>,
) -> Option<Callback<MouseEvent>> {
    if upstreams.is_empty() {
        return None;
    }
    let active = active_distribution.clone();
    let name = name.to_string();
    let upstreams = upstreams.to_vec();
    Some(Callback::from(move |e: MouseEvent| {
        e.prevent_default();
        active.set(Some((name.clone(), upstreams.clone())));
    }))
}

struct ProxyRowContext<'a> {
    locale: Locale,
    proxies: &'a ProxyStore,
    ports: &'a PortStore,
    navigator: &'a Navigator,
    proxies_dispatcher: &'a Dispatch<ProxyStore>,
    can_edit: bool,
    active_distribution: &'a UseStateHandle<Option<(String, Vec<UpstreamHealth>)>>,
}

/// A proxy row. A discovered proxy, or an account that cannot change the proxies, gets only the
/// read actions.
fn proxy_row(entry: &ProxyEntry, ctx: &ProxyRowContext) -> Row {
    let id = entry.id;
    let read_only = entry.is_discovered() || !ctx.can_edit;
    let log_onclick = route_onclick(ctx.navigator, Route::ProxyLogView { id });
    let config_onclick = route_onclick(ctx.navigator, Route::ProxyView { id });
    let cache_enabled = matches!(&entry.proxy.kind, ProxyKind::Http(http) if http.cache.enabled);
    let onchange = toggle_onchange(id);

    let title = match entry.proxy.name.is_empty() {
        true => id.to_string(),
        false => entry.proxy.name.clone(),
    };

    let status = ctx.proxies.statuses.get(&id).cloned().unwrap_or_default();
    let config_label = match read_only {
        true => "common.view",
        false => "common.edit",
    };
    let distribution = distribution_callback(&title, &status.upstreams, ctx.active_distribution);
    let maintenance = if !read_only && let ProxyKind::Http(http) = &entry.proxy.kind {
        Some((
            http.maintenance.enabled,
            maintenance_onclick(
                id,
                !http.maintenance.enabled,
                ctx.proxies_dispatcher.clone(),
            ),
        ))
    } else {
        None
    };
    let purge = (cache_enabled && ctx.can_edit).then(|| purge_onclick(ctx.locale, id));
    let delete =
        (!read_only).then(|| delete_onclick(ctx.locale, id, ctx.proxies_dispatcher.clone()));

    let actions = render_proxy_actions(
        ctx.locale,
        ProxyRowActions {
            config_onclick,
            config_label,
            log_onclick,
            distribution,
            maintenance,
            purge,
            delete,
        },
    );

    Row {
        key: id.to_string(),
        cells: vec![
            html! { <>{title.clone()}</> },
            source_cell(ctx.locale, entry),
            addresses_cell(entry, ctx.ports),
            status_cell(ctx.locale, &status, &title, ctx.active_distribution),
            active_toggle(entry.proxy.active, read_only, onchange),
        ],
        actions,
    }
}

fn route_onclick(navigator: &Navigator, route: Route) -> Callback<MouseEvent> {
    let navigator = navigator.clone();
    Callback::from(move |_| {
        navigator.push(&route);
    })
}

/// Deletes the proxy after the confirmation of the account.
fn delete_onclick(
    locale: Locale,
    id: ShortId,
    proxies_dispatcher: Dispatch<ProxyStore>,
) -> Callback<MouseEvent> {
    Callback::from(move |e: MouseEvent| {
        e.prevent_default();
        let question = locale.tf("common.confirm_delete", &[("id", &id.to_string())]);
        let proxies_dispatcher = proxies_dispatcher.clone();
        dialog::confirm_then(locale, question, async move {
            let _ = delete_site(id).await;
            load_proxies(proxies_dispatcher);
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

/// The host names of an HTTP proxy, or the protocol and the address of every port of the other
/// kinds, which have no host names. A host name that looks like a public domain opens the site in
/// a new tab; a wildcard or an internal name stays plain text.
fn addresses_cell(entry: &ProxyEntry, ports: &PortStore) -> Html {
    if let ProxyKind::Http(http) = &entry.proxy.kind {
        return html! {
            <>
                { for http.vhosts.iter().map(|vhost| {
                    let vhost = vhost.to_string();
                    let url = format!("https://{vhost}");
                    let link = !vhost.contains('*');
                    html! {
                        <>
                            if link {
                                <a class={LINK_CLASS} href={url} target="_blank" rel="noopener noreferrer">{vhost}</a>
                            } else {
                                <span>{vhost}</span>
                            }
                            { " " }
                        </>
                    }
                }) }
            </>
        };
    }
    let text = entry
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
        .join(", ");
    html! { <span>{text}</span> }
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
fn status_cell(
    locale: Locale,
    status: &ProxyStatus,
    proxy_name: &str,
    active_distribution: &UseStateHandle<Option<(String, Vec<UpstreamHealth>)>>,
) -> Html {
    let (status_key, color) = match status.state {
        ProxyState::Active => ("state.active", "bg-green-500"),
        ProxyState::Inactive => ("state.inactive", "bg-neutral-500"),
        ProxyState::Maintenance => ("state.maintenance", "bg-amber-500"),
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

    let on_dist_click = {
        let active = active_distribution.clone();
        let name = proxy_name.to_string();
        let upstreams = status.upstreams.clone();
        Callback::from(move |e: MouseEvent| {
            e.prevent_default();
            active.set(Some((name.clone(), upstreams.clone())));
        })
    };

    html! {
        <div>
            {badge}
            <div class="flex items-center gap-1.5 mt-0.5 flex-wrap">
                <span class="block text-xs text-neutral-500 dark:text-neutral-400" title={title}>{text}</span>
                if !status.upstreams.is_empty() {
                    <button
                        type="button"
                        onclick={on_dist_click}
                        class="inline-flex items-center gap-1 px-1.5 py-0.5 rounded text-[10px] font-medium bg-neutral-100 hover:bg-neutral-200 dark:bg-neutral-800 dark:hover:bg-neutral-700 text-neutral-600 dark:text-neutral-300 border border-neutral-200/60 dark:border-neutral-700/60 transition cursor-pointer"
                        title={locale.t("proxies.distribution")}
                    >
                        <svg class="w-2.5 h-2.5 text-neutral-500 dark:text-neutral-400" fill="none" stroke="currentColor" viewBox="0 0 24 24">
                            <path stroke-linecap="round" stroke-linejoin="round" stroke-width="2" d="M9 19v-6a2 2 0 00-2-2H5a2 2 0 00-2 2v6a2 2 0 002 2h2a2 2 0 002-2zm0 0V9a2 2 0 012-2h2a2 2 0 012 2v10m-6 0a2 2 0 002 2h2a2 2 0 002-2m0 0V5a2 2 0 012-2h2a2 2 0 012 2v14a2 2 0 01-2 2h-2a2 2 0 01-2-2z" />
                        </svg>
                        <span>{locale.t("proxies.distribution")}</span>
                    </button>
                }
            </div>
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

fn maintenance_onclick(
    id: ShortId,
    enabled: bool,
    proxies_dispatcher: Dispatch<ProxyStore>,
) -> Callback<MouseEvent> {
    Callback::from(move |e: MouseEvent| {
        e.prevent_default();
        let proxies_dispatcher = proxies_dispatcher.clone();
        wasm_bindgen_futures::spawn_local(async move {
            let _ = toggle_maintenance(id, enabled).await;
            load_proxies(proxies_dispatcher);
        });
    })
}

async fn toggle_maintenance(id: ShortId, enabled: bool) -> Result<(), gloo_net::Error> {
    let mut entry: ProxyEntry = Request::get(&format!("{API_ENDPOINT}/proxies/{id}"))
        .send()
        .await?
        .json()
        .await?;
    if let ProxyKind::Http(ref mut http) = entry.proxy.kind {
        http.maintenance.enabled = enabled;
    }
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
            ..Default::default()
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
