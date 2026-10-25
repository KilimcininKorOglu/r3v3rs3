use crate::API_ENDPOINT;
use crate::auth::use_ensure_auth;
use crate::components::action_button::{ActionIconButton, ActionVariant};
use crate::format::format_duration;
use crate::i18n::use_locale;
use crate::pages::Route;
use gloo_net::http::Request;
use r3v3rs3_api::ct::{
    CtAlertStatus, CtLogEntry, CtMonitorConfig, CtMonitorStatus, DismissCtAlertRequest,
    UpdateCtConfigRequest,
};
use r3v3rs3_api::i18n::Locale;
use wasm_bindgen_futures::spawn_local;
use yew::prelude::*;
use yew_router::prelude::*;

#[derive(Clone)]
struct CtFormHandles {
    enabled: UseStateHandle<bool>,
    auto_discover: UseStateHandle<bool>,
    alert_unknown: UseStateHandle<bool>,
    interval_hours: UseStateHandle<u64>,
    domains: UseStateHandle<String>,
    issuers: UseStateHandle<String>,
}

#[derive(Clone)]
struct CtDataHandles {
    status: UseStateHandle<Option<CtMonitorStatus>>,
    config: UseStateHandle<Option<CtMonitorConfig>>,
    entries: UseStateHandle<Vec<CtLogEntry>>,
    loading: UseStateHandle<bool>,
}

fn create_load_data_callback(data: CtDataHandles, form: CtFormHandles) -> Callback<()> {
    Callback::from(move |_: ()| {
        let d = data.clone();
        let f = form.clone();
        d.loading.set(true);
        spawn_local(async move {
            refresh_all_ct_data(&d, &f).await;
            d.loading.set(false);
        });
    })
}

async fn refresh_all_ct_data(d: &CtDataHandles, f: &CtFormHandles) {
    if let Ok(st) = fetch_ct_status().await {
        d.status.set(Some(st));
    }
    if let Ok(cfg) = fetch_ct_config().await {
        f.enabled.set(cfg.enabled);
        f.auto_discover.set(cfg.auto_discover_domains);
        f.alert_unknown.set(cfg.alert_on_unknown_issuer);
        f.interval_hours.set(cfg.check_interval_secs / 3600);
        f.domains.set(cfg.monitored_domains.join(", "));
        f.issuers.set(cfg.allowed_issuers.join(", "));
        d.config.set(Some(cfg));
    }
    if let Ok(ents) = fetch_ct_entries().await {
        d.entries.set(ents);
    }
}

fn create_check_callback(
    checking: UseStateHandle<bool>,
    reload: Callback<()>,
) -> Callback<MouseEvent> {
    Callback::from(move |_| {
        let chk = checking.clone();
        let rel = reload.clone();
        chk.set(true);
        spawn_local(async move {
            let _ = trigger_ct_check().await;
            chk.set(false);
            rel.emit(());
        });
    })
}

fn create_save_callback(
    form: CtFormHandles,
    show_settings: UseStateHandle<bool>,
    reload: Callback<()>,
) -> Callback<MouseEvent> {
    Callback::from(move |_| {
        let req = build_update_config_request(
            *form.enabled,
            *form.auto_discover,
            *form.alert_unknown,
            *form.interval_hours,
            &form.domains,
            &form.issuers,
        );
        let show = show_settings.clone();
        let rel = reload.clone();
        spawn_local(async move {
            let _ = save_ct_config(req).await;
            show.set(false);
            rel.emit(());
        });
    })
}

fn create_toggle_settings(show_settings: UseStateHandle<bool>) -> Callback<MouseEvent> {
    Callback::from(move |_| show_settings.set(!*show_settings))
}

