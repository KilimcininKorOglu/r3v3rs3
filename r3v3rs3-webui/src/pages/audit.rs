use super::Route;
use super::accounts::{HINT_CLASS, INPUT_CLASS, LABEL_CLASS};
use super::app_log::download_text_file;
use super::settings::{failure_box, fetch_json, send_request};
use crate::API_ENDPOINT;
use crate::auth::use_ensure_auth;
use crate::components::data_list::{Column, Row, list_card};
use crate::format::format_time;
use crate::i18n::use_locale;
use crate::store::SessionStore;
use gloo_net::http::Request;
use r3v3rs3_api::{
    app::AppConfig,
    audit::{AuditAction, AuditEntry, MAX_QUERY_LIMIT},
    i18n::Locale,
};
use std::time::Duration;
use wasm_bindgen_futures::spawn_local;
use web_sys::{HtmlInputElement, HtmlSelectElement};
use web_time::{SystemTime, UNIX_EPOCH};
use yew::prelude::*;
use yew_router::prelude::*;
use yewdux::prelude::*;

const DAY_MS: u64 = 24 * 60 * 60 * 1000;

const COLUMNS: [Column; 7] = [
    Column {
        label: "audit.time",
        class: "whitespace-nowrap",
    },
    Column {
        label: "audit.username",
        class: "whitespace-nowrap",
    },
    Column {
        label: "audit.action",
        class: "whitespace-nowrap",
    },
    Column {
        label: "audit.resource_id",
        class: "whitespace-nowrap",
    },
    Column {
        label: "audit.summary",
        class: "",
    },
    Column {
        label: "audit.client",
        class: "whitespace-nowrap",
    },
    Column {
        label: "audit.node",
        class: "whitespace-nowrap",
    },
];

/// Each period of the filter in days, with the translation key of its name.
const PERIODS: [(u64, &str); 3] = [
    (1, "audit.period_1"),
    (7, "audit.period_7"),
    (31, "audit.period_31"),
];

const RETENTIONS: [(u64, &str); 5] = [
    (30, "audit.retention_30d"),
    (90, "audit.retention_90d"),
    (180, "audit.retention_180d"),
    (365, "audit.retention_1y"),
    (730, "audit.retention_2y"),
];

#[derive(Clone, PartialEq)]
struct Filter {
    search: String,
    action: Option<AuditAction>,
    username: String,
    resource_id: String,
    days: u64,
}

impl Default for Filter {
    fn default() -> Self {
        Self {
            search: String::new(),
            action: None,
            username: String::new(),
            resource_id: String::new(),
            days: 7,
        }
    }
}

/// The entries, or the message of the failed request. `None` while the request runs.
type Entries = UseStateHandle<Option<Result<Vec<AuditEntry>, String>>>;

#[function_component(AuditLog)]
pub fn audit_log() -> Html {
    use_ensure_auth();
    let locale = use_locale();
    let (session, _) = use_store::<SessionStore>();
    let filter = use_state(Filter::default);
    let entries: Entries = use_state(|| None);
    let config = use_state(|| Option::<AppConfig>::None);
    let notice = use_state(|| Option::<String>::None);

    let entries_cloned = entries.clone();
    use_effect_with((*filter).clone(), move |filter| {
        load_entries(locale, &entries_cloned, filter);
    });

    let config_cloned = config.clone();
    let signed_in = session.info.is_some();
    let is_admin = session.is_admin();
    use_effect_with((signed_in, is_admin), move |&(signed_in, is_admin)| {
        if signed_in && is_admin {
            load_config(locale, &config_cloned);
        }
    });

    if signed_in && !is_admin {
        return html! { <Redirect<Route> to={Route::Home} /> };
    }

    let on_retention_change = retention_callback(locale, &config, &notice);
    let (list, failure) = resolve_entries(&entries);
    let rows = list
        .unwrap_or_default()
        .iter()
        .enumerate()
        .map(|(index, entry)| entry_row(locale, index, entry))
        .collect::<Vec<_>>();
    html! {
        <>
            { toolbar_view(locale, is_admin, &config, list, on_retention_change) }
            if let Some(msg) = notice.as_ref() {
                { notice_box(msg) }
            }
            { filter_view(locale, &filter) }
            { failure.map(failure_box).unwrap_or_default() }
            { list_card(locale, list.is_some(), "audit.empty", &COLUMNS, &rows) }
            <p class={HINT_CLASS}>{locale.t("audit.limit_hint")}</p>
        </>
    }
}

