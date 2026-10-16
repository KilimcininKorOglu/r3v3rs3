//! The variables of an app store entry that the user fills before the first deployment.

use super::accounts::{HINT_CLASS, INPUT_CLASS, LABEL_CLASS};
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
            <label class={LABEL_CLASS}>{locale.t("apps.store_env")}</label>
            {rows}
            <p class={HINT_CLASS}>{locale.t("apps.store_env_hint")}</p>
        </>
    }
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
