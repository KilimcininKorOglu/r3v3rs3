use super::app_log::download_text_file;
use crate::{API_ENDPOINT, auth::use_ensure_auth, format::format_instant, i18n::use_locale};
use gloo_net::http::Request;
use gloo_timers::callback::Timeout;
use r3v3rs3_api::i18n::Locale;
use r3v3rs3_api::log::{LogLevel, SystemLogRow};
use time::OffsetDateTime;
use web_sys::{Element, HtmlInputElement};
use yew::prelude::*;
use yew_router::prelude::use_navigator;

#[derive(Properties, PartialEq)]
pub struct Props {
    pub id: String,
}

#[function_component(LogView)]
pub fn log_view(props: &Props) -> Html {
    use_ensure_auth();
    let locale = use_locale();
    let ul_ref = use_node_ref();
    let search = use_state(String::new);
    let log: UseStateHandle<Vec<SystemLogRow>> = use_state(Vec::<SystemLogRow>::new);
    let id = props.id.clone();
    let log_cloned = log.clone();
    let ul_ref_cloned = ul_ref.clone();

    use_effect_with((), move |_| {
        poll_log(id.clone(), ul_ref_cloned.clone(), log_cloned, vec![], None);
    });

    let Some(navigator) = use_navigator() else {
        return html!();
    };
    let back_onclick = Callback::from(move |_| {
        navigator.back();
    });

    let (filtered_rows, matched, total) = filter_system_logs(&log, &search);
    let id_for_download = props.id.clone();
    let log_for_download = log.clone();
    let on_download = Callback::from(move |_: MouseEvent| {
        let content = export_system_logs(&log_for_download);
        download_text_file(&format!("{id_for_download}-logs.txt"), &content);
    });

    html! {
        <>
            <div class="flex flex-col sm:flex-row items-stretch sm:items-center justify-between gap-2 mb-4">
                <button onclick={back_onclick} class="inline-flex items-center text-neutral-500 dark:text-neutral-200 bg-white dark:bg-neutral-800 border border-neutral-300 dark:border-neutral-700 focus:outline-none hover:bg-neutral-100 hover:dark:bg-neutral-900 focus:ring-4 focus:ring-neutral-200 dark:focus:ring-neutral-600 font-medium rounded-lg text-sm px-4 py-2 self-start" type="button">
                    <img src="/assets/icons/arrow-back.svg" class="w-4 h-4 mr-1" />
                    {locale.t("common.back")}
                </button>
                { log_controls_view(locale, &search, on_download) }
            </div>
            if !search.is_empty() {
                <p class="text-xs text-neutral-500 dark:text-neutral-400 mb-2">
                    {locale.tf("log.matches", &[("matched", &matched.to_string()), ("total", &total.to_string())])}
                </p>
            }
            <ul ref={ul_ref.clone()} class="overflow-auto max-h-[70vh] bg-white dark:bg-neutral-800 shadow-sm border border-neutral-300 dark:border-neutral-700 rounded-md">
            { filtered_rows.into_iter().map(render_log_row).collect::<Html>() }
            if log.is_empty() {
                <li class="my-8 px-4 sm:px-16 text-lg sm:text-xl font-bold text-neutral-500 dark:text-neutral-300 text-center">{locale.t("log.empty")}</li>
            }
            </ul>
        </>
    }
}

fn log_controls_view(
    locale: Locale,
    search: &UseStateHandle<String>,
    on_download: Callback<MouseEvent>,
) -> Html {
    let search_input = {
        let search = search.clone();
        Callback::from(move |e: InputEvent| {
            if let Some(input) = e.target_dyn_into::<HtmlInputElement>() {
                search.set(input.value());
            }
        })
    };
    let clear_search = {
        let search = search.clone();
        Callback::from(move |_: MouseEvent| search.set(String::new()))
    };

    html! {
        <div class="flex items-center gap-2">
            <div class="relative flex-1 sm:w-64">
                <input
                    type="text"
                    placeholder={locale.t("log.search")}
                    value={(**search).clone()}
                    oninput={search_input}
                    class="w-full text-xs sm:text-sm px-3 py-1.5 rounded-lg border border-neutral-300 dark:border-neutral-700 bg-white dark:bg-neutral-800 text-neutral-900 dark:text-neutral-100 placeholder-neutral-400 focus:outline-none focus:ring-2 focus:ring-blue-500"
                />
                if !search.is_empty() {
                    <button
                        onclick={clear_search}
                        type="button"
                        class="absolute right-2.5 top-1.5 text-xs text-neutral-400 hover:text-neutral-600 dark:hover:text-neutral-200"
                    >
                        {"✕"}
                    </button>
                }
            </div>
            <button
                onclick={on_download}
                type="button"
                class="inline-flex items-center text-xs sm:text-sm px-3 py-1.5 rounded-lg border border-neutral-300 dark:border-neutral-700 bg-white dark:bg-neutral-800 hover:bg-neutral-50 dark:hover:bg-neutral-700 font-medium"
            >
                <svg class="w-3.5 h-3.5 mr-1" fill="none" stroke="currentColor" viewBox="0 0 24 24">
                    <path stroke-linecap="round" stroke-linejoin="round" stroke-width="2" d="M4 16v1a3 3 0 003 3h10a3 3 0 003-3v-1m-4-4l-4 4m0 0l-4-4m4 4V4"></path>
                </svg>
                {locale.t("log.download")}
            </button>
        </div>
    }
}

