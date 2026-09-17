use crate::{
    API_ENDPOINT,
    auth::use_ensure_auth,
    components::{
        http_proxy_config::{select_field, select_setter},
        proxy_config::ProxyConfig,
    },
    i18n::use_locale,
    pages::Route,
};
use gloo_net::http::Request;
use r3v3rs3_api::fixed_response::{FixedRedirect, FixedResponse, FixedStatus};
use r3v3rs3_api::i18n::Locale;
use r3v3rs3_api::proxy::{self, HttpProxy, Proxy, ProxyKind};
use r3v3rs3_api::redirect::RedirectStatus;
use std::collections::HashMap;
use yew::prelude::*;
use yew_router::prelude::*;

/// A template that fills the first route of a new proxy.
#[derive(Debug, Clone, Copy, Default, PartialEq)]
enum Preset {
    #[default]
    Proxy,
    Redirect,
    NotFound,
}

impl Preset {
    const ALL: [Self; 3] = [Self::Proxy, Self::Redirect, Self::NotFound];

    fn as_str(self) -> &'static str {
        match self {
            Self::Proxy => "proxy",
            Self::Redirect => "redirect",
            Self::NotFound => "not_found",
        }
    }

    fn label_key(self) -> &'static str {
        match self {
            Self::Proxy => "proxy_form.preset_proxy",
            Self::Redirect => "proxy_form.preset_redirect",
            Self::NotFound => "proxy_form.preset_not_found",
        }
    }

    fn parse(value: &str) -> Self {
        Self::ALL
            .into_iter()
            .find(|preset| preset.as_str() == value)
            .unwrap_or_default()
    }

    /// The new proxy of the template: a redirect host sends every path to the same path of the
    /// target with 301, and a 404 host answers every request with 404.
    fn proxy(self) -> Proxy {
        let response = match self {
            Self::Proxy => return Proxy::default(),
            Self::Redirect => FixedResponse::Redirect(FixedRedirect {
                target: String::new(),
                status: RedirectStatus::MovedPermanently,
                preserve_path: true,
            }),
            Self::NotFound => FixedResponse::Status(FixedStatus {
                status: 404,
                body: String::new(),
            }),
        };
        let route = proxy::Route {
            path: "/".into(),
            response: Some(response),
            ..Default::default()
        };
        let http = HttpProxy {
            routes: vec![route],
            ..Default::default()
        };
        Proxy {
            kind: ProxyKind::Http(Box::new(http)),
            ..Default::default()
        }
    }
}

#[function_component(NewProxy)]
pub fn new_proxy() -> Html {
    use_ensure_auth();
    let locale = use_locale();

    let navigator = use_navigator().unwrap();
    let preset = use_state(Preset::default);

    let entry = use_state::<Result<Proxy, HashMap<String, String>>, _>(|| Err(Default::default()));
    let entry_cloned = entry.clone();
    let onchanged: Callback<Result<Proxy, HashMap<String, String>>> =
        Callback::from(move |updated| {
            entry_cloned.set(updated);
        });

    let navigator_cloned = navigator.clone();
    let cancel_onclick = Callback::from(move |_| {
        navigator_cloned.push(&Route::Proxies);
    });

    let is_loading = use_state(|| false);

    let entry_cloned = entry.clone();
    let is_loading_cloned = is_loading;
    let onsubmit = Callback::from(move |event: SubmitEvent| {
        event.prevent_default();
        if *is_loading_cloned {
            return;
        }
        let navigator = navigator.clone();
        let is_loading_cloned = is_loading_cloned.clone();
        if let Ok(entry) = (*entry_cloned).clone() {
            is_loading_cloned.set(true);
            wasm_bindgen_futures::spawn_local(async move {
                if create_port(&entry).await.is_ok() {
                    navigator.push(&Route::Proxies);
                }
                is_loading_cloned.set(false);
            });
        }
    });

    html! {
        <>
            <form {onsubmit} class="bg-white dark:bg-neutral-800 shadow-sm p-5 border border-neutral-300 dark:border-neutral-700 rounded-md">
                { preset_field(locale, &preset) }

                <ProxyConfig key={preset.as_str()} proxy={preset.proxy()} {onchanged} />

                <div class="flex flex-col-reverse gap-2 mt-4 sm:flex-row sm:items-center sm:justify-end">
                    <button type="button" onclick={cancel_onclick} class="inline-flex justify-center items-center text-neutral-500 bg-neutral-50 dark:text-neutral-200 dark:bg-neutral-800 focus:outline-none hover:bg-neutral-100 hover:dark:bg-neutral-900 focus:ring-4 focus:ring-neutral-200 dark:focus:ring-neutral-600 font-medium rounded-lg text-sm px-4 py-2">
                        {locale.t("common.cancel")}
                    </button>
                    <button type="submit" disabled={entry.is_err()} class="inline-flex justify-center items-center text-neutral-500 bg-neutral-50 dark:text-neutral-200 dark:bg-neutral-800 border border-neutral-300 dark:border-neutral-600 focus:outline-none hover:bg-neutral-100 hover:dark:bg-neutral-900 focus:ring-4 focus:ring-neutral-200 dark:focus:ring-neutral-600 font-medium rounded-lg text-sm px-4 py-2">
                        {locale.t("common.create")}
                    </button>
                </div>
            </form>
        </>
    }
}

/// The template select. A new template mounts the proxy form again with its proxy.
fn preset_field(locale: Locale, preset: &UseStateHandle<Preset>) -> Html {
    let options = Preset::ALL
        .iter()
        .map(|item| {
            html! {
                <option selected={*item == **preset} value={item.as_str()}>{locale.t(item.label_key())}</option>
            }
        })
        .collect::<Html>();
    html! {
        <div class="mb-6">
            { select_field(
                locale.t("proxy_form.preset"),
                select_setter(preset, Preset::parse),
                options,
                Some(locale.t("proxy_form.preset_hint")),
            ) }
        </div>
    }
}

async fn create_port(entry: &Proxy) -> Result<(), gloo_net::Error> {
    Request::post(&format!("{API_ENDPOINT}/proxies"))
        .json(entry)?
        .send()
        .await?
        .json()
        .await
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn presets_fill_the_first_route_with_a_fixed_response() {
        for preset in Preset::ALL {
            assert_eq!(Preset::parse(preset.as_str()), preset);
        }
        assert_eq!(Preset::Proxy.proxy(), Proxy::default());

        let routes = |preset: Preset| match preset.proxy().kind {
            ProxyKind::Http(http) => http.routes,
            _ => Vec::new(),
        };
        let redirect = routes(Preset::Redirect);
        let Some(FixedResponse::Redirect(response)) = &redirect[0].response else {
            panic!("expected a redirect: {redirect:?}");
        };
        assert_eq!(response.status, RedirectStatus::MovedPermanently);
        assert!(response.preserve_path && redirect[0].servers.is_empty());

        let not_found = routes(Preset::NotFound);
        let expected = FixedResponse::Status(FixedStatus {
            status: 404,
            body: String::new(),
        });
        assert_eq!(not_found[0].response, Some(expected));
    }
}