fn render_drawer_if_visible(
    locale: Locale,
    show: bool,
    form: &CtFormHandles,
    save_cb: Callback<MouseEvent>,
) -> Html {
    if !show {
        return html! {};
    }
    render_settings_drawer(locale, form, save_cb)
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
    let loading = use_state(bool::default);

    let data_handles = CtDataHandles {
        status,
        config,
        entries,
        loading,
    };

    let form_handles = CtFormHandles {
        enabled: use_state(default_true),
        auto_discover: use_state(default_true),
        alert_unknown: use_state(default_true),
        interval_hours: use_state(default_interval),
        domains: use_state(String::default),
        issuers: use_state(String::default),
    };

    let checking = use_state(bool::default);
    let show_settings = use_state(bool::default);
    let rogue_only = use_state(bool::default);
    let search_filter = use_state(String::default);

    let load_data = create_load_data_callback(data_handles.clone(), form_handles.clone());
    {
        let cb = load_data.clone();
        use_effect_with((), move |_| {
            cb.emit(());
        });
    }

    let check_now_onclick = create_check_callback(checking.clone(), load_data.clone());
    let toggle_settings = create_toggle_settings(show_settings.clone());
    let save_settings_onclick = create_save_callback(
        form_handles.clone(),
        show_settings.clone(),
        load_data.clone(),
    );

    let filtered = filter_entries(&data_handles.entries, *rogue_only, &search_filter);
    let drawer =
        render_drawer_if_visible(locale, *show_settings, &form_handles, save_settings_onclick);
    let summary = render_summary_if_present(locale, data_handles.status.as_ref());

    html! {
        <div class="space-y-6">
            { render_tabs(locale) }
            { render_header(locale, *checking, check_now_onclick, toggle_settings) }
            { drawer }
            { summary }
            { render_filter_bar(locale, &rogue_only, &search_filter) }
            { render_entries_table(locale, &filtered, load_data) }
        </div>
    }
}

fn render_tabs(locale: Locale) -> Html {
    let tabs = [
        (Route::Certs, "certs.tab_server"),
        (Route::AcmeCerts, "certs.tab_acme"),
        (Route::CtLogs, "certs.tab_ct_logs"),
        (Route::ClientCerts, "certs.tab_client"),
        (Route::RootCerts, "certs.tab_root"),
        (Route::DnsCredentials, "certs.tab_dns_credentials"),
    ];

    html! {
        <div class="border-b border-neutral-200 dark:border-neutral-800">
            <nav class="flex space-x-6">
                { for tabs.into_iter().map(|(route, label)| {
                    let is_active = route == Route::CtLogs;
                    let active_cls = if is_active {
                        "border-primary-500 text-primary-600 dark:text-primary-400 font-semibold border-b-2"
                    } else {
                        "border-transparent text-neutral-500 hover:text-neutral-700 dark:hover:text-neutral-300"
                    };
                    html! {
                        <Link<Route> to={route} classes={classes!("py-3", "px-1", "text-sm", active_cls)}>
                            { locale.t(label) }
                        </Link<Route>>
                    }
                }) }
            </nav>
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
                <h1 class="text-2xl font-bold tracking-tight text-neutral-900 dark:text-white">
                    { locale.t("certs.ct_title") }
                </h1>
                <p class="text-sm text-neutral-500 dark:text-neutral-400 mt-1">
                    { locale.t("certs.ct_subtitle") }
                </p>
            </div>
            <div class="flex items-center gap-2">
                <button
                    onclick={check_now}
                    disabled={checking}
                    class="inline-flex items-center gap-2 px-4 py-2 text-sm font-medium rounded-lg text-white bg-primary-600 hover:bg-primary-700 focus:ring-4 focus:ring-primary-300 dark:focus:ring-primary-800 disabled:opacity-50"
                >
                    if checking {
                        <div class="w-4 h-4 border-2 border-white border-t-transparent rounded-full animate-spin" />
                        <span>{ locale.t("certs.ct_checking") }</span>
                    } else {
                        <img src="/assets/icons/refresh.svg" class="w-4 h-4 invert dark:invert-0" />
                        <span>{ locale.t("certs.ct_check_now") }</span>
                    }
                </button>
                <button
                    onclick={toggle_settings}
                    class="inline-flex items-center gap-2 px-4 py-2 text-sm font-medium rounded-lg bg-neutral-100 hover:bg-neutral-200 dark:bg-neutral-800 dark:hover:bg-neutral-700 text-neutral-700 dark:text-neutral-200 border border-neutral-300 dark:border-neutral-700"
                >
                    <img src="/assets/icons/create.svg" class="w-4 h-4 text-neutral-500" />
                    <span>{ locale.t("certs.ct_settings") }</span>
                </button>
            </div>
        </div>
    }
}

