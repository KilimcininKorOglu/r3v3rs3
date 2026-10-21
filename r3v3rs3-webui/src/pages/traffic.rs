use super::app_log::download_text_file;
use crate::{API_ENDPOINT, auth::use_ensure_auth, format::format_instant, i18n::use_locale};
use gloo_net::eventsource::futures::EventSource;
use gloo_net::http::Request;
use r3v3rs3_api::i18n::Locale;
use r3v3rs3_api::id::ShortId;
use r3v3rs3_api::traffic::AccessLogEntry;
use wasm_bindgen_futures::spawn_local;
use web_sys::{HtmlInputElement, HtmlSelectElement};
use yew::prelude::*;
use yew_router::prelude::use_navigator;

const MAX_UI_ENTRIES: usize = 1000;

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum StatusCategory {
    Success2xx,
    Redirect3xx,
    ClientError4xx,
    ServerError5xx,
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct TrafficStats {
    pub total: usize,
    pub count_2xx: usize,
    pub count_3xx: usize,
    pub count_4xx: usize,
    pub count_5xx: usize,
    pub avg_latency_ms: f64,
}

struct HeaderControls {
    pub live: bool,
    pub auto_scroll: bool,
    pub on_toggle_live: Callback<MouseEvent>,
    pub on_toggle_scroll: Callback<MouseEvent>,
    pub on_clear: Callback<MouseEvent>,
    pub on_export: Callback<MouseEvent>,
}

#[function_component(TrafficView)]
pub fn traffic_view() -> Html {
    use_ensure_auth();
    let locale = use_locale();
    let navigator = use_navigator();
    let entries = use_state(Vec::<AccessLogEntry>::new);
    let live = use_state(|| true);
    let auto_scroll = use_state(|| true);
    let search = use_state(String::new);
    let status_filter = use_state(|| Option::<StatusCategory>::None);
    let selected_proxy = use_state(|| Option::<ShortId>::None);
    let table_ref = use_node_ref();

    use_initial_traffic_load(&entries);
    use_live_traffic_stream(&entries, &live);
    use_traffic_auto_scroll(&table_ref, &auto_scroll, &entries);

    let stats = calculate_stats(&entries);
    let filtered = filter_traffic_logs(&entries, &search, *status_filter, *selected_proxy);

    let on_back = Callback::from(move |_| {
        if let Some(nav) = &navigator {
            nav.back();
        }
    });
    let controls = HeaderControls {
        live: *live,
        auto_scroll: *auto_scroll,
        on_toggle_live: {
            let live = live.clone();
            Callback::from(move |_: MouseEvent| live.set(!*live))
        },
        on_toggle_scroll: {
            let auto_scroll = auto_scroll.clone();
            Callback::from(move |_: MouseEvent| auto_scroll.set(!*auto_scroll))
        },
        on_clear: {
            let entries = entries.clone();
            Callback::from(move |_: MouseEvent| entries.set(Vec::new()))
        },
        on_export: {
            let entries = entries.clone();
            Callback::from(move |_: MouseEvent| {
                let export = export_traffic_json(&entries);
                download_text_file("access-traffic.json", &export);
            })
        },
    };

    html! {
        <div class="space-y-4">
            { header_view(locale, on_back, controls) }
            { stats_cards_view(locale, &stats) }
            { filters_toolbar_view(locale, &search, &status_filter, &selected_proxy) }
            { table_view(locale, &table_ref, &filtered) }
        </div>
    }
}

fn header_view(locale: Locale, on_back: Callback<MouseEvent>, controls: HeaderControls) -> Html {
    let HeaderControls {
        live,
        auto_scroll,
        on_toggle_live,
        on_toggle_scroll,
        on_clear,
        on_export,
    } = controls;
    html! {
        <div class="flex flex-col sm:flex-row items-start sm:items-center justify-between gap-3 pb-2 border-b dark:border-neutral-700">
            <div class="flex items-center gap-3">
                <button
                    onclick={on_back}
                    type="button"
                    class="inline-flex items-center text-neutral-500 dark:text-neutral-300 bg-white dark:bg-neutral-800 border border-neutral-300 dark:border-neutral-700 hover:bg-neutral-100 dark:hover:bg-neutral-700 rounded-lg text-sm px-3 py-1.5 font-medium"
                >
                    <img src="/assets/icons/arrow-back.svg" class="w-4 h-4 mr-1" />
                    {locale.t("common.back")}
                </button>
                <div>
                    <h1 class="text-xl font-bold text-neutral-900 dark:text-neutral-100 flex items-center gap-2">
                        {locale.t("traffic.title")}
                        { live_badge(locale, live) }
                    </h1>
                    <p class="text-xs text-neutral-500 dark:text-neutral-400">
                        {locale.t("traffic.subtitle")}
                    </p>
                </div>
            </div>
            <div class="flex items-center gap-2 flex-wrap">
                <button
                    onclick={on_toggle_live}
                    type="button"
                    class={if live {
                        "inline-flex items-center text-xs px-3 py-1.5 rounded-lg border border-amber-300 dark:border-amber-700 bg-amber-50 dark:bg-amber-950/40 text-amber-700 dark:text-amber-300 font-medium hover:bg-amber-100"
                    } else {
                        "inline-flex items-center text-xs px-3 py-1.5 rounded-lg border border-emerald-300 dark:border-emerald-700 bg-emerald-50 dark:bg-emerald-950/40 text-emerald-700 dark:text-emerald-300 font-medium hover:bg-emerald-100"
                    }}
                >
                    {if live { locale.t("traffic.pause") } else { locale.t("traffic.resume") }}
                </button>
                <button
                    onclick={on_toggle_scroll}
                    type="button"
                    class={if auto_scroll {
                        "inline-flex items-center text-xs px-3 py-1.5 rounded-lg border border-blue-300 dark:border-blue-700 bg-blue-50 dark:bg-blue-950/40 text-blue-700 dark:text-blue-300 font-medium"
                    } else {
                        "inline-flex items-center text-xs px-3 py-1.5 rounded-lg border border-neutral-300 dark:border-neutral-700 bg-white dark:bg-neutral-800 text-neutral-600 dark:text-neutral-300 font-medium"
                    }}
                >
                    {locale.t("traffic.auto_scroll")}
                </button>
                <button
                    onclick={on_clear}
                    type="button"
                    class="inline-flex items-center text-xs px-3 py-1.5 rounded-lg border border-neutral-300 dark:border-neutral-700 bg-white dark:bg-neutral-800 hover:bg-neutral-100 dark:hover:bg-neutral-700 text-neutral-700 dark:text-neutral-200 font-medium"
                >
                    {locale.t("traffic.clear")}
                </button>
                <button
                    onclick={on_export}
                    type="button"
                    class="inline-flex items-center text-xs px-3 py-1.5 rounded-lg border border-neutral-300 dark:border-neutral-700 bg-white dark:bg-neutral-800 hover:bg-neutral-100 dark:hover:bg-neutral-700 text-neutral-700 dark:text-neutral-200 font-medium"
                >
                    {locale.t("traffic.download")}
                </button>
                <a
                    href="/api/metrics/grafana"
                    target="_blank"
                    download="r3v3rs3-grafana-overview.json"
                    title={locale.t("traffic.grafana_tooltip")}
                    class="inline-flex items-center text-xs px-3 py-1.5 rounded-lg border border-indigo-300 dark:border-indigo-700 bg-indigo-50 dark:bg-indigo-950/40 hover:bg-indigo-100 dark:hover:bg-indigo-900/60 text-indigo-700 dark:text-indigo-300 font-medium"
                >
                    <svg class="w-3.5 h-3.5 mr-1" fill="none" stroke="currentColor" viewBox="0 0 24 24">
                        <path stroke-linecap="round" stroke-linejoin="round" stroke-width="2" d="M9 19v-6a2 2 0 00-2-2H5a2 2 0 00-2 2v6a2 2 0 002 2h2a2 2 0 002-2zm0 0V9a2 2 0 012-2h2a2 2 0 012 2v10m-6 0a2 2 0 002 2h2a2 2 0 002-2m0 0V5a2 2 0 012-2h2a2 2 0 012 2v14a2 2 0 01-2 2h-2a2 2 0 01-2-2z" />
                    </svg>
                    {locale.t("traffic.grafana_dashboard")}
                </a>
            </div>
        </div>
    }
}

fn live_badge(locale: Locale, live: bool) -> Html {
    if live {
        html! {
            <span class="inline-flex items-center gap-1.5 px-2 py-0.5 rounded-full text-xs font-medium bg-emerald-100 text-emerald-800 dark:bg-emerald-900/60 dark:text-emerald-300">
                <span class="w-1.5 h-1.5 rounded-full bg-emerald-500 animate-pulse"></span>
                {locale.t("traffic.live")}
            </span>
        }
    } else {
        html! {
            <span class="inline-flex items-center gap-1.5 px-2 py-0.5 rounded-full text-xs font-medium bg-amber-100 text-amber-800 dark:bg-amber-900/60 dark:text-amber-300">
                <span class="w-1.5 h-1.5 rounded-full bg-amber-500"></span>
                {locale.t("traffic.paused")}
            </span>
        }
    }
}

fn stats_cards_view(locale: Locale, stats: &TrafficStats) -> Html {
    html! {
        <div class="grid grid-cols-2 sm:grid-cols-3 lg:grid-cols-6 gap-3">
            <div class="p-3 bg-white dark:bg-neutral-800 rounded-lg border border-neutral-200 dark:border-neutral-700 shadow-sm">
                <span class="text-xs text-neutral-500 dark:text-neutral-400">{locale.t("traffic.stat_total")}</span>
                <p class="text-lg font-bold text-neutral-900 dark:text-neutral-100">{stats.total}</p>
            </div>
            <div class="p-3 bg-white dark:bg-neutral-800 rounded-lg border border-neutral-200 dark:border-neutral-700 shadow-sm">
                <span class="text-xs text-emerald-600 dark:text-emerald-400">{locale.t("traffic.stat_success")}</span>
                <p class="text-lg font-bold text-emerald-600 dark:text-emerald-400">{stats.count_2xx}</p>
            </div>
            <div class="p-3 bg-white dark:bg-neutral-800 rounded-lg border border-neutral-200 dark:border-neutral-700 shadow-sm">
                <span class="text-xs text-blue-600 dark:text-blue-400">{locale.t("traffic.stat_redirect")}</span>
                <p class="text-lg font-bold text-blue-600 dark:text-blue-400">{stats.count_3xx}</p>
            </div>
            <div class="p-3 bg-white dark:bg-neutral-800 rounded-lg border border-neutral-200 dark:border-neutral-700 shadow-sm">
                <span class="text-xs text-amber-600 dark:text-amber-400">{locale.t("traffic.stat_client_err")}</span>
                <p class="text-lg font-bold text-amber-600 dark:text-amber-400">{stats.count_4xx}</p>
            </div>
            <div class="p-3 bg-white dark:bg-neutral-800 rounded-lg border border-neutral-200 dark:border-neutral-700 shadow-sm">
                <span class="text-xs text-red-600 dark:text-red-400">{locale.t("traffic.stat_server_err")}</span>
                <p class="text-lg font-bold text-red-600 dark:text-red-400">{stats.count_5xx}</p>
            </div>
            <div class="p-3 bg-white dark:bg-neutral-800 rounded-lg border border-neutral-200 dark:border-neutral-700 shadow-sm">
                <span class="text-xs text-neutral-500 dark:text-neutral-400">{locale.t("traffic.stat_avg_latency")}</span>
                <p class="text-lg font-bold text-neutral-900 dark:text-neutral-100">{format_latency(stats.avg_latency_ms)}</p>
            </div>
        </div>
    }
}

fn filters_toolbar_view(
    locale: Locale,
    search: &UseStateHandle<String>,
    status_filter: &UseStateHandle<Option<StatusCategory>>,
    _selected_proxy: &UseStateHandle<Option<ShortId>>,
) -> Html {
    let on_search_input = {
        let search = search.clone();
        Callback::from(move |e: InputEvent| {
            if let Some(input) = e.target_dyn_into::<HtmlInputElement>() {
                search.set(input.value());
            }
        })
    };
    let on_clear_search = {
        let search = search.clone();
        Callback::from(move |_: MouseEvent| search.set(String::new()))
    };
    let on_status_change = {
        let status_filter = status_filter.clone();
        Callback::from(move |e: Event| {
            if let Some(select) = e.target_dyn_into::<HtmlSelectElement>() {
                let val = select.value();
                let cat = match val.as_str() {
                    "2xx" => Some(StatusCategory::Success2xx),
                    "3xx" => Some(StatusCategory::Redirect3xx),
                    "4xx" => Some(StatusCategory::ClientError4xx),
                    "5xx" => Some(StatusCategory::ServerError5xx),
                    _ => None,
                };
                status_filter.set(cat);
            }
        })
    };

    html! {
        <div class="flex flex-col sm:flex-row gap-2 items-stretch sm:items-center">
            <div class="relative flex-1">
                <input
                    type="text"
                    placeholder={locale.t("traffic.search_placeholder")}
                    value={(**search).clone()}
                    oninput={on_search_input}
                    class="w-full text-xs sm:text-sm px-3 py-2 rounded-lg border border-neutral-300 dark:border-neutral-700 bg-white dark:bg-neutral-800 text-neutral-900 dark:text-neutral-100 placeholder-neutral-400 focus:outline-none focus:ring-2 focus:ring-blue-500"
                />
                if !search.is_empty() {
                    <button
                        onclick={on_clear_search}
                        type="button"
                        class="absolute right-2.5 top-2 text-xs text-neutral-400 hover:text-neutral-600 dark:hover:text-neutral-200"
                    >
                        {"✕"}
                    </button>
                }
            </div>
            <select
                onchange={on_status_change}
                class="text-xs sm:text-sm px-3 py-2 rounded-lg border border-neutral-300 dark:border-neutral-700 bg-white dark:bg-neutral-800 text-neutral-900 dark:text-neutral-100 focus:outline-none focus:ring-2 focus:ring-blue-500"
            >
                <option value="all">{locale.t("traffic.filter_all_status")}</option>
                <option value="2xx">{locale.t("traffic.filter_2xx")}</option>
                <option value="3xx">{locale.t("traffic.filter_3xx")}</option>
                <option value="4xx">{locale.t("traffic.filter_4xx")}</option>
                <option value="5xx">{locale.t("traffic.filter_5xx")}</option>
            </select>
        </div>
    }
}

fn table_view(locale: Locale, table_ref: &NodeRef, filtered: &[AccessLogEntry]) -> Html {
    if filtered.is_empty() {
        return html! {
            <div class="py-12 bg-white dark:bg-neutral-800 rounded-lg border border-neutral-200 dark:border-neutral-700 text-center">
                <p class="text-sm text-neutral-500 dark:text-neutral-400 font-medium">
                    {locale.t("traffic.no_matches")}
                </p>
            </div>
        };
    }

    html! {
        <div ref={table_ref.clone()} class="overflow-x-auto overflow-y-auto max-h-[65vh] bg-white dark:bg-neutral-800 rounded-lg border border-neutral-200 dark:border-neutral-700 shadow-sm">
            <table class="w-full text-left text-xs sm:text-sm">
                <thead class="bg-neutral-50 dark:bg-neutral-900/60 border-b dark:border-neutral-700 text-neutral-600 dark:text-neutral-400 font-semibold sticky top-0 z-10">
                    <tr>
                        <th class="px-3 py-2">{locale.t("traffic.col_time")}</th>
                        <th class="px-3 py-2">{locale.t("traffic.col_status")}</th>
                        <th class="px-3 py-2">{locale.t("traffic.col_method")}</th>
                        <th class="px-3 py-2">{locale.t("traffic.col_path")}</th>
                        <th class="px-3 py-2">{locale.t("traffic.col_client")}</th>
                        <th class="px-3 py-2">{locale.t("traffic.col_latency")}</th>
                        <th class="px-3 py-2">{locale.t("traffic.col_proxy")}</th>
                    </tr>
                </thead>
                <tbody class="divide-y divide-neutral-200 dark:divide-neutral-700 font-mono">
                    { for filtered.iter().map(render_entry_row) }
                </tbody>
            </table>
        </div>
    }
}

fn render_entry_row(entry: &AccessLogEntry) -> Html {
    let time_str = format_instant(&entry.timestamp);
    let time_part = time_str.split('T').nth(1).unwrap_or(&time_str);
    let status_class = status_badge_class(entry.status);
    let method_class = method_badge_class(&entry.method);
    let latency_str = format_latency(entry.latency_ms);
    let latency_cls = latency_class(entry.latency_ms);
    let proxy_name = entry
        .resource_id
        .map(|id| id.to_string())
        .unwrap_or_else(|| "-".to_string());

    html! {
        <tr class="hover:bg-neutral-50 dark:hover:bg-neutral-750/50">
            <td class="px-3 py-2 whitespace-nowrap text-xs text-neutral-500 dark:text-neutral-400">
                {time_part}
            </td>
            <td class="px-3 py-2 whitespace-nowrap">
                <span class={classes!("px-2", "py-0.5", "rounded", "font-bold", "text-xs", status_class)}>
                    {entry.status}
                </span>
            </td>
            <td class="px-3 py-2 whitespace-nowrap">
                <span class={classes!("px-1.5", "py-0.5", "rounded", "border", "font-bold", "text-xs", method_class)}>
                    {&entry.method}
                </span>
            </td>
            <td class="px-3 py-2 break-all font-sans text-xs sm:text-sm">
                <span class="font-medium text-neutral-900 dark:text-neutral-100">{&entry.host}</span>
                <span class="text-neutral-600 dark:text-neutral-300">{&entry.uri}</span>
                if let Some(err) = &entry.error {
                    <div class="text-xs text-red-600 dark:text-red-400 mt-0.5 font-mono">{err}</div>
                }
            </td>
            <td class="px-3 py-2 whitespace-nowrap text-xs text-neutral-600 dark:text-neutral-400">
                {&entry.client_ip}
            </td>
            <td class="px-3 py-2 whitespace-nowrap">
                <span class={latency_cls}>{latency_str}</span>
            </td>
            <td class="px-3 py-2 whitespace-nowrap text-xs text-neutral-500 dark:text-neutral-400">
                {proxy_name}
            </td>
        </tr>
    }
}

pub fn format_latency(ms: f64) -> String {
    if ms < 1.0 {
        format!("{:.2} ms", ms)
    } else if ms < 1000.0 {
        format!("{:.1} ms", ms)
    } else {
        format!("{:.2} s", ms / 1000.0)
    }
}

pub fn status_badge_class(status: u16) -> &'static str {
    match status {
        200..=299 => "bg-emerald-100 text-emerald-800 dark:bg-emerald-900/50 dark:text-emerald-300",
        300..=399 => "bg-blue-100 text-blue-800 dark:bg-blue-900/50 dark:text-blue-300",
        400..=499 => "bg-amber-100 text-amber-800 dark:bg-amber-900/50 dark:text-amber-300",
        500..=599 => "bg-red-100 text-red-800 dark:bg-red-900/50 dark:text-red-300",
        _ => "bg-neutral-100 text-neutral-800 dark:bg-neutral-700 dark:text-neutral-300",
    }
}

