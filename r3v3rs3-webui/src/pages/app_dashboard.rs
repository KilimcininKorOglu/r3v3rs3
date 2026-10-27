//! The dedicated dashboard and unified overview console for an application.

use super::Route;
use super::app_deployments::back_header;
use super::app_list::{FAST_REFRESH_MS, SLOW_REFRESH_MS, is_unfinished};
use super::app_metrics::AppMetrics;
use super::app_services::AppServices;
use super::settings::{failure_box, fetch_json};
use crate::API_ENDPOINT;
use crate::auth::use_ensure_auth;
use crate::components::app_tabs::{AppTab, AppTabs};
use crate::dialog;
use crate::format::{format_bytes, format_time, format_uptime};
use crate::i18n::use_locale;
use crate::store::SessionStore;
use gloo_net::http::Request;
use gloo_timers::callback::Interval;
use r3v3rs3_api::i18n::Locale;
use r3v3rs3_api::id::ShortId;
use r3v3rs3_api::platform::{AppDashboardSummary, DeploymentEntry, DeploymentStatus};
use wasm_bindgen_futures::spawn_local;
use yew::prelude::*;
use yew_router::prelude::*;
use yewdux::prelude::*;

type Loaded = Option<Result<AppDashboardSummary, String>>;

#[derive(Properties, PartialEq)]
pub struct Props {
    pub id: ShortId,
}

fn is_summary_busy(data: &Loaded) -> bool {
    matches!(data, Some(Ok(summary)) if summary.latest_deployment.as_ref().is_some_and(|d| is_unfinished(d.status)))
}

fn summary_and_error(data: &Loaded) -> (Option<AppDashboardSummary>, Option<String>) {
    match data {
        Some(Ok(s)) => (Some(s.clone()), None),
        Some(Err(e)) => (None, Some(e.clone())),
        None => (None, None),
    }
}

#[hook]
fn use_dashboard_poll(
    locale: Locale,
    id: ShortId,
    data: &UseStateHandle<Loaded>,
    signed_in: bool,
    busy: bool,
    reload: u32,
) {
    let data = data.clone();
    use_effect_with((signed_in, busy, reload), move |&(signed_in, busy, _)| {
        let interval = signed_in.then(|| {
            let load = move || load_summary(locale, id, data.clone());
            load();
            let interval_ms = if busy {
                FAST_REFRESH_MS
            } else {
                SLOW_REFRESH_MS
            };
            Interval::new(interval_ms, load)
        });
        move || drop(interval)
    });
}

fn render_loading_skeleton(locale: Locale, navigator: &Navigator) -> Html {
    html! {
        <div class="space-y-6 animate-pulse">
            { back_header(locale, navigator, locale.t("apps.dashboard").to_string()) }
            <div class="grid grid-cols-1 md:grid-cols-4 gap-4">
                <div class="h-28 bg-white dark:bg-neutral-800 rounded-lg border border-neutral-200 dark:border-neutral-700 p-4"></div>
                <div class="h-28 bg-white dark:bg-neutral-800 rounded-lg border border-neutral-200 dark:border-neutral-700 p-4"></div>
                <div class="h-28 bg-white dark:bg-neutral-800 rounded-lg border border-neutral-200 dark:border-neutral-700 p-4"></div>
                <div class="h-28 bg-white dark:bg-neutral-800 rounded-lg border border-neutral-200 dark:border-neutral-700 p-4"></div>
            </div>
            <div class="grid grid-cols-1 lg:grid-cols-3 gap-6">
                <div class="lg:col-span-2 h-72 bg-white dark:bg-neutral-800 rounded-lg border border-neutral-200 dark:border-neutral-700 p-4"></div>
                <div class="h-72 bg-white dark:bg-neutral-800 rounded-lg border border-neutral-200 dark:border-neutral-700 p-4"></div>
            </div>
        </div>
    }
}

