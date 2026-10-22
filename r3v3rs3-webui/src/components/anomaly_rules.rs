use crate::API_ENDPOINT;
use crate::i18n::use_locale;
use crate::store::SessionStore;
use gloo_net::http::Request;
use r3v3rs3_api::anomaly::AnomalyAlertRules;
use web_sys::HtmlInputElement;
use yew::prelude::*;
use yewdux::prelude::*;

const INPUT_CLASS: &str = "bg-neutral-50 border border-neutral-300 text-neutral-900 text-sm rounded-lg focus:ring-blue-500 focus:border-blue-500 block w-full p-2.5 dark:bg-neutral-700 dark:border-neutral-600 dark:placeholder-neutral-400 dark:text-white dark:focus:ring-blue-500 dark:focus:border-blue-500";

#[derive(Clone, Default, PartialEq)]
struct FormFields {
    enabled: bool,
    max_p99_latency_ms: String,
    max_5xx_percent: String,
    max_request_rate_rps: String,
    max_event_loop_lag_ms: String,
}

impl FormFields {
    fn from_rules(rules: &AnomalyAlertRules) -> Self {
        Self {
            enabled: rules.enabled,
            max_p99_latency_ms: rules
                .max_p99_latency_ms
                .map(|v| v.to_string())
                .unwrap_or_default(),
            max_5xx_percent: rules
                .max_5xx_percent
                .map(|v| v.to_string())
                .unwrap_or_default(),
            max_request_rate_rps: rules
                .max_request_rate_rps
                .map(|v| v.to_string())
                .unwrap_or_default(),
            max_event_loop_lag_ms: rules
                .max_event_loop_lag_ms
                .map(|v| v.to_string())
                .unwrap_or_default(),
        }
    }

    fn to_rules(&self) -> AnomalyAlertRules {
        AnomalyAlertRules {
            enabled: self.enabled,
            max_p99_latency_ms: self.max_p99_latency_ms.trim().parse::<u32>().ok(),
            max_5xx_percent: self.max_5xx_percent.trim().parse::<u8>().ok(),
            max_request_rate_rps: self.max_request_rate_rps.trim().parse::<u32>().ok(),
            max_event_loop_lag_ms: self.max_event_loop_lag_ms.trim().parse::<u32>().ok(),
        }
    }
}

/// Anomaly alert rules threshold configuration card on the Settings page.
#[function_component(AnomalyRulesCard)]
pub fn anomaly_rules_card() -> Html {
    let locale = use_locale();
    let (session, _) = use_store::<SessionStore>();
    let form = use_state(FormFields::default);
    let notice = use_state(|| Option::<String>::None);
    let is_loading = use_state(|| false);
    let is_saving = use_state(|| false);

    {
        let form = form.clone();
        use_effect_with(session.info.is_some(), move |signed_in| {
            if *signed_in {
                let form = form.clone();
                wasm_bindgen_futures::spawn_local(async move {
                    if let Ok(rules) = fetch_anomaly_rules().await {
                        form.set(FormFields::from_rules(&rules));
                    }
                });
            }
        });
    }

    let on_submit = {
        let form = form.clone();
        let notice = notice.clone();
        let is_saving = is_saving.clone();
        Callback::from(move |e: SubmitEvent| {
            e.prevent_default();
            let form = form.clone();
            let notice = notice.clone();
            let is_saving = is_saving.clone();
            is_saving.set(true);

            wasm_bindgen_futures::spawn_local(async move {
                let new_rules = form.to_rules();
                match save_anomaly_rules(&new_rules).await {
                    Ok(saved) => {
                        form.set(FormFields::from_rules(&saved));
                        notice.set(Some(locale.t("anomalies.rules_saved").to_string()));
                    }
                    Err(err) => {
                        notice.set(Some(err));
                    }
                }
                is_saving.set(false);
            });
        })
    };

    html! {
        <form onsubmit={on_submit} class="mt-4 bg-white dark:bg-neutral-800 shadow-sm p-5 border border-neutral-300 dark:border-neutral-700 rounded-md">
            if let Some(msg) = &*notice {
                <div class="mb-4 p-3 rounded text-sm bg-blue-50 text-blue-800 dark:bg-blue-900/30 dark:text-blue-300 border border-blue-200 dark:border-blue-800">
                    {msg}
                </div>
            }

            <h2 class="text-lg font-semibold text-neutral-900 dark:text-neutral-200">
                {locale.t("anomalies.rules_card_title")}
            </h2>
            <p class="mt-1 text-sm text-neutral-500 dark:text-neutral-400">
                {locale.t("anomalies.subtitle")}
            </p>

            <div class="mt-4 flex items-center gap-2">
                <input
                    type="checkbox"
                    id="anomaly_enabled"
                    checked={form.enabled}
                    onchange={{
                        let form = form.clone();
                        Callback::from(move |e: Event| {
                            let input: HtmlInputElement = e.target_unchecked_into();
                            let mut next = (*form).clone();
                            next.enabled = input.checked();
                            form.set(next);
                        })
                    }}
                    class="rounded border-neutral-300 dark:border-neutral-600 text-blue-600 focus:ring-blue-500"
                />
                <label for="anomaly_enabled" class="text-sm font-medium text-neutral-900 dark:text-neutral-200 cursor-pointer">
                    {locale.t("anomalies.enabled")}
                </label>
            </div>

            <div class="mt-4 grid grid-cols-1 md:grid-cols-2 gap-4">
                { render_input_field(
                    &form,
                    locale.t("anomalies.p99_latency"),
                    locale.t("anomalies.p99_latency_hint"),
                    &form.max_p99_latency_ms,
                    |f, v| f.max_p99_latency_ms = v,
                ) }
                { render_input_field(
                    &form,
                    locale.t("anomalies.5xx_rate"),
                    locale.t("anomalies.5xx_rate_hint"),
                    &form.max_5xx_percent,
                    |f, v| f.max_5xx_percent = v,
                ) }
                { render_input_field(
                    &form,
                    locale.t("anomalies.traffic_spike"),
                    locale.t("anomalies.traffic_spike_hint"),
                    &form.max_request_rate_rps,
                    |f, v| f.max_request_rate_rps = v,
                ) }
                { render_input_field(
                    &form,
                    locale.t("anomalies.event_loop_lag"),
                    locale.t("anomalies.event_loop_lag_hint"),
                    &form.max_event_loop_lag_ms,
                    |f, v| f.max_event_loop_lag_ms = v,
                ) }
            </div>

            <div class="mt-5 flex justify-end">
                <button
                    type="submit"
                    disabled={*is_saving || *is_loading}
                    class="inline-flex justify-center items-center text-white bg-blue-600 hover:bg-blue-700 focus:ring-4 focus:ring-blue-300 font-medium rounded-lg text-sm px-4 py-2 transition disabled:opacity-50"
                >
                    {locale.t("anomalies.save_rules")}
                </button>
            </div>
        </form>
    }
}

