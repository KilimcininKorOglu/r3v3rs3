//! Centralized database backups, automated schedules, and disaster recovery center.

use super::app_files::format_bytes;
use super::resource_page::{FormCard, Notice, delete_on_click, form_modal, notice_view};
use super::settings::{failure_box, fetch_json, format_network_error, send_json, send_request};
use crate::API_ENDPOINT;
use crate::auth::use_ensure_auth;
use crate::components::action_button::{ActionIconButton, ActionVariant};
use crate::components::backup_drill_modal::BackupDrillModal;
use crate::components::button::{Button, ButtonKind, ButtonSize, button_class};
use crate::components::data_list::{Column, ListContent, ListOptions, Row, list_view};
use crate::components::modal::{Modal, ModalSize};
use crate::components::tabs::{TabItem, Tabs};
use crate::dialog::{self, Icon};
use crate::format::{format_rfc3339, format_time, format_unix_time};
use crate::i18n::use_locale;
use crate::store::SessionStore;
use gloo_net::http::Request;
use r3v3rs3_api::i18n::Locale;
use r3v3rs3_api::id::ShortId;
use r3v3rs3_api::platform::{
    AppEntry, BackupConfig, BackupEntry, BackupsOverview, CreateDisasterRecoveryRequest,
    DisasterRecoveryBackupEntry, RestoreDisasterRecoveryRequest, RunBackupRequest,
    S3BackupSnapshot, S3Config,
};
use wasm_bindgen_futures::spawn_local;
use web_sys::HtmlInputElement;
use yew::prelude::*;
use yewdux::prelude::*;

const INPUT_CLASS: &str = "w-full rounded-lg border border-neutral-300 bg-neutral-50 text-neutral-900 dark:border-neutral-600 dark:bg-neutral-700 dark:text-white px-3 py-1.5 text-sm focus:ring-primary-500 focus:border-primary-500";
const CARD: &str = "rounded-md border border-neutral-300 bg-white shadow-sm dark:border-neutral-700 dark:bg-neutral-800";
const TILE: &str = "p-4 rounded-md border border-neutral-300 bg-white shadow-sm dark:border-neutral-700 dark:bg-neutral-800";
const TILE_LABEL: &str = "text-xs text-neutral-500 dark:text-neutral-400 font-medium";
const FIELD_LABEL: &str = "text-xs font-medium text-neutral-700 dark:text-neutral-300";
const HINT: &str = "text-[11px] text-neutral-500 dark:text-neutral-400";
const BADGE: &str = "inline-flex items-center gap-1 px-2 py-0.5 rounded text-xs font-medium border";
const BADGE_OK: &str = "bg-emerald-50 text-emerald-700 border-emerald-300 dark:bg-emerald-950 dark:text-emerald-400 dark:border-emerald-800";
const BADGE_FAIL: &str =
    "bg-red-50 text-red-700 border-red-300 dark:bg-red-950 dark:text-red-400 dark:border-red-800";
const BADGE_INFO: &str = "bg-blue-50 text-blue-700 border-blue-300 dark:bg-blue-950 dark:text-blue-400 dark:border-blue-800";
const BADGE_MUTED: &str = "bg-neutral-100 text-neutral-600 border-neutral-300 dark:bg-neutral-800 dark:text-neutral-400 dark:border-neutral-700";

const BACKUP_COLUMNS: [Column; 5] = [
    Column {
        label: "backups.database",
        class: "",
    },
    Column {
        label: "backups.backup_id",
        class: "font-mono text-xs",
    },
    Column {
        label: "backups.size",
        class: "font-mono text-xs whitespace-nowrap",
    },
    Column {
        label: "backups.time",
        class: "text-xs whitespace-nowrap",
    },
    Column {
        label: "backups.verified",
        class: "",
    },
];

const S3_COLUMNS: [Column; 4] = [
    Column {
        label: "backups.database",
        class: "",
    },
    Column {
        label: "backups.snapshot_key",
        class: "font-mono break-all",
    },
    Column {
        label: "backups.size",
        class: "font-mono whitespace-nowrap",
    },
    Column {
        label: "backups.time",
        class: "whitespace-nowrap",
    },
];

const DR_COLUMNS: [Column; 5] = [
    Column {
        label: "backups.backup_id",
        class: "font-mono text-xs",
    },
    Column {
        label: "backups.time",
        class: "text-xs whitespace-nowrap",
    },
    Column {
        label: "backups.size",
        class: "font-mono text-xs whitespace-nowrap",
    },
    Column {
        label: "settings.dr_files_count",
        class: "text-xs",
    },
    Column {
        label: "backups.status",
        class: "",
    },
];

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

/// The selected backup section tab.
#[derive(Clone, Copy, PartialEq, Eq, Default)]
pub enum BackupTab {
    #[default]
    Databases,
    DisasterRecovery,
    Configuration,
}