fn load_entries(locale: Locale, entries: &Entries, filter: &Filter) {
    let entries = entries.clone();
    entries.set(None);
    let params = query_params(filter, now_ms());
    spawn_local(async move {
        entries.set(Some(get_entries(locale, params).await));
    });
}

fn load_config(locale: Locale, config: &UseStateHandle<Option<AppConfig>>) {
    let config = config.clone();
    spawn_local(async move {
        let req = Request::get(&format!("{API_ENDPOINT}/config"));
        if let Ok(cfg) = fetch_json::<AppConfig>(locale, req).await {
            config.set(Some(cfg));
        }
    });
}

fn resolve_entries(entries: &Entries) -> (Option<&[AuditEntry]>, Option<&str>) {
    match entries.as_ref() {
        Some(Ok(list)) => (Some(list.as_slice()), None),
        Some(Err(message)) => (Some(&[][..]), Some(message.as_str())),
        None => (None, None),
    }
}

fn notice_box(msg: &str) -> Html {
    html! {
        <div class="mb-4 p-3 rounded-lg bg-green-50 dark:bg-green-950/40 border border-green-200 dark:border-green-800 text-xs text-green-700 dark:text-green-300">
            {msg}
        </div>
    }
}

fn toolbar_view(
    locale: Locale,
    is_admin: bool,
    config: &Option<AppConfig>,
    entries: Option<&[AuditEntry]>,
    on_retention_change: Callback<Event>,
) -> Html {
    let on_export = {
        let entries = entries.map(|e| e.to_vec()).unwrap_or_default();
        Callback::from(move |e: MouseEvent| {
            e.prevent_default();
            if entries.is_empty() {
                return;
            }
            let csv = export_audit_csv(&entries, locale);
            let filename = format!("audit-log-{}.csv", now_ms());
            download_text_file(&filename, &csv);
        })
    };
    let current_days = config
        .as_ref()
        .map(|c| c.log.audit_log_retention.as_secs() / 86_400)
        .unwrap_or(365);
    html! {
        <div class="flex flex-wrap items-center justify-between gap-4 mb-4">
            <button
                type="button"
                onclick={on_export}
                class="inline-flex items-center gap-1.5 text-neutral-700 dark:text-neutral-200 bg-white dark:bg-neutral-800 border border-neutral-300 dark:border-neutral-700 hover:bg-neutral-50 hover:dark:bg-neutral-900 focus:ring-4 focus:ring-neutral-200 dark:focus:ring-neutral-700 font-medium rounded-lg text-xs px-3 py-2"
            >
                <img src="/assets/icons/download.svg" class="w-3.5 h-3.5 mr-1 dark:invert opacity-80" alt="" />
                {locale.t("audit.export_csv")}
            </button>
            if is_admin {
                <div class="flex items-center gap-2">
                    <label class="text-xs font-medium text-neutral-500 dark:text-neutral-400 whitespace-nowrap" for="audit-retention">
                        {locale.t("audit.retention")}
                    </label>
                    <select
                        id="audit-retention"
                        class="bg-neutral-50 border border-neutral-300 text-neutral-900 text-xs rounded-lg focus:ring-blue-500 focus:border-blue-500 block p-1.5 dark:bg-neutral-800 dark:border-neutral-700 dark:placeholder-neutral-400 dark:text-white"
                        onchange={on_retention_change}
                    >
                        { for RETENTIONS.iter().map(|(days, key)| html! {
                            <option value={days.to_string()} selected={*days == current_days}>
                                {locale.t(key)}
                            </option>
                        }) }
                    </select>
                </div>
            }
        </div>
    }
}

fn retention_callback(
    locale: Locale,
    config: &UseStateHandle<Option<AppConfig>>,
    notice: &UseStateHandle<Option<String>>,
) -> Callback<Event> {
    let (config, notice) = (config.clone(), notice.clone());
    Callback::from(move |event: Event| {
        let select: HtmlSelectElement = event.target_unchecked_into();
        let Ok(days) = select.value().parse::<u64>() else {
            return;
        };
        let Some(mut cfg) = (*config).clone() else {
            return;
        };
        cfg.log.audit_log_retention = Duration::from_secs(days * 86_400);
        let (config, notice) = (config.clone(), notice.clone());
        spawn_local(async move {
            let Ok(req) = Request::put(&format!("{API_ENDPOINT}/config")).json(&cfg) else {
                return;
            };
            if send_request(locale, req).await.is_ok() {
                config.set(Some(cfg));
                notice.set(Some(locale.t("audit.retention_saved").to_string()));
            }
        });
    })
}

