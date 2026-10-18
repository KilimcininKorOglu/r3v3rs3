use crate::API_ENDPOINT;
use crate::auth::use_ensure_auth;
use crate::components::data_list::{
    Column, LINK_CLASS, Row, SelectionProps, selectable_list_card, status_badge,
};
use crate::dialog;
use crate::i18n::use_locale;
use crate::pages::Route;
use crate::pages::settings::{failure_box, fetch_json, send_request};
use crate::store::SessionStore;
use gloo_net::http::Request;
use gloo_timers::callback::Interval;
use r3v3rs3_api::i18n::Locale;
use r3v3rs3_api::platform::{
    AppEntry, AppSource, CatalogUpdateStatus, DeploymentEntry, DeploymentStatus, ImageUpdateStatus,
};
use std::collections::HashSet;
use yew::prelude::*;
use yew_router::prelude::*;
use yewdux::prelude::*;

/// The refresh interval while a deployment of an app is unfinished.
pub const FAST_REFRESH_MS: u32 = 2_000;

/// The refresh interval while every deployment is finished.
pub const SLOW_REFRESH_MS: u32 = 10_000;

const COLUMNS: [Column; 4] = [
    Column {
        label: "common.name",
        class: "whitespace-nowrap",
    },
    Column {
        label: "apps.source",
        class: "",
    },
    Column {
        label: "apps.domains",
        class: "",
    },
    Column {
        label: "apps.last_deployment",
        class: "w-48",
    },
];

/// An app with its newest deployment.
#[derive(Clone, PartialEq)]
struct AppRow {
    app: AppEntry,
    latest: Option<DeploymentEntry>,
}

impl AppRow {
    fn is_busy(&self) -> bool {
        self.latest
            .as_ref()
            .is_some_and(|deployment| is_unfinished(deployment.status))
    }
}

type Loaded = Option<Result<Vec<AppRow>, String>>;

#[function_component(AppList)]
pub fn app_list() -> Html {
    use_ensure_auth();
    let locale = use_locale();
    let (session, _) = use_store::<SessionStore>();
    let rows = use_state(|| Loaded::None);
    let failure = use_state(|| Option::<String>::None);
    let reload = use_state(|| 0_u32);
    let selected = use_state(HashSet::<String>::new);

    let busy = rows
        .as_ref()
        .and_then(|rows| rows.as_ref().ok())
        .is_some_and(|rows| rows.iter().any(AppRow::is_busy));
    // The admin API answers 401 until the session loads, so the list waits for it.
    let signed_in = session.info.is_some();
    let rows_cloned = rows.clone();
    use_effect_with((signed_in, busy, *reload), move |&(signed_in, busy, _)| {
        let interval = signed_in.then(|| {
            let load = move || load_rows(locale, rows_cloned.clone());
            load();
            Interval::new(refresh_interval(busy), load)
        });
        move || drop(interval)
    });

    let can_edit = session.can_edit();
    let Some(navigator) = use_navigator() else {
        return html!();
    };
    let deploy = deploy_callback(locale, &failure, &reload);
    let upgrade = upgrade_callback(locale, &failure, &reload);

    let (loaded, list, error) = match &*rows {
        None => (false, Vec::new(), None),
        Some(Ok(list)) => (true, list.clone(), None),
        Some(Err(err)) => (true, Vec::new(), Some(err.clone())),
    };
    let table_rows = list
        .iter()
        .cloned()
        .map(|row| app_row(locale, row, can_edit, &deploy, &upgrade, &navigator))
        .collect::<Vec<_>>();
    let selection = build_selection(locale, can_edit, &list, &selected, &failure, &reload);
    html! {
        <>
            if let Some(err) = error.as_ref().or(failure.as_ref()) {
                { failure_box(err) }
            }
            { selectable_list_card(locale, loaded, "apps.empty", &COLUMNS, &table_rows, selection.as_ref()) }
            if can_edit {
                { add_button(locale, &navigator) }
            }
        </>
    }
}

