//! The backups of one managed database: take, download, restore and delete, plus the backup
//! settings of the server.

use super::app_deployments::back_header;
use super::resource_page::{Notice, delete_on_click, notice_view};
use super::settings::{fetch_json, send_request};
use crate::API_ENDPOINT;
use crate::auth::use_ensure_auth;
use crate::dialog::{self, Icon};
use crate::format::format_time;
use crate::i18n::use_locale;
use crate::store::SessionStore;
use gloo_net::http::Request;
use r3v3rs3_api::i18n::Locale;
use r3v3rs3_api::id::ShortId;
use r3v3rs3_api::platform::{BackupConfig, BackupEntry, S3Config};
use wasm_bindgen_futures::spawn_local;
use yew::prelude::*;
use yew_router::prelude::*;
use yewdux::prelude::*;

const INPUT_CLASS: &str =
    "w-full rounded bg-neutral-800 text-neutral-100 border border-neutral-700 px-2 py-1.5 text-sm";
const BUTTON_CLASS: &str =
    "px-3 py-1.5 rounded bg-neutral-100 text-neutral-900 font-medium disabled:opacity-50";

/// The S3 and the schedule fields of the settings form, as the inputs hold them.
#[derive(Clone, Default, PartialEq)]
struct FormState {
    schedule: String,
    keep: String,
    endpoint: String,
    region: String,
    prefix: String,
    access_key: String,
    secret_key: String,
}

impl FormState {
    fn from_config(config: &BackupConfig) -> Self {
        let s3 = config.s3.clone().unwrap_or_default();
        Self {
            schedule: config.schedule.clone(),
            keep: config.keep.to_string(),
            endpoint: s3.endpoint,
            region: s3.region,
            prefix: s3.prefix,
            access_key: s3.access_key,
            secret_key: String::new(),
        }
    }

    fn to_config(&self) -> BackupConfig {
        let s3 = (!self.endpoint.trim().is_empty()).then(|| S3Config {
            endpoint: self.endpoint.trim().to_string(),
            region: self.region.trim().to_string(),
            prefix: self.prefix.trim().to_string(),
            access_key: self.access_key.trim().to_string(),
            secret_key: self.secret_key.clone(),
        });
        BackupConfig {
            schedule: self.schedule.trim().to_string(),
            keep: self.keep.trim().parse().unwrap_or(7),
            s3,
        }
    }
}

#[function_component(DatabaseBackups)]
pub fn database_backups(props: &Props) -> Html {
    use_ensure_auth();
    let locale = use_locale();
    let (session, _) = use_store::<SessionStore>();
    let backups = use_state(Vec::<BackupEntry>::new);
    let config = use_state(|| Option::<BackupConfig>::None);
    let notice = use_state(|| Option::<Notice<()>>::None);
    let form = use_state(FormState::default);
    let busy = use_state(|| false);
    let id = props.id;

    let signed_in = session.info.is_some();
    let can_edit = session.can_edit();
    {
        let (backups, config, form, notice) = (
            backups.clone(),
            config.clone(),
            form.clone(),
            notice.clone(),
        );
        use_effect_with((signed_in, id), move |&(signed_in, id)| {
            if signed_in {
                reload(locale, &backups, &config, &form, &notice, id);
            }
        });
    }

    if session.info.is_none() {
        return html!();
    }
    let navigator = use_navigator().unwrap();
    let reload = {
        let (backups, config, form, notice) = (
            backups.clone(),
            config.clone(),
            form.clone(),
            notice.clone(),
        );
        Callback::from(move |()| reload(locale, &backups, &config, &form, &notice, id))
    };
    html! {
        <>
            { back_header(locale, &navigator, locale.t("backups.title").to_string()) }
            { notice_view(&notice, |_| html!()) }
            if can_edit {
                <div class="mb-4">
                    <button type="button" class={BUTTON_CLASS} disabled={*busy}
                        onclick={{
                            let (busy, notice, reload) = (busy.clone(), notice.clone(), reload.clone());
                            Callback::from(move |_| {
                                spawn_local(take_backup(locale, id, busy.clone(), notice.clone(), reload.clone()));
                            })
                        }}
                    >
                        {locale.t("backups.take")}
                    </button>
                </div>
            }
            { list_view(locale, id, &backups, can_edit, reload.clone()) }
            if can_edit {
                { settings_view(locale, &config, &form, &notice) }
            }
        </>
    }
}

#[derive(Properties, PartialEq)]
pub struct Props {
    pub id: ShortId,
}

