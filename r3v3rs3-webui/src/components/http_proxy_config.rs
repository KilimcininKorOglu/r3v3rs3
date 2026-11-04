use super::auth_config::AuthConfig;
use super::domain_ssl_status::DomainCertCoverage;
use super::grpc_tester::GrpcTester;
use super::proxy_dry_run::ProxyDryRun;
use crate::i18n::use_locale;
use r3v3rs3_api::cidr::{format_cidr_list, parse_cidr_list};
use r3v3rs3_api::client_ip::ClientIpConfig;
use r3v3rs3_api::header_rules::{format_header_rules, parse_header_rules};
use r3v3rs3_api::i18n::Locale;
use r3v3rs3_api::id::ShortId;
use r3v3rs3_api::policy::{AuthPolicy, IpFilter};
use r3v3rs3_api::proxy::HttpProxy;
use r3v3rs3_api::redirect::RedirectRule;
use r3v3rs3_api::security_headers::{SecurityProfile, apply_profile_to_rules};
use std::collections::HashMap;
use yew::prelude::*;

pub use super::proxy_subcomponents::cache_config::*;
pub use super::proxy_subcomponents::common::*;
pub use super::proxy_subcomponents::routing_config::*;
pub use super::proxy_subcomponents::security_config::*;
pub use super::proxy_subcomponents::upstream_config::*;

#[derive(Properties, PartialEq)]
pub struct Props {
    #[prop_or_default]
    pub proxy: HttpProxy,
    pub onchanged: Callback<Result<HttpProxy, HashMap<String, String>>>,
}

#[derive(Clone, PartialEq)]
pub struct ProxyForm {
    pub vhosts: String,
    pub upgrade_insecure: bool,
    pub trust_cdn: bool,
    pub trusted_proxies: String,
    pub allow: String,
    pub deny: String,
    pub allow_countries: String,
    pub deny_countries: String,
    pub rate_limit: RateLimitForm,
    pub concurrency_limit: ConcurrencyLimitForm,
    pub auth: crate::components::auth_config::AuthForm,
    pub access_list: Option<ShortId>,
    pub request_headers: String,
    pub response_headers: String,
    pub compression: CompressionForm,
    pub cache: CacheForm,
    pub h2c: bool,
    pub client_cert: Option<ShortId>,
    pub timeouts: TimeoutsForm,
    pub retry: RetryForm,
    pub sticky: StickyForm,
    pub max_body_size: String,
    /// One redirect rule on each line.
    pub redirects: String,
    pub maintenance: MaintenanceForm,
    pub error_page_theme: Option<ShortId>,
    pub error_pages: Vec<ErrorPageForm>,
    pub hide_branding: bool,
    pub bot_blocker: bool,
    pub grpc_web: bool,
    pub stream_priority: r3v3rs3_api::priority::StreamPriorityConfig,
}

impl ProxyForm {
    pub fn new(proxy: &HttpProxy) -> Self {
        Self {
            vhosts: proxy
                .vhosts
                .iter()
                .map(|host| host.to_string())
                .collect::<Vec<_>>()
                .join(", "),
            upgrade_insecure: proxy.upgrade_insecure,
            trust_cdn: proxy.client_ip.trust_cdn,
            trusted_proxies: format_cidr_list(&proxy.client_ip.trusted_proxies),
            allow: format_cidr_list(&proxy.ip_filter.allow),
            deny: format_cidr_list(&proxy.ip_filter.deny),
            allow_countries: proxy.ip_filter.allow_countries.join(", "),
            deny_countries: proxy.ip_filter.deny_countries.join(", "),
            rate_limit: RateLimitForm::new(&proxy.rate_limit),
            concurrency_limit: ConcurrencyLimitForm::new(&proxy.concurrency_limit),
            auth: crate::components::auth_config::AuthForm::new(&proxy.auth),
            access_list: proxy.access_list,
            request_headers: format_header_rules(&proxy.headers.request),
            response_headers: format_header_rules(&proxy.headers.response),
            compression: CompressionForm::new(&proxy.compression),
            cache: CacheForm::new(&proxy.cache),
            h2c: proxy.h2c,
            client_cert: proxy.client_cert,
            timeouts: TimeoutsForm::new(&proxy.timeouts),
            retry: RetryForm::new(&proxy.retry),
            sticky: StickyForm::new(&proxy.sticky),
            max_body_size: proxy.max_body_size.to_string(),
            redirects: proxy
                .redirects
                .iter()
                .map(ToString::to_string)
                .collect::<Vec<_>>()
                .join("\n"),
            maintenance: MaintenanceForm::new(&proxy.maintenance),
            error_page_theme: proxy.error_page_theme,
            error_pages: proxy
                .error_pages
                .iter()
                .map(|p| ErrorPageForm {
                    status: p.status.to_string(),
                    body: p.body.clone(),
                })
                .collect(),
            hide_branding: proxy.hide_branding,
            bot_blocker: proxy.bot_blocker,
            grpc_web: proxy.grpc_web,
            stream_priority: proxy.stream_priority.clone(),
        }
    }
}

