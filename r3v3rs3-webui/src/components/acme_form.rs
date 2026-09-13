//! The account, challenge and domain name fields that every ACME provider form shares.

use crate::components::http_proxy_config::{
    error_view, parse_comma_list, HINT_CLASS, INPUT_CLASS, LABEL_CLASS,
};
use base64::{engine::general_purpose, Engine};
use r3v3rs3_api::{
    acme::{Acme, AcmeConfig, AcmeRequest, DnsProvider, ExternalAccountBinding, DNS_01, HTTP_01},
    error::Error,
    i18n::Locale,
    subject_name::SubjectName,
};
use std::{
    collections::{HashMap, HashSet},
    rc::Rc,
    str::FromStr,
};
use wasm_bindgen::{JsCast, UnwrapThrowExt};
use web_sys::{HtmlInputElement, HtmlSelectElement};
use yew::prelude::*;

/// The `provider` names of the DNS providers and their labels, which are not translated.
pub const DNS_PROVIDERS: [(&str, &str); 4] = [
    ("cloudflare", "Cloudflare"),
    ("route53", "Route 53"),
    ("digitalocean", "DigitalOcean"),
    ("hetzner", "Hetzner Cloud"),
];

pub type AcmeResult = Result<AcmeRequest, HashMap<String, String>>;

/// The labels of the EAB fields. `None` hides the fields.
pub type EabLabels = Option<(&'static str, &'static str)>;

#[derive(Clone, PartialEq)]
pub struct AcmeFields {
    pub eab_kid: String,
    pub eab_hmac_key: String,
    pub email: String,
    pub domain_names: String,
    pub challenge_type: String,
    pub dns_provider: String,
    pub api_token: String,
    pub access_key_id: String,
    pub secret_access_key: String,
    /// The error keys of the fields that the user changed.
    pub touched: HashSet<&'static str>,
}

impl Default for AcmeFields {
    fn default() -> Self {
        Self {
            eab_kid: String::new(),
            eab_hmac_key: String::new(),
            email: String::new(),
            domain_names: String::new(),
            challenge_type: HTTP_01.to_string(),
            dns_provider: DNS_PROVIDERS[0].0.to_string(),
            api_token: String::new(),
            access_key_id: String::new(),
            secret_access_key: String::new(),
            touched: HashSet::new(),
        }
    }
}

pub struct FieldChange {
    key: &'static str,
    set: fn(&mut AcmeFields, String),
    value: String,
}

impl Reducible for AcmeFields {
    type Action = FieldChange;

    fn reduce(self: Rc<Self>, action: FieldChange) -> Rc<Self> {
        let mut fields = (*self).clone();
        (action.set)(&mut fields, action.value);
        fields.touched.insert(action.key);
        fields.into()
    }
}

/// Returns a callback that stores the value of an input or a select element and marks `key` as touched.
/// A reducer applies the change to the latest fields, so two quick changes do not overwrite each other.
fn setter(
    fields: &UseReducerHandle<AcmeFields>,
    key: &'static str,
    set: fn(&mut AcmeFields, String),
) -> Callback<Event> {
    let fields = fields.clone();
    Callback::from(move |event: Event| {
        let target = event.target().unwrap_throw();
        let value = match target.dyn_ref::<HtmlInputElement>() {
            Some(input) => input.value(),
            None => target.unchecked_into::<HtmlSelectElement>().value(),
        };
        fields.dispatch(FieldChange { key, set, value });
    })
}

/// The label of a DNS provider name. An unknown name is shown as it is.
pub fn dns_provider_label(name: &str) -> &str {
    DNS_PROVIDERS
        .iter()
        .find(|(provider, _)| *provider == name)
        .map_or(name, |(_, label)| label)
}

/// Shows the error of `key` after the user changed the field or tried to submit the form.
fn field_error(
    fields: &AcmeFields,
    errors: &HashMap<String, String>,
    show_errors: bool,
    key: &str,
) -> Html {
    let visible = show_errors || fields.touched.contains(key);
    error_view(errors.get(key).filter(|_| visible))
}

