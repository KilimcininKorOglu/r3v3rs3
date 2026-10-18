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
use web_sys::HtmlInputElement;
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
    let search = use_state(String::new);
    let selected_category = use_state(|| Option::<String>::None);

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

    let tr = locale == Locale::Tr;
    let categories = unique_categories(&catalog.catalog.apps, tr);
    let query = search.trim().to_lowercase();
    let selected_cat = (*selected_category).as_deref();
    let filtered_apps: Vec<&StoreApp> = catalog
        .catalog
        .apps
        .iter()
        .filter(|app| app_matches(app, tr, &query, selected_cat))
        .collect();

    let on_search = on_search_input(&search);
    let onrefresh = refresh_callback(locale, &response, &notice, &loading);

    html! {
        <>
            <div class="flex flex-col sm:flex-row sm:items-center justify-between gap-3 mb-3">
                <h1 class="text-xl font-semibold">{locale.t("apps.store_title")}</h1>
                <div class="flex items-center gap-2">
                    <input
                        type="search"
                        class="px-3 py-1.5 rounded bg-neutral-800 border border-neutral-700 text-sm text-neutral-100 placeholder-neutral-500 focus:outline-none focus:border-neutral-500 w-full sm:w-64"
                        placeholder={locale.t("apps.store_search")}
                        value={(*search).clone()}
                        oninput={on_search}
                    />
                    <button type="button" class="px-3 py-1.5 rounded bg-neutral-700 text-neutral-100 hover:bg-neutral-600 disabled:opacity-50 text-sm shrink-0" disabled={*loading} onclick={onrefresh}>
                        {locale.t("apps.store_refresh")}
                    </button>
                </div>
            </div>
            { notice_view(&notice, |_: &()| html! { {locale.t("apps.store_refreshed")} }) }
            { category_filter(locale, &categories, &selected_category) }
            if filtered_apps.is_empty() {
                <div class="text-center py-12 text-neutral-400">
                    {locale.t("apps.store_no_results")}
                </div>
            } else {
                <div class="grid gap-4 md:grid-cols-2 xl:grid-cols-3">
                    { for filtered_apps.into_iter().map(|app| store_card(locale, app)) }
                </div>
            }
        </>
    }
}

