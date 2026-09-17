use crate::components::acme_form::{AcmeFields, AcmeResult, build_request, fields_view};
use crate::components::http_proxy_config::use_entry_errors;
use crate::i18n::use_locale;
use r3v3rs3_api::acme::AcmeConfig;
use std::collections::HashMap;
use yew::prelude::*;

#[derive(Properties, PartialEq)]
pub struct Props {
    pub name: String,
    pub url: String,
    #[prop_or_default]
    pub eab: bool,
    #[prop_or_default]
    pub show_errors: bool,
    pub onchanged: Callback<AcmeResult>,
}

#[function_component(AcmeProvider)]
pub fn acme_provider(props: &Props) -> Html {
    let locale = use_locale();
    let fields = use_reducer_eq(AcmeFields::default);

    let config = AcmeConfig {
        active: true,
        provider: props.name.clone(),
        renewal_days: 60,
    };
    let entry = build_request(
        locale,
        &fields,
        props.eab,
        &props.url,
        config,
        HashMap::new(),
    );
    let errors = use_entry_errors(entry, props.onchanged.clone());
    let eab = props
        .eab
        .then_some(("acme.eab_key_id", "acme.eab_hmac_key"));
    fields_view(locale, &fields, &errors, props.show_errors, eab)
}