/// The EAB, email, challenge and domain name fields.
pub fn fields_view(
    locale: Locale,
    fields: &UseReducerHandle<AcmeFields>,
    errors: &HashMap<String, String>,
    show_errors: bool,
    eab: EabLabels,
) -> Html {
    html! {
        <>
            { eab_view(locale, fields, errors, show_errors, eab) }

            <label class={LABEL_CLASS}>{locale.t("acme.email")}</label>
            <input type="email" placeholder="admin@example.com" value={fields.email.clone()} onchange={setter(fields, "email", |f, v| f.email = v)} class={INPUT_CLASS} />
            { field_error(fields, errors, show_errors, "email") }

            { challenge_view(locale, fields, errors, show_errors) }

            <label class={LABEL_CLASS}>{locale.t("acme.domain_names")}</label>
            <input type="text" autocapitalize="off" placeholder="example.com, *.example.com" value={fields.domain_names.clone()} onchange={setter(fields, "domain_names", |f, v| f.domain_names = v)} class={INPUT_CLASS} />
            { field_error(fields, errors, show_errors, "domain_names") }
            <p class={HINT_CLASS}>{locale.t("acme.domain_names_hint")}</p>
        </>
    }
}

fn eab_view(
    locale: Locale,
    fields: &UseReducerHandle<AcmeFields>,
    errors: &HashMap<String, String>,
    show_errors: bool,
    eab: EabLabels,
) -> Html {
    let Some((key_id_label, hmac_key_label)) = eab else {
        return html! {};
    };
    html! {
        <>
            <label class={LABEL_CLASS}>{locale.t(key_id_label)}</label>
            <input type="text" value={fields.eab_kid.clone()} onchange={setter(fields, "eab_kid", |f, v| f.eab_kid = v)} class={INPUT_CLASS} />
            { field_error(fields, errors, show_errors, "eab_kid") }

            <label class={LABEL_CLASS}>{locale.t(hmac_key_label)}</label>
            <input type="password" autocomplete="off" value={fields.eab_hmac_key.clone()} onchange={setter(fields, "eab_hmac_key", |f, v| f.eab_hmac_key = v)} class={INPUT_CLASS} />
            { field_error(fields, errors, show_errors, "eab_hmac_key") }
        </>
    }
}

fn challenge_view(
    locale: Locale,
    fields: &UseReducerHandle<AcmeFields>,
    errors: &HashMap<String, String>,
    show_errors: bool,
) -> Html {
    let challenge = fields.challenge_type.as_str();
    html! {
        <>
            <label class={LABEL_CLASS}>{locale.t("acme.challenge")}</label>
            <select onchange={setter(fields, "challenge_type", |f, v| f.challenge_type = v)} class={INPUT_CLASS}>
                <option selected={challenge == HTTP_01} value={HTTP_01}>{"HTTP-01"}</option>
                <option selected={challenge == DNS_01} value={DNS_01}>{"DNS-01"}</option>
            </select>
            <p class={HINT_CLASS}>{locale.t("acme.challenge_hint")}</p>

            if challenge == DNS_01 {
                { dns_provider_view(locale, fields, errors, show_errors) }
            }
        </>
    }
}

fn dns_provider_view(
    locale: Locale,
    fields: &UseReducerHandle<AcmeFields>,
    errors: &HashMap<String, String>,
    show_errors: bool,
) -> Html {
    let provider = fields.dns_provider.as_str();
    let credentials = if provider == "route53" {
        html! {
            <>
                <label class={LABEL_CLASS}>{locale.t("acme.access_key_id")}</label>
                <input type="text" autocomplete="off" value={fields.access_key_id.clone()} onchange={setter(fields, "dns_provider", |f, v| f.access_key_id = v)} class={INPUT_CLASS} />

                <label class={LABEL_CLASS}>{locale.t("acme.secret_access_key")}</label>
                <input type="password" autocomplete="off" value={fields.secret_access_key.clone()} onchange={setter(fields, "dns_provider", |f, v| f.secret_access_key = v)} class={INPUT_CLASS} />
            </>
        }
    } else {
        html! {
            <>
                <label class={LABEL_CLASS}>{locale.t("acme.api_token")}</label>
                <input type="password" autocomplete="off" value={fields.api_token.clone()} onchange={setter(fields, "dns_provider", |f, v| f.api_token = v)} class={INPUT_CLASS} />
            </>
        }
    };
    html! {
        <>
            <label class={LABEL_CLASS}>{locale.t("acme.dns_provider")}</label>
            <select onchange={setter(fields, "dns_provider", |f, v| f.dns_provider = v)} class={INPUT_CLASS}>
                { DNS_PROVIDERS.iter().map(|(name, label)| html! {
                    <option selected={provider == *name} value={*name}>{*label}</option>
                }).collect::<Html>() }
            </select>

            { credentials }
            { field_error(fields, errors, show_errors, "dns_provider") }
            <p class={HINT_CLASS}>{locale.t("acme.dns_credentials_hint")}</p>
        </>
    }
}

