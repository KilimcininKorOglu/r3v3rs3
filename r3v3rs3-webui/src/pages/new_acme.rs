use crate::components::acme_form::AcmeResult;
use crate::components::acme_provider::AcmeProvider;
use crate::components::custom_acme::CustomAcme;
use crate::components::http_proxy_config::error_view;
use crate::pages::cert_list::{CertsQuery, CertsTab};
use crate::pages::settings::send_json;
use crate::{API_ENDPOINT, auth::use_ensure_auth, i18n::use_locale, pages::Route};
use gloo_net::http::Request;
use r3v3rs3_api::acme::AcmeRequest;
use r3v3rs3_api::i18n::Locale;
use std::fmt::Display;
use wasm_bindgen::{JsCast, UnwrapThrowExt};
use web_sys::HtmlSelectElement;
use yew::prelude::*;
use yew_router::prelude::*;

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Provider {
    LetsEncrypt,
    GoogleTrustServices,
    ZeroSSL,
    Custom,
}

impl Provider {
    fn html(&self, onchanged: Callback<AcmeResult>, show_errors: bool) -> Html {
        match self {
            Provider::LetsEncrypt => {
                html! { <AcmeProvider name={self.to_string()} url={"https://acme-v02.api.letsencrypt.org/directory"} {show_errors} {onchanged} /> }
            }
            Provider::GoogleTrustServices => {
                html! { <AcmeProvider name={self.to_string()} eab={true} url={"https://dv.acme-v02.api.pki.goog/directory"} {show_errors} {onchanged} /> }
            }
            Provider::ZeroSSL => {
                html! { <AcmeProvider name={self.to_string()} eab={true} url={"https://acme.zerossl.com/v2/DV90"} {show_errors} {onchanged} /> }
            }
            Provider::Custom => html! { <CustomAcme {show_errors} {onchanged} /> },
        }
    }

    /// Provider names are not translated. They are stored with the ACME configuration.
    fn label(self, locale: Locale) -> String {
        match self {
            Provider::Custom => locale.t("acme.custom").to_string(),
            _ => self.to_string(),
        }
    }
}

impl Display for Provider {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Provider::LetsEncrypt => write!(f, "Let's Encrypt"),
            Provider::GoogleTrustServices => write!(f, "Google Trust Services"),
            Provider::ZeroSSL => write!(f, "ZeroSSL"),
            Provider::Custom => write!(f, "Custom"),
        }
    }
}

const PROVIDERS: &[Provider] = &[
    Provider::LetsEncrypt,
    Provider::GoogleTrustServices,
    Provider::ZeroSSL,
    Provider::Custom,
];

