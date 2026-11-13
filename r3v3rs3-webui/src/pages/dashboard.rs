//! The system dashboard overview page.

use crate::API_ENDPOINT;
use crate::auth::use_ensure_auth;
use crate::components::button::{ButtonKind, ButtonSize, button_class};
use crate::components::data_list::{
    Column, ListContent, ListOptions, Row, list_view, status_badge,
};
use crate::format::{format_disk_bytes, format_time};
use crate::i18n::use_locale;
use crate::pages::Route;
use crate::pages::settings::{failure_box, fetch_json};
use crate::store::SessionStore;
use gloo_net::http::Request;
use gloo_timers::callback::Interval;
use r3v3rs3_api::dashboard::{
    AppsSummary, DashboardSummary, DatabasesSummary, DeploymentSummary, DiskSpace, PortsSummary,
    TargetsSummary,
};
use r3v3rs3_api::i18n::Locale;
use yew::prelude::*;
use yew_router::prelude::*;
use yewdux::prelude::*;

const REFRESH_INTERVAL_MS: u32 = 10_000;

const CARD: &str = "bg-white dark:bg-neutral-800 p-5 rounded-md border border-neutral-300 dark:border-neutral-700 shadow-sm";
const STAT_CARD: &str = "block bg-white dark:bg-neutral-800 p-5 rounded-md border border-neutral-300 dark:border-neutral-700 hover:border-primary-500 dark:hover:border-primary-400 transition-colors shadow-sm";
const STAT_LABEL: &str = "text-sm font-medium text-neutral-500 dark:text-neutral-400";
const STAT_VALUE: &str = "text-2xl font-bold text-neutral-900 dark:text-neutral-100";
const STAT_HINT: &str = "mt-1 text-xs text-neutral-500 dark:text-neutral-400";

const DEPLOYMENT_COLUMNS: [Column; 4] = [
    Column {
        label: "common.name",
        class: "",
    },
    Column {
        label: "common.status",
        class: "",
    },
    Column {
        label: "deployments.trigger",
        class: "",
    },
    Column {
        label: "deployments.started",
        class: "whitespace-nowrap",
    },
];

#[function_component(Dashboard)]
pub fn dashboard() -> Html {
    use_ensure_auth();
    let locale = use_locale();
    let (session, _) = use_store::<SessionStore>();
    let summary = use_state(|| Option::<DashboardSummary>::None);
    // The error of the last refresh; the next successful refresh clears it.
    let error = use_state(|| Option::<String>::None);

    let signed_in = session.info.is_some();
    let summary_cloned = summary.clone();
    let error_cloned = error.clone();
    use_effect_with((signed_in,), move |&(signed_in,)| {
        let interval = signed_in.then(|| {
            let load = move || {
                let (summary, error) = (summary_cloned.clone(), error_cloned.clone());
                wasm_bindgen_futures::spawn_local(async move {
                    let path = format!("{API_ENDPOINT}/dashboard");
                    match fetch_json(locale, Request::get(&path)).await {
                        Ok(data) => {
                            summary.set(Some(data));
                            error.set(None);
                        }
                        Err(message) => error.set(Some(message)),
                    }
                });
            };
            load();
            Interval::new(REFRESH_INTERVAL_MS, load)
        });
        move || drop(interval)
    });

    let failure = error.as_deref().map(failure_box).unwrap_or_default();
    let Some(data) = (*summary).clone() else {
        if error.is_some() {
            return failure;
        }
        return html! {
            <div class="p-4 text-center text-neutral-500 dark:text-neutral-400">
                {locale.t("dashboard.loading")}
            </div>
        };
    };

    html! {
        <div class="space-y-6">
            { failure }
            { render_stat_cards(locale, &data) }
            if let Some(disk) = &data.disk {
                { render_disk_card(locale, disk) }
            }
            if !data.recent_deployments.is_empty() {
                { render_recent_deployments(locale, &data.recent_deployments) }
            }
            { render_quick_links(locale, session.platform_enabled) }
        </div>
    }
}

fn render_stat_cards(locale: Locale, data: &DashboardSummary) -> Html {
    html! {
        <div class="grid grid-cols-1 sm:grid-cols-2 lg:grid-cols-3 xl:grid-cols-5 gap-4">
            { render_ports_card(locale, &data.ports) }
            { render_proxies_card(locale, &data.proxies) }
            if let Some(apps) = &data.apps {
                { render_apps_card(locale, apps) }
            }
            if let Some(databases) = &data.databases {
                { render_databases_card(locale, databases) }
            }
            if let Some(targets) = &data.targets {
                { render_targets_card(locale, targets) }
            }
        </div>
    }
}

