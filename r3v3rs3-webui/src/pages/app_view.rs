//! The page that adds an app, or shows and changes an existing one with its Git token and its
//! environment variables.

use super::Route;
use super::app_commands::AppCommands;
use super::app_env::AppEnv;
use super::app_form::{AppForm, FormOptions, fields_view};
use super::app_git_token::AppGitToken;
use super::app_image_update::AppImageUpdate;
use super::app_webhook::AppWebhook;
use super::resource_page::{FormCard, Notice, delete_on_click, form_card, notice_view};
use super::settings::{fetch_json, send_request};
use super::store_env::{StoreEnvValue, fields_view as store_env_fields, parse as parse_store_env};
use crate::API_ENDPOINT;
use crate::auth::use_ensure_auth;
use crate::components::data_list::DANGER_LINK_CLASS;
use crate::dialog;
use crate::i18n::use_locale;
use crate::store::SessionStore;
use gloo_net::http::Request;
use r3v3rs3_api::git_connection::GitConnectionEntry;
use r3v3rs3_api::i18n::Locale;
use r3v3rs3_api::id::ShortId;
use r3v3rs3_api::platform::{AppEntry, AppRequest, AppSource, EnvEntry, TargetEntry};
use r3v3rs3_api::port::PortEntry;
use r3v3rs3_api::store::StoreResponse;
use wasm_bindgen_futures::spawn_local;
use yew::prelude::*;
use yew_router::prelude::*;
use yewdux::prelude::*;

type AppNotice = UseStateHandle<Option<Notice<()>>>;

/// The states that the load of the page fills.
struct Loaded {
    app: UseStateHandle<Option<AppEntry>>,
    form: UseStateHandle<AppForm>,
    targets: UseStateHandle<Vec<TargetEntry>>,
    connections: UseStateHandle<Vec<GitConnectionEntry>>,
    ports: UseStateHandle<Vec<PortEntry>>,
    store_env: UseStateHandle<Vec<StoreEnvValue>>,
    notice: AppNotice,
}

#[derive(Properties, PartialEq)]
pub struct Props {
    /// The app to show. `None` adds an app.
    #[prop_or_default]
    pub id: Option<ShortId>,
    /// The id of the app store entry that fills the new app form.
    #[prop_or_default]
    pub store: Option<String>,
}

#[function_component(AppView)]
pub fn app_view(props: &Props) -> Html {
    use_ensure_auth();
    let locale = use_locale();
    let (session, _) = use_store::<SessionStore>();
    let Some(navigator) = use_navigator() else {
        return html!();
    };
    let app = use_state(|| Option::<AppEntry>::None);
    let form = use_state(AppForm::default);
    let targets = use_state(Vec::<TargetEntry>::new);
    let connections = use_state(Vec::<GitConnectionEntry>::new);
    let ports = use_state(Vec::<PortEntry>::new);
    let store_env = use_state(Vec::<StoreEnvValue>::new);
    let notice = use_state(|| Option::<Notice<()>>::None);

    // The admin API answers 401 until the session loads, so the page waits for it.
    let signed_in = session.info.is_some();
    let state = Loaded {
        app: app.clone(),
        form: form.clone(),
        targets: targets.clone(),
        connections: connections.clone(),
        ports: ports.clone(),
        store_env: store_env.clone(),
        notice: notice.clone(),
    };
    // Only an account that changes the apps reads the connections.
    let can_edit = session.can_edit();
    let store = props.store.clone();
    let databases = use_state(Vec::<(ShortId, String)>::new);
    let databases_for_load = databases.clone();
    let env_groups = use_state(Vec::<(ShortId, String)>::new);
    let env_groups_for_load = env_groups.clone();
    use_effect_with((props.id, signed_in), move |&(id, signed_in)| {
        if signed_in {
            load(
                locale,
                id,
                store,
                can_edit,
                state,
                &databases_for_load,
                &env_groups_for_load,
            );
        }
    });

    if props.id.is_some() && app.is_none() {
        return html! { { notice_view(&notice, |_: &()| html! {}) } };
    }
    let saved = saved_callback(&app, &form, &notice, &navigator);
    let on_upgrade = upgrade_catalog_callback(locale, &app, &form, &notice);
    let inputs = FormInputs {
        targets: &targets,
        connections: &connections,
        ports: &ports,
        databases: &databases,
        env_groups: &env_groups,
        store_env: &store_env,
        can_edit,
        auto_deploy: props.store.is_some(),
    };
    html! {
        <>
            if let Some(entry) = (*app).as_ref() {
                <crate::components::app_tabs::AppTabs
                    id={entry.id}
                    name={entry.name.to_string()}
                    active={crate::components::app_tabs::AppTab::Settings}
                    domains={entry.spec.domains.clone()}
                />
            }
            if let Some(entry) = (*app).as_ref().filter(|_| can_edit) {
                { catalog_upgrade_banner(locale, entry, &on_upgrade) }
            }
            { form_view(locale, &form, inputs, &notice, (*app).as_ref(), saved) }
            if let Some(entry) = (*app).clone().filter(|_| can_edit) {
                { edit_extras(locale, entry, &app, &navigator) }
            }
        </>
    }
}

