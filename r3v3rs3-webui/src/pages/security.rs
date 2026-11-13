//! Security center consolidating Access Lists, Banned IPs, Quarantined IPs, and Anomaly Detection Rules.

use super::access_lists::AccessLists;
use super::resource_page::{FormCard, add_button, form_modal};
use super::settings::{Notice, fetch_json, notice_view, send_request};
use crate::API_ENDPOINT;
use crate::auth::use_ensure_auth;
use crate::components::action_button::{ActionIconButton, ActionVariant};
use crate::components::anomaly_rules::AnomalyRulesCard;
use crate::components::data_list::{Column, ListContent, ListOptions, Row, list_view};
use crate::components::geoip_card::GeoIpCard;
use crate::components::tabs::{TabItem, Tabs};
use crate::dialog;
use crate::i18n::use_locale;
use gloo_net::http::Request;
use r3v3rs3_api::auth::BannedIpEntry;
use r3v3rs3_api::i18n::Locale;
use r3v3rs3_api::quarantine::QuarantinedIpEntry;
use serde_json::json;
use web_sys::HtmlInputElement;
use yew::prelude::*;

const TABLE_FRAME: &str = "mt-4 rounded-md border border-neutral-300 dark:border-neutral-700";
const INPUT_CLASS: &str = "w-full bg-neutral-50 border border-neutral-300 text-neutral-900 text-sm rounded-lg focus:ring-primary-500 focus:border-primary-500 p-2.5 dark:bg-neutral-700 dark:border-neutral-600 dark:placeholder-neutral-400 dark:text-white";

const BANNED_COLUMNS: [Column; 3] = [
    Column {
        label: "settings.banned_ips",
        class: "font-mono",
    },
    Column {
        label: "settings.failures_count",
        class: "",
    },
    Column {
        label: "settings.remaining_time",
        class: "font-mono whitespace-nowrap",
    },
];

const QUARANTINE_COLUMNS: [Column; 4] = [
    Column {
        label: "settings.quarantined_ips",
        class: "font-mono",
    },
    Column {
        label: "settings.quarantine_reason",
        class: "",
    },
    Column {
        label: "settings.quarantine_violations",
        class: "font-mono",
    },
    Column {
        label: "settings.remaining_time",
        class: "font-mono whitespace-nowrap",
    },
];

/// The active security section tab.
#[derive(Clone, Copy, PartialEq, Eq, Default)]
pub enum SecurityTab {
    #[default]
    AccessLists,
    BannedIps,
    QuarantinedIps,
    AnomalyRules,
    GeoIp,
}

#[function_component(Security)]
pub fn security() -> Html {
    use_ensure_auth();
    let locale = use_locale();
    let active_tab = use_state(SecurityTab::default);

    html! {
        <div class="space-y-6">
            { render_header(locale) }
            { render_tabs(locale, &active_tab) }

            { match *active_tab {
                SecurityTab::AccessLists => html! {
                    <AccessLists />
                },
                SecurityTab::BannedIps => html! {
                    <BannedIpsCard />
                },
                SecurityTab::QuarantinedIps => html! {
                    <QuarantinedIpsCard />
                },
                SecurityTab::AnomalyRules => html! {
                    <AnomalyRulesCard />
                },
                SecurityTab::GeoIp => html! {
                    <GeoIpCard />
                },
            } }
        </div>
    }
}

fn render_header(locale: Locale) -> Html {
    html! {
        <div>
            <h1 class="text-2xl font-bold tracking-tight text-neutral-900 dark:text-neutral-100">
                { locale.t("security.title") }
            </h1>
            <p class="mt-1 text-sm text-neutral-500 dark:text-neutral-400">
                { locale.t("security.subtitle") }
            </p>
        </div>
    }
}

