//! Live Request Inspector and Traffic Replay Console component.

use crate::API_ENDPOINT;
use crate::components::http_proxy_config::{HINT_CLASS, INPUT_CLASS};
use crate::format::format_instant;
use crate::i18n::use_locale;
use crate::pages::traffic::{
    format_latency, latency_class, method_badge_class, status_badge_class,
};
use gloo_net::http::Request;
use r3v3rs3_api::i18n::Locale;
use r3v3rs3_api::traffic::{
    AccessLogEntry, HeaderEntry, RequestReplayRequest, RequestReplayResponse,
};
use wasm_bindgen::{JsCast, JsValue};
use wasm_bindgen_futures::spawn_local;
use web_sys::HtmlInputElement;
use yew::prelude::*;

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum InspectorTab {
    Overview,
    Headers,
    Payload,
    Replay,
}

#[derive(Properties, PartialEq)]
pub struct Props {
    pub entry: AccessLogEntry,
    pub on_close: Callback<()>,
}

#[function_component(TrafficInspector)]
pub fn traffic_inspector(props: &Props) -> Html {
    let locale = use_locale();
    let tab = use_state(|| InspectorTab::Overview);
    let raw_mode = use_state(|| false);
    let copied = use_state(|| false);
    let default_url = format!("http://{}{}", props.entry.host, props.entry.uri);
    let target_url = use_state(|| default_url);
    let replay_loading = use_state(|| false);
    let replay_result = use_state(|| Option::<RequestReplayResponse>::None);

    let on_close = props.on_close.clone();
    let on_backdrop = {
        let on_close = on_close.clone();
        Callback::from(move |_| on_close.emit(()))
    };
    let on_close_btn = {
        let on_close = on_close.clone();
        Callback::from(move |_| on_close.emit(()))
    };

    let on_copy_curl = create_copy_curl_callback(&props.entry, &target_url, &copied);
    let on_replay =
        create_replay_callback(&props.entry, &target_url, &replay_loading, &replay_result);

    let replay_args = ReplayTabArgs {
        target_url: &target_url,
        copied: *copied,
        replay_loading: *replay_loading,
        replay_result: replay_result.as_ref(),
        on_copy_curl,
        on_replay,
        on_url_change: {
            let target_url = target_url.clone();
            Callback::from(move |e: Event| {
                if let Some(target) = e.target_dyn_into::<HtmlInputElement>() {
                    target_url.set(target.value());
                }
            })
        },
    };

    html! {
        <div>
            <div class="fixed inset-0 bg-neutral-900/40 backdrop-blur-xs z-40 transition-opacity" onclick={on_backdrop}></div>
            <div class="fixed inset-y-0 right-0 z-50 w-full sm:w-[540px] md:w-[620px] bg-white dark:bg-neutral-800 shadow-2xl flex flex-col border-l border-neutral-200 dark:border-neutral-700 animate-slide-left">
                { render_drawer_header(&locale, &props.entry, on_close_btn) }
                { render_tab_bar(&locale, *tab, {
                    let tab = tab.clone();
                    Callback::from(move |t| tab.set(t))
                }) }
                <div class="flex-1 overflow-y-auto p-4 sm:p-5 space-y-4">
                    { render_tab_content(&locale, *tab, props, &raw_mode, replay_args) }
                </div>
            </div>
        </div>
    }
}

fn create_copy_curl_callback(
    entry: &AccessLogEntry,
    target_url: &UseStateHandle<String>,
    copied: &UseStateHandle<bool>,
) -> Callback<MouseEvent> {
    let copied = copied.clone();
    let entry = entry.clone();
    let target_url = target_url.clone();
    Callback::from(move |_| {
        let curl = generate_curl_command(&entry, &target_url);
        copy_to_clipboard(&curl);
        copied.set(true);
        let copied = copied.clone();
        gloo_timers::callback::Timeout::new(2000, move || {
            copied.set(false);
        })
        .forget();
    })
}

