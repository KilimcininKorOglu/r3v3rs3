//! The backups of one managed database: take, download, restore, test and delete.

use super::Route;
use super::app_deployments::back_header;
use super::resource_page::{Notice, delete_on_click, notice_view};
use super::settings::{fetch_json, send_request};
use crate::API_ENDPOINT;
use crate::auth::use_ensure_auth;
use crate::components::action_button::{ActionIconButton, ActionVariant};
use crate::components::backup_drill_modal::BackupDrillModal;
use crate::components::wal_segments_modal::WalSegmentsModal;
use crate::dialog::{self, Icon};
use crate::format::{format_bytes, format_time};
use crate::i18n::use_locale;
use crate::store::SessionStore;
use gloo_net::http::Request;
use r3v3rs3_api::container::AppName;
use r3v3rs3_api::i18n::Locale;
use r3v3rs3_api::id::ShortId;
use r3v3rs3_api::platform::{BackupEntry, PitrRange, PitrRestoreRequest};
use wasm_bindgen_futures::spawn_local;
use yew::prelude::*;
use yew_router::prelude::*;
use yewdux::prelude::*;

const BUTTON_CLASS: &str =
    "px-3 py-1.5 rounded bg-neutral-100 text-neutral-900 font-medium disabled:opacity-50";

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
    let id = props.id;

    let signed_in = session.info.is_some();
    let can_edit = session.can_edit();
    {
        let (backups, notice, pitr_range) = (backups.clone(), notice.clone(), pitr_range.clone());
        use_effect_with((signed_in, id), move |&(signed_in, id)| {
            if signed_in {
                reload(locale, &backups, &notice, &pitr_range, id);
            }
        });
    }

    if session.info.is_none() {
        return html!();
    }
    let navigator = use_navigator().unwrap();
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
            { back_header(locale, &navigator, locale.t("backups.title").to_string()) }
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
            <button type="button" class="px-3 py-1.5 rounded bg-blue-600 text-white font-medium hover:bg-blue-500 mr-2 text-sm" {onclick}>
                {locale.t("databases.pitr_restore")}
            </button>
        }
    });
    let wal_view_btn = html! {
        <button
            type="button"
            class="px-3 py-1.5 rounded bg-neutral-700 text-neutral-200 font-medium hover:bg-neutral-600 text-sm mr-2 inline-flex items-center gap-1.5"
            onclick={on_open_wal.clone()}
        >
            <svg class="w-4 h-4 text-cyan-400" fill="none" stroke="currentColor" viewBox="0 0 24 24">
                <path stroke-linecap="round" stroke-linejoin="round" stroke-width="2" d="M15 12a3 3 0 11-6 0 3 3 0 016 0z" />
                <path stroke-linecap="round" stroke-linejoin="round" stroke-width="2" d="M2.458 12C3.732 7.943 7.523 5 12 5c4.478 0 8.268 2.943 9.542 7-1.274 4.057-5.064 7-9.542 7-4.477 0-8.268-2.943-9.542-7z" />
            </svg>
            {locale.t("databases.wal_segments")}
        </button>
    };
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
            <div class="flex items-center flex-wrap gap-2">
                <span class="font-medium text-neutral-200 mr-1">{locale.t("databases.pitr_window")}</span>
                <span class="font-mono text-neutral-300 text-sm">{window_text}</span>
                <span class="text-xs text-neutral-400">
                    {format!("({} base,", pitr.base_backups_count)}
                </span>
                <button
                    type="button"
                    class="inline-flex items-center gap-1 px-1.5 py-0.5 rounded text-xs font-mono bg-neutral-700/80 hover:bg-neutral-600 text-cyan-300 hover:text-cyan-200 transition cursor-pointer"
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
            <div class="flex items-center">
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
    html! {
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
                if let Ok(req) = Request::post(&path).build() {
                    let _ = send_request(locale, req).await;
                    reload.emit(());
                    s.set(None);
                }
                drill_running.set(false);
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

/// The table of the backups of the database.
fn list_view(
    locale: Locale,
    id: ShortId,
    backups: &[BackupEntry],
    can_edit: bool,
    reload: Callback<()>,
    selected_drill_backup: UseStateHandle<Option<BackupEntry>>,
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
            let on_drill_click = {
                let s = selected_drill_backup.clone();
                let e = entry.clone();
                Callback::from(move |ev: MouseEvent| {
                    ev.prevent_default();
                    s.set(Some(e.clone()));
                })
            };
            let status_badge = match entry.verification_status.as_deref() {
                Some("valid") => {
                    let label = if let Some(drill) = &entry.drill && drill.table_count > 0 {
                        locale.tf(
                            "backups.drill_passed_tables",
                            &[("count", &drill.table_count.to_string())],
                        )
                    } else {
                        format!("✓ {}", locale.t("backups.verified"))
                    };
                    html! {
                        <button type="button" class="text-xs text-green-400 hover:text-green-300 mr-3 cursor-pointer inline-flex items-center gap-1 font-medium" onclick={on_drill_click} title={locale.t("backups.view_drill")}>
                            {label}
                        </button>
                    }
                }
                Some(err) => html! {
                    <button type="button" class="text-xs text-red-400 hover:text-red-300 mr-3 cursor-pointer inline-flex items-center gap-1 font-medium" onclick={on_drill_click} title={err.to_string()}>
                        {"✗ "}{locale.t("backups.verify_failed")}
                    </button>
                },
                None => html! {
                    <button type="button" class="text-xs text-neutral-400 hover:text-neutral-200 mr-3 cursor-pointer font-medium" onclick={on_drill_click} title={locale.t("backups.view_drill")}>
                        {locale.t("backups.test")}
                    </button>
                },
            };
            html! {
                <tr class="border-b border-neutral-700">
                    <td class="py-1.5">{format_time(entry.created_at.max(0) as u64)}</td>
                    <td class="py-1.5 font-mono text-neutral-400">{format_bytes(entry.size)}</td>
                    <td class="py-1.5 font-mono text-neutral-400">{entry.sha256.chars().take(12).collect::<String>()}</td>
                    <td class="py-1.5 text-right">
                        <div class="inline-flex items-center justify-end gap-1.5">
                            {status_badge}
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
                        </div>
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

fn storage_settings_redirect(locale: Locale) -> Html {
    html! {
        <div class="mt-6 rounded border border-neutral-700 p-4 bg-neutral-800/40 flex flex-col sm:flex-row sm:items-center justify-between gap-4">
            <div>
                <p class="font-medium text-neutral-200">{locale.t("backups.storage_settings_title")}</p>
                <p class="text-xs text-neutral-400 mt-1">{locale.t("backups.storage_settings_hint")}</p>
            </div>
            <Link<Route>
                to={Route::Backups}
                classes="px-3 py-1.5 rounded bg-blue-600 text-white hover:bg-blue-500 text-sm font-medium shrink-0 inline-flex items-center gap-1.5 justify-center"
            >
                {locale.t("backups.manage_storage")}
            </Link<Route>>
        </div>
    }
}