fn render_summary_cards(locale: Locale, status: &CtMonitorStatus) -> Html {
    let rogues_count = status.active_rogue_alerts_count;
    let rogues_card_cls = if rogues_count > 0 {
        "p-4 rounded-xl border border-red-300 dark:border-red-900 bg-red-50 dark:bg-red-950/40"
    } else {
        "p-4 rounded-xl border border-neutral-200 dark:border-neutral-800 bg-white dark:bg-neutral-900"
    };

    let last_check_text = match status.last_check_at {
        Some(ts) => format_duration(locale, ts),
        None => "-".to_string(),
    };
    let next_check_text = match status.next_check_at {
        Some(ts) => format_duration(locale, ts),
        None => "-".to_string(),
    };

    html! {
        <div class="grid grid-cols-1 sm:grid-cols-2 lg:grid-cols-4 gap-4">
            <div class="p-4 rounded-xl border border-neutral-200 dark:border-neutral-800 bg-white dark:bg-neutral-900">
                <div class="text-xs font-medium text-neutral-500 dark:text-neutral-400">
                    { locale.t("certs.ct_monitored_domains") }
                </div>
                <div class="mt-2 text-2xl font-bold text-neutral-900 dark:text-white">
                    { status.monitored_domains.len() }
                </div>
            </div>

            <div class={rogues_card_cls}>
                <div class="text-xs font-medium text-neutral-500 dark:text-neutral-400">
                    { locale.t("certs.ct_active_rogue_alerts") }
                </div>
                <div class={classes!("mt-2", "text-2xl", "font-bold", if rogues_count > 0 { "text-red-600 dark:text-red-400" } else { "text-neutral-900 dark:text-white" })}>
                    { rogues_count }
                </div>
            </div>

            <div class="p-4 rounded-xl border border-neutral-200 dark:border-neutral-800 bg-white dark:bg-neutral-900">
                <div class="text-xs font-medium text-neutral-500 dark:text-neutral-400">
                    { locale.t("certs.ct_last_check") }
                </div>
                <div class="mt-2 text-sm font-semibold text-neutral-800 dark:text-neutral-200">
                    { last_check_text }
                </div>
                <div class="text-xs text-neutral-400 mt-1 uppercase tracking-wider">
                    { &status.last_check_status }
                </div>
            </div>

            <div class="p-4 rounded-xl border border-neutral-200 dark:border-neutral-800 bg-white dark:bg-neutral-900">
                <div class="text-xs font-medium text-neutral-500 dark:text-neutral-400">
                    { locale.t("certs.ct_next_check") }
                </div>
                <div class="mt-2 text-sm font-semibold text-neutral-800 dark:text-neutral-200">
                    { next_check_text }
                </div>
            </div>
        </div>
    }
}