fn create_replay_callback(
    entry: &AccessLogEntry,
    target_url: &UseStateHandle<String>,
    replay_loading: &UseStateHandle<bool>,
    replay_result: &UseStateHandle<Option<RequestReplayResponse>>,
) -> Callback<MouseEvent> {
    let entry = entry.clone();
    let target_url = target_url.clone();
    let replay_loading = replay_loading.clone();
    let replay_result = replay_result.clone();
    Callback::from(move |_| {
        let entry = entry.clone();
        let url = (*target_url).clone();
        let replay_loading = replay_loading.clone();
        let replay_result = replay_result.clone();
        replay_loading.set(true);
        spawn_local(async move {
            let req_payload = RequestReplayRequest {
                method: entry.method.clone(),
                url,
                headers: entry.request_headers.clone(),
                body: entry.request_body.clone(),
                timeout_ms: Some(10_000),
            };
            let res = Request::post(&format!("{API_ENDPOINT}/traffic/replay")).json(&req_payload);
            if let Ok(req) = res
                && let Ok(resp) = req.send().await
                && let Ok(data) = resp.json::<RequestReplayResponse>().await
            {
                replay_result.set(Some(data));
            }
            replay_loading.set(false);
        });
    })
}

fn render_tab_content(
    locale: &Locale,
    tab: InspectorTab,
    props: &Props,
    raw_mode: &UseStateHandle<bool>,
    replay_args: ReplayTabArgs<'_>,
) -> Html {
    match tab {
        InspectorTab::Overview => render_overview_tab(locale, &props.entry),
        InspectorTab::Headers => render_headers_tab(locale, &props.entry),
        InspectorTab::Payload => render_payload_tab(locale, &props.entry, **raw_mode, {
            let raw_mode = raw_mode.clone();
            Callback::from(move |_| raw_mode.set(!*raw_mode))
        }),
        InspectorTab::Replay => render_replay_tab(locale, replay_args),
    }
}

fn render_drawer_header(
    locale: &Locale,
    entry: &AccessLogEntry,
    on_close: Callback<MouseEvent>,
) -> Html {
    let status_cls = status_badge_class(entry.status);
    let method_cls = method_badge_class(&entry.method);
    let is_ws = entry.is_websocket == Some(true);

    html! {
        <div class="px-5 py-4 border-b border-neutral-200 dark:border-neutral-700 flex items-center justify-between gap-3 bg-neutral-50 dark:bg-neutral-900">
            <div class="flex items-center gap-2 min-w-0">
                <span class={classes!("px-2", "py-0.5", "rounded", "text-xs", "font-bold", "border", method_cls)}>
                    {&entry.method}
                </span>
                <span class={classes!("px-2", "py-0.5", "rounded", "text-xs", "font-bold", status_cls)}>
                    {entry.status}
                </span>
                if is_ws {
                    <span class="px-2 py-0.5 rounded text-xs font-semibold bg-purple-100 text-purple-800 dark:bg-purple-900/60 dark:text-purple-300">
                        {"WS"}
                    </span>
                }
                <span class="font-mono text-sm font-medium text-neutral-800 dark:text-neutral-200 truncate" title={format!("{}{}", entry.host, entry.uri)}>
                    {&entry.uri}
                </span>
            </div>
            <button
                type="button"
                onclick={on_close}
                class="p-1 rounded text-neutral-400 hover:text-neutral-600 dark:hover:text-neutral-200 hover:bg-neutral-100 dark:hover:bg-neutral-700 transition"
                title={locale.t("traffic.close_inspector")}
            >
                <svg class="w-5 h-5" fill="none" viewBox="0 0 24 24" stroke="currentColor">
                    <path stroke-linecap="round" stroke-linejoin="round" stroke-width="2" d="M6 18L18 6M6 6l12 12" />
                </svg>
            </button>
        </div>
    }
}