#[function_component(AppDashboard)]
pub fn app_dashboard(props: &Props) -> Html {
    use_ensure_auth();
    let locale = use_locale();
    let (session, _) = use_store::<SessionStore>();
    let Some(navigator) = use_navigator() else {
        return html!();
    };

    let id = props.id;
    let data = use_state(|| Loaded::None);
    let failure = use_state(|| Option::<String>::None);
    let reload = use_state(|| 0_u32);

    let busy = is_summary_busy(&data);
    let signed_in = session.info.is_some();
    use_dashboard_poll(locale, id, &data, signed_in, busy, *reload);

    let can_edit = session.can_edit();
    let (summary, error) = summary_and_error(&data);
    let active_error = error.or_else(|| (*failure).clone());

    if let Some(s) = summary {
        html! {
            <div class="space-y-6">
                if let Some(err) = active_error {
                    { failure_box(&err) }
                }
                { render_dashboard_content(DashboardParams {
                    id,
                    locale,
                    can_edit,
                    busy,
                    summary: s,
                    failure: failure.clone(),
                    reload: reload.clone(),
                    navigator: navigator.clone(),
                }) }
            </div>
        }
    } else if let Some(err) = active_error {
        html! {
            <div class="space-y-6">
                { back_header(locale, &navigator, locale.t("apps.dashboard").to_string()) }
                { failure_box(&err) }
            </div>
        }
    } else {
        render_loading_skeleton(locale, &navigator)
    }
}

fn load_summary(locale: Locale, id: ShortId, state: UseStateHandle<Loaded>) {
    spawn_local(async move {
        let path = format!("{API_ENDPOINT}/apps/{id}/dashboard");
        state.set(Some(
            fetch_json::<AppDashboardSummary>(locale, Request::get(&path)).await,
        ));
    });
}

struct DashboardParams {
    id: ShortId,
    locale: Locale,
    can_edit: bool,
    busy: bool,
    summary: AppDashboardSummary,
    failure: UseStateHandle<Option<String>>,
    reload: UseStateHandle<u32>,
    navigator: Navigator,
}

fn render_dashboard_content(p: DashboardParams) -> Html {
    let status_str = p
        .summary
        .running_deployment
        .as_ref()
        .map(|d| match d.status {
            DeploymentStatus::Running => "running",
            DeploymentStatus::Deploying | DeploymentStatus::Building => "deploying",
            DeploymentStatus::Failed => "failed",
            _ => "stopped",
        });

    html! {
        <>
            <AppTabs
                id={p.id}
                name={p.summary.app.name.to_string()}
                active={AppTab::Overview}
                status={status_str.map(ToString::to_string)}
                domains={p.summary.app.spec.domains.clone()}
            />

            { render_quick_actions(&p) }

            if let Some(diag) = &p.summary.diagnostics {
                { render_diagnostics_alert(p.locale, diag) }
            }

            <div class="grid grid-cols-1 lg:grid-cols-2 gap-6">
                { render_telemetry_card(p.locale, &p.summary) }
                { render_deployment_card(p.locale, &p.summary) }
            </div>

            <AppMetrics id={p.id} />

            if !p.summary.services.is_empty() {
                <AppServices id={p.id} locale={p.locale} />
            }

            { render_logs_preview(p.locale, p.id, &p.summary.logs_preview, p.summary.log_size_bytes, &p.navigator) }
        </>
    }
}

