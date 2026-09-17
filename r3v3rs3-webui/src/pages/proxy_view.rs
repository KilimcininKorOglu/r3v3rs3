use crate::{
    API_ENDPOINT,
    auth::use_ensure_auth,
    components::proxy_config::ProxyConfig,
    i18n::use_locale,
    pages::Route,
    store::{ProxyStore, SessionStore},
};
use gloo_net::http::Request;
use r3v3rs3_api::{
    id::ShortId,
    proxy::{Proxy, ProxyEntry},
};
use std::collections::HashMap;
use yew::prelude::*;
use yew_router::prelude::*;
use yewdux::prelude::*;

#[derive(Properties, PartialEq)]
pub struct Props {
    pub id: ShortId,
}

#[function_component(ProxyView)]
pub fn proxy_view(props: &Props) -> Html {
    use_ensure_auth();
    let locale = use_locale();

    let (proxies, _) = use_store::<ProxyStore>();
    let (session, _) = use_store::<SessionStore>();
    let can_edit = session.can_edit_proxies();
    let site = use_state(|| proxies.entries.iter().find(|e| e.id == props.id).cloned());
    let id = props.id;
    let proxy_cloned = site.clone();
    use_effect_with((), move |_| {
        wasm_bindgen_futures::spawn_local(async move {
            if let Ok(entry) = get_site(id).await {
                proxy_cloned.set(Some(entry));
            }
        });
    });

    let navigator = use_navigator().unwrap();

    let navigator_cloned = navigator.clone();
    let cancel_onclick = Callback::from(move |_| {
        navigator_cloned.push(&Route::Proxies);
    });

    let entry = use_state::<Result<Proxy, HashMap<String, String>>, _>(|| Err(Default::default()));
    let entry_cloned = entry.clone();
    let onchanged: Callback<Result<Proxy, HashMap<String, String>>> =
        Callback::from(move |updated| {
            entry_cloned.set(updated);
        });

    let is_loading = use_state(|| false);

    let id = props.id;
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
                if update_site(id, &entry).await.is_ok() {
                    navigator.push(&Route::Proxies);
                }
                is_loading_cloned.set(false);
            });
        }
    });

    html! {
        <>
            if let Some(proxy_entry) = &*site {
                <form {onsubmit} class="bg-white dark:bg-neutral-800 shadow-sm p-5 border border-neutral-300 dark:border-neutral-700 rounded-md">
                    if let Some(source) = &proxy_entry.source {
                        <p class="mb-4 p-3 text-sm text-blue-800 dark:text-blue-300 bg-blue-50 dark:bg-neutral-900 border border-blue-200 dark:border-blue-900 rounded-md">
                            {locale.tf("proxies.read_only_notice", &[("provider", source.provider.name()), ("resource", &source.resource)])}
                        </p>
                    }
                    <fieldset disabled={proxy_entry.is_discovered() || !can_edit}>
                        <ProxyConfig proxy={proxy_entry.proxy.clone()} {onchanged} />
                    </fieldset>

                    <div class="flex flex-col-reverse gap-2 mt-4 sm:flex-row sm:items-center sm:justify-end">
                        <button type="button" onclick={cancel_onclick} class="inline-flex justify-center items-center text-neutral-500 bg-neutral-50 dark:text-neutral-200 dark:bg-neutral-800 focus:outline-none hover:bg-neutral-100 hover:dark:bg-neutral-900 focus:ring-4 focus:ring-neutral-200 dark:focus:ring-neutral-600 font-medium rounded-lg text-sm px-4 py-2">
                            {locale.t(if proxy_entry.is_discovered() || !can_edit { "common.back" } else { "common.cancel" })}
                        </button>
                        if !proxy_entry.is_discovered() && can_edit {
                            <button type="submit" disabled={entry.is_err()} class="inline-flex justify-center items-center text-neutral-500 bg-neutral-50 dark:text-neutral-200 dark:bg-neutral-800 border border-neutral-300 dark:border-neutral-600 focus:outline-none hover:bg-neutral-100 hover:dark:bg-neutral-900 focus:ring-4 focus:ring-neutral-200 dark:focus:ring-neutral-600 font-medium rounded-lg text-sm px-4 py-2">
                                {locale.t("common.update")}
                            </button>
                        }
                    </div>
                </form>
            } else {
                <Redirect<Route> to={Route::Proxies}/>
            }
        </>
    }
}

async fn get_site(id: ShortId) -> Result<ProxyEntry, gloo_net::Error> {
    Request::get(&format!("{API_ENDPOINT}/proxies/{id}"))
        .send()
        .await?
        .json()
        .await
}

async fn update_site(id: ShortId, entry: &Proxy) -> Result<(), gloo_net::Error> {
    Request::put(&format!("{API_ENDPOINT}/proxies/{id}"))
        .json(entry)?
        .send()
        .await?
        .json()
        .await
}