fn render_settings_drawer(
    locale: Locale,
    form: &CtFormHandles,
    save_cb: Callback<MouseEvent>,
) -> Html {
    let on_toggle_enabled = {
        let enabled = form.enabled.clone();
        Callback::from(move |_| enabled.set(!*enabled))
    };
    let on_toggle_auto = {
        let auto_discover = form.auto_discover.clone();
        Callback::from(move |_| auto_discover.set(!*auto_discover))
    };
    let on_toggle_alert = {
        let alert_unknown = form.alert_unknown.clone();
        Callback::from(move |_| alert_unknown.set(!*alert_unknown))
    };

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

    let on_domains_input = {
        let domains = form.domains.clone();
        Callback::from(move |e: InputEvent| {
            if let Some(target) = e.target_dyn_into::<web_sys::HtmlInputElement>() {
                domains.set(target.value());
            }
        })
    };

    let on_issuers_input = {
        let issuers = form.issuers.clone();
        Callback::from(move |e: InputEvent| {
            if let Some(target) = e.target_dyn_into::<web_sys::HtmlInputElement>() {
                issuers.set(target.value());
            }
        })
    };

    html! {
        <div class="p-5 border border-neutral-200 dark:border-neutral-800 rounded-xl bg-neutral-50 dark:bg-neutral-900/60 space-y-4">
            <h2 class="text-base font-semibold text-neutral-900 dark:text-white">
                { locale.t("certs.ct_settings") }
            </h2>
            <div class="grid grid-cols-1 md:grid-cols-2 gap-4">
                <label class="flex items-center gap-3 cursor-pointer">
                    <input type="checkbox" checked={*form.enabled} onclick={on_toggle_enabled} class="rounded border-neutral-300 dark:border-neutral-700 text-primary-600 focus:ring-primary-500" />
                    <div>
                        <div class="text-sm font-medium text-neutral-800 dark:text-neutral-200">{ locale.t("certs.ct_enable") }</div>
                        <div class="text-xs text-neutral-500 dark:text-neutral-400">{ locale.t("certs.ct_enable_hint") }</div>
                    </div>
                </label>

                <label class="flex items-center gap-3 cursor-pointer">
                    <input type="checkbox" checked={*form.auto_discover} onclick={on_toggle_auto} class="rounded border-neutral-300 dark:border-neutral-700 text-primary-600 focus:ring-primary-500" />
                    <div>
                        <div class="text-sm font-medium text-neutral-800 dark:text-neutral-200">{ locale.t("certs.ct_auto_discover") }</div>
                        <div class="text-xs text-neutral-500 dark:text-neutral-400">{ locale.t("certs.ct_auto_discover_hint") }</div>
                    </div>
                </label>

                <label class="flex items-center gap-3 cursor-pointer">
                    <input type="checkbox" checked={*form.alert_unknown} onclick={on_toggle_alert} class="rounded border-neutral-300 dark:border-neutral-700 text-primary-600 focus:ring-primary-500" />
                    <div>
                        <div class="text-sm font-medium text-neutral-800 dark:text-neutral-200">{ locale.t("certs.ct_alert_unknown") }</div>
                        <div class="text-xs text-neutral-500 dark:text-neutral-400">{ locale.t("certs.ct_alert_unknown_hint") }</div>
                    </div>
                </label>

                <div>
                    <label class="block text-sm font-medium text-neutral-800 dark:text-neutral-200">{ locale.t("certs.ct_check_interval") }</label>
                    <input type="number" min="1" value={form.interval_hours.to_string()} oninput={on_interval_input} class="mt-1 w-full text-sm rounded-lg border border-neutral-300 dark:border-neutral-700 bg-white dark:bg-neutral-800 text-neutral-900 dark:text-white px-3 py-2" />
                </div>
            </div>

            <div>
                <label class="block text-sm font-medium text-neutral-800 dark:text-neutral-200">{ locale.t("certs.ct_custom_domains") }</label>
                <div class="text-xs text-neutral-500 dark:text-neutral-400 mb-1">{ locale.t("certs.ct_custom_domains_hint") }</div>
                <input type="text" value={(*form.domains).clone()} oninput={on_domains_input} placeholder="example.com, test.org" class="w-full text-sm rounded-lg border border-neutral-300 dark:border-neutral-700 bg-white dark:bg-neutral-800 text-neutral-900 dark:text-white px-3 py-2" />
            </div>

            <div>
                <label class="block text-sm font-medium text-neutral-800 dark:text-neutral-200">{ locale.t("certs.ct_allowed_issuers") }</label>
                <div class="text-xs text-neutral-500 dark:text-neutral-400 mb-1">{ locale.t("certs.ct_allowed_issuers_hint") }</div>
                <input type="text" value={(*form.issuers).clone()} oninput={on_issuers_input} placeholder="Let's Encrypt, Google Trust Services, DigiCert" class="w-full text-sm rounded-lg border border-neutral-300 dark:border-neutral-700 bg-white dark:bg-neutral-800 text-neutral-900 dark:text-white px-3 py-2" />
            </div>

            <div class="flex justify-end pt-2">
                <button onclick={save_cb} class="px-4 py-2 text-sm font-medium rounded-lg text-white bg-primary-600 hover:bg-primary-700">
                    { locale.t("certs.ct_save_config") }
                </button>
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
    let on_search_input = {
        let search_filter = search_filter.clone();
        Callback::from(move |e: InputEvent| {
            if let Some(target) = e.target_dyn_into::<web_sys::HtmlInputElement>() {
                search_filter.set(target.value());
            }
        })
    };

    html! {
        <div class="flex flex-col sm:flex-row items-center justify-between gap-3">
            <div class="w-full sm:w-72">
                <input
                    type="text"
                    value={(**search_filter).clone()}
                    oninput={on_search_input}
                    placeholder={locale.t("certs.ct_search_placeholder").to_string()}
                    class="w-full text-sm rounded-lg border border-neutral-300 dark:border-neutral-700 bg-white dark:bg-neutral-800 text-neutral-900 dark:text-white px-3 py-2"
                />
            </div>
            <label class="flex items-center gap-2 cursor-pointer self-start sm:self-center">
                <input type="checkbox" checked={**rogue_only} onclick={on_toggle_rogue} class="rounded border-neutral-300 dark:border-neutral-700 text-red-600 focus:ring-red-500" />
                <span class="text-sm font-medium text-red-600 dark:text-red-400">
                    { locale.t("certs.ct_status_rogue") }
                </span>
            </label>
        </div>
    }
}

fn render_entries_table(locale: Locale, entries: &[CtLogEntry], reload_cb: Callback<()>) -> Html {
    if entries.is_empty() {
        return html! {
            <div class="p-8 text-center border border-neutral-200 dark:border-neutral-800 rounded-xl bg-white dark:bg-neutral-900 text-neutral-500 dark:text-neutral-400 text-sm">
                { locale.t("certs.ct_no_entries") }
            </div>
        };
    }

    html! {
        <div class="overflow-x-auto border border-neutral-200 dark:border-neutral-800 rounded-xl bg-white dark:bg-neutral-900 shadow-sm">
            <table class="w-full text-left text-sm text-neutral-600 dark:text-neutral-300">
                <thead class="text-xs uppercase bg-neutral-50 dark:bg-neutral-800/60 text-neutral-500 dark:text-neutral-400 border-b border-neutral-200 dark:border-neutral-800">
                    <tr>
                        <th class="px-4 py-3">{ "Domain / SANs" }</th>
                        <th class="px-4 py-3">{ locale.t("certs.issuer") }</th>
                        <th class="px-4 py-3">{ locale.t("certs.ct_serial") }</th>
                        <th class="px-4 py-3">{ locale.t("certs.ct_validity") }</th>
                        <th class="px-4 py-3">{ "Status" }</th>
                        <th class="px-4 py-3 text-right">{ locale.t("common.actions") }</th>
                    </tr>
                </thead>
                <tbody class="divide-y divide-neutral-200 dark:divide-neutral-800">
                    { for entries.iter().map(|entry| render_entry_row(locale, entry, reload_cb.clone())) }
                </tbody>
            </table>
        </div>
    }
}

fn render_entry_row(locale: Locale, entry: &CtLogEntry, reload_cb: Callback<()>) -> Html {
    let row_cls = if entry.is_rogue && entry.status != CtAlertStatus::Dismissed {
        "bg-red-50/60 dark:bg-red-950/20 hover:bg-red-50 dark:hover:bg-red-950/30 transition-colors"
    } else {
        "hover:bg-neutral-50 dark:hover:bg-neutral-800/40 transition-colors"
    };

    let not_before = entry
        .not_before
        .map(|ts| format_duration(locale, ts))
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
            spawn_local(async move {
                let msg = locale.tf("certs.ct_confirm_dismiss", &[("domain", &domain)]);
                if crate::dialog::confirm(locale, &msg).await {
                    let _ = dismiss_ct_alert(&entry_id).await;
                    reload_cb.emit(());
                }
            });
        })
    };

    html! {
        <tr class={row_cls}>
            <td class="px-4 py-3">
                <div class="font-medium text-neutral-900 dark:text-white">{ &entry.domain }</div>
                if !entry.san.is_empty() {
                    <div class="text-xs text-neutral-400 truncate max-w-xs">{ entry.san.join(", ") }</div>
                }
            </td>
            <td class="px-4 py-3 max-w-xs">
                <div class="truncate text-xs font-mono" title={entry.issuer_name.clone()}>
                    { &entry.issuer_name }
                </div>
            </td>
            <td class="px-4 py-3 text-xs font-mono text-neutral-500">
                { truncate_serial(&entry.serial_number) }
            </td>
            <td class="px-4 py-3 text-xs">
                <div>{ not_before }</div>
                <div class="text-neutral-400">{"→ "}{ not_after }</div>
            </td>
            <td class="px-4 py-3">
                { render_status_badge(locale, entry) }
            </td>
            <td class="px-4 py-3 text-right">
                if entry.is_rogue && entry.status != CtAlertStatus::Dismissed {
                    <ActionIconButton
                        variant={ActionVariant::Pause}
                        title={locale.t("certs.ct_dismiss").to_string()}
                        onclick={dismiss_onclick}
                    />
                }
            </td>
        </tr>
    }
}