fn render_quick_actions(p: &DashboardParams) -> Html {
    let on_deploy = deploy_click_callback(p);
    let on_restart = restart_click_callback(p);
    let on_rollback = rollback_click_callback(p);

    let (nav_term, nav_logs, id) = (p.navigator.clone(), p.navigator.clone(), p.id);
    let on_terminal =
        Callback::from(move |_: MouseEvent| nav_term.push(&Route::AppTerminal { id }));
    let on_logs = Callback::from(move |_: MouseEvent| nav_logs.push(&Route::AppLog { id }));

    html! {
        <div class="flex flex-wrap items-center justify-between gap-3 p-4 bg-white dark:bg-neutral-800 rounded-lg border border-neutral-200 dark:border-neutral-700 shadow-sm">
            <div class="flex flex-wrap items-center gap-2">
                if p.can_edit && !p.busy {
                    <button
                        type="button"
                        onclick={on_deploy}
                        class="inline-flex items-center gap-1.5 text-white bg-blue-600 hover:bg-blue-700 font-medium rounded-lg text-sm px-3.5 py-2 shadow-sm"
                    >
                        <svg class="w-4 h-4" fill="none" stroke="currentColor" viewBox="0 0 24 24">
                            <path stroke-linecap="round" stroke-linejoin="round" stroke-width="2" d="M14.752 11.168l-3.197-2.132A1 1 0 0010 9.87v4.263a1 1 0 001.555.832l3.197-2.132a1 1 0 000-1.664z" />
                        </svg>
                        {p.locale.t("apps.deploy")}
                    </button>
                    <button
                        type="button"
                        onclick={on_restart}
                        class="inline-flex items-center gap-1.5 text-neutral-700 dark:text-neutral-200 bg-white dark:bg-neutral-700 border border-neutral-300 dark:border-neutral-600 hover:bg-neutral-50 hover:dark:bg-neutral-600 font-medium rounded-lg text-sm px-3.5 py-2"
                    >
                        <svg class="w-4 h-4" fill="none" stroke="currentColor" viewBox="0 0 24 24">
                            <path stroke-linecap="round" stroke-linejoin="round" stroke-width="2" d="M4 4v5h.582m15.356 2A8.001 8.001 0 004.582 9m0 0H9m11 11v-5h-.581m0 0a8.003 8.003 0 01-15.357-2m15.357 2H15" />
                        </svg>
                        {p.locale.t("apps.restart")}
                    </button>
                }
                if p.can_edit && p.summary.running_deployment.is_some() {
                    <button
                        type="button"
                        onclick={on_rollback}
                        class="inline-flex items-center gap-1.5 text-amber-700 dark:text-amber-300 bg-amber-50 dark:bg-amber-950/40 border border-amber-200 dark:border-amber-800 hover:bg-amber-100 hover:dark:bg-amber-900/60 font-medium rounded-lg text-sm px-3.5 py-2"
                    >
                        <svg class="w-4 h-4" fill="none" stroke="currentColor" viewBox="0 0 24 24">
                            <path stroke-linecap="round" stroke-linejoin="round" stroke-width="2" d="M12.066 11.2a1 1 0 000 1.6l5.334 4A1 1 0 0019 16V8a1 1 0 00-1.6-.8l-5.334 4zM4.066 11.2a1 1 0 000 1.6l5.334 4A1 1 0 0011 16V8a1 1 0 00-1.6-.8l-5.334 4z" />
                        </svg>
                        {p.locale.t("apps.rollback")}
                    </button>
                }
            </div>
            <div class="flex items-center gap-2">
                <button
                    type="button"
                    onclick={on_terminal}
                    class="inline-flex items-center gap-1 text-neutral-600 dark:text-neutral-300 hover:text-neutral-900 hover:dark:text-white px-3 py-1.5 rounded-lg border border-neutral-300 dark:border-neutral-600 text-sm font-medium"
                >
                    <img src="/assets/icons/terminal.svg" class="w-4 h-4 mr-1" />
                    {p.locale.t("apps.tabs_terminal")}
                </button>
                <button
                    type="button"
                    onclick={on_logs}
                    class="inline-flex items-center gap-1 text-neutral-600 dark:text-neutral-300 hover:text-neutral-900 hover:dark:text-white px-3 py-1.5 rounded-lg border border-neutral-300 dark:border-neutral-600 text-sm font-medium"
                >
                    <img src="/assets/icons/description.svg" class="w-4 h-4 mr-1" />
                    {p.locale.t("apps.tabs_logs")}
                </button>
            </div>
        </div>
    }
}

