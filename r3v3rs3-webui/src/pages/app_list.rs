use crate::API_ENDPOINT;
use crate::auth::use_ensure_auth;
use crate::components::data_list::{Column, LINK_CLASS, Row, list_card, status_badge};
use crate::dialog;
use crate::i18n::use_locale;
use crate::pages::settings::{failure_box, fetch_json};
use crate::store::SessionStore;
use gloo_net::http::Request;
use gloo_timers::callback::Interval;
use r3v3rs3_api::i18n::Locale;
use r3v3rs3_api::platform::{AppEntry, AppSource, DeploymentEntry, DeploymentStatus};
use yew::prelude::*;
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
    let deploy = {
        let failure = failure.clone();
        let reload = reload.clone();
        Callback::from(move |row: AppRow| {
            let question = locale.tf("apps.confirm_deploy", &[("name", row.app.name.as_str())]);
            let failure = failure.clone();
            let reload = reload.clone();
            dialog::confirm_then(locale, question, async move {
                match start_deployment(locale, &row.app).await {
                    Ok(_) => failure.set(None),
                    Err(err) => failure.set(Some(err)),
                }
                reload.set(*reload + 1);
            });
        })
    };

    let (loaded, list, error) = match &*rows {
        None => (false, Vec::new(), None),
        Some(Ok(list)) => (true, list.clone(), None),
        Some(Err(err)) => (true, Vec::new(), Some(err.clone())),
    };
    let table_rows = list
        .into_iter()
        .map(|row| app_row(locale, row, can_edit, &deploy))
        .collect::<Vec<_>>();
    html! {
        <>
            if let Some(err) = error.as_ref().or(failure.as_ref()) {
                { failure_box(err) }
            }
            { list_card(locale, loaded, "apps.empty", &COLUMNS, &table_rows) }
        </>
    }
}

fn app_row(locale: Locale, row: AppRow, can_edit: bool, deploy: &Callback<AppRow>) -> Row {
    let domains = row
        .app
        .spec
        .domains
        .iter()
        .map(ToString::to_string)
        .collect::<Vec<_>>()
        .join(", ");
    let busy = row.is_busy();
    let deploy_onclick = {
        let deploy = deploy.clone();
        let row = row.clone();
        Callback::from(move |e: MouseEvent| {
            e.prevent_default();
            deploy.emit(row.clone());
        })
    };
    Row {
        key: row.app.id.to_string(),
        cells: vec![
            html! { <>{row.app.name.as_str()}</> },
            source_cell(locale, &row.app.spec.source),
            html! { <span class="break-all">{domains}</span> },
            deployment_cell(locale, row.latest.as_ref()),
        ],
        actions: html! {
            if can_edit && !busy {
                <a class={LINK_CLASS} onclick={deploy_onclick}>{locale.t("apps.deploy")}</a>
            }
        },
    }
}

/// The kind of the source, and the image or the repository with its branch.
fn source_cell(locale: Locale, source: &AppSource) -> Html {
    let (kind, detail) = match source {
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
            <span class="block">{locale.t(kind)}</span>
            <span class="block text-xs text-neutral-500 dark:text-neutral-400 break-all">{detail}</span>
        </div>
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
    }
}

/// Whether the deployment still changes its status.
pub fn is_unfinished(status: DeploymentStatus) -> bool {
    matches!(
        status,
        DeploymentStatus::Queued | DeploymentStatus::Building | DeploymentStatus::Deploying
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
