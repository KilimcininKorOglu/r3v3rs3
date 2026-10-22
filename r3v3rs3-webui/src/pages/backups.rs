//! Centralized database backups, automated schedules, and disaster recovery center.

use super::app_files::format_bytes;
use super::resource_page::{Notice, delete_on_click, notice_view};
use super::settings::{fetch_json, send_json, send_request};
use crate::API_ENDPOINT;
use crate::auth::use_ensure_auth;
use crate::components::backup_drill_modal::BackupDrillModal;
use crate::dialog::{self, Icon};
use crate::format::format_time;
use crate::i18n::use_locale;
use crate::store::SessionStore;
use gloo_net::http::Request;
use r3v3rs3_api::i18n::Locale;
use r3v3rs3_api::id::ShortId;
use r3v3rs3_api::platform::{
    AppEntry, BackupConfig, BackupEntry, BackupsOverview, RunBackupRequest, S3BackupSnapshot,
    S3Config,
};
use wasm_bindgen_futures::spawn_local;
use yew::prelude::*;
use yewdux::prelude::*;

const INPUT_CLASS: &str = "w-full rounded bg-neutral-800 text-neutral-100 border border-neutral-700 px-3 py-1.5 text-sm focus:border-neutral-500 focus:outline-none";
const BUTTON_PRIMARY: &str = "px-3 py-1.5 rounded bg-blue-600 hover:bg-blue-500 text-white font-medium text-sm transition-colors flex items-center gap-1.5 disabled:opacity-50";
const BUTTON_SECONDARY: &str = "px-3 py-1.5 rounded bg-neutral-800 hover:bg-neutral-700 text-neutral-200 border border-neutral-700 font-medium text-sm transition-colors flex items-center gap-1.5";

/// The backup settings form state.
#[derive(Clone, PartialEq, Default)]
pub struct FormState {
    pub schedule: String,
    pub keep: String,
    pub heartbeat_url: String,
    pub endpoint: String,
    pub region: String,
    pub prefix: String,
    pub access_key: String,
    pub secret_key: String,
    pub restore_test_enabled: bool,
}

impl FormState {
    pub fn from_config(config: &BackupConfig) -> Self {
        let s3 = config.s3.clone().unwrap_or_default();
        Self {
            schedule: config.schedule.clone(),
            keep: config.keep.to_string(),
            heartbeat_url: config.heartbeat_url.clone().unwrap_or_default(),
            endpoint: s3.endpoint,
            region: s3.region,
            prefix: s3.prefix,
            access_key: s3.access_key,
            secret_key: String::new(),
            restore_test_enabled: config.restore_test_enabled,
        }
    }