fn bulk_deploy_callback(
    locale: Locale,
    selected: &UseStateHandle<HashSet<String>>,
    list: &[AppRow],
    failure: &UseStateHandle<Option<String>>,
    reload: &UseStateHandle<u32>,
) -> Callback<MouseEvent> {
    let (selected, failure, reload, list) = (
        selected.clone(),
        failure.clone(),
        reload.clone(),
        list.to_vec(),
    );
    Callback::from(move |e: MouseEvent| {
        e.prevent_default();
        let selected_ids = (*selected).clone();
        let question = locale.tf(
            "apps.bulk_confirm_deploy",
            &[("count", &selected_ids.len().to_string())],
        );
        let (selected, failure, reload, list) = (
            selected.clone(),
            failure.clone(),
            reload.clone(),
            list.clone(),
        );
        dialog::confirm_then(locale, question, async move {
            let mut last_err = None;
            for id_str in selected_ids {
                let Some(row) = list.iter().find(|r| r.app.id.to_string() == id_str) else {
                    continue;
                };
                if let Err(err) = start_deployment(locale, &row.app).await {
                    last_err = Some(err);
                }
            }
            if let Some(err) = last_err {
                failure.set(Some(err));
            }
            selected.set(HashSet::new());
            reload.set(*reload + 1);
        });
    })
}

fn bulk_delete_callback(
    locale: Locale,
    selected: &UseStateHandle<HashSet<String>>,
    failure: &UseStateHandle<Option<String>>,
    reload: &UseStateHandle<u32>,
) -> Callback<MouseEvent> {
    let (selected, failure, reload) = (selected.clone(), failure.clone(), reload.clone());
    Callback::from(move |e: MouseEvent| {
        e.prevent_default();
        let selected_ids = (*selected).clone();
        let question = locale.tf(
            "apps.bulk_confirm_delete",
            &[("count", &selected_ids.len().to_string())],
        );
        let (selected, failure, reload) = (selected.clone(), failure.clone(), reload.clone());
        dialog::confirm_then(locale, question, async move {
            let mut last_err = None;
            for id_str in selected_ids {
                let path = format!("{API_ENDPOINT}/apps/{id_str}");
                let request = Request::delete(&path).build().map_err(|e| e.to_string());
                match request {
                    Ok(req) => {
                        if let Err(err) = send_request(locale, req).await {
                            last_err = Some(err);
                        }
                    }
                    Err(err) => last_err = Some(err),
                }
            }
            if let Some(err) = last_err {
                failure.set(Some(err));
            }
            selected.set(HashSet::new());
            reload.set(*reload + 1);
        });
    })
}

fn build_selection(
    locale: Locale,
    can_edit: bool,
    list: &[AppRow],
    selected: &UseStateHandle<HashSet<String>>,
    failure: &UseStateHandle<Option<String>>,
    reload: &UseStateHandle<u32>,
) -> Option<SelectionProps> {
    if !can_edit {
        return None;
    }
    let all_keys: HashSet<String> = list.iter().map(|r| r.app.id.to_string()).collect();
    let all_selected = !all_keys.is_empty() && all_keys.iter().all(|k| selected.contains(k));

    let on_toggle = {
        let selected = selected.clone();
        Callback::from(move |key: String| {
            let mut set = (*selected).clone();
            if set.contains(&key) {
                set.remove(&key);
            } else {
                set.insert(key);
            }
            selected.set(set);
        })
    };

    let on_toggle_all = {
        let selected = selected.clone();
        let all_keys = all_keys.clone();
        Callback::from(move |()| {
            if all_selected {
                selected.set(HashSet::new());
            } else {
                selected.set(all_keys.clone());
            }
        })
    };

    let on_clear = {
        let selected = selected.clone();
        Some(Callback::from(move |_: MouseEvent| {
            selected.set(HashSet::new())
        }))
    };

    let bulk_deploy = bulk_deploy_callback(locale, selected, list, failure, reload);
    let bulk_delete = bulk_delete_callback(locale, selected, failure, reload);

    let bulk_actions = Some(html! {
        <>
            <button
                type="button"
                onclick={bulk_deploy}
                class="inline-flex items-center text-white bg-blue-600 hover:bg-blue-700 focus:ring-4 focus:ring-blue-300 font-medium rounded-lg text-xs px-2.5 py-1.5 dark:bg-blue-600 dark:hover:bg-blue-700 dark:focus:ring-blue-800"
            >
                {locale.t("apps.bulk_deploy")}
            </button>
            <button
                type="button"
                onclick={bulk_delete}
                class="inline-flex items-center text-red-600 dark:text-red-400 bg-white dark:bg-neutral-800 border border-red-300 dark:border-red-700 focus:outline-none hover:bg-red-50 hover:dark:bg-red-950 focus:ring-4 focus:ring-red-200 dark:focus:ring-red-900 font-medium rounded-lg text-xs px-2.5 py-1.5"
            >
                {locale.t("common.bulk_delete")}
            </button>
        </>
    });

    Some(SelectionProps {
        selected: (**selected).clone(),
        on_toggle,
        on_toggle_all,
        all_selected,
        selectable_keys: Some(all_keys),
        bulk_actions,
        on_clear,
    })
}