#[function_component(Backups)]
pub fn backups() -> Html {
    use_ensure_auth();
    let locale = use_locale();
    let (session, _) = use_store::<SessionStore>();
    let active_tab = use_state(BackupTab::default);
    let overview = use_state(Option::<BackupsOverview>::default);
    let backups = use_state(Vec::<BackupEntry>::new);
    let databases = use_state(Vec::<AppEntry>::new);
    let s3_snapshots = use_state(|| Ok::<_, String>(Vec::<S3BackupSnapshot>::new()));
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
                    let result = post_empty(locale, &path).await;
                    drill_running.set(false);
                    match result {
                        Ok(()) => {
                            reload.emit(());
                            s.set(None);
                        }
                        Err(err) => {
                            dialog::message(locale, &err, Icon::Error).await;
                        }
                    }
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
            { render_header(locale, can_edit, *active_tab, &show_s3, &show_settings, &show_take) }

            { render_tabs(locale, &active_tab) }

            { match *active_tab {
                BackupTab::Databases => html! {
                    <>
                        { notice_view(&notice, |key| html! { {locale.t(key)} }) }

                        { render_kpi_cards(overview.as_ref(), locale) }

                        if *show_s3 {
                            { render_s3_drawer(locale, &s3_snapshots, can_edit, reload_cb.clone()) }
                        }

                        { render_filter_bar(locale, &search, &engine) }

                        { render_backups_table(locale, &filtered, can_edit, reload_cb.clone(), on_select_drill) }

                        { drill_modal }

                        { render_modals(locale, &show_take, &show_settings, &databases, &selected_db, &config, &form, &notice, reload_cb) }
                    </>
                },
                BackupTab::DisasterRecovery => html! {
                    <DisasterRecoveryTab can_edit={can_edit} />
                },
                BackupTab::Configuration => html! {
                    <ConfigurationBackupTab can_edit={can_edit} />
                },
            } }
        </div>
    }
}

/// Sends a POST request without a body. The error is the message of the failure in the
/// selected language.
async fn post_empty(locale: Locale, path: &str) -> Result<(), String> {
    let request = Request::post(path)
        .build()
        .map_err(|err| format_network_error(locale, &err))?;
    send_request(locale, request).await.map(|_| ())
}

