use crate::components::button::{ButtonKind, ButtonSize, button_class};
use crate::pages::settings::{failure_box, fetch_json};
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
    i18n::Locale,
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
    let site = use_loaded_proxy(locale, props.id, &proxies);

    let Some(navigator) = use_navigator() else {
        return html!();
    };

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
    let onsubmit = submit_callback(locale, props.id, entry.clone(), is_loading, navigator);

    let proxy_entry = match &*site {
        Some(Ok(proxy_entry)) => proxy_entry,
        Some(Err(err)) => return failure_box(err),
        None => return html! {},
    };
    let read_only = proxy_entry.is_discovered() || !can_edit;
    html! {
        <form {onsubmit} class="bg-white dark:bg-neutral-800 shadow-sm p-5 border border-neutral-300 dark:border-neutral-700 rounded-md">
            if let Some(source) = &proxy_entry.source {
                <p class="mb-4 p-3 text-sm text-blue-800 dark:text-blue-300 bg-blue-50 dark:bg-neutral-900 border border-blue-200 dark:border-blue-900 rounded-md">
                    {locale.tf("proxies.read_only_notice", &[("provider", source.provider.name()), ("resource", &source.resource)])}
                </p>
            }
            <fieldset disabled={read_only}>
                <ProxyConfig proxy={proxy_entry.proxy.clone()} {onchanged} />
            </fieldset>

            <div class="flex flex-col-reverse gap-2 mt-4 sm:flex-row sm:items-center sm:justify-end">
                <Link<Route> to={Route::Traffic} classes={button_class(ButtonKind::Secondary, ButtonSize::Md)}>
                    {locale.t("proxies.view_traffic")}
                </Link<Route>>
                <button type="button" onclick={cancel_onclick} class={button_class(ButtonKind::Secondary, ButtonSize::Md)}>
                    {locale.t(if read_only { "common.back" } else { "common.cancel" })}
                </button>
                if !read_only {
                    <button type="submit" disabled={entry.is_err()} class={button_class(ButtonKind::Primary, ButtonSize::Md)}>
                        {locale.t("common.update")}
                    </button>
                }
            </div>
        </form>
    }
}

/// The proxy of the id: the entry of the store first, then the entry or the error that the
/// admin API returns. `None` while a proxy that is not in the store loads.
#[hook]
fn use_loaded_proxy(
    locale: Locale,
    id: ShortId,
    proxies: &ProxyStore,
) -> UseStateHandle<Option<Result<ProxyEntry, String>>> {
    let site = use_state(|| proxies.entries.iter().find(|e| e.id == id).cloned().map(Ok));
    let site_cloned = site.clone();
    use_effect_with((), move |_| {
        wasm_bindgen_futures::spawn_local(async move {
            let path = format!("{API_ENDPOINT}/proxies/{id}");
            site_cloned.set(Some(fetch_json(locale, Request::get(&path)).await));
        });
    });
    site
}

/// Saves the proxy and returns to the proxy list. A submit during a save does nothing.
fn submit_callback(
    locale: Locale,
    id: ShortId,
    entry: UseStateHandle<Result<Proxy, HashMap<String, String>>>,
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
            match update_site(locale, id, &entry).await {
                Ok(()) => {
                    navigator.push(&Route::Proxies);
                }
                Err(err) => {
                    crate::dialog::message(locale, &err, crate::dialog::Icon::Error).await;
                }
            }
            is_loading.set(false);
        });
    })
}

async fn update_site(locale: Locale, id: ShortId, entry: &Proxy) -> Result<(), String> {
    let req = Request::put(&format!("{API_ENDPOINT}/proxies/{id}"))
        .json(entry)
        .map_err(|err| err.to_string())?;
    crate::pages::settings::send_request(locale, req).await?;
    Ok(())
}