/// Asks for a confirmation, starts a deployment of the app and reloads the list.
fn deploy_callback(
    locale: Locale,
    failure: &UseStateHandle<Option<String>>,
    reload: &UseStateHandle<u32>,
) -> Callback<AppRow> {
    let (failure, reload) = (failure.clone(), reload.clone());
    Callback::from(move |row: AppRow| {
        let question = locale.tf("apps.confirm_deploy", &[("name", row.app.name.as_str())]);
        let (failure, reload) = (failure.clone(), reload.clone());
        dialog::confirm_then(locale, question, async move {
            match start_deployment(locale, &row.app).await {
                Ok(_) => failure.set(None),
                Err(err) => failure.set(Some(err)),
            }
            reload.set(*reload + 1);
        });
    })
}

fn upgrade_callback(
    locale: Locale,
    failure: &UseStateHandle<Option<String>>,
    reload: &UseStateHandle<u32>,
) -> Callback<AppRow> {
    let (failure, reload) = (failure.clone(), reload.clone());
    Callback::from(move |row: AppRow| {
        let update_info = row.app.catalog_update.as_ref();
        let target_ver = update_info
            .map(|u| u.catalog_version.as_str())
            .unwrap_or("");
        let question = locale.tf(
            "apps.upgrade_catalog_confirm",
            &[("name", row.app.name.as_str()), ("version", target_ver)],
        );
        let (failure, reload) = (failure.clone(), reload.clone());
        dialog::confirm_then(locale, question, async move {
            match start_upgrade(locale, &row.app).await {
                Ok(_) => failure.set(None),
                Err(err) => failure.set(Some(err)),
            }
            reload.set(*reload + 1);
        });
    })
}

fn add_button(locale: Locale, navigator: &Navigator) -> Html {
    let navigator = navigator.clone();
    let onclick = Callback::from(move |_: MouseEvent| navigator.push(&Route::NewApp));
    html! {
        <div class="flex items-center justify-end my-4">
            <button {onclick} type="button" class="inline-flex items-center text-neutral-500 dark:text-neutral-200 bg-white dark:bg-neutral-800 border border-neutral-300 dark:border-neutral-700 focus:outline-none hover:bg-neutral-100 hover:dark:bg-neutral-900 focus:ring-4 focus:ring-neutral-200 dark:focus:ring-neutral-600 font-medium rounded-lg text-sm px-4 py-2">
                <img src="/assets/icons/add.svg" class="w-4 h-4 mr-1" />
                {locale.t("common.add")}
            </button>
        </div>
    }
}

