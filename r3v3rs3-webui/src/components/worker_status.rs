use crate::{API_ENDPOINT, i18n::use_locale, store::SessionStore};
use gloo_net::http::Request;
use r3v3rs3_api::app::WorkerStatusReport;
use yew::prelude::*;
use yewdux::prelude::*;

/// One row per worker that serves the ports of this master. The card is empty while the
/// worker mode is off.
#[function_component(WorkerStatusCard)]
pub fn worker_status_card() -> Html {
    let locale = use_locale();
    let (session, _) = use_store::<SessionStore>();
    let report = use_state(|| None::<WorkerStatusReport>);

    {
        let report = report.clone();
        use_effect_with(session.info.is_some(), move |signed_in| {
            if *signed_in {
                let report = report.clone();
                wasm_bindgen_futures::spawn_local(async move {
                    match get_workers().await {
                        Ok(new_report) => report.set(Some(new_report)),
                        Err(err) => web_sys::console::error_1(
                            &format!("the worker status is not readable: {err}").into(),
                        ),
                    }
                });
            }
        });
    }

    let Some(report) = &*report else {
        return html! {};
    };
    if report.workers.is_empty() {
        return html! {};
    }
    html! {
        <div class="mt-4 bg-white dark:bg-neutral-800 shadow-sm p-5 border border-neutral-300 dark:border-neutral-700 rounded-md">
            <h2 class="text-lg font-semibold text-neutral-900 dark:text-neutral-200">{locale.t("workers.title")}</h2>
            <ul class="mt-3 text-sm text-neutral-900 dark:text-neutral-200">
                { for report.workers.iter().map(|worker| worker_row(&locale, worker)) }
            </ul>
        </div>
    }
}

/// One line for a worker: its id, the process it runs in and what it serves.
fn worker_row(locale: &r3v3rs3_api::i18n::Locale, worker: &r3v3rs3_api::app::WorkerEntry) -> Html {
    match &worker.status {
        Some(status) => {
            let detail = locale.tf(
                "workers.status",
                &[
                    ("pid", &status.pid.to_string()),
                    ("uptime", &status.uptime_secs.to_string()),
                    ("connections", &status.open_connections.to_string()),
                ],
            );
            html! {
                <li class="flex flex-wrap items-center gap-x-4 py-1">
                    <span class="font-medium">{&worker.id}</span>
                    <span class="bg-green-500 w-2 h-2 rounded-full"></span>
                    <span class="text-neutral-500 dark:text-neutral-400">{detail}</span>
                </li>
            }
        }
        None => html! {
            <li class="flex flex-wrap items-center gap-x-4 py-1">
                <span class="font-medium">{&worker.id}</span>
                <span class="bg-yellow-400 w-2 h-2 rounded-full"></span>
                <span class="text-neutral-500 dark:text-neutral-400">{locale.t("workers.unreachable")}</span>
            </li>
        },
    }
}

async fn get_workers() -> Result<WorkerStatusReport, gloo_net::Error> {
    Request::get(&format!("{API_ENDPOINT}/workers"))
        .send()
        .await?
        .json()
        .await
}

#[cfg(test)]
mod tests {
    use super::*;
    use r3v3rs3_api::i18n::Locale;

    #[test]
    fn every_worker_text_is_translated() {
        for key in ["workers.title", "workers.status", "workers.unreachable"] {
            assert_ne!(Locale::Tr.t(key), key);
            assert_ne!(Locale::En.t(key), key);
        }
    }
}
