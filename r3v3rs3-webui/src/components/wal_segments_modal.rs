use crate::API_ENDPOINT;
use crate::format::{format_bytes, format_time};
use crate::i18n::use_locale;
use crate::pages::settings::fetch_json;
use gloo_net::http::Request;
use r3v3rs3_api::i18n::Locale;
use r3v3rs3_api::id::ShortId;
use r3v3rs3_api::platform::WalSegment;
use wasm_bindgen_futures::spawn_local;
use yew::prelude::*;

const BUTTON_PRIMARY: &str = "inline-flex items-center gap-1.5 px-3 py-1.5 rounded-lg text-xs font-medium bg-neutral-100 text-neutral-900 hover:bg-white transition shadow-sm disabled:opacity-50";
const BUTTON_SECONDARY: &str = "inline-flex items-center gap-1.5 px-3 py-1.5 rounded-lg text-xs font-medium bg-neutral-800 text-neutral-200 hover:bg-neutral-700 border border-neutral-700 transition";

#[derive(Properties, PartialEq, Clone)]
pub struct WalSegmentsModalProps {
    pub db_id: ShortId,
    pub on_close: Callback<()>,
    pub can_edit: bool,
    #[prop_or_default]
    pub on_archived: Option<Callback<()>>,
}

#[function_component(WalSegmentsModal)]
pub fn wal_segments_modal(props: &WalSegmentsModalProps) -> Html {
    let locale = use_locale();
    let segments = use_state(Vec::<WalSegment>::new);
    let loading = use_state(|| true);
    let error = use_state(|| Option::<String>::None);
    let archiving = use_state(|| false);

    let db_id = props.db_id;
    let on_close_click = {
        let on_close = props.on_close.clone();
        Callback::from(move |_| on_close.emit(()))
    };

    use_effect_with(db_id, {
        let segments = segments.clone();
        let loading = loading.clone();
        let error = error.clone();
        move |&id| {
            load_segments(locale, id, segments, loading, error);
        }
    });

    let on_archive_click = make_archive_callback(
        locale,
        db_id,
        segments.clone(),
        loading.clone(),
        error.clone(),
        archiving.clone(),
        props.on_archived.clone(),
    );

    let content = if *loading {
        render_loading(locale)
    } else if let Some(err) = (*error).as_deref() {
        render_error(locale, err)
    } else {
        render_body(locale, &segments)
    };

    html! {
        <div class="fixed inset-0 z-50 flex items-center justify-center p-4 bg-black/70 backdrop-blur-sm animate-fade-in" onclick={on_close_click.clone()}>
            <div class="bg-neutral-900 border border-neutral-700 rounded-2xl shadow-2xl max-w-4xl w-full p-6 text-neutral-100 space-y-5 animate-scale-in" onclick={Callback::from(|e: MouseEvent| e.stop_propagation())}>
                { render_modal_header(locale, on_close_click.clone()) }
                { content }
                { render_modal_footer(locale, on_close_click, props.can_edit, on_archive_click, *archiving) }
            </div>
        </div>
    }
}

fn load_segments(
    locale: Locale,
    id: ShortId,
    segments: UseStateHandle<Vec<WalSegment>>,
    loading: UseStateHandle<bool>,
    error: UseStateHandle<Option<String>>,
) {
    loading.set(true);
    error.set(None);
    spawn_local(async move {
        let url = format!("{API_ENDPOINT}/databases/{id}/wal/segments");
        match fetch_json::<Vec<WalSegment>>(locale, Request::get(&url)).await {
            Ok(list) => {
                segments.set(list);
                loading.set(false);
            }
            Err(err) => {
                error.set(Some(err));
                loading.set(false);
            }
        }
    });
}

fn make_archive_callback(
    locale: Locale,
    db_id: ShortId,
    segments: UseStateHandle<Vec<WalSegment>>,
    loading: UseStateHandle<bool>,
    error: UseStateHandle<Option<String>>,
    archiving: UseStateHandle<bool>,
    on_archived: Option<Callback<()>>,
) -> Callback<MouseEvent> {
    Callback::from(move |ev: MouseEvent| {
        ev.prevent_default();
        let (segments, loading, error, archiving) = (
            segments.clone(),
            loading.clone(),
            error.clone(),
            archiving.clone(),
        );
        let on_archived = on_archived.clone();
        archiving.set(true);
        spawn_local(async move {
            let url = format!("{API_ENDPOINT}/databases/{db_id}/wal/archive");
            let _ = Request::post(&url).send().await;
            archiving.set(false);
            if let Some(cb) = on_archived {
                cb.emit(());
            }
            load_segments(locale, db_id, segments, loading, error);
        });
    })
}

