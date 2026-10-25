//! The container log of the running deployment of an app.

use super::app_deployments::back_header;
use super::app_list::SLOW_REFRESH_MS;
use super::settings::{failure_box, fetch_json};
use crate::API_ENDPOINT;
use crate::auth::use_ensure_auth;
use crate::dialog::{self, Icon};
use crate::format::format_bytes;
use crate::i18n::use_locale;
use crate::store::SessionStore;
use gloo_net::http::Request;
use gloo_timers::callback::Interval;
use r3v3rs3_api::i18n::Locale;
use r3v3rs3_api::id::ShortId;
use r3v3rs3_api::platform::{AppEntry, AppLog, ComposeServiceStatus};
use wasm_bindgen::JsCast;
use wasm_bindgen::prelude::*;
use wasm_bindgen_futures::spawn_local;
use web_sys::{HtmlInputElement, HtmlSelectElement};
use yew::prelude::*;
use yew_router::prelude::*;
use yewdux::prelude::*;

/// The number of initial log lines that the page loads.
const TAIL: u32 = 200;

/// The maximum number of log lines kept in memory during live streaming.
const MAX_BUFFER_LINES: usize = 5000;

/// The name of the app with its log and available compose services.
#[derive(Clone, PartialEq)]
struct LogPage {
    name: String,
    log: AppLog,
    services: Vec<String>,
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
    let Some(navigator) = use_navigator() else {
        return html!();
    };
    let page = use_state(|| Loaded::None);
    let live = use_state(|| true);
    let auto_scroll = use_state(|| true);
    let search = use_state(String::new);
    let selected_service = use_state(|| Option::<String>::None);
    let pre_ref = use_node_ref();

    let signed_in = session.info.is_some();
    let id = props.id;

    {
        let page = page.clone();
        let service = (*selected_service).clone();
        use_effect_with((signed_in, service), move |&(signed_in, ref service)| {
            if signed_in {
                load_page(locale, id, service.clone(), page);
            }
        });
    }

    use_log_stream_effect(id, signed_in, &live, &page, locale, &selected_service);
    use_auto_scroll_effect(&pre_ref, &auto_scroll, &page);

    let refresh = {
        let page = page.clone();
        let service = (*selected_service).clone();
        Callback::from(move |_: MouseEvent| load_page(locale, id, service.clone(), page.clone()))
    };

    let title = page_title(locale, &page);
    let content_props = ContentProps {
        id,
        locale,
        page: &page,
        pre_ref,
        live: &live,
        auto_scroll: &auto_scroll,
        search: &search,
        selected_service: &selected_service,
        refresh,
    };

    html! {
        <>
            if let Some(Ok(p)) = (*page).as_ref() {
                <crate::components::app_tabs::AppTabs
                    id={id}
                    name={p.name.clone()}
                    active={crate::components::app_tabs::AppTab::Logs}
                />
            } else {
                { back_header(locale, &navigator, title) }
            }
            { content_view(content_props) }
        </>
    }
}

fn page_title(locale: Locale, page: &Loaded) -> String {
    match page {
        Some(Ok(p)) => locale.tf("apps.log_title", &[("name", &p.name)]),
        _ => String::new(),
    }
}

#[hook]
fn use_auto_scroll_effect(
    pre_ref: &NodeRef,
    auto_scroll: &UseStateHandle<bool>,
    page: &UseStateHandle<Loaded>,
) {
    let pre_ref = pre_ref.clone();
    let auto_scroll = auto_scroll.clone();
    use_effect_with((*page).clone(), move |_| {
        if *auto_scroll && let Some(pre) = pre_ref.cast::<web_sys::Element>() {
            pre.set_scroll_top(pre.scroll_height());
        }
    });
}