fn render_tabs(locale: Locale, active_tab: &UseStateHandle<BackupTab>) -> Html {
    let tabs = [
        (BackupTab::Databases, "backups.tab_databases"),
        (BackupTab::DisasterRecovery, "backups.tab_disaster_recovery"),
        (BackupTab::Configuration, "backups.tab_configuration"),
    ];
    let tabs = tabs
        .into_iter()
        .map(|(tab, label)| {
            let onclick = {
                let active_tab = active_tab.clone();
                Callback::from(move |_: MouseEvent| active_tab.set(tab))
            };
            TabItem::click(locale.t(label), onclick, **active_tab == tab)
        })
        .collect::<Vec<_>>();
    html! { <Tabs {tabs} /> }
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
    active_tab: BackupTab,
    show_s3: &UseStateHandle<bool>,
    show_settings: &UseStateHandle<bool>,
    show_take: &UseStateHandle<bool>,
) -> Html {
    let s3_btn = {
        let s = show_s3.clone();
        Callback::from(move |_: MouseEvent| s.set(!*s))
    };
    let settings_btn = {
        let s = show_settings.clone();
        Callback::from(move |_: MouseEvent| s.set(true))
    };
    let take_btn = {
        let s = show_take.clone();
        Callback::from(move |_: MouseEvent| s.set(true))
    };

    html! {
        <div class="flex flex-col sm:flex-row sm:items-center justify-between gap-4">
            <div>
                <h1 class="text-2xl font-semibold tracking-tight text-neutral-900 dark:text-neutral-100">{locale.t("backups.title")}</h1>
                <p class="text-sm text-neutral-500 dark:text-neutral-400 mt-1">{locale.t("backups.subtitle")}</p>
            </div>
            if can_edit && active_tab == BackupTab::Databases {
                <div class="flex flex-wrap items-center gap-2">
                    <Button kind={ButtonKind::Secondary} onclick={s3_btn}>
                        <svg class="w-4 h-4" fill="none" stroke="currentColor" viewBox="0 0 24 24">
                            <path stroke-linecap="round" stroke-linejoin="round" stroke-width="2" d="M3 15a4 4 0 004 4h9a5 5 0 10-.1-9.999 5.002 5.002 0 00-9.78 2.096A4.001 4.001 0 003 15z" />
                        </svg>
                        {locale.t("backups.s3_snapshots")}
                    </Button>
                    <Button kind={ButtonKind::Secondary} onclick={settings_btn}>
                        <svg class="w-4 h-4" fill="none" stroke="currentColor" viewBox="0 0 24 24">
                            <path stroke-linecap="round" stroke-linejoin="round" stroke-width="2" d="M10.325 4.317c.426-1.756 2.924-1.756 3.35 0a1.724 1.724 0 002.573 1.066c1.543-.94 3.31.826 2.37 2.37a1.724 1.724 0 001.065 2.572c1.756.426 1.756 2.924 0 3.35a1.724 1.724 0 00-1.066 2.573c.94 1.543-.826 3.31-2.37 2.37a1.724 1.724 0 00-2.572 1.065c-.426 1.756-2.924 1.756-3.35 0a1.724 1.724 0 00-2.573-1.066c-1.543.94-3.31-.826-2.37-2.37a1.724 1.724 0 00-1.065-2.572c-1.756-.426-1.756-2.924 0-3.35a1.724 1.724 0 001.066-2.573c-.94-1.543.826-3.31 2.37-2.37.996.608 2.296.07 2.572-1.065z" />
                            <path stroke-linecap="round" stroke-linejoin="round" stroke-width="2" d="M15 12a3 3 0 11-6 0 3 3 0 016 0z" />
                        </svg>
                        {locale.t("backups.settings")}
                    </Button>
                    <Button kind={ButtonKind::Success} onclick={take_btn}>
                        <svg class="w-4 h-4" fill="none" stroke="currentColor" viewBox="0 0 24 24">
                            <path stroke-linecap="round" stroke-linejoin="round" stroke-width="2" d="M12 4v16m8-8H4" />
                        </svg>
                        {locale.t("backups.action_backup_now")}
                    </Button>
                </div>
            }
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
                <select class={INPUT_CLASS}
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
            <div class={TILE}>
                <div class={TILE_LABEL}>{locale.t("backups.overview_total")}</div>
                <div class="mt-2 flex items-baseline gap-2">
                    <span class="text-2xl font-bold text-neutral-900 dark:text-neutral-100">{total_count}</span>
                    <span class="text-xs text-neutral-500 dark:text-neutral-400 font-mono">{"("}{total_bytes_str}{")"}</span>
                </div>
            </div>

            <div class={TILE}>
                <div class={TILE_LABEL}>{locale.t("backups.overview_last")}</div>
                <div class="mt-2 text-sm font-semibold text-neutral-900 dark:text-neutral-100 truncate">{last_backup_str}</div>
            </div>

            <div class={TILE}>
                <div class={TILE_LABEL}>{locale.t("backups.overview_schedule")}</div>
                <div class="mt-2 flex items-center gap-1.5 flex-wrap">
                    <span class="text-sm font-semibold text-neutral-900 dark:text-neutral-100">{schedule_str}</span>
                    if heartbeat_active {
                        <span class={classes!(BADGE, BADGE_OK)}>{locale.t("backups.heartbeat_active")}</span>
                    }
                </div>
            </div>

            <div class={TILE}>
                <div class={TILE_LABEL}>{locale.t("backups.overview_remote")}</div>
                <div class="mt-2">
                    if s3_active {
                        <span class={classes!(BADGE, BADGE_INFO)}>{locale.t("backups.s3_active")}</span>
                    } else {
                        <span class={classes!(BADGE, BADGE_MUTED)}>{locale.t("backups.local_only")}</span>
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
    let rows = backups
        .iter()
        .map(|b| render_backup_row(locale, b, can_edit, reload.clone(), on_select_drill.clone()))
        .collect::<Vec<_>>();
    let content = ListContent {
        loaded: true,
        empty: "backups.empty",
        columns: &BACKUP_COLUMNS,
        rows: &rows,
    };
    list_view(locale, content, None, &ListOptions::default())
}

fn render_backup_row(
    locale: Locale,
    entry: &BackupEntry,
    can_edit: bool,
    reload: Callback<()>,
    on_select_drill: Callback<BackupEntry>,
) -> Row {
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
                <button type="button" class={classes!(BADGE, BADGE_OK, "cursor-pointer", "hover:opacity-80")} onclick={on_drill_click} title={locale.t("backups.view_drill")}>
                    <svg class="w-3 h-3" fill="none" stroke="currentColor" viewBox="0 0 24 24">
                        <path stroke-linecap="round" stroke-linejoin="round" stroke-width="2" d="M9 12l2 2 4-4m5.618-4.016A11.955 11.955 0 0112 2.944a11.955 11.955 0 01-8.618 3.04A12.02 12.02 0 003 9c0 5.591 3.824 10.29 9 11.622 5.176-1.332 9-6.03 9-11.622 0-1.042-.133-2.052-.382-3.016z" />
                    </svg>
                    {label}
                </button>
            }
        }
        (Some(_), _) => html! {
            <button type="button" class={classes!(BADGE, BADGE_FAIL, "cursor-pointer", "hover:opacity-80")} onclick={on_drill_click} title={locale.t("backups.view_drill")}>
                <svg class="w-3 h-3" fill="none" stroke="currentColor" viewBox="0 0 24 24">
                    <path stroke-linecap="round" stroke-linejoin="round" stroke-width="2" d="M12 9v2m0 4h.01m-6.938 4h13.856c1.54 0 2.502-1.667 1.732-3L13.732 4c-.77-1.333-2.694-1.333-3.464 0L3.34 16c-.77 1.333.192 3 1.732 3z" />
                </svg>
                {locale.t("backups.verify_failed")}
            </button>
        },
        _ => html! {
            <button type="button" class={classes!(BADGE, BADGE_MUTED, "cursor-pointer", "hover:opacity-80")} onclick={on_drill_click} title={locale.t("backups.view_drill")}>
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
                match post_empty(locale, &path).await {
                    Ok(()) => {
                        dialog::message(locale, locale.t("backups.restored"), Icon::Success).await;
                        reload.emit(());
                    }
                    Err(err) => {
                        dialog::message(locale, &err, Icon::Error).await;
                    }
                }
            });
        });
        html! {
            <ActionIconButton
                variant={ActionVariant::Restore}
                title={locale.t("backups.restore").to_string()}
                {onclick}
            />
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
                match post_empty(locale, &path).await {
                    Ok(_) => {
                        dialog::message(locale, locale.t("backups.tested"), Icon::Success).await;
                        reload.emit(());
                    }
                    Err(err) => {
                        dialog::message(locale, &err, Icon::Error).await;
                    }
                }
            });
        });
        html! {
            <ActionIconButton
                variant={ActionVariant::Metrics}
                title={locale.t("backups.action_verify").to_string()}
                {onclick}
            />
        }
    });

    let delete_btn = can_edit.then(|| {
        let question = locale.tf("backups.delete_question", &[("name", &entry.id)]);
        let path = format!("/databases/{}/backups/{}", entry.app, entry.id);
        let onclick = delete_on_click(locale, question, path, reload.clone());
        html! {
            <ActionIconButton
                variant={ActionVariant::Delete}
                title={locale.t("common.delete").to_string()}
                {onclick}
            />
        }
    });

    Row {
        key: entry.id.clone(),
        cells: vec![
            html! {
                <div class="flex items-center gap-2">
                    <span class="font-medium text-neutral-900 dark:text-neutral-200">{&entry.name}</span>
                    <span class={classes!(BADGE, BADGE_MUTED, "font-mono", "uppercase", "text-[10px]")}>
                        {&entry.engine}
                    </span>
                </div>
            },
            html! { {&entry.id} },
            html! { {format_bytes(entry.size)} },
            html! { {format_time(entry.created_at.max(0) as u64)} },
            verify_badge,
        ],
        actions: html! {
            <>
                <ActionIconButton
                    variant={ActionVariant::Download}
                    title={locale.t("backups.download").to_string()}
                    href={download_url}
                    download={filename}
                />
                {verify_btn}
                {restore_btn}
                {delete_btn}
            </>
        },
    }
}

