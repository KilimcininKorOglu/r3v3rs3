use crate::components::button::{Button, ButtonKind};
use crate::components::modal::{Modal, ModalSize};
use crate::i18n::use_locale;
use r3v3rs3_api::platform::{BackupDrillDetails, BackupEntry};
use yew::prelude::*;

#[derive(Properties, PartialEq, Clone)]
pub struct BackupDrillModalProps {
    pub entry: BackupEntry,
    pub on_close: Callback<()>,
    pub on_rerun: Callback<()>,
    #[prop_or_default]
    pub is_running: bool,
}

const TILE: &str = "p-3 rounded-xl bg-neutral-50 border border-neutral-200 dark:bg-neutral-900/50 dark:border-neutral-700 space-y-1";
const TILE_LABEL: &str = "text-[11px] uppercase tracking-wider text-neutral-500 dark:text-neutral-400 block font-semibold";
const TILE_VALUE: &str = "font-mono text-xs text-neutral-900 dark:text-neutral-200 truncate";
const TILE_NOTE: &str = "text-[10px] text-neutral-500 dark:text-neutral-400 font-mono";

#[function_component(BackupDrillModal)]
pub fn backup_drill_modal(props: &BackupDrillModalProps) -> Html {
    let locale = use_locale();
    let entry = &props.entry;
    let drill = entry.drill.as_ref();

    let on_close_click = {
        let on_close = props.on_close.clone();
        Callback::from(move |_| on_close.emit(()))
    };

    let on_rerun_click = {
        let on_rerun = props.on_rerun.clone();
        Callback::from(move |_| on_rerun.emit(()))
    };

    let (status_bg, status_text, status_title, is_valid, is_failed) =
        modal_status_style(locale, entry);

    let footer = html! {
        <div class="flex w-full items-center justify-between gap-2">
            <span class="text-xs font-mono text-neutral-500">
                {locale.tf("backups.drill_backup_id", &[("id", &entry.id.to_string())])}
            </span>
            <div class="flex items-center gap-2">
                <Button kind={ButtonKind::Secondary} onclick={on_close_click}>
                    {locale.t("backups.close")}
                </Button>
                <Button kind={ButtonKind::Success} onclick={on_rerun_click} disabled={props.is_running}>
                    <svg class={classes!("w-3.5", "h-3.5", props.is_running.then_some("animate-spin"))} fill="none" stroke="currentColor" viewBox="0 0 24 24">
                        <path stroke-linecap="round" stroke-linejoin="round" stroke-width="2" d="M4 4v5h.582m15.356 2A8.001 8.001 0 004.582 9m0 0H9m11 11v-5h-.581m0 0a8.003 8.003 0 01-15.357-2m15.357 2H15" />
                    </svg>
                    if props.is_running {
                        {locale.t("backups.drill_running")}
                    } else {
                        {locale.t("backups.drill_run_now")}
                    }
                </Button>
            </div>
        </div>
    };

    html! {
        <Modal title={locale.t("backups.drill_title")} on_close={props.on_close.clone()} size={ModalSize::Lg} {footer}>
            <div class="space-y-5">
                <p class="text-xs text-neutral-500 dark:text-neutral-400">{locale.t("backups.drill_subtitle")}</p>
                <div class={classes!("p-4", "rounded-xl", "border", "flex", "items-start", "gap-3", status_bg)}>
                    <div class="mt-0.5">
                        {render_status_icon(is_valid, is_failed)}
                    </div>
                    <div class="flex-1 min-w-0">
                        <div class="font-medium text-sm">{status_title}</div>
                        <div class="text-xs opacity-90 mt-0.5 font-mono break-words">{status_text}</div>
                    </div>
                </div>

                {render_metrics_grid(locale, entry, drill)}

                {render_tables_section(locale, drill)}
            </div>
        </Modal>
    }
}

fn modal_status_style(
    locale: r3v3rs3_api::i18n::Locale,
    entry: &BackupEntry,
) -> (&'static str, String, &'static str, bool, bool) {
    let drill = entry.drill.as_ref();
    let is_valid = entry.verification_status.as_deref() == Some("valid")
        || drill.map(|d| d.integrity_check_passed).unwrap_or(false);
    let is_failed = entry
        .verification_status
        .as_deref()
        .map(|s| s.starts_with("failed"))
        .unwrap_or(false);

    if is_valid {
        (
            "bg-emerald-50 border-emerald-300 text-emerald-800 dark:bg-emerald-950/60 dark:border-emerald-800 dark:text-emerald-300",
            drill
                .map(|d| d.summary.clone())
                .unwrap_or_else(|| locale.t("backups.drill_valid").to_string()),
            locale.t("backups.drill_passed"),
            true,
            false,
        )
    } else if is_failed {
        (
            "bg-red-50 border-red-300 text-red-800 dark:bg-red-950/60 dark:border-red-800 dark:text-red-300",
            entry
                .verification_status
                .clone()
                .unwrap_or_else(|| locale.t("backups.drill_check_failed").to_string()),
            locale.t("backups.drill_failed"),
            false,
            true,
        )
    } else {
        (
            "bg-neutral-50 border-neutral-300 text-neutral-700 dark:bg-neutral-800/60 dark:border-neutral-700 dark:text-neutral-300",
            locale.t("backups.untested").to_string(),
            locale.t("backups.untested"),
            false,
            false,
        )
    }
}

