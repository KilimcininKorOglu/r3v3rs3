use super::Route;
use super::accounts::{HINT_CLASS, INPUT_CLASS, LABEL_CLASS};
use super::settings::{failure_box, fetch_json};
use crate::API_ENDPOINT;
use crate::auth::use_ensure_auth;
use crate::components::data_list::{Column, Row, list_card};
use crate::i18n::use_locale;
use crate::store::SessionStore;
use gloo_net::http::Request;
use r3v3rs3_api::{
    audit::{AuditAction, AuditEntry, MAX_QUERY_LIMIT},
    i18n::Locale,
};
use time::{OffsetDateTime, format_description::well_known::Rfc3339};
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

#[derive(Clone, PartialEq)]
struct Filter {
    username: String,
    resource_id: String,
    days: u64,
}

impl Default for Filter {
    fn default() -> Self {
        Self {
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

    let entries_cloned = entries.clone();
    use_effect_with((*filter).clone(), move |filter| {
        entries_cloned.set(None);
        let params = query_params(filter, now_ms());
        spawn_local(async move {
            entries_cloned.set(Some(get_entries(locale, params).await));
        });
    });

    if session.info.is_some() && !session.is_admin() {
        return html! { <Redirect<Route> to={Route::Home} /> };
    }

    let (list, failure) = match entries.as_ref() {
        Some(Ok(list)) => (Some(list.as_slice()), None),
        Some(Err(message)) => (Some(&[][..]), Some(message.as_str())),
        None => (None, None),
    };
    let rows = list
        .unwrap_or_default()
        .iter()
        .enumerate()
        .map(|(index, entry)| entry_row(locale, index, entry))
        .collect::<Vec<_>>();
    html! {
        <>
            { filter_view(locale, &filter) }
            { failure.map(failure_box).unwrap_or_default() }
            { list_card(locale, list.is_some(), "audit.empty", &COLUMNS, &rows) }
            <p class={HINT_CLASS}>{locale.t("audit.limit_hint")}</p>
        </>
    }
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
    html! {
        <div class="grid md:grid-cols-3 gap-x-4 mb-4">
            <div>
                <label class={LABEL_CLASS} for="audit-username">{locale.t("audit.username")}</label>
                <input id="audit-username" type="text" class={INPUT_CLASS} value={filter.username.clone()}
                    onchange={text_onchange(|filter: &mut Filter, value: String| filter.username = value)} />
            </div>
            <div>
                <label class={LABEL_CLASS} for="audit-resource">{locale.t("audit.resource_id")}</label>
                <input id="audit-resource" type="text" class={INPUT_CLASS} value={filter.resource_id.clone()}
                    onchange={text_onchange(|filter: &mut Filter, value: String| filter.resource_id = value)} />
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
    let name = serde_json::to_value(action)
        .ok()
        .and_then(|value| value.as_str().map(str::to_string))
        .unwrap_or_default();
    format!("audit.action_{name}")
}

/// The time of an entry in the RFC 3339 form, in UTC.
fn format_time(ms: u64) -> String {
    OffsetDateTime::from_unix_timestamp_nanos(i128::from(ms) * 1_000_000)
        .ok()
        .and_then(|time| time.format(&Rfc3339).ok())
        .unwrap_or_else(|| ms.to_string())
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
            username: " admin ".into(),
            days: 1,
            ..Default::default()
        };
        assert_eq!(
            query_params(&filter, 3 * DAY_MS),
            [
                ("since", (2 * DAY_MS).to_string()),
                ("limit", "500".to_string()),
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
}