pub fn method_badge_class(method: &str) -> &'static str {
    match method {
        "GET" => {
            "text-emerald-700 bg-emerald-50 dark:text-emerald-300 dark:bg-emerald-950/60 border-emerald-300 dark:border-emerald-800"
        }
        "POST" => {
            "text-blue-700 bg-blue-50 dark:text-blue-300 dark:bg-blue-950/60 border-blue-300 dark:border-blue-800"
        }
        "PUT" => {
            "text-amber-700 bg-amber-50 dark:text-amber-300 dark:bg-amber-950/60 border-amber-300 dark:border-amber-800"
        }
        "DELETE" => {
            "text-red-700 bg-red-50 dark:text-red-300 dark:bg-red-950/60 border-red-300 dark:border-red-800"
        }
        "PATCH" => {
            "text-purple-700 bg-purple-50 dark:text-purple-300 dark:bg-purple-950/60 border-purple-300 dark:border-purple-800"
        }
        _ => {
            "text-neutral-700 bg-neutral-50 dark:text-neutral-300 dark:bg-neutral-800 border-neutral-300 dark:border-neutral-700"
        }
    }
}

pub fn latency_class(ms: f64) -> &'static str {
    if ms < 50.0 {
        "text-emerald-600 dark:text-emerald-400 font-mono text-xs font-semibold"
    } else if ms < 300.0 {
        "text-amber-600 dark:text-amber-400 font-mono text-xs font-semibold"
    } else {
        "text-red-600 dark:text-red-400 font-mono text-xs font-semibold"
    }
}

