use crate::{
    auth::use_ensure_auth, components::proxy_config::ProxyConfig, i18n::use_locale, pages::Route,
    API_ENDPOINT,
};
use gloo_net::http::Request;
use r3v3rs3_api::proxy::Proxy;
use std::collections::HashMap;
use yew::prelude::*;
use yew_router::prelude::*;

#[function_component(NewProxy)]
pub fn new_proxy() -> Html {
    use_ensure_auth();
    let locale = use_locale();

    let navigator = use_navigator().unwrap();

    let entry = use_state::<Result<Proxy, HashMap<String, String>>, _>(|| Err(Default::default()));
    let entry_cloned = entry.clone();
    let onchanged: Callback<Result<Proxy, HashMap<String, String>>> =
        Callback::from(move |updated| {
            entry_cloned.set(updated);
        });

    let navigator_cloned = navigator.clone();
    let cancel_onclick = Callback::from(move |_| {
        navigator_cloned.push(&Route::Proxies);
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
                if create_port(&entry).await.is_ok() {
                    navigator.push(&Route::Proxies);
                }
                is_loading_cloned.set(false);
            });
        }
    });

    html! {
        <>
            <form {onsubmit} class="bg-white dark:bg-neutral-800 shadow-sm p-5 border border-neutral-300 dark:border-neutral-700 rounded-md">
                <ProxyConfig {onchanged} />

                <div class="flex flex-col-reverse gap-2 mt-4 sm:flex-row sm:items-center sm:justify-end">
                    <button type="button" onclick={cancel_onclick} class="inline-flex justify-center items-center text-neutral-500 bg-neutral-50 dark:text-neutral-200 dark:bg-neutral-800 focus:outline-none hover:bg-neutral-100 hover:dark:bg-neutral-900 focus:ring-4 focus:ring-neutral-200 dark:focus:ring-neutral-600 font-medium rounded-lg text-sm px-4 py-2">
                        {locale.t("common.cancel")}
                    </button>
                    <button type="submit" disabled={entry.is_err()} class="inline-flex justify-center items-center text-neutral-500 bg-neutral-50 dark:text-neutral-200 dark:bg-neutral-800 border border-neutral-300 dark:border-neutral-600 focus:outline-none hover:bg-neutral-100 hover:dark:bg-neutral-900 focus:ring-4 focus:ring-neutral-200 dark:focus:ring-neutral-600 font-medium rounded-lg text-sm px-4 py-2">
                        {locale.t("common.create")}
                    </button>
                </div>
            </form>
        </>
    }
}

async fn create_port(entry: &Proxy) -> Result<(), gloo_net::Error> {
    Request::post(&format!("{API_ENDPOINT}/proxies"))
        .json(entry)?
        .send()
        .await?
        .json()
        .await
}
