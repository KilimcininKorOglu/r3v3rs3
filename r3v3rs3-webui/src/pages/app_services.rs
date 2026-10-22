//! Service-level health, container status and architecture topology for Docker Compose applications.

use super::Route;
use super::settings::fetch_json;
use crate::API_ENDPOINT;
use crate::components::compose_topology::ComposeTopologyGraph;
use gloo_net::http::Request;
use r3v3rs3_api::container::ContainerHealth;
use r3v3rs3_api::i18n::Locale;
use r3v3rs3_api::id::ShortId;
use r3v3rs3_api::platform::{ComposeServiceStatus, ComposeTopology};
use wasm_bindgen_futures::spawn_local;
use yew::prelude::*;
use yew_router::prelude::*;

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum ComposeViewMode {
    Graph,
    Table,
}

#[derive(Properties, PartialEq)]
pub struct Props {
    pub id: ShortId,
    pub locale: Locale,
}

#[function_component(AppServices)]
pub fn app_services(props: &Props) -> Html {
    let locale = props.locale;
    let id = props.id;
    let services = use_state(|| Option::<Vec<ComposeServiceStatus>>::None);
    let topology = use_state(|| Option::<ComposeTopology>::None);
    let view_mode = use_state(|| ComposeViewMode::Graph);

    {
        let services = services.clone();
        let topology = topology.clone();
        use_effect_with(id, move |&id| {
            load_data(locale, id, services, topology);
        });
    }

    let refresh = {
        let services = services.clone();
        let topology = topology.clone();
        Callback::from(move |_: MouseEvent| {
            load_data(locale, id, services.clone(), topology.clone());
        })
    };

    let Some(list) = (*services).as_ref() else {
        return html! {};
    };

    if list.is_empty() && topology.is_none() {
        return html! {};
    }

    html! {
        <div class="mt-8 bg-white dark:bg-neutral-800 shadow-sm border border-neutral-300 dark:border-neutral-700 rounded-md p-4 sm:p-6">
            { render_header(locale, &view_mode, &refresh) }

            if *view_mode == ComposeViewMode::Graph {
                if let Some(topo) = (*topology).as_ref() {
                    <ComposeTopologyGraph topology={topo.clone()} {locale} />
                } else {
                    <div class="py-8 text-center text-xs text-neutral-400">
                        {locale.t("compose.topology_empty")}
                    </div>
                }
            } else {
                { services_table(locale, id, list) }
            }
        </div>
    }
}

fn render_header(
    locale: Locale,
    view_mode: &UseStateHandle<ComposeViewMode>,
    refresh: &Callback<MouseEvent>,
) -> Html {
    let current_mode = **view_mode;
    let set_graph = {
        let view_mode = view_mode.clone();
        Callback::from(move |_| view_mode.set(ComposeViewMode::Graph))
    };
    let set_table = {
        let view_mode = view_mode.clone();
        Callback::from(move |_| view_mode.set(ComposeViewMode::Table))
    };

    html! {
        <div class="flex flex-wrap items-center justify-between gap-3 mb-5 pb-3 border-b border-neutral-200 dark:border-neutral-700">
            <div>
                <h3 class="text-base font-semibold text-neutral-900 dark:text-neutral-100">
                    {locale.t("compose.topology_title")}
                </h3>
                <p class="text-xs text-neutral-500 dark:text-neutral-400 mt-0.5">
                    {locale.t("compose.topology_desc")}
                </p>
            </div>
            <div class="flex items-center gap-3">
                <div class="inline-flex rounded-lg p-0.5 bg-neutral-100 dark:bg-neutral-900 border border-neutral-200 dark:border-neutral-700">
                    <button
                        type="button"
                        onclick={set_graph}
                        class={tab_class(current_mode == ComposeViewMode::Graph)}
                    >
                        {locale.t("compose.topology_tab_graph")}
                    </button>
                    <button
                        type="button"
                        onclick={set_table}
                        class={tab_class(current_mode == ComposeViewMode::Table)}
                    >
                        {locale.t("compose.topology_tab_table")}
                    </button>
                </div>
                <button
                    type="button"
                    onclick={refresh}
                    class="text-xs sm:text-sm font-medium text-blue-600 dark:text-blue-400 hover:underline"
                >
                    {locale.t("apps.log_refresh")}
                </button>
            </div>
        </div>
    }
}

fn tab_class(active: bool) -> &'static str {
    if active {
        "px-3 py-1 text-xs font-semibold rounded-md bg-white dark:bg-neutral-800 text-neutral-900 dark:text-neutral-100 shadow-sm"
    } else {
        "px-3 py-1 text-xs font-medium rounded-md text-neutral-600 dark:text-neutral-400 hover:text-neutral-900 dark:hover:text-neutral-200"
    }
}