/// Builds the request from the fields. `errors` holds the errors of the fields that the caller
/// checked before, and every error message is in the selected language.
pub fn build_request(
    locale: Locale,
    fields: &AcmeFields,
    eab: bool,
    server_url: &str,
    config: AcmeConfig,
    mut errors: HashMap<String, String>,
) -> AcmeResult {
    let eab = if eab {
        parse_eab(locale, fields, &mut errors)
    } else {
        None
    };
    let email = fields.email.trim();
    if email.is_empty() {
        errors.insert("email".into(), locale.t("acme.email_required").into());
    }
    let identifiers = parse_comma_list(
        locale,
        &fields.domain_names,
        "domain_names",
        &mut errors,
        SubjectName::from_str,
    );
    let acme = Acme {
        config,
        identifiers,
        challenge_type: fields.challenge_type.clone(),
        dns_provider: dns_provider(fields),
    };
    if let Err(err) = acme.validate() {
        let key = match err {
            Error::AcmeDnsProviderRequired => "dns_provider",
            _ => "domain_names",
        };
        errors
            .entry(key.into())
            .or_insert_with(|| locale.error_message(&err));
    }
    if !errors.is_empty() {
        return Err(errors);
    }
    Ok(AcmeRequest {
        server_url: server_url.to_string(),
        contacts: vec![format!("mailto:{email}")],
        eab,
        acme,
    })
}

/// The external account binding, or `None` when both EAB fields are empty.
fn parse_eab(
    locale: Locale,
    fields: &AcmeFields,
    errors: &mut HashMap<String, String>,
) -> Option<ExternalAccountBinding> {
    let key_id = fields.eab_kid.trim();
    let hmac_key = fields.eab_hmac_key.trim();
    if key_id.is_empty() && hmac_key.is_empty() {
        return None;
    }
    if key_id.is_empty() {
        errors.insert(
            "eab_kid".into(),
            locale.t("acme.eab_key_id_required").into(),
        );
    }
    let hmac_key = general_purpose::URL_SAFE_NO_PAD
        .decode(hmac_key.as_bytes())
        .unwrap_or_else(|_| {
            errors.insert(
                "eab_hmac_key".into(),
                locale.t("acme.invalid_eab_hmac_key").into(),
            );
            Vec::new()
        });
    Some(ExternalAccountBinding {
        key_id: key_id.to_string(),
        hmac_key,
    })
}

