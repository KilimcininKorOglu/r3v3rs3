//! Visual domain SSL certificate coverage badges and CNAME alias verification.

use crate::{API_ENDPOINT, i18n::use_locale, store::SessionStore};
use gloo_net::http::Request;
use r3v3rs3_api::{
    cert_coverage::{CnameVerifyResult, DomainCoverageInfo, DomainSslStatus},
    i18n::Locale,
};
use std::collections::HashMap;
use yew::prelude::*;
use yewdux::prelude::*;

#[derive(Properties, PartialEq, Clone)]
pub struct DomainCertCoverageProps {
    pub domains: Vec<String>,
}

async fn check_coverage(
    locale: Locale,
    domains: &[String],
) -> Result<Vec<DomainCoverageInfo>, String> {
    let request = Request::post(&format!("{API_ENDPOINT}/certs/coverage"));
    crate::pages::settings::post_json(locale, request, &domains).await
}

async fn verify_cname(
    locale: Locale,
    domain: &str,
    target: &str,
) -> Result<CnameVerifyResult, String> {
    let request = Request::get(&format!("{API_ENDPOINT}/certs/verify_cname"))
        .query([("domain", domain), ("expected_target", target)]);
    crate::pages::settings::fetch_json(locale, request).await
}

async fn show_error(locale: Locale, err: &str) {
    crate::dialog::message(locale, err, crate::dialog::Icon::Error).await;
}

fn render_wildcard(locale: Locale, domain: &str, pattern: &str) -> Html {
    let badge = locale.tf("certs.wildcard_shared_badge", &[("pattern", pattern)]);
    html! {
        <div class="flex items-center gap-2 mt-1.5 p-2 bg-green-50 dark:bg-green-950/30 border border-green-200 dark:border-green-800 rounded text-xs text-green-900 dark:text-green-200">
            <svg class="w-4 h-4 text-green-600 dark:text-green-400 shrink-0" fill="none" stroke="currentColor" viewBox="0 0 24 24">
                <path stroke-linecap="round" stroke-linejoin="round" stroke-width="2" d="M12 15v2m-6 4h12a2 2 0 002-2v-6a2 2 0 00-2-2H6a2 2 0 00-2 2v6a2 2 0 002 2zm10-10V7a4 4 0 00-8 0v4h8z" />
            </svg>
            <span class="font-medium">{domain}</span>
            <span class="px-2 py-0.5 rounded-full bg-green-100 dark:bg-green-900 text-green-800 dark:text-green-200 font-semibold">
                {badge}
            </span>
        </div>
    }
}

fn render_dedicated(locale: Locale, domain: &str) -> Html {
    html! {
        <div class="flex items-center gap-2 mt-1.5 p-2 bg-green-50 dark:bg-green-950/30 border border-green-200 dark:border-green-800 rounded text-xs text-green-900 dark:text-green-200">
            <svg class="w-4 h-4 text-green-600 dark:text-green-400 shrink-0" fill="none" stroke="currentColor" viewBox="0 0 24 24">
                <path stroke-linecap="round" stroke-linejoin="round" stroke-width="2" d="M12 15v2m-6 4h12a2 2 0 002-2v-6a2 2 0 00-2-2H6a2 2 0 00-2 2v6a2 2 0 002 2zm10-10V7a4 4 0 00-8 0v4h8z" />
            </svg>
            <span class="font-medium">{domain}</span>
            <span class="px-2 py-0.5 rounded-full bg-green-100 dark:bg-green-900 text-green-800 dark:text-green-200 font-semibold">
                {locale.t("certs.dedicated_badge")}
            </span>
        </div>
    }
}

fn render_status_badge(
    locale: Locale,
    domain: &str,
    text_key: &str,
    badge_class: &'static str,
) -> Html {
    html! {
        <div class="flex items-center gap-2 mt-1.5 p-2 bg-neutral-50 dark:bg-neutral-900/40 border border-neutral-200 dark:border-neutral-800 rounded text-xs text-neutral-800 dark:text-neutral-200">
            <span class="font-medium">{domain}</span>
            <span class={classes!("px-2", "py-0.5", "rounded-full", "font-semibold", badge_class)}>
                {locale.t(text_key)}
            </span>
        </div>
    }
}

fn render_cname_badge(locale: Locale, verification: Option<&CnameVerifyResult>) -> Html {
    match verification {
        Some(res) if res.is_valid => html! {
            <span class="px-2 py-0.5 rounded-full bg-green-100 dark:bg-green-900 text-green-800 dark:text-green-200 font-semibold">
                {locale.t("certs.cname_verified_badge")}
            </span>
        },
        Some(res) => html! {
            <span class="text-xs text-red-600 dark:text-red-400">
                {&res.message}
            </span>
        },
        None => html! {},
    }
}

fn render_cname_action(
    locale: Locale,
    domain: String,
    target: String,
    verification: Option<&CnameVerifyResult>,
    on_verify: Callback<(String, String)>,
    is_loading: bool,
) -> Html {
    let is_verified = verification.is_some_and(|r| r.is_valid);
    let onclick = Callback::from(move |_| {
        on_verify.emit((domain.clone(), target.clone()));
    });

    if is_verified {
        html! {}
    } else {
        html! {
            <crate::components::button::Button
                kind={crate::components::button::ButtonKind::Secondary}
                size={crate::components::button::ButtonSize::Sm}
                disabled={is_loading}
                {onclick}
            >
                if is_loading {
                    <span class="animate-spin inline-block w-3 h-3 border-2 border-neutral-600 border-t-transparent rounded-full"></span>
                }
                {locale.t("certs.verify_cname_button")}
            </crate::components::button::Button>
        }
    }
}