fn render_status_badge(locale: Locale, entry: &CtLogEntry) -> Html {
    if !entry.is_rogue {
        return html! {
            <span class="inline-flex items-center px-2 py-0.5 rounded text-xs font-medium bg-emerald-100 text-emerald-800 dark:bg-emerald-950/60 dark:text-emerald-400">
                { locale.t("certs.ct_status_ok") }
            </span>
        };
    }

    match entry.status {
        CtAlertStatus::Investigating => html! {
            <span class="inline-flex items-center gap-1 px-2 py-0.5 rounded text-xs font-semibold bg-red-100 text-red-800 dark:bg-red-900/60 dark:text-red-300" title={entry.rogue_reason.clone().unwrap_or_default()}>
                <span class="w-1.5 h-1.5 rounded-full bg-red-500 animate-pulse" />
                { locale.t("certs.ct_status_rogue") }
            </span>
        },
        CtAlertStatus::Dismissed => html! {
            <span class="inline-flex items-center px-2 py-0.5 rounded text-xs font-medium bg-neutral-200 text-neutral-700 dark:bg-neutral-800 dark:text-neutral-400">
                { locale.t("certs.ct_status_dismissed") }
            </span>
        },
        CtAlertStatus::ConfirmedRogue => html! {
            <span class="inline-flex items-center px-2 py-0.5 rounded text-xs font-semibold bg-red-700 text-white">
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
            ct_api_url: None,
        },
    }
}

