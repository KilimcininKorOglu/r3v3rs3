//! The webhook of an app. The admin API returns a new secret once, so the section shows it only
//! right after it creates the secret.

use super::accounts::HINT_CLASS;
use super::resource_page::{FormCard, Notice, form_card, notice_view};
use super::settings::fetch_json;
use super::targets::copy_box;
use crate::API_ENDPOINT;
use crate::components::data_list::DANGER_LINK_CLASS;
use crate::dialog;
use gloo_net::http::Request;
use r3v3rs3_api::i18n::Locale;
use r3v3rs3_api::id::ShortId;
use r3v3rs3_api::platform::{AppEntry, WebhookSecret};
use yew::prelude::*;

/// The new secret, or `None` after the webhook is turned off.
type HookNotice = UseStateHandle<Option<Notice<Option<WebhookSecret>>>>;

#[derive(Properties, PartialEq)]
pub struct Props {
    pub app: AppEntry,
    pub locale: Locale,
    /// Receives the app after the secret changes.
    pub onchanged: Callback<AppEntry>,
}

#[function_component(AppWebhook)]
pub fn app_webhook(props: &Props) -> Html {
    let locale = props.locale;
    let notice = use_state(|| Option::<Notice<Option<WebhookSecret>>>::None);
    let set = props.app.webhook_secret_set;
    let status = if set {
        "apps.webhook_is_set"
    } else {
        "apps.webhook_not_set"
    };
    let remove = set.then(|| remove_callback(locale, &props.app, &notice, &props.onchanged));
    let fields = html! {
        <>
            <p class={HINT_CLASS}>{locale.t("apps.webhook_hint")}</p>
            <p class={HINT_CLASS}>{locale.t("apps.webhook_providers")}</p>
            <p class="mt-2 text-sm text-neutral-900 dark:text-neutral-200">{locale.t(status)}</p>
            if let Some(onclick) = remove {
                <div class="mt-4">
                    <a class={DANGER_LINK_CLASS} {onclick}>{locale.t("apps.webhook_remove")}</a>
                </div>
            }
        </>
    };
    let id = props.app.id;
    let card = FormCard {
        title: locale.t("apps.webhook").to_string(),
        notice: notice_view(&notice, |secret: &Option<WebhookSecret>| {
            secret_view(locale, id, secret.as_ref())
        }),
        fields,
        error: None,
        cancel: None,
        save_label: if set {
            "apps.webhook_renew"
        } else {
            "apps.webhook_create"
        },
        can_save: true,
    };
    let onsubmit = create_callback(locale, &props.app, &notice, &props.onchanged);
    form_card(locale, card, onsubmit)
}

/// The new secret with the address that the Git provider calls.
fn secret_view(locale: Locale, id: ShortId, secret: Option<&WebhookSecret>) -> Html {
    let Some(secret) = secret else {
        return html! { {locale.t("apps.webhook_removed")} };
    };
    html! {
        <>
            <p>{locale.t("apps.webhook_created")}</p>
            { copy_box(locale.t("apps.webhook_url"), &hook_url(id), 1) }
            { copy_box(locale.t("apps.webhook_secret"), &secret.secret, 1) }
        </>
    }
}

/// The hook address on the origin of this page, which serves the admin API.
fn hook_url(id: ShortId) -> String {
    let origin = web_sys::window()
        .and_then(|window| window.location().origin().ok())
        .unwrap_or_default();
    format!("{origin}/hooks/apps/{id}")
}

/// Creates the secret. A new secret replaces a working one, so that case asks first.
fn create_callback(
    locale: Locale,
    app: &AppEntry,
    notice: &HookNotice,
    onchanged: &Callback<AppEntry>,
) -> Callback<SubmitEvent> {
    let app = app.clone();
    let (notice, onchanged) = (notice.clone(), onchanged.clone());
    Callback::from(move |event: SubmitEvent| {
        event.prevent_default();
        let (notice, onchanged) = (notice.clone(), onchanged.clone());
        let mut updated = app.clone();
        let task = async move {
            let path = format!("{API_ENDPOINT}/apps/{}/webhook_secret", updated.id);
            match fetch_json::<WebhookSecret>(locale, Request::post(&path)).await {
                Ok(secret) => {
                    notice.set(Some(Notice::Saved(Some(secret))));
                    updated.webhook_secret_set = true;
                    onchanged.emit(updated);
                }
                Err(message) => notice.set(Some(Notice::Failed(message))),
            }
        };
        if app.webhook_secret_set {
            let question = locale.tf("apps.confirm_renew_webhook", &[("name", app.name.as_str())]);
            dialog::confirm_then(locale, question, task);
        } else {
            wasm_bindgen_futures::spawn_local(task);
        }
    })
}

fn remove_callback(
    locale: Locale,
    app: &AppEntry,
    notice: &HookNotice,
    onchanged: &Callback<AppEntry>,
) -> Callback<MouseEvent> {
    let (id, name) = (app.id, app.name.to_string());
    let (notice, onchanged) = (notice.clone(), onchanged.clone());
    Callback::from(move |event: MouseEvent| {
        event.prevent_default();
        let question = locale.tf("apps.confirm_remove_webhook", &[("name", &name)]);
        let (notice, onchanged) = (notice.clone(), onchanged.clone());
        dialog::confirm_then(locale, question, async move {
            let path = format!("{API_ENDPOINT}/apps/{id}/webhook_secret");
            match fetch_json::<AppEntry>(locale, Request::delete(&path)).await {
                Ok(app) => {
                    notice.set(Some(Notice::Saved(None)));
                    onchanged.emit(app);
                }
                Err(message) => notice.set(Some(Notice::Failed(message))),
            }
        });
    })
}
