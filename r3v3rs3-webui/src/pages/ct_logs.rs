use super::resource_page::{FormCard, form_modal};
use super::settings::{failure_box, fetch_json, post_empty, send_json};
use crate::API_ENDPOINT;
use crate::auth::use_ensure_auth;
use crate::components::action_button::{ActionIconButton, ActionVariant};
use crate::components::button::{Button, ButtonKind};
use crate::components::data_list::{Column, ListContent, ListOptions, Row, list_view};
use crate::dialog::{self, Icon};
use crate::format::{format_duration, format_unix_time};
use crate::i18n::use_locale;
use gloo_net::http::Request;
use r3v3rs3_api::ct::{
    CtAlertStatus, CtLogEntry, CtMonitorConfig, CtMonitorStatus, DismissCtAlertRequest,
    UpdateCtConfigRequest,
};
use r3v3rs3_api::i18n::Locale;
use std::collections::HashMap;
use wasm_bindgen_futures::spawn_local;
use yew::prelude::*;

const INPUT_CLASS: &str = "w-full rounded-lg border border-neutral-300 bg-neutral-50 text-neutral-900 dark:border-neutral-600 dark:bg-neutral-700 dark:text-white px-3 py-2 text-sm focus:ring-primary-500 focus:border-primary-500";
const CHECKBOX_CLASS: &str =
    "rounded border-neutral-300 dark:border-neutral-600 text-primary-600 focus:ring-primary-500";
const TILE: &str = "p-4 rounded-md border border-neutral-300 bg-white shadow-sm dark:border-neutral-700 dark:bg-neutral-800";
const TILE_ALERT: &str = "p-4 rounded-md border border-red-300 bg-red-50 shadow-sm dark:border-red-900 dark:bg-red-950/40";
const TILE_LABEL: &str = "text-xs font-medium text-neutral-500 dark:text-neutral-400";
const FIELD_LABEL: &str = "block text-sm font-medium text-neutral-800 dark:text-neutral-200";
const HINT: &str = "text-xs text-neutral-500 dark:text-neutral-400";
const BADGE: &str = "inline-flex items-center gap-1 px-2 py-0.5 rounded text-xs font-medium";
const ROGUE_ROW: &str = "bg-red-50/60 dark:bg-red-950/20";

const COLUMNS: [Column; 5] = [
    Column {
        label: "certs.ct_domain_sans",
        class: "",
    },
    Column {
        label: "certs.issuer",
        class: "max-w-xs",
    },
    Column {
        label: "certs.ct_serial",
        class: "text-xs font-mono",
    },
    Column {
        label: "certs.ct_validity",
        class: "text-xs whitespace-nowrap",
    },
    Column {
        label: "common.status",
        class: "",
    },
];

#[derive(Clone)]
struct CtFormHandles {
    enabled: UseStateHandle<bool>,
    auto_discover: UseStateHandle<bool>,
    alert_unknown: UseStateHandle<bool>,
    interval_hours: UseStateHandle<u64>,
    domains: UseStateHandle<String>,
    issuers: UseStateHandle<String>,
    /// The error of the last save, shown in the settings dialog.
    save_error: UseStateHandle<Option<String>>,
}

#[derive(Clone)]
struct CtDataHandles {
    status: UseStateHandle<Option<CtMonitorStatus>>,
    config: UseStateHandle<Option<CtMonitorConfig>>,
    entries: UseStateHandle<Vec<CtLogEntry>>,
    loading: UseStateHandle<bool>,
    /// The error of the last load.
    error: UseStateHandle<Option<String>>,
}

fn create_load_data_callback(
    locale: Locale,
    data: CtDataHandles,
    form: CtFormHandles,
) -> Callback<()> {
    Callback::from(move |_: ()| {
        let d = data.clone();
        let f = form.clone();
        d.loading.set(true);
        spawn_local(async move {
            let loaded = refresh_all_ct_data(locale, &d, &f).await;
            d.error.set(loaded.err());
            d.loading.set(false);
        });
    })
}