#[function_component(NewAcme)]
pub fn new_acme() -> Html {
    use_ensure_auth();
    let locale = use_locale();

    let navigator = use_navigator().unwrap();

    let navigator_cloned = navigator.clone();
    let cancel_onclick = Callback::from(move |_| show_acme(&navigator_cloned));

    let entry = use_state::<AcmeResult, _>(|| Err(Default::default()));
    let entry_cloned = entry.clone();
    let onchanged: Callback<AcmeResult> = Callback::from(move |updated| {
        entry_cloned.set(updated);
    });

    let is_loading = use_state(|| false);
    let show_errors = use_state(|| false);
    let submit_error = use_state(|| Option::<String>::None);

    let onsubmit = submit_callback(
        locale,
        entry.clone(),
        AcmeStates {
            is_loading: is_loading.clone(),
            show_errors: show_errors.clone(),
            submit_error: submit_error.clone(),
        },
        navigator,
    );

    let provider = use_state(|| PROVIDERS[0]);
    let provider_onchange = provider_onchange(provider.clone());

    html! {
        <>
            <form {onsubmit} class="bg-white dark:bg-neutral-800 shadow-sm p-5 border border-neutral-300 dark:border-neutral-700 rounded-md">
                <label class="block mt-4 mb-2 text-sm font-medium text-neutral-900 dark:text-neutral-200">{locale.t("certs.provider")}</label>
                <select onchange={provider_onchange} class="bg-neutral-50 dark:text-neutral-200 dark:bg-neutral-800 dark:border-neutral-600 border border-neutral-300 text-neutral-900 text-sm rounded-lg focus:ring-blue-500 focus:border-blue-500 block w-full p-2.5">
                    { provider_options(locale, &provider) }
                </select>

                { provider.html(onchanged, *show_errors) }

                { error_view(submit_error.as_ref()) }

                <div class="flex flex-col-reverse gap-2 mt-4 sm:flex-row sm:items-center sm:justify-end">
                    <button type="button" onclick={cancel_onclick} class="inline-flex justify-center items-center text-neutral-500 bg-neutral-50 dark:text-neutral-200 dark:bg-neutral-800 focus:outline-none hover:bg-neutral-100 hover:dark:bg-neutral-900 focus:ring-4 focus:ring-neutral-200 dark:focus:ring-neutral-600 font-medium rounded-lg text-sm px-4 py-2">
                        {locale.t("common.cancel")}
                    </button>
                    <button disabled={*is_loading} type="submit" class="inline-flex justify-center items-center text-neutral-500 bg-neutral-50 dark:text-neutral-200 dark:bg-neutral-800 border border-neutral-300 dark:border-neutral-600 focus:outline-none hover:bg-neutral-100 hover:dark:bg-neutral-900 focus:ring-4 focus:ring-neutral-200 dark:focus:ring-neutral-600 font-medium rounded-lg text-sm px-4 py-2">
                        if *is_loading {
                            <svg aria-hidden="true" role="status" class="inline w-4 h-4 mr-3 text-neutral-200 animate-spin dark:text-neutral-600" viewBox="0 0 100 101" fill="none" xmlns="http://www.w3.org/2000/svg">
                            <path d="M100 50.5908C100 78.2051 77.6142 100.591 50 100.591C22.3858 100.591 0 78.2051 0 50.5908C0 22.9766 22.3858 0.59082 50 0.59082C77.6142 0.59082 100 22.9766 100 50.5908ZM9.08144 50.5908C9.08144 73.1895 27.4013 91.5094 50 91.5094C72.5987 91.5094 90.9186 73.1895 90.9186 50.5908C90.9186 27.9921 72.5987 9.67226 50 9.67226C27.4013 9.67226 9.08144 27.9921 9.08144 50.5908Z" fill="#ccc"/>
                            <path d="M93.9676 39.0409C96.393 38.4038 97.8624 35.9116 97.0079 33.5539C95.2932 28.8227 92.871 24.3692 89.8167 20.348C85.8452 15.1192 80.8826 10.7238 75.2124 7.41289C69.5422 4.10194 63.2754 1.94025 56.7698 1.05124C51.7666 0.367541 46.6976 0.446843 41.7345 1.27873C39.2613 1.69328 37.813 4.19778 38.4501 6.62326C39.0873 9.04874 41.5694 10.4717 44.0505 10.1071C47.8511 9.54855 51.7191 9.52689 55.5402 10.0491C60.8642 10.7766 65.9928 12.5457 70.6331 15.2552C75.2735 17.9648 79.3347 21.5619 82.5849 25.841C84.9175 28.9121 86.7997 32.2913 88.1811 35.8758C89.083 38.2158 91.5421 39.6781 93.9676 39.0409Z" fill="#1C64F2"/>
                            </svg>
                        }
                        {locale.t("acme.request")}
                    </button>
                </div>
            </form>
        </>
    }
}

/// Opens the ACME tab of the certificate list.
fn show_acme(navigator: &Navigator) {
    let _ = navigator.push_with_query(
        &Route::Certs,
        &CertsQuery {
            tab: CertsTab::Acme,
        },
    );
}

/// The states of the form that the submit changes.
struct AcmeStates {
    is_loading: UseStateHandle<bool>,
    show_errors: UseStateHandle<bool>,
    submit_error: UseStateHandle<Option<String>>,
}

/// Requests the certificate and opens the ACME tab. A submit during a request does nothing.
fn submit_callback(
    locale: Locale,
    entry: UseStateHandle<AcmeResult>,
    states: AcmeStates,
    navigator: Navigator,
) -> Callback<SubmitEvent> {
    Callback::from(move |event: SubmitEvent| {
        event.prevent_default();
        states.show_errors.set(true);
        if *states.is_loading {
            return;
        }
        let Ok(entry) = (*entry).clone() else {
            return;
        };
        let navigator = navigator.clone();
        let is_loading = states.is_loading.clone();
        let submit_error = states.submit_error.clone();
        is_loading.set(true);
        submit_error.set(None);
        wasm_bindgen_futures::spawn_local(async move {
            match add_acme(locale, &entry).await {
                Ok(()) => show_acme(&navigator),
                Err(message) => submit_error.set(Some(
                    locale.tf("acme.request_failed", &[("message", &message)]),
                )),
            }
            is_loading.set(false);
        });
    })
}

fn provider_onchange(provider: UseStateHandle<Provider>) -> Callback<Event> {
    Callback::from(move |event: Event| {
        let target: HtmlSelectElement = event.target().unwrap_throw().dyn_into().unwrap_throw();
        if let Ok(index) = target.value().parse::<usize>() {
            provider.set(PROVIDERS[index]);
        }
    })
}

fn provider_options(locale: Locale, selected: &Provider) -> Html {
    PROVIDERS
        .iter()
        .enumerate()
        .map(|(i, item)| {
            html! {
                <option selected={selected == item} value={i.to_string()}>{item.label(locale)}</option>
            }
        })
        .collect()
}

async fn add_acme(locale: Locale, req: &AcmeRequest) -> Result<(), String> {
    send_json(locale, Request::post(&format!("{API_ENDPOINT}/acme")), req).await
}
