//! The variables of an app store entry that the user fills before the first deployment.

use super::accounts::{HINT_CLASS, INPUT_CLASS, LABEL_CLASS};
use crate::API_ENDPOINT;
use r3v3rs3_api::i18n::Locale;
use r3v3rs3_api::store::StoreEnv;
use web_sys::HtmlInputElement;
use yew::prelude::*;

/// One variable of the catalog entry with the value that the user typed.
#[derive(Clone, PartialEq)]
pub struct StoreEnvValue {
    pub env: StoreEnv,
    pub value: String,
}

/// Reads the values that the user typed. `None` when a required value is empty; the error
/// message explains it.
pub fn parse(
    locale: Locale,
    values: &[StoreEnvValue],
) -> Result<Vec<r3v3rs3_api::platform::EnvEntry>, String> {
    let mut entries = Vec::with_capacity(values.len());
    for item in values {
        let value = item.value.trim();
        if value.is_empty() {
            return Err(locale.t("apps.store_env_required").to_string());
        }
        entries.push(r3v3rs3_api::platform::EnvEntry {
            key: item
                .env
                .key
                .parse()
                .map_err(|err| locale.error_message(&err))?,
            value: Some(value.to_string()),
            secret: item.env.secret,
        });
    }
    Ok(entries)
}

/// The fields of the variables section of a catalog entry.
pub fn fields_view(locale: Locale, values: &UseStateHandle<Vec<StoreEnvValue>>) -> Html {
    let tr = locale == Locale::Tr;
    let on_auto_generate = auto_generate_secrets(locale, values);
    let rows = (**values)
        .iter()
        .enumerate()
        .map(|(index, item)| {
            let state = values.clone();
            let oninput = Callback::from(move |event: InputEvent| {
                let input: HtmlInputElement = event.target_unchecked_into();
                let mut updated = (*state).clone();
                updated[index].value = input.value();
                state.set(updated);
            });
            html! {
                <>
                    <label class={LABEL_CLASS}>{item.env.key.clone()}</label>
                    <input
                        type={if item.env.secret { "password" } else { "text" }}
                        autocapitalize="off"
                        autocomplete="off"
                        value={item.value.clone()}
                        {oninput}
                        class={INPUT_CLASS}
                    />
                    <p class={HINT_CLASS}>{item.env.description_in(tr)}</p>
                </>
            }
        })
        .collect::<Html>();
    html! {
        <>
            <div class="flex items-center justify-between mb-1 mt-4">
                <label class={LABEL_CLASS}>{locale.t("apps.store_env")}</label>
                <button
                    type="button"
                    class="px-2.5 py-1 text-xs font-medium text-emerald-700 bg-emerald-50 hover:bg-emerald-100 dark:text-emerald-300 dark:bg-emerald-950/60 dark:hover:bg-emerald-900/60 rounded border border-emerald-300 dark:border-emerald-800 transition-colors"
                    onclick={on_auto_generate}
                >
                    {locale.t("apps.store_env_generate_secrets")}
                </button>
            </div>
            {rows}
            <p class={HINT_CLASS}>{locale.t("apps.store_env_hint")}</p>
        </>
    }
}

fn auto_generate_secrets(
    locale: Locale,
    values: &UseStateHandle<Vec<StoreEnvValue>>,
) -> Callback<MouseEvent> {
    let values_state = values.clone();
    Callback::from(move |event: MouseEvent| {
        event.prevent_default();
        let current = (*values_state).clone();
        let entries_req: Vec<r3v3rs3_api::platform::EnvEntry> = current
            .iter()
            .filter_map(|item| {
                item.env
                    .key
                    .parse()
                    .ok()
                    .map(|k| r3v3rs3_api::platform::EnvEntry {
                        key: k,
                        value: if item.value.trim().is_empty() {
                            None
                        } else {
                            Some(item.value.clone())
                        },
                        secret: item.env.secret,
                    })
            })
            .collect();

        let values_state = values_state.clone();
        wasm_bindgen_futures::spawn_local(async move {
            let req_body = r3v3rs3_api::compose_env::ResolveEnvTemplateRequest {
                template: None,
                entries: Some(entries_req),
                overwrite: false,
            };
            let path = format!("{API_ENDPOINT}/compose/env/generate");
            if let Ok(resp) = crate::pages::settings::post_json::<
                r3v3rs3_api::compose_env::ResolveEnvTemplateResponse,
            >(locale, gloo_net::http::Request::post(&path), &req_body)
            .await
            {
                let mut updated = (*values_state).clone();
                for resolved in resp.entries {
                    if let Some(item) = updated
                        .iter_mut()
                        .find(|i| i.env.key == resolved.key.as_str())
                    {
                        if let Some(val) = resolved.value {
                            item.value = val;
                        }
                    }
                }
                values_state.set(updated);
            }
        });
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn value(key: &str, secret: bool, value: &str) -> StoreEnvValue {
        StoreEnvValue {
            env: StoreEnv {
                key: key.to_string(),
                description: "d".to_string(),
                description_tr: String::new(),
                secret,
            },
            value: value.to_string(),
        }
    }

    #[test]
    fn parse_sends_the_filled_values_with_their_secret_marks() {
        let values = vec![
            value("DB_PASSWORD", true, " s3cret "),
            value("SITE_URL", false, "https://shop.example.com"),
        ];
        let entries = parse(Locale::En, &values).unwrap();
        assert_eq!(entries.len(), 2);
        assert_eq!(entries[0].key.as_str(), "DB_PASSWORD");
        assert_eq!(entries[0].value.as_deref(), Some("s3cret"));
        assert!(entries[0].secret);
        assert!(entries[1].value.as_deref().unwrap().starts_with("https://"));
        assert!(!entries[1].secret);
    }

    #[test]
    fn an_empty_value_is_an_error() {
        let values = vec![
            value("DB_PASSWORD", true, "s3cret"),
            value("SITE_URL", false, "  "),
        ];
        assert_eq!(
            parse(Locale::En, &values),
            Err(Locale::En.t("apps.store_env_required").to_string())
        );
        assert_eq!(parse(Locale::En, &[]).unwrap(), Vec::new());
    }

    #[test]
    fn an_invalid_key_is_a_translated_error() {
        let values = vec![value("NO SPACES", false, "x")];
        assert!(parse(Locale::En, &values).is_err());
    }
}
