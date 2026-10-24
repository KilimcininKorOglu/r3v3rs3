//! The metrics card of an app: the health, the latest CPU, memory, network and request values,
//! and the sparklines of the last hours. The collector records a sample every 30 seconds.

use super::settings::fetch_json;
use crate::API_ENDPOINT;
use crate::format::{format_bytes, format_time};
use crate::i18n::use_locale;
use gloo_net::http::Request;
use gloo_timers::callback::Interval;
use r3v3rs3_api::container::{AppResourceDiagnostics, ResourceHealthStatus};
use r3v3rs3_api::i18n::Locale;
use r3v3rs3_api::id::ShortId;
use r3v3rs3_api::platform::AppMetricsPoint;
use wasm_bindgen_futures::spawn_local;
use yew::prelude::*;

/// How often the card reads the samples.
const POLL_MS: u32 = 10_000;

type Loaded = Option<Result<Vec<AppMetricsPoint>, String>>;
type DiagLoaded = Option<AppResourceDiagnostics>;

#[derive(Properties, PartialEq)]
pub struct Props {
    pub id: ShortId,
}

#[function_component(AppMetrics)]
pub fn app_metrics(props: &Props) -> Html {
    let locale = use_locale();
    let points: UseStateHandle<Loaded> = use_state(|| None);
    let diagnostics: UseStateHandle<DiagLoaded> = use_state(|| None);
    let id = props.id;
    {
        let points = points.clone();
        let diagnostics = diagnostics.clone();
        use_effect_with(id, move |&id| {
            let load = move || {
                let points = points.clone();
                let diagnostics = diagnostics.clone();
                spawn_local(async move {
                    let result = fetch_json::<Vec<AppMetricsPoint>>(
                        Locale::En,
                        Request::get(&format!("{API_ENDPOINT}/apps/{id}/metrics")),
                    )
                    .await
                    .map_err(|err| err.to_string());
                    points.set(Some(result));

                    if let Ok(diag) = fetch_json::<AppResourceDiagnostics>(
                        Locale::En,
                        Request::get(&format!("{API_ENDPOINT}/apps/{id}/diagnostics")),
                    )
                    .await
                    {
                        diagnostics.set(Some(diag));
                    }
                });
            };
            load();
            let interval = Interval::new(POLL_MS, load);
            move || drop(interval)
        });
    }
    let body = match points.as_ref() {
        None => html! {},
        Some(Err(err)) => {
            html! { <p class="text-sm text-red-600 dark:text-red-500">{err.clone()}</p> }
        }
        Some(Ok(samples)) => card_body(locale, samples, diagnostics.as_ref()),
    };
    html! {
        <div class="mt-4 bg-white dark:bg-neutral-800 shadow-sm p-5 border border-neutral-300 dark:border-neutral-700 rounded-md">
            <h2 class="text-lg font-semibold text-neutral-900 dark:text-neutral-200">
                {locale.t("metrics.title")}
            </h2>
            { body }
        </div>
    }
}

/// The samples, the latest values and the sparklines, or the note of an app without samples.
fn card_body(
    locale: Locale,
    samples: &[AppMetricsPoint],
    diag: Option<&AppResourceDiagnostics>,
) -> Html {
    let Some(latest) = samples.last() else {
        return html! { <p class="text-sm text-neutral-500 dark:text-neutral-400 mt-2">
            {locale.t("metrics.waiting")}
        </p> };
    };
    html! {
        <>
            <p class="text-sm text-neutral-500 dark:text-neutral-400 mt-1">
                {locale.t("metrics.updated")}{" "}
                {format_time(latest.ts.max(0) as u64)}
            </p>
            <dl class="grid gap-3 mt-3 sm:grid-cols-2 xl:grid-cols-4 text-sm">
                { value_cell(locale, "metrics.cpu", &format!("{:.1} %", latest.cpu)) }
                { value_cell(locale, "metrics.memory", &format_bytes(latest.memory)) }
                { value_cell(locale, "metrics.network", &format_bytes(latest.rx_bytes + latest.tx_bytes)) }
                { value_cell(locale, "metrics.requests", &latest.requests.to_string()) }
            </dl>
            <div class="grid gap-4 mt-4 sm:grid-cols-2">
                { sparkline(locale, "metrics.cpu", samples.iter().map(|point| point.cpu)) }
                { sparkline(locale, "metrics.requests", samples.iter().map(|point| point.requests as f64)) }
            </div>
            { diag_section_html(locale, diag) }
        </>
    }
}

fn diag_section_html(locale: Locale, diag: Option<&AppResourceDiagnostics>) -> Html {
    let Some(diag) = diag else {
        return html! {};
    };
    diagnostics_section(locale, diag)
}