fn render_s3_drawer(
    locale: Locale,
    snapshots: &Result<Vec<S3BackupSnapshot>, String>,
    can_edit: bool,
    reload: Callback<()>,
) -> Html {
    let snapshots = match snapshots {
        Ok(snapshots) => snapshots,
        Err(err) => return html! { <div class={classes!(CARD, "p-4")}>{ failure_box(err) }</div> },
    };
    let rows = snapshots
        .iter()
        .map(|s| render_s3_row(locale, s, can_edit, reload.clone()))
        .collect::<Vec<_>>();
    let content = ListContent {
        loaded: true,
        empty: "databases.s3_empty",
        columns: &S3_COLUMNS,
        rows: &rows,
    };
    let options = ListOptions {
        embedded: true,
        compact: true,
        ..Default::default()
    };

    html! {
        <div class={classes!(CARD, "p-4", "space-y-3")}>
            <h3 class="text-sm font-semibold text-neutral-900 dark:text-neutral-200 flex items-center gap-2">
                <svg class="w-4 h-4 text-primary-600 dark:text-primary-400" fill="none" stroke="currentColor" viewBox="0 0 24 24">
                    <path stroke-linecap="round" stroke-linejoin="round" stroke-width="2" d="M3 15a4 4 0 004 4h9a5 5 0 10-.1-9.999 5.002 5.002 0 00-9.78 2.096A4.001 4.001 0 003 15z" />
                </svg>
                {locale.t("databases.s3_dr_title")}
            </h3>
            <div class="rounded-md border border-neutral-300 dark:border-neutral-700">
                { list_view(locale, content, None, &options) }
            </div>
        </div>
    }
}

fn render_s3_row(
    locale: Locale,
    snap: &S3BackupSnapshot,
    can_edit: bool,
    reload: Callback<()>,
) -> Row {
    let download_url = format!(
        "{API_ENDPOINT}/databases/s3/download?key={}",
        js_sys::encode_uri_component(&snap.key)
    );
    let key = snap.key.clone();
    let restore_btn = can_edit.then(|| {
        let reload = reload.clone();
        let onclick = Callback::from(move |e: MouseEvent| {
            e.prevent_default();
            let key = key.clone();
            let question = locale.tf("databases.s3_restore_question", &[("key", &key)]);
            let reload = reload.clone();
            dialog::confirm_then(locale, question, async move {
                let builder = Request::post(&format!("{API_ENDPOINT}/databases/s3/restore"));
                match send_json(locale, builder, &serde_json::json!({ "key": key })).await {
                    Ok(()) => {
                        dialog::message(locale, locale.t("databases.s3_restored"), Icon::Success)
                            .await;
                        reload.emit(());
                    }
                    Err(err) => {
                        dialog::message(locale, &err, Icon::Error).await;
                    }
                }
            });
        });
        html! {
            <ActionIconButton
                variant={ActionVariant::Restore}
                title={locale.t("databases.s3_restore").to_string()}
                {onclick}
            />
        }
    });

    Row {
        key: snap.key.clone(),
        cells: vec![
            html! { <span class="font-medium">{&snap.database_name}</span> },
            html! { {&snap.key} },
            html! { {format_bytes(snap.size)} },
            html! { {format_rfc3339(&snap.last_modified)} },
        ],
        actions: html! {
            <>
                <ActionIconButton
                    variant={ActionVariant::Download}
                    title={locale.t("backups.download").to_string()}
                    href={download_url}
                    download={format!("{}.dump.gz", snap.backup_id)}
                />
                {restore_btn}
            </>
        },
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
                let body = RunBackupRequest { database_id: id };
                let builder = Request::post(&format!("{API_ENDPOINT}/backups/run"));
                match send_json(locale, builder, &body).await {
                    Ok(()) => {
                        notice.set(Some(Notice::Saved("backups.taken")));
                        show_take.set(false);
                        reload.emit(());
                    }
                    Err(err) => notice.set(Some(Notice::Failed(err))),
                }
            });
        })
    };

    let on_close = {
        let s = show_take.clone();
        Callback::from(move |()| s.set(false))
    };
    let cancel = {
        let on_close = on_close.clone();
        Callback::from(move |_: MouseEvent| on_close.emit(()))
    };
    html! {
        <Modal title={locale.t("backups.action_backup_now")} {on_close} size={ModalSize::Sm}>
            <form onsubmit={submit} class="space-y-4">
                <label class="block space-y-1">
                    <span class={FIELD_LABEL}>{locale.t("backups.select_database")}</span>
                    <select class={INPUT_CLASS} {onchange}>
                        <option key="" value="" selected={current_selected.is_none()}>{locale.t("backups.select_database_placeholder")}</option>
                        { for dbs.iter().map(|d| {
                            let id = d.id;
                            let is_sel = Some(id) == current_selected;
                            html! {
                                <option key={id.to_string()} value={id.to_string()} selected={is_sel}>
                                    {format!("{} ({})", d.name, d.spec.database.as_deref().unwrap_or("db"))}
                                </option>
                            }
                        }) }
                    </select>
                </label>

                <div class="flex justify-end gap-2 pt-2">
                    <Button kind={ButtonKind::Secondary} onclick={cancel}>
                        {locale.t("common.cancel")}
                    </Button>
                    <Button kind={ButtonKind::Success} r#type="submit">
                        {locale.t("backups.take")}
                    </Button>
                </div>
            </form>
        </Modal>
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

    let on_close = {
        let s = show_settings.clone();
        Callback::from(move |()| s.set(false))
    };
    let fields = html! {
                <div class="space-y-4 text-sm">
                    <div class="grid grid-cols-1 sm:grid-cols-2 gap-3">
                        <label class="block space-y-1">
                            <span class={FIELD_LABEL}>{locale.t("backups.schedule")}</span>
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
                            <span class={HINT}>{locale.t("backups.schedule_hint")}</span>
                        </label>

                        <label class="block space-y-1">
                            <span class={FIELD_LABEL}>{locale.t("backups.keep")}</span>
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
                        <span class="text-xs text-neutral-700 dark:text-neutral-300">{locale.t("backups.restore_test_enabled")}</span>
                    </label>

                    <label class="block space-y-1 pt-2 border-t border-neutral-200 dark:border-neutral-700">
                        <span class={FIELD_LABEL}>{locale.t("backups.heartbeat_url")}</span>
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
                        <span class={HINT}>{locale.t("backups.heartbeat_url_hint")}</span>
                    </label>

                    <div class="space-y-3 pt-3 border-t border-neutral-200 dark:border-neutral-700">
                        <h4 class="text-xs font-semibold uppercase text-neutral-500 dark:text-neutral-400 tracking-wider">{locale.t("backups.s3")}</h4>
                        <div class="grid grid-cols-1 sm:grid-cols-2 gap-3">
                            <label class="block space-y-1">
                                <span class="text-xs text-neutral-500 dark:text-neutral-400">{locale.t("backups.endpoint")}</span>
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
                                <span class="text-xs text-neutral-500 dark:text-neutral-400">{locale.t("backups.region")}</span>
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
                                <span class="text-xs text-neutral-500 dark:text-neutral-400">{locale.t("backups.prefix")}</span>
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
                                <span class="text-xs text-neutral-500 dark:text-neutral-400">{locale.t("backups.access_key")}</span>
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
                                <span class="text-xs text-neutral-500 dark:text-neutral-400">{locale.t("backups.secret_key")}</span>
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

                </div>
    };
    let card = FormCard {
        title: locale.t("backups.settings").to_string(),
        notice: html! {},
        fields,
        error: None,
        cancel: None,
        save_label: "common.update",
        can_save: true,
    };
    form_modal(locale, card, submit, on_close)
}