/// The lists that the form offers, the variables of the catalog entry and the rights of the
/// account on the form.
struct FormInputs<'a> {
    targets: &'a [TargetEntry],
    connections: &'a [GitConnectionEntry],
    ports: &'a [PortEntry],
    /// The databases that the app can use, as id and name.
    databases: &'a [(ShortId, String)],
    /// Shared environment variable groups available to link.
    env_groups: &'a [(ShortId, String)],
    /// The variables of an app store entry that the user fills before the first save.
    store_env: &'a UseStateHandle<Vec<StoreEnvValue>>,
    can_edit: bool,
    /// A new app from the app store starts its first deployment at once.
    auto_deploy: bool,
}

fn form_view(
    locale: Locale,
    form: &UseStateHandle<AppForm>,
    inputs: FormInputs<'_>,
    notice: &AppNotice,
    app: Option<&AppEntry>,
    saved: Callback<AppEntry>,
) -> Html {
    let can_edit = inputs.can_edit;
    let parsed = form.parse(locale);
    let env_values = parse_store_env(locale, inputs.store_env);
    let title = match app {
        Some(app) => locale.tf("apps.edit_title", &[("name", app.name.as_str())]),
        None => locale.t("apps.add_title").to_string(),
    };
    let touched = app.is_some() || **form != AppForm::default();
    let options = FormOptions {
        targets: inputs.targets,
        connections: inputs.connections,
        ports: inputs.ports,
        databases: inputs.databases,
        env_groups: inputs.env_groups,
    };
    let card = FormCard {
        title,
        notice: notice_view(notice, |_: &()| html! { {locale.t("apps.saved")} }),
        fields: html! {
            <fieldset disabled={!can_edit}>
                { fields_view(locale, form, options, app.is_some()) }
                if !inputs.store_env.is_empty() {
                    { store_env_fields(locale, inputs.store_env) }
                }
            </fieldset>
        },
        error: parsed
            .as_ref()
            .err()
            .or(env_values.as_ref().err())
            .filter(|_| touched && can_edit)
            .cloned(),
        cancel: None,
        save_label: if app.is_some() {
            "common.update"
        } else {
            "common.create"
        },
        can_save: can_edit && parsed.is_ok() && env_values.is_ok(),
    };
    let onsubmit = submit(
        locale,
        form,
        notice,
        app.map(|app| app.id),
        inputs.store_env,
        inputs.auto_deploy,
        saved,
    );
    form_card(locale, card, onsubmit)
}

/// The Git token, the webhook, the scheduled commands, the environment variables and the delete link of an existing app.
fn edit_extras(
    locale: Locale,
    entry: AppEntry,
    app: &UseStateHandle<Option<AppEntry>>,
    navigator: &Navigator,
) -> Html {
    // The saved source decides the token section, not the unsaved input of the form.
    let uses_git = AppForm::edit(&entry).uses_git();
    let is_image = matches!(entry.spec.source, AppSource::Image { .. });
    let app = app.clone();
    let onchanged = Callback::from(move |updated: AppEntry| app.set(Some(updated)));
    let navigator = navigator.clone();
    let deleted = Callback::from(move |()| navigator.push(&Route::Apps));
    let question = locale.tf("apps.confirm_delete", &[("name", entry.name.as_str())]);
    let delete = delete_on_click(locale, question, format!("/apps/{}", entry.id), deleted);
    html! {
        <>
            if is_image {
                <AppImageUpdate app={entry.clone()} {locale} onchanged={onchanged.clone()} />
            }
            if uses_git {
                <AppGitToken app={entry.clone()} {locale} onchanged={onchanged.clone()} />
            }
            <AppWebhook app={entry.clone()} {locale} {onchanged} />
            <AppCommands id={entry.id} {locale} />
            <AppEnv id={entry.id} {locale} />
            <div class="mt-8 pt-4 border-t border-neutral-200 dark:border-neutral-700 flex justify-end">
                <a class={DANGER_LINK_CLASS} onclick={delete}>{locale.t("apps.delete")}</a>
            </div>
        </>
    }
}

/// A new app opens its own page, so its token and its variables can follow. A changed app
/// stays on the page.
fn saved_callback(
    app: &UseStateHandle<Option<AppEntry>>,
    form: &UseStateHandle<AppForm>,
    notice: &AppNotice,
    navigator: &Navigator,
) -> Callback<AppEntry> {
    let (app, form, notice, navigator) =
        (app.clone(), form.clone(), notice.clone(), navigator.clone());
    Callback::from(move |entry: AppEntry| {
        if app.is_none() {
            navigator.push(&Route::AppDashboard { id: entry.id });
            return;
        }
        form.set(AppForm::edit(&entry));
        app.set(Some(entry));
        notice.set(Some(Notice::Saved(())));
    })
}