fn deploy_click_callback(p: &DashboardParams) -> Callback<MouseEvent> {
    let (locale, id, name, failure, reload) = (
        p.locale,
        p.id,
        p.summary.app.name.clone(),
        p.failure.clone(),
        p.reload.clone(),
    );
    Callback::from(move |e: MouseEvent| {
        e.prevent_default();
        let question = locale.tf("apps.confirm_deploy", &[("name", name.as_str())]);
        let (failure, reload) = (failure.clone(), reload.clone());
        dialog::confirm_then(locale, question, async move {
            let path = format!("{API_ENDPOINT}/apps/{id}/deploy");
            match fetch_json::<DeploymentEntry>(locale, Request::post(&path)).await {
                Ok(_) => failure.set(None),
                Err(err) => failure.set(Some(err)),
            }
            reload.set(*reload + 1);
        });
    })
}

fn restart_click_callback(p: &DashboardParams) -> Callback<MouseEvent> {
    let (locale, id, name, failure, reload) = (
        p.locale,
        p.id,
        p.summary.app.name.clone(),
        p.failure.clone(),
        p.reload.clone(),
    );
    Callback::from(move |e: MouseEvent| {
        e.prevent_default();
        let question = locale.tf("apps.confirm_restart", &[("name", name.as_str())]);
        let (failure, reload) = (failure.clone(), reload.clone());
        dialog::confirm_then(locale, question, async move {
            let path = format!("{API_ENDPOINT}/apps/{id}/restart");
            match Request::post(&path).send().await {
                Ok(_) => failure.set(None),
                Err(err) => failure.set(Some(err.to_string())),
            }
            reload.set(*reload + 1);
        });
    })
}

fn rollback_click_callback(p: &DashboardParams) -> Callback<MouseEvent> {
    let (locale, name, failure, reload, running) = (
        p.locale,
        p.summary.app.name.clone(),
        p.failure.clone(),
        p.reload.clone(),
        p.summary.running_deployment.clone(),
    );
    Callback::from(move |e: MouseEvent| {
        e.prevent_default();
        let Some(running_dep) = &running else {
            return;
        };
        let dep_id = running_dep.id;
        let question = locale.tf("apps.confirm_rollback", &[("name", name.as_str())]);
        let (failure, reload) = (failure.clone(), reload.clone());
        dialog::confirm_then(locale, question, async move {
            let path = format!("{API_ENDPOINT}/deployments/{dep_id}/rollback");
            match fetch_json::<DeploymentEntry>(locale, Request::post(&path)).await {
                Ok(_) => failure.set(None),
                Err(err) => failure.set(Some(err)),
            }
            reload.set(*reload + 1);
        });
    })
}

fn render_diagnostics_alert(
    locale: Locale,
    diag: &r3v3rs3_api::container::AppResourceDiagnostics,
) -> Html {
    if !diag.oom_killed && diag.cpu_throttled_percent < 20.0 {
        return html!();
    }
    let is_crit = diag.oom_killed || diag.cpu_throttled_percent >= 50.0;
    let (bg_cls, text_cls, title_key) = if is_crit {
        (
            "bg-red-50 dark:bg-red-950/40 border-red-200 dark:border-red-800",
            "text-red-800 dark:text-red-300",
            "apps.oom_warning",
        )
    } else {
        (
            "bg-amber-50 dark:bg-amber-950/40 border-amber-200 dark:border-amber-800",
            "text-amber-800 dark:text-amber-300",
            "apps.cfs_throttled",
        )
    };

    html! {
        <div class={classes!("p-4", "rounded-lg", "border", bg_cls, text_cls, "flex", "items-start", "gap-3")}>
            <svg class="w-5 h-5 flex-shrink-0 mt-0.5" fill="none" stroke="currentColor" viewBox="0 0 24 24">
                <path stroke-linecap="round" stroke-linejoin="round" stroke-width="2" d="M12 9v2m0 4h.01m-6.938 4h13.856c1.54 0 2.502-1.667 1.732-3L13.732 4c-.77-1.333-2.694-1.333-3.464 0L3.34 16c-.77 1.333.192 3 1.732 3z" />
            </svg>
            <div>
                <h4 class="font-semibold text-sm">{locale.t(title_key)}</h4>
                <p class="text-xs mt-1">
                    {format!("CFS throttled: {:.1}%, OOM Killed: {}", diag.cpu_throttled_percent, diag.oom_killed)}
                    if let Some(rec) = &diag.recommendation {
                        <span class="block mt-0.5 italic">{rec}</span>
                    }
                </p>
            </div>
        </div>
    }
}