pub fn calculate_stats(entries: &[AccessLogEntry]) -> TrafficStats {
    let total = entries.len();
    if total == 0 {
        return TrafficStats::default();
    }
    let mut count_2xx = 0;
    let mut count_3xx = 0;
    let mut count_4xx = 0;
    let mut count_5xx = 0;
    let mut sum_latency = 0.0;

    for entry in entries {
        sum_latency += entry.latency_ms;
        match entry.status {
            200..=299 => count_2xx += 1,
            300..=399 => count_3xx += 1,
            400..=499 => count_4xx += 1,
            500..=599 => count_5xx += 1,
            _ => {}
        }
    }

    TrafficStats {
        total,
        count_2xx,
        count_3xx,
        count_4xx,
        count_5xx,
        avg_latency_ms: sum_latency / (total as f64),
    }
}

fn matches_status_cat(entry: &AccessLogEntry, cat: Option<StatusCategory>) -> bool {
    let Some(cat) = cat else {
        return true;
    };
    match cat {
        StatusCategory::Success2xx => (200..=299).contains(&entry.status),
        StatusCategory::Redirect3xx => (300..=399).contains(&entry.status),
        StatusCategory::ClientError4xx => (400..=499).contains(&entry.status),
        StatusCategory::ServerError5xx => (500..=599).contains(&entry.status),
    }
}