fn render_log_row(entry: &SystemLogRow) -> Html {
    let timestamp = format_instant(&entry.timestamp);
    let fields = entry
        .fields
        .iter()
        .map(|(k, v)| format!("{k}={v}"))
        .collect::<Vec<String>>()
        .join(" ");
    let log_class = match entry.level {
        LogLevel::Error => "text-red-600 dark:text-red-400",
        LogLevel::Warn => "text-yellow-600 dark:text-yellow-400",
        LogLevel::Info => "text-green-600 dark:text-green-400",
        LogLevel::Debug => "text-blue-600 dark:text-blue-400",
        LogLevel::Trace => "text-neutral-600 dark:text-neutral-400",
    };
    html! {
        <li class="font-mono text-xs sm:text-sm px-4 py-1 border-b dark:border-neutral-700 break-words">
            <span class="mr-2">{timestamp}</span>
            <span class={classes!("font-bold", "mr-2", log_class)}>{
                format!("{: <5}", entry.level.to_string().to_ascii_uppercase())
            }</span>
            <span class="mr-2">{entry.message.clone()}</span>
            <span class="fields">{fields}</span>
        </li>
    }
}

pub fn format_log_row_plain(entry: &SystemLogRow) -> String {
    let timestamp = format_instant(&entry.timestamp);
    let fields = entry
        .fields
        .iter()
        .map(|(k, v)| format!("{k}={v}"))
        .collect::<Vec<String>>()
        .join(" ");
    format!(
        "{} {: <5} {} {}",
        timestamp,
        entry.level.to_string().to_ascii_uppercase(),
        entry.message,
        fields
    )
}

pub fn filter_system_logs<'a>(
    logs: &'a [SystemLogRow],
    query: &str,
) -> (Vec<&'a SystemLogRow>, usize, usize) {
    let total = logs.len();
    if query.trim().is_empty() {
        return (logs.iter().collect(), total, total);
    }
    let q = query.trim().to_lowercase();
    let matched: Vec<&'a SystemLogRow> =
        logs.iter()
            .filter(|row| {
                row.message.to_lowercase().contains(&q)
                    || row.level.to_string().to_lowercase().contains(&q)
                    || row.fields.iter().any(|(k, v)| {
                        k.to_lowercase().contains(&q) || v.to_lowercase().contains(&q)
                    })
            })
            .collect();
    let count = matched.len();
    (matched, count, total)
}

pub fn export_system_logs(logs: &[SystemLogRow]) -> String {
    logs.iter()
        .map(format_log_row_plain)
        .collect::<Vec<String>>()
        .join("\n")
}

fn poll_log(
    id: String,
    ul_ref: NodeRef,
    setter: UseStateHandle<Vec<SystemLogRow>>,
    mut history: Vec<SystemLogRow>,
    time: Option<OffsetDateTime>,
) {
    wasm_bindgen_futures::spawn_local(async move {
        if let Ok(mut list) = get_log(&id, time).await {
            let time = list.last().map(|row| row.timestamp).or(time);
            history.append(&mut list);
            setter.set(history.clone());

            if let Some(elem) = ul_ref.cast::<Element>() {
                if elem.scroll_top() == elem.scroll_height() - elem.client_height() {
                    Timeout::new(0, move || {
                        elem.set_scroll_top(elem.scroll_height());
                    })
                    .forget();
                }
                poll_log(id, ul_ref, setter, history, time);
            }
        }
    });
}

async fn get_log(
    id: &str,
    time: Option<OffsetDateTime>,
) -> Result<Vec<SystemLogRow>, gloo_net::Error> {
    let mut req = Request::get(&format!("{API_ENDPOINT}/logs/{id}"));
    if let Some(time) = time {
        req = req.query([("since", &time.unix_timestamp().to_string())]);
    }
    req.send().await?.json().await
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn filter_system_logs_matches_query() {
        let row1 = SystemLogRow {
            timestamp: OffsetDateTime::UNIX_EPOCH,
            level: LogLevel::Info,
            resource_id: "test".to_string(),
            message: "Proxy started".to_string(),
            fields: Default::default(),
        };
        let row2 = SystemLogRow {
            timestamp: OffsetDateTime::UNIX_EPOCH,
            level: LogLevel::Error,
            resource_id: "test".to_string(),
            message: "Connection failed".to_string(),
            fields: Default::default(),
        };
        let logs = vec![row1, row2];
        let (matched, count, total) = filter_system_logs(&logs, "error");
        assert_eq!(count, 1);
        assert_eq!(total, 2);
        assert_eq!(matched[0].message, "Connection failed");
    }
}
