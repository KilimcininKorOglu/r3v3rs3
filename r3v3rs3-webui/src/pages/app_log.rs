//! The container log of the running deployment of an app.

use super::app_deployments::back_header;
use super::app_list::SLOW_REFRESH_MS;
use super::settings::{failure_box, fetch_json};
use crate::API_ENDPOINT;
use crate::auth::use_ensure_auth;
use crate::i18n::use_locale;
use crate::store::SessionStore;
use gloo_net::http::Request;
use gloo_timers::callback::Interval;
use r3v3rs3_api::i18n::Locale;
use r3v3rs3_api::id::ShortId;
use r3v3rs3_api::platform::{AppEntry, AppLog};
use wasm_bindgen_futures::spawn_local;
use yew::prelude::*;
use yew_router::prelude::*;
use yewdux::prelude::*;

/// The number of log lines that the page shows.
const TAIL: u32 = 200;

/// The name of the app with its log.
#[derive(Clone, PartialEq)]
struct LogPage {
    name: String,
    log: AppLog,
}

type Loaded = Option<Result<LogPage, String>>;

#[derive(Properties, PartialEq)]
pub struct Props {
    pub id: ShortId,
}

#[function_component(AppLogView)]
pub fn app_log_view(props: &Props) -> Html {
    use_ensure_auth();
    let locale = use_locale();
    let (session, _) = use_store::<SessionStore>();
    let navigator = use_navigator().unwrap();
    let page = use_state(|| Loaded::None);
    let pre_ref = use_node_ref();

    // The admin API answers 401 until the session loads, so the page waits for it.
    let signed_in = session.info.is_some();
    let (id, page_cloned) = (props.id, page.clone());
    use_effect_with(signed_in, move |&signed_in| {
        let interval = signed_in.then(|| {
            let load = move || load_page(locale, id, page_cloned.clone());
            load();
            Interval::new(SLOW_REFRESH_MS, load)
        });
        move || drop(interval)
    });

    // Every refresh shows the newest lines at the bottom.
    let pre_cloned = pre_ref.clone();
    use_effect_with((*page).clone(), move |_| {
        if let Some(pre) = pre_cloned.cast::<web_sys::Element>() {
            pre.set_scroll_top(pre.scroll_height());
        }
    });

    let refresh = {
        let page = page.clone();
        Callback::from(move |_: MouseEvent| load_page(locale, id, page.clone()))
    };
    let title = match &*page {
        Some(Ok(page)) => locale.tf("apps.log_title", &[("name", &page.name)]),
        _ => String::new(),
    };
    html! {
        <>
            { back_header(locale, &navigator, title) }
            { log_view(locale, &page, pre_ref, refresh) }
        </>
    }
}

fn log_view(
    locale: Locale,
    page: &Loaded,
    pre_ref: NodeRef,
    refresh: Callback<MouseEvent>,
) -> Html {
    match page {
        None => html! {},
        Some(Err(err)) => failure_box(err),
        Some(Ok(page)) if !page.log.running => html! {
            <p class="my-8 text-lg font-bold text-neutral-500 dark:text-neutral-300 text-center">{locale.t("apps.log_not_running")}</p>
        },
        Some(Ok(page)) => html! {
            <>
                <pre ref={pre_ref} class="overflow-auto max-h-[70vh] p-4 font-mono text-xs sm:text-sm whitespace-pre-wrap break-words bg-white dark:bg-neutral-800 shadow-sm border border-neutral-300 dark:border-neutral-700 rounded-md">{page.log.log.clone()}</pre>
                <div class="flex items-center justify-between mt-2 text-sm text-neutral-500 dark:text-neutral-400">
                    <span>{locale.tf("apps.log_hint", &[("lines", &TAIL.to_string())])}</span>
                    <button onclick={refresh} type="button" class="font-medium text-blue-600 dark:text-blue-400 hover:underline">{locale.t("apps.log_refresh")}</button>
                </div>
            </>
        },
    }
}

fn load_page(locale: Locale, id: ShortId, page: UseStateHandle<Loaded>) {
    spawn_local(async move {
        page.set(Some(fetch_page(locale, id).await));
    });
}

async fn fetch_page(locale: Locale, id: ShortId) -> Result<LogPage, String> {
    let app: AppEntry =
        fetch_json(locale, Request::get(&format!("{API_ENDPOINT}/apps/{id}"))).await?;
    let path = format!("{API_ENDPOINT}/apps/{id}/logs?tail={TAIL}");
    let log = fetch_json(locale, Request::get(&path)).await?;
    Ok(LogPage {
        name: app.name.to_string(),
        log,
    })
}
