//! The page of outbound webhooks and event notifications.

use super::Route;
use super::accounts::{HINT_CLASS, INPUT_CLASS, LABEL_CLASS};
use super::resource_page::{
    FormCard, Notice, delete_on_click, form_card, load, notice_view, reload_callback, row_actions,
    set_on_click, text_input,
};
use super::settings::send_json;
use crate::API_ENDPOINT;
use crate::auth::use_ensure_auth;
use crate::components::data_list::{Column, LINK_CLASS, Row, list_card};
use crate::dialog::{self, Icon};
use crate::i18n::use_locale;
use crate::store::SessionStore;
use gloo_net::http::Request;
use r3v3rs3_api::i18n::Locale;
use r3v3rs3_api::id::ShortId;
use r3v3rs3_api::webhook::{
    CreateWebhookRequest, OutboundWebhook, TestWebhookRequest, TestWebhookResponse,
    UpdateWebhookRequest, WebhookEvent, WebhookEventCategory,
};
use std::collections::HashSet;
use wasm_bindgen_futures::spawn_local;
use yew::prelude::*;
use yew_router::prelude::*;
use yewdux::prelude::*;

const PATH: &str = "/webhooks";

const COLUMNS: [Column; 4] = [
    Column {
        label: "webhooks.name",
        class: "whitespace-nowrap font-medium text-neutral-900 dark:text-neutral-100",
    },
    Column {
        label: "webhooks.url",
        class: "truncate max-w-xs",
    },
    Column {
        label: "webhooks.enabled",
        class: "whitespace-nowrap",
    },
    Column {
        label: "webhooks.events",
        class: "whitespace-nowrap",
    },
];

type WebhooksList = UseStateHandle<Option<Vec<OutboundWebhook>>>;

#[derive(Clone, PartialEq)]
struct Form {
    editing: Option<ShortId>,
    name: String,
    url: String,
    secret: String,
    enabled: bool,
    events: HashSet<WebhookEvent>,
}

impl Default for Form {
    fn default() -> Self {
        Self {
            editing: None,
            name: String::new(),
            url: String::new(),
            secret: String::new(),
            enabled: true,
            events: WebhookEvent::ALL
                .iter()
                .copied()
                .filter(|e| *e != WebhookEvent::TestPing)
                .collect(),
        }
    }
}

impl Form {
    fn edit(entry: &OutboundWebhook) -> Self {
        Self {
            editing: Some(entry.id),
            name: entry.name.clone(),
            url: entry.url.clone(),
            secret: String::new(),
            enabled: entry.enabled,
            events: entry.events.iter().copied().collect(),
        }
    }

    fn validate(&self, locale: Locale) -> Result<(), String> {
        if self.name.trim().is_empty() {
            return Err(locale.t("webhooks.name_required").to_string());
        }
        if self.url.trim().is_empty() {
            return Err(locale.t("webhooks.url_required").to_string());
        }
        if self.events.is_empty() {
            return Err(locale.t("webhooks.events_required").to_string());
        }
        Ok(())
    }
}

#[function_component(Webhooks)]
pub fn webhooks() -> Html {
    use_ensure_auth();
    let locale = use_locale();
    let (session, _) = use_store::<SessionStore>();
    let list = use_state(|| Option::<Vec<OutboundWebhook>>::None);
    let form = use_state(Form::default);
    let notice = use_state(|| Option::<Notice<()>>::None);

    let signed_in = session.info.is_some();
    let list_cloned = list.clone();
    use_effect_with(signed_in, move |&signed_in| {
        if signed_in {
            load(list_cloned, PATH);
        }
        || ()
    });

    if signed_in && !session.is_admin() {
        return html! { <Redirect<Route> to={Route::Home} /> };
    }

    let rows = list
        .as_deref()
        .unwrap_or_default()
        .iter()
        .map(|entry| list_row(locale, entry, &form, &list))
        .collect::<Vec<_>>();

    html! {
        <>
            <div class="mb-6">
                <h1 class="text-2xl font-bold text-neutral-900 dark:text-neutral-100">{locale.t("webhooks.title")}</h1>
                <p class="text-sm text-neutral-500 dark:text-neutral-400 mt-1">{locale.t("webhooks.subtitle")}</p>
            </div>
            { list_card(locale, list.is_some(), "webhooks.empty", &COLUMNS, &rows) }
            { form_view(locale, &form, &notice, &list) }
        </>
    }
}