fn render_modal_header(locale: Locale, on_close: Callback<MouseEvent>) -> Html {
    html! {
        <div class="flex items-start justify-between gap-4 border-b border-neutral-800 pb-4">
            <div class="space-y-1">
                <div class="flex items-center gap-2">
                    <span class="p-1.5 rounded-lg bg-neutral-800 text-neutral-200 border border-neutral-700">
                        <svg class="w-5 h-5 text-cyan-400" fill="none" stroke="currentColor" viewBox="0 0 24 24">
                            <path stroke-linecap="round" stroke-linejoin="round" stroke-width="2" d="M19 11H5m14 0a2 2 0 012 2v6a2 2 0 01-2 2H5a2 2 0 01-2-2v-6a2 2 0 012-2m14 0V9a2 2 0 00-2-2M5 11V9a2 2 0 012-2m0 0V5a2 2 0 012-2h6a2 2 0 012 2v2M7 7h10" />
                        </svg>
                    </span>
                    <h3 class="text-lg font-semibold tracking-tight">{locale.t("databases.wal_segments_title")}</h3>
                </div>
                <p class="text-xs text-neutral-400">{locale.t("databases.wal_segments_subtitle")}</p>
            </div>
            <button type="button" class="text-neutral-400 hover:text-white p-1 rounded-lg hover:bg-neutral-800 transition" onclick={on_close}>
                <svg class="w-5 h-5" fill="none" stroke="currentColor" viewBox="0 0 24 24">
                    <path stroke-linecap="round" stroke-linejoin="round" stroke-width="2" d="M6 18L18 6M6 6l12 12" />
                </svg>
            </button>
        </div>
    }
}

fn render_loading(locale: Locale) -> Html {
    html! {
        <div class="p-8 text-center text-neutral-400 flex items-center justify-center gap-2">
            <svg class="w-5 h-5 animate-spin text-neutral-400" fill="none" stroke="currentColor" viewBox="0 0 24 24">
                <path stroke-linecap="round" stroke-linejoin="round" stroke-width="2" d="M4 4v5h.582m15.356 2A8.001 8.001 0 004.582 9m0 0H9m11 11v-5h-.581m0 0a8.003 8.003 0 01-15.357-2m15.357 2H15" />
            </svg>
            <span>{locale.t("common.loading")}</span>
        </div>
    }
}

fn render_error(locale: Locale, err: &str) -> Html {
    html! {
        <div class="p-4 rounded-xl border border-red-800/60 bg-red-950/30 text-red-300 text-sm flex items-start gap-2">
            <svg class="w-5 h-5 text-red-400 shrink-0 mt-0.5" fill="none" stroke="currentColor" viewBox="0 0 24 24">
                <path stroke-linecap="round" stroke-linejoin="round" stroke-width="2" d="M12 8v4m0 4h.01M21 12a9 9 0 11-18 0 9 9 0 0118 0z" />
            </svg>
            <div>
                <p class="font-medium">{locale.t("databases.wal_load_error")}</p>
                <p class="text-xs text-red-400 font-mono mt-0.5">{err}</p>
            </div>
        </div>
    }
}

fn render_body(locale: Locale, segments: &[WalSegment]) -> Html {
    if segments.is_empty() {
        return html! {
            <div class="p-8 text-center text-neutral-400">
                <p class="text-sm">{locale.t("databases.wal_segments_empty")}</p>
            </div>
        };
    }

    let total_size: u64 = segments.iter().map(|s| s.size).sum();
    let earliest = segments
        .iter()
        .map(|s| s.start_time)
        .filter(|&t| t > 0)
        .min();
    let latest = segments.iter().map(|s| s.end_time).filter(|&t| t > 0).max();

    let range_label = match (earliest, latest) {
        (Some(e), Some(l)) => format!("{} → {}", format_time(e), format_time(l)),
        _ => "-".to_string(),
    };

    html! {
        <div class="space-y-4">
            { render_kpi_summary(locale, segments.len(), total_size, &range_label) }
            { render_table(locale, segments) }
        </div>
    }
}