#[allow(clippy::too_many_arguments)]
fn load_data(
    locale: Locale,
    overview: UseStateHandle<Option<BackupsOverview>>,
    backups: UseStateHandle<Vec<BackupEntry>>,
    databases: UseStateHandle<Vec<AppEntry>>,
    s3_snapshots: UseStateHandle<Result<Vec<S3BackupSnapshot>, String>>,
    config: UseStateHandle<BackupConfig>,
    form: UseStateHandle<FormState>,
    notice: UseStateHandle<Option<Notice<&'static str>>>,
) {
    let not = notice.clone();
    spawn_local(async move {
        let loaded = fetch_json(
            locale,
            Request::get(&format!("{API_ENDPOINT}/backups/overview")),
        )
        .await
        .map(Some);
        set_loaded(&overview, &not, loaded);
    });

    let not = notice.clone();
    spawn_local(async move {
        let loaded = fetch_json(locale, Request::get(&format!("{API_ENDPOINT}/backups"))).await;
        set_loaded(&backups, &not, loaded);
    });

    let not = notice.clone();
    spawn_local(async move {
        let loaded = fetch_json(locale, Request::get(&format!("{API_ENDPOINT}/databases"))).await;
        set_loaded(&databases, &not, loaded);
    });

    // A missing S3 setup is an error of this request, so the S3 drawer shows it
    // and the page notice stays free for the other loads.
    spawn_local(async move {
        let path = format!("{API_ENDPOINT}/databases/s3/backups");
        s3_snapshots.set(fetch_json(locale, Request::get(&path)).await);
    });

    spawn_local(async move {
        let loaded = fetch_json::<BackupConfig>(
            locale,
            Request::get(&format!("{API_ENDPOINT}/backup-config")),
        )
        .await;
        if let Ok(loaded) = &loaded {
            form.set(FormState::from_config(loaded));
        }
        set_loaded(&config, &notice, loaded);
    });
}

/// Stores a loaded value, or shows the error of the load in the page notice.
fn set_loaded<T>(
    state: &UseStateHandle<T>,
    notice: &UseStateHandle<Option<Notice<&'static str>>>,
    loaded: Result<T, String>,
) {
    match loaded {
        Ok(value) => state.set(value),
        Err(err) => notice.set(Some(Notice::Failed(err))),
    }
}

#[derive(Properties, PartialEq, Clone)]
struct SubTabProps {
    can_edit: bool,
}

