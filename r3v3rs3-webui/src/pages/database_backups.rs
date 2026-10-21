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
use r3v3rs3_api::container::AppName;
use r3v3rs3_api::i18n::Locale;
use r3v3rs3_api::id::ShortId;
use r3v3rs3_api::platform::{
    BackupConfig, BackupEntry, PitrRange, PitrRestoreRequest, RsyncConfig, S3BackupSnapshot,
    S3Config, SftpConfig,
};
use wasm_bindgen_futures::spawn_local;
use yew::prelude::*;
use yew_router::prelude::*;
use yewdux::prelude::*;

const INPUT_CLASS: &str =
    "w-full rounded bg-neutral-800 text-neutral-100 border border-neutral-700 px-2 py-1.5 text-sm";
const BUTTON_CLASS: &str =
    "px-3 py-1.5 rounded bg-neutral-100 text-neutral-900 font-medium disabled:opacity-50";

/// The backup settings form state holding S3, SFTP, rsync and restore test fields.
#[derive(Clone, PartialEq)]
struct FormState {
    schedule: String,
    keep: String,
    heartbeat_url: String,
    endpoint: String,
    region: String,
    prefix: String,
    access_key: String,
    secret_key: String,
    sftp_host: String,
    sftp_port: String,
    sftp_user: String,
    sftp_path: String,
    sftp_password: String,
    sftp_key: String,
    rsync_dest: String,
    rsync_ssh_options: String,
    rsync_password: String,
    restore_test_enabled: bool,
}

impl Default for FormState {
    fn default() -> Self {
        Self {
            schedule: String::new(),
            keep: "7".into(),
            heartbeat_url: String::new(),
            endpoint: String::new(),
            region: String::new(),
            prefix: String::new(),
            access_key: String::new(),
            secret_key: String::new(),
            sftp_host: String::new(),
            sftp_port: "22".into(),
            sftp_user: String::new(),
            sftp_path: String::new(),
            sftp_password: String::new(),
            sftp_key: String::new(),
            rsync_dest: String::new(),
            rsync_ssh_options: String::new(),
            rsync_password: String::new(),
            restore_test_enabled: true,
        }
    }
}

