use crate::auth::use_ensure_auth;
use crate::components::data_list::{
    active_toggle, list_card, status_badge, Column, Row, DANGER_LINK_CLASS, LINK_CLASS,
    WARNING_LINK_CLASS,
};
use crate::pages::Route;
use crate::store::PortStore;
use crate::API_ENDPOINT;
use gloo_net::http::Request;
use r3v3rs3_api::{
    id::ShortId,
    port::{PortEntry, PortStatus, SocketState},
};
use std::collections::HashMap;
use yew::prelude::*;
use yew_router::prelude::*;
use yewdux::prelude::*;

const COLUMNS: [Column; 5] = [
    Column {
        label: "Name",
        class: "whitespace-nowrap",
    },
    Column {
        label: "Protocol",
        class: "",
    },
    Column {
        label: "Address",
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

#[function_component(PortList)]
pub fn post_list() -> Html {
    use_ensure_auth();

    let (ports, dispatcher) = use_store::<PortStore>();

    use_effect_with((), move |_| {
        wasm_bindgen_futures::spawn_local(async move {
            if let Ok(res) = get_list().await {
                let mut statuses = HashMap::new();
                for entry in &res {
                    if let Ok(status) = get_status(entry.id).await {
                        statuses.insert(entry.id, status);
                    }
                }
                dispatcher.set(PortStore {
                    entries: res,
                    statuses,
                    loaded: true,
                });
            }
        });
    });

    let navigator = use_navigator().unwrap();

    let navigator_cloned = navigator.clone();
    let new_port_onclick = Callback::from(move |_| {
        navigator_cloned.push(&Route::NewPort);
    });

    let rows = ports
        .entries
        .iter()
        .map(|entry| port_row(entry, &ports, &navigator))
        .collect::<Vec<_>>();
    html! {
        <>
            { list_card(ports.loaded, "List is empty. Click 'Add' to configure a new port.", &COLUMNS, &rows) }
            <div class="flex items-center justify-end my-4">
                <div>
                    <button onclick={new_port_onclick} class="inline-flex items-center text-neutral-500 dark:text-neutral-200 bg-white dark:bg-neutral-800 border border-neutral-300 dark:border-neutral-700 focus:outline-none hover:bg-neutral-100 hover:dark:bg-neutral-900 focus:ring-4 focus:ring-neutral-200 dark:focus:ring-neutral-600 font-medium rounded-lg text-sm px-4 py-2" type="button">
                        <img src="/assets/icons/add.svg" class="w-4 h-4 mr-1" />
                        {"Add"}
                    </button>
                </div>
            </div>
        </>
    }
}

fn port_row(entry: &PortEntry, ports: &PortStore, navigator: &Navigator) -> Row {
    let id = entry.id;
    let title = if entry.port.name.is_empty() {
        id.to_string()
    } else {
        entry.port.name.clone()
    };
    let addr = entry
        .port
        .listen
        .socket_addr()
        .map(|addr| addr.to_string())
        .unwrap_or_default();
    let status = ports.statuses.get(&id).cloned().unwrap_or_default();
    let (status_text, color) = socket_state(&status.state.socket);

    let navigator_cloned = navigator.clone();
    let config_onclick = Callback::from(move |_| {
        navigator_cloned.push(&Route::PortView { id });
    });

    let navigator_cloned = navigator.clone();
    let log_onclick = Callback::from(move |_| {
        navigator_cloned.push(&Route::PortLogView { id });
    });

    let reset_onclick = Callback::from(move |e: MouseEvent| {
        e.prevent_default();
        if gloo_dialogs::confirm(&format!(
            "Are you sure to reset {id}?\nThis operation closes all existing connections. "
        )) {
            wasm_bindgen_futures::spawn_local(async move {
                let _ = reset_port(id).await;
            });
        }
    });

    let delete_onclick = Callback::from(move |e: MouseEvent| {
        e.prevent_default();
        if gloo_dialogs::confirm(&format!("Are you sure to delete {id}?")) {
            wasm_bindgen_futures::spawn_local(async move {
                let _ = delete_port(id).await;
            });
        }
    });

    let onchange = Callback::from(move |_: Event| {
        wasm_bindgen_futures::spawn_local(async move {
            let _ = toggle_port(id).await;
        });
    });

    Row {
        key: id.to_string(),
        cells: vec![
            html! { <>{title}</> },
            html! { <>{entry.port.listen.protocol_name()}</> },
            html! { <>{addr}</> },
            status_badge(status_text, color),
            active_toggle(entry.port.active, onchange),
        ],
        actions: html! {
            <>
                <a class={LINK_CLASS} onclick={config_onclick}>{"Edit"}</a>
                <a class={LINK_CLASS} onclick={log_onclick}>{"Log"}</a>
                <a class={WARNING_LINK_CLASS} onclick={reset_onclick}>{"Reset"}</a>
                <a class={DANGER_LINK_CLASS} onclick={delete_onclick}>{"Delete"}</a>
            </>
        },
    }
}

fn socket_state(state: &SocketState) -> (&'static str, &'static str) {
    match state {
        SocketState::Listening => ("Listening", "bg-green-500"),
        SocketState::Inactive => ("Inactive", "bg-neutral-500"),
        SocketState::AddressAlreadyInUse => ("Address In Use", "bg-red-500"),
        SocketState::PermissionDenied => ("Permission Denied", "bg-red-500"),
        SocketState::AddressNotAvailable => ("Address Unavailable", "bg-red-500"),
        SocketState::Error => ("Error", "bg-red-500"),
        SocketState::Unknown => ("Unknown", "bg-neutral-500"),
    }
}

async fn get_list() -> Result<Vec<PortEntry>, gloo_net::Error> {
    Request::get(&format!("{API_ENDPOINT}/ports"))
        .send()
        .await?
        .json()
        .await
}

async fn get_status(id: ShortId) -> Result<PortStatus, gloo_net::Error> {
    Request::get(&format!("{API_ENDPOINT}/ports/{id}/status"))
        .send()
        .await?
        .json()
        .await
}

async fn delete_port(id: ShortId) -> Result<(), gloo_net::Error> {
    Request::delete(&format!("{API_ENDPOINT}/ports/{id}"))
        .send()
        .await?;
    Ok(())
}

async fn reset_port(id: ShortId) -> Result<(), gloo_net::Error> {
    Request::get(&format!("{API_ENDPOINT}/ports/{id}/reset"))
        .send()
        .await?;
    Ok(())
}
async fn toggle_port(id: ShortId) -> Result<(), gloo_net::Error> {
    let mut entry: PortEntry = Request::get(&format!("{API_ENDPOINT}/ports/{id}"))
        .send()
        .await?
        .json()
        .await?;
    entry.port.active = !entry.port.active;
    Request::put(&format!("{API_ENDPOINT}/ports/{id}"))
        .json(&entry)?
        .send()
        .await?;
    Ok(())
}