#[function_component(DisasterRecoveryTab)]
fn disaster_recovery_tab(props: &SubTabProps) -> Html {
    let locale = use_locale();
    let backups = use_state(|| None::<DrBackups>);
    let passphrase = use_state(String::new);
    let upload_s3 = use_state(|| true);
    let is_creating = use_state(|| false);
    let is_restoring = use_state(|| false);
    let can_edit = props.can_edit;

    let load_backups = {
        let backups = backups.clone();
        Callback::from(move |()| {
            let backups = backups.clone();
            spawn_local(fetch_dr_backups(locale, backups));
        })
    };

    {
        let load_backups = load_backups.clone();
        use_effect_with((), move |_| {
            load_backups.emit(());
            || ()
        });
    }

    let on_create = {
        let is_creating = is_creating.clone();
        let passphrase = passphrase.clone();
        let upload_s3 = upload_s3.clone();
        let load_backups = load_backups.clone();
        Callback::from(move |_: MouseEvent| {
            if *is_creating {
                return;
            }
            is_creating.set(true);
            let is_creating = is_creating.clone();
            let load_backups = load_backups.clone();
            let pass = (*passphrase).clone();
            let body = CreateDisasterRecoveryRequest {
                passphrase: if pass.is_empty() { None } else { Some(pass) },
                upload_s3: *upload_s3,
            };
            spawn_local(async move {
                execute_dr_create(locale, body, load_backups).await;
                is_creating.set(false);
            });
        })
    };

    let on_upload_file = {
        let is_restoring = is_restoring.clone();
        let passphrase = passphrase.clone();
        let load_backups = load_backups.clone();
        Callback::from(move |event: Event| {
            let input: HtmlInputElement = event.target_unchecked_into();
            let Some(file) = input.files().and_then(|files| files.get(0)) else {
                return;
            };
            let is_restoring = is_restoring.clone();
            let pass = (*passphrase).clone();
            let load_backups = load_backups.clone();
            let question = locale.t("settings.dr_restore_confirm").to_string();
            dialog::confirm_then(locale, question, async move {
                is_restoring.set(true);
                handle_dr_file_upload(file, pass, locale, load_backups).await;
                is_restoring.set(false);
            });
        })
    };

    html! {
        <div class="space-y-6">
            <div class={classes!(CARD, "p-5", "space-y-4")}>
                <div>
                    <h2 class="text-base font-semibold text-neutral-900 dark:text-neutral-200">
                        {locale.t("settings.disaster_recovery_title")}
                    </h2>
                    <p class="mt-1 text-sm text-neutral-500 dark:text-neutral-400">
                        {locale.t("settings.disaster_recovery_hint")}
                    </p>
                </div>
                if can_edit {
                    { render_dr_controls(locale, &passphrase, &upload_s3, &is_creating, &is_restoring, on_create, on_upload_file) }
                }
                { render_dr_table(locale, &backups, &passphrase, can_edit, &load_backups) }
            </div>
        </div>
    }
}

/// The disaster recovery bundles, or the error of their load.
type DrBackups = Result<Vec<DisasterRecoveryBackupEntry>, String>;

async fn fetch_dr_backups(locale: Locale, backups: UseStateHandle<Option<DrBackups>>) {
    let path = format!("{API_ENDPOINT}/disaster-recovery/backups");
    backups.set(Some(fetch_json(locale, Request::get(&path)).await));
}

async fn execute_dr_create(
    locale: Locale,
    body: CreateDisasterRecoveryRequest,
    load_backups: Callback<()>,
) {
    let req = Request::post(&format!("{API_ENDPOINT}/disaster-recovery/backups"))
        .header("content-type", "application/json")
        .json(&body);
    match req {
        Ok(built) => match send_request(locale, built).await {
            Ok(_) => {
                dialog::message(locale, locale.t("backups.taken"), Icon::Success).await;
                load_backups.emit(());
            }
            Err(err) => dialog::message(locale, &err, Icon::Error).await,
        },
        Err(err) => dialog::message(locale, &err.to_string(), Icon::Error).await,
    }
}

async fn handle_dr_file_upload(
    file: web_sys::File,
    pass: String,
    locale: Locale,
    load_backups: Callback<()>,
) {
    match wasm_bindgen_futures::JsFuture::from(file.array_buffer()).await {
        Ok(js_buf) => {
            let uint8_array = js_sys::Uint8Array::new(&js_buf);
            let bytes = uint8_array.to_vec();
            handle_dr_restore_upload(locale, bytes, pass, load_backups).await;
        }
        Err(_) => {
            dialog::message(locale, locale.t("backups.file_read_failed"), Icon::Error).await;
        }
    }
}

async fn handle_dr_restore_upload(
    locale: Locale,
    bytes: Vec<u8>,
    passphrase: String,
    load_backups: Callback<()>,
) {
    let mut builder = Request::post(&format!("{API_ENDPOINT}/disaster-recovery/restore-upload"))
        .header("content-type", "application/octet-stream");
    if !passphrase.is_empty() {
        builder = builder.header("x-passphrase", &passphrase);
    }
    let req = builder.body(wasm_bindgen::JsValue::from(js_sys::Uint8Array::from(
        bytes.as_slice(),
    )));
    match req {
        Ok(built) => match send_request(locale, built).await {
            Ok(_) => {
                dialog::message(
                    locale,
                    locale.t("settings.dr_restore_success"),
                    Icon::Success,
                )
                .await;
                load_backups.emit(());
            }
            Err(err) => dialog::message(locale, &err, Icon::Error).await,
        },
        Err(err) => dialog::message(locale, &err.to_string(), Icon::Error).await,
    }
}