fn render_status_icon(is_valid: bool, is_failed: bool) -> Html {
    if is_valid {
        html! {
            <svg class="w-5 h-5 text-emerald-600 dark:text-emerald-400" fill="none" stroke="currentColor" viewBox="0 0 24 24">
                <path stroke-linecap="round" stroke-linejoin="round" stroke-width="2" d="M5 13l4 4L19 7" />
            </svg>
        }
    } else if is_failed {
        html! {
            <svg class="w-5 h-5 text-red-600 dark:text-red-400" fill="none" stroke="currentColor" viewBox="0 0 24 24">
                <path stroke-linecap="round" stroke-linejoin="round" stroke-width="2" d="M12 9v2m0 4h.01m-6.938 4h13.856c1.54 0 2.502-1.667 1.732-3L13.732 4c-.77-1.333-2.694-1.333-3.464 0L3.34 16c-.77 1.333.192 3 1.732 3z" />
            </svg>
        }
    } else {
        html! {
            <svg class="w-5 h-5 text-neutral-500 dark:text-neutral-400" fill="none" stroke="currentColor" viewBox="0 0 24 24">
                <path stroke-linecap="round" stroke-linejoin="round" stroke-width="2" d="M13 16h-1v-4h-1m1-4h.01M21 12a9 9 0 11-18 0 9 9 0 0118 0z" />
            </svg>
        }
    }
}

fn render_metrics_grid(
    locale: r3v3rs3_api::i18n::Locale,
    entry: &BackupEntry,
    drill: Option<&BackupDrillDetails>,
) -> Html {
    let mode = drill
        .map(|d| d.drill_mode.clone())
        .unwrap_or_else(|| format!("{}_dump", entry.engine));
    let duration = drill
        .map(|d| format!("{} ms", d.drill_duration_ms))
        .unwrap_or_else(|| "-".to_string());
    let table_count = drill
        .map(|d| d.table_count.to_string())
        .unwrap_or_else(|| "-".to_string());
    let objects_count = drill
        .map(|d| {
            format!(
                "{} {}",
                d.schema_objects_count,
                locale.t("backups.drill_objects_count")
            )
        })
        .unwrap_or_default();

    html! {
        <div class="grid grid-cols-2 sm:grid-cols-4 gap-3">
            <div class={TILE}>
                <span class={TILE_LABEL}>{locale.t("backups.drill_checksum")}</span>
                <div class={TILE_VALUE} title={entry.sha256.clone()}>
                    {entry.sha256.chars().take(12).collect::<String>()}{"..."}
                </div>
                <span class="inline-flex text-[10px] text-emerald-600 dark:text-emerald-400 font-medium">{"✓ "}{locale.t("backups.drill_integrity")}</span>
            </div>

            <div class={TILE}>
                <span class={TILE_LABEL}>{locale.t("backups.drill_mode")}</span>
                <div class={TILE_VALUE}>{mode}</div>
                <span class={TILE_NOTE}>{duration}</span>
            </div>

            <div class={TILE}>
                <span class={TILE_LABEL}>{locale.t("backups.drill_tables_count")}</span>
                <div class="text-base font-bold text-neutral-900 dark:text-neutral-100">{table_count}</div>
                <span class={TILE_NOTE}>{objects_count}</span>
            </div>

            <div class={TILE}>
                <span class={TILE_LABEL}>{locale.t("backups.drill_data_detected")}</span>
                <div class="text-xs font-semibold">
                    if drill.map(|d| d.has_data).unwrap_or(false) {
                        <span class="text-emerald-600 dark:text-emerald-400">{"✓ "}{locale.t("backups.drill_data_detected")}</span>
                    } else {
                        <span class="text-neutral-500 dark:text-neutral-400">{"-"}</span>
                    }
                </div>
                <span class={TILE_NOTE}>{entry.engine.to_uppercase()}</span>
            </div>
        </div>
    }
}

fn render_tables_section(
    locale: r3v3rs3_api::i18n::Locale,
    drill: Option<&BackupDrillDetails>,
) -> Html {
    let Some(drill) = drill else {
        return html! {};
    };

    if drill.tables_sample.is_empty() {
        return html! {};
    }

    html! {
        <div class="space-y-2">
            <div class="flex items-center justify-between">
                <span class="text-xs font-semibold uppercase tracking-wider text-neutral-500 dark:text-neutral-400">
                    {locale.t("backups.drill_tables_sample")}
                </span>
                <span class="text-xs text-neutral-500">
                    {format!("{} / {}", drill.tables_sample.len(), drill.table_count)}
                </span>
            </div>
            <div class="flex flex-wrap gap-1.5 p-3 rounded-xl bg-neutral-50 border border-neutral-200 dark:bg-neutral-900/60 dark:border-neutral-700 max-h-36 overflow-y-auto">
                {for drill.tables_sample.iter().map(|tbl| {
                    html! {
                        <span class="inline-flex items-center gap-1 px-2.5 py-1 rounded-md text-xs font-mono bg-white text-neutral-800 border border-neutral-200 dark:bg-neutral-800 dark:text-neutral-200 dark:border-neutral-700">
                            <span class="w-1.5 h-1.5 rounded-full bg-emerald-500"></span>
                            {tbl}
                        </span>
                    }
                })}
                if drill.table_count > drill.tables_sample.len() as u32 {
                    <span class="inline-flex items-center px-2 py-1 rounded-md text-xs text-neutral-500 dark:text-neutral-400 italic">
                        {locale.tf("backups.drill_more_tables", &[("count", &(drill.table_count - drill.tables_sample.len() as u32).to_string())])}
                    </span>
                }
            </div>
        </div>
    }
}
