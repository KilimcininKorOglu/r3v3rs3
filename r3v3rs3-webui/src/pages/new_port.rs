use crate::components::button::{ButtonKind, ButtonSize, button_class};
use crate::{
    API_ENDPOINT, auth::use_ensure_auth, components::port_config::PortConfig, i18n::use_locale,
    pages::Route,
};
use gloo_net::http::Request;
use r3v3rs3_api::{i18n::Locale, port::Port};
use std::collections::HashMap;
use yew::prelude::*;
use yew_router::prelude::*;

#[function_component(NewPort)]
pub fn new_port() -> Html {
    use_ensure_auth();
    let locale = use_locale();

    let Some(navigator) = use_navigator() else {
        return html!();
    };
    let entry = use_state::<Result<Port, HashMap<String, String>>, _>(|| Err(Default::default()));
    let entry_cloned = entry.clone();
    let onchanged: Callback<Result<Port, HashMap<String, String>>> =
        Callback::from(move |updated| {
            entry_cloned.set(updated);
        });

    let navigator_cloned = navigator.clone();
    let cancel_onclick = Callback::from(move |_| {
        navigator_cloned.push(&Route::Ports);
    });

    let is_loading = use_state(|| false);

    let entry_cloned = entry.clone();
    let is_loading_cloned = is_loading;
    let onsubmit = Callback::from(move |event: SubmitEvent| {
        event.prevent_default();
        if *is_loading_cloned {
            return;
        }
        let navigator = navigator.clone();
        let is_loading_cloned = is_loading_cloned.clone();
        if let Ok(entry) = (*entry_cloned).clone() {
            is_loading_cloned.set(true);
            wasm_bindgen_futures::spawn_local(async move {
                match create_port(locale, &entry).await {
                    Ok(()) => {
                        navigator.push(&Route::Ports);
                    }
                    Err(err) => {
                        crate::dialog::message(locale, &err, crate::dialog::Icon::Error).await;
                    }
                }
                is_loading_cloned.set(false);
            });
        }
    });

    html! {
        <>
            <form {onsubmit} class="bg-white dark:bg-neutral-800 shadow-sm p-5 border border-neutral-300 dark:border-neutral-700 rounded-md">
                <PortConfig {onchanged} />

                <div class="flex flex-col-reverse gap-2 mt-4 sm:flex-row sm:items-center sm:justify-end">
                    <button type="button" onclick={cancel_onclick} class={button_class(ButtonKind::Secondary, ButtonSize::Md)}>
                        {locale.t("common.cancel")}
                    </button>
                    <button type="submit" disabled={entry.is_err()} class={button_class(ButtonKind::Primary, ButtonSize::Md)}>
                        {locale.t("common.create")}
                    </button>
                </div>
            </form>
        </>
    }
}

async fn create_port(locale: Locale, entry: &Port) -> Result<(), String> {
    crate::pages::settings::send_json(
        locale,
        Request::post(&format!("{API_ENDPOINT}/ports")),
        entry,
    )
    .await
}