fn render_tab_bar(
    locale: &Locale,
    active: InspectorTab,
    on_select: Callback<InspectorTab>,
) -> Html {
    let tabs = [
        (InspectorTab::Overview, locale.t("traffic.tab_overview")),
        (InspectorTab::Headers, locale.t("traffic.tab_headers")),
        (InspectorTab::Payload, locale.t("traffic.tab_payload")),
        (InspectorTab::Replay, locale.t("traffic.tab_replay")),
    ];

    html! {
        <div class="flex border-b border-neutral-200 dark:border-neutral-700 px-5 bg-white dark:bg-neutral-800">
            { for tabs.into_iter().map(|(t, label)| {
                let is_active = active == t;
                let on_click = {
                    let on_select = on_select.clone();
                    Callback::from(move |_| on_select.emit(t))
                };
                let cls = if is_active {
                    "border-primary-500 text-primary-600 dark:text-primary-400 font-semibold"
                } else {
                    "border-transparent text-neutral-500 hover:text-neutral-700 dark:hover:text-neutral-300"
                };
                html! {
                    <button
                        type="button"
                        onclick={on_click}
                        class={classes!("py-3", "px-3", "text-xs", "sm:text-sm", "border-b-2", "transition", cls)}
                    >
                        {label}
                    </button>
                }
            }) }
        </div>
    }
}

fn render_overview_tab(locale: &Locale, entry: &AccessLogEntry) -> Html {
    let time_str = format_instant(&entry.timestamp);
    let latency_str = format_latency(entry.latency_ms);
    let lat_cls = latency_class(entry.latency_ms);
    let proxy_name = entry
        .resource_id
        .map(|id| id.to_string())
        .unwrap_or_else(|| "-".to_string());
    let country = entry.country.as_deref().unwrap_or("-");

    html! {
        <div class="space-y-4 text-xs sm:text-sm font-sans">
            <div class="grid grid-cols-2 gap-3 bg-neutral-50 dark:bg-neutral-800 p-4 rounded-lg border border-neutral-200 dark:border-neutral-700">
                <div>
                    <span class="text-neutral-500 dark:text-neutral-400 block text-xs">{locale.t("traffic.col_time")}</span>
                    <span class="font-mono text-neutral-900 dark:text-neutral-100">{time_str}</span>
                </div>
                <div>
                    <span class="text-neutral-500 dark:text-neutral-400 block text-xs">{locale.t("traffic.col_latency")}</span>
                    <span class={classes!("font-mono", lat_cls)}>{latency_str}</span>
                </div>
                <div>
                    <span class="text-neutral-500 dark:text-neutral-400 block text-xs">{locale.t("traffic.col_client")}</span>
                    <span class="font-mono text-neutral-900 dark:text-neutral-100">{&entry.client_ip}</span>
                </div>
                <div>
                    <span class="text-neutral-500 dark:text-neutral-400 block text-xs">{locale.t("traffic.col_country")}</span>
                    <span class="font-mono font-bold text-neutral-900 dark:text-neutral-100">{country}</span>
                </div>
                <div>
                    <span class="text-neutral-500 dark:text-neutral-400 block text-xs">{"Protocol"}</span>
                    <span class="font-mono text-neutral-900 dark:text-neutral-100">{&entry.proto}</span>
                </div>
                <div>
                    <span class="text-neutral-500 dark:text-neutral-400 block text-xs">{locale.t("traffic.col_proxy")}</span>
                    <span class="font-mono text-neutral-900 dark:text-neutral-100">{proxy_name}</span>
                </div>
            </div>

            <div class="bg-neutral-50 dark:bg-neutral-800 p-4 rounded-lg border border-neutral-200 dark:border-neutral-700 space-y-2">
                <div>
                    <span class="text-neutral-500 dark:text-neutral-400 block text-xs">{"Host"}</span>
                    <span class="font-mono text-neutral-900 dark:text-neutral-100 font-semibold">{&entry.host}</span>
                </div>
                <div>
                    <span class="text-neutral-500 dark:text-neutral-400 block text-xs">{"URI Path"}</span>
                    <span class="font-mono text-neutral-900 dark:text-neutral-100 break-all">{&entry.uri}</span>
                </div>
                if let Some(ct) = &entry.content_type {
                    <div>
                        <span class="text-neutral-500 dark:text-neutral-400 block text-xs">{"Content-Type"}</span>
                        <span class="font-mono text-neutral-700 dark:text-neutral-300">{ct}</span>
                    </div>
                }
                if let Some(err) = &entry.error {
                    <div class="p-2 bg-red-50 dark:bg-red-950/40 border border-red-200 dark:border-red-800 rounded text-red-600 dark:text-red-400 font-mono text-xs">
                        {err}
                    </div>
                }
            </div>
        </div>
    }
}