fn catalog_upgrade_banner(
    locale: Locale,
    app: &AppEntry,
    on_upgrade: &Callback<MouseEvent>,
) -> Html {
    let Some(update) = &app.catalog_update else {
        return html! {};
    };
    let arrow_text = format!(
        "{} \u{2192} {}",
        update.current_version, update.catalog_version
    );
    html! {
        <div class="mb-4 p-4 rounded-lg bg-purple-50 dark:bg-purple-950/40 border border-purple-200 dark:border-purple-800 flex items-center justify-between flex-wrap gap-2">
            <div class="flex items-center gap-3">
                <span class="text-purple-700 dark:text-purple-300 font-semibold text-sm">
                    {locale.t("apps.catalog_update_available")}
                </span>
                <span class="text-xs text-neutral-600 dark:text-neutral-400 font-mono">
                    {arrow_text}
                </span>
            </div>
            <button
                type="button"
                onclick={on_upgrade}
                class="inline-flex items-center text-white bg-purple-600 hover:bg-purple-700 focus:ring-4 focus:ring-purple-300 font-medium rounded-lg text-xs px-3 py-1.5 dark:bg-purple-600 dark:hover:bg-purple-700 dark:focus:ring-purple-800"
            >
                {locale.t("apps.upgrade_catalog")}
            </button>
        </div>
    }
}

fn upgrade_catalog_callback(
    locale: Locale,
    app: &UseStateHandle<Option<AppEntry>>,
    form: &UseStateHandle<AppForm>,
    notice: &AppNotice,
) -> Callback<MouseEvent> {
    let (app, form, notice) = (app.clone(), form.clone(), notice.clone());
    Callback::from(move |e: MouseEvent| {
        e.prevent_default();
        let Some(entry) = (*app).clone() else {
            return;
        };
        let update_info = entry.catalog_update.as_ref();
        let target_ver = update_info
            .map(|u| u.catalog_version.as_str())
            .unwrap_or("");
        let question = locale.tf(
            "apps.upgrade_catalog_confirm",
            &[("name", entry.name.as_str()), ("version", target_ver)],
        );
        let (app, form, notice) = (app.clone(), form.clone(), notice.clone());
        dialog::confirm_then(locale, question, async move {
            let path = format!("{API_ENDPOINT}/apps/{}/upgrade-catalog", entry.id);
            match fetch_json::<AppEntry>(locale, Request::post(&path)).await {
                Ok(updated) => {
                    form.set(AppForm::edit(&updated));
                    app.set(Some(updated));
                    notice.set(Some(Notice::Saved(())));
                }
                Err(err) => notice.set(Some(Notice::Failed(err))),
            }
        });
    })
}

fn submit(
    locale: Locale,
    form: &UseStateHandle<AppForm>,
    notice: &AppNotice,
    id: Option<ShortId>,
    store_env: &UseStateHandle<Vec<StoreEnvValue>>,
    auto_deploy: bool,
    saved: Callback<AppEntry>,
) -> Callback<SubmitEvent> {
    let (form, notice, store_env) = (form.clone(), notice.clone(), store_env.clone());
    Callback::from(move |event: SubmitEvent| {
        event.prevent_default();
        let Ok(request) = form.parse(locale) else {
            return;
        };
        // A catalog entry with variables saves them before the first deployment reads them.
        let env = if id.is_none() && !store_env.is_empty() {
            match parse_store_env(locale, &store_env) {
                Ok(entries) => Some(entries),
                Err(message) => {
                    notice.set(Some(Notice::Failed(message)));
                    return;
                }
            }
        } else {
            None
        };
        let (notice, saved) = (notice.clone(), saved.clone());
        spawn_local(async move {
            match save(locale, id, &request).await {
                Ok(entry) => {
                    if store_entry_saved(locale, &entry, env, auto_deploy, &notice).await {
                        saved.emit(entry)
                    }
                }
                Err(message) => notice.set(Some(Notice::Failed(message))),
            }
        });
    })
}