#[hook]
fn use_log_stream_effect(
    id: ShortId,
    signed_in: bool,
    live: &UseStateHandle<bool>,
    page: &UseStateHandle<Loaded>,
    locale: Locale,
    selected_service: &UseStateHandle<Option<String>>,
) {
    let (page, live) = (page.clone(), live.clone());
    let service = (**selected_service).clone();
    use_effect_with(
        (*live, signed_in, service),
        move |&(live_active, signed_in, ref svc)| {
            let (mut ws_cleanup, mut interval_cleanup) = (None, None);
            if signed_in {
                if live_active {
                    ws_cleanup = start_live_tail(id, page.clone(), svc.as_deref());
                } else {
                    let p = page.clone();
                    let s = svc.clone();
                    interval_cleanup = Some(Interval::new(SLOW_REFRESH_MS, move || {
                        load_page(locale, id, s.clone(), p.clone())
                    }));
                }
            }
            move || {
                if let Some(socket) = ws_cleanup {
                    let _ = socket.close();
                }
                drop(interval_cleanup);
            }
        },
    );
}

struct ContentProps<'a> {
    id: ShortId,
    locale: Locale,
    page: &'a Loaded,
    pre_ref: NodeRef,
    live: &'a UseStateHandle<bool>,
    auto_scroll: &'a UseStateHandle<bool>,
    search: &'a UseStateHandle<String>,
    selected_service: &'a UseStateHandle<Option<String>>,
    refresh: Callback<MouseEvent>,
}

fn content_view(props: ContentProps<'_>) -> Html {
    match props.page {
        None => html! {},
        Some(Err(err)) => failure_box(err),
        Some(Ok(page)) if !page.log.running => html! {
            <p class="my-8 text-lg font-bold text-neutral-500 dark:text-neutral-300 text-center">
                {props.locale.t("apps.log_not_running")}
            </p>
        },
        Some(Ok(page)) => log_box_view(page, props),
    }
}

fn log_box_view(page: &LogPage, props: ContentProps<'_>) -> Html {
    let ContentProps {
        locale,
        pre_ref,
        live,
        auto_scroll,
        search,
        selected_service,
        refresh,
        ..
    } = props;
    let (filtered, matched, total) = filter_log_lines(&page.log.log, search);
    let app_name = page.name.clone();
    let log_text = page.log.log.clone();
    let service_name = (**selected_service).clone();
    let on_download = Callback::from(move |_: MouseEvent| {
        let filename = download_filename(&app_name, service_name.as_deref());
        download_text_file(&filename, &log_text);
    });
    let app_id = props.id;
    let on_vacuum = {
        let refresh = refresh.clone();
        Callback::from(move |_: MouseEvent| {
            let refresh = refresh.clone();
            dialog::confirm_then(
                locale,
                locale.t("apps.log_vacuum_confirm").to_string(),
                async move {
                    let req = Request::post(&format!("{API_ENDPOINT}/apps/{app_id}/logs/vacuum"));
                    if let Ok(res) =
                        fetch_json::<r3v3rs3_api::container::LogVacuumResult>(locale, req).await
                    {
                        let freed = format_bytes(res.freed_bytes);
                        dialog::message(
                            locale,
                            &locale.tf("apps.log_vacuum_success", &[("size", &freed)]),
                            Icon::Success,
                        )
                        .await;
                        refresh.emit(MouseEvent::new("click").unwrap());
                    }
                },
            );
        })
    };

    let toolbar_props = ToolbarProps {
        locale,
        live,
        auto_scroll,
        search,
        selected_service,
        services: &page.services,
        on_download,
        on_vacuum,
        refresh,
    };

    html! {
        <>
            { toolbar_view(toolbar_props) }
            if !search.is_empty() {
                <p class="text-xs text-neutral-500 dark:text-neutral-400 my-1">
                    {locale.tf("apps.log_matches", &[("matched", &matched.to_string()), ("total", &total.to_string())])}
                </p>
            }
            <pre ref={pre_ref} class="overflow-auto max-h-[70vh] p-4 font-mono text-xs sm:text-sm whitespace-pre-wrap break-words bg-white dark:bg-neutral-800 shadow-sm border border-neutral-300 dark:border-neutral-700 rounded-md">
                { filtered }
            </pre>
            <div class="flex items-center justify-between mt-2 text-xs text-neutral-500 dark:text-neutral-400">
                <span>{locale.tf("apps.log_hint", &[("lines", &TAIL.to_string())])}</span>
            </div>
        </>
    }
}