fn status_badge(locale: Locale, status: ResourceHealthStatus) -> Html {
    let (key, bg, fg) = match status {
        ResourceHealthStatus::Healthy => (
            "diagnostics.status_healthy",
            "bg-emerald-100 dark:bg-emerald-950",
            "text-emerald-800 dark:text-emerald-300",
        ),
        ResourceHealthStatus::ThrottledWarning => (
            "diagnostics.status_throttled_warning",
            "bg-amber-100 dark:bg-amber-950",
            "text-amber-800 dark:text-amber-300",
        ),
        ResourceHealthStatus::ThrottledCritical => (
            "diagnostics.status_throttled_critical",
            "bg-red-100 dark:bg-red-950",
            "text-red-800 dark:text-red-300",
        ),
        ResourceHealthStatus::OomRisk => (
            "diagnostics.status_oom_risk",
            "bg-orange-100 dark:bg-orange-950",
            "text-orange-800 dark:text-orange-300",
        ),
        ResourceHealthStatus::OomKilled => (
            "diagnostics.status_oom_killed",
            "bg-red-200 dark:bg-red-900",
            "text-red-900 dark:text-red-100",
        ),
    };
    html! {
        <span class={format!("inline-flex items-center px-2.5 py-0.5 rounded-full text-xs font-medium {bg} {fg}")}>
            {locale.t(key)}
        </span>
    }
}

fn diagnostics_section(locale: Locale, diag: &AppResourceDiagnostics) -> Html {
    if !diag.running {
        return html! {
            <div class="mt-5 pt-4 border-t border-neutral-200 dark:border-neutral-700">
                <h3 class="text-sm font-semibold text-neutral-900 dark:text-neutral-200 mb-2">
                    {locale.t("diagnostics.title")}
                </h3>
                <p class="text-xs text-neutral-500 dark:text-neutral-400">
                    {locale.t("diagnostics.not_running")}
                </p>
            </div>
        };
    }

    let periods_text = format!(
        "{} / {}",
        diag.cpu_throttled_periods, diag.cpu_total_periods
    );
    let time_text = format!("{} ms", diag.cpu_throttled_time_ms);
    let mem_sat_text = format!("{:.1} %", diag.memory_percent);

    html! {
        <div class="mt-5 pt-4 border-t border-neutral-200 dark:border-neutral-700">
            <div class="flex items-center justify-between mb-3">
                <h3 class="text-sm font-semibold text-neutral-900 dark:text-neutral-200">
                    {locale.t("diagnostics.title")}
                </h3>
                { status_badge(locale, diag.status) }
            </div>
            <dl class="grid gap-3 sm:grid-cols-2 xl:grid-cols-4 text-sm">
                { value_cell(locale, "diagnostics.cpu_throttled_percent", &format!("{:.1} %", diag.cpu_throttled_percent)) }
                { value_cell(locale, "diagnostics.throttled_periods", &periods_text) }
                { value_cell(locale, "diagnostics.cpu_throttled_time", &time_text) }
                { value_cell(locale, "diagnostics.memory_saturation", &mem_sat_text) }
                { value_cell(locale, "diagnostics.fail_count", &diag.memory_fail_count.to_string()) }
            </dl>
            { recommendation_box(diag.recommendation.as_deref()) }
        </div>
    }
}

fn recommendation_box(rec: Option<&str>) -> Html {
    let Some(text) = rec else {
        return html! {};
    };
    html! {
        <div class="mt-3 p-3 bg-amber-50 dark:bg-amber-950/40 border border-amber-200 dark:border-amber-800 rounded text-xs text-amber-800 dark:text-amber-200">
            <span class="font-semibold">{"⚠️ "}</span>
            { text }
        </div>
    }
}

/// One label and value of the latest sample.
fn value_cell(locale: Locale, key: &'static str, value: &str) -> Html {
    html! {
        <div>
            <dt class="text-neutral-500 dark:text-neutral-400">{locale.t(key)}</dt>
            <dd class="font-medium text-neutral-900 dark:text-neutral-200">{value}</dd>
        </div>
    }
}

/// A small polyline of the values, without axes. The title carries the range.
fn sparkline(locale: Locale, key: &'static str, values: impl Iterator<Item = f64>) -> Html {
    let values: Vec<f64> = values.collect();
    let max = values.iter().cloned().fold(0.0, f64::max).max(1.0);
    let count = values.len();
    let points = values
        .iter()
        .enumerate()
        .map(|(index, value)| {
            let x = if count <= 1 {
                100.0
            } else {
                index as f64 / (count - 1) as f64 * 100.0
            };
            let y = 24.0 - value / max * 22.0;
            format!("{x:.1},{y:.1}")
        })
        .collect::<Vec<_>>()
        .join(" ");
    html! {
        <div>
            <p class="text-xs text-neutral-500 dark:text-neutral-400 mb-1">{locale.t(key)}</p>
            <svg viewBox="0 0 100 24" preserveAspectRatio="none" class="w-full h-12 rounded bg-neutral-100 dark:bg-neutral-900">
                <polyline fill="none" stroke="currentColor" stroke-width="1.5" points={points}
                    class="text-neutral-500 dark:text-neutral-300" />
            </svg>
        </div>
    }
}