/// Reads the backups and the settings again, and fills the form from the settings when it is
/// still empty.
fn reload(
    locale: Locale,
    backups: &UseStateHandle<Vec<BackupEntry>>,
    config: &UseStateHandle<Option<BackupConfig>>,
    form: &UseStateHandle<FormState>,
    notice: &UseStateHandle<Option<Notice<()>>>,
    id: ShortId,
) {
    let (backups, notice) = (backups.clone(), notice.clone());
    spawn_local(async move {
        match fetch_json::<Vec<BackupEntry>>(
            locale,
            Request::get(&format!("{API_ENDPOINT}/databases/{id}/backups")),
        )
        .await
        {
            Ok(list) => backups.set(list),
            Err(message) => notice.set(Some(Notice::Failed(message))),
        }
    });
    let (config, form) = (config.clone(), form.clone());
    spawn_local(async move {
        if let Ok(loaded) = fetch_json::<BackupConfig>(
            locale,
            Request::get(&format!("{API_ENDPOINT}/backup-config")),
        )
        .await
        {
            if form.is_empty() {
                form.set(FormState::from_config(&loaded));
            }
            config.set(Some(loaded));
        }
    });
}

impl FormState {
    /// Whether the settings have never been loaded, so the form keeps its defaults.
    fn is_empty(&self) -> bool {
        *self == Self::default()
    }
}

/// Takes a backup now.
async fn take_backup(
    locale: Locale,
    id: ShortId,
    busy: UseStateHandle<bool>,
    notice: UseStateHandle<Option<Notice<()>>>,
    reload: Callback<()>,
) {
    busy.set(true);
    let sent = Request::post(&format!("{API_ENDPOINT}/databases/{id}/backups"))
        .build()
        .map(|request| send_request(locale, request));
    match sent {
        Ok(sent) => match sent.await {
            Ok(_) => notice.set(Some(Notice::Saved(()))),
            Err(message) => notice.set(Some(Notice::Failed(message))),
        },
        Err(err) => notice.set(Some(Notice::Failed(err.to_string()))),
    }
    busy.set(false);
    reload.emit(());
}

/// Restores one backup after a confirmation.
fn restore_on_click(locale: Locale, id: ShortId, entry: &BackupEntry) -> Callback<MouseEvent> {
    let entry = entry.clone();
    Callback::from(move |event: MouseEvent| {
        event.prevent_default();
        let question = locale.tf("backups.restore_question", &[("name", &entry.id)]);
        let path = format!("{API_ENDPOINT}/databases/{id}/backups/{}/restore", entry.id);
        dialog::confirm_then(locale, question, async move {
            match Request::post(&path).build().map_err(|err| err.to_string()) {
                Ok(request) => match send_request(locale, request).await {
                    Ok(_) => {
                        dialog::message(locale, locale.t("backups.restored"), Icon::Success).await
                    }
                    Err(message) => dialog::message(locale, &message, Icon::Error).await,
                },
                Err(message) => dialog::message(locale, &message, Icon::Error).await,
            }
        });
    })
}

/// The table of the backups of the database.
fn list_view(
    locale: Locale,
    id: ShortId,
    backups: &[BackupEntry],
    can_edit: bool,
    reload: Callback<()>,
) -> Html {
    if backups.is_empty() {
        return html! { <p class="text-neutral-400 mt-2">{locale.t("backups.empty")}</p> };
    }
    let rows = backups
        .iter()
        .map(|entry| {
            let download =
                format!("{API_ENDPOINT}/databases/{id}/backups/{}/download", entry.id);
            let restore = can_edit.then(|| restore_on_click(locale, id, entry));
            let delete = can_edit.then(|| {
                let question =
                    locale.tf("backups.delete_question", &[("name", entry.id.as_str())]);
                delete_on_click(
                    locale,
                    question,
                    format!("/databases/{id}/backups/{}", entry.id),
                    reload.clone(),
                )
            });
            html! {
                <tr class="border-b border-neutral-700">
                    <td class="py-1.5">{format_time(entry.created_at.max(0) as u64)}</td>
                    <td class="py-1.5 font-mono text-neutral-400">{format_size(entry.size)}</td>
                    <td class="py-1.5 font-mono text-neutral-400">{entry.sha256.chars().take(12).collect::<String>()}</td>
                    <td class="py-1.5 text-right">
                        <a class="text-sm text-neutral-400 hover:text-neutral-200 mr-3" href={download} download={format!("{}.dump.gz", entry.id)}>
                            {locale.t("backups.download")}
                        </a>
                        if let Some(restore) = restore {
                            <a class="text-sm text-neutral-400 hover:text-neutral-200 mr-3" onclick={restore}>
                                {locale.t("backups.restore")}
                            </a>
                        }
                        if let Some(delete) = delete {
                            <a class="text-red-400 hover:text-red-300" onclick={delete}>
                                {locale.t("common.delete")}
                            </a>
                        }
                    </td>
                </tr>
            }
        })
        .collect::<Vec<_>>();
    html! {
        <table class="w-full text-sm mt-3">
            <thead>
                <tr class="text-left text-neutral-400">
                    <th class="py-1.5 font-medium">{locale.t("backups.time")}</th>
                    <th class="py-1.5 font-medium">{locale.t("backups.size")}</th>
                    <th class="py-1.5 font-medium">{locale.t("backups.sha256")}</th>
                    <th />
                </tr>
            </thead>
            <tbody>{ for rows }</tbody>
        </table>
    }
}

