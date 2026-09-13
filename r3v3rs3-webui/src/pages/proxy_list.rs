use std::collections::HashMap;

use crate::auth::use_ensure_auth;
use crate::components::data_list::{
    active_toggle, list_card, status_badge, Column, Row, DANGER_LINK_CLASS, LINK_CLASS,
};
use crate::pages::Route;
use crate::store::{PortStore, ProxyStore};
use crate::API_ENDPOINT;
use gloo_net::http::Request;
use r3v3rs3_api::id::ShortId;
use r3v3rs3_api::port::PortEntry;
use r3v3rs3_api::proxy::{ProxyEntry, ProxyKind, ProxyState, ProxyStatus};
use yew::prelude::*;
use yew_router::prelude::*;
use yewdux::prelude::*;

const COLUMNS: [Column; 4] = [
    Column {
        label: "Name",
        class: "whitespace-nowrap",
    },
    Column {
        label: "Ports",
        class: "",
    },
    Column {
        label: "Status",
        class: "w-48",
    },
    Column {
        label: "Active",
        class: "w-0 whitespace-nowrap text-center",
    },
];

#[function_component(ProxyList)]
pub fn proxy_list() -> Html {
    use_ensure_auth();

    let (ports, ports_dispatcher) = use_store::<PortStore>();
    let (proxies, proxies_dispatcher) = use_store::<ProxyStore>();

    use_effect_with((), move |_| {
        wasm_bindgen_futures::spawn_local(async move {
            if let Ok(res) = get_list().await {
                let mut statuses = HashMap::new();
                for entry in &res {
                    if let Ok(status) = get_status(entry.id).await {
                        statuses.insert(entry.id, status);
                    }
                }
                proxies_dispatcher.set(ProxyStore {
                    entries: res,
                    statuses,
                    loaded: true,
                });
            }
        });
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
        .map(|entry| proxy_row(entry, &proxies, &ports, &navigator))
        .collect::<Vec<_>>();
    html! {
        <>
            { list_card(proxies.loaded, "List is empty. Click 'Add' to configure a new proxy.", &COLUMNS, &rows) }
            <div class="flex items-center justify-end my-4">
                <div>
                    <button onclick={new_proxy_onclick} class="inline-flex items-center text-neutral-500 dark:text-neutral-200 bg-white dark:bg-neutral-800 border border-neutral-300 dark:border-neutral-700 focus:outline-none hover:bg-neutral-100 hover:dark:bg-neutral-900 focus:ring-4 focus:ring-neutral-200 dark:focus:ring-neutral-600 font-medium rounded-lg text-sm px-4 py-2" type="button">
                        <img src="/assets/icons/add.svg" class="w-4 h-4 mr-1" />
                        {"Add"}
                    </button>
                </div>
            </div>
        </>
    }
}

fn proxy_row(
    entry: &ProxyEntry,
    proxies: &ProxyStore,
    ports: &PortStore,
    navigator: &Navigator,
) -> Row {
    let id = entry.id;

    let navigator_cloned = navigator.clone();
    let log_onclick = Callback::from(move |_| {
        navigator_cloned.push(&Route::ProxyLogView { id });
    });

    let navigator_cloned = navigator.clone();
    let config_onclick = Callback::from(move |_| {
        navigator_cloned.push(&Route::ProxyView { id });
    });

    let delete_onclick = Callback::from(move |e: MouseEvent| {
        e.prevent_default();
        if gloo_dialogs::confirm(&format!("Are you sure to delete {id}?")) {
            wasm_bindgen_futures::spawn_local(async move {
                let _ = delete_site(id).await;
            });
        }
    });

    let cache_enabled = matches!(&entry.proxy.kind, ProxyKind::Http(http) if http.cache.enabled);
    let purge_onclick = Callback::from(move |e: MouseEvent| {
        e.prevent_default();
        if gloo_dialogs::confirm(&format!("Are you sure to purge the cache of {id}?")) {
            wasm_bindgen_futures::spawn_local(async move {
                match purge_cache(id).await {
                    Ok(()) => gloo_dialogs::alert("The cache is purged."),
                    Err(err) => gloo_dialogs::alert(&format!("Failed to purge the cache: {err}")),
                }
            });
        }
    });

    let onchange = Callback::from(move |_: Event| {
        wasm_bindgen_futures::spawn_local(async move {
            let _ = toggle_proxy(id).await;
        });
    });

    let port_names = entry
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

    let title = if entry.proxy.name.is_empty() {
        id.to_string()
    } else {
        entry.proxy.name.clone()
    };

    let status = proxies.statuses.get(&id).cloned().unwrap_or_default();
    let (status_text, color) = match status.state {
        ProxyState::Active => ("Active", "bg-green-500"),
        ProxyState::Inactive => ("Inactive", "bg-neutral-500"),
        ProxyState::Unknown => ("Unknown", "bg-neutral-500"),
    };

    Row {
        key: id.to_string(),
        cells: vec![
            html! { <>{title}</> },
            html! { <>{port_names}</> },
            status_badge(status_text, color),
            active_toggle(entry.proxy.active, onchange),
        ],
        actions: html! {
            <>
                <a class={LINK_CLASS} onclick={config_onclick}>{"Edit"}</a>
                <a class={LINK_CLASS} onclick={log_onclick}>{"Log"}</a>
                if cache_enabled {
                    <a class={LINK_CLASS} onclick={purge_onclick}>{"Purge"}</a>
                }
                <a class={DANGER_LINK_CLASS} onclick={delete_onclick}>{"Delete"}</a>
            </>
        },
    }
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