/// The DNS provider of the `dns-01` challenge with the credentials of the selected provider.
fn dns_provider(fields: &AcmeFields) -> Option<DnsProvider> {
    if fields.challenge_type != DNS_01 {
        return None;
    }
    let api_token = fields.api_token.trim().to_string();
    Some(match fields.dns_provider.as_str() {
        "route53" => DnsProvider::Route53 {
            access_key_id: fields.access_key_id.trim().to_string(),
            secret_access_key: fields.secret_access_key.trim().to_string(),
            api_url: None,
        },
        "digitalocean" => DnsProvider::DigitalOcean {
            api_token,
            api_url: None,
        },
        "hetzner" => DnsProvider::Hetzner {
            api_token,
            api_url: None,
        },
        _ => DnsProvider::Cloudflare {
            api_token,
            api_url: None,
        },
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    const SERVER_URL: &str = "https://acme.example.test/directory";

    fn fields(domain_names: &str, challenge_type: &str) -> AcmeFields {
        AcmeFields {
            email: "admin@example.com".into(),
            domain_names: domain_names.into(),
            challenge_type: challenge_type.into(),
            ..AcmeFields::default()
        }
    }

    fn build(fields: &AcmeFields, eab: bool) -> AcmeResult {
        build_request(
            Locale::En,
            fields,
            eab,
            SERVER_URL,
            AcmeConfig::default(),
            HashMap::new(),
        )
    }

    #[test]
    fn domain_names_are_split_at_commas_and_trimmed() {
        let request = build(&fields(" example.com , ,www.example.com ", HTTP_01), false).unwrap();
        assert_eq!(
            request.acme.identifiers,
            vec![
                SubjectName::from_str("example.com").unwrap(),
                SubjectName::from_str("www.example.com").unwrap()
            ]
        );
        assert_eq!(request.acme.dns_provider, None);
        assert_eq!(request.contacts, vec!["mailto:admin@example.com"]);
        assert_eq!(request.eab, None);
    }

    #[test]
    fn a_wildcard_with_http_01_is_a_domain_name_error() {
        let errors = build(&fields("example.com, *.example.com", HTTP_01), false).unwrap_err();
        let expected = Locale::En.error_message(&Error::AcmeWildcardNeedsDnsChallenge {
            identifier: "*.example.com".into(),
        });
        assert_eq!(errors.get("domain_names"), Some(&expected));
    }

    #[test]
    fn dns_01_sends_the_selected_provider_and_its_credentials() {
        let mut input = fields("example.com, *.example.com", DNS_01);
        input.dns_provider = "route53".into();
        input.access_key_id = " AKIDTEST ".into();
        input.secret_access_key = "secret".into();
        input.api_token = "unused".into();

        let request = build(&input, false).unwrap();
        assert_eq!(request.acme.challenge_type, DNS_01);
        assert_eq!(
            request.acme.dns_provider,
            Some(DnsProvider::Route53 {
                access_key_id: "AKIDTEST".into(),
                secret_access_key: "secret".into(),
                api_url: None,
            })
        );
    }

    #[test]
    fn dns_01_without_credentials_is_a_provider_error() {
        let mut input = fields("example.com", DNS_01);
        input.dns_provider = "hetzner".into();
        let errors = build(&input, false).unwrap_err();
        assert_eq!(
            errors.get("dns_provider"),
            Some(&Locale::En.error_message(&Error::AcmeDnsProviderRequired))
        );
    }

    #[test]
    fn missing_values_have_translated_errors() {
        let errors = build(&AcmeFields::default(), false).unwrap_err();
        assert_eq!(
            errors.get("email").map(String::as_str),
            Some(Locale::En.t("acme.email_required"))
        );
        assert_eq!(
            errors.get("domain_names"),
            Some(&Locale::En.error_message(&Error::AcmeIdentifiersMissing))
        );
    }

    #[test]
    fn eab_is_checked_only_when_a_field_is_filled() {
        let mut input = fields("example.com", HTTP_01);
        assert_eq!(build(&input, true).unwrap().eab, None);

        input.eab_hmac_key = "not base64!".into();
        let errors = build(&input, true).unwrap_err();
        assert!(errors.contains_key("eab_kid"));
        assert_eq!(
            errors.get("eab_hmac_key").map(String::as_str),
            Some(Locale::En.t("acme.invalid_eab_hmac_key"))
        );

        input.eab_kid = "kid".into();
        input.eab_hmac_key = "c2VjcmV0".into();
        let eab = build(&input, true).unwrap().eab.unwrap();
        assert_eq!(eab.key_id, "kid");
        assert_eq!(eab.hmac_key, b"secret");
    }

    #[test]
    fn provider_labels_fall_back_to_the_name() {
        assert_eq!(dns_provider_label("route53"), "Route 53");
        assert_eq!(dns_provider_label("other"), "other");
    }
}