/// The size of a backup in KiB or MiB.
fn format_size(size: u64) -> String {
    if size >= 1024 * 1024 {
        format!("{:.1} MiB", size as f64 / (1024.0 * 1024.0))
    } else {
        format!("{:.1} KiB", size as f64 / 1024.0)
    }
}

/// The settings form of the schedule and the S3 target.
fn settings_view(
    locale: Locale,
    config: &UseStateHandle<Option<BackupConfig>>,
    form: &UseStateHandle<FormState>,
    notice: &UseStateHandle<Option<Notice<()>>>,
) -> Html {
    let submit = {
        let (form, notice, config) = (form.clone(), notice.clone(), config.clone());
        Callback::from(move |event: SubmitEvent| {
            event.prevent_default();
            let value = form.to_config();
            let (notice, config) = (notice.clone(), config.clone());
            spawn_local(async move {
                let sent = Request::put(&format!("{API_ENDPOINT}/backup-config"))
                    .json(&value)
                    .map(|request| request.send());
                match sent {
                    Ok(sent) => match sent.await {
                        Ok(response) if response.ok() => {
                            config.set(Some(value));
                            notice.set(Some(Notice::Saved(())));
                        }
                        Ok(response) => notice.set(Some(Notice::Failed(response.status_text()))),
                        Err(err) => notice.set(Some(Notice::Failed(err.to_string()))),
                    },
                    Err(err) => notice.set(Some(Notice::Failed(err.to_string()))),
                }
            });
        })
    };
    let s3 = form.endpoint.trim().is_empty();
    html! {
        <form onsubmit={submit} class="rounded border border-neutral-700 p-4 mt-5">
            <p class="font-medium mb-3">{locale.t("backups.settings")}</p>
            <div class="grid gap-3 md:grid-cols-2">
                <label class="block">
                    <span class="text-sm text-neutral-400">{locale.t("backups.schedule")}</span>
                    <input type="text" class={INPUT_CLASS} placeholder="03:00"
                        value={form.schedule.clone()}
                        onchange={text_input(form, |state| &mut state.schedule)}
                    />
                    <span class="text-xs text-neutral-500">{locale.t("backups.schedule_hint")}</span>
                </label>
                <label class="block">
                    <span class="text-sm text-neutral-400">{locale.t("backups.keep")}</span>
                    <input type="text" class={INPUT_CLASS} value={form.keep.clone()}
                        onchange={text_input(form, |state| &mut state.keep)}
                    />
                </label>
            </div>
            <p class="font-medium mt-4 mb-2">{locale.t("backups.s3")}</p>
            <div class="grid gap-3 md:grid-cols-2">
                <label class="block md:col-span-2">
                    <span class="text-sm text-neutral-400">{locale.t("backups.endpoint")}</span>
                    <input type="text" class={INPUT_CLASS} placeholder="https://s3.example.com/bucket"
                        value={form.endpoint.clone()}
                        onchange={text_input(form, |state| &mut state.endpoint)}
                    />
                    if s3 {
                        <span class="text-xs text-neutral-500">{locale.t("backups.s3_empty")}</span>
                    }
                </label>
                <label class="block">
                    <span class="text-sm text-neutral-400">{locale.t("backups.region")}</span>
                    <input type="text" class={INPUT_CLASS} value={form.region.clone()}
                        onchange={text_input(form, |state| &mut state.region)}
                    />
                </label>
                <label class="block">
                    <span class="text-sm text-neutral-400">{locale.t("backups.prefix")}</span>
                    <input type="text" class={INPUT_CLASS} value={form.prefix.clone()}
                        onchange={text_input(form, |state| &mut state.prefix)}
                    />
                </label>
                <label class="block">
                    <span class="text-sm text-neutral-400">{locale.t("backups.access_key")}</span>
                    <input type="text" class={INPUT_CLASS} value={form.access_key.clone()}
                        onchange={text_input(form, |state| &mut state.access_key)}
                    />
                </label>
                <label class="block">
                    <span class="text-sm text-neutral-400">{locale.t("backups.secret_key")}</span>
                    <input type="password" class={INPUT_CLASS} value={form.secret_key.clone()}
                        onchange={text_input(form, |state| &mut state.secret_key)}
                    />
                    <span class="text-xs text-neutral-500">{locale.t("backups.secret_key_hint")}</span>
                </label>
            </div>
            <div class="flex justify-end mt-3">
                <button type="submit" class={BUTTON_CLASS}>
                    {locale.t("common.update")}
                </button>
            </div>
        </form>
    }
}

/// Returns a callback that writes the value of a text input to a field of the form.
fn text_input(
    form: &UseStateHandle<FormState>,
    select: fn(&mut FormState) -> &mut String,
) -> Callback<Event> {
    let form = form.clone();
    Callback::from(move |event: Event| {
        let input: web_sys::HtmlInputElement = event.target_unchecked_into();
        let mut updated = (*form).clone();
        *select(&mut updated) = input.value();
        form.set(updated);
    })
}