fn ct_get(path: &str) -> gloo_net::http::RequestBuilder {
    Request::get(&format!("{API_ENDPOINT}/certs/ct/{path}"))
}

async fn refresh_all_ct_data(
    locale: Locale,
    d: &CtDataHandles,
    f: &CtFormHandles,
) -> Result<(), String> {
    d.status
        .set(Some(fetch_json(locale, ct_get("status")).await?));
    let cfg: CtMonitorConfig = fetch_json(locale, ct_get("config")).await?;
    f.enabled.set(cfg.enabled);
    f.auto_discover.set(cfg.auto_discover_domains);
    f.alert_unknown.set(cfg.alert_on_unknown_issuer);
    f.interval_hours.set(cfg.check_interval_secs / 3600);
    f.domains.set(cfg.monitored_domains.join(", "));
    f.issuers.set(cfg.allowed_issuers.join(", "));
    d.config.set(Some(cfg));
    d.entries.set(fetch_json(locale, ct_get("entries")).await?);
    Ok(())
}

fn create_check_callback(
    locale: Locale,
    checking: UseStateHandle<bool>,
    reload: Callback<()>,
) -> Callback<MouseEvent> {
    Callback::from(move |_| {
        let chk = checking.clone();
        let rel = reload.clone();
        chk.set(true);
        spawn_local(async move {
            let checked = post_empty(locale, &format!("{API_ENDPOINT}/certs/ct/check")).await;
            chk.set(false);
            rel.emit(());
            match checked {
                Ok(()) => {
                    dialog::message(locale, locale.t("certs.ct_check_started"), Icon::Success)
                        .await;
                }
                Err(err) => dialog::message(locale, &err, Icon::Error).await,
            }
        });
    })
}

fn create_save_callback(
    locale: Locale,
    form: CtFormHandles,
    config: UseStateHandle<Option<CtMonitorConfig>>,
    show_settings: UseStateHandle<bool>,
    reload: Callback<()>,
) -> Callback<SubmitEvent> {
    Callback::from(move |event: SubmitEvent| {
        event.prevent_default();
        // The form has no field for the API URL, and the server stores the whole object, so
        // the loaded URL goes back unchanged.
        let req = build_update_config_request(
            *form.enabled,
            *form.auto_discover,
            *form.alert_unknown,
            *form.interval_hours,
            &form.domains,
            &form.issuers,
            config.as_ref().and_then(|config| config.ct_api_url.clone()),
        );
        let show = show_settings.clone();
        let rel = reload.clone();
        let save_error = form.save_error.clone();
        spawn_local(async move {
            let builder = Request::put(&format!("{API_ENDPOINT}/certs/ct/config"));
            match send_json(locale, builder, &req).await {
                Ok(()) => {
                    show.set(false);
                    rel.emit(());
                    dialog::message(locale, locale.t("certs.ct_config_saved"), Icon::Success).await;
                }
                Err(err) => save_error.set(Some(err)),
            }
        });
    })
}

fn create_toggle_settings(
    show_settings: UseStateHandle<bool>,
    save_error: UseStateHandle<Option<String>>,
) -> Callback<MouseEvent> {
    Callback::from(move |_| {
        save_error.set(None);
        show_settings.set(!*show_settings);
    })
}

fn render_settings_if_visible(
    locale: Locale,
    show_settings: &UseStateHandle<bool>,
    form: &CtFormHandles,
    onsubmit: Callback<SubmitEvent>,
) -> Html {
    if !**show_settings {
        return html! {};
    }
    let on_close = {
        let show_settings = show_settings.clone();
        Callback::from(move |()| show_settings.set(false))
    };
    let card = FormCard {
        title: locale.t("certs.ct_settings").to_string(),
        notice: html! {},
        fields: render_settings_fields(locale, form),
        error: (*form.save_error).clone(),
        cancel: None,
        save_label: "certs.ct_save_config",
        can_save: true,
    };
    form_modal(locale, card, onsubmit, on_close)
}

fn render_summary_if_present(locale: Locale, status: Option<&CtMonitorStatus>) -> Html {
    match status {
        Some(st) => render_summary_cards(locale, st),
        None => html! {},
    }
}

