//! The deployments of an app, newest first, with a rollback to an earlier one.

use super::Route;
use super::app_list::{is_unfinished, refresh_interval, status_style};
use super::settings::{failure_box, fetch_json};
use crate::API_ENDPOINT;
use crate::auth::use_ensure_auth;
use crate::components::data_list::{Column, LINK_CLASS, Row, list_card, status_badge};
use crate::dialog;
use crate::format::format_time;
use crate::i18n::use_locale;
use crate::store::SessionStore;
use gloo_net::http::Request;
use gloo_timers::callback::Interval;
use r3v3rs3_api::i18n::Locale;
use r3v3rs3_api::id::ShortId;
use r3v3rs3_api::platform::{
    AppEntry, AppSource, DeploymentEntry, DeploymentStatus, DeploymentTrigger,
};
use wasm_bindgen_futures::spawn_local;
use yew::prelude::*;
use yew_router::prelude::*;
use yewdux::prelude::*;

/// The length of a commit hash in the list.
const SHORT_COMMIT: usize = 7;

const COLUMNS: [Column; 7] = [
    Column {
        label: "common.status",
        class: "whitespace-nowrap",
    },
    Column {
        label: "deployments.trigger",
        class: "whitespace-nowrap",
    },
    Column {
        label: "deployments.commit",
        class: "",
    },
    Column {
        label: "deployments.user",
        class: "",
    },
    Column {
        label: "deployments.started",
        class: "whitespace-nowrap",
    },
    Column {
        label: "deployments.duration",
        class: "whitespace-nowrap",
    },
    Column {
        label: "deployments.message",
        class: "break-words",
    },
];

/// The app with its deployments, newest first.
#[derive(Clone, PartialEq)]
struct History {
    app: AppEntry,
    deployments: Vec<DeploymentEntry>,
}

impl History {
    fn is_busy(&self) -> bool {
        self.deployments
            .iter()
            .any(|deployment| is_unfinished(deployment.status))
    }
}

type Loaded = Option<Result<History, String>>;

#[derive(Properties, PartialEq)]
pub struct Props {
    pub id: ShortId,
}

#[function_component(AppDeployments)]
pub fn app_deployments(props: &Props) -> Html {
    use_ensure_auth();
    let locale = use_locale();
    let (session, _) = use_store::<SessionStore>();
    let navigator = use_navigator().unwrap();
    let history = use_state(|| Loaded::None);
    let failure = use_state(|| Option::<String>::None);
    let reload = use_state(|| 0_u32);

    let busy = is_busy(&history);
    // The admin API answers 401 until the session loads, so the page waits for it.
    use_polling(
        locale,
        props.id,
        &history,
        session.info.is_some(),
        busy,
        *reload,
    );

    let rollback =
        (session.can_edit() && !busy).then(|| rollback_callback(locale, &failure, &reload));
    let (title, rows, error) = page_parts(locale, &history, rollback.as_ref());
    html! {
        <>
            { back_header(locale, &navigator, title) }
            if let Some(err) = error.as_ref().or(failure.as_ref()) {
                { failure_box(err) }
            }
            { list_card(locale, rows.is_some(), "deployments.empty", &COLUMNS, &rows.unwrap_or_default()) }
        </>
    }
}

fn is_busy(history: &Loaded) -> bool {
    matches!(history, Some(Ok(history)) if history.is_busy())
}

/// Reads the page once the session loads, then again every 2 seconds while a deployment is
/// unfinished and every 10 seconds otherwise.
#[hook]
fn use_polling(
    locale: Locale,
    id: ShortId,
    history: &UseStateHandle<Loaded>,
    signed_in: bool,
    busy: bool,
    reload: u32,
) {
    let history = history.clone();
    use_effect_with((signed_in, busy, reload), move |&(signed_in, busy, _)| {
        let interval = signed_in.then(|| {
            let load = move || load_history(locale, id, history.clone());
            load();
            Interval::new(refresh_interval(busy), load)
        });
        move || drop(interval)
    });
}

