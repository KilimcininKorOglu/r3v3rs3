use crate::components::acme_form::{build_request, fields_view, AcmeFields, AcmeResult};
use crate::components::http_proxy_config::{
    error_view, input_element, text_setter, use_entry_errors, INPUT_CLASS, LABEL_CLASS,
};
use crate::i18n::use_locale;
use r3v3rs3_api::{acme::AcmeConfig, i18n::Locale};
use std::collections::HashMap;
use url::Url;
use yew::prelude::*;

const DEFAULT_RENEWAL_DAYS: u64 = 60;

#[derive(Properties, PartialEq)]
pub struct Props {
    #[prop_or_default]
    pub show_errors: bool,
    pub onchanged: Callback<AcmeResult>,
}

#[function_component(CustomAcme)]
pub fn custom_acme(props: &Props) -> Html {
    let locale = use_locale();
    let name = use_state(String::new);
    let server_url = use_state(String::new);
    let fields = use_reducer_eq(AcmeFields::default);

    let renewal = use_state(|| DEFAULT_RENEWAL_DAYS);
    let renewal_onchange = Callback::from({
        let renewal = renewal.clone();
        move |event: Event| {
            let value = input_element(&event).value();
            renewal.set(value.parse().unwrap_or(DEFAULT_RENEWAL_DAYS));
        }
    });

    let entry = get_request(locale, &name, &server_url, &fields, *renewal);
    let errors = use_entry_errors(entry, props.onchanged.clone());
    let show = |key: &str, value: &str| {
        error_view(
            errors
                .get(key)
                .filter(|_| props.show_errors || !value.is_empty()),
        )
    };
    let eab = Some(("acme.eab_key_id_optional", "acme.eab_hmac_key_optional"));

    html! {
        <>
            <label class={LABEL_CLASS}>{locale.t("common.name")}</label>
            <input type="text" placeholder={locale.t("acme.name_placeholder")} onchange={text_setter(&name)} class={INPUT_CLASS} />
            { show("name", &name) }

            <label class={LABEL_CLASS}>{locale.t("acme.server_url")}</label>
            <input type="url" placeholder="https://example.com/" onchange={text_setter(&server_url)} class={INPUT_CLASS} />
            { show("server_url", &server_url) }

            { fields_view(locale, &fields, &errors, props.show_errors, eab) }

            <label class={LABEL_CLASS}>{locale.t("acme.renewal_interval")}</label>
            <input type="number" value={renewal.to_string()} min="1" onchange={renewal_onchange} class={INPUT_CLASS} />
        </>
    }
}

fn get_request(
    locale: Locale,
    name: &str,
    server_url: &str,
    fields: &AcmeFields,
    renewal: u64,
) -> AcmeResult {
    let mut errors = HashMap::new();
    if name.trim().is_empty() {
        errors.insert("name".into(), locale.t("acme.name_required").into());
    }
    if Url::parse(server_url).is_err() {
        errors.insert(
            "server_url".into(),
            locale.t("acme.invalid_server_url").into(),
        );
    }
    let config = AcmeConfig {
        active: true,
        provider: name.trim().to_string(),
        renewal_days: renewal,
    };
    build_request(locale, fields, true, server_url, config, errors)
}

#[cfg(test)]
mod tests {
    use super::*;
    use r3v3rs3_api::acme::DNS_01;

    #[test]
    fn name_and_server_url_are_checked_with_the_shared_fields() {
        let errors =
            get_request(Locale::En, " ", "not a url", &AcmeFields::default(), 30).unwrap_err();
        assert_eq!(
            errors.get("name").map(String::as_str),
            Some(Locale::En.t("acme.name_required"))
        );
        assert_eq!(
            errors.get("server_url").map(String::as_str),
            Some(Locale::En.t("acme.invalid_server_url"))
        );
        assert!(errors.contains_key("email"));
    }

    #[test]
    fn the_request_keeps_the_name_and_the_renewal_interval() {
        let fields = AcmeFields {
            email: "admin@example.com".into(),
            domain_names: "*.example.com".into(),
            challenge_type: DNS_01.into(),
            api_token: "token".into(),
            ..AcmeFields::default()
        };
        let request = get_request(
            Locale::En,
            " Pebble ",
            "https://localhost:14000/dir",
            &fields,
            30,
        )
        .unwrap();
        assert_eq!(request.acme.config.provider, "Pebble");
        assert_eq!(request.acme.config.renewal_days, 30);
        assert_eq!(request.server_url, "https://localhost:14000/dir");
    }
}