fn render_telemetry_card(locale: Locale, summary: &AppDashboardSummary) -> Html {
    let latest_metric = summary.metrics.last();
    let cpu_val = latest_metric.map(|m| m.cpu).unwrap_or(0.0);
    let mem_bytes = latest_metric.map(|m| m.memory).unwrap_or(0);
    let mem_limit = summary.app.spec.limits.memory_bytes.unwrap_or(0);
    let mem_text = if mem_limit > 0 {
        format!("{} / {}", format_bytes(mem_bytes), format_bytes(mem_limit))
    } else {
        format_bytes(mem_bytes)
    };
    let uptime_str = summary
        .uptime_seconds
        .map(format_uptime)
        .unwrap_or_else(|| "-".to_string());

    html! {
        <div class="p-5 bg-white dark:bg-neutral-800 rounded-lg border border-neutral-200 dark:border-neutral-700 shadow-sm space-y-4">
            <h3 class="font-semibold text-neutral-900 dark:text-neutral-100 flex items-center justify-between">
                <span>{locale.t("apps.live_status")}</span>
                <span class="text-xs font-normal text-neutral-500 dark:text-neutral-400">
                    {format!("{}: {}", locale.t("apps.uptime"), uptime_str)}
                </span>
            </h3>

            <div class="space-y-3">
                <div>
                    <div class="flex justify-between text-xs font-medium text-neutral-600 dark:text-neutral-300 mb-1">
                        <span>{locale.t("apps.cpu_usage")}</span>
                        <span>{format!("{cpu_val:.1}%")}</span>
                    </div>
                    <div class="w-full bg-neutral-200 dark:bg-neutral-700 rounded-full h-2">
                        <div class="bg-blue-600 h-2 rounded-full" style={format!("width: {:.1}%", cpu_val.min(100.0))}></div>
                    </div>
                </div>

                <div>
                    <div class="flex justify-between text-xs font-medium text-neutral-600 dark:text-neutral-300 mb-1">
                        <span>{locale.t("apps.memory_usage")}</span>
                        <span>{mem_text}</span>
                    </div>
                    <div class="w-full bg-neutral-200 dark:bg-neutral-700 rounded-full h-2">
                        if mem_limit > 0 {
                            <div class="bg-emerald-500 h-2 rounded-full" style={format!("width: {:.1}%", ((mem_bytes as f64 / mem_limit as f64) * 100.0).min(100.0))}></div>
                        } else {
                            <div class="bg-emerald-500 h-2 rounded-full" style="width: 25%"></div>
                        }
                    </div>
                </div>

                if let Some(m) = latest_metric {
                    <div class="pt-2 border-t border-neutral-100 dark:border-neutral-700/60 flex justify-between text-xs text-neutral-500 dark:text-neutral-400">
                        <span>{locale.t("apps.network_io")}</span>
                        <span>{format!("RX: {} | TX: {}", format_bytes(m.rx_bytes), format_bytes(m.tx_bytes))}</span>
                    </div>
                }
            </div>
        </div>
    }
}

