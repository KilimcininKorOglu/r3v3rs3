//! The page of the named DNS credentials that the ACME entries reference for `dns-01`.

use super::accounts::{HINT_CLASS, INPUT_CLASS, LABEL_CLASS};
use super::resource_page::{
    FormCard, Notice, delete_on_click, form_card, load, notice_view, reload_callback, row_actions,
    save_result, set_on_click, submit_callback, text_input,
};
use super::settings::send_json;
use crate::API_ENDPOINT;
use crate::auth::use_ensure_auth;
use crate::components::acme_form::{DNS_PROVIDERS, FieldKind, dns_provider_label, provider_info};
use crate::components::data_list::{Column, Row, list_card};
use crate::i18n::use_locale;
use crate::store::SessionStore;
use gloo_net::http::Request;
use r3v3rs3_api::{
    acme::{DnsCredentialInfo, DnsCredentialRequest, DnsProvider},
    error::Error,
    i18n::Locale,
    id::ShortId,
};
use std::collections::HashMap;
use web_sys::HtmlInputElement;
use yew::prelude::*;
use yewdux::prelude::*;

const PATH: &str = "/dns_credentials";

const COLUMNS: [Column; 2] = [
    Column {
        label: "common.name",
        class: "whitespace-nowrap",
    },
    Column {
        label: "acme.dns_provider",
        class: "",
    },
];

type Credentials = UseStateHandle<Option<Vec<DnsCredentialInfo>>>;

#[derive(Clone, PartialEq)]
struct Form {
    /// The id of the edited credential. `None` adds one.
    editing: Option<ShortId>,
    name: String,
    provider: String,
    /// The provider of the stored credential. An unchanged provider with empty fields keeps
    /// its stored secrets.
    orig_provider: Option<String>,
    /// The credentials of the provider by their field names in the API.
    fields: HashMap<&'static str, String>,
}

impl Default for Form {
    fn default() -> Self {
        Self {
            editing: None,
            name: String::new(),
            provider: DNS_PROVIDERS[0].name.to_string(),
            orig_provider: None,
            fields: HashMap::new(),
        }
    }
}

impl Form {
    fn edit(entry: &DnsCredentialInfo) -> Self {
        Self {
            editing: Some(entry.id),
            name: entry.name.clone(),
            provider: entry.provider.clone(),
            orig_provider: Some(entry.provider.clone()),
            fields: HashMap::new(),
        }
    }

    /// Returns the request, or the first error in the selected language.
    fn parse(&self, locale: Locale) -> Result<DnsCredentialRequest, String> {
        let name = self.name.trim();
        if name.is_empty() {
            return Err(locale.error_message(&Error::DnsCredentialNameRequired));
        }
        let dns_provider = self.provider(locale)?;
        Ok(DnsCredentialRequest {
            name: name.to_string(),
            dns_provider,
        })
    }

    /// The provider of the fields. An edit of the same provider with empty fields sends
    /// `None`, which keeps the stored secrets.
    fn provider(&self, locale: Locale) -> Result<Option<DnsProvider>, String> {
        let info = provider_info(&self.provider);
        let unchanged = self.orig_provider.as_deref() == Some(info.name);
        let empty = self.fields.iter().all(|(_, value)| value.trim().is_empty());
        if empty {
            // An unchanged provider keeps its stored secrets. A changed provider has no
            // credentials to store yet.
            return if unchanged {
                Ok(None)
            } else {
                Err(locale.error_message(&Error::AcmeDnsProviderRequired))
            };
        }
        let mut object = serde_json::Map::new();
        object.insert("provider".into(), info.name.into());
        for field in info.fields {
            let value = match (self.fields.get(field.key), &field.kind) {
                (Some(value), _) => value.trim().to_string(),
                (None, FieldKind::Select(options)) => {
                    options.first().copied().unwrap_or_default().to_string()
                }
                (None, _) => String::new(),
            };
            object.insert(field.key.into(), value.into());
        }
        serde_json::from_value(object.into())
            .map(Some)
            .map_err(|_| locale.error_message(&Error::AcmeDnsProviderRequired))
    }
}

