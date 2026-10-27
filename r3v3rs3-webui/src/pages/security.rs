//! Security center consolidating Access Lists, Banned IPs, Quarantined IPs, and Anomaly Detection Rules.

use super::access_lists::AccessLists;
use super::settings::{Notice, fetch_json, notice_view, send_request};
use crate::API_ENDPOINT;
use crate::auth::use_ensure_auth;
use crate::components::action_button::{ActionIconButton, ActionVariant};
use crate::components::anomaly_rules::AnomalyRulesCard;
use crate::dialog;
use crate::i18n::use_locale;
use gloo_net::http::Request;
use r3v3rs3_api::auth::BannedIpEntry;
use r3v3rs3_api::i18n::Locale;
use r3v3rs3_api::quarantine::QuarantinedIpEntry;
use serde_json::json;
use web_sys::HtmlInputElement;
use yew::prelude::*;

/// The active security section tab.
#[derive(Clone, Copy, PartialEq, Eq, Default)]
pub enum SecurityTab {
    #[default]
    AccessLists,
    BannedIps,
    QuarantinedIps,
    AnomalyRules,
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
    ];
    html! {
        <div class="border-b border-neutral-300 dark:border-neutral-800">
            <nav class="flex space-x-6">
                { for tabs.into_iter().map(|(tab, label)| {
                    let is_active = **active_tab == tab;
                    let active_cls = if is_active {
                        "border-blue-500 text-blue-500 dark:text-blue-400 font-semibold border-b-2"
                    } else {
                        "border-transparent text-neutral-500 hover:text-neutral-700 dark:text-neutral-400 dark:hover:text-neutral-200"
                    };
                    let onclick = {
                        let active_tab = active_tab.clone();
                        Callback::from(move |_| active_tab.set(tab))
                    };
                    html! {
                        <button type="button" {onclick} class={classes!("py-3", "px-1", "text-sm", "transition-colors", active_cls)}>
                            { locale.t(label) }
                        </button>
                    }
                }) }
            </nav>
        </div>
    }
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
            if list.is_empty() {
                <p class="mt-4 text-sm text-neutral-400 dark:text-neutral-500 italic">{locale.t("settings.banned_ips_empty")}</p>
            } else {
                <div class="mt-4 overflow-x-auto">
                    <table class="w-full text-sm text-left text-neutral-500 dark:text-neutral-400">
                        <thead class="text-xs uppercase bg-neutral-50 dark:bg-neutral-800 text-neutral-700 dark:text-neutral-300">
                            <tr>
                                <th class="px-4 py-2">{"IP"}</th>
                                <th class="px-4 py-2">{locale.t("settings.failures_count")}</th>
                                <th class="px-4 py-2">{locale.t("settings.remaining_time")}</th>
                                <th class="px-4 py-2"></th>
                            </tr>
                        </thead>
                        <tbody>
                            { for list.iter().map(|entry| {
                                let ip_str = entry.ip.to_string();
                                let on_unban = {
                                    let unban = unban.clone();
                                    let ip_str = ip_str.clone();
                                    Callback::from(move |e: MouseEvent| {
                                        e.prevent_default();
                                        unban.emit(ip_str.clone());
                                    })
                                };
                                let mins = entry.remaining_seconds / 60;
                                let secs = entry.remaining_seconds % 60;
                                let time_str = format!("{mins}m {secs}s");
                                html! {
                                    <tr class="border-b dark:border-neutral-700 hover:bg-neutral-50 dark:hover:bg-neutral-700/50">
                                        <td class="px-4 py-2 font-mono font-medium text-neutral-900 dark:text-white">{entry.ip.to_string()}</td>
                                        <td class="px-4 py-2">{entry.failures}</td>
                                        <td class="px-4 py-2 font-mono text-amber-600 dark:text-amber-400">{time_str}</td>
                                        <td class="px-4 py-2 text-right">
                                            <ActionIconButton
                                                variant={ActionVariant::Delete}
                                                title={locale.t("settings.unban").to_string()}
                                                onclick={on_unban}
                                            />
                                        </td>
                                    </tr>
                                }
                            }) }
                        </tbody>
                    </table>
                </div>
            }
        </div>
    }
}