fn matches_proxy(entry: &AccessLogEntry, proxy_id: Option<ShortId>) -> bool {
    match proxy_id {
        Some(expected) => entry.resource_id == Some(expected),
        None => true,
    }
}

fn matches_search(entry: &AccessLogEntry, query: &str) -> bool {
    if query.is_empty() {
        return true;
    }
    entry.uri.to_lowercase().contains(query)
        || entry.host.to_lowercase().contains(query)
        || entry.client_ip.to_lowercase().contains(query)
        || entry.method.to_lowercase().contains(query)
        || entry.status.to_string().contains(query)
}

pub fn filter_traffic_logs(
    entries: &[AccessLogEntry],
    search: &str,
    status: Option<StatusCategory>,
    proxy_id: Option<ShortId>,
) -> Vec<AccessLogEntry> {
    let query = search.trim().to_lowercase();
    entries
        .iter()
        .filter(|e| {
            matches_status_cat(e, status) && matches_proxy(e, proxy_id) && matches_search(e, &query)
        })
        .cloned()
        .collect()
}

pub fn export_traffic_json(entries: &[AccessLogEntry]) -> String {
    serde_json::to_string_pretty(entries).unwrap_or_default()
}

#[hook]
fn use_initial_traffic_load(entries: &UseStateHandle<Vec<AccessLogEntry>>) {
    let entries = entries.clone();
    use_effect_with((), move |_| {
        spawn_local(async move {
            let req = Request::get(&format!("{API_ENDPOINT}/traffic"));
            if let Ok(resp) = req.send().await
                && let Ok(list) = resp.json::<Vec<AccessLogEntry>>().await
            {
                entries.set(list);
            }
        });
    });
}