fn test_webhook_click(locale: Locale, id: ShortId) -> Callback<MouseEvent> {
    Callback::from(move |e: MouseEvent| {
        e.prevent_default();
        spawn_local(async move {
            let req = TestWebhookRequest {
                webhook_id: Some(id),
                url: None,
                secret: None,
                event: Some(WebhookEvent::TestPing),
            };
            let url = format!("{API_ENDPOINT}/webhooks/test");
            let resp = Request::post(&url).json(&req);
            match resp {
                Ok(req) => match req.send().await {
                    Ok(res) if res.ok() => {
                        let parsed: Result<TestWebhookResponse, _> = res.json().await;
                        match parsed {
                            Ok(body) if body.success => {
                                let msg = locale.tf(
                                    "webhooks.test_success",
                                    &[
                                        ("status", &body.status_code.unwrap_or(200).to_string()),
                                        ("elapsed", &body.elapsed_ms.to_string()),
                                    ],
                                );
                                dialog::message(locale, &msg, Icon::Success).await;
                            }
                            Ok(body) => {
                                let msg =
                                    locale.tf("webhooks.test_failed", &[("reason", &body.message)]);
                                dialog::message(locale, &msg, Icon::Error).await;
                            }
                            Err(err) => {
                                dialog::message(locale, &err.to_string(), Icon::Error).await;
                            }
                        }
                    }
                    Ok(res) => {
                        let text = res.text().await.unwrap_or_default();
                        dialog::message(locale, &text, Icon::Error).await;
                    }
                    Err(err) => {
                        dialog::message(locale, &err.to_string(), Icon::Error).await;
                    }
                },
                Err(err) => {
                    dialog::message(locale, &err.to_string(), Icon::Error).await;
                }
            }
        });
    })
}

fn list_row(
    locale: Locale,
    entry: &OutboundWebhook,
    form: &UseStateHandle<Form>,
    list: &WebhooksList,
) -> Row {
    let question = locale.t("webhooks.delete_confirm").to_string();
    let path = format!("{PATH}/{}", entry.id);
    let delete = delete_on_click(locale, question, path, reload_callback(list, PATH));
    let test_cb = test_webhook_click(locale, entry.id);

    let edit_cb = set_on_click(form, Form::edit(entry));
    let actions = html! {
        <div class="flex items-center space-x-2">
            <button type="button" onclick={test_cb} class={LINK_CLASS}>
                {locale.t("webhooks.test_button")}
            </button>
            { row_actions(locale, edit_cb, Some(delete)) }
        </div>
    };

    let status_badge = if entry.enabled {
        html! {
            <span class="inline-flex items-center px-2 py-0.5 rounded text-xs font-medium bg-emerald-100 text-emerald-800 dark:bg-emerald-900/40 dark:text-emerald-300">
                {locale.t("webhooks.status_active")}
            </span>
        }
    } else {
        html! {
            <span class="inline-flex items-center px-2 py-0.5 rounded text-xs font-medium bg-neutral-100 text-neutral-800 dark:bg-neutral-800 dark:text-neutral-300">
                {locale.t("webhooks.status_disabled")}
            </span>
        }
    };

    let secret_badge = if entry.has_secret {
        html! {
            <span class="ml-2 inline-flex items-center px-1.5 py-0.5 rounded text-xs font-medium bg-blue-100 text-blue-800 dark:bg-blue-900/40 dark:text-blue-300">
                {locale.t("webhooks.has_secret")}
            </span>
        }
    } else {
        html! {}
    };

    let events_count = entry.events.len().to_string();
    let events_label = locale.tf("webhooks.events_count", &[("count", &events_count)]);

    Row {
        key: entry.id.to_string(),
        cells: vec![
            html! {
                <div class="flex items-center">
                    <span class="font-medium text-neutral-900 dark:text-neutral-100">{entry.name.clone()}</span>
                    {secret_badge}
                </div>
            },
            html! { <span class="font-mono text-xs truncate">{entry.url.clone()}</span> },
            html! { <>{status_badge}</> },
            html! { <span class="text-xs text-neutral-600 dark:text-neutral-400">{events_label}</span> },
        ],
        actions,
    }
}

