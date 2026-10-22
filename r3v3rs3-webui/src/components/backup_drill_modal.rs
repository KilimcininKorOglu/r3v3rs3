use crate::i18n::use_locale;
use r3v3rs3_api::platform::{BackupDrillDetails, BackupEntry};
use yew::prelude::*;

const BUTTON_PRIMARY: &str = "inline-flex items-center gap-1.5 px-3 py-1.5 rounded-lg text-xs font-medium bg-white text-neutral-900 hover:bg-neutral-100 transition shadow-sm disabled:opacity-50";
const BUTTON_SECONDARY: &str = "inline-flex items-center gap-1.5 px-3 py-1.5 rounded-lg text-xs font-medium bg-neutral-800 text-neutral-200 hover:bg-neutral-700 border border-neutral-700 transition";

#[derive(Properties, PartialEq, Clone)]
pub struct BackupDrillModalProps {
    pub entry: BackupEntry,
    pub on_close: Callback<()>,
    pub on_rerun: Callback<()>,
    #[prop_or_default]
    pub is_running: bool,
}

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

    html! {
        <div class="fixed inset-0 z-50 flex items-center justify-center p-4 bg-black/70 backdrop-blur-sm animate-fade-in" onclick={on_close_click.clone()}>
            <div class="bg-neutral-900 border border-neutral-700 rounded-2xl shadow-2xl max-w-2xl w-full p-6 text-neutral-100 space-y-5 animate-scale-in" onclick={Callback::from(|e: MouseEvent| e.stop_propagation())}>
                <div class="flex items-start justify-between gap-4 border-b border-neutral-800 pb-4">
                    <div class="space-y-1">
                        <div class="flex items-center gap-2">
                            <span class="p-1.5 rounded-lg bg-neutral-800 text-neutral-200 border border-neutral-700">
                                <svg class="w-5 h-5 text-emerald-400" fill="none" stroke="currentColor" viewBox="0 0 24 24">
                                    <path stroke-linecap="round" stroke-linejoin="round" stroke-width="2" d="M9 12l2 2 4-4m5.618-4.016A11.955 11.955 0 0112 2.944a11.955 11.955 0 01-8.618 3.04A12.02 12.02 0 003 9c0 5.591 3.824 10.29 9 11.622 5.176-1.332 9-6.03 9-11.622 0-1.042-.133-2.052-.382-3.016z" />
                                </svg>
                            </span>
                            <h3 class="text-lg font-semibold tracking-tight">{locale.t("backups.drill_title")}</h3>
                        </div>
                        <p class="text-xs text-neutral-400">{locale.t("backups.drill_subtitle")}</p>
                    </div>
                    <button type="button" class="text-neutral-400 hover:text-white p-1 rounded-lg hover:bg-neutral-800 transition" onclick={on_close_click.clone()}>
                        <svg class="w-5 h-5" fill="none" stroke="currentColor" viewBox="0 0 24 24">
                            <path stroke-linecap="round" stroke-linejoin="round" stroke-width="2" d="M6 18L18 6M6 6l12 12" />
                        </svg>
                    </button>
                </div>

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

                <div class="flex items-center justify-between pt-4 border-t border-neutral-800">
                    <span class="text-xs font-mono text-neutral-500">
                        {format!("ID: {}", entry.id)}
                    </span>
                    <div class="flex items-center gap-2">
                        <button type="button" class={BUTTON_SECONDARY} onclick={on_close_click}>
                            {locale.t("backups.close")}
                        </button>
                        <button type="button" class={BUTTON_PRIMARY} onclick={on_rerun_click} disabled={props.is_running}>
                            <svg class={classes!("w-3.5", "h-3.5", props.is_running.then_some("animate-spin"))} fill="none" stroke="currentColor" viewBox="0 0 24 24">
                                <path stroke-linecap="round" stroke-linejoin="round" stroke-width="2" d="M4 4v5h.582m15.356 2A8.001 8.001 0 004.582 9m0 0H9m11 11v-5h-.581m0 0a8.003 8.003 0 01-15.357-2m15.357 2H15" />
                            </svg>
                            if props.is_running {
                                {locale.t("backups.drill_running")}
                            } else {
                                {locale.t("backups.drill_run_now")}
                            }
                        </button>
                    </div>
                </div>
            </div>
        </div>
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
            "bg-emerald-950/60 border-emerald-800 text-emerald-300",
            drill
                .map(|d| d.summary.clone())
                .unwrap_or_else(|| "Valid".to_string()),
            locale.t("backups.drill_passed"),
            true,
            false,
        )
    } else if is_failed {
        (
            "bg-red-950/60 border-red-800 text-red-300",
            entry
                .verification_status
                .clone()
                .unwrap_or_else(|| "Failed integrity check".to_string()),
            locale.t("backups.drill_failed"),
            false,
            true,
        )
    } else {
        (
            "bg-neutral-800/60 border-neutral-700 text-neutral-300",
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
            <svg class="w-5 h-5 text-emerald-400" fill="none" stroke="currentColor" viewBox="0 0 24 24">
                <path stroke-linecap="round" stroke-linejoin="round" stroke-width="2" d="M5 13l4 4L19 7" />
            </svg>
        }
    } else if is_failed {
        html! {
            <svg class="w-5 h-5 text-red-400" fill="none" stroke="currentColor" viewBox="0 0 24 24">
                <path stroke-linecap="round" stroke-linejoin="round" stroke-width="2" d="M12 9v2m0 4h.01m-6.938 4h13.856c1.54 0 2.502-1.667 1.732-3L13.732 4c-.77-1.333-2.694-1.333-3.464 0L3.34 16c-.77 1.333.192 3 1.732 3z" />
            </svg>
        }
    } else {
        html! {
            <svg class="w-5 h-5 text-neutral-400" fill="none" stroke="currentColor" viewBox="0 0 24 24">
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
            <div class="p-3 rounded-xl bg-neutral-800/50 border border-neutral-700/60 space-y-1">
                <span class="text-[11px] uppercase tracking-wider text-neutral-400 block font-semibold">{locale.t("backups.drill_checksum")}</span>
                <div class="font-mono text-xs text-neutral-200 truncate" title={entry.sha256.clone()}>
                    {entry.sha256.chars().take(12).collect::<String>()}{"..."}
                </div>
                <span class="inline-flex text-[10px] text-emerald-400 font-medium">{"✓ "}{locale.t("backups.drill_integrity")}</span>
            </div>

            <div class="p-3 rounded-xl bg-neutral-800/50 border border-neutral-700/60 space-y-1">
                <span class="text-[11px] uppercase tracking-wider text-neutral-400 block font-semibold">{locale.t("backups.drill_mode")}</span>
                <div class="font-mono text-xs text-neutral-200 truncate">{mode}</div>
                <span class="text-[10px] text-neutral-400 font-mono">{duration}</span>
            </div>

            <div class="p-3 rounded-xl bg-neutral-800/50 border border-neutral-700/60 space-y-1">
                <span class="text-[11px] uppercase tracking-wider text-neutral-400 block font-semibold">{locale.t("backups.drill_tables_count")}</span>
                <div class="text-base font-bold text-neutral-100">{table_count}</div>
                <span class="text-[10px] text-neutral-400">{objects_count}</span>
            </div>

            <div class="p-3 rounded-xl bg-neutral-800/50 border border-neutral-700/60 space-y-1">
                <span class="text-[11px] uppercase tracking-wider text-neutral-400 block font-semibold">{locale.t("backups.drill_data_detected")}</span>
                <div class="text-xs font-semibold">
                    if drill.map(|d| d.has_data).unwrap_or(false) {
                        <span class="text-emerald-400">{"✓ "}{locale.t("backups.drill_data_detected")}</span>
                    } else {
                        <span class="text-neutral-400">{"-"}</span>
                    }
                </div>
                <span class="text-[10px] text-neutral-400 font-mono">{entry.engine.to_uppercase()}</span>
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
                <span class="text-xs font-semibold uppercase tracking-wider text-neutral-400">
                    {locale.t("backups.drill_tables_sample")}
                </span>
                <span class="text-xs text-neutral-500">
                    {format!("{} / {}", drill.tables_sample.len(), drill.table_count)}
                </span>
            </div>
            <div class="flex flex-wrap gap-1.5 p-3 rounded-xl bg-neutral-950/60 border border-neutral-800 max-h-36 overflow-y-auto">
                {for drill.tables_sample.iter().map(|tbl| {
                    html! {
                        <span class="inline-flex items-center gap-1 px-2.5 py-1 rounded-md text-xs font-mono bg-neutral-800/80 text-neutral-200 border border-neutral-700/60">
                            <span class="w-1.5 h-1.5 rounded-full bg-emerald-400"></span>
                            {tbl}
                        </span>
                    }
                })}
                if drill.table_count > drill.tables_sample.len() as u32 {
                    <span class="inline-flex items-center px-2 py-1 rounded-md text-xs text-neutral-400 italic">
                        {format!("+{} more...", drill.table_count - drill.tables_sample.len() as u32)}
                    </span>
                }
            </div>
        </div>
    }
}