pub fn parse_proxy_access(
    locale: Locale,
    form: &ProxyForm,
    errors: &mut HashMap<String, String>,
) -> (IpFilter, AuthPolicy) {
    parse_access_filter(
        locale,
        &AccessFilterParams {
            access_list: form.access_list,
            allow: &form.allow,
            deny: &form.deny,
            allow_countries: &form.allow_countries,
            deny_countries: &form.deny_countries,
            auth_form: &form.auth,
        },
        errors,
    )
}

#[function_component(HttpProxyConfig)]
pub fn http_proxy_config(props: &Props) -> Html {
    let locale = use_locale();
    let form = use_state(|| ProxyForm::new(&props.proxy));
    let upstream = use_upstream_form(props.proxy.load_balancing, props.proxy.health_check.clone());
    let circuit_breaker = use_state(|| CircuitBreakerForm::new(&props.proxy.circuit_breaker));
    let client_certs = use_client_certs();
    let access_lists = use_access_lists();
    let themes = use_error_page_themes();
    let routes = use_state(|| {
        let routes = props
            .proxy
            .routes
            .iter()
            .map(RouteForm::new)
            .collect::<Vec<_>>();
        if routes.is_empty() {
            vec![RouteForm::empty()]
        } else {
            routes
        }
    });

    let parsed_proxy = get_proxy(locale, &form, &routes, &upstream, &circuit_breaker);
    let current_proxy = parsed_proxy.as_ref().ok().cloned();
    let errors = use_entry_errors(parsed_proxy, props.onchanged.clone());

    let on_apply_profile = {
        let form = form.clone();
        Callback::from(move |profile: SecurityProfile| {
            let existing = parse_header_rules(&form.response_headers).unwrap_or_default();
            let updated = apply_profile_to_rules(&existing, profile);
            let mut form_data = (*form).clone();
            form_data.response_headers = format_header_rules(&updated);
            form.set(form_data);
        })
    };

    html! {
        <>
            { toggle(state_input(&form, checked, |form, value| form.upgrade_insecure = value), form.upgrade_insecure, locale.t("http_form.upgrade_insecure"), "my-6") }

            { toggle(state_input(&form, checked, |form, value| form.bot_blocker = value), form.bot_blocker, locale.t("http_form.bot_blocker"), "my-6") }
            <p class={HINT_CLASS}>{locale.t("http_form.bot_blocker_hint")}</p>

            { toggle(state_input(&form, checked, |form, value| form.grpc_web = value), form.grpc_web, locale.t("http_form.grpc_web"), "my-6") }
            <p class={HINT_CLASS}>{locale.t("http_form.grpc_web_hint")}</p>

            { toggle(state_input(&form, checked, |form, value| form.stream_priority.enabled = value), form.stream_priority.enabled, locale.t("proxies.stream_priority"), "my-6") }
            <p class={HINT_CLASS}>{locale.t("proxies.stream_priority_hint")}</p>
            { stream_priority_subtoggles(
                locale,
                &form.stream_priority,
                state_input(&form, checked, |form, value| form.stream_priority.inject_header = value),
                state_input(&form, checked, |form, value| form.stream_priority.adaptive_concurrency = value),
            ) }

            <label class={LABEL_CLASS}>{locale.t("http_form.vhosts")}</label>
            <input type="text" autocapitalize="off" value={form.vhosts.clone()} onchange={state_input(&form, text, |form, value| form.vhosts = value)} class={INPUT_CLASS} placeholder="example.com" />
            { error_view(errors.get("vhosts")) }
            <p class={HINT_CLASS}>{locale.t("http_form.vhosts_hint")}</p>
            <DomainCertCoverage domains={split_vhosts(&form.vhosts)} />

            { toggle(state_input(&form, checked, |form, value| form.trust_cdn = value), form.trust_cdn, locale.t("http_form.trust_cdn"), "mt-6") }
            <p class={HINT_CLASS}>{locale.t("http_form.trust_cdn_hint")}</p>

            <label class={LABEL_CLASS}>{locale.t("http_form.trusted_proxies")}</label>
            <input type="text" autocapitalize="off" value={form.trusted_proxies.clone()} onchange={state_input(&form, text, |form, value| form.trusted_proxies = value)} class={INPUT_CLASS} placeholder="10.0.0.0/8, 192.168.1.10" />
            { error_view(errors.get("trusted_proxies")) }
            <p class={HINT_CLASS}>{locale.t("http_form.trusted_proxies_hint")}</p>

            { access_list_view(
                locale,
                state_input(&form, select_value, |form, value| form.access_list = parse_optional_id(&value)),
                form.access_list,
                &access_lists,
                "http_form.access_list_hint",
            ) }

            if form.access_list.is_none() {
                <label class={LABEL_CLASS}>{locale.t("http_form.allow")}</label>
                <input type="text" autocapitalize="off" value={form.allow.clone()} onchange={state_input(&form, text, |form, value| form.allow = value)} class={INPUT_CLASS} placeholder="192.168.0.0/16" />
                { error_view(errors.get("allow")) }
                <p class={HINT_CLASS}>{locale.t("http_form.allow_hint")}</p>

                <label class={LABEL_CLASS}>{locale.t("http_form.deny")}</label>
                <input type="text" autocapitalize="off" value={form.deny.clone()} onchange={state_input(&form, text, |form, value| form.deny = value)} class={INPUT_CLASS} placeholder="203.0.113.0/24" />
                { error_view(errors.get("deny")) }
                <p class={HINT_CLASS}>{locale.t("http_form.deny_hint")}</p>

                <div class="pt-2">
                    <crate::components::geoip_wizard::GeoIpRuleWizard
                        allow_countries={form.allow_countries.clone()}
                        deny_countries={form.deny_countries.clone()}
                        on_allow_change={let form = form.clone(); Callback::from(move |val: String| {
                            let mut f = (*form).clone();
                            f.allow_countries = val;
                            form.set(f);
                        })}
                        on_deny_change={let form = form.clone(); Callback::from(move |val: String| {
                            let mut f = (*form).clone();
                            f.deny_countries = val;
                            form.set(f);
                        })}
                    />
                </div>
            }

            <label class={SECTION_CLASS}>{locale.t("http_form.rate_limit")}</label>
            { rate_limit_view(
                locale,
                &form.rate_limit,
                state_input(&form, text, |form, value| form.rate_limit.requests = value),
                state_input(&form, period, |form, value| form.rate_limit.per = value),
                state_input(&form, text, |form, value| form.rate_limit.burst = value),
            ) }
            { error_view(errors.get("rate_limit")) }
            <p class={HINT_CLASS}>{locale.t("http_form.rate_limit_hint")}</p>

            <label class={SECTION_CLASS}>{locale.t("http_form.concurrency_limit")}</label>
            { concurrency_limit_view(
                locale,
                &form.concurrency_limit,
                state_input(&form, text, |form, value| form.concurrency_limit.max = value),
                state_input(&form, text, |form, value| form.concurrency_limit.header = value),
            ) }
            { error_view(errors.get("concurrency_limit")) }
            <p class={HINT_CLASS}>{locale.t("http_form.concurrency_limit_hint")}</p>

            if form.access_list.is_none() {
                <label class={SECTION_CLASS}>{locale.t("http_form.authentication")}</label>
                <AuthConfig form={form.auth.clone()} onchange={state_update(&form, |form, value| form.auth = value)} />
                { error_view(errors.get("auth")) }
            }

            <label class={SECTION_CLASS}>{locale.t("http_form.header_rules")}</label>
            { header_rules_view(
                locale,
                &form.request_headers,
                &form.response_headers,
                state_input(&form, text_area, |form, value| form.request_headers = value),
                state_input(&form, text_area, |form, value| form.response_headers = value),
                on_apply_profile,
            ) }
            { error_view(errors.get("headers")) }
            <p class={HINT_CLASS}>{locale.t("http_form.header_rules_hint")}</p>

            <label class={SECTION_CLASS}>{locale.t("http_form.redirects")}</label>
            <textarea rows="3" autocapitalize="off" spellcheck="false" placeholder={"301 ^example\\.com/old/(.*)$ https://example.com/new/${1}"} value={form.redirects.clone()} onchange={state_input(&form, text_area, |form, value| form.redirects = value)} class={classes!(INPUT_CLASS, "font-mono")} />
            { error_view(errors.get("redirects")) }
            <p class={HINT_CLASS}>{locale.t("http_form.redirects_hint")}</p>

            <label class={SECTION_CLASS}>{locale.t("http_form.compression")}</label>
            { compression_view(locale, &form.compression, state_update(&form, |form, value| form.compression = value)) }
            { error_view(errors.get("compression")) }
            <p class={HINT_CLASS}>{locale.t("http_form.compression_hint")}</p>

            <label class={SECTION_CLASS}>{locale.t("http_form.cache")}</label>
            { cache_view(locale, &form.cache, state_update(&form, |form, value| form.cache = value)) }
            { error_view(errors.get("cache")) }
            <p class={HINT_CLASS}>{locale.t("http_form.cache_hint")}</p>

            { maintenance_view(locale, &form.maintenance, state_update(&form, |form, value| form.maintenance = value), &errors) }
            { error_pages_view(
                locale,
                &ErrorPagesViewProps {
                    error_pages: &form.error_pages,
                    error_page_theme: form.error_page_theme,
                    hide_branding: form.hide_branding,
                    on_pages_change: state_update(&form, |form, value| form.error_pages = value),
                    on_theme_change: state_input(&form, select_value, |form, value| form.error_page_theme = parse_optional_id(&value)),
                    on_branding_change: state_input(&form, checked, |form, value| form.hide_branding = value),
                    errors: &errors,
                    themes: &themes,
                },
            ) }

            <label class={SECTION_CLASS}>{locale.t("http_form.upstream_protocol")}</label>
            <div>
                { toggle(state_input(&form, checked, |form, value| form.h2c = value), form.h2c, locale.t("http_form.h2c"), "mt-4") }
            </div>
            <p class={HINT_CLASS}>{locale.t("http_form.h2c_hint")}</p>
            { client_cert_view(
                locale,
                state_input(&form, select_value, |form, value| form.client_cert = parse_optional_id(&value)),
                form.client_cert,
                &client_certs,
            ) }

            <label class={SECTION_CLASS}>{locale.t("http_form.timeouts")}</label>
            { timeouts_view(
                locale,
                &form.timeouts,
                state_input(&form, text, |form, value| form.timeouts.connect = value),
                state_input(&form, text, |form, value| form.timeouts.request = value),
            ) }
            { error_view(errors.get("timeouts")) }
            <p class={HINT_CLASS}>{locale.t("http_form.timeouts_hint")}</p>

            <label class={SECTION_CLASS}>{locale.t("http_form.body_limit")}</label>
            { body_limit_input(locale, &form.max_body_size, state_input(&form, text, |form, value| form.max_body_size = value)) }
            { error_view(errors.get("max_body_size")) }
            <p class={HINT_CLASS}>{locale.t("http_form.body_limit_hint")}</p>

            <label class={SECTION_CLASS}>{locale.t("http_form.retries")}</label>
            { retry_view(locale, &form.retry, state_update(&form, |form, value| form.retry = value)) }
            { error_view(errors.get("retry")) }
            <p class={HINT_CLASS}>{locale.t("http_form.retries_hint")}</p>

            <label class={SECTION_CLASS}>{locale.t("http_form.sticky_sessions")}</label>
            { sticky_view(
                locale,
                &form.sticky,
                state_input(&form, checked, |form, value| form.sticky.enabled = value),
                state_input(&form, text, |form, value| form.sticky.name = value),
                state_input(&form, text, |form, value| form.sticky.max_age = value),
            ) }
            { error_view(errors.get("sticky")) }
            <p class={HINT_CLASS}>{locale.t("http_form.sticky_hint")}</p>

            { upstream_form_view(locale, &upstream, &errors, true) }

            { circuit_breaker_view(locale, &circuit_breaker, &errors, "proxy_form.circuit_breaker_hint") }

            <label class={SECTION_CLASS}>{locale.t("http_form.routes")}</label>
            <p class={HINT_CLASS}>{locale.t("http_form.routes_hint")}</p>

            { routes.iter().enumerate().map(|(i, route)| {
                route_view(locale, &routes, i, route, &access_lists, errors.get(&format!("routes_{i}")))
            }).collect::<Html>() }

            <ProxyDryRun proxy={current_proxy.clone()} />
            <GrpcTester default_target={default_grpc_target(&current_proxy)} />
        </>
    }
}