fn render_deployment_card(locale: Locale, summary: &AppDashboardSummary) -> Html {
    let dep = summary
        .running_deployment
        .as_ref()
        .or(summary.latest_deployment.as_ref());
    let commit_str = dep
        .as_ref()
        .and_then(|d| d.commit_sha.as_ref())
        .map(|c| {
            let short = if c.len() > 7 { &c[..7] } else { c.as_str() };
            short.to_string()
        })
        .unwrap_or_else(|| "-".to_string());

    let trigger_str = dep.as_ref().map(|d| d.trigger.as_str()).unwrap_or("-");
    let msg_str = dep
        .as_ref()
        .and_then(|d| d.message.as_ref())
        .cloned()
        .unwrap_or_default();
    let author_str = dep.as_ref().map(|d| d.username.clone()).unwrap_or_default();
    let target_node = summary.app.target.to_string();

    html! {
        <div class="p-5 bg-white dark:bg-neutral-800 rounded-lg border border-neutral-200 dark:border-neutral-700 shadow-sm space-y-4">
            <h3 class="font-semibold text-neutral-900 dark:text-neutral-100 flex items-center justify-between">
                <span>{locale.t("apps.active_deployment")}</span>
                <span class="text-xs px-2 py-0.5 rounded bg-neutral-100 dark:bg-neutral-700 text-neutral-600 dark:text-neutral-300 font-mono">
                    {format!("Node: {target_node}")}
                </span>
            </h3>

            <div class="space-y-2 text-sm">
                <div class="flex items-center gap-2">
                    <span class="text-neutral-500 dark:text-neutral-400 text-xs w-16">{"Commit:"}</span>
                    <span class="font-mono text-xs bg-neutral-100 dark:bg-neutral-700 px-1.5 py-0.5 rounded font-semibold text-blue-600 dark:text-blue-400">
                        {commit_str}
                    </span>
                    <span class="text-xs text-neutral-400">{format!("({trigger_str})")}</span>
                </div>

                if !msg_str.is_empty() {
                    <div class="flex items-start gap-2">
                        <span class="text-neutral-500 dark:text-neutral-400 text-xs w-16">{"Message:"}</span>
                        <span class="text-xs text-neutral-700 dark:text-neutral-200 italic line-clamp-1">{msg_str}</span>
                    </div>
                }

                if !author_str.is_empty() {
                    <div class="flex items-center gap-2">
                        <span class="text-neutral-500 dark:text-neutral-400 text-xs w-16">{"Author:"}</span>
                        <span class="text-xs text-neutral-600 dark:text-neutral-300">{author_str}</span>
                    </div>
                }

                if let Some(d) = dep {
                    <div class="flex items-center gap-2 pt-1 border-t border-neutral-100 dark:border-neutral-700/60 text-xs text-neutral-400">
                        <span>{format!("Started: {}", format_time(d.started_at))}</span>
                    </div>
                }
            </div>
        </div>
    }
}

fn render_logs_preview(
    locale: Locale,
    id: ShortId,
    preview: &str,
    log_size_bytes: Option<u64>,
    navigator: &Navigator,
) -> Html {
    let nav = navigator.clone();
    let on_full_logs = Callback::from(move |_: MouseEvent| nav.push(&Route::AppLog { id }));
    let size_str = log_size_bytes.map(format_bytes).unwrap_or_default();

    html! {
        <div class="p-5 bg-white dark:bg-neutral-800 rounded-lg border border-neutral-200 dark:border-neutral-700 shadow-sm space-y-3">
            <div class="flex items-center justify-between">
                <div class="flex items-center gap-2">
                    <h3 class="font-semibold text-neutral-900 dark:text-neutral-100">
                        {locale.t("apps.recent_logs")}
                    </h3>
                    if !size_str.is_empty() {
                        <span class="text-xs text-neutral-500 font-mono">{format!("({size_str})")}</span>
                    }
                </div>
                <button
                    type="button"
                    onclick={on_full_logs}
                    class="text-xs text-blue-600 dark:text-blue-400 hover:underline font-medium"
                >
                    {locale.t("apps.tabs_logs")}
                </button>
            </div>

            <pre class="p-3 bg-neutral-950 text-neutral-200 font-mono text-xs rounded-md overflow-x-auto max-h-48 leading-relaxed whitespace-pre-wrap select-all">
                if preview.trim().is_empty() {
                    <span class="text-neutral-500 italic">{"No logs available yet."}</span>
                } else {
                    {preview}
                }
            </pre>
        </div>
    }
}
