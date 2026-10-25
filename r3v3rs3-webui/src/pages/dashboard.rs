//! The system dashboard overview page.

use crate::API_ENDPOINT;
use crate::auth::use_ensure_auth;
use crate::components::data_list::status_badge;
use crate::format::format_time;
use crate::i18n::use_locale;
use crate::pages::Route;
use crate::pages::settings::fetch_json;
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

#[function_component(Dashboard)]
pub fn dashboard() -> Html {
    use_ensure_auth();
    let locale = use_locale();
    let (session, _) = use_store::<SessionStore>();
    let summary = use_state(|| Option::<DashboardSummary>::None);

    let signed_in = session.info.is_some();
    let summary_cloned = summary.clone();
    use_effect_with((signed_in,), move |&(signed_in,)| {
        let interval = signed_in.then(|| {
            let load = move || {
                let summary = summary_cloned.clone();
                wasm_bindgen_futures::spawn_local(async move {
                    if let Ok(data) =
                        fetch_json(locale, Request::get(&format!("{API_ENDPOINT}/dashboard"))).await
                    {
                        summary.set(Some(data));
                    }
                });
            };
            load();
            Interval::new(REFRESH_INTERVAL_MS, load)
        });
        move || drop(interval)
    });

    let Some(data) = (*summary).clone() else {
        return html! {
            <div class="p-4 text-center text-neutral-500 dark:text-neutral-400">
                {locale.t("dashboard.loading")}
            </div>
        };
    };

    html! {
        <div class="space-y-6">
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

fn render_ports_card(locale: Locale, ports: &PortsSummary) -> Html {
    let subtext = locale.tf(
        "dashboard.active_count",
        &[
            ("active", &ports.active.to_string()),
            ("total", &ports.total.to_string()),
        ],
    );
    html! {
        <Link<Route> to={Route::Ports} classes="block bg-white dark:bg-neutral-800 p-5 rounded-lg border border-neutral-200 dark:border-neutral-700 hover:border-blue-500 dark:hover:border-blue-400 transition-colors shadow-sm">
            <div class="flex items-center justify-between">
                <span class="text-sm font-medium text-neutral-500 dark:text-neutral-400">{locale.t("dashboard.ports")}</span>
                <img src="/assets/icons/wifi.svg" alt="" class="w-5 h-5 opacity-70" />
            </div>
            <div class="mt-2 text-2xl font-bold text-neutral-900 dark:text-neutral-100">{ports.total}</div>
            <div class="mt-1 text-xs text-neutral-500 dark:text-neutral-400">{subtext}</div>
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
        <Link<Route> to={Route::Proxies} classes="block bg-white dark:bg-neutral-800 p-5 rounded-lg border border-neutral-200 dark:border-neutral-700 hover:border-blue-500 dark:hover:border-blue-400 transition-colors shadow-sm">
            <div class="flex items-center justify-between">
                <span class="text-sm font-medium text-neutral-500 dark:text-neutral-400">{locale.t("dashboard.proxies")}</span>
                <img src="/assets/icons/swap-horizontal.svg" alt="" class="w-5 h-5 opacity-70" />
            </div>
            <div class="mt-2 text-2xl font-bold text-neutral-900 dark:text-neutral-100">{proxies.total}</div>
            <div class="mt-1 text-xs text-neutral-500 dark:text-neutral-400">{subtext}</div>
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
        <Link<Route> to={Route::Apps} classes="block bg-white dark:bg-neutral-800 p-5 rounded-lg border border-neutral-200 dark:border-neutral-700 hover:border-blue-500 dark:hover:border-blue-400 transition-colors shadow-sm">
            <div class="flex items-center justify-between">
                <span class="text-sm font-medium text-neutral-500 dark:text-neutral-400">{locale.t("dashboard.apps")}</span>
                <img src="/assets/icons/cube.svg" alt="" class="w-5 h-5 opacity-70" />
            </div>
            <div class="mt-2 flex items-baseline gap-2">
                <span class="text-2xl font-bold text-neutral-900 dark:text-neutral-100">{apps.total}</span>
                if apps.update_available > 0 {
                    <span class="text-xs font-semibold px-2 py-0.5 rounded bg-blue-100 text-blue-800 dark:bg-blue-900 dark:text-blue-200">
                        {locale.tf("dashboard.update_badge", &[("count", &apps.update_available.to_string())])}
                    </span>
                }
            </div>
            <div class="mt-1 text-xs text-neutral-500 dark:text-neutral-400">{subtext}</div>
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
        <Link<Route> to={Route::Databases} classes="block bg-white dark:bg-neutral-800 p-5 rounded-lg border border-neutral-200 dark:border-neutral-700 hover:border-blue-500 dark:hover:border-blue-400 transition-colors shadow-sm">
            <div class="flex items-center justify-between">
                <span class="text-sm font-medium text-neutral-500 dark:text-neutral-400">{locale.t("dashboard.databases")}</span>
                <img src="/assets/icons/database.svg" alt="" class="w-5 h-5 opacity-70" />
            </div>
            <div class="mt-2 text-2xl font-bold text-neutral-900 dark:text-neutral-100">{databases.total}</div>
            <div class="mt-1 text-xs text-neutral-500 dark:text-neutral-400">{subtext}</div>
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
        <Link<Route> to={Route::Targets} classes="block bg-white dark:bg-neutral-800 p-5 rounded-lg border border-neutral-200 dark:border-neutral-700 hover:border-blue-500 dark:hover:border-blue-400 transition-colors shadow-sm">
            <div class="flex items-center justify-between">
                <span class="text-sm font-medium text-neutral-500 dark:text-neutral-400">{locale.t("dashboard.targets")}</span>
                <img src="/assets/icons/server.svg" alt="" class="w-5 h-5 opacity-70" />
            </div>
            <div class="mt-2 text-2xl font-bold text-neutral-900 dark:text-neutral-100">{targets.total}</div>
            <div class="mt-1 text-xs text-neutral-500 dark:text-neutral-400">{subtext}</div>
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
        <div class="bg-white dark:bg-neutral-800 p-5 rounded-lg border border-neutral-200 dark:border-neutral-700 shadow-sm">
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
    html! {
        <div class="bg-white dark:bg-neutral-800 p-5 rounded-lg border border-neutral-200 dark:border-neutral-700 shadow-sm">
            <h3 class="text-base font-semibold text-neutral-800 dark:text-neutral-200 mb-4">{locale.t("dashboard.recent_deployments")}</h3>
            <div class="overflow-x-auto">
                <table class="w-full text-sm text-left">
                    <thead class="text-xs text-neutral-500 uppercase border-b border-neutral-200 dark:border-neutral-700">
                        <tr>
                            <th class="py-2 pr-4">{locale.t("common.name")}</th>
                            <th class="py-2 px-4">{locale.t("common.status")}</th>
                            <th class="py-2 px-4">{locale.t("deployments.trigger")}</th>
                            <th class="py-2 pl-4 text-right">{locale.t("deployments.started")}</th>
                        </tr>
                    </thead>
                    <tbody class="divide-y divide-neutral-100 dark:divide-neutral-700/50">
                        { for deployments.iter().map(|d| render_deployment_row(locale, d)) }
                    </tbody>
                </table>
            </div>
        </div>
    }
}

fn render_deployment_row(locale: Locale, d: &DeploymentSummary) -> Html {
    let (status_key, badge_color) = super::app_list::status_style(d.status);
    let trigger_str = trigger_text(locale, d.trigger);
    html! {
        <tr key={d.id.to_string()} class="hover:bg-neutral-50 dark:hover:bg-neutral-700/20">
            <td class="py-3 pr-4 font-medium">
                <Link<Route> to={Route::AppDashboard { id: d.app_id }} classes="text-blue-600 dark:text-blue-400 hover:underline">
                    {&d.app_name}
                </Link<Route>>
            </td>
            <td class="py-3 px-4">{ status_badge(locale.t(status_key), badge_color) }</td>
            <td class="py-3 px-4 text-neutral-600 dark:text-neutral-300">{trigger_str}</td>
            <td class="py-3 pl-4 text-right text-neutral-500 dark:text-neutral-400 whitespace-nowrap">{format_time(d.started_at)}</td>
        </tr>
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

fn render_quick_links(locale: Locale, platform_enabled: bool) -> Html {
    html! {
        <div class="bg-white dark:bg-neutral-800 p-5 rounded-lg border border-neutral-200 dark:border-neutral-700 shadow-sm">
            <h3 class="text-sm font-semibold text-neutral-800 dark:text-neutral-200 mb-3">{locale.t("dashboard.quick_links")}</h3>
            <div class="flex flex-wrap gap-3">
                <Link<Route> to={Route::NewProxy} classes="inline-flex items-center px-3 py-1.5 text-xs font-medium rounded border border-neutral-300 dark:border-neutral-600 text-neutral-700 dark:text-neutral-200 hover:bg-neutral-100 dark:hover:bg-neutral-700">
                    {"+ "}{locale.t("common.add")}{" "}{locale.t("nav.proxies")}
                </Link<Route>>
                <Link<Route> to={Route::NewPort} classes="inline-flex items-center px-3 py-1.5 text-xs font-medium rounded border border-neutral-300 dark:border-neutral-600 text-neutral-700 dark:text-neutral-200 hover:bg-neutral-100 dark:hover:bg-neutral-700">
                    {"+ "}{locale.t("common.add")}{" "}{locale.t("nav.ports")}
                </Link<Route>>
                if platform_enabled {
                    <Link<Route> to={Route::NewApp} classes="inline-flex items-center px-3 py-1.5 text-xs font-medium rounded border border-neutral-300 dark:border-neutral-600 text-neutral-700 dark:text-neutral-200 hover:bg-neutral-100 dark:hover:bg-neutral-700">
                        {"+ "}{locale.t("common.add")}{" "}{locale.t("nav.apps")}
                    </Link<Route>>
                    <Link<Route> to={Route::AppStore} classes="inline-flex items-center px-3 py-1.5 text-xs font-medium rounded border border-neutral-300 dark:border-neutral-600 text-neutral-700 dark:text-neutral-200 hover:bg-neutral-100 dark:hover:bg-neutral-700">
                        {locale.t("nav.app_store")}
                    </Link<Route>>
                }
                <Link<Route> to={Route::Settings} classes="inline-flex items-center px-3 py-1.5 text-xs font-medium rounded border border-neutral-300 dark:border-neutral-600 text-neutral-700 dark:text-neutral-200 hover:bg-neutral-100 dark:hover:bg-neutral-700">
                    {locale.t("nav.settings")}
                </Link<Route>>
            </div>
        </div>
    }
}

fn format_disk_bytes(bytes: u64) -> String {
    const GIB: f64 = 1024.0 * 1024.0 * 1024.0;
    const TIB: f64 = 1024.0 * GIB;
    let b = bytes as f64;
    if b >= TIB {
        format!("{:.1} TiB", b / TIB)
    } else if b >= GIB {
        format!("{:.1} GiB", b / GIB)
    } else {
        format!("{:.1} MiB", b / (1024.0 * 1024.0))
    }
}