#[function_component(QuarantinedIpsCard)]
fn quarantined_ips_card() -> Html {
    let locale = use_locale();
    let reload = use_state(|| 0);
    let list = use_state(Vec::<QuarantinedIpEntry>::new);
    let notice = use_state(|| Option::<Notice>::None);
    let manual_ip = use_state(String::new);
    let manual_reason = use_state(String::new);

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
        let manual_ip = manual_ip.clone();
        let manual_reason = manual_reason.clone();
        Callback::from(move |e: SubmitEvent| {
            e.prevent_default();
            let ip_str = (*manual_ip).trim().to_string();
            if ip_str.is_empty() {
                return;
            }
            let reason_str = (*manual_reason).trim().to_string();
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
                manual_ip.clone(),
                manual_reason.clone(),
            ));
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

            <form onsubmit={on_manual_submit} class="mt-4 mb-4 flex flex-col sm:flex-row gap-2">
                <input
                    type="text"
                    value={(*manual_ip).clone()}
                    oninput={let manual_ip = manual_ip.clone(); Callback::from(move |e: InputEvent| {
                        if let Some(target) = e.target_dyn_into::<HtmlInputElement>() {
                            manual_ip.set(target.value());
                        }
                    })}
                    placeholder={locale.t("settings.quarantine_ip_placeholder").to_string()}
                    class="flex-1 bg-neutral-50 dark:text-neutral-200 dark:bg-neutral-800 dark:border-neutral-600 border border-neutral-300 text-neutral-900 text-sm rounded-lg p-2"
                />
                <input
                    type="text"
                    value={(*manual_reason).clone()}
                    oninput={let manual_reason = manual_reason.clone(); Callback::from(move |e: InputEvent| {
                        if let Some(target) = e.target_dyn_into::<HtmlInputElement>() {
                            manual_reason.set(target.value());
                        }
                    })}
                    placeholder={locale.t("settings.quarantine_reason_placeholder").to_string()}
                    class="flex-1 bg-neutral-50 dark:text-neutral-200 dark:bg-neutral-800 dark:border-neutral-600 border border-neutral-300 text-neutral-900 text-sm rounded-lg p-2"
                />
                <button
                    type="submit"
                    class="inline-flex items-center justify-center px-4 py-2 text-sm font-medium text-white bg-red-600 hover:bg-red-700 rounded-lg shadow-sm"
                >
                    {locale.t("settings.quarantine_submit")}
                </button>
            </form>

            { quarantined_table_view(locale, &list, unquarantine) }
        </div>
    }
}

async fn submit_manual_quarantine(
    locale: Locale,
    ip_str: String,
    reason: Option<String>,
    notice: UseStateHandle<Option<Notice>>,
    reload: UseStateHandle<usize>,
    manual_ip: UseStateHandle<String>,
    manual_reason: UseStateHandle<String>,
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
                manual_ip.set(String::new());
                manual_reason.set(String::new());
                reload.set(*reload + 1);
            }
            Err(err) => notice.set(Some(Notice::Failed(err))),
        }
    }
}

fn quarantined_table_view(
    locale: Locale,
    list: &[QuarantinedIpEntry],
    unquarantine: Callback<String>,
) -> Html {
    if list.is_empty() {
        return html! {
            <p class="mt-4 text-sm text-neutral-400 dark:text-neutral-500 italic">{locale.t("settings.quarantined_ips_empty")}</p>
        };
    }

    html! {
        <div class="mt-4 overflow-x-auto">
            <table class="w-full text-sm text-left text-neutral-500 dark:text-neutral-400">
                <thead class="text-xs uppercase bg-neutral-50 dark:bg-neutral-800 text-neutral-700 dark:text-neutral-300">
                    <tr>
                        <th class="px-4 py-2">{"IP"}</th>
                        <th class="px-4 py-2">{locale.t("settings.quarantine_reason")}</th>
                        <th class="px-4 py-2">{locale.t("settings.quarantine_violations")}</th>
                        <th class="px-4 py-2">{locale.t("settings.remaining_time")}</th>
                        <th class="px-4 py-2"></th>
                    </tr>
                </thead>
                <tbody>
                    { for list.iter().map(|entry| {
                        let ip_str = entry.ip.to_string();
                        let on_release = {
                            let unquarantine = unquarantine.clone();
                            let ip_str = ip_str.clone();
                            Callback::from(move |_| {
                                unquarantine.emit(ip_str.clone());
                            })
                        };
                        let mins = entry.remaining_seconds / 60;
                        let secs = entry.remaining_seconds % 60;
                        let time_str = format!("{mins}m {secs}s");
                        html! {
                            <tr class="border-b dark:border-neutral-700 hover:bg-neutral-50 dark:hover:bg-neutral-700/50">
                                <td class="px-4 py-2 font-mono font-medium text-neutral-900 dark:text-white">{entry.ip.to_string()}</td>
                                <td class="px-4 py-2 text-neutral-700 dark:text-neutral-300">{&entry.reason}</td>
                                <td class="px-4 py-2 font-mono text-amber-600 dark:text-amber-400">{entry.violations}</td>
                                <td class="px-4 py-2 font-mono text-neutral-500 dark:text-neutral-400">{time_str}</td>
                                <td class="px-4 py-2 text-right">
                                    <ActionIconButton
                                        variant={ActionVariant::Delete}
                                        title={locale.t("settings.unquarantine").to_string()}
                                        onclick={on_release}
                                    />
                                </td>
                            </tr>
                        }
                    }) }
                </tbody>
            </table>
        </div>
    }
}
