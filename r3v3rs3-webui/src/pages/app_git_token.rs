//! The Git token of an app. The admin API never returns the token, so the form only sets or
//! removes it.

use super::accounts::{HINT_CLASS, INPUT_CLASS};
use super::resource_page::{FormCard, Notice, form_card, notice_view, text_input};
use super::settings::{fetch_json, send_request};
use crate::API_ENDPOINT;
use crate::components::data_list::DANGER_LINK_CLASS;
use crate::dialog;
use gloo_net::http::Request;
use r3v3rs3_api::i18n::Locale;
use r3v3rs3_api::platform::{AppEntry, GitTokenRequest};
use wasm_bindgen_futures::spawn_local;
use yew::prelude::*;

#[derive(Properties, PartialEq)]
pub struct Props {
    pub app: AppEntry,
    pub locale: Locale,
    /// Receives the app after the token changes.
    pub onchanged: Callback<AppEntry>,
}

#[function_component(AppGitToken)]
pub fn app_git_token(props: &Props) -> Html {
    let locale = props.locale;
    let token = use_state(String::new);
    let notice = use_state(|| Option::<Notice<()>>::None);

    let status = if props.app.git_token_set {
        "apps.git_token_is_set"
    } else {
        "apps.git_token_not_set"
    };
    let remove = props
        .app
        .git_token_set
        .then(|| remove_callback(locale, &props.app, &notice, &props.onchanged));
    let fields = html! {
        <>
            <p class={HINT_CLASS}>{locale.t("apps.git_token_hint")}</p>
            <p class="mt-2 text-sm text-neutral-900 dark:text-neutral-200">{locale.t(status)}</p>
            <input type="password" autocomplete="new-password" aria-label={locale.t("apps.git_token")} placeholder={locale.t("apps.git_token")} value={(*token).clone()} oninput={text_input(&token, |t| t)} class={classes!(INPUT_CLASS, "mt-2")} />
            if let Some(onclick) = remove {
                <div class="mt-4">
                    <a class={DANGER_LINK_CLASS} {onclick}>{locale.t("apps.git_token_remove")}</a>
                </div>
            }
        </>
    };
    let card = FormCard {
        title: locale.t("apps.git_token").to_string(),
        notice: notice_view(
            &notice,
            |_: &()| html! { {locale.t("apps.git_token_saved")} },
        ),
        fields,
        error: None,
        cancel: None,
        save_label: "apps.git_token_save",
        can_save: !token.is_empty(),
    };
    let onsubmit = save_callback(locale, &props.app, &token, &notice, &props.onchanged);
    form_card(locale, card, onsubmit)
}

fn save_callback(
    locale: Locale,
    app: &AppEntry,
    token: &UseStateHandle<String>,
    notice: &UseStateHandle<Option<Notice<()>>>,
    onchanged: &Callback<AppEntry>,
) -> Callback<SubmitEvent> {
    let id = app.id;
    let (token, notice, onchanged) = (token.clone(), notice.clone(), onchanged.clone());
    Callback::from(move |event: SubmitEvent| {
        event.prevent_default();
        let body = GitTokenRequest {
            token: (*token).clone(),
        };
        let (token, notice, onchanged) = (token.clone(), notice.clone(), onchanged.clone());
        spawn_local(async move {
            let path = format!("{API_ENDPOINT}/apps/{id}/git_token");
            let result = match Request::put(&path).json(&body) {
                Ok(request) => read_app(locale, request).await,
                Err(err) => Err(err.to_string()),
            };
            token.set(String::new());
            apply(result, &notice, &onchanged);
        });
    })
}

fn remove_callback(
    locale: Locale,
    app: &AppEntry,
    notice: &UseStateHandle<Option<Notice<()>>>,
    onchanged: &Callback<AppEntry>,
) -> Callback<MouseEvent> {
    let (id, name) = (app.id, app.name.to_string());
    let (notice, onchanged) = (notice.clone(), onchanged.clone());
    Callback::from(move |event: MouseEvent| {
        event.prevent_default();
        let question = locale.tf("apps.confirm_remove_git_token", &[("name", &name)]);
        let (notice, onchanged) = (notice.clone(), onchanged.clone());
        dialog::confirm_then(locale, question, async move {
            let path = format!("{API_ENDPOINT}/apps/{id}/git_token");
            let result = fetch_json(locale, Request::delete(&path)).await;
            apply(result, &notice, &onchanged);
        });
    })
}

async fn read_app(locale: Locale, request: Request) -> Result<AppEntry, String> {
    send_request(locale, request)
        .await?
        .json()
        .await
        .map_err(|err| err.to_string())
}

fn apply(
    result: Result<AppEntry, String>,
    notice: &UseStateHandle<Option<Notice<()>>>,
    onchanged: &Callback<AppEntry>,
) {
    match result {
        Ok(app) => {
            notice.set(Some(Notice::Saved(())));
            onchanged.emit(app);
        }
        Err(message) => notice.set(Some(Notice::Failed(message))),
    }
}