    pub fn to_config(&self, original: &BackupConfig) -> BackupConfig {
        let s3 = (!self.endpoint.trim().is_empty()).then(|| S3Config {
            endpoint: self.endpoint.trim().to_string(),
            region: self.region.trim().to_string(),
            prefix: self.prefix.trim().to_string(),
            access_key: self.access_key.trim().to_string(),
            secret_key: self.secret_key.clone(),
        });
        BackupConfig {
            schedule: self.schedule.trim().to_string(),
            keep: self.keep.trim().parse().unwrap_or(7).max(1),
            s3,
            sftp: original.sftp.clone(),
            rsync: original.rsync.clone(),
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

/// Filter backups by search term and engine.
pub fn filter_backups(backups: &[BackupEntry], search: &str, engine: &str) -> Vec<BackupEntry> {
    let query = search.trim().to_lowercase();
    let engine_filter = engine.trim().to_lowercase();
    backups
        .iter()
        .filter(|b| {
            let matches_engine = engine_filter.is_empty()
                || engine_filter == "all"
                || b.engine.to_lowercase() == engine_filter;
            if !matches_engine {
                return false;
            }
            if query.is_empty() {
                return true;
            }
            b.name.to_lowercase().contains(&query)
                || b.id.to_lowercase().contains(&query)
                || b.engine.to_lowercase().contains(&query)
        })
        .cloned()
        .collect()
}

/// Format the scheduled plan text.
pub fn format_schedule_text(locale: Locale, schedule: &str) -> String {
    let trimmed = schedule.trim();
    if trimmed.is_empty() {
        locale.t("backups.disabled").to_string()
    } else {
        locale.tf("backups.daily_utc", &[("time", trimmed)])
    }
}

fn default_engine() -> String {
    "all".to_string()
}

#[function_component(Backups)]
pub fn backups() -> Html {
    use_ensure_auth();
    let locale = use_locale();
    let (session, _) = use_store::<SessionStore>();
    let overview = use_state(Option::<BackupsOverview>::default);
    let backups = use_state(Vec::<BackupEntry>::new);
    let databases = use_state(Vec::<AppEntry>::new);
    let s3_snapshots = use_state(Vec::<S3BackupSnapshot>::new);
    let config = use_state(BackupConfig::default);
    let form = use_state(FormState::default);
    let search = use_state(String::new);
    let engine = use_state(default_engine);
    let show_settings = use_state(bool::default);
    let show_take = use_state(bool::default);
    let show_s3 = use_state(bool::default);
    let selected_db = use_state(Option::<ShortId>::default);
    let notice = use_state(Option::<Notice<&'static str>>::default);
    let selected_drill_backup = use_state(|| None::<BackupEntry>);
    let drill_running = use_state(|| false);
    let can_edit = session.can_edit();

    let reload_cb = {
        let (overview, backups, databases, s3_snapshots, config, form, notice) = (
            overview.clone(),
            backups.clone(),
            databases.clone(),
            s3_snapshots.clone(),
            config.clone(),
            form.clone(),
            notice.clone(),
        );
        Callback::from(move |_| {
            load_data(
                locale,
                overview.clone(),
                backups.clone(),
                databases.clone(),
                s3_snapshots.clone(),
                config.clone(),
                form.clone(),
                notice.clone(),
            );
        })
    };

    {
        let reload = reload_cb.clone();
        use_effect_with((), move |_| {
            reload.emit(());
        });
    }

    let filtered = filter_backups(&backups, &search, &engine);

    let on_select_drill = {
        let s = selected_drill_backup.clone();
        Callback::from(move |entry: BackupEntry| s.set(Some(entry)))
    };

    let drill_modal = (*selected_drill_backup).clone().map(|entry| {
        let on_close = {
            let s = selected_drill_backup.clone();
            Callback::from(move |()| s.set(None))
        };
        let on_rerun = {
            let (id, backup_id) = (entry.app, entry.id.clone());
            let reload = reload_cb.clone();
            let s = selected_drill_backup.clone();
            let drill_running = drill_running.clone();
            Callback::from(move |()| {
                let (s, reload, drill_running) = (s.clone(), reload.clone(), drill_running.clone());
                let path = format!("{API_ENDPOINT}/databases/{id}/backups/{backup_id}/test");
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
        html! {
            <BackupDrillModal
                entry={entry}
                on_close={on_close}
                on_rerun={on_rerun}
                is_running={*drill_running}
            />
        }
    });

    html! {
        <div class="space-y-6">
            { render_header(locale, can_edit, &show_s3, &show_settings, &show_take) }

            { notice_view(&notice, |key| html! { {locale.t(key)} }) }

            { render_kpi_cards(overview.as_ref(), locale) }

            if *show_s3 {
                { render_s3_drawer(locale, &s3_snapshots, can_edit, reload_cb.clone()) }
            }

            { render_filter_bar(locale, &search, &engine) }

            <div class="rounded-lg border border-neutral-800 bg-neutral-900/60 overflow-hidden">
                { render_backups_table(locale, &filtered, can_edit, reload_cb.clone(), on_select_drill) }
            </div>

            { drill_modal }

            { render_modals(locale, &show_take, &show_settings, &databases, &selected_db, &config, &form, &notice, reload_cb) }
        </div>
    }
}

#[allow(clippy::too_many_arguments)]
fn render_modals(
    locale: Locale,
    show_take: &UseStateHandle<bool>,
    show_settings: &UseStateHandle<bool>,
    databases: &UseStateHandle<Vec<AppEntry>>,
    selected_db: &UseStateHandle<Option<ShortId>>,
    config: &UseStateHandle<BackupConfig>,
    form: &UseStateHandle<FormState>,
    notice: &UseStateHandle<Option<Notice<&'static str>>>,
    reload: Callback<()>,
) -> Html {
    html! {
        <>
            if **show_take {
                { render_take_modal(locale, databases.clone(), selected_db.clone(), show_take.clone(), notice.clone(), reload.clone()) }
            }
            if **show_settings {
                { render_settings_modal(locale, config.clone(), form.clone(), show_settings.clone(), notice.clone(), reload) }
            }
        </>
    }
}

fn render_header(
    locale: Locale,
    can_edit: bool,
    show_s3: &UseStateHandle<bool>,
    show_settings: &UseStateHandle<bool>,
    show_take: &UseStateHandle<bool>,
) -> Html {
    let s3_btn = {
        let s = show_s3.clone();
        Callback::from(move |_| s.set(!*s))
    };
    let settings_btn = {
        let s = show_settings.clone();
        Callback::from(move |_| s.set(true))
    };
    let take_btn = {
        let s = show_take.clone();
        Callback::from(move |_| s.set(true))
    };

    html! {
        <div class="flex flex-col sm:flex-row sm:items-center justify-between gap-4">
            <div>
                <h1 class="text-2xl font-semibold tracking-tight">{locale.t("backups.title")}</h1>
                <p class="text-sm text-neutral-400 mt-1">{locale.t("backups.subtitle")}</p>
            </div>
            <div class="flex flex-wrap items-center gap-2">
                if can_edit {
                    <button type="button" class={BUTTON_SECONDARY} onclick={s3_btn}>
                        <svg class="w-4 h-4" fill="none" stroke="currentColor" viewBox="0 0 24 24">
                            <path stroke-linecap="round" stroke-linejoin="round" stroke-width="2" d="M3 15a4 4 0 004 4h9a5 5 0 10-.1-9.999 5.002 5.002 0 00-9.78 2.096A4.001 4.001 0 003 15z" />
                        </svg>
                        {locale.t("backups.disaster_recovery")}
                    </button>
                    <button type="button" class={BUTTON_SECONDARY} onclick={settings_btn}>
                        <svg class="w-4 h-4" fill="none" stroke="currentColor" viewBox="0 0 24 24">
                            <path stroke-linecap="round" stroke-linejoin="round" stroke-width="2" d="M10.325 4.317c.426-1.756 2.924-1.756 3.35 0a1.724 1.724 0 002.573 1.066c1.543-.94 3.31.826 2.37 2.37a1.724 1.724 0 001.065 2.572c1.756.426 1.756 2.924 0 3.35a1.724 1.724 0 00-1.066 2.573c.94 1.543-.826 3.31-2.37 2.37a1.724 1.724 0 00-2.572 1.065c-.426 1.756-2.924 1.756-3.35 0a1.724 1.724 0 00-2.573-1.066c-1.543.94-3.31-.826-2.37-2.37a1.724 1.724 0 00-1.065-2.572c-1.756-.426-1.756-2.924 0-3.35a1.724 1.724 0 001.066-2.573c-.94-1.543.826-3.31 2.37-2.37.996.608 2.296.07 2.572-1.065z" />
                            <path stroke-linecap="round" stroke-linejoin="round" stroke-width="2" d="M15 12a3 3 0 11-6 0 3 3 0 016 0z" />
                        </svg>
                        {locale.t("backups.settings")}
                    </button>
                    <button type="button" class={BUTTON_PRIMARY} onclick={take_btn}>
                        <svg class="w-4 h-4" fill="none" stroke="currentColor" viewBox="0 0 24 24">
                            <path stroke-linecap="round" stroke-linejoin="round" stroke-width="2" d="M12 4v16m8-8H4" />
                        </svg>
                        {locale.t("backups.action_backup_now")}
                    </button>
                }
            </div>
        </div>
    }
}

fn render_filter_bar(
    locale: Locale,
    search: &UseStateHandle<String>,
    engine: &UseStateHandle<String>,
) -> Html {
    let on_search = {
        let s = search.clone();
        Callback::from(move |e: InputEvent| {
            let val = e
                .target_unchecked_into::<web_sys::HtmlInputElement>()
                .value();
            s.set(val);
        })
    };
    let on_engine = {
        let eng = engine.clone();
        Callback::from(move |e: Event| {
            let val = e
                .target_unchecked_into::<web_sys::HtmlSelectElement>()
                .value();
            eng.set(val);
        })
    };

    html! {
        <div class="flex flex-col sm:flex-row items-stretch sm:items-center justify-between gap-3">
            <div class="flex items-center gap-2 flex-1 max-w-md">
                <input type="text" class={INPUT_CLASS}
                    placeholder={locale.t("backups.filter_search")}
                    value={(**search).clone()}
                    oninput={on_search}
                />
            </div>
            <div class="flex items-center gap-2">
                <select class="rounded bg-neutral-800 text-neutral-100 border border-neutral-700 px-3 py-1.5 text-sm focus:outline-none"
                    value={(**engine).clone()}
                    onchange={on_engine}
                >
                    <option value="all">{locale.t("backups.filter_all_engines")}</option>
                    <option value="postgres">{"PostgreSQL"}</option>
                    <option value="mysql">{"MySQL"}</option>
                    <option value="mariadb">{"MariaDB"}</option>
                    <option value="mongodb">{"MongoDB"}</option>
                    <option value="redis">{"Redis"}</option>
                </select>
            </div>
        </div>
    }
}

fn render_kpi_cards(overview: Option<&BackupsOverview>, locale: Locale) -> Html {
    let (total_count, total_bytes_str, last_backup_str, schedule_str, heartbeat_active, s3_active) =
        match overview {
            Some(o) => (
                o.total_backups.to_string(),
                format_bytes(o.total_bytes),
                o.last_backup_at
                    .map(|ts| format_time(ts.max(0) as u64))
                    .unwrap_or_else(|| locale.t("backups.never").to_string()),
                format_schedule_text(locale, &o.schedule),
                o.heartbeat_configured,
                o.s3_enabled,
            ),
            None => ("-".into(), "-".into(), "-".into(), "-".into(), false, false),
        };

    html! {
        <div class="grid grid-cols-1 sm:grid-cols-2 lg:grid-cols-4 gap-4">
            <div class="p-4 rounded-lg border border-neutral-800 bg-neutral-900/60">
                <div class="text-xs text-neutral-400 font-medium">{locale.t("backups.overview_total")}</div>
                <div class="mt-2 flex items-baseline gap-2">
                    <span class="text-2xl font-bold text-neutral-100">{total_count}</span>
                    <span class="text-xs text-neutral-400 font-mono">{"("}{total_bytes_str}{")"}</span>
                </div>
            </div>

            <div class="p-4 rounded-lg border border-neutral-800 bg-neutral-900/60">
                <div class="text-xs text-neutral-400 font-medium">{locale.t("backups.overview_last")}</div>
                <div class="mt-2 text-sm font-semibold text-neutral-100 truncate">{last_backup_str}</div>
            </div>

            <div class="p-4 rounded-lg border border-neutral-800 bg-neutral-900/60">
                <div class="text-xs text-neutral-400 font-medium">{locale.t("backups.overview_schedule")}</div>
                <div class="mt-2 flex items-center gap-1.5 flex-wrap">
                    <span class="text-sm font-semibold text-neutral-100">{schedule_str}</span>
                    if heartbeat_active {
                        <span class="inline-flex items-center px-1.5 py-0.5 rounded text-[10px] font-medium bg-emerald-950 text-emerald-400 border border-emerald-800">
                            {locale.t("backups.heartbeat_active")}
                        </span>
                    }
                </div>
            </div>

            <div class="p-4 rounded-lg border border-neutral-800 bg-neutral-900/60">
                <div class="text-xs text-neutral-400 font-medium">{locale.t("backups.overview_remote")}</div>
                <div class="mt-2">
                    if s3_active {
                        <span class="inline-flex items-center px-2 py-0.5 rounded text-xs font-medium bg-blue-950 text-blue-400 border border-blue-800">
                            {locale.t("backups.s3_active")}
                        </span>
                    } else {
                        <span class="inline-flex items-center px-2 py-0.5 rounded text-xs font-medium bg-neutral-800 text-neutral-400 border border-neutral-700">
                            {locale.t("backups.local_only")}
                        </span>
                    }
                </div>
            </div>
        </div>
    }
}

fn render_backups_table(
    locale: Locale,
    backups: &[BackupEntry],
    can_edit: bool,
    reload: Callback<()>,
    on_select_drill: Callback<BackupEntry>,
) -> Html {
    if backups.is_empty() {
        return html! {
            <div class="p-8 text-center text-neutral-400 text-sm">
                {locale.t("backups.empty")}
            </div>
        };
    }

    html! {
        <div class="overflow-x-auto">
            <table class="w-full text-left text-sm">
                <thead class="border-b border-neutral-800 bg-neutral-900/90 text-xs text-neutral-400 font-medium">
                    <tr>
                        <th class="px-4 py-3">{locale.t("backups.database")}</th>
                        <th class="px-4 py-3">{"ID"}</th>
                        <th class="px-4 py-3">{locale.t("backups.size")}</th>
                        <th class="px-4 py-3">{locale.t("backups.time")}</th>
                        <th class="px-4 py-3">{locale.t("backups.verified")}</th>
                        <th class="px-4 py-3 text-right">{locale.t("common.actions")}</th>
                    </tr>
                </thead>
                <tbody class="divide-y divide-neutral-800/60">
                    { for backups.iter().map(|b| render_backup_row(locale, b, can_edit, reload.clone(), on_select_drill.clone())) }
                </tbody>
            </table>
        </div>
    }
}

fn render_backup_row(
    locale: Locale,
    entry: &BackupEntry,
    can_edit: bool,
    reload: Callback<()>,
    on_select_drill: Callback<BackupEntry>,
) -> Html {
    let download_url = format!(
        "{API_ENDPOINT}/databases/{}/backups/{}/download",
        entry.app, entry.id
    );
    let filename = format!("{}.dump.gz", entry.id);

    let on_drill_click = {
        let on_select_drill = on_select_drill.clone();
        let entry = entry.clone();
        Callback::from(move |e: MouseEvent| {
            e.prevent_default();
            on_select_drill.emit(entry.clone());
        })
    };

    let verify_badge = match (entry.verified_at, entry.verification_status.as_deref()) {
        (Some(_), Some("valid")) => {
            let label = if let Some(drill) = &entry.drill
                && drill.table_count > 0
            {
                locale.tf(
                    "backups.drill_passed_tables",
                    &[("count", &drill.table_count.to_string())],
                )
            } else {
                locale.t("backups.verified").to_string()
            };
            html! {
                <button type="button" class="inline-flex items-center gap-1 px-2 py-0.5 rounded text-xs font-medium bg-emerald-950 text-emerald-400 border border-emerald-800 hover:bg-emerald-900/60 cursor-pointer transition" onclick={on_drill_click} title={locale.t("backups.view_drill")}>
                    <svg class="w-3 h-3" fill="none" stroke="currentColor" viewBox="0 0 24 24">
                        <path stroke-linecap="round" stroke-linejoin="round" stroke-width="2" d="M9 12l2 2 4-4m5.618-4.016A11.955 11.955 0 0112 2.944a11.955 11.955 0 01-8.618 3.04A12.02 12.02 0 003 9c0 5.591 3.824 10.29 9 11.622 5.176-1.332 9-6.03 9-11.622 0-1.042-.133-2.052-.382-3.016z" />
                    </svg>
                    {label}
                </button>
            }
        }
        (Some(_), _) => html! {
            <button type="button" class="inline-flex items-center gap-1 px-2 py-0.5 rounded text-xs font-medium bg-red-950 text-red-400 border border-red-800 hover:bg-red-900/60 cursor-pointer transition" onclick={on_drill_click} title={locale.t("backups.view_drill")}>
                <svg class="w-3 h-3" fill="none" stroke="currentColor" viewBox="0 0 24 24">
                    <path stroke-linecap="round" stroke-linejoin="round" stroke-width="2" d="M12 9v2m0 4h.01m-6.938 4h13.856c1.54 0 2.502-1.667 1.732-3L13.732 4c-.77-1.333-2.694-1.333-3.464 0L3.34 16c-.77 1.333.192 3 1.732 3z" />
                </svg>
                {locale.t("backups.verify_failed")}
            </button>
        },
        _ => html! {
            <button type="button" class="inline-flex items-center gap-1 px-2 py-0.5 rounded text-xs font-medium bg-neutral-800 text-neutral-400 border border-neutral-700 hover:bg-neutral-700/60 cursor-pointer transition" onclick={on_drill_click} title={locale.t("backups.view_drill")}>
                {locale.t("backups.untested")}
            </button>
        },
    };

    let restore_btn = can_edit.then(|| {
        let (id, backup_id) = (entry.app, entry.id.clone());
        let reload = reload.clone();
        let onclick = Callback::from(move |e: MouseEvent| {
            e.prevent_default();
            let question = locale.tf("backups.restore_question", &[("name", &backup_id)]);
            let path = format!("{API_ENDPOINT}/databases/{id}/backups/{backup_id}/restore");
            let reload = reload.clone();
            dialog::confirm_then(locale, question, async move {
                if let Ok(req) = Request::post(&path).build() {
                    let _ = send_request(locale, req).await;
                    reload.emit(());
                }
            });
        });
        html! {
            <button type="button" class="text-xs text-blue-400 hover:text-blue-300 font-medium" {onclick}>
                {locale.t("backups.restore")}
            </button>
        }
    });

    let verify_btn = can_edit.then(|| {
        let (id, backup_id) = (entry.app, entry.id.clone());
        let reload = reload.clone();
        let onclick = Callback::from(move |e: MouseEvent| {
            e.prevent_default();
            let path = format!("{API_ENDPOINT}/databases/{id}/backups/{backup_id}/test");
            let reload = reload.clone();
            spawn_local(async move {
                let Ok(req) = Request::post(&path).build() else {
                    return;
                };
                if send_request(locale, req).await.is_ok() {
                    dialog::message(locale, locale.t("backups.tested"), Icon::Success).await;
                    reload.emit(());
                }
            });
        });
        html! {
            <button type="button" class="text-xs text-neutral-400 hover:text-neutral-200 font-medium" {onclick}>
                {locale.t("backups.action_verify")}
            </button>
        }
    });

    let delete_btn = can_edit.then(|| {
        let question = locale.tf("backups.delete_question", &[("name", &entry.id)]);
        let path = format!("/databases/{}/backups/{}", entry.app, entry.id);
        let onclick = delete_on_click(locale, question, path, reload.clone());
        html! {
            <button type="button" class="text-xs text-red-400 hover:text-red-300 font-medium" {onclick}>
                {locale.t("common.delete")}
            </button>
        }
    });

    html! {
        <tr class="hover:bg-neutral-800/30 transition-colors">
            <td class="px-4 py-3">
                <div class="flex items-center gap-2">
                    <span class="font-medium text-neutral-200">{&entry.name}</span>
                    <span class="px-1.5 py-0.5 rounded text-[10px] font-mono uppercase bg-neutral-800 text-neutral-300 border border-neutral-700">
                        {&entry.engine}
                    </span>
                </div>
            </td>
            <td class="px-4 py-3 font-mono text-xs text-neutral-400">{&entry.id}</td>
            <td class="px-4 py-3 font-mono text-xs text-neutral-300">{format_bytes(entry.size)}</td>
            <td class="px-4 py-3 text-xs text-neutral-400">{format_time(entry.created_at.max(0) as u64)}</td>
            <td class="px-4 py-3">{verify_badge}</td>
            <td class="px-4 py-3 text-right">
                <div class="flex items-center justify-end gap-3">
                    <a class="text-xs text-neutral-300 hover:text-white font-medium" href={download_url} download={filename}>
                        {locale.t("backups.download")}
                    </a>
                    {verify_btn}
                    {restore_btn}
                    {delete_btn}
                </div>
            </td>
        </tr>
    }
}

fn render_s3_drawer(
    locale: Locale,
    snapshots: &[S3BackupSnapshot],
    can_edit: bool,
    reload: Callback<()>,
) -> Html {
    let content = if snapshots.is_empty() {
        html! {
            <div class="p-6 text-center text-sm text-neutral-400">
                {locale.t("databases.s3_empty")}
            </div>
        }
    } else {
        html! {
            <div class="overflow-x-auto">
                <table class="w-full text-left text-xs">
                    <thead class="border-b border-neutral-800 text-neutral-400">
                        <tr>
                            <th class="px-4 py-2 font-medium">{locale.t("backups.database")}</th>
                            <th class="px-4 py-2 font-medium">{"Snapshot Key"}</th>
                            <th class="px-4 py-2 font-medium">{locale.t("backups.size")}</th>
                            <th class="px-4 py-2 font-medium">{locale.t("backups.time")}</th>
                            <th class="px-4 py-2 text-right font-medium">{locale.t("common.actions")}</th>
                        </tr>
                    </thead>
                    <tbody class="divide-y divide-neutral-800/50">
                        { for snapshots.iter().map(|s| render_s3_row(locale, s, can_edit, reload.clone())) }
                    </tbody>
                </table>
            </div>
        }
    };

    html! {
        <div class="p-4 rounded-lg border border-neutral-700 bg-neutral-900/90 space-y-3">
            <div class="flex items-center justify-between">
                <h3 class="text-sm font-semibold text-neutral-200 flex items-center gap-2">
                    <svg class="w-4 h-4 text-blue-400" fill="none" stroke="currentColor" viewBox="0 0 24 24">
                        <path stroke-linecap="round" stroke-linejoin="round" stroke-width="2" d="M3 15a4 4 0 004 4h9a5 5 0 10-.1-9.999 5.002 5.002 0 00-9.78 2.096A4.001 4.001 0 003 15z" />
                    </svg>
                    {locale.t("databases.s3_dr_title")}
                </h3>
            </div>
            {content}
        </div>
    }
}

fn render_s3_row(
    locale: Locale,
    snap: &S3BackupSnapshot,
    can_edit: bool,
    reload: Callback<()>,
) -> Html {
    let download_url = format!("{API_ENDPOINT}/databases/s3/download?key={}", snap.key);
    let key = snap.key.clone();
    let restore_btn = can_edit.then(|| {
        let reload = reload.clone();
        let onclick = Callback::from(move |e: MouseEvent| {
            e.prevent_default();
            let key = key.clone();
            let question = locale.tf("databases.s3_restore_question", &[("key", &key)]);
            let reload = reload.clone();
            dialog::confirm_then(locale, question, async move {
                let req_body = serde_json::json!({
                    "key": key,
                });
                if let Ok(req) = Request::post(&format!("{API_ENDPOINT}/databases/s3/restore"))
                    .header("Content-Type", "application/json")
                    .body(req_body.to_string())
                {
                    let _ = send_request(locale, req).await;
                    reload.emit(());
                }
            });
        });
        html! {
            <button type="button" class="text-blue-400 hover:text-blue-300 font-medium" {onclick}>
                {locale.t("databases.s3_restore")}
            </button>
        }
    });

    html! {
        <tr class="hover:bg-neutral-800/40">
            <td class="px-4 py-2 font-medium text-neutral-200">{&snap.database_name}</td>
            <td class="px-4 py-2 font-mono text-neutral-400">{&snap.key}</td>
            <td class="px-4 py-2 font-mono text-neutral-300">{format_bytes(snap.size)}</td>
            <td class="px-4 py-2 text-neutral-400">{&snap.last_modified}</td>
            <td class="px-4 py-2 text-right">
                <div class="flex items-center justify-end gap-3">
                    <a class="text-neutral-300 hover:text-white font-medium" href={download_url} download={format!("{}.dump.gz", snap.backup_id)}>
                        {locale.t("backups.download")}
                    </a>
                    {restore_btn}
                </div>
            </td>
        </tr>
    }
}

fn render_take_modal(
    locale: Locale,
    databases: UseStateHandle<Vec<AppEntry>>,
    selected_db: UseStateHandle<Option<ShortId>>,
    show_take: UseStateHandle<bool>,
    notice: UseStateHandle<Option<Notice<&'static str>>>,
    reload: Callback<()>,
) -> Html {
    let dbs = (*databases).clone();
    let current_selected = (*selected_db).or_else(|| dbs.first().map(|d| d.id));

    let onchange = {
        let selected_db = selected_db.clone();
        Callback::from(move |e: Event| {
            let val = e
                .target_unchecked_into::<web_sys::HtmlSelectElement>()
                .value();
            if let Ok(id) = val.parse::<ShortId>() {
                selected_db.set(Some(id));
            }
        })
    };

    let submit = {
        let (show_take, notice, reload) = (show_take.clone(), notice.clone(), reload.clone());
        let db_id = current_selected;
        Callback::from(move |e: SubmitEvent| {
            e.prevent_default();
            let Some(id) = db_id else {
                return;
            };
            let (show_take, notice, reload) = (show_take.clone(), notice.clone(), reload.clone());
            spawn_local(async move {
                let req = RunBackupRequest { database_id: id };
                let body = serde_json::to_string(&req).unwrap_or_default();
                let sent = Request::post(&format!("{API_ENDPOINT}/backups/run"))
                    .header("Content-Type", "application/json")
                    .body(body);
                match sent {
                    Ok(req) => match send_request(locale, req).await {
                        Ok(_) => {
                            notice.set(Some(Notice::Saved("backups.taken")));
                            show_take.set(false);
                            reload.emit(());
                        }
                        Err(err) => notice.set(Some(Notice::Failed(err.to_string()))),
                    },
                    Err(err) => notice.set(Some(Notice::Failed(err.to_string()))),
                }
            });
        })
    };

    html! {
        <div class="fixed inset-0 z-50 flex items-center justify-center bg-black/60 backdrop-blur-sm p-4">
            <div class="w-full max-w-md rounded-lg border border-neutral-700 bg-neutral-900 p-6 space-y-4 shadow-xl">
                <div class="flex items-center justify-between">
                    <h3 class="text-base font-semibold text-neutral-100">{locale.t("backups.action_backup_now")}</h3>
                    <button type="button" class="text-neutral-400 hover:text-neutral-200"
                        onclick={let s = show_take.clone(); Callback::from(move |_| s.set(false))}
                    >
                        {"✕"}
                    </button>
                </div>

                <form onsubmit={submit} class="space-y-4">
                    <label class="block space-y-1">
                        <span class="text-xs font-medium text-neutral-300">{locale.t("backups.select_database")}</span>
                        <select class={INPUT_CLASS} {onchange}>
                            { for dbs.iter().map(|d| {
                                let id = d.id;
                                let is_sel = Some(id) == current_selected;
                                html! {
                                    <option value={id.to_string()} selected={is_sel}>
                                        {format!("{} ({})", d.name, d.spec.database.as_deref().unwrap_or("db"))}
                                    </option>
                                }
                            }) }
                        </select>
                    </label>

                    <div class="flex justify-end gap-2 pt-2">
                        <button type="button" class={BUTTON_SECONDARY}
                            onclick={let s = show_take.clone(); Callback::from(move |_| s.set(false))}
                        >
                            {locale.t("common.cancel")}
                        </button>
                        <button type="submit" class={BUTTON_PRIMARY}>
                            {locale.t("backups.take")}
                        </button>
                    </div>
                </form>
            </div>
        </div>
    }
}

fn render_settings_modal(
    locale: Locale,
    config: UseStateHandle<BackupConfig>,
    form: UseStateHandle<FormState>,
    show_settings: UseStateHandle<bool>,
    notice: UseStateHandle<Option<Notice<&'static str>>>,
    reload: Callback<()>,
) -> Html {
    let submit = {
        let (form, config, show_settings, notice, reload) = (
            form.clone(),
            config.clone(),
            show_settings.clone(),
            notice.clone(),
            reload.clone(),
        );
        Callback::from(move |e: SubmitEvent| {
            e.prevent_default();
            let new_cfg = form.to_config(&config);
            let (show_settings, notice, reload) =
                (show_settings.clone(), notice.clone(), reload.clone());
            spawn_local(async move {
                let sent = Request::put(&format!("{API_ENDPOINT}/backup-config"))
                    .header("Content-Type", "application/json");
                match send_json(locale, sent, &new_cfg).await {
                    Ok(_) => {
                        notice.set(Some(Notice::Saved("backups.saved")));
                        show_settings.set(false);
                        reload.emit(());
                    }
                    Err(err) => notice.set(Some(Notice::Failed(err.to_string()))),
                }
            });
        })
    };

    html! {
        <div class="fixed inset-0 z-50 flex items-center justify-center bg-black/60 backdrop-blur-sm p-4 overflow-y-auto">
            <div class="w-full max-w-xl rounded-lg border border-neutral-700 bg-neutral-900 p-6 space-y-4 shadow-xl my-8">
                <div class="flex items-center justify-between border-b border-neutral-800 pb-3">
                    <h3 class="text-base font-semibold text-neutral-100">{locale.t("backups.settings")}</h3>
                    <button type="button" class="text-neutral-400 hover:text-neutral-200"
                        onclick={let s = show_settings.clone(); Callback::from(move |_| s.set(false))}
                    >
                        {"✕"}
                    </button>
                </div>

                <form onsubmit={submit} class="space-y-4 text-sm">
                    <div class="grid grid-cols-1 sm:grid-cols-2 gap-3">
                        <label class="block space-y-1">
                            <span class="text-xs font-medium text-neutral-300">{locale.t("backups.schedule")}</span>
                            <input type="text" class={INPUT_CLASS} placeholder="03:00"
                                value={form.schedule.clone()}
                                oninput={
                                    let form = form.clone();
                                    Callback::from(move |e: InputEvent| {
                                        let val = e.target_unchecked_into::<web_sys::HtmlInputElement>().value();
                                        let mut u = (*form).clone();
                                        u.schedule = val;
                                        form.set(u);
                                    })
                                }
                            />
                            <span class="text-[11px] text-neutral-500">{locale.t("backups.schedule_hint")}</span>
                        </label>

                        <label class="block space-y-1">
                            <span class="text-xs font-medium text-neutral-300">{locale.t("backups.keep")}</span>
                            <input type="number" min="1" max="999" class={INPUT_CLASS}
                                value={form.keep.clone()}
                                oninput={
                                    let form = form.clone();
                                    Callback::from(move |e: InputEvent| {
                                        let val = e.target_unchecked_into::<web_sys::HtmlInputElement>().value();
                                        let mut u = (*form).clone();
                                        u.keep = val;
                                        form.set(u);
                                    })
                                }
                            />
                        </label>
                    </div>

                    <label class="flex items-center gap-2 pt-1">
                        <input type="checkbox" checked={form.restore_test_enabled}
                            onchange={
                                let form = form.clone();
                                Callback::from(move |e: Event| {
                                    let checked = e.target_unchecked_into::<web_sys::HtmlInputElement>().checked();
                                    let mut u = (*form).clone();
                                    u.restore_test_enabled = checked;
                                    form.set(u);
                                })
                            }
                        />
                        <span class="text-xs text-neutral-300">{locale.t("backups.restore_test_enabled")}</span>
                    </label>

                    <label class="block space-y-1 pt-2 border-t border-neutral-800">
                        <span class="text-xs font-medium text-neutral-300">{locale.t("backups.heartbeat_url")}</span>
                        <input type="text" class={INPUT_CLASS} placeholder="https://hc-ping.com/..."
                            value={form.heartbeat_url.clone()}
                            oninput={
                                let form = form.clone();
                                Callback::from(move |e: InputEvent| {
                                    let val = e.target_unchecked_into::<web_sys::HtmlInputElement>().value();
                                    let mut u = (*form).clone();
                                    u.heartbeat_url = val;
                                    form.set(u);
                                })
                            }
                        />
                        <span class="text-[11px] text-neutral-500">{locale.t("backups.heartbeat_url_hint")}</span>
                    </label>

                    <div class="space-y-3 pt-3 border-t border-neutral-800">
                        <h4 class="text-xs font-semibold uppercase text-neutral-400 tracking-wider">{locale.t("backups.s3")}</h4>
                        <div class="grid grid-cols-1 sm:grid-cols-2 gap-3">
                            <label class="block space-y-1">
                                <span class="text-xs text-neutral-400">{locale.t("backups.endpoint")}</span>
                                <input type="text" class={INPUT_CLASS} placeholder="s3.eu-central-1.amazonaws.com"
                                    value={form.endpoint.clone()}
                                    oninput={
                                        let form = form.clone();
                                        Callback::from(move |e: InputEvent| {
                                            let val = e.target_unchecked_into::<web_sys::HtmlInputElement>().value();
                                            let mut u = (*form).clone();
                                            u.endpoint = val;
                                            form.set(u);
                                        })
                                    }
                                />
                            </label>

                            <label class="block space-y-1">
                                <span class="text-xs text-neutral-400">{locale.t("backups.region")}</span>
                                <input type="text" class={INPUT_CLASS} placeholder="auto"
                                    value={form.region.clone()}
                                    oninput={
                                        let form = form.clone();
                                        Callback::from(move |e: InputEvent| {
                                            let val = e.target_unchecked_into::<web_sys::HtmlInputElement>().value();
                                            let mut u = (*form).clone();
                                            u.region = val;
                                            form.set(u);
                                        })
                                    }
                                />
                            </label>

                            <label class="block space-y-1">
                                <span class="text-xs text-neutral-400">{locale.t("backups.prefix")}</span>
                                <input type="text" class={INPUT_CLASS} placeholder="my-bucket/backups"
                                    value={form.prefix.clone()}
                                    oninput={
                                        let form = form.clone();
                                        Callback::from(move |e: InputEvent| {
                                            let val = e.target_unchecked_into::<web_sys::HtmlInputElement>().value();
                                            let mut u = (*form).clone();
                                            u.prefix = val;
                                            form.set(u);
                                        })
                                    }
                                />
                            </label>

                            <label class="block space-y-1">
                                <span class="text-xs text-neutral-400">{locale.t("backups.access_key")}</span>
                                <input type="text" class={INPUT_CLASS}
                                    value={form.access_key.clone()}
                                    oninput={
                                        let form = form.clone();
                                        Callback::from(move |e: InputEvent| {
                                            let val = e.target_unchecked_into::<web_sys::HtmlInputElement>().value();
                                            let mut u = (*form).clone();
                                            u.access_key = val;
                                            form.set(u);
                                        })
                                    }
                                />
                            </label>

                            <label class="block space-y-1 sm:col-span-2">
                                <span class="text-xs text-neutral-400">{locale.t("backups.secret_key")}</span>
                                <input type="password" class={INPUT_CLASS}
                                    placeholder={locale.t("backups.secret_key_hint")}
                                    value={form.secret_key.clone()}
                                    oninput={
                                        let form = form.clone();
                                        Callback::from(move |e: InputEvent| {
                                            let val = e.target_unchecked_into::<web_sys::HtmlInputElement>().value();
                                            let mut u = (*form).clone();
                                            u.secret_key = val;
                                            form.set(u);
                                        })
                                    }
                                />
                            </label>
                        </div>
                    </div>

                    <div class="flex justify-end gap-2 pt-3 border-t border-neutral-800">
                        <button type="button" class={BUTTON_SECONDARY}
                            onclick={let s = show_settings.clone(); Callback::from(move |_| s.set(false))}
                        >
                            {locale.t("common.cancel")}
                        </button>
                        <button type="submit" class={BUTTON_PRIMARY}>
                            {locale.t("common.update")}
                        </button>
                    </div>
                </form>
            </div>
        </div>
    }
}

#[allow(clippy::too_many_arguments)]
fn load_data(
    locale: Locale,
    overview: UseStateHandle<Option<BackupsOverview>>,
    backups: UseStateHandle<Vec<BackupEntry>>,
    databases: UseStateHandle<Vec<AppEntry>>,
    s3_snapshots: UseStateHandle<Vec<S3BackupSnapshot>>,
    config: UseStateHandle<BackupConfig>,
    form: UseStateHandle<FormState>,
    notice: UseStateHandle<Option<Notice<&'static str>>>,
) {
    let ov = overview.clone();
    spawn_local(async move {
        if let Ok(loaded) = fetch_json::<BackupsOverview>(
            locale,
            Request::get(&format!("{API_ENDPOINT}/backups/overview")),
        )
        .await
        {
            ov.set(Some(loaded));
        }
    });

    let bk = backups.clone();
    let not = notice.clone();
    spawn_local(async move {
        match fetch_json::<Vec<BackupEntry>>(
            locale,
            Request::get(&format!("{API_ENDPOINT}/backups")),
        )
        .await
        {
            Ok(loaded) => bk.set(loaded),
            Err(err) => not.set(Some(Notice::Failed(err))),
        }
    });

    let db = databases.clone();
    spawn_local(async move {
        if let Ok(loaded) =
            fetch_json::<Vec<AppEntry>>(locale, Request::get(&format!("{API_ENDPOINT}/databases")))
                .await
        {
            db.set(loaded);
        }
    });

    let s3 = s3_snapshots.clone();
    spawn_local(async move {
        if let Ok(loaded) = fetch_json::<Vec<S3BackupSnapshot>>(
            locale,
            Request::get(&format!("{API_ENDPOINT}/databases/s3/backups")),
        )
        .await
        {
            s3.set(loaded);
        }
    });

    let cfg = config.clone();
    let frm = form.clone();
    spawn_local(async move {
        if let Ok(loaded) = fetch_json::<BackupConfig>(
            locale,
            Request::get(&format!("{API_ENDPOINT}/backup-config")),
        )
        .await
        {
            frm.set(FormState::from_config(&loaded));
            cfg.set(loaded);
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn filter_backups_matches_search_and_engine() {
        let backups = vec![
            BackupEntry {
                id: "b1".into(),
                app: ShortId::new(),
                name: "shop-db".into(),
                engine: "postgres".into(),
                size: 1024,
                sha256: "abc".into(),
                created_at: 1000,
                verified_at: Some(1005),
                verification_status: Some("valid".into()),
                drill: None,
            },
            BackupEntry {
                id: "b2".into(),
                app: ShortId::new(),
                name: "cache".into(),
                engine: "redis".into(),
                size: 512,
                sha256: "def".into(),
                created_at: 2000,
                verified_at: None,
                verification_status: None,
                drill: None,
            },
        ];

        // Empty filter returns all
        assert_eq!(filter_backups(&backups, "", "all").len(), 2);

        // Search by database name
        let by_name = filter_backups(&backups, "shop", "all");
        assert_eq!(by_name.len(), 1);
        assert_eq!(by_name[0].name, "shop-db");

        // Filter by engine
        let by_engine = filter_backups(&backups, "", "redis");
        assert_eq!(by_engine.len(), 1);
        assert_eq!(by_engine[0].engine, "redis");

        // Non-matching query returns empty
        assert_eq!(filter_backups(&backups, "unknown", "all").len(), 0);
    }

    #[test]
    fn schedule_text_formatting() {
        assert_eq!(format_schedule_text(Locale::En, ""), "Disabled");
        assert_eq!(
            format_schedule_text(Locale::En, "03:00"),
            "Daily at 03:00 UTC"
        );
    }
}