struct ToolbarProps<'a> {
    locale: Locale,
    live: &'a UseStateHandle<bool>,
    auto_scroll: &'a UseStateHandle<bool>,
    search: &'a UseStateHandle<String>,
    selected_service: &'a UseStateHandle<Option<String>>,
    services: &'a [String],
    on_download: Callback<MouseEvent>,
    on_vacuum: Callback<MouseEvent>,
    refresh: Callback<MouseEvent>,
}

fn toolbar_view(props: ToolbarProps<'_>) -> Html {
    let ToolbarProps {
        locale,
        live,
        auto_scroll,
        search,
        selected_service,
        services,
        on_download,
        on_vacuum,
        refresh,
    } = props;

    let search_input = {
        let search = search.clone();
        Callback::from(move |e: InputEvent| {
            if let Some(input) = e.target_dyn_into::<HtmlInputElement>() {
                search.set(input.value());
            }
        })
    };
    let clear_search = {
        let search = search.clone();
        Callback::from(move |_: MouseEvent| search.set(String::new()))
    };
    let toggle_live = {
        let live = live.clone();
        Callback::from(move |_: MouseEvent| live.set(!*live))
    };
    let toggle_scroll = {
        let auto_scroll = auto_scroll.clone();
        Callback::from(move |_: Event| auto_scroll.set(!*auto_scroll))
    };
    let service_select = {
        let selected_service = selected_service.clone();
        Callback::from(move |e: Event| {
            if let Some(select) = e.target_dyn_into::<HtmlSelectElement>() {
                let val = select.value();
                if val.is_empty() {
                    selected_service.set(None);
                } else {
                    selected_service.set(Some(val));
                }
            }
        })
    };

    html! {
        <div class="flex flex-col sm:flex-row items-stretch sm:items-center justify-between gap-2 mb-2">
            <div class="flex items-center gap-2 flex-wrap flex-1">
                <div class="relative flex-1 max-w-xs">
                    <input
                        type="text"
                        placeholder={locale.t("apps.log_search")}
                        value={(**search).clone()}
                        oninput={search_input}
                        class="w-full text-xs sm:text-sm px-3 py-1.5 rounded-lg border border-neutral-300 dark:border-neutral-700 bg-white dark:bg-neutral-800 text-neutral-900 dark:text-neutral-100 placeholder-neutral-400 focus:outline-none focus:ring-2 focus:ring-blue-500"
                    />
                    if !search.is_empty() {
                        <button
                            onclick={clear_search}
                            type="button"
                            class="absolute right-2.5 top-1.5 text-xs text-neutral-400 hover:text-neutral-600 dark:hover:text-neutral-200"
                        >
                            {"✕"}
                        </button>
                    }
                </div>
                if !services.is_empty() {
                    <div class="flex items-center gap-1.5">
                        <label class="text-xs text-neutral-500 dark:text-neutral-400 font-medium whitespace-nowrap">
                            {locale.t("apps.service_select_label")}
                        </label>
                        <select
                            onchange={service_select}
                            class="text-xs sm:text-sm px-2.5 py-1.5 rounded-lg border border-neutral-300 dark:border-neutral-700 bg-white dark:bg-neutral-800 text-neutral-900 dark:text-neutral-100 focus:outline-none focus:ring-2 focus:ring-blue-500"
                        >
                            <option value="" selected={selected_service.is_none()}>{locale.t("apps.service_select_all")}</option>
                            { for services.iter().map(|s| html! {
                                <option value={s.clone()} selected={selected_service.as_deref() == Some(s.as_str())}>{s}</option>
                            }) }
                        </select>
                    </div>
                }
            </div>
            <div class="flex items-center gap-3 self-end sm:self-auto">
                <label class="inline-flex items-center gap-1.5 text-xs text-neutral-600 dark:text-neutral-300 cursor-pointer select-none">
                    <input
                        type="checkbox"
                        checked={**auto_scroll}
                        onchange={toggle_scroll}
                        class="rounded border-neutral-300 dark:border-neutral-700 text-blue-600 focus:ring-blue-500"
                    />
                    {locale.t("apps.log_auto_scroll")}
                </label>
                <button
                    onclick={toggle_live}
                    type="button"
                    class="inline-flex items-center gap-1.5 text-xs sm:text-sm px-3 py-1.5 rounded-lg border border-neutral-300 dark:border-neutral-700 bg-white dark:bg-neutral-800 hover:bg-neutral-50 dark:hover:bg-neutral-700 font-medium"
                >
                    if **live {
                        <span class="w-2 h-2 rounded-full bg-green-500 animate-pulse"></span>
                        {locale.t("apps.log_live")}
                    } else {
                        <span class="w-2 h-2 rounded-full bg-neutral-400"></span>
                        {locale.t("apps.log_paused")}
                    }
                </button>
                if !**live {
                    <button
                        onclick={refresh}
                        type="button"
                        class="text-xs sm:text-sm font-medium text-blue-600 dark:text-blue-400 hover:underline"
                    >
                        {locale.t("apps.log_refresh")}
                    </button>
                }
                <button
                    onclick={on_download}
                    type="button"
                    class="inline-flex items-center text-xs sm:text-sm px-3 py-1.5 rounded-lg border border-neutral-300 dark:border-neutral-700 bg-white dark:bg-neutral-800 hover:bg-neutral-50 dark:hover:bg-neutral-700 font-medium"
                >
                    <svg class="w-3.5 h-3.5 mr-1" fill="none" stroke="currentColor" viewBox="0 0 24 24">
                        <path stroke-linecap="round" stroke-linejoin="round" stroke-width="2" d="M4 16v1a3 3 0 003 3h10a3 3 0 003-3v-1m-4-4l-4 4m0 0l-4-4m4 4V4"></path>
                    </svg>
                    {locale.t("apps.log_download")}
                </button>
                <button
                    onclick={on_vacuum}
                    type="button"
                    class="inline-flex items-center text-xs sm:text-sm px-3 py-1.5 rounded-lg border border-red-300 dark:border-red-700 bg-red-50 dark:bg-red-950/30 text-red-700 dark:text-red-300 hover:bg-red-100 dark:hover:bg-red-900/40 font-medium"
                >
                    <svg class="w-3.5 h-3.5 mr-1" fill="none" stroke="currentColor" viewBox="0 0 24 24">
                        <path stroke-linecap="round" stroke-linejoin="round" stroke-width="2" d="M19 7l-.867 12.142A2 2 0 0116.138 21H7.862a2 2 0 01-1.995-1.858L5 7m5 4v6m4-6v6m1-10V4a1 1 0 00-1-1h-4a1 1 0 00-1 1v3M4 7h16"></path>
                    </svg>
                    {locale.t("apps.log_vacuum")}
                </button>
            </div>
        </div>
    }
}