fn render_cname_item(
    locale: Locale,
    domain: &str,
    target: &str,
    verification: Option<&CnameVerifyResult>,
    on_verify: Callback<(String, String)>,
    is_loading: bool,
) -> Html {
    let hint = locale.tf("certs.cname_target_hint", &[("target", target)]);
    html! {
        <div class="flex flex-wrap items-center justify-between gap-2 mt-1.5 p-2 bg-amber-50 dark:bg-amber-950/30 border border-amber-200 dark:border-amber-800 rounded text-xs text-amber-900 dark:text-amber-200">
            <div class="flex flex-wrap items-center gap-2">
                <span class="font-medium">{domain}</span>
                <span class="px-2 py-0.5 rounded-full bg-amber-100 dark:bg-amber-900 text-amber-800 dark:text-amber-200 font-semibold">
                    {locale.t("certs.cname_needed_badge")}
                </span>
                <span class="text-neutral-600 dark:text-neutral-400">
                    {hint}
                </span>
            </div>
            <div class="flex items-center gap-2">
                {render_cname_badge(locale, verification)}
                {render_cname_action(locale, domain.to_string(), target.to_string(), verification, on_verify, is_loading)}
            </div>
        </div>
    }
}

fn render_item(
    locale: Locale,
    info: &DomainCoverageInfo,
    cname_status: &HashMap<String, CnameVerifyResult>,
    on_verify: Callback<(String, String)>,
    loading_domain: Option<&str>,
) -> Html {
    match &info.status {
        DomainSslStatus::CoveredByWildcard { pattern, .. } => {
            render_wildcard(locale, &info.domain, pattern)
        }
        DomainSslStatus::DedicatedCert { .. } => render_dedicated(locale, &info.domain),
        DomainSslStatus::CnameOrExternal {
            suggested_cname_target,
        } => {
            let verification = cname_status.get(&info.domain);
            let is_loading = loading_domain == Some(&info.domain);
            render_cname_item(
                locale,
                &info.domain,
                suggested_cname_target,
                verification,
                on_verify,
                is_loading,
            )
        }
        DomainSslStatus::Expired { .. } => render_status_badge(
            locale,
            &info.domain,
            "certs.expired",
            "bg-red-100 dark:bg-red-900 text-red-800 dark:text-red-200",
        ),
        DomainSslStatus::NoCertificate => render_status_badge(
            locale,
            &info.domain,
            "certs.status_not_found",
            "bg-neutral-100 dark:bg-neutral-800 text-neutral-700 dark:text-neutral-300",
        ),
    }
}

/// Visual status card displaying SSL coverage and CNAME guidance for domains.
#[function_component(DomainCertCoverage)]
pub fn domain_cert_coverage(props: &DomainCertCoverageProps) -> Html {
    let locale = use_locale();
    let (session, _) = use_store::<SessionStore>();
    let coverage_list = use_state(Vec::<DomainCoverageInfo>::new);
    let cname_status = use_state(HashMap::<String, CnameVerifyResult>::new);
    let loading_domain = use_state(|| None::<String>);

    {
        let coverage_list = coverage_list.clone();
        let domains = props.domains.clone();
        use_effect_with(
            (session.info.is_some(), domains),
            move |(signed_in, domains)| {
                if *signed_in && !domains.is_empty() {
                    let coverage_list = coverage_list.clone();
                    let domains = domains.clone();
                    wasm_bindgen_futures::spawn_local(async move {
                        match check_coverage(locale, &domains).await {
                            Ok(res) => coverage_list.set(res),
                            Err(err) => show_error(locale, &err).await,
                        }
                    });
                } else if domains.is_empty() {
                    coverage_list.set(Vec::new());
                }
            },
        );
    }

    let on_verify = {
        let cname_status = cname_status.clone();
        let loading_domain = loading_domain.clone();
        Callback::from(move |(domain, target): (String, String)| {
            let cname_status = cname_status.clone();
            let loading_domain = loading_domain.clone();
            loading_domain.set(Some(domain.clone()));
            wasm_bindgen_futures::spawn_local(async move {
                match verify_cname(locale, &domain, &target).await {
                    Ok(res) => {
                        let mut current = (*cname_status).clone();
                        current.insert(domain, res);
                        cname_status.set(current);
                    }
                    Err(err) => show_error(locale, &err).await,
                }
                loading_domain.set(None);
            });
        })
    };

    if coverage_list.is_empty() {
        return html! {};
    }

    let current_loading = (*loading_domain).as_deref();
    html! {
        <div class="mt-2 mb-3">
            <label class="block mb-1 text-xs font-medium text-neutral-600 dark:text-neutral-400">
                {locale.t("certs.ssl_coverage_title")}
            </label>
            <div class="space-y-1">
                { for coverage_list.iter().map(|info| render_item(locale, info, &cname_status, on_verify.clone(), current_loading)) }
            </div>
        </div>
    }
}
