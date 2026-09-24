//! The page that adds an app, or shows and changes an existing one with its Git token and its
//! environment variables.

use super::Route;
use super::app_env::AppEnv;
use super::app_form::{AppForm, fields_view};
use super::app_git_token::AppGitToken;
use super::app_webhook::AppWebhook;
use super::resource_page::{FormCard, Notice, delete_on_click, form_card, notice_view};
use super::settings::{fetch_json, send_request};
use crate::API_ENDPOINT;
use crate::auth::use_ensure_auth;
use crate::components::data_list::DANGER_LINK_CLASS;
use crate::i18n::use_locale;
use crate::store::SessionStore;
use gloo_net::http::Request;
use r3v3rs3_api::git_connection::GitConnectionEntry;
use r3v3rs3_api::i18n::Locale;
use r3v3rs3_api::id::ShortId;
use r3v3rs3_api::platform::{AppEntry, AppRequest, TargetEntry};
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
    notice: AppNotice,
}

#[derive(Properties, PartialEq)]
pub struct Props {
    /// The app to show. `None` adds an app.
    #[prop_or_default]
    pub id: Option<ShortId>,
}

#[function_component(AppView)]
pub fn app_view(props: &Props) -> Html {
    use_ensure_auth();
    let locale = use_locale();
    let (session, _) = use_store::<SessionStore>();
    let navigator = use_navigator().unwrap();
    let app = use_state(|| Option::<AppEntry>::None);
    let form = use_state(AppForm::default);
    let targets = use_state(Vec::<TargetEntry>::new);
    let connections = use_state(Vec::<GitConnectionEntry>::new);
    let notice = use_state(|| Option::<Notice<()>>::None);

    // The admin API answers 401 until the session loads, so the page waits for it.
    let signed_in = session.info.is_some();
    let state = Loaded {
        app: app.clone(),
        form: form.clone(),
        targets: targets.clone(),
        connections: connections.clone(),
        notice: notice.clone(),
    };
    // Only an account that changes the apps reads the connections.
    let can_edit = session.can_edit();
    use_effect_with((props.id, signed_in), move |&(id, signed_in)| {
        if signed_in {
            load(locale, id, can_edit, state);
        }
    });

    if props.id.is_some() && app.is_none() {
        return html! { { notice_view(&notice, |_: &()| html! {}) } };
    }
    let saved = saved_callback(&app, &form, &notice, &navigator);
    let lists = Lists {
        targets: &targets,
        connections: &connections,
    };
    html! {
        <>
            { form_view(locale, &form, lists, &notice, (*app).as_ref(), can_edit, saved) }
            if let Some(entry) = (*app).clone().filter(|_| can_edit) {
                { edit_extras(locale, entry, &app, &navigator) }
            }
        </>
    }
}

/// The targets and the connections that the form offers.
struct Lists<'a> {
    targets: &'a [TargetEntry],
    connections: &'a [GitConnectionEntry],
}

fn form_view(
    locale: Locale,
    form: &UseStateHandle<AppForm>,
    lists: Lists<'_>,
    notice: &AppNotice,
    app: Option<&AppEntry>,
    can_edit: bool,
    saved: Callback<AppEntry>,
) -> Html {
    let parsed = form.parse(locale);
    let title = match app {
        Some(app) => locale.tf("apps.edit_title", &[("name", app.name.as_str())]),
        None => locale.t("apps.add_title").to_string(),
    };
    let touched = app.is_some() || **form != AppForm::default();
    let card = FormCard {
        title,
        notice: notice_view(notice, |_: &()| html! { {locale.t("apps.saved")} }),
        fields: html! {
            <fieldset disabled={!can_edit}>
                { fields_view(locale, form, lists.targets, lists.connections, app.is_some()) }
            </fieldset>
        },
        error: parsed
            .as_ref()
            .err()
            .filter(|_| touched && can_edit)
            .cloned(),
        cancel: None,
        save_label: if app.is_some() {
            "common.update"
        } else {
            "common.create"
        },
        can_save: can_edit && parsed.is_ok(),
    };
    let onsubmit = submit(locale, form, notice, app.map(|app| app.id), saved);
    form_card(locale, card, onsubmit)
}

/// The Git token, the webhook, the environment variables and the delete link of an existing app.
fn edit_extras(
    locale: Locale,
    entry: AppEntry,
    app: &UseStateHandle<Option<AppEntry>>,
    navigator: &Navigator,
) -> Html {
    // The saved source decides the token section, not the unsaved input of the form.
    let uses_git = AppForm::edit(&entry).uses_git();
    let app = app.clone();
    let onchanged = Callback::from(move |updated: AppEntry| app.set(Some(updated)));
    let navigator = navigator.clone();
    let deleted = Callback::from(move |()| navigator.push(&Route::Apps));
    let question = locale.tf("apps.confirm_delete", &[("name", entry.name.as_str())]);
    let delete = delete_on_click(locale, question, format!("/apps/{}", entry.id), deleted);
    html! {
        <>
            if uses_git {
                <AppGitToken app={entry.clone()} {locale} onchanged={onchanged.clone()} />
            }
            <AppWebhook app={entry.clone()} {locale} {onchanged} />
            <AppEnv id={entry.id} {locale} />
            <div class="flex justify-end mt-4">
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
            navigator.push(&Route::AppView { id: entry.id });
            return;
        }
        form.set(AppForm::edit(&entry));
        app.set(Some(entry));
        notice.set(Some(Notice::Saved(())));
    })
}

fn submit(
    locale: Locale,
    form: &UseStateHandle<AppForm>,
    notice: &AppNotice,
    id: Option<ShortId>,
    saved: Callback<AppEntry>,
) -> Callback<SubmitEvent> {
    let (form, notice) = (form.clone(), notice.clone());
    Callback::from(move |event: SubmitEvent| {
        event.prevent_default();
        let Ok(request) = form.parse(locale) else {
            return;
        };
        let (notice, saved) = (notice.clone(), saved.clone());
        spawn_local(async move {
            match save(locale, id, &request).await {
                Ok(entry) => saved.emit(entry),
                Err(message) => notice.set(Some(Notice::Failed(message))),
            }
        });
    })
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

/// Reads the targets, the connections and the app. A failed read shows its message.
fn load(locale: Locale, id: Option<ShortId>, can_edit: bool, loaded: Loaded) {
    let Loaded {
        app,
        form,
        targets,
        connections,
        notice,
    } = loaded;
    spawn_local(async move {
        match fetch_json(locale, Request::get(&format!("{API_ENDPOINT}/targets"))).await {
            Ok(list) => targets.set(list),
            Err(message) => notice.set(Some(Notice::Failed(message))),
        }
        if can_edit {
            let path = format!("{API_ENDPOINT}/git/connections");
            match fetch_json(locale, Request::get(&path)).await {
                Ok(list) => connections.set(list),
                Err(message) => notice.set(Some(Notice::Failed(message))),
            }
        }
        let Some(id) = id else {
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