impl FormState {
    fn from_config(config: &BackupConfig) -> Self {
        let s3 = config.s3.clone().unwrap_or_default();
        let sftp = config.sftp.clone().unwrap_or_default();
        let rsync = config.rsync.clone().unwrap_or_default();
        Self {
            schedule: config.schedule.clone(),
            keep: config.keep.to_string(),
            heartbeat_url: config.heartbeat_url.clone().unwrap_or_default(),
            endpoint: s3.endpoint,
            region: s3.region,
            prefix: s3.prefix,
            access_key: s3.access_key,
            secret_key: String::new(),
            sftp_host: sftp.host,
            sftp_port: if sftp.port > 0 {
                sftp.port.to_string()
            } else {
                "22".to_string()
            },
            sftp_user: sftp.user,
            sftp_path: sftp.path,
            sftp_password: String::new(),
            sftp_key: String::new(),
            rsync_dest: rsync.destination,
            rsync_ssh_options: rsync.ssh_options,
            rsync_password: String::new(),
            restore_test_enabled: config.restore_test_enabled,
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
        let sftp = (!self.sftp_host.trim().is_empty()).then(|| SftpConfig {
            host: self.sftp_host.trim().to_string(),
            port: self.sftp_port.trim().parse().unwrap_or(22),
            user: self.sftp_user.trim().to_string(),
            path: self.sftp_path.trim().to_string(),
            password: self.sftp_password.clone(),
            key: self.sftp_key.clone(),
        });
        let rsync = (!self.rsync_dest.trim().is_empty()).then(|| RsyncConfig {
            destination: self.rsync_dest.trim().to_string(),
            ssh_options: self.rsync_ssh_options.trim().to_string(),
            password: self.rsync_password.clone(),
        });
        BackupConfig {
            schedule: self.schedule.trim().to_string(),
            keep: self.keep.trim().parse().unwrap_or(7),
            s3,
            sftp,
            rsync,
            restore_test_enabled: self.restore_test_enabled,
            heartbeat_url: {
                let trimmed = self.heartbeat_url.trim();
                if trimmed.is_empty() {
                    None
                } else {
                    Some(trimmed.to_string())
                }
            },
        }
    }
}

#[function_component(DatabaseBackups)]
pub fn database_backups(props: &Props) -> Html {
    use_ensure_auth();
    let locale = use_locale();
    let (session, _) = use_store::<SessionStore>();
    let backups = use_state(Vec::<BackupEntry>::new);
    let s3_snapshots = use_state(Vec::<S3BackupSnapshot>::new);
    let config = use_state(|| Option::<BackupConfig>::None);
    let pitr_range = use_state(|| Option::<PitrRange>::None);
    let notice = use_state(|| Option::<Notice<&'static str>>::None);
    let form = use_state(FormState::default);
    let busy = use_state(|| false);
    let id = props.id;

    let signed_in = session.info.is_some();
    let can_edit = session.can_edit();
    {
        let (backups, s3_snapshots, config, form, notice, pitr_range) = (
            backups.clone(),
            s3_snapshots.clone(),
            config.clone(),
            form.clone(),
            notice.clone(),
            pitr_range.clone(),
        );
        use_effect_with((signed_in, id), move |&(signed_in, id)| {
            if signed_in {
                reload(locale, &backups, &config, &form, &notice, &pitr_range, id);
                fetch_s3_snapshots(locale, &s3_snapshots);
            }
        });
    }

    if session.info.is_none() {
        return html!();
    }
    let navigator = use_navigator().unwrap();
    let reload = {
        let (backups, s3_snapshots, config, form, notice, pitr_range) = (
            backups.clone(),
            s3_snapshots.clone(),
            config.clone(),
            form.clone(),
            notice.clone(),
            pitr_range.clone(),
        );
        Callback::from(move |()| {
            reload(locale, &backups, &config, &form, &notice, &pitr_range, id);
            fetch_s3_snapshots(locale, &s3_snapshots);
        })
    };
    html! {
        <>
            { back_header(locale, &navigator, locale.t("backups.title").to_string()) }
            { notice_view(&notice, |key| html! { {locale.t(key)} }) }
            { pitr_banner_view(locale, id, (*pitr_range).as_ref(), can_edit, &notice, &reload) }
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
            { s3_list_view(locale, id, &s3_snapshots, can_edit, reload.clone()) }
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

fn fetch_s3_snapshots(locale: Locale, s3_snapshots: &UseStateHandle<Vec<S3BackupSnapshot>>) {
    let s3_state = s3_snapshots.clone();
    spawn_local(async move {
        if let Ok(loaded) = fetch_json::<Vec<S3BackupSnapshot>>(
            locale,
            Request::get(&format!("{API_ENDPOINT}/databases/s3/backups")),
        )
        .await
        {
            s3_state.set(loaded);
        }
    });
}

/// Reads the backups and the settings again, and fills the form from the settings when it is
/// still empty.
fn reload(
    locale: Locale,
    backups: &UseStateHandle<Vec<BackupEntry>>,
    config: &UseStateHandle<Option<BackupConfig>>,
    form: &UseStateHandle<FormState>,
    notice: &UseStateHandle<Option<Notice<&'static str>>>,
    pitr_range: &UseStateHandle<Option<PitrRange>>,
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
    let pitr_state = pitr_range.clone();
    spawn_local(async move {
        if let Ok(loaded) = fetch_json::<PitrRange>(
            locale,
            Request::get(&format!("{API_ENDPOINT}/databases/{id}/pitr")),
        )
        .await
        {
            pitr_state.set(Some(loaded));
        }
    });
}

fn parse_datetime_ms(input: &str) -> Option<u64> {
    let trimmed = input.trim();
    if let Ok(ms) = trimmed.parse::<u64>() {
        return Some(ms);
    }
    if let Ok(dt) =
        time::OffsetDateTime::parse(trimmed, &time::format_description::well_known::Rfc3339)
    {
        let nanos = dt.unix_timestamp_nanos();
        if nanos >= 0 {
            return Some((nanos / 1_000_000) as u64);
        }
    }
    None
}

fn pitr_restore_action(
    locale: Locale,
    id: ShortId,
    default_time: u64,
    reload: Callback<()>,
) -> Callback<MouseEvent> {
    Callback::from(move |event: MouseEvent| {
        event.prevent_default();
        let prompt_text = locale.t("databases.pitr_prompt");
        let default_val = format_time(default_time);
        let reload = reload.clone();
        spawn_local(async move {
            let Some(input) = dialog::prompt(locale, prompt_text, &default_val).await else {
                return;
            };
            let Some(target_time) = parse_datetime_ms(&input) else {
                dialog::message(
                    locale,
                    locale.t("error.pitr_target_out_of_range"),
                    Icon::Error,
                )
                .await;
                return;
            };
            let branch_prompt = locale.t("databases.pitr_branch_prompt");
            let branch_input = dialog::prompt(locale, branch_prompt, "")
                .await
                .unwrap_or_default();
            let trimmed = branch_input.trim();
            let new_name = if !trimmed.is_empty() {
                trimmed.parse().ok()
            } else {
                None
            };
            send_pitr_restore(locale, id, target_time, new_name, reload).await;
        });
    })
}

async fn send_pitr_restore(
    locale: Locale,
    id: ShortId,
    target_time: u64,
    new_name: Option<AppName>,
    reload: Callback<()>,
) {
    let req_body = PitrRestoreRequest {
        target_time,
        new_name,
        host_port: None,
    };
    let Ok(body) = serde_json::to_string(&req_body) else {
        return;
    };
    let req = Request::post(&format!("{API_ENDPOINT}/databases/{id}/pitr/restore"))
        .header("Content-Type", "application/json")
        .body(body);
    match req {
        Ok(b) => match b.send().await {
            Ok(res) if res.ok() => {
                dialog::message(locale, locale.t("databases.pitr_restored"), Icon::Success).await;
                reload.emit(());
            }
            Ok(res) => {
                let msg = res.text().await.unwrap_or_default();
                dialog::message(locale, &msg, Icon::Error).await;
            }
            Err(e) => dialog::message(locale, &e.to_string(), Icon::Error).await,
        },
        Err(e) => dialog::message(locale, &e.to_string(), Icon::Error).await,
    }
}

fn archive_wal_action(
    id: ShortId,
    notice: UseStateHandle<Option<Notice<&'static str>>>,
    reload: Callback<()>,
) -> Callback<MouseEvent> {
    Callback::from(move |event: MouseEvent| {
        event.prevent_default();
        let notice = notice.clone();
        let reload = reload.clone();
        spawn_local(async move {
            let req = Request::post(&format!("{API_ENDPOINT}/databases/{id}/wal/archive"))
                .send()
                .await;
            match req {
                Ok(res) if res.ok() => {
                    notice.set(Some(Notice::Saved("databases.wal_archived")));
                    reload.emit(());
                }
                Ok(res) => {
                    let msg = res.text().await.unwrap_or_default();
                    notice.set(Some(Notice::Failed(msg)));
                }
                Err(e) => notice.set(Some(Notice::Failed(e.to_string()))),
            }
        });
    })
}

fn pitr_banner_view(
    locale: Locale,
    id: ShortId,
    pitr: Option<&PitrRange>,
    can_edit: bool,
    notice: &UseStateHandle<Option<Notice<&'static str>>>,
    reload: &Callback<()>,
) -> Html {
    let Some(pitr) = pitr else {
        return html!();
    };
    let window_text = match (pitr.earliest, pitr.latest) {
        (Some(earliest), Some(latest)) => {
            format!("{} → {}", format_time(earliest), format_time(latest))
        }
        _ => locale.t("databases.pitr_no_window").to_string(),
    };
    let restore_btn = (can_edit && pitr.earliest.is_some()).then(|| {
        let def_time = pitr.latest.unwrap_or_else(|| pitr.earliest.unwrap_or(0));
        let onclick = pitr_restore_action(locale, id, def_time, reload.clone());
        html! {
            <button type="button" class="px-3 py-1.5 rounded bg-blue-600 text-white font-medium hover:bg-blue-500 mr-2 text-sm" {onclick}>
                {locale.t("databases.pitr_restore")}
            </button>
        }
    });
    let wal_btn = can_edit.then(|| {
        let onclick = archive_wal_action(id, notice.clone(), reload.clone());
        html! {
            <button type="button" class="px-3 py-1.5 rounded bg-neutral-700 text-neutral-200 font-medium hover:bg-neutral-600 text-sm" {onclick}>
                {locale.t("databases.wal_archive")}
            </button>
        }
    });
    html! {
        <div class="mb-4 p-3 bg-neutral-800/80 rounded border border-neutral-700 flex flex-wrap items-center justify-between gap-2">
            <div>
                <span class="font-medium text-neutral-200 mr-2">{locale.t("databases.pitr_window")}</span>
                <span class="font-mono text-neutral-300 text-sm">{window_text}</span>
                <span class="ml-3 text-xs text-neutral-400">
                    {format!("({} base, {} WAL)", pitr.base_backups_count, pitr.wal_segments_count)}
                </span>
            </div>
            <div class="flex items-center">
                { restore_btn }
                { wal_btn }
            </div>
        </div>
    }
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
    notice: UseStateHandle<Option<Notice<&'static str>>>,
    reload: Callback<()>,
) {
    busy.set(true);
    let sent = Request::post(&format!("{API_ENDPOINT}/databases/{id}/backups"))
        .build()
        .map(|request| send_request(locale, request));
    match sent {
        Ok(sent) => match sent.await {
            Ok(_) => notice.set(Some(Notice::Saved("backups.taken"))),
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
                    Err(err) => dialog::message(locale, &err.to_string(), Icon::Error).await,
                },
                Err(err) => dialog::message(locale, &err.to_string(), Icon::Error).await,
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
            let test = can_edit.then(|| {
                let (reload, backup_id) = (reload.clone(), entry.id.clone());
                Callback::from(move |_| {
                    let (reload, backup_id) = (reload.clone(), backup_id.clone());
                    spawn_local(async move {
                        let _ = Request::post(&format!("{API_ENDPOINT}/databases/{id}/backups/{backup_id}/test")).send().await;
                        reload.emit(());
                    });
                })
            });
            let status_badge = match entry.verification_status.as_deref() {
                Some("valid") => html! {
                    <span class="text-xs text-green-400 mr-3" title="Dump verified">
                        {"✓ "}{locale.t("backups.verified")}
                    </span>
                },
                Some(err) => html! {
                    <span class="text-xs text-red-400 mr-3" title={err.to_string()}>
                        {"✗ "}{locale.t("backups.verify_failed")}
                    </span>
                },
                None => {
                    if let Some(test_click) = test {
                        html! {
                            <a class="text-xs text-neutral-400 hover:text-neutral-200 mr-3 cursor-pointer" onclick={test_click}>
                                {locale.t("backups.test")}
                            </a>
                        }
                    } else {
                        html! {}
                    }
                }
            };
            html! {
                <tr class="border-b border-neutral-700">
                    <td class="py-1.5">{format_time(entry.created_at.max(0) as u64)}</td>
                    <td class="py-1.5 font-mono text-neutral-400">{format_size(entry.size)}</td>
                    <td class="py-1.5 font-mono text-neutral-400">{entry.sha256.chars().take(12).collect::<String>()}</td>
                    <td class="py-1.5 text-right">
                        {status_badge}
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

fn restore_s3_on_click(
    locale: Locale,
    id: ShortId,
    snap: &S3BackupSnapshot,
    reload: Callback<()>,
) -> Callback<MouseEvent> {
    let snap = snap.clone();
    Callback::from(move |event: MouseEvent| {
        event.prevent_default();
        let question = locale.tf("databases.s3_restore_question", &[("key", &snap.key)]);
        let reload = reload.clone();
        let key = snap.key.clone();
        dialog::confirm_then(locale, question, async move {
            let req_body = serde_json::json!({
                "key": key,
                "target_db_id": id,
            });
            let sent = Request::post(&format!("{API_ENDPOINT}/databases/s3/restore"))
                .header("Content-Type", "application/json")
                .body(req_body.to_string());
            match sent {
                Ok(req) => match send_request(locale, req).await {
                    Ok(_) => {
                        dialog::message(locale, locale.t("databases.s3_restored"), Icon::Success)
                            .await;
                        reload.emit(());
                    }
                    Err(err) => dialog::message(locale, &err.to_string(), Icon::Error).await,
                },
                Err(err) => dialog::message(locale, &err.to_string(), Icon::Error).await,
            }
        });
    })
}

fn s3_list_view(
    locale: Locale,
    id: ShortId,
    s3_snapshots: &[S3BackupSnapshot],
    can_edit: bool,
    reload: Callback<()>,
) -> Html {
    if s3_snapshots.is_empty() {
        return html!();
    }
    let rows = s3_snapshots.iter().map(|snap| {
        let download = format!("{API_ENDPOINT}/databases/s3/download?key={}", snap.key);
        let restore = can_edit.then(|| restore_s3_on_click(locale, id, snap, reload.clone()));
        html! {
            <tr class="border-b border-neutral-700">
                <td class="py-1.5 font-mono text-sm text-neutral-300">{&snap.database_name}{"/"}{&snap.backup_id}</td>
                <td class="py-1.5 font-mono text-neutral-400">{format_size(snap.size)}</td>
                <td class="py-1.5 text-neutral-400">{&snap.last_modified}</td>
                <td class="py-1.5 text-right">
                    <a class="text-sm text-neutral-400 hover:text-neutral-200 mr-3" href={download} download={format!("{}.dump.gz", snap.backup_id)}>
                        {locale.t("backups.download")}
                    </a>
                    if let Some(restore) = restore {
                        <a class="text-sm text-blue-400 hover:text-blue-300 cursor-pointer" onclick={restore}>
                            {locale.t("databases.s3_restore")}
                        </a>
                    }
                </td>
            </tr>
        }
    });
    html! {
        <div class="mt-6 rounded border border-neutral-700 p-4">
            <p class="font-medium mb-2 text-neutral-200">{locale.t("databases.s3_browser")}</p>
            <table class="w-full text-sm">
                <thead>
                    <tr class="text-left text-neutral-400">
                        <th class="py-1.5 font-medium">{locale.t("databases.engine")}</th>
                        <th class="py-1.5 font-medium">{locale.t("backups.size")}</th>
                        <th class="py-1.5 font-medium">{locale.t("backups.time")}</th>
                        <th />
                    </tr>
                </thead>
                <tbody>{ for rows }</tbody>
            </table>
        </div>
    }
}

/// The settings form of the schedule and the S3 target.
fn settings_view(
    locale: Locale,
    config: &UseStateHandle<Option<BackupConfig>>,
    form: &UseStateHandle<FormState>,
    notice: &UseStateHandle<Option<Notice<&'static str>>>,
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
                            notice.set(Some(Notice::Saved("backups.saved")));
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
                <label class="flex items-center space-x-2 md:col-span-2">
                    <input type="checkbox" checked={form.restore_test_enabled}
                        onchange={checkbox_input(form, |state| &mut state.restore_test_enabled)}
                    />
                    <span class="text-sm text-neutral-300">{locale.t("backups.restore_test_enabled")}</span>
                </label>
                <label class="block md:col-span-2">
                    <span class="text-sm text-neutral-400">{locale.t("backups.heartbeat_url")}</span>
                    <input type="text" class={INPUT_CLASS} placeholder="https://hc-ping.com/..."
                        value={form.heartbeat_url.clone()}
                        onchange={text_input(form, |state| &mut state.heartbeat_url)}
                    />
                    <span class="text-xs text-neutral-500">{locale.t("backups.heartbeat_url_hint")}</span>
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
            <p class="font-medium mt-4 mb-2">{locale.t("backups.sftp")}</p>
            <div class="grid gap-3 md:grid-cols-2">
                <label class="block">
                    <span class="text-sm text-neutral-400">{locale.t("backups.sftp_host")}</span>
                    <input type="text" class={INPUT_CLASS} placeholder="sftp.example.com"
                        value={form.sftp_host.clone()}
                        onchange={text_input(form, |state| &mut state.sftp_host)}
                    />
                </label>
                <label class="block">
                    <span class="text-sm text-neutral-400">{locale.t("backups.sftp_port")}</span>
                    <input type="text" class={INPUT_CLASS} placeholder="22"
                        value={form.sftp_port.clone()}
                        onchange={text_input(form, |state| &mut state.sftp_port)}
                    />
                </label>
                <label class="block">
                    <span class="text-sm text-neutral-400">{locale.t("backups.sftp_user")}</span>
                    <input type="text" class={INPUT_CLASS}
                        value={form.sftp_user.clone()}
                        onchange={text_input(form, |state| &mut state.sftp_user)}
                    />
                </label>
                <label class="block">
                    <span class="text-sm text-neutral-400">{locale.t("backups.sftp_path")}</span>
                    <input type="text" class={INPUT_CLASS} placeholder="/backups"
                        value={form.sftp_path.clone()}
                        onchange={text_input(form, |state| &mut state.sftp_path)}
                    />
                </label>
                <label class="block">
                    <span class="text-sm text-neutral-400">{locale.t("backups.sftp_password")}</span>
                    <input type="password" class={INPUT_CLASS}
                        value={form.sftp_password.clone()}
                        onchange={text_input(form, |state| &mut state.sftp_password)}
                    />
                </label>
                <label class="block">
                    <span class="text-sm text-neutral-400">{locale.t("backups.sftp_key")}</span>
                    <textarea rows="2" class={INPUT_CLASS}
                        value={form.sftp_key.clone()}
                        onchange={textarea_input(form, |state| &mut state.sftp_key)}
                    />
                </label>
            </div>
            <p class="font-medium mt-4 mb-2">{locale.t("backups.rsync")}</p>
            <div class="grid gap-3 md:grid-cols-2">
                <label class="block md:col-span-2">
                    <span class="text-sm text-neutral-400">{locale.t("backups.rsync_destination")}</span>
                    <input type="text" class={INPUT_CLASS} placeholder="user@remote.example.com:/backups"
                        value={form.rsync_dest.clone()}
                        onchange={text_input(form, |state| &mut state.rsync_dest)}
                    />
                </label>
                <label class="block">
                    <span class="text-sm text-neutral-400">{locale.t("backups.rsync_ssh_options")}</span>
                    <input type="text" class={INPUT_CLASS} placeholder="-p 22 -i /path/key"
                        value={form.rsync_ssh_options.clone()}
                        onchange={text_input(form, |state| &mut state.rsync_ssh_options)}
                    />
                </label>
                <label class="block">
                    <span class="text-sm text-neutral-400">{locale.t("backups.rsync_password")}</span>
                    <input type="password" class={INPUT_CLASS}
                        value={form.rsync_password.clone()}
                        onchange={text_input(form, |state| &mut state.rsync_password)}
                    />
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

/// Returns a callback that writes the checked state of a checkbox to a field of the form.
fn checkbox_input(
    form: &UseStateHandle<FormState>,
    select: fn(&mut FormState) -> &mut bool,
) -> Callback<Event> {
    let form = form.clone();
    Callback::from(move |event: Event| {
        let input: web_sys::HtmlInputElement = event.target_unchecked_into();
        let mut updated = (*form).clone();
        *select(&mut updated) = input.checked();
        form.set(updated);
    })
}

/// Returns a callback that writes the value of a textarea to a field of the form.
fn textarea_input(
    form: &UseStateHandle<FormState>,
    select: fn(&mut FormState) -> &mut String,
) -> Callback<Event> {
    let form = form.clone();
    Callback::from(move |event: Event| {
        let input: web_sys::HtmlTextAreaElement = event.target_unchecked_into();
        let mut updated = (*form).clone();
        *select(&mut updated) = input.value();
        form.set(updated);
    })
}