fn render_dr_controls(
    locale: Locale,
    passphrase: &UseStateHandle<String>,
    upload_s3: &UseStateHandle<bool>,
    is_creating: &UseStateHandle<bool>,
    is_restoring: &UseStateHandle<bool>,
    on_create: Callback<MouseEvent>,
    on_upload_file: Callback<Event>,
) -> Html {
    let on_pass_change = {
        let passphrase = passphrase.clone();
        Callback::from(move |e: Event| {
            let input: HtmlInputElement = e.target_unchecked_into();
            passphrase.set(input.value());
        })
    };
    let on_s3_change = {
        let upload_s3 = upload_s3.clone();
        Callback::from(move |e: Event| {
            let input: HtmlInputElement = e.target_unchecked_into();
            upload_s3.set(input.checked());
        })
    };

    html! {
        <div class="flex flex-col sm:flex-row flex-wrap items-center gap-3">
            <div class="w-full sm:w-64">
                <input
                    type="password"
                    placeholder={locale.t("settings.dr_passphrase_placeholder")}
                    value={(**passphrase).clone()}
                    onchange={on_pass_change}
                    class={INPUT_CLASS}
                />
            </div>
            <label class="inline-flex items-center gap-2 text-sm text-neutral-700 dark:text-neutral-300 cursor-pointer">
                <input
                    type="checkbox"
                    checked={**upload_s3}
                    onchange={on_s3_change}
                    class="rounded border-neutral-300 dark:border-neutral-600 text-primary-600 focus:ring-primary-500"
                />
                {locale.t("settings.dr_upload_s3")}
            </label>
            <Button kind={ButtonKind::Success} onclick={on_create} disabled={**is_creating}>
                <svg class="w-4 h-4" fill="none" stroke="currentColor" viewBox="0 0 24 24">
                    <path stroke-linecap="round" stroke-linejoin="round" stroke-width="2" d="M12 4v16m8-8H4" />
                </svg>
                {locale.t("settings.create_dr_backup")}
            </Button>
            <label class={button_class(ButtonKind::Secondary, ButtonSize::Md)}>
                <svg class="w-4 h-4" fill="none" stroke="currentColor" viewBox="0 0 24 24">
                    <path stroke-linecap="round" stroke-linejoin="round" stroke-width="2" d="M4 16v1a3 3 0 003 3h10a3 3 0 003-3v-1m-4-8l4-4m0 0l4 4m-4-4v12" />
                </svg>
                {locale.t("settings.upload_restore_dr")}
                <input
                    type="file"
                    accept=".r3bundle"
                    class="hidden"
                    onchange={on_upload_file}
                    disabled={**is_restoring}
                />
            </label>
        </div>
    }
}

fn render_dr_table(
    locale: Locale,
    backups: &Option<DrBackups>,
    passphrase: &UseStateHandle<String>,
    can_edit: bool,
    load_backups: &Callback<()>,
) -> Html {
    let entries = match backups {
        Some(Err(err)) => return failure_box(err),
        Some(Ok(entries)) => entries.as_slice(),
        None => &[],
    };
    let rows = entries
        .iter()
        .map(|entry| render_dr_row(locale, entry, passphrase, can_edit, load_backups))
        .collect::<Vec<_>>();
    let content = ListContent {
        loaded: backups.is_some(),
        empty: "backups.empty",
        columns: &DR_COLUMNS,
        rows: &rows,
    };
    let options = ListOptions {
        embedded: true,
        ..Default::default()
    };
    html! {
        <div class="rounded-md border border-neutral-300 dark:border-neutral-700">
            { list_view(locale, content, None, &options) }
        </div>
    }
}

fn render_dr_row(
    locale: Locale,
    entry: &DisasterRecoveryBackupEntry,
    passphrase: &UseStateHandle<String>,
    can_edit: bool,
    load_backups: &Callback<()>,
) -> Row {
    let date_str = format_unix_time(locale, entry.created_at);
    let download_url = format!(
        "{API_ENDPOINT}/disaster-recovery/backups/{}/download",
        entry.id
    );

    let on_restore = {
        let id = entry.id.clone();
        let pass = (**passphrase).clone();
        let load_backups = load_backups.clone();
        Callback::from(move |_: MouseEvent| {
            let id = id.clone();
            let pass = pass.clone();
            let load_backups = load_backups.clone();
            let question = locale.t("settings.dr_restore_confirm").to_string();
            dialog::confirm_then(locale, question, async move {
                let body = RestoreDisasterRecoveryRequest {
                    passphrase: if pass.is_empty() { None } else { Some(pass) },
                };
                let builder = Request::post(&format!(
                    "{API_ENDPOINT}/disaster-recovery/backups/{id}/restore"
                ));
                match send_json(locale, builder, &body).await {
                    Ok(()) => {
                        dialog::message(
                            locale,
                            locale.t("settings.dr_restore_success"),
                            Icon::Success,
                        )
                        .await;
                        load_backups.emit(());
                    }
                    Err(err) => dialog::message(locale, &err, Icon::Error).await,
                }
            });
        })
    };

    let on_delete = delete_on_click(
        locale,
        locale.tf("backups.delete_question", &[("name", &entry.id)]),
        format!("/disaster-recovery/backups/{}", entry.id),
        load_backups.clone(),
    );

    let status_badge = if entry.s3_replicated {
        html! { <span class={classes!(BADGE, BADGE_OK)}>{locale.t("settings.dr_s3_badge")}</span> }
    } else {
        html! { <span class={classes!(BADGE, BADGE_MUTED)}>{locale.t("settings.dr_local_only")}</span> }
    };

    Row {
        key: entry.id.clone(),
        cells: vec![
            html! { {&entry.id} },
            html! { {date_str} },
            html! { {format_bytes(entry.size)} },
            html! { {entry.files_count} },
            status_badge,
        ],
        actions: html! {
            <>
                <ActionIconButton
                    variant={ActionVariant::Download}
                    title={locale.t("backups.download").to_string()}
                    href={download_url}
                    download={format!("{}.r3bundle", entry.id)}
                />
                if can_edit {
                    <ActionIconButton
                        variant={ActionVariant::Restore}
                        title={locale.t("backups.restore").to_string()}
                        onclick={on_restore}
                    />
                    <ActionIconButton
                        variant={ActionVariant::Delete}
                        title={locale.t("common.delete").to_string()}
                        onclick={on_delete}
                    />
                }
            </>
        },
    }
}

