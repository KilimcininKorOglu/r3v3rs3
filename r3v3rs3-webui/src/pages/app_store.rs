//! The app store: the catalog of the apps that install in one click.

use super::Route;
use super::resource_page::{Notice, notice_view};
use super::settings::fetch_json;
use crate::API_ENDPOINT;
use crate::auth::use_ensure_auth;
use crate::i18n::use_locale;
use crate::store::SessionStore;
use gloo_net::http::Request;
use r3v3rs3_api::i18n::Locale;
use r3v3rs3_api::store::{StoreApp, StoreResponse};
use wasm_bindgen_futures::spawn_local;
use yew::prelude::*;
use yew_router::prelude::*;
use yewdux::prelude::*;

type StoreNotice = UseStateHandle<Option<Notice<()>>>;

#[function_component(AppStore)]
pub fn app_store() -> Html {
    use_ensure_auth();
    let locale = use_locale();
    let (session, _) = use_store::<SessionStore>();
    let response = use_state(|| Option::<StoreResponse>::None);
    let notice: StoreNotice = use_state(|| None);
    let loading = use_state(|| false);

    // The admin API answers 401 until the session loads, so the page waits for it.
    let signed_in = session.info.is_some();
    {
        let response = response.clone();
        let notice = notice.clone();
        let load_locale = locale;
        use_effect_with(signed_in, move |&signed_in| {
            if signed_in {
                let (response, notice) = (response.clone(), notice.clone());
                spawn_local(async move {
                    match fetch_json::<StoreResponse>(
                        load_locale,
                        Request::get(&format!("{API_ENDPOINT}/store")),
                    )
                    .await
                    {
                        Ok(catalog) => response.set(Some(catalog)),
                        Err(message) => notice.set(Some(Notice::Failed(message))),
                    }
                });
            }
        });
    }

    if session.info.is_none() {
        return html!();
    }
    let Some(catalog) = (*response).clone() else {
        return html! { { notice_view(&notice, |_: &()| html! {}) } };
    };
    let onrefresh = refresh_callback(locale, &response, &notice, &loading);
    html! {
        <>
            <div class="flex items-center justify-between mb-4">
                <h1 class="text-xl font-semibold">{locale.t("apps.store_title")}</h1>
                <button type="button" class="px-3 py-1.5 rounded bg-neutral-700 text-neutral-100 hover:bg-neutral-600 disabled:opacity-50" disabled={*loading} onclick={onrefresh}>
                    {locale.t("apps.store_refresh")}
                </button>
            </div>
            { notice_view(&notice, |_: &()| html! {}) }
            <div class="grid gap-4 md:grid-cols-2 xl:grid-cols-3">
                { for catalog.catalog.apps.iter().map(|app| store_card(locale, app)) }
            </div>
        </>
    }
}

fn refresh_callback(
    locale: Locale,
    response: &UseStateHandle<Option<StoreResponse>>,
    notice: &StoreNotice,
    loading: &UseStateHandle<bool>,
) -> Callback<MouseEvent> {
    let (response, notice, loading) = (response.clone(), notice.clone(), loading.clone());
    Callback::from(move |_| {
        if *loading {
            return;
        }
        loading.set(true);
        let request = Request::post(&format!("{API_ENDPOINT}/store/refresh"));
        let (response, notice, loading) = (response.clone(), notice.clone(), loading.clone());
        spawn_local(async move {
            loading.set(false);
            match fetch_json::<StoreResponse>(locale, request).await {
                Ok(updated) => {
                    response.set(Some(updated));
                    notice.set(Some(Notice::Saved(())));
                }
                Err(message) => notice.set(Some(Notice::Failed(message))),
            }
        });
    })
}

fn store_card(locale: Locale, app: &StoreApp) -> Html {
    let route = Route::NewStoreApp {
        store: app.id.clone(),
    };
    let website = app.website.clone();
    let tr = locale == Locale::Tr;
    html! {
        <div class="flex flex-col rounded bg-neutral-800 shadow p-4 gap-2">
            <div class="flex items-start justify-between gap-2">
                <h2 class="font-semibold">{app.name.clone()}</h2>
                if let Some(url) = website {
                    <a href={url} target="_blank" rel="noopener noreferrer" class="text-sm text-neutral-400 hover:text-neutral-200">
                        {locale.t("apps.store_website")}
                    </a>
                }
            </div>
            <p class="text-sm text-neutral-300 grow">{app.description_in(tr)}</p>
            <p class="text-xs text-neutral-500">{app.image.clone()}</p>
            <div>
                <Link<Route> to={route} classes="inline-block px-3 py-1.5 rounded bg-neutral-100 text-neutral-900 font-medium hover:bg-white">
                    {locale.t("apps.store_install")}
                </Link<Route>>
            </div>
        </div>
    }
}