fn on_search_input(search: &UseStateHandle<String>) -> Callback<InputEvent> {
    let search = search.clone();
    Callback::from(move |e: InputEvent| {
        let input: HtmlInputElement = e.target_unchecked_into();
        search.set(input.value());
    })
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

fn category_filter(
    locale: Locale,
    categories: &[String],
    selected: &UseStateHandle<Option<String>>,
) -> Html {
    let on_all = {
        let selected = selected.clone();
        Callback::from(move |_| selected.set(None))
    };
    let all_active = selected.is_none();
    let all_class = if all_active {
        "px-3 py-1 text-xs rounded-full font-medium bg-neutral-100 text-neutral-900 cursor-pointer"
    } else {
        "px-3 py-1 text-xs rounded-full font-medium bg-neutral-800 text-neutral-400 hover:bg-neutral-700 hover:text-neutral-200 cursor-pointer"
    };

    html! {
        <div class="flex flex-wrap items-center gap-1.5 mb-4">
            <button type="button" class={all_class} onclick={on_all}>
                {locale.t("apps.store_all_categories")}
            </button>
            { for categories.iter().map(|cat| category_pill(cat, selected)) }
        </div>
    }
}

fn category_pill(category: &str, selected: &UseStateHandle<Option<String>>) -> Html {
    let is_active = selected.as_deref() == Some(category);
    let class = if is_active {
        "px-3 py-1 text-xs rounded-full font-medium bg-neutral-100 text-neutral-900 cursor-pointer"
    } else {
        "px-3 py-1 text-xs rounded-full font-medium bg-neutral-800 text-neutral-400 hover:bg-neutral-700 hover:text-neutral-200 cursor-pointer"
    };
    let onclick = {
        let selected = selected.clone();
        let cat = category.to_string();
        Callback::from(move |_| {
            if is_active {
                selected.set(None);
            } else {
                selected.set(Some(cat.clone()));
            }
        })
    };
    html! {
        <button type="button" class={class} {onclick}>
            {category}
        </button>
    }
}

pub(crate) fn unique_categories(apps: &[StoreApp], tr: bool) -> Vec<String> {
    let mut categories: Vec<String> = apps
        .iter()
        .filter_map(|app| app.category_in(tr).map(str::to_string))
        .collect();
    categories.sort_unstable();
    categories.dedup();
    categories
}

fn matches_category(app: &StoreApp, tr: bool, selected: Option<&str>) -> bool {
    let Some(selected) = selected else {
        return true;
    };
    app.category_in(tr) == Some(selected)
}

fn matches_query(app: &StoreApp, tr: bool, query: &str) -> bool {
    if query.is_empty() {
        return true;
    }
    if app.name.to_lowercase().contains(query) {
        return true;
    }
    if app.description_in(tr).to_lowercase().contains(query) {
        return true;
    }
    if app
        .category_in(tr)
        .is_some_and(|cat| cat.to_lowercase().contains(query))
    {
        return true;
    }
    app.image
        .as_deref()
        .is_some_and(|img| img.to_lowercase().contains(query))
}

pub(crate) fn app_matches(app: &StoreApp, tr: bool, query: &str, category: Option<&str>) -> bool {
    matches_category(app, tr, category) && matches_query(app, tr, query)
}

fn store_card(locale: Locale, app: &StoreApp) -> Html {
    let route = Route::NewStoreApp {
        store: app.id.clone(),
    };
    let website = app.website.clone();
    let tr = locale == Locale::Tr;
    let category = app.category_in(tr);
    let screenshot = app.screenshot.clone();

    html! {
        <div class="flex flex-col rounded bg-neutral-800 shadow p-4 gap-2 border border-neutral-700/50 hover:border-neutral-600 transition-colors">
            if let Some(src) = screenshot {
                <div class="overflow-hidden rounded bg-neutral-900 border border-neutral-700/40 mb-1 h-36 flex items-center justify-center">
                    <img src={src} alt={app.name.clone()} class="w-full h-full object-cover object-top hover:scale-105 transition-transform duration-200" loading="lazy" />
                </div>
            }
            <div class="flex items-start justify-between gap-2">
                <div class="flex items-center gap-2 flex-wrap">
                    <h2 class="font-semibold text-neutral-100">{app.name.clone()}</h2>
                    if let Some(cat) = category {
                        <span class="text-xs px-2 py-0.5 rounded-full bg-neutral-700 text-neutral-300 font-medium">
                            {cat}
                        </span>
                    }
                </div>
                if let Some(url) = website {
                    <a href={url} target="_blank" rel="noopener noreferrer" class="text-sm text-neutral-400 hover:text-neutral-200 shrink-0">
                        {locale.t("apps.store_website")}
                    </a>
                }
            </div>
            <p class="text-sm text-neutral-300 grow line-clamp-3">{app.description_in(tr)}</p>
            <p class="text-xs text-neutral-500 font-mono truncate">{app.image.clone()}</p>
            <div class="pt-2">
                <Link<Route> to={route} classes="inline-block px-3 py-1.5 rounded bg-neutral-100 text-neutral-900 font-medium hover:bg-white text-sm">
                    {locale.t("apps.store_install")}
                </Link<Route>>
            </div>
        </div>
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn filtering_by_category_and_search() {
        let app1 = StoreApp {
            id: "uptime-kuma".into(),
            name: "Uptime Kuma".into(),
            description: "Self-hosted monitoring".into(),
            category: Some("Monitoring".into()),
            category_tr: Some("İzleme".into()),
            image: Some("louislam/uptime-kuma:1".into()),
            ..StoreApp::default()
        };
        let app2 = StoreApp {
            id: "vaultwarden".into(),
            name: "Vaultwarden".into(),
            description: "Password manager".into(),
            category: Some("Security".into()),
            category_tr: Some("Güvenlik".into()),
            image: Some("vaultwarden/server".into()),
            ..StoreApp::default()
        };

        let apps = vec![app1.clone(), app2.clone()];
        let cats_en = unique_categories(&apps, false);
        assert_eq!(cats_en, vec!["Monitoring", "Security"]);
        let cats_tr = unique_categories(&apps, true);
        assert_eq!(cats_tr, vec!["Güvenlik", "İzleme"]);

        assert!(app_matches(&app1, false, "", None));
        assert!(app_matches(&app1, false, "uptime", None));
        assert!(app_matches(&app1, false, "monitoring", None));
        assert!(!app_matches(&app1, false, "vault", None));

        assert!(app_matches(&app1, false, "", Some("Monitoring")));
        assert!(!app_matches(&app1, false, "", Some("Security")));

        assert!(app_matches(&app1, true, "", Some("İzleme")));
        assert!(!app_matches(&app1, true, "", Some("Güvenlik")));
    }
}
