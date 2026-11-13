use std::collections::HashMap;

use crate::components::button::{ButtonKind, ButtonSize, button_class};
use crate::pages::settings::{failure_box, fetch_json, send_json};
use crate::{
    API_ENDPOINT,
    auth::use_ensure_auth,
    components::port_config::PortConfig,
    i18n::use_locale,
    pages::Route,
    store::{PortStore, SessionStore},
};
use gloo_net::http::Request;
use r3v3rs3_api::{
    i18n::Locale,
    id::ShortId,
    port::{Port, PortEntry},
};
use yew::prelude::*;
use yew_router::prelude::*;
use yewdux::prelude::*;

#[derive(Properties, PartialEq)]
pub struct Props {
    pub id: ShortId,
}

#[function_component(PortView)]
pub fn port_view(props: &Props) -> Html {
    use_ensure_auth();
    let locale = use_locale();

    let (ports, _) = use_store::<PortStore>();
    let (session, _) = use_store::<SessionStore>();
    let can_edit = session.can_edit();
    let port = use_loaded_port(locale, props.id, &ports);

    let Some(navigator) = use_navigator() else {
        return html!();
    };

    let navigator_cloned = navigator.clone();
    let cancel_onclick = Callback::from(move |_| {
        navigator_cloned.push(&Route::Ports);
    });

    let entry = use_state::<Result<Port, HashMap<String, String>>, _>(|| Err(Default::default()));
    let entry_cloned = entry.clone();
    let onchanged: Callback<Result<Port, HashMap<String, String>>> =
        Callback::from(move |updated| {
            entry_cloned.set(updated);
        });

    let is_loading = use_state(|| false);
    let onsubmit = submit_callback(locale, props.id, entry.clone(), is_loading, navigator);

    let port_entry = match &*port {
        Some(Ok(port_entry)) => port_entry,
        Some(Err(err)) => return failure_box(err),
        None => return html! {},
    };
    html! {
        <form {onsubmit} class="bg-white dark:bg-neutral-800 shadow-sm p-5 border border-neutral-300 dark:border-neutral-700 rounded-md">
            <fieldset disabled={!can_edit}>
                <PortConfig port={port_entry.port.clone()} {onchanged} />
            </fieldset>

            <div class="flex flex-col-reverse gap-2 mt-4 sm:flex-row sm:items-center sm:justify-end">
                <button type="button" onclick={cancel_onclick} class={button_class(ButtonKind::Secondary, ButtonSize::Md)}>
                    {locale.t(if can_edit { "common.cancel" } else { "common.back" })}
                </button>
                if can_edit {
                    <button type="submit" disabled={entry.is_err()} class={button_class(ButtonKind::Primary, ButtonSize::Md)}>
                        {locale.t("common.update")}
                    </button>
                }
            </div>
        </form>
    }
}

/// The port of the id: the entry of the store first, then the entry or the error that the
/// admin API returns. `None` while a port that is not in the store loads.
#[hook]
fn use_loaded_port(
    locale: Locale,
    id: ShortId,
    ports: &PortStore,
) -> UseStateHandle<Option<Result<PortEntry, String>>> {
    let port = use_state(|| ports.entries.iter().find(|e| e.id == id).cloned().map(Ok));
    let port_cloned = port.clone();
    use_effect_with((), move |_| {
        wasm_bindgen_futures::spawn_local(async move {
            let path = format!("{API_ENDPOINT}/ports/{id}");
            port_cloned.set(Some(fetch_json(locale, Request::get(&path)).await));
        });
    });
    port
}

/// Saves the port and returns to the port list. A submit during a save does nothing.
fn submit_callback(
    locale: Locale,
    id: ShortId,
    entry: UseStateHandle<Result<Port, HashMap<String, String>>>,
    is_loading: UseStateHandle<bool>,
    navigator: Navigator,
) -> Callback<SubmitEvent> {
    Callback::from(move |event: SubmitEvent| {
        event.prevent_default();
        if *is_loading {
            return;
        }
        let Ok(entry) = (*entry).clone() else {
            return;
        };
        let navigator = navigator.clone();
        let is_loading = is_loading.clone();
        is_loading.set(true);
        wasm_bindgen_futures::spawn_local(async move {
            match update_port(locale, id, &entry).await {
                Ok(()) => {
                    navigator.push(&Route::Ports);
                }
                Err(err) => {
                    crate::dialog::message(locale, &err, crate::dialog::Icon::Error).await;
                }
            }
            is_loading.set(false);
        });
    })
}

async fn update_port(locale: Locale, id: ShortId, entry: &Port) -> Result<(), String> {
    send_json(
        locale,
        Request::put(&format!("{API_ENDPOINT}/ports/{id}")),
        entry,
    )
    .await
}