fn event_label(_locale: Locale, event: WebhookEvent) -> &'static str {
    match event {
        WebhookEvent::DeploySuccess => "webhooks.event_deploy_success",
        WebhookEvent::DeployFailure => "webhooks.event_deploy_failure",
        WebhookEvent::DeployPendingApproval => "webhooks.event_deploy_pending_approval",
        WebhookEvent::AppCrashed => "webhooks.event_app_crashed",
        WebhookEvent::BackupSuccess => "webhooks.event_backup_success",
        WebhookEvent::BackupFailure => "webhooks.event_backup_failure",
        WebhookEvent::BackupUploadFailed => "webhooks.event_backup_upload_failed",
        WebhookEvent::BackupRestored => "webhooks.event_backup_restored",
        WebhookEvent::BackupVerificationFailed => "webhooks.event_backup_verification_failed",
        WebhookEvent::CertExpiring => "webhooks.event_cert_expiring",
        WebhookEvent::CertRenewed => "webhooks.event_cert_renewed",
        WebhookEvent::CertRenewalFailed => "webhooks.event_cert_renewal_failed",
        WebhookEvent::SecurityIpBanned => "webhooks.event_security_ip_banned",
        WebhookEvent::TargetOffline => "webhooks.event_target_offline",
        WebhookEvent::SystemDiskPressure => "webhooks.event_system_disk_pressure",
        WebhookEvent::SystemDiskLow => "webhooks.event_system_disk_low",
        WebhookEvent::SystemBandwidthSpike => "webhooks.event_system_bandwidth_spike",
        WebhookEvent::SystemHighLatency => "webhooks.event_system_high_latency",
        WebhookEvent::SystemHighErrorRate => "webhooks.event_system_high_error_rate",
        WebhookEvent::SystemTrafficSpike => "webhooks.event_system_traffic_spike",
        WebhookEvent::SystemEventLoopLag => "webhooks.event_system_event_loop_lag",
        WebhookEvent::SystemFdExhaustion => "webhooks.event_system_fd_exhaustion",
        WebhookEvent::SystemSocketExhaustion => "webhooks.event_system_socket_exhaustion",
        WebhookEvent::TestPing => "webhooks.event_test_ping",
    }
}

fn category_label(category: WebhookEventCategory) -> &'static str {
    match category {
        WebhookEventCategory::Deployment => "webhooks.category_deployment",
        WebhookEventCategory::Backup => "webhooks.category_backup",
        WebhookEventCategory::Security => "webhooks.category_security",
        WebhookEventCategory::System => "webhooks.category_system",
        WebhookEventCategory::Test => "webhooks.category_test",
    }
}

fn category_events(cat: WebhookEventCategory) -> Vec<WebhookEvent> {
    WebhookEvent::ALL
        .iter()
        .copied()
        .filter(|e| e.category() == cat && *e != WebhookEvent::TestPing)
        .collect()
}