fn render_input_field<F>(
    form: &UseStateHandle<FormFields>,
    label: &'static str,
    hint: &'static str,
    value: &str,
    updater: F,
) -> Html
where
    F: Fn(&mut FormFields, String) + 'static,
{
    let form = form.clone();
    let oninput = Callback::from(move |e: InputEvent| {
        let input: HtmlInputElement = e.target_unchecked_into();
        let mut next = (*form).clone();
        updater(&mut next, input.value());
        form.set(next);
    });

    html! {
        <div>
            <label class="block text-sm font-medium text-neutral-900 dark:text-neutral-200">
                {label}
            </label>
            <input
                type="number"
                value={value.to_string()}
                {oninput}
                class={classes!("mt-1", INPUT_CLASS)}
            />
            <p class="mt-1 text-xs text-neutral-500 dark:text-neutral-400">
                {hint}
            </p>
        </div>
    }
}

async fn fetch_anomaly_rules() -> Result<AnomalyAlertRules, gloo_net::Error> {
    Request::get(&format!("{API_ENDPOINT}/system/anomalies/rules"))
        .send()
        .await?
        .json()
        .await
}

async fn save_anomaly_rules(rules: &AnomalyAlertRules) -> Result<AnomalyAlertRules, String> {
    let res = Request::put(&format!("{API_ENDPOINT}/system/anomalies/rules"))
        .json(rules)
        .map_err(|e| e.to_string())?
        .send()
        .await
        .map_err(|e| e.to_string())?;

    if res.ok() {
        res.json::<AnomalyAlertRules>()
            .await
            .map_err(|e| e.to_string())
    } else {
        Err(format!(
            "Error {}: Failed to save anomaly alert rules",
            res.status()
        ))
    }
}

#[cfg(test)]
mod tests {
    use r3v3rs3_api::i18n::Locale;

    #[test]
    fn every_anomaly_text_is_translated() {
        for key in [
            "anomalies.title",
            "anomalies.subtitle",
            "anomalies.rules_card_title",
            "anomalies.enabled",
            "anomalies.p99_latency",
            "anomalies.p99_latency_hint",
            "anomalies.5xx_rate",
            "anomalies.5xx_rate_hint",
            "anomalies.traffic_spike",
            "anomalies.traffic_spike_hint",
            "anomalies.event_loop_lag",
            "anomalies.event_loop_lag_hint",
            "anomalies.save_rules",
            "anomalies.rules_saved",
            "anomalies.no_active_anomalies",
            "anomalies.active_badge",
            "anomalies.critical",
            "anomalies.warning",
            "anomalies.notifications_center",
            "anomalies.view_traffic",
            "anomalies.clear_all",
        ] {
            assert_ne!(Locale::Tr.t(key), key);
            assert_ne!(Locale::En.t(key), key);
        }
    }
}
