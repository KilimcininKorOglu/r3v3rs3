//! The metrics card of an app: the health, the latest CPU, memory, network and request values,
//! and the sparklines of the last hours. The collector records a sample every 30 seconds.

use super::settings::fetch_json;
use crate::API_ENDPOINT;
use crate::format::format_time;
use crate::i18n::use_locale;
use gloo_net::http::Request;
use gloo_timers::callback::Interval;
use r3v3rs3_api::i18n::Locale;
use r3v3rs3_api::id::ShortId;
use r3v3rs3_api::platform::AppMetricsPoint;
use wasm_bindgen_futures::spawn_local;
use yew::prelude::*;

/// How often the card reads the samples.
const POLL_MS: u32 = 10_000;

type Loaded = Option<Result<Vec<AppMetricsPoint>, String>>;

#[derive(Properties, PartialEq)]
pub struct Props {
    pub id: ShortId,
}

#[function_component(AppMetrics)]
pub fn app_metrics(props: &Props) -> Html {
    let locale = use_locale();
    let points: UseStateHandle<Loaded> = use_state(|| None);
    let id = props.id;
    {
        let points = points.clone();
        use_effect_with(id, move |&id| {
            let load = move || {
                let points = points.clone();
                spawn_local(async move {
                    let result = fetch_json::<Vec<AppMetricsPoint>>(
                        Locale::En,
                        Request::get(&format!("{API_ENDPOINT}/apps/{id}/metrics")),
                    )
                    .await
                    .map_err(|err| err.to_string());
                    points.set(Some(result));
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
        Some(Ok(samples)) => card_body(locale, samples),
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
fn card_body(locale: Locale, samples: &[AppMetricsPoint]) -> Html {
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
        </>
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

/// A size in KiB or MiB.
fn format_bytes(size: u64) -> String {
    if size >= 1024 * 1024 {
        format!("{:.1} MiB", size as f64 / (1024.0 * 1024.0))
    } else {
        format!("{:.1} KiB", size as f64 / 1024.0)
    }
}