fn filter_view(locale: Locale, filter: &UseStateHandle<Filter>) -> Html {
    let text_onchange = |apply: fn(&mut Filter, String)| {
        let filter = filter.clone();
        Callback::from(move |event: Event| {
            let input: HtmlInputElement = event.target_unchecked_into();
            let mut next = (*filter).clone();
            apply(&mut next, input.value());
            filter.set(next);
        })
    };
    let action_onchange = {
        let filter = filter.clone();
        Callback::from(move |event: Event| {
            let select: HtmlSelectElement = event.target_unchecked_into();
            let val = select.value();
            let action = serde_json::from_value::<AuditAction>(serde_json::Value::String(val)).ok();
            let mut next = (*filter).clone();
            next.action = action;
            filter.set(next);
        })
    };
    let period_onchange = {
        let filter = filter.clone();
        Callback::from(move |event: Event| {
            let select: HtmlSelectElement = event.target_unchecked_into();
            if let Ok(days) = select.value().parse() {
                filter.set(Filter {
                    days,
                    ..(*filter).clone()
                });
            }
        })
    };
    let current_action = filter.action.map(action_name).unwrap_or_default();
    html! {
        <div class="grid md:grid-cols-3 lg:grid-cols-5 gap-3 mb-4">
            <div>
                <label class={LABEL_CLASS} for="audit-search">{locale.t("audit.search")}</label>
                <input id="audit-search" type="text" class={INPUT_CLASS} placeholder={locale.t("audit.search_placeholder")} value={filter.search.clone()}
                    onchange={text_onchange(|f, v| f.search = v)} />
            </div>
            <div>
                <label class={LABEL_CLASS} for="audit-action">{locale.t("audit.action")}</label>
                <select id="audit-action" class={INPUT_CLASS} onchange={action_onchange}>
                    <option value="" selected={filter.action.is_none()}>{locale.t("audit.all_actions")}</option>
                    { for AuditAction::ALL.iter().map(|&action| {
                        let name = action_name(action);
                        let selected = name == current_action;
                        html! {
                            <option value={name} {selected}>{locale.t(&action_key(action))}</option>
                        }
                    }) }
                </select>
            </div>
            <div>
                <label class={LABEL_CLASS} for="audit-username">{locale.t("audit.username")}</label>
                <input id="audit-username" type="text" class={INPUT_CLASS} value={filter.username.clone()}
                    onchange={text_onchange(|f, v| f.username = v)} />
            </div>
            <div>
                <label class={LABEL_CLASS} for="audit-resource">{locale.t("audit.resource_id")}</label>
                <input id="audit-resource" type="text" class={INPUT_CLASS} value={filter.resource_id.clone()}
                    onchange={text_onchange(|f, v| f.resource_id = v)} />
            </div>
            <div>
                <label class={LABEL_CLASS} for="audit-period">{locale.t("audit.period")}</label>
                <select id="audit-period" class={INPUT_CLASS} onchange={period_onchange}>
                    { for PERIODS.iter().map(|(days, key)| html! {
                        <option value={days.to_string()} selected={*days == filter.days}>{locale.t(key)}</option>
                    }) }
                </select>
            </div>
        </div>
    }
}

fn entry_row(locale: Locale, index: usize, entry: &AuditEntry) -> Row {
    let text = |value: &str| html! { <>{value.to_string()}</> };
    let client = entry.client.map(|ip| ip.to_string()).unwrap_or_default();
    Row {
        key: index.to_string(),
        cells: vec![
            text(&format_time(entry.time)),
            text(&entry.username),
            text(locale.t(&action_key(entry.action))),
            text(entry.resource_id.as_deref().unwrap_or_default()),
            text(&entry.summary),
            text(&client),
            text(&entry.node),
        ],
        actions: html! {},
    }
}

