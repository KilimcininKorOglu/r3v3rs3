use crate::API_ENDPOINT;
use crate::components::http_proxy_config::{HINT_CLASS, INPUT_CLASS};
use crate::i18n::use_locale;
use gloo_net::http::Request;
use r3v3rs3_api::i18n::Locale;
use r3v3rs3_api::proxy::{HttpProxy, ProxyDryRunRequest, ProxyDryRunResult};
use std::net::IpAddr;
use wasm_bindgen::{JsCast, UnwrapThrowExt};
use web_sys::{HtmlInputElement, HtmlSelectElement};
use yew::prelude::*;

const HTTP_METHODS: &[&str] = &["GET", "POST", "PUT", "DELETE", "PATCH", "HEAD", "OPTIONS"];

#[derive(Properties, PartialEq)]
pub struct Props {
    #[prop_or_default]
    pub proxy: Option<HttpProxy>,
}

#[function_component(ProxyDryRun)]
pub fn proxy_dry_run(props: &Props) -> Html {
    let locale = use_locale();
    let is_open = use_state(|| false);
    let url = use_state(|| "https://example.com/".to_string());
    let method = use_state(|| "GET".to_string());
    let client_ip = use_state(String::new);
    let is_loading = use_state(|| false);
    let result = use_state::<Option<Result<ProxyDryRunResult, String>>, _>(|| None);

    let toggle_open = {
        let is_open = is_open.clone();
        Callback::from(move |_| is_open.set(!*is_open))
    };

    let on_url_change = {
        let url = url.clone();
        Callback::from(move |event: Event| {
            let target: HtmlInputElement = event.target().unwrap_throw().dyn_into().unwrap_throw();
            url.set(target.value());
        })
    };

    let on_method_change = {
        let method = method.clone();
        Callback::from(move |event: Event| {
            let target: HtmlSelectElement = event.target().unwrap_throw().dyn_into().unwrap_throw();
            method.set(target.value());
        })
    };

    let on_ip_change = {
        let client_ip = client_ip.clone();
        Callback::from(move |event: Event| {
            let target: HtmlInputElement = event.target().unwrap_throw().dyn_into().unwrap_throw();
            client_ip.set(target.value());
        })
    };

    let on_test = {
        let proxy = props.proxy.clone();
        let url = url.clone();
        let method = method.clone();
        let client_ip = client_ip.clone();
        let is_loading = is_loading.clone();
        let result = result.clone();

        Callback::from(move |_| {
            let Some(proxy) = proxy.clone() else {
                result.set(Some(Err(locale
                    .t("http_form.dry_run_form_invalid")
                    .to_string())));
                return;
            };

            let req_body = ProxyDryRunRequest {
                proxy,
                url: (*url).clone(),
                method: Some((*method).clone()),
                client_ip: client_ip.trim().parse::<IpAddr>().ok(),
            };

            let is_loading = is_loading.clone();
            let result = result.clone();

            is_loading.set(true);
            wasm_bindgen_futures::spawn_local(async move {
                execute_dry_run_request(req_body, is_loading, result).await;
            });
        })
    };

    html! {
        <div class="mt-8 border border-neutral-300 dark:border-neutral-700 rounded-lg bg-neutral-50 dark:bg-neutral-800/60 p-5">
            <div class="flex items-center justify-between cursor-pointer select-none" onclick={toggle_open}>
                <div class="flex items-center gap-2">
                    <svg class="w-5 h-5 text-blue-600 dark:text-blue-400" fill="none" stroke="currentColor" viewBox="0 0 24 24">
                        <path stroke-linecap="round" stroke-linejoin="round" stroke-width="2" d="M13 10V3L4 14h7v7l9-11h-7z" />
                    </svg>
                    <span class="text-sm font-semibold text-neutral-900 dark:text-neutral-100">
                        {locale.t("http_form.dry_run_title")}
                    </span>
                    <span class="text-xs px-2 py-0.5 rounded-full bg-blue-100 text-blue-800 dark:bg-blue-900/60 dark:text-blue-300">
                        {"Dry Run"}
                    </span>
                </div>
                <svg class={classes!("w-5", "h-5", "text-neutral-500", "transition-transform", (*is_open).then_some("rotate-180"))} fill="none" stroke="currentColor" viewBox="0 0 24 24">
                    <path stroke-linecap="round" stroke-linejoin="round" stroke-width="2" d="M19 9l-7 7-7-7" />
                </svg>
            </div>

            if *is_open {
                <div class="mt-4 pt-4 border-t border-neutral-200 dark:border-neutral-700">
                    <p class="text-xs text-neutral-500 dark:text-neutral-400 mb-4">
                        {locale.t("http_form.dry_run_desc")}
                    </p>

                    <label class="block mb-1 text-xs font-medium text-neutral-700 dark:text-neutral-300">
                        {locale.t("http_form.dry_run_url")}
                    </label>
                    <input
                        type="text"
                        value={(*url).clone()}
                        onchange={on_url_change}
                        placeholder="https://example.com/api/users"
                        class={INPUT_CLASS}
                    />
                    <p class={HINT_CLASS}>{locale.t("http_form.dry_run_url_hint")}</p>

                    <div class="grid grid-cols-1 sm:grid-cols-2 gap-4 mt-3">
                        <div>
                            <label class="block mb-1 text-xs font-medium text-neutral-700 dark:text-neutral-300">
                                {locale.t("http_form.dry_run_method")}
                            </label>
                            <select onchange={on_method_change} class={INPUT_CLASS}>
                                { HTTP_METHODS.iter().map(|m| html! {
                                    <option value={*m} selected={*method == *m}>{m}</option>
                                }).collect::<Html>() }
                            </select>
                        </div>
                        <div>
                            <label class="block mb-1 text-xs font-medium text-neutral-700 dark:text-neutral-300">
                                {locale.t("http_form.dry_run_client_ip")}
                            </label>
                            <input
                                type="text"
                                value={(*client_ip).clone()}
                                onchange={on_ip_change}
                                placeholder="192.168.1.10"
                                class={INPUT_CLASS}
                            />
                            <p class={HINT_CLASS}>{locale.t("http_form.dry_run_client_ip_hint")}</p>
                        </div>
                    </div>

                    <div class="mt-4 flex items-center gap-3">
                        <button
                            type="button"
                            onclick={on_test}
                            disabled={*is_loading}
                            class="inline-flex items-center gap-2 px-4 py-2 bg-blue-600 hover:bg-blue-700 disabled:opacity-50 text-white rounded-lg text-sm font-medium transition-colors"
                        >
                            if *is_loading {
                                <span class="animate-spin inline-block w-4 h-4 border-2 border-white border-t-transparent rounded-full"></span>
                            }
                            {locale.t("http_form.dry_run_button")}
                        </button>
                    </div>

                    if let Some(res) = &*result {
                        { dry_run_result_view(locale, res) }
                    }
                </div>
            }
        </div>
    }
}

