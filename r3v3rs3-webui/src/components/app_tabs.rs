use crate::i18n::use_locale;
use crate::pages::Route;
use r3v3rs3_api::id::ShortId;
use r3v3rs3_api::subject_name::SubjectName;
use yew::prelude::*;
use yew_router::prelude::*;

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum AppTab {
    Overview,
    Deployments,
    Logs,
    Terminal,
    Files,
    Settings,
}

#[derive(Properties, PartialEq)]
pub struct Props {
    pub id: ShortId,
    pub name: String,
    pub active: AppTab,
    #[prop_or_default]
    pub status: Option<String>,
    #[prop_or_default]
    pub domains: Vec<SubjectName>,
}

fn status_badge_class(status: &str) -> &'static str {
    match status {
        "running" | "healthy" => {
            "bg-emerald-100 text-emerald-800 dark:bg-emerald-900/50 dark:text-emerald-300"
        }
        "deploying" | "building" | "queued" => {
            "bg-blue-100 text-blue-800 dark:bg-blue-900/50 dark:text-blue-300"
        }
        "failed" | "dead" => "bg-red-100 text-red-800 dark:bg-red-900/50 dark:text-red-300",
        _ => "bg-neutral-100 text-neutral-800 dark:bg-neutral-700 dark:text-neutral-300",
    }
}

#[function_component(AppTabs)]
pub fn app_tabs(props: &Props) -> Html {
    let locale = use_locale();
    let id = props.id;
    let tabs = [
        (
            AppTab::Overview,
            Route::AppDashboard { id },
            "apps.tabs_overview",
        ),
        (
            AppTab::Deployments,
            Route::AppDeployments { id },
            "apps.tabs_deployments",
        ),
        (AppTab::Logs, Route::AppLog { id }, "apps.tabs_logs"),
        (
            AppTab::Terminal,
            Route::AppTerminal { id },
            "apps.tabs_terminal",
        ),
        (AppTab::Files, Route::AppFiles { id }, "apps.tabs_files"),
        (
            AppTab::Settings,
            Route::AppSettings { id },
            "apps.tabs_settings",
        ),
    ];

    html! {
        <div class="mb-6 space-y-4">
            <div class="flex flex-wrap items-center justify-between gap-4">
                <div class="flex items-center gap-3">
                    <Link<Route>
                        to={Route::Apps}
                        classes="inline-flex items-center text-neutral-500 dark:text-neutral-200 bg-white dark:bg-neutral-800 border border-neutral-300 dark:border-neutral-700 hover:bg-neutral-100 hover:dark:bg-neutral-900 font-medium rounded-lg text-sm px-3 py-1.5"
                    >
                        <img src="/assets/icons/arrow-back.svg" class="w-4 h-4 mr-1" />
                        {locale.t("common.back")}
                    </Link<Route>>
                    <h1 class="text-xl font-bold text-neutral-900 dark:text-neutral-100 flex items-center gap-2">
                        {&props.name}
                        if let Some(status) = &props.status {
                            <span class={classes!(
                                "inline-flex", "items-center", "px-2", "py-0.5", "rounded", "text-xs", "font-medium",
                                status_badge_class(status.as_str())
                            )}>
                                {status}
                            </span>
                        }
                    </h1>
                </div>
                if !props.domains.is_empty() {
                    <div class="flex flex-wrap items-center gap-2">
                        { for props.domains.iter().map(|domain| {
                            let url = format!("https://{domain}");
                            html! {
                                <a
                                    href={url}
                                    target="_blank"
                                    rel="noopener noreferrer"
                                    class="inline-flex items-center gap-1 text-xs text-blue-600 dark:text-blue-400 hover:underline bg-blue-50 dark:bg-blue-950/40 px-2 py-1 rounded border border-blue-200 dark:border-blue-900"
                                >
                                    <svg class="w-3.5 h-3.5" fill="none" stroke="currentColor" viewBox="0 0 24 24">
                                        <path stroke-linecap="round" stroke-linejoin="round" stroke-width="2" d="M10 6H6a2 2 0 00-2 2v10a2 2 0 002 2h10a2 2 0 002-2v-4M14 4h6m0 0v6m0-6L10 14" />
                                    </svg>
                                    {domain}
                                </a>
                            }
                        }) }
                    </div>
                }
            </div>

            <div class="border-b border-neutral-200 dark:border-neutral-800">
                <nav class="flex space-x-6 overflow-x-auto">
                    { for tabs.into_iter().map(|(tab, route, label)| {
                        let is_active = tab == props.active;
                        let active_cls = if is_active {
                            "border-blue-600 text-blue-600 dark:text-blue-400 font-semibold border-b-2"
                        } else {
                            "border-transparent text-neutral-500 hover:text-neutral-700 dark:hover:text-neutral-300"
                        };
                        html! {
                            <Link<Route> to={route} classes={classes!("py-3", "px-1", "text-sm", "whitespace-nowrap", active_cls)}>
                                { locale.t(label) }
                            </Link<Route>>
                        }
                    }) }
                </nav>
            </div>
        </div>
    }
}
