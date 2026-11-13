//! The backups of one managed database: take, download, restore, test and delete.

use super::Route;
use super::app_deployments::back_header;
use super::resource_page::{Notice, delete_on_click, notice_view};
use super::settings::{fetch_json, send_json, send_request};
use crate::API_ENDPOINT;
use crate::auth::use_ensure_auth;
use crate::components::action_button::{ActionIconButton, ActionVariant};
use crate::components::app_tabs::{AppTab, AppTabs};
use crate::components::backup_drill_modal::BackupDrillModal;
use crate::components::button::{Button, ButtonKind, ButtonSize, button_class};
use crate::components::data_list::{Column, Row, list_card};
use crate::components::wal_segments_modal::WalSegmentsModal;
use crate::dialog::{self, Icon};
use crate::format::{format_bytes, format_time};
use crate::i18n::use_locale;
use crate::store::SessionStore;
use gloo_net::http::Request;
use r3v3rs3_api::container::AppName;
use r3v3rs3_api::i18n::Locale;
use r3v3rs3_api::id::ShortId;
use r3v3rs3_api::platform::{AppEntry, BackupEntry, PitrRange, PitrRestoreRequest};
use wasm_bindgen_futures::spawn_local;
use yew::prelude::*;
use yew_router::prelude::*;
use yewdux::prelude::*;

#[function_component(DatabaseBackups)]
pub fn database_backups(props: &Props) -> Html {
    use_ensure_auth();
    let locale = use_locale();
    let (session, _) = use_store::<SessionStore>();
    let backups = use_state(Vec::<BackupEntry>::new);
    let pitr_range = use_state(Option::<PitrRange>::default);
    let notice = use_state(Option::<Notice<&'static str>>::default);
    let selected_drill_backup = use_state(Option::<BackupEntry>::default);
    let drill_running = use_state(bool::default);
    let show_wal_modal = use_state(bool::default);
    let busy = use_state(bool::default);
    let app = use_state(|| Option::<AppEntry>::None);
    let id = props.id;

    let signed_in = session.info.is_some();
    let can_edit = session.can_edit();
    {
        let (backups, notice, pitr_range, app) = (
            backups.clone(),
            notice.clone(),
            pitr_range.clone(),
            app.clone(),
        );
        use_effect_with((signed_in, id), move |&(signed_in, id)| {
            if signed_in {
                reload(locale, &backups, &notice, &pitr_range, id);
                load_app(locale, id, &app);
            }
        });
    }

    if session.info.is_none() {
        return html!();
    }
    let Some(navigator) = use_navigator() else {
        return html!();
    };
    let reload = make_reload_cb(
        locale,
        backups.clone(),
        notice.clone(),
        pitr_range.clone(),
        id,
    );
    let drill_modal = build_drill_modal(
        locale,
        id,
        &selected_drill_backup,
        reload.clone(),
        &drill_running,
    );
    let on_open_wal = {
        let s = show_wal_modal.clone();
        Callback::from(move |ev: MouseEvent| {
            ev.prevent_default();
            s.set(true);
        })
    };
    let wal_modal = (*show_wal_modal).then(|| {
        let on_close = {
            let s = show_wal_modal.clone();
            Callback::from(move |()| s.set(false))
        };
        let on_archived = {
            let reload = reload.clone();
            Some(Callback::from(move |()| reload.emit(())))
        };
        html! {
            <WalSegmentsModal
                db_id={id}
                on_close={on_close}
                can_edit={can_edit}
                on_archived={on_archived}
            />
        }
    });

    html! {
        <>
            if let Some(a) = (*app).as_ref() {
                <AppTabs
                    id={a.id}
                    name={a.name.to_string()}
                    active={AppTab::Backups}
                    is_database=true
                    domains={a.spec.domains.clone()}
                />
            } else {
                { back_header(locale, &navigator, locale.t("backups.title").to_string()) }
            }
            { notice_view(&notice, |key| html! { {locale.t(key)} }) }
            { pitr_banner_view(locale, id, (*pitr_range).as_ref(), can_edit, &notice, &reload, on_open_wal) }
            if can_edit {
                { take_backup_button(locale, id, busy.clone(), notice.clone(), reload.clone()) }
            }
            { list_view(locale, id, &backups, can_edit, reload.clone(), selected_drill_backup.clone()) }
            { drill_modal }
            { wal_modal }
            { storage_settings_redirect(locale) }
        </>
    }
}

fn load_app(locale: Locale, id: ShortId, app: &UseStateHandle<Option<AppEntry>>) {
    let app = app.clone();
    spawn_local(async move {
        if let Ok(entry) =
            fetch_json::<AppEntry>(locale, Request::get(&format!("{API_ENDPOINT}/apps/{id}"))).await
        {
            app.set(Some(entry));
        }
    });
}

#[derive(Properties, PartialEq)]
pub struct Props {
    pub id: ShortId,
}

fn make_reload_cb(
    locale: Locale,
    backups: UseStateHandle<Vec<BackupEntry>>,
    notice: UseStateHandle<Option<Notice<&'static str>>>,
    pitr_range: UseStateHandle<Option<PitrRange>>,
    id: ShortId,
) -> Callback<()> {
    Callback::from(move |()| {
        reload(locale, &backups, &notice, &pitr_range, id);
    })
}

/// Reads the backups and the PITR range of the database.
fn reload(
    locale: Locale,
    backups: &UseStateHandle<Vec<BackupEntry>>,
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
            // A cancelled prompt stops the restore; an empty name restores in place.
            let Some(branch_input) = dialog::prompt(locale, branch_prompt, "").await else {
                return;
            };
            match branch_name(locale, &branch_input) {
                Ok(new_name) => send_pitr_restore(locale, id, target_time, new_name, reload).await,
                Err(message) => dialog::message(locale, &message, Icon::Error).await,
            }
        });
    })
}

