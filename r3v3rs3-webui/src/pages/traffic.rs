use super::app_log::download_text_file;
use crate::components::traffic_inspector::TrafficInspector;
use crate::{API_ENDPOINT, auth::use_ensure_auth, format::format_instant, i18n::use_locale};
use gloo_net::eventsource::futures::EventSource;
use gloo_net::http::Request;
use r3v3rs3_api::i18n::Locale;
use r3v3rs3_api::id::ShortId;
use r3v3rs3_api::traffic::{AccessLogEntry, CountryTrafficStat, TrafficGeoDistribution};
use std::collections::{BTreeMap, BTreeSet};
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
    pub show_geo_map: bool,
    pub on_toggle_live: Callback<MouseEvent>,
    pub on_toggle_scroll: Callback<MouseEvent>,
    pub on_toggle_map: Callback<MouseEvent>,
    pub on_clear: Callback<MouseEvent>,
    pub on_export: Callback<MouseEvent>,
}

fn create_header_controls(
    live: &UseStateHandle<bool>,
    auto_scroll: &UseStateHandle<bool>,
    show_geo_map: &UseStateHandle<bool>,
    entries: &UseStateHandle<Vec<AccessLogEntry>>,
) -> HeaderControls {
    HeaderControls {
        live: **live,
        auto_scroll: **auto_scroll,
        show_geo_map: **show_geo_map,
        on_toggle_live: {
            let live = live.clone();
            Callback::from(move |_: MouseEvent| live.set(!*live))
        },
        on_toggle_scroll: {
            let auto_scroll = auto_scroll.clone();
            Callback::from(move |_: MouseEvent| auto_scroll.set(!*auto_scroll))
        },
        on_toggle_map: {
            let show_geo_map = show_geo_map.clone();
            Callback::from(move |_: MouseEvent| show_geo_map.set(!*show_geo_map))
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
    }
}

fn create_inspector_callbacks(
    selected_entry: &UseStateHandle<Option<AccessLogEntry>>,
) -> (Callback<AccessLogEntry>, Callback<()>) {
    let on_select = {
        let selected_entry = selected_entry.clone();
        Callback::from(move |entry: AccessLogEntry| {
            selected_entry.set(Some(entry));
        })
    };
    let on_close = {
        let selected_entry = selected_entry.clone();
        Callback::from(move |_| {
            selected_entry.set(None);
        })
    };
    (on_select, on_close)
}

fn create_back_callback(navigator: Option<yew_router::prelude::Navigator>) -> Callback<MouseEvent> {
    Callback::from(move |_| {
        if let Some(nav) = &navigator {
            nav.back();
        }
    })
}

fn create_country_callback(
    selected_country: &UseStateHandle<Option<String>>,
) -> Callback<Option<String>> {
    let selected_country = selected_country.clone();
    Callback::from(move |code: Option<String>| {
        selected_country.set(code);
    })
}

fn inspector_drawer(entry: &Option<AccessLogEntry>, on_close: Callback<()>) -> Html {
    if let Some(entry) = entry.clone() {
        html! { <TrafficInspector entry={entry} on_close={on_close} /> }
    } else {
        html! {}
    }
}

#[derive(Clone, Copy)]
pub struct CountryMapItem {
    pub code: &'static str,
    pub name: &'static str,
    pub path: &'static str,
}

pub static CONTINENT_OUTLINES: &[&str] = &[
    "M 70 80 L 150 75 L 320 60 L 320 140 L 290 140 L 290 220 L 240 225 L 250 280 L 205 280 L 180 230 L 150 190 L 70 120 Z",
    "M 260 285 L 320 310 L 400 310 L 410 390 L 350 490 L 325 490 L 310 350 Z",
    "M 450 185 L 470 130 L 515 80 L 560 80 L 565 190 L 525 220 L 445 210 Z",
    "M 450 225 L 555 220 L 600 220 L 600 260 L 585 385 L 575 445 L 525 435 L 470 310 L 450 250 Z",
    "M 565 70 L 890 70 L 870 150 L 820 180 L 810 270 L 730 230 L 710 320 L 675 280 L 640 290 L 595 280 L 565 190 Z",
    "M 760 340 L 890 340 L 935 410 L 925 460 L 870 430 L 760 420 Z",
];

pub static COUNTRY_PATHS: &[CountryMapItem] = &[
    CountryMapItem {
        code: "US",
        name: "United States",
        path: "M 150 140 L 290 140 L 290 220 L 190 230 L 150 190 Z M 70 80 L 150 80 L 140 130 L 70 120 Z",
    },
    CountryMapItem {
        code: "CA",
        name: "Canada",
        path: "M 140 70 L 320 60 L 320 135 L 140 135 Z",
    },
    CountryMapItem {
        code: "MX",
        name: "Mexico",
        path: "M 180 230 L 240 225 L 250 280 L 205 280 Z",
    },
    CountryMapItem {
        code: "BR",
        name: "Brazil",
        path: "M 320 310 L 400 310 L 410 390 L 340 410 L 310 350 Z",
    },
    CountryMapItem {
        code: "AR",
        name: "Argentina",
        path: "M 320 415 L 360 415 L 350 490 L 325 490 Z",
    },
    CountryMapItem {
        code: "GB",
        name: "United Kingdom",
        path: "M 470 130 L 485 125 L 485 150 L 470 150 Z",
    },
    CountryMapItem {
        code: "FR",
        name: "France",
        path: "M 475 160 L 505 160 L 500 190 L 475 190 Z",
    },
    CountryMapItem {
        code: "DE",
        name: "Germany",
        path: "M 505 145 L 530 145 L 530 175 L 505 175 Z",
    },
    CountryMapItem {
        code: "IT",
        name: "Italy",
        path: "M 515 180 L 535 180 L 540 220 L 525 220 Z",
    },
    CountryMapItem {
        code: "ES",
        name: "Spain",
        path: "M 450 185 L 475 185 L 470 215 L 445 210 Z",
    },
    CountryMapItem {
        code: "NL",
        name: "Netherlands",
        path: "M 498 140 L 510 140 L 510 150 L 498 150 Z",
    },
    CountryMapItem {
        code: "PL",
        name: "Poland",
        path: "M 532 145 L 560 145 L 558 175 L 532 175 Z",
    },
    CountryMapItem {
        code: "SE",
        name: "Sweden",
        path: "M 515 80 L 560 80 L 550 140 L 515 140 Z",
    },
    CountryMapItem {
        code: "TR",
        name: "Turkey",
        path: "M 565 190 L 615 190 L 610 215 L 565 215 Z",
    },
    CountryMapItem {
        code: "RU",
        name: "Russia",
        path: "M 565 70 L 890 70 L 870 150 L 565 140 Z",
    },
    CountryMapItem {
        code: "CN",
        name: "China",
        path: "M 690 180 L 820 180 L 810 270 L 700 270 Z",
    },
    CountryMapItem {
        code: "IN",
        name: "India",
        path: "M 670 230 L 730 230 L 710 320 L 675 280 Z",
    },
    CountryMapItem {
        code: "JP",
        name: "Japan",
        path: "M 845 175 L 870 175 L 860 240 L 840 240 Z",
    },
    CountryMapItem {
        code: "KR",
        name: "South Korea",
        path: "M 815 195 L 835 195 L 835 220 L 815 220 Z",
    },
    CountryMapItem {
        code: "SG",
        name: "Singapore",
        path: "M 735 325 L 748 325 L 748 335 L 735 335 Z",
    },
    CountryMapItem {
        code: "AU",
        name: "Australia",
        path: "M 770 340 L 890 340 L 870 430 L 760 420 Z",
    },
    CountryMapItem {
        code: "NZ",
        name: "New Zealand",
        path: "M 905 410 L 935 410 L 925 460 L 900 450 Z",
    },
    CountryMapItem {
        code: "ZA",
        name: "South Africa",
        path: "M 525 385 L 585 385 L 575 445 L 525 435 Z",
    },
    CountryMapItem {
        code: "EG",
        name: "Egypt",
        path: "M 555 220 L 600 220 L 595 260 L 555 260 Z",
    },
    CountryMapItem {
        code: "NG",
        name: "Nigeria",
        path: "M 470 260 L 535 260 L 535 320 L 470 310 Z",
    },
    CountryMapItem {
        code: "SA",
        name: "Saudi Arabia",
        path: "M 600 225 L 650 235 L 640 290 L 595 280 Z",
    },
    CountryMapItem {
        code: "AE",
        name: "United Arab Emirates",
        path: "M 645 250 L 665 250 L 665 270 L 645 270 Z",
    },
];

#[function_component(TrafficView)]
pub fn traffic_view() -> Html {
    use_ensure_auth();
    let locale = use_locale();
    let navigator = use_navigator();
    let entries = use_state(Vec::<AccessLogEntry>::new);
    let live = use_state(|| true);
    let auto_scroll = use_state(|| true);
    let show_geo_map = use_state(|| true);
    let search = use_state(String::new);
    let status_filter = use_state(|| Option::<StatusCategory>::None);
    let selected_proxy = use_state(|| Option::<ShortId>::None);
    let selected_country = use_state(|| Option::<String>::None);
    let selected_entry = use_state(|| Option::<AccessLogEntry>::None);
    let table_ref = use_node_ref();

    use_initial_traffic_load(&entries);
    use_live_traffic_stream(&entries, &live);
    use_traffic_auto_scroll(&table_ref, &auto_scroll, &entries);

    let stats = calculate_stats(&entries);
    let geo_dist = calculate_geo_distribution(&entries);
    let available_countries = extract_available_countries(&entries);
    let filtered = filter_traffic_logs(
        &entries,
        &search,
        *status_filter,
        *selected_proxy,
        selected_country.as_deref(),
    );

    let on_back = create_back_callback(navigator);
    let controls = create_header_controls(&live, &auto_scroll, &show_geo_map, &entries);
    let on_select_country = create_country_callback(&selected_country);
    let (on_select_entry, on_close_inspector) = create_inspector_callbacks(&selected_entry);

    html! {
        <div class="space-y-4">
            { header_view(locale, on_back, controls) }
            { stats_cards_view(locale, &stats) }
            if *show_geo_map {
                { geo_heatmap_view(locale, &geo_dist, selected_country.as_deref(), on_select_country) }
            }
            { filters_toolbar_view(locale, &search, &status_filter, &selected_country, &available_countries) }
            { table_view(locale, &table_ref, &filtered, selected_entry.as_ref().map(|e| e.id), on_select_entry) }
            { inspector_drawer(&selected_entry, on_close_inspector) }
        </div>
    }
}

fn header_view(locale: Locale, on_back: Callback<MouseEvent>, controls: HeaderControls) -> Html {
    let HeaderControls {
        live,
        auto_scroll,
        show_geo_map,
        on_toggle_live,
        on_toggle_scroll,
        on_toggle_map,
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
                    onclick={on_toggle_map}
                    type="button"
                    class={if show_geo_map {
                        "inline-flex items-center text-xs px-3 py-1.5 rounded-lg border border-indigo-300 dark:border-indigo-700 bg-indigo-50 dark:bg-indigo-950/40 text-indigo-700 dark:text-indigo-300 font-medium"
                    } else {
                        "inline-flex items-center text-xs px-3 py-1.5 rounded-lg border border-neutral-300 dark:border-neutral-700 bg-white dark:bg-neutral-800 text-neutral-600 dark:text-neutral-300 font-medium"
                    }}
                >
                    <svg class="w-3.5 h-3.5 mr-1" fill="none" stroke="currentColor" viewBox="0 0 24 24">
                        <path stroke-linecap="round" stroke-linejoin="round" stroke-width="2" d="M3.055 11H5a2 2 0 012 2v1a2 2 0 002 2 2 2 0 012 2v2.945M8 3.935V5.5A2.5 2.5 0 0010.5 8h.5a2 2 0 012 2 2 2 0 104 0 2 2 0 012-2h1.064M15 20.488V18a2 2 0 012-2h3.064M21 12a9 9 0 11-18 0 9 9 0 0118 0z" />
                    </svg>
                    {locale.t("traffic.toggle_map")}
                </button>
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

fn geo_heatmap_view(
    locale: Locale,
    geo: &TrafficGeoDistribution,
    selected: Option<&str>,
    on_select: Callback<Option<String>>,
) -> Html {
    let on_clear_click = {
        let on_select = on_select.clone();
        Callback::from(move |_: MouseEvent| on_select.emit(None))
    };

    html! {
        <div class="bg-white dark:bg-neutral-800 rounded-lg border border-neutral-200 dark:border-neutral-700 shadow-sm p-4 space-y-3">
            <div class="flex items-center justify-between border-b dark:border-neutral-700 pb-2">
                <div>
                    <h2 class="text-sm font-bold text-neutral-900 dark:text-neutral-100 flex items-center gap-2">
                        <svg class="w-4 h-4 text-blue-600 dark:text-blue-400" fill="none" stroke="currentColor" viewBox="0 0 24 24">
                            <path stroke-linecap="round" stroke-linejoin="round" stroke-width="2" d="M3.055 11H5a2 2 0 012 2v1a2 2 0 002 2 2 2 0 104 0 2 2 0 012-2h1.064M15 20.488V18a2 2 0 012-2h3.064M21 12a9 9 0 11-18 0 9 9 0 0118 0z" />
                        </svg>
                        {locale.t("traffic.geo_map_title")}
                    </h2>
                    <p class="text-xs text-neutral-500 dark:text-neutral-400">
                        {locale.t("traffic.geo_map_desc")}
                    </p>
                </div>
                if let Some(code) = selected {
                    <div class="flex items-center gap-1.5 bg-amber-50 dark:bg-amber-950/40 text-amber-800 dark:text-amber-200 border border-amber-300 dark:border-amber-700 px-2 py-0.5 rounded text-xs">
                        <span>{locale.t("traffic.filter_country")}{": "}</span>
                        <span class="font-bold">{code}</span>
                        <button
                            type="button"
                            onclick={on_clear_click}
                            class="ml-1 text-amber-600 hover:text-amber-900 dark:hover:text-amber-100 font-bold"
                        >
                            {"✕"}
                        </button>
                    </div>
                }
            </div>

            if geo.total_resolved == 0 {
                <div class="py-8 text-center text-xs text-neutral-500 dark:text-neutral-400 bg-neutral-50 dark:bg-neutral-900/40 rounded border border-dashed border-neutral-300 dark:border-neutral-700">
                    <p>{locale.t("traffic.no_geo_data")}</p>
                </div>
            } else {
                <div class="grid grid-cols-1 lg:grid-cols-3 gap-4 items-start">
                    <div class="lg:col-span-2">
                        { render_world_map_svg(geo, selected, on_select.clone()) }
                    </div>
                    <div class="lg:col-span-1">
                        { render_top_countries_list(locale, geo, selected, on_select) }
                    </div>
                </div>
            }
        </div>
    }
}

pub fn country_heat_fill(count: u64, max_count: u64, is_selected: bool) -> &'static str {
    if is_selected {
        return "#f59e0b";
    }
    if count == 0 || max_count == 0 {
        return "#cbd5e1";
    }
    let ratio = count as f64 / max_count as f64;
    if ratio >= 0.75 {
        "#1d4ed8"
    } else if ratio >= 0.50 {
        "#3b82f6"
    } else if ratio >= 0.25 {
        "#60a5fa"
    } else {
        "#93c5fd"
    }
}

fn render_world_map_svg(
    geo: &TrafficGeoDistribution,
    selected: Option<&str>,
    on_select: Callback<Option<String>>,
) -> Html {
    let max_count = geo
        .top_countries
        .first()
        .map(|s| s.count)
        .unwrap_or(1)
        .max(1);
    html! {
        <div class="w-full bg-slate-50 dark:bg-neutral-900/50 rounded-lg p-2 border border-slate-200 dark:border-neutral-700/60 flex items-center justify-center">
            <svg viewBox="0 0 1000 500" class="w-full h-auto max-h-64 select-none" xmlns="http://www.w3.org/2000/svg">
                <g class="continents-base opacity-40">
                    { for CONTINENT_OUTLINES.iter().map(|d| html! {
                        <path d={*d} fill="#94a3b8" stroke="#64748b" stroke-width="0.8" />
                    }) }
                </g>
                <g class="countries-layer">
                    { for COUNTRY_PATHS.iter().map(|c| render_country_svg_path(c, geo, max_count, selected, on_select.clone())) }
                </g>
            </svg>
        </div>
    }
}

fn render_country_svg_path(
    item: &CountryMapItem,
    geo: &TrafficGeoDistribution,
    max_count: u64,
    selected: Option<&str>,
    on_select: Callback<Option<String>>,
) -> Html {
    let is_sel = selected == Some(item.code);
    let count = geo.country_counts.get(item.code).copied().unwrap_or(0);
    let fill = country_heat_fill(count, max_count, is_sel);
    let stroke = if is_sel { "#d97706" } else { "#475569" };
    let stroke_width = if is_sel { "2" } else { "0.8" };
    let code = item.code.to_string();
    let onclick = {
        let code = code.clone();
        Callback::from(move |_: MouseEvent| {
            if is_sel {
                on_select.emit(None);
            } else {
                on_select.emit(Some(code.clone()));
            }
        })
    };
    let title_text = format!("{}: {} requests", item.name, count);

    html! {
        <path
            d={item.path}
            fill={fill}
            stroke={stroke}
            stroke-width={stroke_width}
            class="transition-colors duration-150 cursor-pointer hover:opacity-80"
            onclick={onclick}
        >
            <title>{title_text}</title>
        </path>
    }
}

fn render_top_countries_list(
    locale: Locale,
    geo: &TrafficGeoDistribution,
    selected: Option<&str>,
    on_select: Callback<Option<String>>,
) -> Html {
    html! {
        <div class="space-y-2">
            <h3 class="text-xs font-semibold uppercase tracking-wider text-neutral-500 dark:text-neutral-400">
                {locale.t("traffic.top_countries")}
            </h3>
            <div class="space-y-1.5 max-h-56 overflow-y-auto pr-1">
                { for geo.top_countries.iter().map(|stat| render_country_stat_row(stat, selected, on_select.clone())) }
            </div>
        </div>
    }
}

fn render_country_stat_row(
    stat: &CountryTrafficStat,
    selected: Option<&str>,
    on_select: Callback<Option<String>>,
) -> Html {
    let is_sel = selected == Some(&stat.country);
    let country_code = stat.country.clone();
    let onclick = {
        let code = country_code.clone();
        Callback::from(move |_: MouseEvent| {
            if is_sel {
                on_select.emit(None);
            } else {
                on_select.emit(Some(code.clone()));
            }
        })
    };
    let width_style = format!("width: {:.1}%;", stat.percentage.clamp(2.0, 100.0));
    let row_class = if is_sel {
        "flex items-center justify-between text-xs p-1.5 rounded bg-amber-50 dark:bg-amber-950/40 border border-amber-300 dark:border-amber-700 cursor-pointer"
    } else {
        "flex items-center justify-between text-xs p-1.5 rounded hover:bg-neutral-50 dark:hover:bg-neutral-700/50 cursor-pointer"
    };

    html! {
        <div class={row_class} onclick={onclick}>
            <div class="flex items-center gap-2 flex-1 min-w-0 mr-2">
                <span class="font-mono font-bold text-xs px-1.5 py-0.5 rounded bg-neutral-200 dark:bg-neutral-700 text-neutral-800 dark:text-neutral-200">
                    {&stat.country}
                </span>
                <div class="flex-1 bg-neutral-200 dark:bg-neutral-700 h-2 rounded-full overflow-hidden">
                    <div class="bg-blue-600 dark:bg-blue-500 h-full rounded-full" style={width_style}></div>
                </div>
            </div>
            <div class="text-right whitespace-nowrap text-xs text-neutral-600 dark:text-neutral-400 font-mono">
                <span class="font-semibold text-neutral-900 dark:text-neutral-100">{stat.count}</span>
                <span class="ml-1 text-neutral-500">{"("}{stat.percentage}{"%)"}</span>
            </div>
        </div>
    }
}

fn filters_toolbar_view(
    locale: Locale,
    search: &UseStateHandle<String>,
    status_filter: &UseStateHandle<Option<StatusCategory>>,
    selected_country: &UseStateHandle<Option<String>>,
    available_countries: &[String],
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
    let on_country_change = {
        let selected_country = selected_country.clone();
        Callback::from(move |e: Event| {
            if let Some(select) = e.target_dyn_into::<HtmlSelectElement>() {
                let val = select.value();
                if val.is_empty() || val == "all" {
                    selected_country.set(None);
                } else {
                    selected_country.set(Some(val));
                }
            }
        })
    };

    let status_val = match **status_filter {
        Some(StatusCategory::Success2xx) => "2xx",
        Some(StatusCategory::Redirect3xx) => "3xx",
        Some(StatusCategory::ClientError4xx) => "4xx",
        Some(StatusCategory::ServerError5xx) => "5xx",
        None => "all",
    };

    html! {
        <div class="flex flex-col sm:flex-row gap-2 items-stretch sm:items-center flex-wrap">
            <div class="relative flex-1 min-w-[180px]">
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
                value={status_val}
                class="text-xs sm:text-sm px-3 py-2 rounded-lg border border-neutral-300 dark:border-neutral-700 bg-white dark:bg-neutral-800 text-neutral-900 dark:text-neutral-100 focus:outline-none focus:ring-2 focus:ring-blue-500"
            >
                <option value="all" selected={status_filter.is_none()}>{locale.t("traffic.filter_all_status")}</option>
                <option value="2xx" selected={matches!(**status_filter, Some(StatusCategory::Success2xx))}>{locale.t("traffic.filter_2xx")}</option>
                <option value="3xx" selected={matches!(**status_filter, Some(StatusCategory::Redirect3xx))}>{locale.t("traffic.filter_3xx")}</option>
                <option value="4xx" selected={matches!(**status_filter, Some(StatusCategory::ClientError4xx))}>{locale.t("traffic.filter_4xx")}</option>
                <option value="5xx" selected={matches!(**status_filter, Some(StatusCategory::ServerError5xx))}>{locale.t("traffic.filter_5xx")}</option>
            </select>
            <select
                onchange={on_country_change}
                value={selected_country.as_deref().unwrap_or("all").to_string()}
                class="text-xs sm:text-sm px-3 py-2 rounded-lg border border-neutral-300 dark:border-neutral-700 bg-white dark:bg-neutral-800 text-neutral-900 dark:text-neutral-100 focus:outline-none focus:ring-2 focus:ring-blue-500"
            >
                <option value="all" selected={selected_country.is_none()}>{locale.t("traffic.filter_all_countries")}</option>
                { for available_countries.iter().map(|code| html! {
                    <option value={code.clone()} selected={selected_country.as_deref() == Some(code)}>{code}</option>
                }) }
            </select>
        </div>
    }
}

fn table_view(
    locale: Locale,
    table_ref: &NodeRef,
    filtered: &[AccessLogEntry],
    selected_id: Option<u64>,
    on_select: Callback<AccessLogEntry>,
) -> Html {
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
        <div class="space-y-1.5">
            <div class="flex items-center justify-between text-xs text-neutral-500 dark:text-neutral-400 px-1">
                <span class="inline-flex items-center gap-1.5 font-medium">
                    <svg class="w-3.5 h-3.5 text-primary-500" fill="none" viewBox="0 0 24 24" stroke="currentColor">
                        <path stroke-linecap="round" stroke-linejoin="round" stroke-width="2" d="M13 16h-1v-4h-1m1-4h.01M21 12a9 9 0 11-18 0 9 9 0 0118 0z" />
                    </svg>
                    {locale.t("traffic.click_to_inspect")}
                </span>
                <span class="font-mono">{format!("{} items", filtered.len())}</span>
            </div>
            <div ref={table_ref.clone()} class="overflow-x-auto overflow-y-auto max-h-[65vh] bg-white dark:bg-neutral-800 rounded-lg border border-neutral-200 dark:border-neutral-700 shadow-sm">
                <table class="w-full text-left text-xs sm:text-sm">
                    <thead class="bg-neutral-50 dark:bg-neutral-900/60 border-b dark:border-neutral-700 text-neutral-600 dark:text-neutral-400 font-semibold sticky top-0 z-10">
                        <tr>
                            <th class="px-3 py-2">{locale.t("traffic.col_time")}</th>
                            <th class="px-3 py-2">{locale.t("traffic.col_status")}</th>
                            <th class="px-3 py-2">{locale.t("traffic.col_method")}</th>
                            <th class="px-3 py-2">{locale.t("traffic.col_path")}</th>
                            <th class="px-3 py-2">{locale.t("traffic.col_client")}</th>
                            <th class="px-3 py-2">{locale.t("traffic.col_country")}</th>
                            <th class="px-3 py-2">{locale.t("traffic.col_latency")}</th>
                            <th class="px-3 py-2">{locale.t("traffic.col_proxy")}</th>
                        </tr>
                    </thead>
                    <tbody class="divide-y divide-neutral-200 dark:divide-neutral-700 font-mono">
                        { for filtered.iter().map(|entry| render_entry_row(entry, selected_id == Some(entry.id), on_select.clone())) }
                    </tbody>
                </table>
            </div>
        </div>
    }
}

fn render_entry_row(
    entry: &AccessLogEntry,
    is_selected: bool,
    on_select: Callback<AccessLogEntry>,
) -> Html {
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
    let country_display = entry.country.as_deref().unwrap_or("-");
    let on_click = {
        let entry = entry.clone();
        Callback::from(move |_| on_select.emit(entry.clone()))
    };
    let row_cls = if is_selected {
        "bg-blue-50 dark:bg-blue-950/40 ring-1 ring-blue-500/50 cursor-pointer"
    } else {
        "hover:bg-neutral-100 dark:hover:bg-neutral-700/40 cursor-pointer transition-colors"
    };

    html! {
        <tr onclick={on_click} class={row_cls}>
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
            <td class="px-3 py-2 whitespace-nowrap text-xs">
                <span class="inline-flex items-center px-1.5 py-0.5 rounded text-xs font-semibold bg-neutral-100 dark:bg-neutral-700/80 text-neutral-800 dark:text-neutral-200 font-mono">
                    {country_display}
                </span>
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

pub use crate::format::format_latency;

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

pub fn calculate_geo_distribution(entries: &[AccessLogEntry]) -> TrafficGeoDistribution {
    let mut counts = BTreeMap::<String, u64>::new();
    let mut total_resolved = 0u64;

    for entry in entries {
        if let Some(c) = &entry.country {
            let code = c.trim().to_uppercase();
            if !code.is_empty() {
                *counts.entry(code).or_default() += 1;
                total_resolved += 1;
            }
        }
    }

    let mut list: Vec<(String, u64)> = counts.iter().map(|(k, v)| (k.clone(), *v)).collect();
    list.sort_by(|a, b| b.1.cmp(&a.1).then_with(|| a.0.cmp(&b.0)));

    let top_countries = list
        .into_iter()
        .take(10)
        .map(|(country, count)| {
            let percentage = if total_resolved > 0 {
                ((count as f64 / total_resolved as f64) * 1000.0).round() / 10.0
            } else {
                0.0
            };
            CountryTrafficStat {
                country,
                count,
                percentage,
            }
        })
        .collect();

    TrafficGeoDistribution {
        total_resolved,
        top_countries,
        country_counts: counts,
    }
}

pub fn extract_available_countries(entries: &[AccessLogEntry]) -> Vec<String> {
    let mut set = BTreeSet::new();
    for entry in entries {
        if let Some(c) = &entry.country {
            let code = c.trim().to_uppercase();
            if !code.is_empty() {
                set.insert(code);
            }
        }
    }
    set.into_iter().collect()
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

fn matches_entry_country(entry: &AccessLogEntry, country: Option<&str>) -> bool {
    let Some(expected) = country else {
        return true;
    };
    entry
        .country
        .as_deref()
        .is_some_and(|c| c.eq_ignore_ascii_case(expected))
}

pub fn filter_traffic_logs(
    entries: &[AccessLogEntry],
    search: &str,
    status: Option<StatusCategory>,
    proxy_id: Option<ShortId>,
    country: Option<&str>,
) -> Vec<AccessLogEntry> {
    let query = search.trim().to_lowercase();
    let country_query = country.map(str::trim).filter(|c| !c.is_empty());
    entries
        .iter()
        .filter(|e| {
            matches_status_cat(e, status)
                && matches_proxy(e, proxy_id)
                && matches_search(e, &query)
                && matches_entry_country(e, country_query)
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

    fn sample_log(
        status: u16,
        latency_ms: f64,
        uri: &str,
        country: Option<&str>,
    ) -> AccessLogEntry {
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
            country: country.map(|c| c.to_string()),
            request_headers: Vec::new(),
            response_headers: Vec::new(),
            request_body: None,
            response_body: None,
            content_type: None,
            is_websocket: None,
        }
    }

    #[test]
    fn test_calculate_stats() {
        let logs = vec![
            sample_log(200, 10.0, "/api/a", Some("TR")),
            sample_log(301, 20.0, "/api/b", Some("US")),
            sample_log(404, 30.0, "/api/c", None),
            sample_log(500, 40.0, "/api/d", Some("DE")),
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
    fn test_calculate_geo_distribution() {
        let logs = vec![
            sample_log(200, 10.0, "/api/a", Some("TR")),
            sample_log(200, 12.0, "/api/b", Some("TR")),
            sample_log(200, 15.0, "/api/c", Some("US")),
            sample_log(500, 18.0, "/api/d", None),
        ];
        let geo = calculate_geo_distribution(&logs);
        assert_eq!(geo.total_resolved, 3);
        assert_eq!(geo.top_countries.len(), 2);
        assert_eq!(geo.top_countries[0].country, "TR");
        assert_eq!(geo.top_countries[0].count, 2);
        assert_eq!(geo.top_countries[0].percentage, 66.7);
        assert_eq!(geo.top_countries[1].country, "US");
        assert_eq!(geo.top_countries[1].count, 1);
        assert_eq!(geo.top_countries[1].percentage, 33.3);
    }

    #[test]
    fn test_extract_available_countries() {
        let logs = vec![
            sample_log(200, 10.0, "/api/a", Some("TR")),
            sample_log(200, 12.0, "/api/b", Some("US")),
            sample_log(200, 15.0, "/api/c", Some("TR")),
            sample_log(500, 18.0, "/api/d", None),
        ];
        let countries = extract_available_countries(&logs);
        assert_eq!(countries, vec!["TR".to_string(), "US".to_string()]);
    }

    #[test]
    fn test_filter_traffic_logs() {
        let logs = vec![
            sample_log(200, 10.0, "/health", Some("TR")),
            sample_log(404, 20.0, "/missing", Some("US")),
        ];
        let filtered = filter_traffic_logs(&logs, "health", None, None, None);
        assert_eq!(filtered.len(), 1);
        assert_eq!(filtered[0].status, 200);

        let filtered_status =
            filter_traffic_logs(&logs, "", Some(StatusCategory::ClientError4xx), None, None);
        assert_eq!(filtered_status.len(), 1);
        assert_eq!(filtered_status[0].uri, "/missing");

        let filtered_country = filter_traffic_logs(&logs, "", None, None, Some("tr"));
        assert_eq!(filtered_country.len(), 1);
        assert_eq!(filtered_country[0].country.as_deref(), Some("TR"));

        let filtered_none = filter_traffic_logs(&logs, "", None, None, Some("de"));
        assert_eq!(filtered_none.len(), 0);
    }

    #[test]
    fn test_country_heat_fill() {
        assert_eq!(country_heat_fill(10, 10, true), "#f59e0b");
        assert_eq!(country_heat_fill(0, 10, false), "#cbd5e1");
        assert_eq!(country_heat_fill(8, 10, false), "#1d4ed8");
        assert_eq!(country_heat_fill(6, 10, false), "#3b82f6");
        assert_eq!(country_heat_fill(3, 10, false), "#60a5fa");
        assert_eq!(country_heat_fill(1, 10, false), "#93c5fd");
    }

    #[test]
    fn test_format_latency() {
        assert_eq!(format_latency(0.42), "0.42 ms");
        assert_eq!(format_latency(15.2), "15.2 ms");
        assert_eq!(format_latency(1500.0), "1.50 s");
    }
}