fn default_true() -> bool {
    true
}

fn default_interval() -> u64 {
    6
}

fn filter_entries(entries: &[CtLogEntry], rogue_only: bool, search: &str) -> Vec<CtLogEntry> {
    entries
        .iter()
        .filter(|e| filter_entry_view(e, rogue_only, search))
        .cloned()
        .collect()
}

#[function_component(CtLogs)]
pub fn ct_logs() -> Html {
    use_ensure_auth();
    let locale = use_locale();

    let status = use_state(Option::default);
    let config = use_state(Option::default);
    let entries = use_state(Vec::default);
    let loading = use_state(|| true);

    let data_handles = CtDataHandles {
        status,
        config,
        entries,
        loading,
        error: use_state(Option::default),
    };

    let form_handles = CtFormHandles {
        enabled: use_state(default_true),
        auto_discover: use_state(default_true),
        alert_unknown: use_state(default_true),
        interval_hours: use_state(default_interval),
        domains: use_state(String::default),
        issuers: use_state(String::default),
        save_error: use_state(Option::default),
    };

    let checking = use_state(bool::default);
    let show_settings = use_state(bool::default);
    let rogue_only = use_state(bool::default);
    let search_filter = use_state(String::default);

    let load_data = create_load_data_callback(locale, data_handles.clone(), form_handles.clone());
    {
        let cb = load_data.clone();
        use_effect_with((), move |_| {
            cb.emit(());
        });
    }

    let check_now_onclick = create_check_callback(locale, checking.clone(), load_data.clone());
    let toggle_settings =
        create_toggle_settings(show_settings.clone(), form_handles.save_error.clone());
    let save_settings = create_save_callback(
        locale,
        form_handles.clone(),
        data_handles.config.clone(),
        show_settings.clone(),
        load_data.clone(),
    );

    let filtered = filter_entries(&data_handles.entries, *rogue_only, &search_filter);
    let settings = render_settings_if_visible(locale, &show_settings, &form_handles, save_settings);
    let summary = render_summary_if_present(locale, data_handles.status.as_ref());

    html! {
        <div class="space-y-6">
            { render_header(locale, *checking, check_now_onclick, toggle_settings) }
            { settings }
            { data_handles.error.as_deref().map(failure_box).unwrap_or_default() }
            { summary }
            { render_filter_bar(locale, &rogue_only, &search_filter) }
            { render_entries_table(locale, &filtered, !*data_handles.loading, load_data) }
        </div>
    }
}

fn render_header(
    locale: Locale,
    checking: bool,
    check_now: Callback<MouseEvent>,
    toggle_settings: Callback<MouseEvent>,
) -> Html {
    html! {
        <div class="flex flex-col sm:flex-row sm:items-center sm:justify-between gap-4">
            <div>
                <h1 class="text-2xl font-semibold tracking-tight text-neutral-900 dark:text-neutral-100">
                    { locale.t("certs.ct_title") }
                </h1>
                <p class="text-sm text-neutral-500 dark:text-neutral-400 mt-1">
                    { locale.t("certs.ct_subtitle") }
                </p>
            </div>
            <div class="flex items-center gap-2">
                <Button kind={ButtonKind::Secondary} onclick={toggle_settings}>
                    <svg class="w-4 h-4" fill="none" stroke="currentColor" viewBox="0 0 24 24" aria-hidden="true">
                        <path stroke-linecap="round" stroke-linejoin="round" stroke-width="2" d="M10.325 4.317c.426-1.756 2.924-1.756 3.35 0a1.724 1.724 0 002.573 1.066c1.543-.94 3.31.826 2.37 2.37a1.724 1.724 0 001.065 2.572c1.756.426 1.756 2.924 0 3.35a1.724 1.724 0 00-1.066 2.573c.94 1.543-.826 3.31-2.37 2.37a1.724 1.724 0 00-2.572 1.065c-.426 1.756-2.924 1.756-3.35 0a1.724 1.724 0 00-2.573-1.066c-1.543.94-3.31-.826-2.37-2.37a1.724 1.724 0 00-1.065-2.572c-1.756-.426-1.756-2.924 0-3.35a1.724 1.724 0 001.066-2.573c-.94-1.543.826-3.31 2.37-2.37.996.608 2.296.07 2.572-1.065z" />
                        <path stroke-linecap="round" stroke-linejoin="round" stroke-width="2" d="M15 12a3 3 0 11-6 0 3 3 0 016 0z" />
                    </svg>
                    { locale.t("certs.ct_settings") }
                </Button>
                <Button kind={ButtonKind::Primary} onclick={check_now} disabled={checking}>
                    if checking {
                        <span class="w-4 h-4 border-2 border-white border-t-transparent rounded-full animate-spin" />
                        { locale.t("certs.ct_checking") }
                    } else {
                        <svg class="w-4 h-4" fill="none" stroke="currentColor" viewBox="0 0 24 24" aria-hidden="true">
                            <path stroke-linecap="round" stroke-linejoin="round" stroke-width="2" d="M4 4v5h.582m15.356 2A8.001 8.001 0 004.582 9m0 0H9m11 11v-5h-.581m0 0a8.003 8.003 0 01-15.357-2m15.357 2H15" />
                        </svg>
                        { locale.t("certs.ct_check_now") }
                    }
                </Button>
            </div>
        </div>
    }
}