/// The title, the rows and the load error of the page. The rows are `None` until the first load.
fn page_parts(
    locale: Locale,
    history: &Loaded,
    rollback: Option<&Callback<DeploymentEntry>>,
) -> (String, Option<Vec<Row>>, Option<String>) {
    match history {
        None => (String::new(), None, None),
        Some(Err(err)) => (String::new(), Some(Vec::new()), Some(err.clone())),
        Some(Ok(history)) => {
            let rows = history
                .deployments
                .iter()
                .map(|deployment| deployment_row(locale, &history.app, deployment, rollback))
                .collect();
            let title = locale.tf("deployments.title", &[("name", history.app.name.as_str())]);
            (title, Some(rows), None)
        }
    }
}

/// The back button to the app list and the title of the page.
pub fn back_header(locale: Locale, navigator: &Navigator, title: String) -> Html {
    let navigator = navigator.clone();
    let onclick = Callback::from(move |_: MouseEvent| navigator.push(&Route::Apps));
    html! {
        <div class="flex flex-wrap items-center gap-4 mb-4">
            <button {onclick} type="button" class="inline-flex items-center text-neutral-500 dark:text-neutral-200 bg-white dark:bg-neutral-800 border border-neutral-300 dark:border-neutral-700 focus:outline-none hover:bg-neutral-100 hover:dark:bg-neutral-900 focus:ring-4 focus:ring-neutral-200 dark:focus:ring-neutral-600 font-medium rounded-lg text-sm px-4 py-2">
                <img src="/assets/icons/arrow-back.svg" class="w-4 h-4 mr-1" />
                {locale.t("common.back")}
            </button>
            <h2 class="text-lg font-semibold text-neutral-900 dark:text-neutral-200">{title}</h2>
        </div>
    }
}

fn deployment_row(
    locale: Locale,
    app: &AppEntry,
    deployment: &DeploymentEntry,
    rollback: Option<&Callback<DeploymentEntry>>,
) -> Row {
    let (status_key, color) = status_style(deployment.status);
    let commit = deployment
        .commit_sha
        .as_deref()
        .map(|sha| sha.chars().take(SHORT_COMMIT).collect::<String>())
        .unwrap_or_default();
    let action = rollback
        .filter(|_| can_roll_back(app, deployment))
        .map(|rollback| {
            let (rollback, deployment) = (rollback.clone(), deployment.clone());
            Callback::from(move |event: MouseEvent| {
                event.prevent_default();
                rollback.emit(deployment.clone());
            })
        });
    Row {
        key: deployment.id.to_string(),
        cells: vec![
            status_badge(locale.t(status_key), color),
            html! { <>{locale.t(trigger_key(deployment.trigger))}</> },
            html! { <span class="font-mono" title={deployment.commit_sha.clone()}>{commit}</span> },
            html! { <>{deployment.username.clone()}</> },
            html! { <>{format_time(deployment.started_at)}</> },
            html! { <>{duration(locale, deployment)}</> },
            html! { <>{deployment.message.clone().unwrap_or_default()}</> },
        ],
        actions: html! {
            if let Some(onclick) = action {
                <a class={LINK_CLASS} {onclick}>{locale.t("deployments.rollback")}</a>
            }
        },
    }
}

/// Whether a rollback can start the deployment again: a Compose app needs its commit, every
/// other app its image digest. The running deployment needs no rollback.
fn can_roll_back(app: &AppEntry, deployment: &DeploymentEntry) -> bool {
    if is_unfinished(deployment.status) || deployment.status == DeploymentStatus::Running {
        return false;
    }
    match app.spec.source {
        AppSource::Compose { .. } => deployment.commit_sha.is_some(),
        _ => deployment.image_digest.is_some(),
    }
}

fn trigger_key(trigger: DeploymentTrigger) -> &'static str {
    match trigger {
        DeploymentTrigger::Manual => "deployments.trigger_manual",
        DeploymentTrigger::Webhook => "deployments.trigger_webhook",
        DeploymentTrigger::Rollback => "deployments.trigger_rollback",
        DeploymentTrigger::Api => "deployments.trigger_api",
    }
}

/// The run time of a finished deployment in minutes and seconds.
fn duration(locale: Locale, deployment: &DeploymentEntry) -> String {
    let Some(finished_at) = deployment.finished_at else {
        return String::new();
    };
    let seconds = finished_at.saturating_sub(deployment.started_at) / 1000;
    let (minutes, seconds) = ((seconds / 60).to_string(), (seconds % 60).to_string());
    locale.tf(
        "deployments.duration_value",
        &[("minutes", &minutes), ("seconds", &seconds)],
    )
}