fn render_tabs(locale: Locale, active_tab: &UseStateHandle<SecurityTab>) -> Html {
    let tabs = [
        (SecurityTab::AccessLists, "security.tab_access_lists"),
        (SecurityTab::BannedIps, "security.tab_banned_ips"),
        (SecurityTab::QuarantinedIps, "security.tab_quarantined_ips"),
        (SecurityTab::AnomalyRules, "security.tab_anomaly_rules"),
        (SecurityTab::GeoIp, "security.tab_geoip"),
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

/// Minutes and seconds of a remaining ban time.
fn remaining_text(seconds: u64) -> String {
    format!("{}m {}s", seconds / 60, seconds % 60)
}

#[function_component(BannedIpsCard)]
fn banned_ips_card() -> Html {
    let locale = use_locale();
    let reload = use_state(|| 0);
    let list = use_state(Vec::<BannedIpEntry>::new);
    let notice = use_state(|| Option::<Notice>::None);

    let reload_val = *reload;
    let list_handle = list.clone();
    use_effect_with(reload_val, move |_| {
        wasm_bindgen_futures::spawn_local(async move {
            if let Ok(entries) = fetch_json::<Vec<BannedIpEntry>>(
                locale,
                Request::get(&format!("{API_ENDPOINT}/banned-ips")),
            )
            .await
            {
                list_handle.set(entries);
            }
        });
    });

    let unban = {
        let reload = reload.clone();
        let notice = notice.clone();
        Callback::from(move |ip: String| {
            let reload = reload.clone();
            let notice = notice.clone();
            let question = locale.tf("settings.unban_confirm", &[("ip", &ip)]);
            dialog::confirm_then(locale, question, async move {
                let req = Request::delete(&format!("{API_ENDPOINT}/banned-ips/{ip}"));
                if let Ok(built) = req.build() {
                    match send_request(locale, built).await {
                        Ok(_) => {
                            notice.set(Some(Notice::Success(
                                locale.t("settings.unbanned").to_string(),
                            )));
                            reload.set(*reload + 1);
                        }
                        Err(err) => notice.set(Some(Notice::Failed(err))),
                    }
                }
            });
        })
    };

    html! {
        <div class="bg-white dark:bg-neutral-800 shadow-sm p-5 border border-neutral-300 dark:border-neutral-700 rounded-md">
            { notice_view(&notice) }
            <h2 class="text-lg font-semibold text-neutral-900 dark:text-neutral-200">{locale.t("settings.banned_ips")}</h2>
            <p class="mt-2 text-sm text-neutral-500 dark:text-neutral-400">{locale.t("settings.banned_ips_hint")}</p>
            <div class={TABLE_FRAME}>
                { banned_table_view(locale, &list, unban) }
            </div>
        </div>
    }
}

fn banned_table_view(locale: Locale, list: &[BannedIpEntry], unban: Callback<String>) -> Html {
    let rows = list
        .iter()
        .map(|entry| {
            let ip = entry.ip.to_string();
            let on_unban = {
                let (unban, ip) = (unban.clone(), ip.clone());
                Callback::from(move |e: MouseEvent| {
                    e.prevent_default();
                    unban.emit(ip.clone());
                })
            };
            Row {
                key: ip.clone(),
                cells: vec![
                    html! { {ip} },
                    html! { {entry.failures} },
                    html! { {remaining_text(entry.remaining_seconds)} },
                ],
                actions: html! {
                    <ActionIconButton
                        variant={ActionVariant::Delete}
                        title={locale.t("settings.unban").to_string()}
                        onclick={on_unban}
                    />
                },
            }
        })
        .collect::<Vec<_>>();
    let content = ListContent {
        loaded: true,
        empty: "settings.banned_ips_empty",
        columns: &BANNED_COLUMNS,
        rows: &rows,
    };
    list_view(locale, content, None, &table_options())
}

/// The options of the tables inside the cards: the address column keeps its untranslated name.
fn table_options() -> ListOptions {
    ListOptions {
        embedded: true,
        headings: Some(vec!["IP".to_string()]),
        ..Default::default()
    }
}

#[function_component(QuarantinedIpsCard)]
fn quarantined_ips_card() -> Html {
    let locale = use_locale();
    let reload = use_state(|| 0);
    let list = use_state(Vec::<QuarantinedIpEntry>::new);
    let notice = use_state(|| Option::<Notice>::None);
    let form = use_quarantine_form();

    let reload_val = *reload;
    let list_handle = list.clone();
    use_effect_with(reload_val, move |_| {
        wasm_bindgen_futures::spawn_local(async move {
            if let Ok(entries) = fetch_json::<Vec<QuarantinedIpEntry>>(
                locale,
                Request::get(&format!("{API_ENDPOINT}/security/quarantine")),
            )
            .await
            {
                list_handle.set(entries);
            }
        });
    });

    let unquarantine = {
        let reload = reload.clone();
        let notice = notice.clone();
        Callback::from(move |ip: String| {
            let reload = reload.clone();
            let notice = notice.clone();
            let question = locale.tf("settings.unquarantine_confirm", &[("ip", &ip)]);
            dialog::confirm_then(locale, question, async move {
                let req = Request::delete(&format!("{API_ENDPOINT}/security/quarantine/{ip}"));
                if let Ok(built) = req.build() {
                    match send_request(locale, built).await {
                        Ok(_) => {
                            notice.set(Some(Notice::Success(
                                locale.t("settings.unquarantined").to_string(),
                            )));
                            reload.set(*reload + 1);
                        }
                        Err(err) => notice.set(Some(Notice::Failed(err))),
                    }
                }
            });
        })
    };

    let on_manual_submit = {
        let reload = reload.clone();
        let notice = notice.clone();
        let form = form.clone();
        Callback::from(move |e: SubmitEvent| {
            e.prevent_default();
            let ip_str = (*form.ip).trim().to_string();
            if ip_str.is_empty() {
                return;
            }
            let reason_str = (*form.reason).trim().to_string();
            let reason = if reason_str.is_empty() {
                None
            } else {
                Some(reason_str)
            };
            wasm_bindgen_futures::spawn_local(submit_manual_quarantine(
                locale,
                ip_str,
                reason,
                notice.clone(),
                reload.clone(),
                form.clone(),
            ));
        })
    };
    let on_open = {
        let form = form.clone();
        Callback::from(move |_: MouseEvent| {
            form.error.set(None);
            form.open.set(true);
        })
    };

    html! {
        <div class="bg-white dark:bg-neutral-800 shadow-sm p-5 border border-neutral-300 dark:border-neutral-700 rounded-md">
            { notice_view(&notice) }
            <div class="flex items-center justify-between mb-2">
                <div>
                    <h2 class="text-lg font-semibold text-neutral-900 dark:text-neutral-200">{locale.t("settings.quarantined_ips")}</h2>
                    <p class="mt-1 text-sm text-neutral-500 dark:text-neutral-400">{locale.t("settings.quarantined_ips_hint")}</p>
                </div>
                <span class="inline-flex items-center px-2.5 py-0.5 rounded-full text-xs font-medium bg-red-100 text-red-800 dark:bg-red-900/50 dark:text-red-300">
                    {locale.t("settings.auto_quarantine_active")}
                </span>
            </div>

            { quarantined_table_view(locale, &list, unquarantine) }
            { add_button(locale, on_open) }
            { quarantine_modal(locale, &form, on_manual_submit) }
        </div>
    }
}

/// The state of the form that quarantines an address by hand.
#[derive(Clone)]
struct QuarantineForm {
    open: UseStateHandle<bool>,
    ip: UseStateHandle<String>,
    reason: UseStateHandle<String>,
    /// The error of the last submit, shown in the modal.
    error: UseStateHandle<Option<String>>,
}

#[hook]
fn use_quarantine_form() -> QuarantineForm {
    QuarantineForm {
        open: use_state(|| false),
        ip: use_state(String::new),
        reason: use_state(String::new),
        error: use_state(|| None),
    }
}

fn text_field(label: String, state: &UseStateHandle<String>, placeholder: String) -> Html {
    let oninput = {
        let state = state.clone();
        Callback::from(move |e: InputEvent| {
            if let Some(target) = e.target_dyn_into::<HtmlInputElement>() {
                state.set(target.value());
            }
        })
    };
    html! {
        <label class="block mt-4">
            <span class="block mb-2 text-sm font-medium text-neutral-900 dark:text-white">{label}</span>
            <input type="text" value={(**state).clone()} {oninput} {placeholder} class={INPUT_CLASS} />
        </label>
    }
}

fn quarantine_modal(
    locale: Locale,
    form: &QuarantineForm,
    onsubmit: Callback<SubmitEvent>,
) -> Html {
    if !*form.open {
        return html! {};
    }
    let on_close = {
        let open = form.open.clone();
        Callback::from(move |()| open.set(false))
    };
    let fields = html! {
        <>
            { text_field("IP".to_string(), &form.ip, locale.t("settings.quarantine_ip_placeholder").to_string()) }
            { text_field(locale.t("settings.quarantine_reason").to_string(), &form.reason, locale.t("settings.quarantine_reason_placeholder").to_string()) }
        </>
    };
    let card = FormCard {
        title: locale.t("settings.quarantine_submit").to_string(),
        notice: html! {},
        fields,
        error: (*form.error).clone(),
        cancel: None,
        save_label: "settings.quarantine_submit",
        can_save: true,
    };
    form_modal(locale, card, onsubmit, on_close)
}

async fn submit_manual_quarantine(
    locale: Locale,
    ip_str: String,
    reason: Option<String>,
    notice: UseStateHandle<Option<Notice>>,
    reload: UseStateHandle<usize>,
    form: QuarantineForm,
) {
    let body = json!({
        "ip": ip_str,
        "reason": reason,
        "duration_seconds": 3600
    });
    if let Ok(req) = Request::post(&format!("{API_ENDPOINT}/security/quarantine")).json(&body) {
        match send_request(locale, req).await {
            Ok(_) => {
                notice.set(Some(Notice::Success(
                    locale.t("settings.quarantined_success").to_string(),
                )));
                form.ip.set(String::new());
                form.reason.set(String::new());
                form.open.set(false);
                reload.set(*reload + 1);
            }
            Err(err) => form.error.set(Some(err)),
        }
    }
}

fn quarantined_table_view(
    locale: Locale,
    list: &[QuarantinedIpEntry],
    unquarantine: Callback<String>,
) -> Html {
    let rows = list
        .iter()
        .map(|entry| {
            let ip = entry.ip.to_string();
            let on_release = {
                let (unquarantine, ip) = (unquarantine.clone(), ip.clone());
                Callback::from(move |_| unquarantine.emit(ip.clone()))
            };
            Row {
                key: ip.clone(),
                cells: vec![
                    html! { {ip} },
                    html! { {&entry.reason} },
                    html! { {entry.violations} },
                    html! { {remaining_text(entry.remaining_seconds)} },
                ],
                actions: html! {
                    <ActionIconButton
                        variant={ActionVariant::Delete}
                        title={locale.t("settings.unquarantine").to_string()}
                        onclick={on_release}
                    />
                },
            }
        })
        .collect::<Vec<_>>();
    let content = ListContent {
        loaded: true,
        empty: "settings.quarantined_ips_empty",
        columns: &QUARANTINE_COLUMNS,
        rows: &rows,
    };
    html! {
        <div class={TABLE_FRAME}>
            { list_view(locale, content, None, &table_options()) }
        </div>
    }
}