/// The translated name of the state of the last check. A state that this panel does not know
/// stays as the server sent it.
fn check_status_text(locale: Locale, status: &str) -> String {
    let key = match status {
        "idle" => "certs.ct_check_status_idle",
        "checking" => "certs.ct_check_status_checking",
        "ok" => "certs.ct_check_status_ok",
        "partial_error" => "certs.ct_check_status_partial_error",
        other => return other.to_string(),
    };
    locale.t(key).to_string()
}

fn render_summary_cards(locale: Locale, status: &CtMonitorStatus) -> Html {
    let rogues_count = status.active_rogue_alerts_count;
    let (rogues_tile, rogues_value) = if rogues_count > 0 {
        (TILE_ALERT, "text-red-600 dark:text-red-400")
    } else {
        (TILE, "text-neutral-900 dark:text-neutral-100")
    };

    let last_check_text = match status.last_check_at {
        Some(ts) => format_unix_time(locale, ts),
        None => "-".to_string(),
    };
    let next_check_text = match status.next_check_at {
        Some(ts) => format_unix_time(locale, ts),
        None => "-".to_string(),
    };

    html! {
        <div class="grid grid-cols-1 sm:grid-cols-2 lg:grid-cols-4 gap-4">
            <div class={TILE}>
                <div class={TILE_LABEL}>{ locale.t("certs.ct_monitored_domains") }</div>
                <div class="mt-2 text-2xl font-bold text-neutral-900 dark:text-neutral-100">
                    { status.monitored_domains.len() }
                </div>
            </div>

            <div class={rogues_tile}>
                <div class={TILE_LABEL}>{ locale.t("certs.ct_active_rogue_alerts") }</div>
                <div class={classes!("mt-2", "text-2xl", "font-bold", rogues_value)}>
                    { rogues_count }
                </div>
            </div>

            <div class={TILE}>
                <div class={TILE_LABEL}>{ locale.t("certs.ct_last_check") }</div>
                <div class="mt-2 text-sm font-semibold text-neutral-900 dark:text-neutral-100">
                    { last_check_text }
                </div>
                <div class="text-xs text-neutral-500 dark:text-neutral-400 mt-1 uppercase tracking-wider">
                    { check_status_text(locale, &status.last_check_status) }
                </div>
            </div>

            <div class={TILE}>
                <div class={TILE_LABEL}>{ locale.t("certs.ct_next_check") }</div>
                <div class="mt-2 text-sm font-semibold text-neutral-900 dark:text-neutral-100">
                    { next_check_text }
                </div>
            </div>
        </div>
    }
}