async fn execute_dry_run_request(
    req_body: ProxyDryRunRequest,
    is_loading: UseStateHandle<bool>,
    result: UseStateHandle<Option<Result<ProxyDryRunResult, String>>>,
) {
    let res = Request::post(&format!("{API_ENDPOINT}/proxies/dry-run")).json(&req_body);
    match res {
        Ok(req) => match req.send().await {
            Ok(resp) if resp.ok() => match resp.json::<ProxyDryRunResult>().await {
                Ok(data) => result.set(Some(Ok(data))),
                Err(e) => result.set(Some(Err(e.to_string()))),
            },
            Ok(resp) => {
                let text = resp.text().await.unwrap_or_else(|_| "Error".to_string());
                result.set(Some(Err(text)));
            }
            Err(e) => result.set(Some(Err(e.to_string()))),
        },
        Err(e) => result.set(Some(Err(e.to_string()))),
    }
    is_loading.set(false);
}

fn dry_run_matched_view(locale: Locale, data: &ProxyDryRunResult) -> Html {
    html! {
        <div class="mt-4 p-4 rounded-lg border border-emerald-300 dark:border-emerald-800 bg-emerald-50 dark:bg-emerald-950/30">
            <div class="flex items-center gap-2 mb-2">
                <svg class="w-5 h-5 text-emerald-600 dark:text-emerald-400" fill="none" stroke="currentColor" viewBox="0 0 24 24">
                    <path stroke-linecap="round" stroke-linejoin="round" stroke-width="2" d="M5 13l4 4L19 7" />
                </svg>
                <span class="text-sm font-semibold text-emerald-900 dark:text-emerald-200">
                    {locale.t("http_form.dry_run_matched")}
                </span>
                if let Some(idx) = data.route_index {
                    <span class="ml-2 text-xs font-mono px-2 py-0.5 rounded bg-emerald-200 dark:bg-emerald-900 text-emerald-900 dark:text-emerald-200">
                        {format!("Route #{} ({})", idx + 1, data.route_path.as_deref().unwrap_or("/"))}
                    </span>
                }
            </div>

            if let Some(target) = &data.upstream_url {
                <div class="text-xs text-neutral-800 dark:text-neutral-200 mt-1">
                    <strong class="font-medium">{format!("{}: ", locale.t("http_form.dry_run_target"))}</strong>
                    <code class="px-1.5 py-0.5 rounded bg-neutral-200 dark:bg-neutral-800 font-mono text-blue-600 dark:text-blue-400">{target}</code>
                </div>
            }
            if let Some(redirect) = &data.redirect_url {
                <div class="text-xs text-neutral-800 dark:text-neutral-200 mt-1">
                    <strong class="font-medium">{format!("{}: ", locale.t("http_form.dry_run_redirect"))}</strong>
                    <code class="px-1.5 py-0.5 rounded bg-neutral-200 dark:bg-neutral-800 font-mono text-purple-600 dark:text-purple-400">
                        {format!("{} -> {}", data.redirect_status.unwrap_or(302), redirect)}
                    </code>
                </div>
            }

            <div class="flex flex-wrap gap-2 mt-3">
                if data.disallowed_by_ip {
                    <span class="text-xs px-2 py-0.5 rounded bg-red-100 text-red-800 dark:bg-red-950 dark:text-red-300 font-medium">
                        {locale.t("http_form.dry_run_ip_denied")}
                    </span>
                } else {
                    <span class="text-xs px-2 py-0.5 rounded bg-emerald-100 text-emerald-800 dark:bg-emerald-950 dark:text-emerald-300 font-medium">
                        {locale.t("http_form.dry_run_ip_allowed")}
                    </span>
                }
                if let Some(auth) = &data.auth_required {
                    <span class="text-xs px-2 py-0.5 rounded bg-blue-100 text-blue-800 dark:bg-blue-950 dark:text-blue-300 font-medium">
                        {auth}
                    </span>
                }
            </div>

            <p class="text-xs text-neutral-600 dark:text-neutral-400 mt-2 font-mono">
                {&data.explanation}
            </p>
        </div>
    }
}

