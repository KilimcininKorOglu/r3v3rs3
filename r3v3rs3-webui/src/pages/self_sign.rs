use crate::{
    API_ENDPOINT,
    auth::use_ensure_auth,
    components::http_proxy_config::{
        HINT_CLASS, INPUT_CLASS, LABEL_CLASS, error_view, select_field, select_setter,
    },
    i18n::use_locale,
    pages::{
        Route,
        cert_list::{CertsQuery, CertsTab, get_cert_list},
    },
};
use gloo_net::http::Request;
use r3v3rs3_api::{
    cert::{CertInfo, CertKind, SelfSignedCertKind, SelfSignedCertRequest},
    i18n::Locale,
    id::ShortId,
    subject_name::SubjectName,
};
use serde_derive::{Deserialize, Serialize};
use std::str::FromStr;
use wasm_bindgen::{JsCast, UnwrapThrowExt};
use web_sys::HtmlInputElement;
use yew::prelude::*;
use yew_router::prelude::*;

const GENERATE_CA: &str = "generate";

#[derive(Default, Clone, Serialize, Deserialize)]
pub struct SelfSignQuery {
    #[serde(default)]
    pub kind: SelfSignedCertKind,
}

#[function_component(SelfSign)]
pub fn self_sign() -> Html {
    use_ensure_auth();
    let locale = use_locale();

    let location = use_location().unwrap();
    let query = location.query::<SelfSignQuery>().unwrap_or_default();
    let kind = use_state(|| query.kind);

    let navigator = use_navigator().unwrap();
    let cancel_onclick = Callback::from({
        let navigator = navigator.clone();
        let kind = *kind;
        move |_| show_certs(&navigator, kind)
    });

    let san = use_state(String::new);
    let san_onchange = Callback::from({
        let san = san.clone();
        move |event: Event| {
            let target: HtmlInputElement = event.target().unwrap_throw().dyn_into().unwrap_throw();
            san.set(target.value());
        }
    });

    let (ca_cert, ca_cert_list) = use_ca_certs();

    let validation = use_state(|| false);
    let entry = get_request(locale, &san, *ca_cert, *kind);
    let error = entry.as_ref().err().filter(|_| *validation).cloned();
    let is_loading = use_state(|| false);

    let onsubmit = submit_callback(entry, validation, is_loading, navigator);

    html! {
        <>
            <form {onsubmit} class="bg-white dark:bg-neutral-800 shadow-sm p-5 border border-neutral-300 dark:border-neutral-700 rounded-md">
                { kind_view(locale, &kind) }

                <label class={LABEL_CLASS}>{locale.t("certs.san")}</label>
                <input type="text" value={san.to_string()} onchange={san_onchange} class={INPUT_CLASS} placeholder="example.com" />
                { error_view(error.as_ref()) }
                <p class={HINT_CLASS}>{locale.t("certs.san_hint")}</p>

                { ca_cert_view(locale, &ca_cert, &ca_cert_list) }

                <div class="flex flex-col-reverse gap-2 mt-4 sm:flex-row sm:items-center sm:justify-end">
                    <button type="button" onclick={cancel_onclick} class="inline-flex justify-center items-center text-neutral-500 bg-neutral-50 dark:text-neutral-200 dark:bg-neutral-800 focus:outline-none hover:bg-neutral-100 hover:dark:bg-neutral-900 focus:ring-4 focus:ring-neutral-200 dark:focus:ring-neutral-600 font-medium rounded-lg text-sm px-4 py-2">
                        {locale.t("common.cancel")}
                    </button>
                    <button type="submit" class="inline-flex justify-center items-center text-neutral-500 bg-neutral-50 dark:text-neutral-200 dark:bg-neutral-800 border border-neutral-300 dark:border-neutral-600 focus:outline-none hover:bg-neutral-100 hover:dark:bg-neutral-900 focus:ring-4 focus:ring-neutral-200 dark:focus:ring-neutral-600 font-medium rounded-lg text-sm px-4 py-2">
                        {locale.t("certs.sign")}
                    </button>
                </div>
            </form>
        </>
    }
}

/// The root certificates that can sign, and the selected one. The default is the first of them.
#[hook]
fn use_ca_certs() -> (UseStateHandle<ShortId>, UseStateHandle<Vec<CertInfo>>) {
    let ca_cert = use_state(|| ShortId::from_str(GENERATE_CA).unwrap_throw());
    let ca_cert_list = use_state(Vec::<CertInfo>::new);
    let ca_cert_cloned = ca_cert.clone();
    let list_cloned = ca_cert_list.clone();
    use_effect_with((), move |_| {
        wasm_bindgen_futures::spawn_local(async move {
            let Ok(res) = get_cert_list().await else {
                return;
            };
            let list = res
                .into_iter()
                .filter(|cert| cert.has_private_key && cert.kind == CertKind::Root)
                .collect::<Vec<_>>();
            if let Some(cert) = list.first() {
                ca_cert_cloned.set(cert.id);
            }
            list_cloned.set(list);
        });
    });
    (ca_cert, ca_cert_list)
}