fn render_kpi_summary(locale: Locale, count: usize, total_size: u64, range: &str) -> Html {
    html! {
        <div class="grid grid-cols-1 sm:grid-cols-3 gap-3">
            <div class="p-3 rounded-xl bg-neutral-800/60 border border-neutral-700/60">
                <div class="text-xs text-neutral-400">{locale.t("databases.wal_total_segments")}</div>
                <div class="text-xl font-bold font-mono text-neutral-100 mt-1">{count}</div>
            </div>
            <div class="p-3 rounded-xl bg-neutral-800/60 border border-neutral-700/60">
                <div class="text-xs text-neutral-400">{locale.t("databases.wal_total_size")}</div>
                <div class="text-xl font-bold font-mono text-cyan-400 mt-1">{format_bytes(total_size)}</div>
            </div>
            <div class="p-3 rounded-xl bg-neutral-800/60 border border-neutral-700/60">
                <div class="text-xs text-neutral-400">{locale.t("databases.wal_segment_range")}</div>
                <div class="text-xs font-mono text-neutral-300 mt-1.5 truncate" title={range.to_string()}>{range}</div>
            </div>
        </div>
    }
}

fn render_table(locale: Locale, segments: &[WalSegment]) -> Html {
    let rows = segments.iter().map(|seg| {
        let sha_short = seg.sha256.chars().take(12).collect::<String>();
        let range = if seg.start_time > 0 && seg.end_time > 0 {
            format!("{} → {}", format_time(seg.start_time), format_time(seg.end_time))
        } else {
            "-".to_string()
        };
        html! {
            <tr class="border-b border-neutral-800 text-xs">
                <td class="py-2 font-mono text-neutral-200 font-medium">{&seg.filename}</td>
                <td class="py-2 font-mono text-neutral-400">{format_bytes(seg.size)}</td>
                <td class="py-2 font-mono text-neutral-400 truncate max-w-xs" title={range.clone()}>{range}</td>
                <td class="py-2 font-mono text-neutral-400" title={seg.sha256.clone()}>{sha_short}</td>
                <td class="py-2 text-neutral-400">{format_time(seg.created_at)}</td>
            </tr>
        }
    });

    html! {
        <div class="overflow-x-auto max-h-72 overflow-y-auto rounded-xl border border-neutral-800">
            <table class="w-full text-left">
                <thead class="sticky top-0 bg-neutral-800/90 backdrop-blur-sm text-neutral-400 text-xs border-b border-neutral-700">
                    <tr>
                        <th class="py-2 px-3 font-medium">{locale.t("databases.wal_segment_filename")}</th>
                        <th class="py-2 px-3 font-medium">{locale.t("databases.wal_segment_size")}</th>
                        <th class="py-2 px-3 font-medium">{locale.t("databases.wal_segment_range")}</th>
                        <th class="py-2 px-3 font-medium">{locale.t("databases.wal_segment_sha256")}</th>
                        <th class="py-2 px-3 font-medium">{locale.t("databases.wal_segment_archived_at")}</th>
                    </tr>
                </thead>
                <tbody class="divide-y divide-neutral-800/60 px-3">
                    { for rows }
                </tbody>
            </table>
        </div>
    }
}

fn render_modal_footer(
    locale: Locale,
    on_close: Callback<MouseEvent>,
    can_edit: bool,
    on_archive: Callback<MouseEvent>,
    is_archiving: bool,
) -> Html {
    html! {
        <div class="flex items-center justify-between pt-4 border-t border-neutral-800">
            <div />
            <div class="flex items-center gap-2">
                <button type="button" class={BUTTON_SECONDARY} onclick={on_close}>
                    {locale.t("backups.close")}
                </button>
                if can_edit {
                    <button type="button" class={BUTTON_PRIMARY} onclick={on_archive} disabled={is_archiving}>
                        <svg class={classes!("w-3.5", "h-3.5", is_archiving.then_some("animate-spin"))} fill="none" stroke="currentColor" viewBox="0 0 24 24">
                            <path stroke-linecap="round" stroke-linejoin="round" stroke-width="2" d="M4 4v5h.582m15.356 2A8.001 8.001 0 004.582 9m0 0H9m11 11v-5h-.581m0 0a8.003 8.003 0 01-15.357-2m15.357 2H15" />
                        </svg>
                        if is_archiving {
                            {locale.t("databases.wal_archiving")}
                        } else {
                            {locale.t("databases.wal_archive_now")}
                        }
                    </button>
                }
            </div>
        </div>
    }
}