fn dry_run_unmatched_view(locale: Locale, data: &ProxyDryRunResult) -> Html {
    html! {
        <div class="mt-4 p-4 rounded-lg border border-amber-300 dark:border-amber-800 bg-amber-50 dark:bg-amber-950/30">
            <div class="flex items-center gap-2 mb-2">
                <svg class="w-5 h-5 text-amber-600 dark:text-amber-400" fill="none" stroke="currentColor" viewBox="0 0 24 24">
                    <path stroke-linecap="round" stroke-linejoin="round" stroke-width="2" d="M12 9v2m0 4h.01m-6.938 4h13.856c1.54 0 2.502-1.667 1.732-3L13.732 4c-.77-1.333-2.694-1.333-3.464 0L3.34 16c-.77 1.333.192 3 1.732 3z" />
                </svg>
                <span class="text-sm font-semibold text-amber-900 dark:text-amber-200">
                    {locale.t("http_form.dry_run_no_match")}
                </span>
            </div>
            <p class="text-xs text-amber-800 dark:text-amber-300">
                {&data.explanation}
            </p>
        </div>
    }
}

fn dry_run_result_view(locale: Locale, res: &Result<ProxyDryRunResult, String>) -> Html {
    match res {
        Ok(data) if data.matched => dry_run_matched_view(locale, data),
        Ok(data) => dry_run_unmatched_view(locale, data),
        Err(err) => html! {
            <div class="mt-4 p-3 rounded-lg border border-red-300 dark:border-red-800 bg-red-50 dark:bg-red-950/30 text-xs text-red-700 dark:text-red-300">
                {err}
            </div>
        },
    }
}