fn app_row(
    locale: Locale,
    row: AppRow,
    can_edit: bool,
    deploy: &Callback<AppRow>,
    upgrade: &Callback<AppRow>,
    navigator: &Navigator,
) -> Row {
    let domains_cell = html! {
        <span class="break-all">
            { for row.app.spec.domains.iter().map(|domain| {
                let domain = domain.to_string();
                let link = !domain.contains('*');
                html! {
                    <>
                        if link {
                            <a class={LINK_CLASS} href={format!("https://{domain}")} target="_blank" rel="noopener noreferrer">{domain}</a>
                        } else {
                            <span>{domain}</span>
                        }
                        { " " }
                    </>
                }
            }) }
        </span>
    };
    let busy = row.is_busy();
    let deploy_onclick = {
        let deploy = deploy.clone();
        let row = row.clone();
        Callback::from(move |e: MouseEvent| {
            e.prevent_default();
            deploy.emit(row.clone());
        })
    };
    let upgrade_available = row.app.catalog_update.is_some();
    let upgrade_onclick = {
        let upgrade = upgrade.clone();
        let row = row.clone();
        Callback::from(move |e: MouseEvent| {
            e.prevent_default();
            upgrade.emit(row.clone());
        })
    };
    let view_label = if can_edit {
        "common.edit"
    } else {
        "common.view"
    };
    let view_onclick = link(navigator, Route::AppView { id: row.app.id });
    let deployments_onclick = link(navigator, Route::AppDeployments { id: row.app.id });
    let log_onclick = link(navigator, Route::AppLog { id: row.app.id });
    Row {
        key: row.app.id.to_string(),
        cells: vec![
            html! { <>{row.app.name.as_str()}</> },
            source_cell(locale, &row.app),
            html! { <>{domains_cell}</> },
            deployment_cell(locale, row.latest.as_ref()),
        ],
        actions: html! {
            <>
                <a class={LINK_CLASS} onclick={view_onclick}>{locale.t(view_label)}</a>
                <a class={LINK_CLASS} onclick={deployments_onclick}>{locale.t("apps.deployments")}</a>
                <a class={LINK_CLASS} onclick={log_onclick}>{locale.t("common.log")}</a>
                if can_edit && !busy && upgrade_available {
                    <a class={LINK_CLASS} onclick={upgrade_onclick}>{locale.t("apps.upgrade_catalog")}</a>
                }
                if can_edit && !busy {
                    <a class={LINK_CLASS} onclick={deploy_onclick}>{locale.t("apps.deploy")}</a>
                }
            </>
        },
    }
}

/// A click handler that opens `route`.
fn link(navigator: &Navigator, route: Route) -> Callback<MouseEvent> {
    let navigator = navigator.clone();
    Callback::from(move |e: MouseEvent| {
        e.prevent_default();
        navigator.push(&route);
    })
}

/// The kind of the source, and the image or the repository with its branch.
fn source_cell(locale: Locale, app: &AppEntry) -> Html {
    let (kind, detail) = match &app.spec.source {
        AppSource::Image { image } => ("apps.source_image", image.as_str().to_string()),
        AppSource::Git {
            repository, branch, ..
        } => (
            "apps.source_git",
            format!("{}#{}", repository.as_str(), branch.as_str()),
        ),
        AppSource::Compose {
            repository, branch, ..
        } => (
            "apps.source_compose",
            format!("{}#{}", repository.as_str(), branch.as_str()),
        ),
    };
    html! {
        <div>
            <div class="flex items-center gap-1.5 flex-wrap">
                <span>{locale.t(kind)}</span>
                { update_badge(locale, app.update.as_ref()) }
                { catalog_update_badge(locale, app.catalog_update.as_ref()) }
            </div>
            <span class="block text-xs text-neutral-500 dark:text-neutral-400 break-all">{detail}</span>
        </div>
    }
}

fn catalog_update_badge(locale: Locale, update: Option<&CatalogUpdateStatus>) -> Html {
    let Some(update) = update else {
        return html! {};
    };
    let title = format!(
        "{} \u{2192} {}",
        update.current_version, update.catalog_version
    );
    html! {
        <span class="inline-flex items-center px-1.5 py-0.5 rounded text-[10px] font-medium bg-purple-100 text-purple-800 dark:bg-purple-900 dark:text-purple-200" {title}>
            {locale.t("apps.catalog_update_available")}
        </span>
    }
}

fn update_badge(locale: Locale, update: Option<&ImageUpdateStatus>) -> Html {
    let Some(update) = update else {
        return html! {};
    };
    if !update.update_available {
        return html! {};
    }
    html! {
        <span class="inline-flex items-center px-1.5 py-0.5 ml-2 rounded text-[10px] font-medium bg-blue-100 text-blue-800 dark:bg-blue-900 dark:text-blue-200">
            {locale.t("apps.update_available")}
        </span>
    }
}

/// The status of the newest deployment, or a note that the app never ran.
fn deployment_cell(locale: Locale, latest: Option<&DeploymentEntry>) -> Html {
    let Some(deployment) = latest else {
        return html! {
            <span class="text-neutral-500 dark:text-neutral-400">{locale.t("apps.never_deployed")}</span>
        };
    };
    let (key, color) = status_style(deployment.status);
    status_badge(locale.t(key), color)
}

