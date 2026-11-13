use crate::API_ENDPOINT;
use crate::components::button::{Button, ButtonKind};
use crate::components::data_list::{Column, ListContent, ListOptions, Row, list_view};
use crate::components::modal::{Modal, ModalSize};
use crate::format::{format_bytes, format_time};
use crate::i18n::use_locale;
use crate::pages::settings::{failure_box, fetch_json};
use gloo_net::http::Request;
use r3v3rs3_api::i18n::Locale;
use r3v3rs3_api::id::ShortId;
use r3v3rs3_api::platform::WalSegment;
use wasm_bindgen_futures::spawn_local;
use yew::prelude::*;

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

    let footer = render_modal_footer(
        locale,
        on_close_click,
        props.can_edit,
        on_archive_click,
        *archiving,
    );
    html! {
        <Modal title={locale.t("databases.wal_segments_title")} on_close={props.on_close.clone()} size={ModalSize::Xl} {footer}>
            <div class="space-y-5">
                <p class="text-xs text-neutral-500 dark:text-neutral-400">{locale.t("databases.wal_segments_subtitle")}</p>
                { content }
            </div>
        </Modal>
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
            let result = crate::pages::settings::post_empty(locale, &url).await;
            archiving.set(false);
            if let Err(message) = result {
                error.set(Some(message));
                return;
            }
            if let Some(cb) = on_archived {
                cb.emit(());
            }
            load_segments(locale, db_id, segments, loading, error);
        });
    })
}

fn render_loading(locale: Locale) -> Html {
    html! {
        <div class="p-8 text-center text-neutral-500 dark:text-neutral-400 flex items-center justify-center gap-2">
            <svg class="w-5 h-5 animate-spin" fill="none" stroke="currentColor" viewBox="0 0 24 24">
                <path stroke-linecap="round" stroke-linejoin="round" stroke-width="2" d="M4 4v5h.582m15.356 2A8.001 8.001 0 004.582 9m0 0H9m11 11v-5h-.581m0 0a8.003 8.003 0 01-15.357-2m15.357 2H15" />
            </svg>
            <span>{locale.t("common.loading")}</span>
        </div>
    }
}

fn render_error(locale: Locale, err: &str) -> Html {
    failure_box(&format!("{}: {err}", locale.t("databases.wal_load_error")))
}

fn render_body(locale: Locale, segments: &[WalSegment]) -> Html {
    if segments.is_empty() {
        return html! {
            <div class="p-8 text-center text-neutral-500 dark:text-neutral-400">
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

const KPI: &str = "p-3 rounded-xl bg-neutral-50 border border-neutral-200 dark:bg-neutral-900/50 dark:border-neutral-700";
const KPI_LABEL: &str = "text-xs text-neutral-500 dark:text-neutral-400";

fn render_kpi_summary(locale: Locale, count: usize, total_size: u64, range: &str) -> Html {
    html! {
        <div class="grid grid-cols-1 sm:grid-cols-3 gap-3">
            <div class={KPI}>
                <div class={KPI_LABEL}>{locale.t("databases.wal_total_segments")}</div>
                <div class="text-xl font-bold font-mono text-neutral-900 dark:text-neutral-100 mt-1">{count}</div>
            </div>
            <div class={KPI}>
                <div class={KPI_LABEL}>{locale.t("databases.wal_total_size")}</div>
                <div class="text-xl font-bold font-mono text-cyan-700 dark:text-cyan-400 mt-1">{format_bytes(total_size)}</div>
            </div>
            <div class={KPI}>
                <div class={KPI_LABEL}>{locale.t("databases.wal_segment_range")}</div>
                <div class="text-xs font-mono text-neutral-700 dark:text-neutral-300 mt-1.5 truncate" title={range.to_string()}>{range}</div>
            </div>
        </div>
    }
}

const COLUMNS: [Column; 5] = [
    Column {
        label: "databases.wal_segment_filename",
        class: "font-mono",
    },
    Column {
        label: "databases.wal_segment_size",
        class: "font-mono whitespace-nowrap",
    },
    Column {
        label: "databases.wal_segment_range",
        class: "font-mono",
    },
    Column {
        label: "databases.wal_segment_sha256",
        class: "font-mono",
    },
    Column {
        label: "databases.wal_segment_archived_at",
        class: "whitespace-nowrap",
    },
];

fn render_table(locale: Locale, segments: &[WalSegment]) -> Html {
    let rows = segments
        .iter()
        .map(|seg| {
            let range = if seg.start_time > 0 && seg.end_time > 0 {
                format!("{} → {}", format_time(seg.start_time), format_time(seg.end_time))
            } else {
                "-".to_string()
            };
            Row {
                key: seg.filename.clone(),
                cells: vec![
                    html! { {&seg.filename} },
                    html! { {format_bytes(seg.size)} },
                    html! { {range} },
                    html! { <span title={seg.sha256.clone()}>{seg.sha256.chars().take(12).collect::<String>()}</span> },
                    html! { {format_time(seg.created_at)} },
                ],
                actions: html! {},
            }
        })
        .collect::<Vec<_>>();
    let content = ListContent {
        loaded: true,
        empty: "databases.wal_segments_empty",
        columns: &COLUMNS,
        rows: &rows,
    };
    let options = ListOptions {
        embedded: true,
        compact: true,
        no_actions: true,
        max_height: Some("max-h-72"),
        ..Default::default()
    };
    html! {
        <div class="rounded-md border border-neutral-300 dark:border-neutral-700">
            { list_view(locale, content, None, &options) }
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
        <>
            <Button kind={ButtonKind::Secondary} onclick={on_close}>
                {locale.t("backups.close")}
            </Button>
            if can_edit {
                <Button kind={ButtonKind::Primary} onclick={on_archive} disabled={is_archiving}>
                    <svg class={classes!("w-3.5", "h-3.5", is_archiving.then_some("animate-spin"))} fill="none" stroke="currentColor" viewBox="0 0 24 24">
                        <path stroke-linecap="round" stroke-linejoin="round" stroke-width="2" d="M4 4v5h.582m15.356 2A8.001 8.001 0 004.582 9m0 0H9m11 11v-5h-.581m0 0a8.003 8.003 0 01-15.357-2m15.357 2H15" />
                    </svg>
                    if is_archiving {
                        {locale.t("databases.wal_archiving")}
                    } else {
                        {locale.t("databases.wal_archive_now")}
                    }
                </Button>
            }
        </>
    }
}