/// The translation key of the name of an action.
fn action_key(action: AuditAction) -> String {
    let name = action_name(action);
    format!("audit.action_{name}")
}

fn action_name(action: AuditAction) -> String {
    serde_json::to_value(action)
        .ok()
        .and_then(|value| value.as_str().map(str::to_string))
        .unwrap_or_default()
}

pub fn export_audit_csv(entries: &[AuditEntry], locale: Locale) -> String {
    let mut csv = String::from("time,username,action,resource_id,summary,client,node\r\n");
    for entry in entries {
        let time = format_time(entry.time);
        let username = escape_csv(&entry.username);
        let action = escape_csv(locale.t(&action_key(entry.action)));
        let resource_id = escape_csv(entry.resource_id.as_deref().unwrap_or(""));
        let summary = escape_csv(&entry.summary);
        let client = escape_csv(&entry.client.map(|ip| ip.to_string()).unwrap_or_default());
        let node = escape_csv(&entry.node);
        csv.push_str(&format!(
            "{time},{username},{action},{resource_id},{summary},{client},{node}\r\n"
        ));
    }
    csv
}

fn escape_csv(field: &str) -> String {
    if field.contains(',') || field.contains('"') || field.contains('\n') || field.contains('\r') {
        format!("\"{}\"", field.replace('"', "\"\""))
    } else {
        field.to_string()
    }
}

fn now_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |elapsed| {
            u64::try_from(elapsed.as_millis()).unwrap_or(u64::MAX)
        })
}

/// The query parameters of the filter at `now` in Unix milliseconds. An empty field does not
/// filter.
fn query_params(filter: &Filter, now: u64) -> Vec<(&'static str, String)> {
    let since = now.saturating_sub(filter.days * DAY_MS);
    let mut params = vec![
        ("since", since.to_string()),
        ("limit", MAX_QUERY_LIMIT.to_string()),
    ];
    let search = filter.search.trim();
    if !search.is_empty() {
        params.push(("search", search.to_string()));
    }
    if let Some(action) = filter.action {
        let name = action_name(action);
        if !name.is_empty() {
            params.push(("action", name));
        }
    }
    let fields = [
        ("username", &filter.username),
        ("resource_id", &filter.resource_id),
    ];
    for (key, value) in fields {
        let value = value.trim();
        if !value.is_empty() {
            params.push((key, value.to_string()));
        }
    }
    params
}

async fn get_entries(
    locale: Locale,
    params: Vec<(&'static str, String)>,
) -> Result<Vec<AuditEntry>, String> {
    let request = Request::get(&format!("{API_ENDPOINT}/audit")).query(params);
    fetch_json(locale, request).await
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_filter_asks_for_the_period_and_the_filled_fields() {
        let filter = Filter {
            search: " special ".into(),
            action: Some(AuditAction::Login),
            username: " admin ".into(),
            days: 1,
            ..Default::default()
        };
        assert_eq!(
            query_params(&filter, 3 * DAY_MS),
            [
                ("since", (2 * DAY_MS).to_string()),
                ("limit", "500".to_string()),
                ("search", "special".to_string()),
                ("action", "login".to_string()),
                ("username", "admin".to_string()),
            ]
        );
    }

    #[test]
    fn each_action_and_time_has_a_text() {
        for locale in Locale::ALL {
            for action in AuditAction::ALL {
                let key = action_key(action);
                assert_ne!(locale.t(&key), key);
            }
        }
        assert_eq!(
            action_key(AuditAction::LoginFailed),
            "audit.action_login_failed"
        );
        assert_eq!(format_time(1_500), "1970-01-01T00:00:01.5Z");
    }

    #[test]
    fn export_audit_csv_formats_lines_and_escapes() {
        let entry = AuditEntry {
            time: 1_000,
            username: "admin,special".into(),
            client: Some("127.0.0.1".parse().unwrap()),
            action: AuditAction::Login,
            resource_id: Some("res\"1\"".into()),
            summary: "line1\nline2".into(),
            node: "node1".into(),
        };
        let csv = export_audit_csv(&[entry], Locale::En);
        assert!(csv.starts_with("time,username,action,resource_id,summary,client,node\r\n"));
        assert!(csv.contains("\"admin,special\""));
        assert!(csv.contains("\"res\"\"1\"\"\""));
        assert!(csv.contains("\"line1\nline2\""));
    }
}