#[function_component(DnsCredentials)]
pub fn dns_credentials() -> Html {
    use_ensure_auth();
    let locale = use_locale();
    let (session, _) = use_store::<SessionStore>();
    let credentials = use_state(|| Option::<Vec<DnsCredentialInfo>>::None);
    let form = use_state(Form::default);
    let notice = use_state(|| Option::<Notice<()>>::None);

    let list_cloned = credentials.clone();
    use_effect_with((), move |_| load(list_cloned, PATH));

    let can_edit = session.can_edit();
    let rows = credentials
        .as_deref()
        .unwrap_or_default()
        .iter()
        .map(|entry| list_row(locale, entry, can_edit, &form, &credentials))
        .collect::<Vec<_>>();
    html! {
        <>
            { list_card(locale, credentials.is_some(), "dns_credentials.empty", &COLUMNS, &rows) }
            if can_edit {
                { form_view(locale, &form, &notice, &credentials) }
            }
        </>
    }
}

fn list_row(
    locale: Locale,
    entry: &DnsCredentialInfo,
    can_edit: bool,
    form: &UseStateHandle<Form>,
    credentials: &Credentials,
) -> Row {
    let actions = if can_edit {
        let question = locale.tf("dns_credentials.confirm_delete", &[("name", &entry.name)]);
        let path = format!("{PATH}/{}", entry.id);
        let delete = delete_on_click(locale, question, path, reload_callback(credentials, PATH));
        row_actions(locale, set_on_click(form, Form::edit(entry)), Some(delete))
    } else {
        html! {}
    };
    Row {
        key: entry.id.to_string(),
        cells: vec![
            html! { <>{entry.name.clone()}</> },
            html! { <>{dns_provider_label(&entry.provider)}</> },
        ],
        actions,
    }
}

fn form_view(
    locale: Locale,
    form: &UseStateHandle<Form>,
    notice: &UseStateHandle<Option<Notice<()>>>,
    credentials: &Credentials,
) -> Html {
    let parsed = form.parse(locale);
    let title = match form.editing {
        Some(_) => locale.tf("dns_credentials.edit_title", &[("name", &form.name)]),
        None => locale.t("dns_credentials.add_title").to_string(),
    };
    let touched = form.editing.is_some() || !form.name.is_empty();
    let editing = form.editing.is_some();
    let fields = html! {
        <>
            <label class={LABEL_CLASS}>{locale.t("common.name")}</label>
            <input type="text" autocomplete="off" placeholder="Cloudflare main" value={form.name.clone()} oninput={text_input(form, |f| &mut f.name)} class={INPUT_CLASS} />

            <label class={LABEL_CLASS}>{locale.t("acme.dns_provider")}</label>
            <select oninput={text_input(form, |f| &mut f.provider)} class={INPUT_CLASS}>
                { DNS_PROVIDERS.iter().map(|info| html! {
                    <option selected={info.name == form.provider} value={info.name}>{info.label}</option>
                }).collect::<Html>() }
            </select>

            { fields_of(locale, form) }

            if editing {
                <p class={HINT_CLASS}>{locale.t("dns_credentials.keep_hint")}</p>
            }
        </>
    };
    let card = FormCard {
        title,
        notice: notice_view(
            notice,
            |()| html! { <p>{locale.t("dns_credentials.saved")}</p> },
        ),
        fields,
        error: parsed.as_ref().err().filter(|_| touched).cloned(),
        cancel: form
            .editing
            .is_some()
            .then(|| set_on_click(form, Form::default())),
        save_label: "dns_credentials.save",
        can_save: parsed.is_ok(),
    };
    form_card(locale, card, submit(locale, form, notice, credentials))
}