/// Asks for a confirmation, repeats the deployment and reloads the page.
fn rollback_callback(
    locale: Locale,
    failure: &UseStateHandle<Option<String>>,
    reload: &UseStateHandle<u32>,
) -> Callback<DeploymentEntry> {
    let (failure, reload) = (failure.clone(), reload.clone());
    Callback::from(move |deployment: DeploymentEntry| {
        let id = deployment.id;
        let question = locale.tf("deployments.confirm_rollback", &[("id", &id.to_string())]);
        let (failure, reload) = (failure.clone(), reload.clone());
        dialog::confirm_then(locale, question, async move {
            let path = format!("{API_ENDPOINT}/deployments/{id}/rollback");
            match fetch_json::<DeploymentEntry>(locale, Request::post(&path)).await {
                Ok(_) => failure.set(None),
                Err(err) => failure.set(Some(err)),
            }
            reload.set(*reload + 1);
        });
    })
}

fn load_history(locale: Locale, id: ShortId, history: UseStateHandle<Loaded>) {
    spawn_local(async move {
        history.set(Some(fetch_history(locale, id).await));
    });
}

async fn fetch_history(locale: Locale, id: ShortId) -> Result<History, String> {
    let app = fetch_json(locale, Request::get(&format!("{API_ENDPOINT}/apps/{id}"))).await?;
    let path = format!("{API_ENDPOINT}/apps/{id}/deployments");
    let deployments = fetch_json(locale, Request::get(&path)).await?;
    Ok(History { app, deployments })
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn app(source: serde_json::Value) -> AppEntry {
        serde_json::from_value(json!({
            "id": "app",
            "name": "shop",
            "target": "local",
            "spec": {"source": source, "port": 80},
            "created_at": 0,
            "updated_at": 0
        }))
        .unwrap()
    }

    fn deployment(status: DeploymentStatus, commit: bool, digest: bool) -> DeploymentEntry {
        DeploymentEntry {
            id: "dep".parse().unwrap(),
            app: "app".parse().unwrap(),
            status,
            trigger: DeploymentTrigger::Manual,
            commit_sha: commit.then(|| "a".repeat(40)),
            image_digest: digest.then(|| format!("sha256:{}", "b".repeat(64))),
            username: "admin".into(),
            started_at: 1_000,
            finished_at: Some(66_000),
            message: None,
        }
    }

    #[test]
    fn a_rollback_needs_what_the_source_starts_again() {
        let image = app(json!({"type": "image", "image": "nginx:1.27"}));
        let compose = app(json!({
            "type": "compose",
            "repository": "https://github.com/owner/shop.git",
            "service": "web"
        }));
        let superseded = DeploymentStatus::Superseded;
        assert!(can_roll_back(&image, &deployment(superseded, false, true)));
        assert!(!can_roll_back(&image, &deployment(superseded, true, false)));
        assert!(can_roll_back(
            &compose,
            &deployment(superseded, true, false)
        ));
        assert!(!can_roll_back(
            &compose,
            &deployment(superseded, false, true)
        ));
        for status in [DeploymentStatus::Running, DeploymentStatus::Building] {
            assert!(!can_roll_back(&image, &deployment(status, true, true)));
        }
    }

    #[test]
    fn a_finished_deployment_shows_its_run_time() {
        let finished = deployment(DeploymentStatus::Failed, false, false);
        assert_eq!(
            duration(Locale::En, &finished),
            Locale::En.tf(
                "deployments.duration_value",
                &[("minutes", "1"), ("seconds", "5")]
            )
        );
        let running = DeploymentEntry {
            finished_at: None,
            ..finished
        };
        assert_eq!(duration(Locale::En, &running), "");
        for trigger in [
            DeploymentTrigger::Manual,
            DeploymentTrigger::Webhook,
            DeploymentTrigger::Rollback,
            DeploymentTrigger::Api,
        ] {
            assert_ne!(Locale::Tr.t(trigger_key(trigger)), trigger_key(trigger));
        }
    }
}