fn render_headers_tab(locale: &Locale, entry: &AccessLogEntry) -> Html {
    html! {
        <div class="space-y-5">
            <div>
                <h4 class="text-xs font-bold uppercase tracking-wider text-neutral-500 dark:text-neutral-400 mb-2">
                    {locale.t("traffic.req_headers")}
                    <span class="ml-2 font-normal lowercase text-neutral-400 font-mono">
                        {format!("({})", entry.request_headers.len())}
                    </span>
                </h4>
                { render_headers_list(locale, &entry.request_headers) }
            </div>

            <div>
                <h4 class="text-xs font-bold uppercase tracking-wider text-neutral-500 dark:text-neutral-400 mb-2">
                    {locale.t("traffic.res_headers")}
                    <span class="ml-2 font-normal lowercase text-neutral-400 font-mono">
                        {format!("({})", entry.response_headers.len())}
                    </span>
                </h4>
                { render_headers_list(locale, &entry.response_headers) }
            </div>
        </div>
    }
}

fn render_headers_list(locale: &Locale, headers: &[HeaderEntry]) -> Html {
    if headers.is_empty() {
        return html! {
            <p class="text-xs text-neutral-400 dark:text-neutral-500 italic p-3 bg-neutral-50 dark:bg-neutral-800 rounded border border-neutral-200 dark:border-neutral-700">
                {locale.t("traffic.no_headers")}
            </p>
        };
    }
    html! {
        <div class="border border-neutral-200 dark:border-neutral-700 rounded-lg overflow-hidden bg-neutral-50 dark:bg-neutral-800 font-mono text-xs divide-y divide-neutral-200 dark:divide-neutral-700">
            { for headers.iter().map(render_header_row) }
        </div>
    }
}

fn render_header_row(h: &HeaderEntry) -> Html {
    html! {
        <div class="px-3 py-1.5 flex flex-col sm:flex-row sm:items-baseline gap-1 sm:gap-3 hover:bg-neutral-100 dark:hover:bg-neutral-700/50">
            <span class="font-semibold text-neutral-700 dark:text-neutral-300 sm:w-1/3 break-all">{format!("{}:", h.name)}</span>
            <span class="text-neutral-600 dark:text-neutral-400 sm:w-2/3 break-all">{&h.value}</span>
        </div>
    }
}

fn render_payload_tab(
    locale: &Locale,
    entry: &AccessLogEntry,
    raw_mode: bool,
    on_toggle_mode: Callback<MouseEvent>,
) -> Html {
    let body = entry.request_body.as_deref().unwrap_or_default();
    if body.is_empty() {
        return html! {
            <div class="p-6 text-center text-xs text-neutral-400 dark:text-neutral-500 italic bg-neutral-50 dark:bg-neutral-800 rounded-lg border border-neutral-200 dark:border-neutral-700">
                {locale.t("traffic.no_body")}
            </div>
        };
    }

    let display_text = if raw_mode {
        body.to_string()
    } else {
        format_json_pretty(body)
    };

    html! {
        <div class="space-y-3">
            <div class="flex items-center justify-between">
                <span class="text-xs font-mono text-neutral-500 dark:text-neutral-400">
                    {format!("Length: {} bytes", body.len())}
                </span>
                <button
                    type="button"
                    onclick={on_toggle_mode}
                    class="px-2.5 py-1 text-xs font-medium rounded border border-neutral-300 dark:border-neutral-600 hover:bg-neutral-100 dark:hover:bg-neutral-700 transition"
                >
                    {if raw_mode { locale.t("traffic.formatted_view") } else { locale.t("traffic.raw_view") }}
                </button>
            </div>
            <pre class="p-4 bg-neutral-900 text-neutral-100 font-mono text-xs rounded-lg overflow-x-auto max-h-[460px] whitespace-pre-wrap break-all border border-neutral-800">
                {display_text}
            </pre>
        </div>
    }
}