fn settings_checkbox(
    locale: Locale,
    state: &UseStateHandle<bool>,
    label: &'static str,
    hint: &'static str,
) -> Html {
    let onclick = {
        let state = state.clone();
        Callback::from(move |_: MouseEvent| state.set(!*state))
    };
    html! {
        <label class="flex items-center gap-3 cursor-pointer">
            <input type="checkbox" checked={**state} {onclick} class={CHECKBOX_CLASS} />
            <div>
                <div class="text-sm font-medium text-neutral-800 dark:text-neutral-200">{ locale.t(label) }</div>
                <div class={HINT}>{ locale.t(hint) }</div>
            </div>
        </label>
    }
}

fn text_oninput(state: &UseStateHandle<String>) -> Callback<InputEvent> {
    let state = state.clone();
    Callback::from(move |e: InputEvent| {
        if let Some(target) = e.target_dyn_into::<web_sys::HtmlInputElement>() {
            state.set(target.value());
        }
    })
}

fn render_settings_fields(locale: Locale, form: &CtFormHandles) -> Html {
    let on_interval_input = {
        let interval = form.interval_hours.clone();
        Callback::from(move |e: InputEvent| {
            if let Some(target) = e.target_dyn_into::<web_sys::HtmlInputElement>()
                && let Ok(val) = target.value().parse::<u64>()
            {
                interval.set(val.max(1));
            }
        })
    };

    html! {
        <div class="space-y-4">
            <div class="grid grid-cols-1 md:grid-cols-2 gap-4">
                { settings_checkbox(locale, &form.enabled, "certs.ct_enable", "certs.ct_enable_hint") }
                { settings_checkbox(locale, &form.auto_discover, "certs.ct_auto_discover", "certs.ct_auto_discover_hint") }
                { settings_checkbox(locale, &form.alert_unknown, "certs.ct_alert_unknown", "certs.ct_alert_unknown_hint") }
                <div>
                    <label class={FIELD_LABEL}>{ locale.t("certs.ct_check_interval") }</label>
                    <input type="number" min="1" value={form.interval_hours.to_string()} oninput={on_interval_input} class={classes!(INPUT_CLASS, "mt-1")} />
                </div>
            </div>

            <div>
                <label class={FIELD_LABEL}>{ locale.t("certs.ct_custom_domains") }</label>
                <div class={classes!(HINT, "mb-1")}>{ locale.t("certs.ct_custom_domains_hint") }</div>
                <input type="text" value={(*form.domains).clone()} oninput={text_oninput(&form.domains)} placeholder="example.com, test.org" class={INPUT_CLASS} />
            </div>

            <div>
                <label class={FIELD_LABEL}>{ locale.t("certs.ct_allowed_issuers") }</label>
                <div class={classes!(HINT, "mb-1")}>{ locale.t("certs.ct_allowed_issuers_hint") }</div>
                <input type="text" value={(*form.issuers).clone()} oninput={text_oninput(&form.issuers)} placeholder="Let's Encrypt, Google Trust Services, DigiCert" class={INPUT_CLASS} />
            </div>
        </div>
    }
}

fn render_filter_bar(
    locale: Locale,
    rogue_only: &UseStateHandle<bool>,
    search_filter: &UseStateHandle<String>,
) -> Html {
    let on_toggle_rogue = {
        let rogue_only = rogue_only.clone();
        Callback::from(move |_| rogue_only.set(!*rogue_only))
    };

    html! {
        <div class="flex flex-col sm:flex-row items-center justify-between gap-3">
            <div class="w-full sm:w-72">
                <input
                    type="text"
                    value={(**search_filter).clone()}
                    oninput={text_oninput(search_filter)}
                    placeholder={locale.t("certs.ct_search_placeholder").to_string()}
                    class={INPUT_CLASS}
                />
            </div>
            <label class="flex items-center gap-2 cursor-pointer self-start sm:self-center">
                <input type="checkbox" checked={**rogue_only} onclick={on_toggle_rogue} class="rounded border-neutral-300 dark:border-neutral-600 text-red-600 focus:ring-red-500" />
                <span class="text-sm font-medium text-red-600 dark:text-red-400">
                    { locale.t("certs.ct_status_rogue") }
                </span>
            </label>
        </div>
    }
}