fn render_category_box(
    locale: Locale,
    cat: WebhookEventCategory,
    form: &UseStateHandle<Form>,
) -> Html {
    let events = category_events(cat);
    let title_key = category_label(cat);

    let form_select_all = form.clone();
    let events_for_all = events.clone();
    let on_select_all = Callback::from(move |e: MouseEvent| {
        e.prevent_default();
        let mut next = (*form_select_all).clone();
        for ev in &events_for_all {
            next.events.insert(*ev);
        }
        form_select_all.set(next);
    });

    let form_clear = form.clone();
    let events_for_clear = events.clone();
    let on_clear = Callback::from(move |e: MouseEvent| {
        e.prevent_default();
        let mut next = (*form_clear).clone();
        for ev in &events_for_clear {
            next.events.remove(ev);
        }
        form_clear.set(next);
    });

    html! {
        <div class="p-3 border border-neutral-200 dark:border-neutral-700 rounded-md bg-neutral-50 dark:bg-neutral-900/50">
            <div class="flex items-center justify-between mb-2">
                <span class="text-xs font-semibold uppercase tracking-wider text-neutral-700 dark:text-neutral-300">
                    {locale.t(title_key)}
                </span>
                <div class="space-x-2 text-xs">
                    <button type="button" onclick={on_select_all} class="text-primary-600 hover:underline">
                        {locale.t("webhooks.select_all")}
                    </button>
                    <span class="text-neutral-400">{"•"}</span>
                    <button type="button" onclick={on_clear} class="text-neutral-500 hover:underline">
                        {locale.t("webhooks.deselect_all")}
                    </button>
                </div>
            </div>
            <div class="grid grid-cols-1 sm:grid-cols-2 gap-2">
                { for events.into_iter().map(|ev| render_event_checkbox(locale, ev, form)) }
            </div>
        </div>
    }
}

fn render_event_checkbox(locale: Locale, ev: WebhookEvent, form: &UseStateHandle<Form>) -> Html {
    let is_checked = form.events.contains(&ev);
    let form_cloned = form.clone();
    let onchange = Callback::from(move |_| {
        let mut next = (*form_cloned).clone();
        if next.events.contains(&ev) {
            next.events.remove(&ev);
        } else {
            next.events.insert(ev);
        }
        form_cloned.set(next);
    });

    html! {
        <label class="flex items-center space-x-2 text-sm text-neutral-800 dark:text-neutral-200 cursor-pointer">
            <input
                type="checkbox"
                checked={is_checked}
                {onchange}
                class="rounded border-neutral-300 dark:border-neutral-600 text-primary-600 focus:ring-primary-500 h-4 w-4"
            />
            <span>{locale.t(event_label(locale, ev))}</span>
        </label>
    }
}

fn form_view(
    locale: Locale,
    form: &UseStateHandle<Form>,
    notice: &UseStateHandle<Option<Notice<()>>>,
    list: &WebhooksList,
) -> Html {
    let is_editing = form.editing.is_some();
    let title = if is_editing {
        locale.t("webhooks.edit_webhook").to_string()
    } else {
        locale.t("webhooks.new_webhook").to_string()
    };
    let validation = form.validate(locale);

    let form_enabled = form.clone();
    let on_toggle_enabled = Callback::from(move |_| {
        let mut next = (*form_enabled).clone();
        next.enabled = !next.enabled;
        form_enabled.set(next);
    });

    let secret_placeholder = if is_editing {
        locale.t("webhooks.secret_edit_placeholder")
    } else {
        locale.t("webhooks.secret_placeholder")
    };

    let fields = html! {
        <div class="space-y-4 mt-3">
            <div>
                <label class={LABEL_CLASS}>{locale.t("webhooks.name")}</label>
                <input
                    type="text"
                    autocomplete="off"
                    placeholder={locale.t("webhooks.name_placeholder")}
                    value={form.name.clone()}
                    oninput={text_input(form, |f| &mut f.name)}
                    class={INPUT_CLASS}
                />
            </div>

            <div>
                <label class={LABEL_CLASS}>{locale.t("webhooks.url")}</label>
                <input
                    type="url"
                    autocomplete="off"
                    placeholder={locale.t("webhooks.url_placeholder")}
                    value={form.url.clone()}
                    oninput={text_input(form, |f| &mut f.url)}
                    class={INPUT_CLASS}
                />
            </div>

            <div>
                <label class={LABEL_CLASS}>{locale.t("webhooks.secret")}</label>
                <input
                    type="password"
                    autocomplete="new-password"
                    placeholder={secret_placeholder}
                    value={form.secret.clone()}
                    oninput={text_input(form, |f| &mut f.secret)}
                    class={INPUT_CLASS}
                />
            </div>

            <div class="flex items-center space-x-2 pt-1">
                <input
                    type="checkbox"
                    id="webhook-enabled"
                    checked={form.enabled}
                    onchange={on_toggle_enabled}
                    class="rounded border-neutral-300 dark:border-neutral-600 text-primary-600 focus:ring-primary-500 h-4 w-4"
                />
                <label for="webhook-enabled" class="text-sm font-medium text-neutral-800 dark:text-neutral-200 cursor-pointer">
                    {locale.t("webhooks.enabled_hint")}
                </label>
            </div>

            <div class="pt-2">
                <label class={LABEL_CLASS}>{locale.t("webhooks.events")}</label>
                <p class={HINT_CLASS}>{locale.t("webhooks.events_hint")}</p>
                <div class="space-y-3 mt-2">
                    { render_category_box(locale, WebhookEventCategory::Deployment, form) }
                    { render_category_box(locale, WebhookEventCategory::Backup, form) }
                    { render_category_box(locale, WebhookEventCategory::Security, form) }
                    { render_category_box(locale, WebhookEventCategory::System, form) }
                </div>
            </div>
        </div>
    };

    let oncancel = form
        .editing
        .is_some()
        .then(|| set_on_click(form, Form::default()));

    let card = FormCard {
        title,
        notice: notice_view(notice, |()| html! { <p>{locale.t("webhooks.created")}</p> }),
        fields,
        error: validation.err(),
        cancel: oncancel,
        save_label: if is_editing {
            "common.update"
        } else {
            "webhooks.new_webhook"
        },
        can_save: form.validate(locale).is_ok(),
    };

    let onsubmit = submit_webhook(locale, form, notice, list);
    form_card(locale, card, onsubmit)
}