fn start_live_tail(
    id: ShortId,
    page: UseStateHandle<Loaded>,
    service: Option<&str>,
) -> Option<web_sys::WebSocket> {
    let url = ws_logs_url(id, service)?;
    let socket = web_sys::WebSocket::new(&url).ok()?;
    let on_message = Closure::<dyn FnMut(web_sys::MessageEvent)>::wrap(Box::new(
        move |event: web_sys::MessageEvent| {
            if let Some(text) = event.data().as_string() {
                let current_page = (*page).clone();
                if let Some(Ok(mut p)) = current_page {
                    p.log.log.push_str(&text);
                    p.log.log = trim_log_buffer(&p.log.log, MAX_BUFFER_LINES);
                    page.set(Some(Ok(p)));
                }
            }
        },
    ));
    socket.set_onmessage(Some(on_message.as_ref().unchecked_ref()));
    on_message.forget();
    Some(socket)
}

fn ws_logs_url(id: ShortId, service: Option<&str>) -> Option<String> {
    let window = web_sys::window()?;
    let location = window.location();
    let protocol = if location.protocol().ok()? == "https:" {
        "wss"
    } else {
        "ws"
    };
    let host = location.host().ok()?;
    let service_param = match service {
        Some(s) if !s.is_empty() => format!("&service={s}"),
        _ => String::new(),
    };
    Some(format!(
        "{protocol}://{host}{API_ENDPOINT}/apps/{id}/logs/live?tail={TAIL}{service_param}"
    ))
}

pub fn download_filename(app_name: &str, service: Option<&str>) -> String {
    match service {
        Some(s) if !s.is_empty() => format!("{app_name}-{s}-logs.txt"),
        _ => format!("{app_name}-logs.txt"),
    }
}