/// Signs the certificate and opens the certificate tab. A submit during a request does nothing.
fn submit_callback(
    entry: Result<SelfSignedCertRequest, String>,
    validation: UseStateHandle<bool>,
    is_loading: UseStateHandle<bool>,
    navigator: Navigator,
) -> Callback<SubmitEvent> {
    Callback::from(move |event: SubmitEvent| {
        event.prevent_default();
        validation.set(true);
        if *is_loading {
            return;
        }
        let Ok(entry) = entry.clone() else {
            return;
        };
        is_loading.set(true);
        let navigator = navigator.clone();
        let is_loading = is_loading.clone();
        wasm_bindgen_futures::spawn_local(async move {
            if request_self_sign(&entry).await.is_ok() {
                show_certs(&navigator, entry.kind);
            }
            is_loading.set(false);
        });
    })
}

/// Opens the certificate tab that lists certificates of this kind.
fn show_certs(navigator: &Navigator, kind: SelfSignedCertKind) {
    let _ = navigator.push_with_query(
        &Route::Certs,
        &CertsQuery {
            tab: CertsTab::for_kind(kind.into()),
        },
    );
}

fn kind_view(locale: Locale, kind: &UseStateHandle<SelfSignedCertKind>) -> Html {
    let options = html! {
        <>
            <option selected={**kind == SelfSignedCertKind::Server} value="server">{locale.t("certs.kind_server")}</option>
            <option selected={**kind == SelfSignedCertKind::Client} value="client">{locale.t("certs.kind_client")}</option>
        </>
    };
    select_field(
        locale.t("certs.kind"),
        select_setter(kind, parse_kind),
        options,
        Some(locale.t("certs.kind_hint")),
    )
}

fn ca_cert_view(locale: Locale, ca_cert: &UseStateHandle<ShortId>, list: &[CertInfo]) -> Html {
    let options = html! {
        <>
            { list.iter().map(|cert| {
                html! {
                    <option selected={**ca_cert == cert.id} value={cert.id.to_string()}>{format!("{} ({})", cert.issuer, cert.id)}</option>
                }
            }).collect::<Html>() }
            <option selected={ca_cert.to_string() == GENERATE_CA} value={GENERATE_CA}>{locale.t("certs.generate_ca")}</option>
        </>
    };
    let onchange = select_setter(ca_cert, |value| value.parse().unwrap_throw());
    select_field(locale.t("certs.ca_certificate"), onchange, options, None)
}

fn parse_kind(value: &str) -> SelfSignedCertKind {
    if value == "client" {
        SelfSignedCertKind::Client
    } else {
        SelfSignedCertKind::Server
    }
}

/// Builds the request, or returns the translated error of the subject names.
fn get_request(
    locale: Locale,
    san: &str,
    ca_cert: ShortId,
    kind: SelfSignedCertKind,
) -> Result<SelfSignedCertRequest, String> {
    let mut names = Vec::new();
    for name in san.split(',').map(str::trim).filter(|s| !s.is_empty()) {
        names.push(SubjectName::from_str(name).map_err(|err| locale.error_message(&err))?);
    }
    if names.is_empty() {
        return Err(locale.t("certs.san_required").into());
    }
    Ok(SelfSignedCertRequest {
        san: names,
        ca_cert: Some(ca_cert).filter(|id| id.to_string() != GENERATE_CA),
        kind,
    })
}

async fn request_self_sign(req: &SelfSignedCertRequest) -> Result<(), gloo_net::Error> {
    Request::post(&format!("{API_ENDPOINT}/certs/self_sign"))
        .json(&req)?
        .send()
        .await?
        .json()
        .await
}

#[cfg(test)]
mod tests {
    use super::*;

    fn generate() -> ShortId {
        ShortId::from_str(GENERATE_CA).unwrap()
    }

    #[test]
    fn request_trims_names_and_keeps_the_kind() {
        let request = get_request(
            Locale::En,
            "a.example.com, b.example.com",
            generate(),
            parse_kind("client"),
        )
        .unwrap();
        assert_eq!(
            request.san,
            vec![
                SubjectName::from_str("a.example.com").unwrap(),
                SubjectName::from_str("b.example.com").unwrap()
            ]
        );
        assert_eq!(request.ca_cert, None);
        assert_eq!(request.kind, SelfSignedCertKind::Client);
    }

    #[test]
    fn request_needs_a_subject_name() {
        let error = get_request(Locale::En, " , ", generate(), parse_kind("server")).unwrap_err();
        assert_eq!(error, Locale::En.t("certs.san_required"));
    }
}