/// The credential fields of the selected provider.
fn fields_of(locale: Locale, form: &UseStateHandle<Form>) -> Html {
    provider_info(&form.provider)
        .fields
        .iter()
        .map(|field| {
            let value = form.fields.get(field.key).cloned().unwrap_or_default();
            let oninput = {
                let form = form.clone();
                let key = field.key;
                Callback::from(move |event: yew::InputEvent| {
                    let input: HtmlInputElement = event.target_unchecked_into();
                    let mut updated = (*form).clone();
                    updated.fields.insert(key, input.value());
                    form.set(updated);
                })
            };
            let input = match field.kind {
                FieldKind::Select(options) => html! {
                    <select {oninput} class={INPUT_CLASS}>
                        { options.iter().map(|option| html! {
                            <option selected={value == *option} value={*option}>{*option}</option>
                        }).collect::<Html>() }
                    </select>
                },
                FieldKind::Multiline => html! {
                    <textarea rows="4" autocapitalize="off" spellcheck="false" {value} {oninput} class={classes!(INPUT_CLASS, "font-mono")} />
                },
                FieldKind::Secret => html! {
                    <input type="password" autocomplete="off" {value} {oninput} class={INPUT_CLASS} />
                },
                FieldKind::Text => html! {
                    <input type="text" autocomplete="off" {value} {oninput} class={INPUT_CLASS} />
                },
            };
            html! {
                <>
                    <label class={LABEL_CLASS}>{locale.t(field.label)}</label>
                    { input }
                </>
            }
        })
        .collect::<Html>()
}

fn submit(
    locale: Locale,
    form: &UseStateHandle<Form>,
    notice: &UseStateHandle<Option<Notice<()>>>,
    credentials: &Credentials,
) -> Callback<SubmitEvent> {
    let done = save_result(form, notice, reload_callback(credentials, PATH));
    let form = form.clone();
    submit_callback(
        move || {
            form.parse(locale)
                .ok()
                .map(|request| (form.editing, request))
        },
        move |(editing, request)| save(locale, editing, request),
        done,
    )
}

/// Adds the credential, or replaces the credential with the id `editing`.
async fn save(
    locale: Locale,
    editing: Option<ShortId>,
    request: DnsCredentialRequest,
) -> Result<(), String> {
    let builder = match editing {
        Some(id) => Request::put(&format!("{API_ENDPOINT}{PATH}/{id}")),
        None => Request::post(&format!("{API_ENDPOINT}{PATH}")),
    };
    send_json(locale, builder, &request).await
}

#[cfg(test)]
mod tests {
    use super::*;

    fn form(provider: &str, fields: &[(&'static str, &str)]) -> Form {
        Form {
            editing: None,
            name: "Cloudflare main".into(),
            provider: provider.into(),
            orig_provider: None,
            fields: fields
                .iter()
                .map(|(key, value)| (*key, (*value).to_string()))
                .collect(),
        }
    }

    #[test]
    fn a_form_builds_the_provider_of_its_fields() {
        let request = form(
            "route53",
            &[("access_key_id", "AKID"), ("secret_access_key", "s")],
        )
        .parse(Locale::En)
        .unwrap();
        assert_eq!(request.name, "Cloudflare main");
        assert!(request.dns_provider.is_some());
        assert_eq!(
            serde_json::to_value(&request.dns_provider).unwrap()["provider"],
            "route53"
        );
    }

    #[test]
    fn an_empty_edit_of_the_same_provider_keeps_the_stored_secrets() {
        let mut edited = Form::edit(&DnsCredentialInfo {
            id: "abcdef12".parse().unwrap(),
            name: "Cloudflare main".into(),
            provider: "cloudflare".into(),
        });
        edited.name = "Renamed".into();
        let request = edited.parse(Locale::En).unwrap();
        assert_eq!(request.name, "Renamed");
        assert_eq!(request.dns_provider, None);

        let changed = Form {
            provider: "hetzner".into(),
            ..edited
        };
        assert!(changed.parse(Locale::En).is_err());
    }

    #[test]
    fn a_name_is_required() {
        let empty = Form {
            name: " ".into(),
            ..form("cloudflare", &[("api_token", "t")])
        };
        assert_eq!(
            empty.parse(Locale::En),
            Err(Locale::En.error_message(&Error::DnsCredentialNameRequired))
        );
    }
}
