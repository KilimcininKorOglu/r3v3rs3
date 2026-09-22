use crate::API_ENDPOINT;
use crate::auth::use_ensure_auth;
use crate::components::data_list::{
    Column, DANGER_LINK_CLASS, LINK_CLASS, Row, WARNING_LINK_CLASS, active_toggle, list_card,
    status_badge,
};
use crate::dialog;
use crate::i18n::use_locale;
use crate::pages::Route;
use crate::store::{PortStore, SessionStore};
use gloo_net::http::Request;
use r3v3rs3_api::{
    i18n::Locale,
    id::ShortId,
    port::{PortEntry, PortState, PortStatus, SocketState},
    tls::TlsState,
};
use std::collections::HashMap;
use yew::prelude::*;
use yew_router::prelude::*;
use yewdux::prelude::*;

const COLUMNS: [Column; 5] = [
    Column {
        label: "common.name",
        class: "whitespace-nowrap",
    },
    Column {
        label: "common.protocol",
        class: "",
    },
    Column {
        label: "ports.address",
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

#[function_component(PortList)]
pub fn post_list() -> Html {
    use_ensure_auth();
    let locale = use_locale();

    let (ports, dispatcher) = use_store::<PortStore>();
    let (session, _) = use_store::<SessionStore>();
    let can_edit = session.can_edit();

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
        .map(|entry| port_row(locale, entry, &ports, &navigator, can_edit))
        .collect::<Vec<_>>();
    html! {
        <>
            { list_card(locale, ports.loaded, "ports.empty", &COLUMNS, &rows) }
            if can_edit {
                <div class="flex items-center justify-end my-4">
                    <div>
                        <button onclick={new_port_onclick} class="inline-flex items-center text-neutral-500 dark:text-neutral-200 bg-white dark:bg-neutral-800 border border-neutral-300 dark:border-neutral-700 focus:outline-none hover:bg-neutral-100 hover:dark:bg-neutral-900 focus:ring-4 focus:ring-neutral-200 dark:focus:ring-neutral-600 font-medium rounded-lg text-sm px-4 py-2" type="button">
                            <img src="/assets/icons/add.svg" class="w-4 h-4 mr-1" />
                            {locale.t("common.add")}
                        </button>
                    </div>
                </div>
            }
        </>
    }
}

/// A port row. An account that cannot change the ports gets only the read actions.
fn port_row(
    locale: Locale,
    entry: &PortEntry,
    ports: &PortStore,
    navigator: &Navigator,
    can_edit: bool,
) -> Row {
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
    let (status_key, color) = port_state(&status.state);

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
        let question = locale.tf("ports.confirm_reset", &[("id", &id.to_string())]);
        dialog::confirm_then(locale, question, async move {
            let _ = reset_port(id).await;
        });
    });

    let delete_onclick = Callback::from(move |e: MouseEvent| {
        e.prevent_default();
        let question = locale.tf("common.confirm_delete", &[("id", &id.to_string())]);
        dialog::confirm_then(locale, question, async move {
            let _ = delete_port(id).await;
        });
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
            status_badge(locale.t(status_key), color),
            active_toggle(entry.port.active, !can_edit, onchange),
        ],
        actions: html! {
            <>
                <a class={LINK_CLASS} onclick={config_onclick}>{locale.t(if can_edit { "common.edit" } else { "common.view" })}</a>
                <a class={LINK_CLASS} onclick={log_onclick}>{locale.t("common.log")}</a>
                if can_edit {
                    <a class={WARNING_LINK_CLASS} onclick={reset_onclick}>{locale.t("ports.reset")}</a>
                    <a class={DANGER_LINK_CLASS} onclick={delete_onclick}>{locale.t("common.delete")}</a>
                }
            </>
        },
    }
}

/// Returns the translation key and the color of the port state. A TLS error hides the socket state,
/// because the port closes every connection.
fn port_state(state: &PortState) -> (&'static str, &'static str) {
    if state.tls == Some(TlsState::Error) {
        return ("state.tls_error", "bg-red-500");
    }
    socket_state(&state.socket)
}

/// Returns the translation key and the color of the socket state.
fn socket_state(state: &SocketState) -> (&'static str, &'static str) {
    match state {
        SocketState::Listening => ("state.listening", "bg-green-500"),
        SocketState::Inactive => ("state.inactive", "bg-neutral-500"),
        SocketState::AddressAlreadyInUse => ("state.address_in_use", "bg-red-500"),
        SocketState::PermissionDenied => ("state.permission_denied", "bg-red-500"),
        SocketState::AddressNotAvailable => ("state.address_unavailable", "bg-red-500"),
        SocketState::Error => ("state.error", "bg-red-500"),
        SocketState::Unknown => ("state.unknown", "bg-neutral-500"),
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
