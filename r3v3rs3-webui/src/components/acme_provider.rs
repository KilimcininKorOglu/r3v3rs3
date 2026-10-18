use crate::components::acme_form::{
    AcmeFields, AcmeResult, build_request, fields_view, use_dns_credentials,
};
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
    pub eab_optional: bool,
    #[prop_or_default]
    pub show_errors: bool,
    pub onchanged: Callback<AcmeResult>,
}

#[function_component(AcmeProvider)]
pub fn acme_provider(props: &Props) -> Html {
    let locale = use_locale();
    let fields = use_reducer_eq(AcmeFields::default);
    let credentials = use_dns_credentials();

    let config = AcmeConfig {
        active: true,
        provider: props.name.clone(),
        renewal_days: 60,
    };
    let entry = build_request(
        locale,
        &fields,
        props.eab || props.eab_optional,
        &props.url,
        config,
        HashMap::new(),
    );
    let errors = use_entry_errors(entry, props.onchanged.clone());
    let eab = if props.eab {
        Some(("acme.eab_key_id", "acme.eab_hmac_key"))
    } else if props.eab_optional {
        Some(("acme.eab_key_id_optional", "acme.eab_hmac_key_optional"))
    } else {
        None
    };
    fields_view(
        locale,
        &fields,
        &errors,
        props.show_errors,
        eab,
        &credentials,
    )
}