#[function_component(ConfigurationBackupTab)]
fn configuration_backup_tab(props: &SubTabProps) -> Html {
    let locale = use_locale();
    let is_exporting = use_state(|| false);
    let is_restoring = use_state(|| false);
    let can_edit = props.can_edit;

    let on_export = {
        let is_exporting = is_exporting.clone();
        Callback::from(move |_: MouseEvent| {
            if *is_exporting {
                return;
            }
            is_exporting.set(true);
            let is_exporting = is_exporting.clone();
            spawn_local(async move {
                let exported = export_config(locale).await;
                is_exporting.set(false);
                match exported {
                    Ok(text) => {
                        let now = js_sys::Date::now() as u64 / 1000;
                        crate::pages::app_log::download_text_file(
                            &format!("r3v3rs3-config-backup-{now}.json"),
                            &text,
                        );
                    }
                    Err(err) => dialog::message(locale, &err, Icon::Error).await,
                }
            });
        })
    };

    let on_file_change = {
        let is_restoring = is_restoring.clone();
        Callback::from(move |event: Event| {
            let input: HtmlInputElement = event.target_unchecked_into();
            let Some(file) = input.files().and_then(|files| files.get(0)) else {
                return;
            };
            let is_restoring = is_restoring.clone();
            let question = locale.t("settings.restore_confirm").to_string();
            dialog::confirm_then(locale, question, async move {
                is_restoring.set(true);
                if let Ok(js_val) = wasm_bindgen_futures::JsFuture::from(file.text()).await {
                    let text = js_val.as_string().unwrap_or_default();
                    handle_config_restore_payload(locale, &text).await;
                }
                is_restoring.set(false);
            });
        })
    };

    html! {
        <div class="space-y-6">
            <div class={classes!(CARD, "p-5", "space-y-4")}>
                <div>
                    <h2 class="text-base font-semibold text-neutral-900 dark:text-neutral-200">{locale.t("settings.backup_restore_title")}</h2>
                    <p class="mt-1 text-sm text-neutral-500 dark:text-neutral-400">{locale.t("settings.backup_restore_hint")}</p>
                </div>
                <div class="flex flex-wrap items-center gap-4 pt-2">
                    <Button kind={ButtonKind::Secondary} onclick={on_export} disabled={*is_exporting}>
                        <svg class="w-4 h-4" fill="none" stroke="currentColor" viewBox="0 0 24 24">
                            <path stroke-linecap="round" stroke-linejoin="round" stroke-width="2" d="M4 16v1a3 3 0 003 3h10a3 3 0 003-3v-1m-4-4l-4 4m0 0l-4-4m4 4V4" />
                        </svg>
                        {locale.t("settings.export_backup")}
                    </Button>
                    if can_edit {
                        <label class={button_class(ButtonKind::Success, ButtonSize::Md)}>
                            <svg class="w-4 h-4" fill="none" stroke="currentColor" viewBox="0 0 24 24">
                                <path stroke-linecap="round" stroke-linejoin="round" stroke-width="2" d="M4 16v1a3 3 0 003 3h10a3 3 0 003-3v-1m-4-8l4-4m0 0l4 4m-4-4v12" />
                            </svg>
                            {locale.t("settings.restore_backup")}
                            <input
                                type="file"
                                accept=".json"
                                class="hidden"
                                onchange={on_file_change}
                                disabled={*is_restoring}
                            />
                        </label>
                    }
                </div>
            </div>
        </div>
    }
}

/// Reads the configuration backup. The error is the message of the failure in the selected
/// language.
async fn export_config(locale: Locale) -> Result<String, String> {
    let request = Request::get(&format!("{API_ENDPOINT}/config/backup"))
        .build()
        .map_err(|err| format_network_error(locale, &err))?;
    send_request(locale, request)
        .await?
        .text()
        .await
        .map_err(|_| locale.t("common.response_parse_failed").to_string())
}

async fn handle_config_restore_payload(locale: Locale, text: &str) {
    let Ok(val) = serde_json::from_str::<serde_json::Value>(text) else {
        dialog::message(
            locale,
            locale.t("settings.restore_invalid_file"),
            Icon::Error,
        )
        .await;
        return;
    };
    let req = Request::post(&format!("{API_ENDPOINT}/config/restore"))
        .header("content-type", "application/json")
        .json(&val);
    match req {
        Ok(built) => match send_request(locale, built).await {
            Ok(_) => {
                dialog::message(locale, locale.t("settings.restore_success"), Icon::Success).await
            }
            Err(err) => dialog::message(locale, &err, Icon::Error).await,
        },
        Err(err) => dialog::message(locale, &err.to_string(), Icon::Error).await,
    }
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