/// The head of a stat card: its name and its icon.
fn stat_head(label: &str, icon: &'static str) -> Html {
    html! {
        <div class="flex items-center justify-between">
            <span class={STAT_LABEL}>{label}</span>
            <img src={icon} alt="" class="w-5 h-5 opacity-70" />
        </div>
    }
}

fn render_ports_card(locale: Locale, ports: &PortsSummary) -> Html {
    let subtext = locale.tf(
        "dashboard.active_count",
        &[
            ("active", &ports.active.to_string()),
            ("total", &ports.total.to_string()),
        ],
    );
    html! {
        <Link<Route> to={Route::Ports} classes={STAT_CARD}>
            { stat_head(locale.t("dashboard.ports"), "/assets/icons/wifi.svg") }
            <div class={classes!("mt-2", STAT_VALUE)}>{ports.total}</div>
            <div class={STAT_HINT}>{subtext}</div>
        </Link<Route>>
    }
}

fn render_proxies_card(locale: Locale, proxies: &r3v3rs3_api::dashboard::ProxiesSummary) -> Html {
    let subtext = locale.tf(
        "dashboard.active_count",
        &[
            ("active", &proxies.active.to_string()),
            ("total", &proxies.total.to_string()),
        ],
    );
    html! {
        <Link<Route> to={Route::Proxies} classes={STAT_CARD}>
            { stat_head(locale.t("dashboard.proxies"), "/assets/icons/swap-horizontal.svg") }
            <div class={classes!("mt-2", STAT_VALUE)}>{proxies.total}</div>
            <div class={STAT_HINT}>{subtext}</div>
        </Link<Route>>
    }
}

fn render_apps_card(locale: Locale, apps: &AppsSummary) -> Html {
    let subtext = locale.tf(
        "dashboard.running_count",
        &[
            ("running", &apps.running.to_string()),
            ("total", &apps.total.to_string()),
        ],
    );
    html! {
        <Link<Route> to={Route::Apps} classes={STAT_CARD}>
            { stat_head(locale.t("dashboard.apps"), "/assets/icons/cube.svg") }
            <div class="mt-2 flex items-baseline gap-2">
                <span class={STAT_VALUE}>{apps.total}</span>
                if apps.update_available > 0 {
                    <span class="text-xs font-semibold px-2 py-0.5 rounded bg-primary-100 text-primary-800 dark:bg-primary-900 dark:text-primary-200">
                        {locale.tf("dashboard.update_badge", &[("count", &apps.update_available.to_string())])}
                    </span>
                }
            </div>
            <div class={STAT_HINT}>{subtext}</div>
        </Link<Route>>
    }
}

fn render_databases_card(locale: Locale, databases: &DatabasesSummary) -> Html {
    let subtext = locale.tf(
        "dashboard.running_count",
        &[
            ("running", &databases.running.to_string()),
            ("total", &databases.total.to_string()),
        ],
    );
    html! {
        <Link<Route> to={Route::Databases} classes={STAT_CARD}>
            { stat_head(locale.t("dashboard.databases"), "/assets/icons/database.svg") }
            <div class={classes!("mt-2", STAT_VALUE)}>{databases.total}</div>
            <div class={STAT_HINT}>{subtext}</div>
        </Link<Route>>
    }
}

fn render_targets_card(locale: Locale, targets: &TargetsSummary) -> Html {
    let subtext = locale.tf(
        "dashboard.online_count",
        &[
            ("online", &targets.online.to_string()),
            ("total", &targets.total.to_string()),
        ],
    );
    html! {
        <Link<Route> to={Route::Targets} classes={STAT_CARD}>
            { stat_head(locale.t("dashboard.targets"), "/assets/icons/server.svg") }
            <div class={classes!("mt-2", STAT_VALUE)}>{targets.total}</div>
            <div class={STAT_HINT}>{subtext}</div>
        </Link<Route>>
    }
}