fn load_data(
    locale: Locale,
    id: ShortId,
    services: UseStateHandle<Option<Vec<ComposeServiceStatus>>>,
    topology: UseStateHandle<Option<ComposeTopology>>,
) {
    spawn_local(async move {
        let path = format!("{API_ENDPOINT}/apps/{id}/services");
        if let Ok(list) = fetch_json::<Vec<ComposeServiceStatus>>(locale, Request::get(&path)).await
        {
            services.set(Some(list));
        }
        let topo_path = format!("{API_ENDPOINT}/apps/{id}/topology");
        if let Ok(topo) = fetch_json::<ComposeTopology>(locale, Request::get(&topo_path)).await {
            topology.set(Some(topo));
        }
    });
}

fn services_table(locale: Locale, app_id: ShortId, list: &[ComposeServiceStatus]) -> Html {
    html! {
        <div class="overflow-x-auto -mx-4 -mb-4">
            <table class="w-full text-left border-collapse text-xs">
                <thead>
                    <tr class="border-b border-neutral-200 dark:border-neutral-700 bg-neutral-50 dark:bg-neutral-800/50">
                        <th class="py-2 px-4 font-medium text-neutral-600 dark:text-neutral-300">{locale.t("apps.service_name")}</th>
                        <th class="py-2 px-4 font-medium text-neutral-600 dark:text-neutral-300">{locale.t("apps.service_container")}</th>
                        <th class="py-2 px-4 font-medium text-neutral-600 dark:text-neutral-300">{locale.t("apps.service_state")}</th>
                        <th class="py-2 px-4 font-medium text-neutral-600 dark:text-neutral-300">{locale.t("apps.service_health")}</th>
                        <th class="py-2 px-4 font-medium text-neutral-600 dark:text-neutral-300">{locale.t("apps.service_state")}</th>
                        <th class="py-2 px-4 text-right font-medium text-neutral-600 dark:text-neutral-300"></th>
                    </tr>
                </thead>
                <tbody class="divide-y divide-neutral-200 dark:divide-neutral-700">
                    { for list.iter().map(|s| service_row(locale, app_id, s)) }
                </tbody>
            </table>
        </div>
    }
}

fn service_row(locale: Locale, app_id: ShortId, s: &ComposeServiceStatus) -> Html {
    let exit_text = match s.exit_code {
        Some(code) => locale.tf("apps.service_exit_code", &[("code", &code.to_string())]),
        None => "-".to_string(),
    };

    html! {
        <tr class="hover:bg-neutral-50 dark:hover:bg-neutral-800/30">
            <td class="py-2.5 px-4 font-mono font-medium text-neutral-900 dark:text-neutral-100">{&s.service}</td>
            <td class="py-2.5 px-4 font-mono text-neutral-500 dark:text-neutral-400">{&s.container}</td>
            <td class="py-2.5 px-4">{state_badge(&s.state)}</td>
            <td class="py-2.5 px-4">{health_badge(locale, s.health)}</td>
            <td class="py-2.5 px-4 text-neutral-500 dark:text-neutral-400">{exit_text}</td>
            <td class="py-2.5 px-4 text-right">
                <Link<Route>
                    to={Route::AppLog { id: app_id }}
                    classes="text-xs text-blue-600 dark:text-blue-400 hover:underline"
                >
                    {locale.t("apps.log_title").replace(" {name}", "")}
                </Link<Route>>
            </td>
        </tr>
    }
}

pub fn state_badge(state: &str) -> Html {
    if state == "running" {
        html! {
            <span class="inline-flex items-center gap-1.5 text-green-600 dark:text-green-400 font-medium">
                <span class="w-1.5 h-1.5 rounded-full bg-green-500"></span>
                {state}
            </span>
        }
    } else {
        html! {
            <span class="inline-flex items-center gap-1.5 text-neutral-500 dark:text-neutral-400 font-medium">
                <span class="w-1.5 h-1.5 rounded-full bg-neutral-400"></span>
                {state}
            </span>
        }
    }
}

pub fn health_badge(locale: Locale, health: Option<ContainerHealth>) -> Html {
    match health {
        Some(ContainerHealth::Healthy) => html! {
            <span class="inline-flex items-center px-1.5 py-0.5 rounded text-[11px] font-medium bg-green-100 text-green-800 dark:bg-green-900/30 dark:text-green-300">
                {locale.t("apps.service_healthy")}
            </span>
        },
        Some(ContainerHealth::Starting) => html! {
            <span class="inline-flex items-center px-1.5 py-0.5 rounded text-[11px] font-medium bg-amber-100 text-amber-800 dark:bg-amber-900/30 dark:text-amber-300">
                {locale.t("apps.service_starting")}
            </span>
        },
        Some(ContainerHealth::Unhealthy) => html! {
            <span class="inline-flex items-center px-1.5 py-0.5 rounded text-[11px] font-medium bg-red-100 text-red-800 dark:bg-red-900/30 dark:text-red-300">
                {locale.t("apps.service_unhealthy")}
            </span>
        },
        None => html! {
            <span class="text-neutral-400 dark:text-neutral-500">
                {locale.t("apps.service_no_healthcheck")}
            </span>
        },
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn state_badge_formats_running_and_other() {
        let _ = state_badge("running");
        let _ = state_badge("exited");
    }
}