async fn fetch_ct_status() -> Result<CtMonitorStatus, gloo_net::Error> {
    Request::get(&format!("{API_ENDPOINT}/certs/ct/status"))
        .send()
        .await?
        .json()
        .await
}

async fn fetch_ct_config() -> Result<CtMonitorConfig, gloo_net::Error> {
    Request::get(&format!("{API_ENDPOINT}/certs/ct/config"))
        .send()
        .await?
        .json()
        .await
}

async fn fetch_ct_entries() -> Result<Vec<CtLogEntry>, gloo_net::Error> {
    Request::get(&format!("{API_ENDPOINT}/certs/ct/entries"))
        .send()
        .await?
        .json()
        .await
}

async fn save_ct_config(req: UpdateCtConfigRequest) -> Result<(), gloo_net::Error> {
    Request::put(&format!("{API_ENDPOINT}/certs/ct/config"))
        .json(&req)?
        .send()
        .await?;
    Ok(())
}

async fn trigger_ct_check() -> Result<usize, gloo_net::Error> {
    Request::post(&format!("{API_ENDPOINT}/certs/ct/check"))
        .send()
        .await?
        .json()
        .await
}

async fn dismiss_ct_alert(id: &str) -> Result<(), gloo_net::Error> {
    let body = DismissCtAlertRequest {
        id: id.to_string(),
        status: Some(CtAlertStatus::Dismissed),
    };
    Request::post(&format!("{API_ENDPOINT}/certs/ct/dismiss"))
        .json(&body)?
        .send()
        .await?;
    Ok(())
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
    fn test_truncate_serial() {
        assert_eq!(truncate_serial("123456"), "123456");
        assert_eq!(truncate_serial("12345678901234567890"), "1234567890123456…");
    }
}