struct ReplayTabArgs<'a> {
    target_url: &'a UseStateHandle<String>,
    copied: bool,
    replay_loading: bool,
    replay_result: Option<&'a RequestReplayResponse>,
    on_copy_curl: Callback<MouseEvent>,
    on_replay: Callback<MouseEvent>,
    on_url_change: Callback<Event>,
}

fn render_replay_tab(locale: &Locale, args: ReplayTabArgs<'_>) -> Html {
    let curl_btn_cls = if args.copied {
        "bg-emerald-600 text-white"
    } else {
        "bg-neutral-100 hover:bg-neutral-200 dark:bg-neutral-700 dark:hover:bg-neutral-600 text-neutral-800 dark:text-neutral-200"
    };

    html! {
        <div class="space-y-5 text-xs sm:text-sm font-sans">
            <div>
                <label class="block text-xs font-semibold text-neutral-700 dark:text-neutral-300 mb-1">
                    {locale.t("traffic.target_url")}
                </label>
                <input
                    type="text"
                    value={(**args.target_url).clone()}
                    onchange={args.on_url_change}
                    class={classes!(INPUT_CLASS, "font-mono", "text-xs")}
                />
                <p class={HINT_CLASS}>
                    {"Adjust target URL or proxy domain before firing a replay."}
                </p>
            </div>

            <div class="flex items-center gap-3">
                <button
                    type="button"
                    onclick={args.on_copy_curl}
                    class={classes!("flex", "items-center", "gap-1.5", "px-3.5", "py-2", "rounded-md", "text-xs", "font-semibold", "transition", curl_btn_cls)}
                >
                    <svg class="w-4 h-4" fill="none" viewBox="0 0 24 24" stroke="currentColor">
                        <path stroke-linecap="round" stroke-linejoin="round" stroke-width="2" d="M8 16H6a2 2 0 01-2-2V6a2 2 0 012-2h8a2 2 0 012 2v2m-6 12h8a2 2 0 002-2v-8a2 2 0 00-2-2h-8a2 2 0 00-2 2v8a2 2 0 002 2z" />
                    </svg>
                    {if args.copied { locale.t("traffic.curl_copied") } else { locale.t("traffic.copy_curl") }}
                </button>

                <button
                    type="button"
                    onclick={args.on_replay}
                    disabled={args.replay_loading}
                    class="flex items-center gap-1.5 px-3.5 py-2 rounded-md text-xs font-semibold bg-primary-600 hover:bg-primary-700 text-white disabled:opacity-50 transition shadow-xs"
                >
                    if args.replay_loading {
                        <svg class="animate-spin h-3.5 w-3.5" viewBox="0 0 24 24">
                            <circle class="opacity-25" cx="12" cy="12" r="10" stroke="currentColor" stroke-width="4" fill="none"></circle>
                            <path class="opacity-75" fill="currentColor" d="M4 12a8 8 0 018-8V0C5.373 0 0 5.373 0 12h4z"></path>
                        </svg>
                        {locale.t("traffic.replaying")}
                    } else {
                        <svg class="w-4 h-4" fill="none" viewBox="0 0 24 24" stroke="currentColor">
                            <path stroke-linecap="round" stroke-linejoin="round" stroke-width="2" d="M4 4v5h.582m15.356 2A8.001 8.001 0 004.582 9m0 0H9m11 11v-5h-.581m0 0a8.003 8.003 0 01-15.357-2m15.357 2H15" />
                        </svg>
                        {locale.t("traffic.replay_btn")}
                    }
                </button>
            </div>

            if let Some(res) = args.replay_result {
                { render_replay_response_card(locale, res) }
            }
        </div>
    }
}