/// An entry that is still an open rogue alert.
fn is_open_alert(entry: &CtLogEntry) -> bool {
    entry.is_rogue && entry.status != CtAlertStatus::Dismissed
}

fn render_entries_table(
    locale: Locale,
    entries: &[CtLogEntry],
    loaded: bool,
    reload_cb: Callback<()>,
) -> Html {
    let rows = entries
        .iter()
        .map(|entry| render_entry_row(locale, entry, reload_cb.clone()))
        .collect::<Vec<_>>();
    let row_class: HashMap<String, &'static str> = entries
        .iter()
        .filter(|entry| is_open_alert(entry))
        .map(|entry| (entry.id.clone(), ROGUE_ROW))
        .collect();
    let content = ListContent {
        loaded,
        empty: "certs.ct_no_entries",
        columns: &COLUMNS,
        rows: &rows,
    };
    let options = ListOptions {
        row_class,
        ..Default::default()
    };
    list_view(locale, content, None, &options)
}

fn render_entry_row(locale: Locale, entry: &CtLogEntry, reload_cb: Callback<()>) -> Row {
    let not_before = entry
        .not_before
        .map(|ts| format_unix_time(locale, ts))
        .unwrap_or_else(|| "-".to_string());
    let not_after = entry
        .not_after
        .map(|ts| format_duration(locale, ts))
        .unwrap_or_else(|| "-".to_string());

    let dismiss_onclick = {
        let entry_id = entry.id.clone();
        let domain = entry.domain.clone();
        Callback::from(move |_| {
            let entry_id = entry_id.clone();
            let domain = domain.clone();
            let reload_cb = reload_cb.clone();
            let msg = locale.tf("certs.ct_confirm_dismiss", &[("domain", &domain)]);
            dialog::confirm_then(locale, msg, async move {
                let body = DismissCtAlertRequest {
                    id: entry_id,
                    status: Some(CtAlertStatus::Dismissed),
                };
                let builder = Request::post(&format!("{API_ENDPOINT}/certs/ct/dismiss"));
                match send_json(locale, builder, &body).await {
                    Ok(()) => reload_cb.emit(()),
                    Err(err) => dialog::message(locale, &err, Icon::Error).await,
                }
            });
        })
    };

    let actions = if is_open_alert(entry) {
        html! {
            <ActionIconButton
                variant={ActionVariant::Pause}
                title={locale.t("certs.ct_dismiss").to_string()}
                onclick={dismiss_onclick}
            />
        }
    } else {
        html! {}
    };

    Row {
        key: entry.id.clone(),
        cells: vec![
            html! {
                <>
                    <div class="font-medium text-neutral-900 dark:text-neutral-100">{ &entry.domain }</div>
                    if !entry.san.is_empty() {
                        <div class="text-xs text-neutral-500 dark:text-neutral-400 truncate max-w-xs">{ entry.san.join(", ") }</div>
                    }
                </>
            },
            html! {
                <div class="truncate text-xs font-mono" title={entry.issuer_name.clone()}>
                    { &entry.issuer_name }
                </div>
            },
            html! { { truncate_serial(&entry.serial_number) } },
            html! {
                <>
                    <div>{ not_before }</div>
                    <div class="text-neutral-500 dark:text-neutral-400">{"→ "}{ not_after }</div>
                </>
            },
            render_status_badge(locale, entry),
        ],
        actions,
    }
}