#[hook]
fn use_live_traffic_stream(
    entries: &UseStateHandle<Vec<AccessLogEntry>>,
    live: &UseStateHandle<bool>,
) {
    let entries = entries.clone();
    use_effect_with(**live, move |&is_live| {
        if !is_live {
            return;
        }
        let entries = entries.clone();
        spawn_local(async move {
            let mut source = match EventSource::new(&format!("{API_ENDPOINT}/traffic/live")) {
                Ok(source) => source,
                Err(_) => return,
            };
            let mut stream = match source.subscribe("message") {
                Ok(stream) => stream,
                Err(_) => return,
            };
            use futures::StreamExt;
            while let Some(Ok((_, msg))) = stream.next().await {
                if let Some(text) = msg.data().as_string()
                    && let Ok(entry) = serde_json::from_str::<AccessLogEntry>(&text)
                {
                    let mut current = (*entries).clone();
                    current.insert(0, entry);
                    if current.len() > MAX_UI_ENTRIES {
                        current.pop();
                    }
                    entries.set(current);
                }
            }
        });
    });
}

#[hook]
fn use_traffic_auto_scroll(
    table_ref: &NodeRef,
    auto_scroll: &UseStateHandle<bool>,
    entries: &UseStateHandle<Vec<AccessLogEntry>>,
) {
    let table_ref = table_ref.clone();
    let auto_scroll = auto_scroll.clone();
    use_effect_with((**entries).len(), move |_| {
        if *auto_scroll && let Some(elem) = table_ref.cast::<web_sys::Element>() {
            elem.set_scroll_top(0);
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;
    use time::OffsetDateTime;

    fn sample_log(status: u16, latency_ms: f64, uri: &str) -> AccessLogEntry {
        AccessLogEntry {
            id: 1,
            timestamp: OffsetDateTime::UNIX_EPOCH,
            resource_id: Some("proxy1".parse().unwrap()),
            client_ip: "127.0.0.1".to_string(),
            method: "GET".to_string(),
            host: "test.local".to_string(),
            uri: uri.to_string(),
            status,
            latency_ms,
            proto: "HTTP/1.1".to_string(),
            error: None,
        }
    }

    #[test]
    fn test_calculate_stats() {
        let logs = vec![
            sample_log(200, 10.0, "/api/a"),
            sample_log(301, 20.0, "/api/b"),
            sample_log(404, 30.0, "/api/c"),
            sample_log(500, 40.0, "/api/d"),
        ];
        let stats = calculate_stats(&logs);
        assert_eq!(stats.total, 4);
        assert_eq!(stats.count_2xx, 1);
        assert_eq!(stats.count_3xx, 1);
        assert_eq!(stats.count_4xx, 1);
        assert_eq!(stats.count_5xx, 1);
        assert_eq!(stats.avg_latency_ms, 25.0);
    }

    #[test]
    fn test_filter_traffic_logs() {
        let logs = vec![
            sample_log(200, 10.0, "/health"),
            sample_log(404, 20.0, "/missing"),
        ];
        let filtered = filter_traffic_logs(&logs, "health", None, None);
        assert_eq!(filtered.len(), 1);
        assert_eq!(filtered[0].status, 200);

        let filtered_status =
            filter_traffic_logs(&logs, "", Some(StatusCategory::ClientError4xx), None);
        assert_eq!(filtered_status.len(), 1);
        assert_eq!(filtered_status[0].uri, "/missing");
    }

    #[test]
    fn test_format_latency() {
        assert_eq!(format_latency(0.42), "0.42 ms");
        assert_eq!(format_latency(15.2), "15.2 ms");
        assert_eq!(format_latency(1500.0), "1.50 s");
    }
}