fn render_disk_card(locale: Locale, disk: &DiskSpace) -> Html {
    let used_text = format_disk_bytes(disk.used_bytes);
    let total_text = format_disk_bytes(disk.total_bytes);
    let percent_str = disk.used_percent.to_string();
    let text = locale.tf(
        "dashboard.used_of",
        &[
            ("used", &used_text),
            ("total", &total_text),
            ("percent", &percent_str),
        ],
    );

    let bar_color = if disk.used_percent >= 90 {
        "bg-red-500"
    } else if disk.used_percent >= 75 {
        "bg-amber-500"
    } else {
        "bg-emerald-500"
    };

    html! {
        <div class={CARD}>
            <div class="flex items-center justify-between mb-2">
                <h3 class="text-sm font-semibold text-neutral-800 dark:text-neutral-200">{locale.t("dashboard.disk_space")}</h3>
                <span class="text-xs font-medium text-neutral-500 dark:text-neutral-400">{text}</span>
            </div>
            <div class="w-full bg-neutral-200 dark:bg-neutral-700 rounded-full h-3 overflow-hidden">
                <div class={classes!("h-3", "rounded-full", "transition-all", "duration-500", bar_color)} style={format!("width: {}%", disk.used_percent)}></div>
            </div>
        </div>
    }
}

fn render_recent_deployments(locale: Locale, deployments: &[DeploymentSummary]) -> Html {
    let rows = deployments
        .iter()
        .map(|d| render_deployment_row(locale, d))
        .collect::<Vec<_>>();
    let content = ListContent {
        loaded: true,
        empty: "dashboard.no_deployments",
        columns: &DEPLOYMENT_COLUMNS,
        rows: &rows,
    };
    let options = ListOptions {
        embedded: true,
        no_actions: true,
        ..Default::default()
    };
    html! {
        <div class={CARD}>
            <h3 class="text-base font-semibold text-neutral-800 dark:text-neutral-200 mb-4">{locale.t("dashboard.recent_deployments")}</h3>
            <div class="rounded-md border border-neutral-300 dark:border-neutral-700">
                { list_view(locale, content, None, &options) }
            </div>
        </div>
    }
}

fn render_deployment_row(locale: Locale, d: &DeploymentSummary) -> Row {
    let (status_key, badge_color) = super::app_list::status_style(d.status);
    Row {
        key: d.id.to_string(),
        cells: vec![
            html! {
                <Link<Route> to={Route::AppDashboard { id: d.app_id }} classes="text-primary-600 dark:text-primary-400 hover:underline">
                    {&d.app_name}
                </Link<Route>>
            },
            status_badge(locale.t(status_key), badge_color),
            html! { {trigger_text(locale, d.trigger)} },
            html! { {format_time(d.started_at)} },
        ],
        actions: html! {},
    }
}

fn trigger_text(locale: Locale, trigger: r3v3rs3_api::platform::DeploymentTrigger) -> &'static str {
    match trigger {
        r3v3rs3_api::platform::DeploymentTrigger::Manual => locale.t("deployments.trigger_manual"),
        r3v3rs3_api::platform::DeploymentTrigger::Webhook => {
            locale.t("deployments.trigger_webhook")
        }
        r3v3rs3_api::platform::DeploymentTrigger::Rollback => {
            locale.t("deployments.trigger_rollback")
        }
        r3v3rs3_api::platform::DeploymentTrigger::Api => locale.t("deployments.trigger_api"),
        r3v3rs3_api::platform::DeploymentTrigger::Auto => locale.t("deployments.trigger_auto"),
    }
}

fn quick_link(to: Route, label: String) -> Html {
    html! {
        <Link<Route> {to} classes={button_class(ButtonKind::Secondary, ButtonSize::Sm)}>
            {label}
        </Link<Route>>
    }
}

fn render_quick_links(locale: Locale, platform_enabled: bool) -> Html {
    let add = |key: &str| format!("+ {}", locale.t(key));
    html! {
        <div class={CARD}>
            <h3 class="text-sm font-semibold text-neutral-800 dark:text-neutral-200 mb-3">{locale.t("dashboard.quick_links")}</h3>
            <div class="flex flex-wrap gap-3">
                { quick_link(Route::NewProxy, add("dashboard.add_proxy")) }
                { quick_link(Route::NewPort, add("dashboard.add_port")) }
                if platform_enabled {
                    { quick_link(Route::NewApp, add("dashboard.add_app")) }
                    { quick_link(Route::AppStore, locale.t("nav.app_store").to_string()) }
                }
                { quick_link(Route::Settings, locale.t("nav.settings").to_string()) }
            </div>
        </div>
    }
}