fn render_status_badge(locale: Locale, entry: &CtLogEntry) -> Html {
    if !entry.is_rogue {
        return html! {
            <span class={classes!(BADGE, "bg-emerald-100", "text-emerald-800", "dark:bg-emerald-950/60", "dark:text-emerald-400")}>
                { locale.t("certs.ct_status_ok") }
            </span>
        };
    }

    match entry.status {
        CtAlertStatus::Investigating => html! {
            <span class={classes!(BADGE, "font-semibold", "bg-red-100", "text-red-800", "dark:bg-red-900/60", "dark:text-red-300")} title={entry.rogue_reason.clone().unwrap_or_default()}>
                <span class="w-1.5 h-1.5 rounded-full bg-red-500 animate-pulse" />
                { locale.t("certs.ct_status_rogue") }
            </span>
        },
        CtAlertStatus::Dismissed => html! {
            <span class={classes!(BADGE, "bg-neutral-200", "text-neutral-700", "dark:bg-neutral-700", "dark:text-neutral-300")}>
                { locale.t("certs.ct_status_dismissed") }
            </span>
        },
        CtAlertStatus::ConfirmedRogue => html! {
            <span class={classes!(BADGE, "font-semibold", "bg-red-700", "text-white")}>
                { locale.t("certs.ct_status_confirmed") }
            </span>
        },
    }
}

fn truncate_serial(serial: &str) -> String {
    if serial.len() > 16 {
        format!("{}…", &serial[..16])
    } else {
        serial.to_string()
    }
}

fn filter_entry_view(entry: &CtLogEntry, rogue_only: bool, search: &str) -> bool {
    if rogue_only && (!entry.is_rogue || entry.status == CtAlertStatus::Dismissed) {
        return false;
    }
    let query = search.trim().to_lowercase();
    if query.is_empty() {
        return true;
    }
    entry.domain.to_lowercase().contains(&query)
        || entry.common_name.to_lowercase().contains(&query)
        || entry.issuer_name.to_lowercase().contains(&query)
        || entry.san.iter().any(|s| s.to_lowercase().contains(&query))
}

fn build_update_config_request(
    enabled: bool,
    auto_discover: bool,
    alert_unknown: bool,
    interval_hours: u64,
    domains_str: &str,
    issuers_str: &str,
    ct_api_url: Option<String>,
) -> UpdateCtConfigRequest {
    let monitored_domains = domains_str
        .split(',')
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
        .collect();

    let allowed_issuers = issuers_str
        .split(',')
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
        .collect();

    UpdateCtConfigRequest {
        config: CtMonitorConfig {
            enabled,
            check_interval_secs: interval_hours.max(1) * 3600,
            monitored_domains,
            auto_discover_domains: auto_discover,
            allowed_issuers,
            alert_on_unknown_issuer: alert_unknown,
            ct_api_url,
        },
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_filter_entry_view() {
        let entry = CtLogEntry {
            id: "1".into(),
            domain: "example.com".into(),
            issuer_name: "Let's Encrypt".into(),
            common_name: "example.com".into(),
            san: vec!["www.example.com".into()],
            serial_number: "1234567890abcdef".into(),
            logged_at: None,
            not_before: None,
            not_after: None,
            is_rogue: false,
            rogue_reason: None,
            status: CtAlertStatus::Investigating,
        };
        assert!(filter_entry_view(&entry, false, "example"));
        assert!(!filter_entry_view(&entry, false, "google"));
        assert!(!filter_entry_view(&entry, true, ""));
    }

    #[test]
    fn a_saved_config_keeps_the_api_url_that_the_form_does_not_show() {
        let url = "https://ct.example.test/?q={domain}".to_string();
        let req = build_update_config_request(
            true,
            false,
            true,
            0,
            " a.example.test, ,b.example.test ",
            "Issuer One",
            Some(url.clone()),
        );
        assert_eq!(req.config.ct_api_url, Some(url));
        assert_eq!(
            req.config.monitored_domains,
            ["a.example.test", "b.example.test"]
        );
        assert_eq!(req.config.check_interval_secs, 3600);
    }

    #[test]
    fn the_check_states_of_the_server_are_translated() {
        assert_eq!(check_status_text(Locale::Tr, "ok"), "Başarılı");
        assert_eq!(
            check_status_text(Locale::En, "partial_error"),
            "Partly failed"
        );
        assert_eq!(check_status_text(Locale::En, "new_state"), "new_state");
    }

    #[test]
    fn test_truncate_serial() {
        assert_eq!(truncate_serial("123456"), "123456");
        assert_eq!(truncate_serial("12345678901234567890"), "1234567890123456…");
    }
}