/// The name of the new database of a restore: `None` for an empty input, which restores in
/// place, or the error of an invalid name.
fn branch_name(locale: Locale, input: &str) -> Result<Option<AppName>, String> {
    let trimmed = input.trim();
    if trimmed.is_empty() {
        return Ok(None);
    }
    trimmed
        .parse()
        .map(Some)
        .map_err(|err| locale.error_message(&err))
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
    let builder = Request::post(&format!("{API_ENDPOINT}/databases/{id}/pitr/restore"));
    match send_json(locale, builder, &req_body).await {
        Ok(()) => {
            dialog::message(locale, locale.t("databases.pitr_restored"), Icon::Success).await;
            reload.emit(());
        }
        Err(message) => dialog::message(locale, &message, Icon::Error).await,
    }
}

fn archive_wal_action(
    locale: Locale,
    id: ShortId,
    notice: UseStateHandle<Option<Notice<&'static str>>>,
    reload: Callback<()>,
) -> Callback<MouseEvent> {
    Callback::from(move |event: MouseEvent| {
        event.prevent_default();
        let notice = notice.clone();
        let reload = reload.clone();
        spawn_local(async move {
            let path = format!("{API_ENDPOINT}/databases/{id}/wal/archive");
            let sent = match Request::post(&path).build() {
                Ok(request) => send_request(locale, request).await.map(|_| ()),
                Err(err) => Err(err.to_string()),
            };
            match sent {
                Ok(()) => {
                    notice.set(Some(Notice::Saved("databases.wal_archived")));
                    reload.emit(());
                }
                Err(message) => notice.set(Some(Notice::Failed(message))),
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
    on_open_wal: Callback<MouseEvent>,
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
            <Button kind={ButtonKind::Primary} size={ButtonSize::Sm} {onclick}>
                {locale.t("databases.pitr_restore")}
            </Button>
        }
    });
    let wal_view_btn = html! {
        <Button kind={ButtonKind::Secondary} size={ButtonSize::Sm} onclick={on_open_wal.clone()}>
            <svg class="w-4 h-4 text-cyan-600 dark:text-cyan-400" fill="none" stroke="currentColor" viewBox="0 0 24 24">
                <path stroke-linecap="round" stroke-linejoin="round" stroke-width="2" d="M15 12a3 3 0 11-6 0 3 3 0 016 0z" />
                <path stroke-linecap="round" stroke-linejoin="round" stroke-width="2" d="M2.458 12C3.732 7.943 7.523 5 12 5c4.478 0 8.268 2.943 9.542 7-1.274 4.057-5.064 7-9.542 7-4.477 0-8.268-2.943-9.542-7z" />
            </svg>
            {locale.t("databases.wal_segments")}
        </Button>
    };
    let wal_btn = can_edit.then(|| {
        let onclick = archive_wal_action(locale, id, notice.clone(), reload.clone());
        html! {
            <Button kind={ButtonKind::Secondary} size={ButtonSize::Sm} {onclick}>
                {locale.t("databases.wal_archive")}
            </Button>
        }
    });
    html! {
        <div class="mb-4 p-3 bg-white dark:bg-neutral-800 shadow-sm border border-neutral-300 dark:border-neutral-700 rounded-md flex flex-wrap items-center justify-between gap-2">
            <div class="flex items-center flex-wrap gap-2">
                <span class="font-medium text-neutral-900 dark:text-neutral-200 mr-1">{locale.t("databases.pitr_window")}</span>
                <span class="font-mono text-neutral-700 dark:text-neutral-300 text-sm">{window_text}</span>
                <span class="text-xs text-neutral-500 dark:text-neutral-400">
                    {format!("({} base,", pitr.base_backups_count)}
                </span>
                <button
                    type="button"
                    class="inline-flex items-center gap-1 px-1.5 py-0.5 rounded text-xs font-mono bg-neutral-100 hover:bg-neutral-200 text-cyan-700 dark:bg-neutral-700 dark:hover:bg-neutral-600 dark:text-cyan-300 transition cursor-pointer"
                    onclick={on_open_wal}
                    title={locale.t("databases.view_wal_segments")}
                >
                    <svg class="w-3.5 h-3.5" fill="none" stroke="currentColor" viewBox="0 0 24 24">
                        <path stroke-linecap="round" stroke-linejoin="round" stroke-width="2" d="M15 12a3 3 0 11-6 0 3 3 0 016 0z" />
                        <path stroke-linecap="round" stroke-linejoin="round" stroke-width="2" d="M2.458 12C3.732 7.943 7.523 5 12 5c4.478 0 8.268 2.943 9.542 7-1.274 4.057-5.064 7-9.542 7-4.477 0-8.268-2.943-9.542-7z" />
                    </svg>
                    <span>{format!("{} WAL)", pitr.wal_segments_count)}</span>
                </button>
            </div>
            <div class="flex flex-wrap items-center gap-2">
                { restore_btn }
                { wal_view_btn }
                { wal_btn }
            </div>
        </div>
    }
}

fn take_backup_button(
    locale: Locale,
    id: ShortId,
    busy: UseStateHandle<bool>,
    notice: UseStateHandle<Option<Notice<&'static str>>>,
    reload: Callback<()>,
) -> Html {
    let onclick = {
        let (busy, notice, reload) = (busy.clone(), notice.clone(), reload.clone());
        Callback::from(move |_| {
            spawn_local(take_backup(
                locale,
                id,
                busy.clone(),
                notice.clone(),
                reload.clone(),
            ));
        })
    };
    html! {
        <div class="mb-4">
            <Button kind={ButtonKind::Success} disabled={*busy} {onclick}>
                {locale.t("backups.take")}
            </Button>
        </div>
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

fn build_drill_modal(
    locale: Locale,
    id: ShortId,
    selected_drill_backup: &UseStateHandle<Option<BackupEntry>>,
    reload: Callback<()>,
    drill_running: &UseStateHandle<bool>,
) -> Option<Html> {
    let entry = (**selected_drill_backup).clone()?;
    let on_close = {
        let s = selected_drill_backup.clone();
        Callback::from(move |()| s.set(None))
    };
    let on_rerun = {
        let (db_id, backup_id) = (id, entry.id.clone());
        let reload = reload.clone();
        let s = selected_drill_backup.clone();
        let drill_running = drill_running.clone();
        Callback::from(move |()| {
            let (s, reload, drill_running) = (s.clone(), reload.clone(), drill_running.clone());
            let path = format!("{API_ENDPOINT}/databases/{db_id}/backups/{backup_id}/test");
            drill_running.set(true);
            spawn_local(async move {
                let sent = match Request::post(&path).build() {
                    Ok(req) => send_request(locale, req).await.map(|_| ()),
                    Err(err) => Err(err.to_string()),
                };
                drill_running.set(false);
                reload.emit(());
                match sent {
                    Ok(()) => s.set(None),
                    Err(message) => dialog::message(locale, &message, Icon::Error).await,
                }
            });
        })
    };
    Some(html! {
        <BackupDrillModal
            entry={entry}
            on_close={on_close}
            on_rerun={on_rerun}
            is_running={**drill_running}
        />
    })
}

const COLUMNS: [Column; 4] = [
    Column {
        label: "backups.time",
        class: "whitespace-nowrap",
    },
    Column {
        label: "backups.size",
        class: "whitespace-nowrap font-mono",
    },
    Column {
        label: "backups.sha256",
        class: "font-mono",
    },
    Column {
        label: "common.status",
        class: "whitespace-nowrap",
    },
];

const DRILL_BUTTON: &str = "text-xs font-medium cursor-pointer hover:underline";

/// The verification of a backup; a click opens its drill.
fn verification_cell(locale: Locale, entry: &BackupEntry, onclick: Callback<MouseEvent>) -> Html {
    match entry.verification_status.as_deref() {
        Some("valid") => {
            let label = if let Some(drill) = &entry.drill
                && drill.table_count > 0
            {
                locale.tf(
                    "backups.drill_passed_tables",
                    &[("count", &drill.table_count.to_string())],
                )
            } else {
                format!("✓ {}", locale.t("backups.verified"))
            };
            html! {
                <button type="button" class={classes!(DRILL_BUTTON, "text-green-600", "dark:text-green-400")} {onclick} title={locale.t("backups.view_drill")}>
                    {label}
                </button>
            }
        }
        Some(err) => html! {
            <button type="button" class={classes!(DRILL_BUTTON, "text-red-600", "dark:text-red-400")} {onclick} title={err.to_string()}>
                {"✗ "}{locale.t("backups.verify_failed")}
            </button>
        },
        None => html! {
            <button type="button" class={classes!(DRILL_BUTTON, "text-neutral-500", "dark:text-neutral-400")} {onclick} title={locale.t("backups.view_drill")}>
                {locale.t("backups.test")}
            </button>
        },
    }
}

/// The download, restore and delete actions of a backup.
fn backup_actions(
    locale: Locale,
    id: ShortId,
    entry: &BackupEntry,
    can_edit: bool,
    reload: &Callback<()>,
) -> Html {
    let download = format!(
        "{API_ENDPOINT}/databases/{id}/backups/{}/download",
        entry.id
    );
    let restore = can_edit.then(|| restore_on_click(locale, id, entry));
    let delete = can_edit.then(|| {
        let question = locale.tf("backups.delete_question", &[("name", entry.id.as_str())]);
        delete_on_click(
            locale,
            question,
            format!("/databases/{id}/backups/{}", entry.id),
            reload.clone(),
        )
    });
    html! {
        <>
            <ActionIconButton
                variant={ActionVariant::Download}
                title={locale.t("backups.download").to_string()}
                href={download}
                download={format!("{}.dump.gz", entry.id)}
            />
            if let Some(restore) = restore {
                <ActionIconButton
                    variant={ActionVariant::Restore}
                    title={locale.t("backups.restore").to_string()}
                    onclick={restore}
                />
            }
            if let Some(delete) = delete {
                <ActionIconButton
                    variant={ActionVariant::Delete}
                    title={locale.t("common.delete").to_string()}
                    onclick={delete}
                />
            }
        </>
    }
}

/// The list of the backups of the database.
fn list_view(
    locale: Locale,
    id: ShortId,
    backups: &[BackupEntry],
    can_edit: bool,
    reload: Callback<()>,
    selected_drill_backup: UseStateHandle<Option<BackupEntry>>,
) -> Html {
    let rows = backups
        .iter()
        .map(|entry| {
            let on_drill_click = {
                let s = selected_drill_backup.clone();
                let e = entry.clone();
                Callback::from(move |ev: MouseEvent| {
                    ev.prevent_default();
                    s.set(Some(e.clone()));
                })
            };
            Row {
                key: entry.id.clone(),
                cells: vec![
                    html! { {format_time(entry.created_at.max(0) as u64)} },
                    html! { {format_bytes(entry.size)} },
                    html! { {entry.sha256.chars().take(12).collect::<String>()} },
                    verification_cell(locale, entry, on_drill_click),
                ],
                actions: backup_actions(locale, id, entry, can_edit, &reload),
            }
        })
        .collect::<Vec<_>>();
    list_card(locale, true, "backups.empty", &COLUMNS, &rows)
}

fn storage_settings_redirect(locale: Locale) -> Html {
    html! {
        <div class="mt-6 p-4 bg-white dark:bg-neutral-800 shadow-sm border border-neutral-300 dark:border-neutral-700 rounded-md flex flex-col sm:flex-row sm:items-center justify-between gap-4">
            <div>
                <p class="font-medium text-neutral-900 dark:text-neutral-200">{locale.t("backups.storage_settings_title")}</p>
                <p class="text-xs text-neutral-500 dark:text-neutral-400 mt-1">{locale.t("backups.storage_settings_hint")}</p>
            </div>
            <Link<Route>
                to={Route::Backups}
                classes={button_class(ButtonKind::Secondary, ButtonSize::Md)}
            >
                {locale.t("backups.manage_storage")}
            </Link<Route>>
        </div>
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_invalid_branch_name_stops_the_restore() {
        assert_eq!(branch_name(Locale::En, "  "), Ok(None));
        assert_eq!(
            branch_name(Locale::En, " orders-copy ").map(|name| name.map(|n| n.to_string())),
            Ok(Some("orders-copy".to_string()))
        );
        assert!(branch_name(Locale::En, "Not A Name!").is_err());
    }
}