fn submit_webhook(
    locale: Locale,
    form: &UseStateHandle<Form>,
    notice: &UseStateHandle<Option<Notice<()>>>,
    list: &WebhooksList,
) -> Callback<SubmitEvent> {
    let form_cloned = form.clone();
    let notice_cloned = notice.clone();
    let list_cloned = list.clone();

    Callback::from(move |e: SubmitEvent| {
        e.prevent_default();
        let form_state = form_cloned.clone();
        let notice_state = notice_cloned.clone();
        let list_state = list_cloned.clone();

        spawn_local(async move {
            let res = if let Some(id) = form_state.editing {
                let secret_opt = if form_state.secret.trim().is_empty() {
                    None
                } else {
                    Some(form_state.secret.trim().to_string())
                };
                let req = UpdateWebhookRequest {
                    name: Some(form_state.name.trim().to_string()),
                    url: Some(form_state.url.trim().to_string()),
                    secret: secret_opt,
                    enabled: Some(form_state.enabled),
                    events: Some(form_state.events.iter().copied().collect()),
                };
                let builder = Request::put(&format!("{API_ENDPOINT}/webhooks/{id}"));
                send_json(locale, builder, &req).await
            } else {
                let secret_opt = if form_state.secret.trim().is_empty() {
                    None
                } else {
                    Some(form_state.secret.trim().to_string())
                };
                let req = CreateWebhookRequest {
                    name: form_state.name.trim().to_string(),
                    url: form_state.url.trim().to_string(),
                    secret: secret_opt,
                    enabled: form_state.enabled,
                    events: form_state.events.iter().copied().collect(),
                };
                let builder = Request::post(&format!("{API_ENDPOINT}/webhooks"));
                send_json(locale, builder, &req).await
            };

            match res {
                Ok(_) => {
                    form_state.set(Form::default());
                    notice_state.set(Some(Notice::Saved(())));
                    load(list_state, PATH);
                }
                Err(err) => {
                    notice_state.set(Some(Notice::Failed(err.to_string())));
                }
            }
        });
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn form_validation() {
        let locale = Locale::En;
        let mut form = Form::default();
        assert!(form.validate(locale).is_err());

        form.name = "Slack".to_string();
        form.url = "https://hooks.slack.com".to_string();
        assert!(form.validate(locale).is_ok());

        form.events.clear();
        assert!(form.validate(locale).is_err());
    }
}