fn render_replay_response_card(locale: &Locale, res: &RequestReplayResponse) -> Html {
    let status_cls = status_badge_class(res.status);
    let lat_cls = latency_class(res.latency_ms);
    let lat_str = format_latency(res.latency_ms);

    html! {
        <div class="p-4 bg-neutral-50 dark:bg-neutral-800 border border-neutral-200 dark:border-neutral-700 rounded-lg space-y-3 font-mono text-xs">
            <div class="flex items-center justify-between pb-2 border-b border-neutral-200 dark:border-neutral-700">
                <span class={classes!("px-2", "py-0.5", "rounded", "font-bold", status_cls)}>
                    {format!("{} Response", res.status)}
                </span>
                <span class={classes!("font-semibold", lat_cls)}>
                    {format!("{}: {}", locale.t("traffic.replay_latency"), lat_str)}
                </span>
            </div>

            if let Some(err) = &res.error {
                <div class="p-2 bg-red-50 dark:bg-red-950/40 text-red-600 dark:text-red-400 rounded">
                    {err}
                </div>
            }

            if !res.body.is_empty() {
                <div>
                    <span class="font-sans font-bold text-neutral-500 block mb-1">{"Response Body"}</span>
                    <pre class="p-3 bg-neutral-900 text-neutral-100 rounded overflow-x-auto max-h-[220px] whitespace-pre-wrap break-all">
                        {&res.body}
                    </pre>
                </div>
            }
        </div>
    }
}

pub fn generate_curl_command(entry: &AccessLogEntry, target_url: &str) -> String {
    let mut curl = format!("curl -X {} '{}'", entry.method, target_url);
    for h in &entry.request_headers {
        let lower = h.name.to_ascii_lowercase();
        if lower != "host" && lower != "content-length" {
            let escaped = h.value.replace('\'', "'\\''");
            curl.push_str(&format!(" \\\n  -H '{}: {}'", h.name, escaped));
        }
    }
    if let Some(body) = &entry.request_body
        && !body.is_empty()
    {
        let escaped = body.replace('\'', "'\\''");
        curl.push_str(&format!(" \\\n  --data-raw '{}'", escaped));
    }
    curl
}

pub fn format_json_pretty(text: &str) -> String {
    if let Ok(val) = serde_json::from_str::<serde_json::Value>(text) {
        serde_json::to_string_pretty(&val).unwrap_or_else(|_| text.to_string())
    } else {
        text.to_string()
    }
}

fn copy_to_clipboard(text: &str) {
    let Some(window) = web_sys::window() else {
        return;
    };
    let Ok(nav) = js_sys::Reflect::get(&window, &JsValue::from_str("navigator")) else {
        return;
    };
    let Ok(cb) = js_sys::Reflect::get(&nav, &JsValue::from_str("clipboard")) else {
        return;
    };
    let Ok(write) = js_sys::Reflect::get(&cb, &JsValue::from_str("writeText")) else {
        return;
    };
    if let Ok(func) = write.dyn_into::<js_sys::Function>() {
        let _ = func.call1(&cb, &JsValue::from_str(text));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_generate_curl_command() {
        let entry = AccessLogEntry {
            id: 1,
            timestamp: time::OffsetDateTime::UNIX_EPOCH,
            resource_id: None,
            client_ip: "127.0.0.1".to_string(),
            method: "POST".to_string(),
            host: "api.example.com".to_string(),
            uri: "/v1/test".to_string(),
            status: 200,
            latency_ms: 10.0,
            proto: "HTTP/1.1".to_string(),
            error: None,
            country: None,
            request_headers: vec![
                HeaderEntry::new("Authorization", "Bearer abc"),
                HeaderEntry::new("Content-Type", "application/json"),
                HeaderEntry::new("Host", "api.example.com"),
            ],
            response_headers: Vec::new(),
            request_body: Some(r#"{"msg":"hi"}"#.to_string()),
            response_body: None,
            content_type: Some("application/json".to_string()),
            is_websocket: Some(false),
        };

        let cmd = generate_curl_command(&entry, "http://api.example.com/v1/test");
        assert!(cmd.contains("curl -X POST 'http://api.example.com/v1/test'"));
        assert!(cmd.contains("-H 'Authorization: Bearer abc'"));
        assert!(cmd.contains("-H 'Content-Type: application/json'"));
        assert!(!cmd.contains("-H 'Host:"));
        assert!(cmd.contains(r#"--data-raw '{"msg":"hi"}'"#));
    }

    #[test]
    fn test_format_json_pretty() {
        let raw = r#"{"a":1,"b":2}"#;
        let formatted = format_json_pretty(raw);
        assert!(formatted.contains('\n'));
        assert!(formatted.contains(r#""a": 1"#));

        let plain = "Not a json";
        assert_eq!(format_json_pretty(plain), plain);
    }
}