pub fn download_text_file(filename: &str, content: &str) {
    let Some(window) = web_sys::window() else {
        return;
    };
    let Some(document) = window.document() else {
        return;
    };
    let array = js_sys::Array::new();
    array.push(&JsValue::from_str(content));
    let Ok(blob) = web_sys::Blob::new_with_str_sequence(&array) else {
        return;
    };
    let Ok(url) = web_sys::Url::create_object_url_with_blob(&blob) else {
        return;
    };
    let Ok(element) = document.create_element("a") else {
        return;
    };
    let _ = element.set_attribute("href", &url);
    let _ = element.set_attribute("download", filename);
    if let Ok(html_el) = element.dyn_into::<web_sys::HtmlElement>() {
        html_el.click();
    }
    let _ = web_sys::Url::revoke_object_url(&url);
}

pub fn trim_log_buffer(buffer: &str, max_lines: usize) -> String {
    let lines: Vec<&str> = buffer.lines().collect();
    if lines.len() > max_lines {
        lines[lines.len() - max_lines..].join("\n") + "\n"
    } else {
        buffer.to_string()
    }
}

pub fn filter_log_lines(buffer: &str, query: &str) -> (String, usize, usize) {
    let lines: Vec<&str> = buffer.lines().collect();
    let total = lines.len();
    if query.trim().is_empty() {
        return (buffer.to_string(), total, total);
    }
    let query_lower = query.trim().to_lowercase();
    let filtered: Vec<&str> = lines
        .into_iter()
        .filter(|line| line.to_lowercase().contains(&query_lower))
        .collect();
    let matched = filtered.len();
    (filtered.join("\n"), matched, total)
}

fn load_page(locale: Locale, id: ShortId, service: Option<String>, page: UseStateHandle<Loaded>) {
    spawn_local(async move {
        page.set(Some(fetch_page(locale, id, service.as_deref()).await));
    });
}

async fn fetch_page(locale: Locale, id: ShortId, service: Option<&str>) -> Result<LogPage, String> {
    let app: AppEntry =
        fetch_json(locale, Request::get(&format!("{API_ENDPOINT}/apps/{id}"))).await?;
    let mut services = Vec::new();
    if app.spec.source.is_compose()
        && let Ok(list) = fetch_json::<Vec<ComposeServiceStatus>>(
            locale,
            Request::get(&format!("{API_ENDPOINT}/apps/{id}/services")),
        )
        .await
    {
        services = list.into_iter().map(|s| s.service).collect();
    }
    let service_param = match service {
        Some(s) if !s.is_empty() => format!("&service={s}"),
        _ => String::new(),
    };
    let path = format!("{API_ENDPOINT}/apps/{id}/logs?tail={TAIL}{service_param}");
    let log = fetch_json(locale, Request::get(&path)).await?;
    Ok(LogPage {
        name: app.name.to_string(),
        log,
        services,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn filter_log_lines_empty_query_returns_all() {
        let text = "Line 1\nLine 2\nLine 3";
        let (filtered, matched, total) = filter_log_lines(text, "");
        assert_eq!(filtered, text);
        assert_eq!(matched, 3);
        assert_eq!(total, 3);
    }

    #[test]
    fn filter_log_lines_matches_case_insensitively() {
        let text = "INFO: starting server\nERROR: port in use\nINFO: ready";
        let (filtered, matched, total) = filter_log_lines(text, "error");
        assert_eq!(filtered, "ERROR: port in use");
        assert_eq!(matched, 1);
        assert_eq!(total, 3);
    }

    #[test]
    fn trim_log_buffer_caps_lines() {
        let text = "1\n2\n3\n4\n5";
        let trimmed = trim_log_buffer(text, 3);
        assert_eq!(trimmed, "3\n4\n5\n");
    }

    #[test]
    fn download_filename_formats_correctly() {
        assert_eq!(download_filename("my-app", None), "my-app-logs.txt");
        assert_eq!(download_filename("my-app", Some("")), "my-app-logs.txt");
        assert_eq!(
            download_filename("my-app", Some("web")),
            "my-app-web-logs.txt"
        );
        assert_eq!(
            download_filename("my-app", Some("db")),
            "my-app-db-logs.txt"
        );
    }
}