/// The translation key and the dot color of a deployment status.
pub fn status_style(status: DeploymentStatus) -> (&'static str, &'static str) {
    match status {
        DeploymentStatus::Queued => ("deployments.status_queued", "bg-neutral-400"),
        DeploymentStatus::Building => ("deployments.status_building", "bg-yellow-400"),
        DeploymentStatus::Deploying => ("deployments.status_deploying", "bg-yellow-400"),
        DeploymentStatus::Running => ("deployments.status_running", "bg-green-500"),
        DeploymentStatus::Superseded => ("deployments.status_superseded", "bg-neutral-500"),
        DeploymentStatus::Failed => ("deployments.status_failed", "bg-red-500"),
        DeploymentStatus::Cancelled => ("deployments.status_cancelled", "bg-neutral-500"),
        DeploymentStatus::PendingApproval => {
            ("deployments.status_pending_approval", "bg-amber-500")
        }
    }
}

/// Whether the deployment still changes its status.
pub fn is_unfinished(status: DeploymentStatus) -> bool {
    matches!(
        status,
        DeploymentStatus::Queued
            | DeploymentStatus::Building
            | DeploymentStatus::Deploying
            | DeploymentStatus::PendingApproval
    )
}

pub fn refresh_interval(busy: bool) -> u32 {
    if busy {
        FAST_REFRESH_MS
    } else {
        SLOW_REFRESH_MS
    }
}

fn load_rows(locale: Locale, rows: UseStateHandle<Loaded>) {
    wasm_bindgen_futures::spawn_local(async move {
        rows.set(Some(fetch_rows(locale).await));
    });
}

/// The apps with the newest deployment of each.
async fn fetch_rows(locale: Locale) -> Result<Vec<AppRow>, String> {
    let apps: Vec<AppEntry> =
        fetch_json(locale, Request::get(&format!("{API_ENDPOINT}/apps"))).await?;
    let mut rows = Vec::with_capacity(apps.len());
    for app in apps {
        let path = format!("{API_ENDPOINT}/apps/{}/deployments", app.id);
        let deployments: Vec<DeploymentEntry> = fetch_json(locale, Request::get(&path)).await?;
        rows.push(AppRow {
            latest: deployments.into_iter().next(),
            app,
        });
    }
    Ok(rows)
}

async fn start_deployment(locale: Locale, app: &AppEntry) -> Result<DeploymentEntry, String> {
    let path = format!("{API_ENDPOINT}/apps/{}/deploy", app.id);
    fetch_json(locale, Request::post(&path)).await
}

async fn start_upgrade(locale: Locale, app: &AppEntry) -> Result<AppEntry, String> {
    let path = format!("{API_ENDPOINT}/apps/{}/upgrade-catalog", app.id);
    fetch_json(locale, Request::post(&path)).await
}

#[cfg(test)]
mod tests {
    use super::*;

    const STATUSES: [DeploymentStatus; 7] = [
        DeploymentStatus::Queued,
        DeploymentStatus::Building,
        DeploymentStatus::Deploying,
        DeploymentStatus::Running,
        DeploymentStatus::Superseded,
        DeploymentStatus::Failed,
        DeploymentStatus::Cancelled,
    ];

    #[test]
    fn every_status_has_a_translated_name() {
        for status in STATUSES {
            let (key, _) = status_style(status);
            assert_ne!(Locale::En.t(key), key);
            assert_ne!(Locale::Tr.t(key), key);
        }
    }

    #[test]
    fn an_unfinished_deployment_refreshes_faster() {
        let unfinished = STATUSES
            .into_iter()
            .filter(|status| is_unfinished(*status))
            .collect::<Vec<_>>();
        assert_eq!(
            unfinished,
            [
                DeploymentStatus::Queued,
                DeploymentStatus::Building,
                DeploymentStatus::Deploying
            ]
        );
        assert_eq!(refresh_interval(true), FAST_REFRESH_MS);
        assert_eq!(refresh_interval(false), SLOW_REFRESH_MS);
    }
}