/// Saves the variables of a catalog entry and starts the first deployment. Returns `false`
/// when one of the two failed and the notice explains why.
async fn store_entry_saved(
    locale: Locale,
    entry: &AppEntry,
    env: Option<Vec<EnvEntry>>,
    auto_deploy: bool,
    notice: &AppNotice,
) -> bool {
    if let Some(env) = env {
        let path = format!("{API_ENDPOINT}/apps/{}/env", entry.id);
        let saved_env = match Request::put(&path).json(&env) {
            Ok(request) => send_request(locale, request).await,
            Err(err) => Err(err.to_string()),
        };
        if let Err(message) = saved_env {
            notice.set(Some(Notice::Failed(format!(
                "{}: {}",
                locale.t("apps.store_env_failed"),
                message
            ))));
            return false;
        }
    }
    // An app that came from the app store starts its first deployment at once.
    if auto_deploy {
        let path = format!("{API_ENDPOINT}/apps/{}/deploy", entry.id);
        if let Err(message) = fetch_json::<serde_json::Value>(locale, Request::post(&path)).await {
            notice.set(Some(Notice::Failed(format!(
                "{}: {}",
                locale.t("apps.store_deploy_failed"),
                message
            ))));
            return false;
        }
    }
    true
}

async fn save(
    locale: Locale,
    id: Option<ShortId>,
    request: &AppRequest,
) -> Result<AppEntry, String> {
    let builder = match id {
        Some(id) => Request::put(&format!("{API_ENDPOINT}/apps/{id}")),
        None => Request::post(&format!("{API_ENDPOINT}/apps")),
    };
    let request = builder.json(request).map_err(|err| err.to_string())?;
    send_request(locale, request)
        .await?
        .json()
        .await
        .map_err(|err| err.to_string())
}

/// Reads the targets, the connections, the ports, the databases and the app. A failed read
/// shows its message. Without an app id, the app store entry of `store` fills the new form.
fn load(
    locale: Locale,
    id: Option<ShortId>,
    store: Option<String>,
    can_edit: bool,
    loaded: Loaded,
    databases: &UseStateHandle<Vec<(ShortId, String)>>,
    env_groups: &UseStateHandle<Vec<(ShortId, String)>>,
) {
    let databases = databases.clone();
    let env_groups = env_groups.clone();
    let Loaded {
        app,
        form,
        targets,
        connections,
        ports,
        store_env,
        notice,
    } = loaded;
    spawn_local(async move {
        match fetch_json(locale, Request::get(&format!("{API_ENDPOINT}/targets"))).await {
            Ok(list) => targets.set(list),
            Err(message) => notice.set(Some(Notice::Failed(message))),
        }
        if let Ok(list) =
            fetch_json::<Vec<AppEntry>>(locale, Request::get(&format!("{API_ENDPOINT}/databases")))
                .await
        {
            databases.set(
                list.into_iter()
                    .map(|app| (app.id, app.name.to_string()))
                    .collect(),
            );
        }
        if let Ok(list) = fetch_json::<Vec<r3v3rs3_api::platform::EnvGroupSummary>>(
            locale,
            Request::get(&format!("{API_ENDPOINT}/env-groups")),
        )
        .await
        {
            env_groups.set(
                list.into_iter()
                    .map(|group| (group.id, group.name))
                    .collect(),
            );
        }
        if can_edit {
            let path = format!("{API_ENDPOINT}/git/connections");
            match fetch_json(locale, Request::get(&path)).await {
                Ok(list) => connections.set(list),
                Err(message) => notice.set(Some(Notice::Failed(message))),
            }
            let path = format!("{API_ENDPOINT}/ports");
            match fetch_json(locale, Request::get(&path)).await {
                Ok(list) => ports.set(list),
                Err(message) => notice.set(Some(Notice::Failed(message))),
            }
        }
        let Some(id) = id else {
            if let Some(store) = store {
                fill_from_store(locale, &store, &form, &store_env, &notice).await;
            }
            return;
        };
        let path = format!("{API_ENDPOINT}/apps/{id}");
        match fetch_json::<AppEntry>(locale, Request::get(&path)).await {
            Ok(entry) => {
                form.set(AppForm::edit(&entry));
                app.set(Some(entry));
            }
            Err(message) => notice.set(Some(Notice::Failed(message))),
        }
    });
}

/// Fills the new form from an app store entry and lists its variables.
async fn fill_from_store(
    locale: Locale,
    store: &str,
    form: &UseStateHandle<AppForm>,
    store_env: &UseStateHandle<Vec<StoreEnvValue>>,
    notice: &AppNotice,
) {
    let path = format!("{API_ENDPOINT}/store");
    match fetch_json::<StoreResponse>(locale, Request::get(&path)).await {
        Ok(response) => match response.catalog.apps.iter().find(|app| app.id == store) {
            Some(entry) => {
                form.set(AppForm::from_store(entry));
                store_env.set(
                    entry
                        .env
                        .iter()
                        .cloned()
                        .map(|env| StoreEnvValue {
                            env,
                            value: String::new(),
                        })
                        .collect(),
                );
            }
            None => notice.set(Some(Notice::Failed(
                locale.t("apps.store_missing").to_string(),
            ))),
        },
        Err(message) => notice.set(Some(Notice::Failed(message))),
    }
}