pub fn default_grpc_target(proxy: &Option<HttpProxy>) -> Option<String> {
    proxy.as_ref().and_then(|p| {
        p.routes
            .iter()
            .flat_map(|r| &r.servers)
            .map(|s| s.url.to_string())
            .next()
    })
}

pub fn get_proxy(
    locale: Locale,
    form: &ProxyForm,
    routes: &[RouteForm],
    upstream: &UpstreamForm,
    circuit_breaker: &CircuitBreakerForm,
) -> Result<HttpProxy, HashMap<String, String>> {
    let mut errors = HashMap::new();
    let (load_balancing, health_check) = upstream.parse(locale, true, &mut errors);
    let circuit_breaker = circuit_breaker.parse(locale, &mut errors);
    let vhosts = parse_vhosts(locale, &form.vhosts, &mut errors);
    let routes = parse_routes(locale, routes, &mut errors);
    let trusted_proxies = or_error(
        translated(locale, parse_cidr_list(&form.trusted_proxies)),
        "trusted_proxies",
        &mut errors,
    );
    let (ip_filter, auth) = parse_proxy_access(locale, form, &mut errors);
    let rate_limit = parse_rate_limit(locale, &form.rate_limit, "rate_limit", &mut errors);
    let concurrency_limit = parse_concurrency_limit(
        locale,
        &form.concurrency_limit,
        "concurrency_limit",
        &mut errors,
    );
    let headers = parse_header_rules_form(
        locale,
        &form.request_headers,
        &form.response_headers,
        "headers",
        &mut errors,
    );
    let compression = parse_compression(locale, &form.compression, "compression", &mut errors);
    let cache = parse_cache(locale, &form.cache, "cache", &mut errors);
    let timeouts = parse_timeouts(locale, &form.timeouts, "timeouts", &mut errors);
    let retry = parse_retry(locale, &form.retry, "retry", &mut errors);
    let sticky = parse_sticky(locale, &form.sticky, "sticky", &mut errors);
    let max_body_size = or_error(
        parse_size(locale, &form.max_body_size, "http_form.max_body_size_name"),
        "max_body_size",
        &mut errors,
    );
    let redirects = or_error(
        parse_lines(
            locale,
            &form.redirects,
            "http_form.redirects_error",
            RedirectRule::parse_line,
        ),
        "redirects",
        &mut errors,
    );
    let maintenance = parse_maintenance(locale, &form.maintenance, &mut errors);
    let error_pages = parse_error_pages(locale, &form.error_pages, &mut errors);

    if !errors.is_empty() {
        return Err(errors);
    }
    Ok(HttpProxy {
        vhosts,
        routes,
        upgrade_insecure: form.upgrade_insecure,
        client_ip: ClientIpConfig {
            trust_cdn: form.trust_cdn,
            trusted_proxies,
        },
        ip_filter,
        rate_limit,
        concurrency_limit,
        auth,
        access_list: form.access_list,
        headers,
        compression,
        cache,
        h2c: form.h2c,
        client_cert: form.client_cert,
        timeouts,
        load_balancing,
        health_check,
        circuit_breaker,
        retry,
        sticky,
        max_body_size,
        redirects,
        maintenance,
        error_page_theme: form.error_page_theme,
        error_pages,
        hide_branding: form.hide_branding,
        bot_blocker: form.bot_blocker,
        grpc_web: form.grpc_web,
        stream_priority: form.stream_priority.clone(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use r3v3rs3_api::error::Error;
    use r3v3rs3_api::fixed_response::{FixedRedirect, FixedResponse, FixedStatus};
    use r3v3rs3_api::mirror::Mirror;
    use r3v3rs3_api::policy::ConcurrencyLimit;
    use r3v3rs3_api::proxy::{Route, Server};
    use r3v3rs3_api::redirect::RedirectStatus;
    use r3v3rs3_api::rewrite::PathRewrite;
    use r3v3rs3_api::upstream::{
        DEFAULT_WEIGHT, HealthCheck, LoadBalancing, RetryOn, RetryPolicy, StickyCookie,
        UpstreamTimeouts,
    };
    use std::time::Duration;

    #[test]
    fn seconds_round_trip_and_stay_in_range() {
        assert_eq!(format_seconds(Duration::ZERO), "0");
        assert_eq!(format_seconds(Duration::from_millis(300)), "1");
        assert_eq!(format_seconds(Duration::from_secs(90)), "90");

        let connect = "proxy_form.connect_timeout_name";
        assert_eq!(
            parse_seconds(Locale::En, " 90 ", connect, 1),
            Ok(Duration::from_secs(90))
        );
        assert_eq!(
            parse_seconds(Locale::En, "0", "proxy_form.request_timeout_name", 0),
            Ok(Duration::ZERO)
        );
        let err = parse_seconds(Locale::En, "0", connect, 1).unwrap_err();
        assert!(err.starts_with("Connect timeout"), "{err}");
        assert!(parse_seconds(Locale::En, "86401", connect, 1).is_err());
        assert!(parse_seconds(Locale::En, "1.5", connect, 1).is_err());
    }

    #[test]
    fn fixed_response_routes_round_trip_without_servers() {
        let routes = [
            FixedResponse::Redirect(FixedRedirect {
                target: "https://example.com".into(),
                status: RedirectStatus::MovedPermanently,
                preserve_path: false,
            }),
            FixedResponse::Status(FixedStatus {
                status: 410,
                body: "gone".into(),
            }),
        ]
        .map(|response| Route {
            path: "/".into(),
            response: Some(response),
            ..Default::default()
        });
        for route in routes {
            let form = RouteForm::new(&route);
            let mut errors = HashMap::new();
            assert_eq!(
                parse_route(Locale::En, &form, "routes_0", &mut errors),
                Some(route)
            );
            assert!(errors.is_empty(), "{errors:?}");
        }
    }

    #[test]
    fn an_access_list_replaces_the_ip_filter_and_the_authentication() {
        let office: ShortId = "office".parse().unwrap();
        let route = RouteForm {
            servers: vec!["http://127.0.0.1:9000/".into()],
            access_list: Some(office),
            override_ip_filter: true,
            allow: "not a block".into(),
            override_auth: true,
            ..RouteForm::empty()
        };
        let mut errors = HashMap::new();
        let parsed = parse_route(Locale::En, &route, "routes_0", &mut errors).unwrap();
        assert!(errors.is_empty(), "{errors:?}");
        assert_eq!(parsed.access_list, Some(office));
        assert_eq!((&parsed.ip_filter, &parsed.auth), (&None, &None));
        assert_eq!(RouteForm::new(&parsed).access_list, Some(office));

        let proxy = ProxyForm {
            access_list: Some(office),
            allow: "not a block".into(),
            ..ProxyForm::new(&HttpProxy::default())
        };
        let (ip_filter, auth) = parse_proxy_access(Locale::En, &proxy, &mut errors);
        assert!(errors.is_empty(), "{errors:?}");
        assert!(ip_filter.is_empty() && auth.is_none());
        let own = ProxyForm {
            access_list: None,
            ..proxy
        };
        parse_proxy_access(Locale::En, &own, &mut errors);
        assert!(errors.contains_key("allow"));
        assert_eq!(parse_optional_id(""), None);
    }

    #[test]
    fn a_route_reports_missing_servers_and_an_empty_redirect_target() {
        let mut errors = HashMap::new();
        let proxy = RouteForm::empty();
        assert!(parse_route(Locale::Tr, &proxy, "routes_0", &mut errors).is_some());
        let expected = Locale::Tr.t("http_form.servers_required");
        assert_eq!(errors.get("routes_0").map(String::as_str), Some(expected));

        let redirect = RouteForm {
            response: ResponseForm {
                kind: RouteKind::Redirect,
                ..proxy.response.clone()
            },
            ..proxy
        };
        let mut errors = HashMap::new();
        parse_route(Locale::Tr, &redirect, "routes_0", &mut errors);
        let err = Error::InvalidRedirectTarget {
            target: String::new(),
        };
        let expected = Locale::Tr.error_message(&err);
        assert_eq!(errors.get("routes_0"), Some(&expected));
    }

    #[test]
    fn route_timeouts_apply_only_when_overridden() {
        let route = Route {
            servers: vec![Server::new("http://127.0.0.1:9000/".parse().unwrap())],
            timeouts: Some(UpstreamTimeouts {
                connect: Duration::from_secs(3),
                request: Duration::from_secs(120),
            }),
            ..Default::default()
        };
        let form = RouteForm::new(&route);
        let mut errors = HashMap::new();
        assert_eq!(
            parse_route(Locale::En, &form, "routes_0", &mut errors),
            Some(route)
        );

        let inherited = RouteForm {
            override_timeouts: false,
            ..form.clone()
        };
        let parsed = parse_route(Locale::En, &inherited, "routes_0", &mut errors);
        assert_eq!(parsed.unwrap().timeouts, None);
        assert!(errors.is_empty());

        let invalid = RouteForm {
            timeouts: TimeoutsForm {
                connect: "0".into(),
                request: "5".into(),
            },
            ..form
        };
        parse_route(Locale::En, &invalid, "routes_0", &mut errors);
        assert!(errors.contains_key("routes_0"));
    }

    #[test]
    fn route_retry_round_trips_and_reports_invalid_attempts() {
        let retry = RetryPolicy {
            attempts: 3,
            retry_on: vec![RetryOn::Connect, RetryOn::Http503],
            replay_body_limit: 4096,
        };
        let route = Route {
            servers: vec![Server::new("http://127.0.0.1:9000/".parse().unwrap())],
            retry: Some(retry.clone()),
            ..Default::default()
        };
        let form = RouteForm::new(&route);
        let mut errors = HashMap::new();
        assert_eq!(
            parse_route(Locale::En, &form, "routes_0", &mut errors),
            Some(route)
        );
        assert!(errors.is_empty());

        assert_eq!(
            toggled(&retry.retry_on, RetryOn::Timeout, true),
            [RetryOn::Connect, RetryOn::Timeout, RetryOn::Http503]
        );
        assert_eq!(
            toggled(&retry.retry_on, RetryOn::Connect, false),
            [RetryOn::Http503]
        );

        let invalid = RouteForm {
            retry: RetryForm {
                attempts: "11".into(),
                ..form.retry.clone()
            },
            ..form
        };
        parse_route(Locale::Tr, &invalid, "routes_0", &mut errors);
        let expected = Locale::Tr.error_message(&Error::InvalidRetryAttempts);
        assert_eq!(errors.get("routes_0"), Some(&expected));
    }

    #[test]
    fn redirect_rules_round_trip_and_report_the_line() {
        let line = r"301 ^a\.test/(.*)$ https://b.test/${1}";
        let proxy = HttpProxy {
            redirects: vec![RedirectRule::parse_line(line).unwrap()],
            ..Default::default()
        };
        let form = ProxyForm::new(&proxy);
        assert_eq!(form.redirects, line);
        let key = "http_form.redirects_error";
        let text = format!("# comment\n\n{}", form.redirects);
        let parsed = parse_lines(Locale::En, &text, key, RedirectRule::parse_line);
        assert_eq!(parsed, Ok(proxy.redirects));

        let text = "301 ^a$ /b\n303 ^a$ /b";
        let err = parse_lines(Locale::Tr, text, key, RedirectRule::parse_line).unwrap_err();
        let message = Locale::Tr.error_message(&Error::InvalidRedirectStatus { status: 303 });
        let expected = Locale::Tr.tf(key, &[("line", "2"), ("error", &message)]);
        assert_eq!(err, expected);
    }

    #[test]
    fn route_mirror_round_trips_and_reports_an_invalid_percent() {
        let route = Route {
            servers: vec![Server::new("http://127.0.0.1:9000/".parse().unwrap())],
            mirror: Some(Mirror {
                servers: vec![Server::new("http://127.0.0.1:9100/".parse().unwrap())],
                percent: 10,
                max_body_size: 1024,
            }),
            ..Default::default()
        };
        let form = RouteForm::new(&route);
        let mut errors = HashMap::new();
        assert_eq!(
            parse_route(Locale::En, &form, "routes_0", &mut errors),
            Some(route)
        );
        assert!(errors.is_empty());

        let off = RouteForm {
            mirror: false,
            ..form.clone()
        };
        let parsed = parse_route(Locale::En, &off, "routes_0", &mut errors);
        assert_eq!(parsed.unwrap().mirror, None);

        let invalid = RouteForm {
            mirror_percent: "0".into(),
            ..form
        };
        parse_route(Locale::Tr, &invalid, "routes_0", &mut errors);
        let name = Locale::Tr.t("http_form.mirror_percent_name");
        let expected = Locale::Tr.tf("proxy_form.percent_range", &[("name", name)]);
        assert_eq!(errors.get("routes_0"), Some(&expected));
    }

    #[test]
    fn route_rewrite_round_trips_and_reports_invalid_values() {
        let route = Route {
            servers: vec![Server::new("http://127.0.0.1:9000/".parse().unwrap())],
            rewrite: PathRewrite {
                strip_prefix: false,
                regex: Some("^/items/([0-9]+)$".parse().unwrap()),
                replacement: "/item/${1}".into(),
                add_prefix: "/v2".into(),
            },
            ..Default::default()
        };
        let form = RouteForm::new(&route);
        let mut errors = HashMap::new();
        assert_eq!(
            parse_route(Locale::En, &form, "routes_0", &mut errors),
            Some(route)
        );
        assert!(errors.is_empty());

        let cases = [
            (
                RouteForm {
                    path_regex: "(".into(),
                    ..form.clone()
                },
                Error::InvalidPathRegex {
                    pattern: "(".into(),
                },
            ),
            (
                RouteForm {
                    add_prefix: "v2".into(),
                    ..form
                },
                Error::InvalidPathPrefix {
                    prefix: "v2".into(),
                },
            ),
        ];
        for (invalid, err) in cases {
            let mut errors = HashMap::new();
            parse_route(Locale::Tr, &invalid, "routes_0", &mut errors);
            let expected = Locale::Tr.error_message(&err);
            assert_eq!(errors.get("routes_0"), Some(&expected));
        }
    }

    #[test]
    fn route_body_limit_round_trips_and_reports_an_invalid_size() {
        let route = Route {
            servers: vec![Server::new("http://127.0.0.1:9000/".parse().unwrap())],
            max_body_size: Some(1024),
            ..Default::default()
        };
        let form = RouteForm::new(&route);
        let mut errors = HashMap::new();
        assert_eq!(
            parse_route(Locale::En, &form, "routes_0", &mut errors),
            Some(route)
        );
        let unlimited = RouteForm {
            max_body_size: "0".into(),
            ..form.clone()
        };
        let parsed = parse_route(Locale::En, &unlimited, "routes_0", &mut errors);
        assert_eq!(parsed.unwrap().max_body_size, Some(0));
        assert!(errors.is_empty());

        let invalid = RouteForm {
            max_body_size: "1k".into(),
            ..form
        };
        parse_route(Locale::Tr, &invalid, "routes_0", &mut errors);
        let name = Locale::Tr.t("http_form.max_body_size_name");
        let expected = Locale::Tr.tf("http_form.whole_number", &[("name", name)]);
        assert_eq!(errors.get("routes_0"), Some(&expected));
    }

    #[test]
    fn sticky_form_round_trips_and_reports_an_invalid_name() {
        let sticky = StickyCookie {
            enabled: true,
            name: "srv".into(),
            max_age: Some(Duration::from_secs(3600)),
        };
        let form = StickyForm::new(&sticky);
        let mut errors = HashMap::new();
        assert_eq!(
            parse_sticky(Locale::En, &form, "sticky", &mut errors),
            sticky
        );
        assert!(errors.is_empty());

        let session = StickyForm {
            max_age: "0".into(),
            ..form.clone()
        };
        let parsed = parse_sticky(Locale::En, &session, "sticky", &mut errors);
        assert_eq!(parsed.max_age, None);
        assert_eq!(StickyForm::new(&parsed).max_age, "0");

        let invalid = StickyForm {
            name: "a b".into(),
            ..form
        };
        parse_sticky(Locale::Tr, &invalid, "sticky", &mut errors);
        let expected =
            Locale::Tr.error_message(&Error::InvalidStickyCookieName { name: "a b".into() });
        assert_eq!(errors.get("sticky"), Some(&expected));
    }

    #[test]
    fn server_lines_skip_the_empty_lines() {
        assert_eq!(
            server_lines(" http://a:80/ \n\n\thttp://b:80/\n"),
            ["http://a:80/", "http://b:80/"]
        );
    }

    #[test]
    fn server_lines_carry_an_optional_weight() {
        let weighted = parse_server_line(Locale::En, "http://a:8080/  3").unwrap();
        assert_eq!(weighted.weight, 3);
        assert_eq!(format_server_line(&weighted), "http://a:8080/ 3");

        let plain = parse_server_line(Locale::En, "http://a:8080/").unwrap();
        assert_eq!(plain.weight, DEFAULT_WEIGHT);
        assert_eq!(format_server_line(&plain), "http://a:8080/");

        let err = parse_server_line(Locale::Tr, "http://a:8080/ -1").unwrap_err();
        assert_eq!(
            err,
            Locale::Tr.tf("proxy_form.invalid_weight", &[("value", "-1")])
        );
        assert!(parse_server_line(Locale::En, "http://a:80/ 1 2").is_err());
        assert!(parse_server_line(Locale::En, "http://a:80/ 65536").is_err());
    }

    #[test]
    fn an_srv_server_line_has_no_weight() {
        let srv =
            parse_server_line(Locale::En, "http+srv://_http._tcp.app.example.com/api").unwrap();
        assert_eq!(srv.url.srv_name(), Some("_http._tcp.app.example.com"));
        assert_eq!(
            format_server_line(&srv),
            "http+srv://_http._tcp.app.example.com/api"
        );
        let err =
            parse_server_line(Locale::Tr, "http+srv://_http._tcp.app.example.com/ 3").unwrap_err();
        assert_eq!(err, Locale::Tr.t("proxy_form.srv_weight"));
        assert!(parse_server_line(Locale::En, "dns+srv://_http._tcp.app.example.com/").is_err());
    }

    #[test]
    fn upstream_form_round_trips_and_reports_invalid_values() {
        let health_check = HealthCheck {
            max_fails: 3,
            fail_timeout: Duration::from_secs(10),
            interval: Duration::from_secs(15),
            timeout: Duration::from_secs(2),
            path: "/health".into(),
            grpc_service: None,
        };
        let form = UpstreamForm::new(LoadBalancing::First, &health_check);
        let mut errors = HashMap::new();
        assert_eq!(
            form.parse(Locale::En, true, &mut errors),
            (LoadBalancing::First, health_check)
        );
        assert!(errors.is_empty());

        let grpc_health_check = HealthCheck {
            max_fails: 3,
            fail_timeout: Duration::from_secs(10),
            interval: Duration::from_secs(15),
            timeout: Duration::from_secs(2),
            path: String::new(),
            grpc_service: Some("helloworld.Greeter".into()),
        };
        let form_grpc = UpstreamForm::new(LoadBalancing::First, &grpc_health_check);
        let mut errors = HashMap::new();
        assert_eq!(
            form_grpc.parse(Locale::En, true, &mut errors),
            (LoadBalancing::First, grpc_health_check)
        );
        assert!(errors.is_empty());

        form.parse(Locale::En, false, &mut errors);
        assert!(errors.contains_key(UPSTREAM_KEY), "a TCP proxy has no path");

        let invalid_values = [("x", "10", "0"), ("3", "0", "0"), ("3", "10", "-1")];
        for (max_fails, fail_timeout, interval) in invalid_values {
            let invalid = UpstreamForm {
                max_fails: max_fails.into(),
                fail_timeout: fail_timeout.into(),
                interval: interval.into(),
                ..form.clone()
            };
            let mut errors = HashMap::new();
            invalid.parse(Locale::En, true, &mut errors);
            assert!(errors.contains_key(UPSTREAM_KEY));
        }
    }

    #[test]
    fn error_pages_round_trip_and_report_invalid_values() {
        let pages = [
            ErrorPageForm {
                status: "404".into(),
                body: "<h1>Not Found</h1>".into(),
            },
            ErrorPageForm {
                status: "502".into(),
                body: "<h1>Bad Gateway</h1>".into(),
            },
        ];
        let mut errors = HashMap::new();
        let parsed = parse_error_pages(Locale::En, &pages, &mut errors);
        assert!(errors.is_empty());
        assert_eq!(parsed.len(), 2);
        assert_eq!(parsed[0].status, 404);
        assert_eq!(parsed[1].status, 502);

        let invalid = [ErrorPageForm {
            status: "200".into(),
            body: "ok".into(),
        }];
        let mut errors = HashMap::new();
        parse_error_pages(Locale::En, &invalid, &mut errors);
        assert!(errors.contains_key("error_pages_0_status"));
    }

    #[test]
    fn concurrency_limit_form_round_trips_and_validates_header() {
        let limit = ConcurrencyLimit {
            max: 10,
            header: Some("x-api-key".into()),
        };
        let form = ConcurrencyLimitForm::new(&limit);
        assert_eq!(form.max, "10");
        assert_eq!(form.header, "x-api-key");

        let mut errors = HashMap::new();
        let parsed = parse_concurrency_limit(Locale::En, &form, "test", &mut errors);
        assert!(errors.is_empty());
        assert_eq!(parsed, limit);

        let invalid = ConcurrencyLimitForm {
            max: "abc".into(),
            header: "bad header!".into(),
        };
        let mut errors = HashMap::new();
        let _ = parse_concurrency_limit(Locale::En, &invalid, "test", &mut errors);
        assert!(errors.contains_key("test"));
    }
}
