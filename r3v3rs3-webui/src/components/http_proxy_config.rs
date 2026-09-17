use super::auth_config::{AuthConfig, AuthForm};
use crate::API_ENDPOINT;
use crate::i18n::use_locale;
use crate::pages::cert_list::get_cert_list;
use gloo_net::http::Request;
use r3v3rs3_api::access_list::AccessListEntry;
use r3v3rs3_api::cache::CacheConfig;
use r3v3rs3_api::cert::{CertInfo, CertKind};
use r3v3rs3_api::cidr::{format_cidr_list, parse_cidr_list};
use r3v3rs3_api::client_ip::ClientIpConfig;
use r3v3rs3_api::compression::{
    Compression, CompressionAlgorithm, format_mime_list, parse_mime_list,
};
use r3v3rs3_api::error::Error;
use r3v3rs3_api::fixed_response::{FIXED_STATUSES, FixedRedirect, FixedResponse, FixedStatus};
use r3v3rs3_api::header_rules::{HeaderRule, HeaderRules, format_header_rules};
use r3v3rs3_api::i18n::Locale;
use r3v3rs3_api::id::ShortId;
use r3v3rs3_api::mirror::{DEFAULT_MIRROR_BODY_SIZE, Mirror};
use r3v3rs3_api::policy::{AuthPolicy, IpFilter, RateLimit, RatePeriod};
use r3v3rs3_api::proxy::{HttpProxy, Route, Server, ServerUrl};
use r3v3rs3_api::redirect::{RedirectRule, RedirectStatus};
use r3v3rs3_api::rewrite::{PathRegex, PathRewrite};
use r3v3rs3_api::upstream::{
    CircuitBreaker, DEFAULT_WEIGHT, HealthCheck, LoadBalancing, MAX_RETRY_ATTEMPTS, RetryOn,
    RetryPolicy, StickyCookie, UpstreamTimeouts,
};
use r3v3rs3_api::vhost::VirtualHost;
use std::collections::HashMap;
use std::str::FromStr;
use std::time::Duration;
use wasm_bindgen::{JsCast, UnwrapThrowExt};
use web_sys::{HtmlInputElement, HtmlSelectElement, HtmlTextAreaElement};
use yew::prelude::*;

pub const LABEL_CLASS: &str =
    "block mt-4 mb-2 text-sm font-medium text-neutral-900 dark:text-neutral-200";
const SECTION_CLASS: &str = "block mt-6 text-sm font-medium text-neutral-900 dark:text-neutral-200";
pub const INPUT_CLASS: &str = "bg-neutral-50 dark:text-neutral-200 dark:bg-neutral-800 dark:border-neutral-600 border border-neutral-300 text-neutral-900 text-sm rounded-lg focus:ring-blue-500 focus:border-blue-500 block w-full p-2.5";
pub const HINT_CLASS: &str = "mt-2 text-sm text-neutral-500 dark:text-neutral-400";
const ERROR_CLASS: &str = "mt-2 text-sm text-red-600 dark:text-red-500";
const TOGGLE_CLASS: &str = "shrink-0 w-9 h-5 bg-neutral-200 dark:bg-neutral-600 peer-focus:outline-none peer-focus:ring-4 peer-focus:ring-blue-300 rounded-full peer peer-checked:after:translate-x-full peer-checked:after:border-white after:content-[''] after:absolute after:top-[2px] after:left-[2px] after:bg-white after:border-neutral-300 after:border after:rounded-full after:h-4 after:w-4 after:transition-all peer-checked:bg-blue-600";
pub(super) const BUTTON_CLASS: &str = "inline-flex items-center px-4 py-2 text-sm font-medium text-neutral-500 dark:text-neutral-200 bg-white dark:bg-neutral-800 border border-neutral-300 dark:border-neutral-700 hover:bg-neutral-100 hover:dark:bg-neutral-900 focus:z-10 focus:ring-4 focus:ring-neutral-200 dark:focus:ring-neutral-600";

#[derive(Properties, PartialEq)]
pub struct Props {
    #[prop_or_default]
    pub proxy: HttpProxy,
    pub onchanged: Callback<Result<HttpProxy, HashMap<String, String>>>,
}

#[derive(Clone, PartialEq)]
struct ProxyForm {
    vhosts: String,
    upgrade_insecure: bool,
    trust_cdn: bool,
    trusted_proxies: String,
    allow: String,
    deny: String,
    rate_limit: RateLimitForm,
    auth: AuthForm,
    access_list: Option<ShortId>,
    request_headers: String,
    response_headers: String,
    compression: CompressionForm,
    cache: CacheForm,
    h2c: bool,
    client_cert: Option<ShortId>,
    timeouts: TimeoutsForm,
    retry: RetryForm,
    sticky: StickyForm,
    max_body_size: String,
    /// One redirect rule on each line.
    redirects: String,
}

impl ProxyForm {
    fn new(proxy: &HttpProxy) -> Self {
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
            rate_limit: RateLimitForm::new(&proxy.rate_limit),
            auth: AuthForm::new(&proxy.auth),
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
        }
    }
}

#[derive(Clone, PartialEq)]
struct TimeoutsForm {
    connect: String,
    request: String,
}

impl TimeoutsForm {
    fn new(timeouts: &UpstreamTimeouts) -> Self {
        Self {
            connect: format_seconds(timeouts.connect),
            request: format_seconds(timeouts.request),
        }
    }
}

#[derive(Clone, PartialEq)]
struct RetryForm {
    attempts: String,
    retry_on: Vec<RetryOn>,
    replay_body_limit: String,
}

impl RetryForm {
    fn new(retry: &RetryPolicy) -> Self {
        Self {
            attempts: retry.attempts.to_string(),
            retry_on: retry.retry_on.clone(),
            replay_body_limit: retry.replay_body_limit.to_string(),
        }
    }
}

#[derive(Clone, PartialEq)]
struct StickyForm {
    enabled: bool,
    name: String,
    /// `0` is a cookie without `Max-Age`.
    max_age: String,
}

impl StickyForm {
    fn new(sticky: &StickyCookie) -> Self {
        Self {
            enabled: sticky.enabled,
            name: sticky.name.clone(),
            max_age: sticky.max_age.map_or_else(|| "0".into(), format_seconds),
        }
    }
}

/// The errors of an [`UpstreamForm`] use this key.
pub(super) const UPSTREAM_KEY: &str = "upstream";

/// The load balancing policy and the health check of a proxy.
#[derive(Clone, PartialEq)]
pub(super) struct UpstreamForm {
    load_balancing: LoadBalancing,
    max_fails: String,
    fail_timeout: String,
    interval: String,
    timeout: String,
    path: String,
}

impl UpstreamForm {
    pub(super) fn new(load_balancing: LoadBalancing, health_check: &HealthCheck) -> Self {
        Self {
            load_balancing,
            max_fails: health_check.max_fails.to_string(),
            fail_timeout: format_seconds(health_check.fail_timeout),
            interval: format_seconds(health_check.interval),
            timeout: format_seconds(health_check.timeout),
            path: health_check.path.clone(),
        }
    }

    /// Records an error under [`UPSTREAM_KEY`]. `http` allows a health check path.
    pub(super) fn parse(
        &self,
        locale: Locale,
        http: bool,
        errors: &mut HashMap<String, String>,
    ) -> (LoadBalancing, HealthCheck) {
        let mut seconds = |value: &str, name_key: &str, min: u64| {
            or_error(
                parse_seconds(locale, value, name_key, min),
                UPSTREAM_KEY,
                errors,
            )
        };
        let fail_timeout = seconds(&self.fail_timeout, "proxy_form.fail_timeout_name", 1);
        let interval = seconds(&self.interval, "proxy_form.check_interval_name", 0);
        let timeout = seconds(&self.timeout, "proxy_form.check_timeout_name", 1);
        let health_check = HealthCheck {
            max_fails: or_error(
                parse_count(locale, &self.max_fails, "proxy_form.max_fails"),
                UPSTREAM_KEY,
                errors,
            ),
            fail_timeout,
            interval,
            timeout,
            path: self.path.trim().to_string(),
        };
        if let Err(err) = health_check.validate(http) {
            errors
                .entry(UPSTREAM_KEY.to_string())
                .or_insert_with(|| locale.error_message(&err));
        }
        (self.load_balancing, health_check)
    }
}

#[hook]
pub(super) fn use_upstream_form(
    load_balancing: LoadBalancing,
    health_check: HealthCheck,
) -> UseStateHandle<UpstreamForm> {
    use_state(move || UpstreamForm::new(load_balancing, &health_check))
}

/// The errors of a [`CircuitBreakerForm`] use this key.
pub(super) const CIRCUIT_BREAKER_KEY: &str = "circuit_breaker";

/// The circuit breaker of an HTTP or TCP proxy.
#[derive(Clone, PartialEq, Debug)]
pub(super) struct CircuitBreakerForm {
    enabled: bool,
    failure_ratio: String,
    min_requests: String,
    window: String,
    open_duration: String,
}

impl CircuitBreakerForm {
    pub(super) fn new(breaker: &CircuitBreaker) -> Self {
        Self {
            enabled: breaker.enabled,
            failure_ratio: breaker.failure_ratio.to_string(),
            min_requests: breaker.min_requests.to_string(),
            window: format_seconds(breaker.window),
            open_duration: format_seconds(breaker.open_duration),
        }
    }

    /// Records an error under [`CIRCUIT_BREAKER_KEY`].
    pub(super) fn parse(
        &self,
        locale: Locale,
        errors: &mut HashMap<String, String>,
    ) -> CircuitBreaker {
        let key = CIRCUIT_BREAKER_KEY;
        let failure_ratio = or_error(
            parse_percent(locale, &self.failure_ratio, "proxy_form.failure_ratio_name"),
            key,
            errors,
        );
        let min_requests = or_error(
            parse_count(locale, &self.min_requests, "proxy_form.min_requests_name"),
            key,
            errors,
        );
        let mut seconds = |value: &str, name_key: &str| {
            or_error(parse_seconds(locale, value, name_key, 1), key, errors)
        };
        let window = seconds(&self.window, "proxy_form.breaker_window_name");
        let open_duration = seconds(&self.open_duration, "proxy_form.open_duration_name");
        let breaker = CircuitBreaker {
            enabled: self.enabled,
            failure_ratio,
            min_requests,
            window,
            open_duration,
        };
        if let Err(err) = breaker.validate() {
            errors
                .entry(key.to_string())
                .or_insert_with(|| locale.error_message(&err));
        }
        breaker
    }
}

fn parse_percent(locale: Locale, value: &str, name_key: &str) -> Result<u8, String> {
    value
        .trim()
        .parse::<u8>()
        .ok()
        .filter(|percent| (1..=100).contains(percent))
        .ok_or_else(|| locale.tf("proxy_form.percent_range", &[("name", locale.t(name_key))]))
}

/// The circuit breaker toggle and its inputs. `hint_key` explains the breaker for the protocol.
pub(super) fn circuit_breaker_view(
    locale: Locale,
    form: &UseStateHandle<CircuitBreakerForm>,
    errors: &HashMap<String, String>,
    hint_key: &'static str,
) -> Html {
    html! {
        <>
            <label class={SECTION_CLASS}>{locale.t("proxy_form.circuit_breaker")}</label>
            <div>
                { toggle(state_input(form, checked, |form, value| form.enabled = value), form.enabled, locale.t("proxy_form.enable_circuit_breaker"), "mt-4") }
            </div>
            if form.enabled {
                <div class="grid grid-cols-1 sm:grid-cols-2 gap-x-4">
                    <div>
                        <label class={LABEL_CLASS}>{locale.t("proxy_form.failure_ratio")}</label>
                        <input type="number" min="1" max="100" value={form.failure_ratio.clone()} onchange={state_input(form, text, |form, value| form.failure_ratio = value)} class={INPUT_CLASS} />
                    </div>
                    <div>
                        <label class={LABEL_CLASS}>{locale.t("proxy_form.min_requests")}</label>
                        <input type="number" min="1" value={form.min_requests.clone()} onchange={state_input(form, text, |form, value| form.min_requests = value)} class={INPUT_CLASS} />
                    </div>
                    <div>{ seconds_input(locale.t("proxy_form.breaker_window"), &form.window, 1, state_input(form, text, |form, value| form.window = value)) }</div>
                    <div>{ seconds_input(locale.t("proxy_form.open_duration"), &form.open_duration, 1, state_input(form, text, |form, value| form.open_duration = value)) }</div>
                </div>
            }
            { error_view(errors.get(CIRCUIT_BREAKER_KEY)) }
            <p class={HINT_CLASS}>{locale.t(hint_key)}</p>
        </>
    }
}

#[derive(Clone, PartialEq)]
struct CacheForm {
    enabled: bool,
    max_size: String,
    max_entry_size: String,
    default_ttl: String,
}

impl CacheForm {
    fn new(cache: &CacheConfig) -> Self {
        Self {
            enabled: cache.enabled,
            max_size: cache.max_size.to_string(),
            max_entry_size: cache.max_entry_size.to_string(),
            default_ttl: cache.default_ttl.as_secs().to_string(),
        }
    }
}

#[derive(Clone, PartialEq)]
struct CompressionForm {
    /// Selected algorithms in the order of preference.
    algorithms: Vec<CompressionAlgorithm>,
    min_size: String,
    mime_types: String,
}

impl CompressionForm {
    fn new(compression: &Compression) -> Self {
        Self {
            algorithms: compression.algorithms.clone(),
            min_size: compression.min_size.to_string(),
            mime_types: format_mime_list(&compression.mime_types),
        }
    }
}

#[derive(Clone, PartialEq)]
struct RouteForm {
    path: String,
    response: ResponseForm,
    servers: Vec<String>,
    override_ip_filter: bool,
    allow: String,
    deny: String,
    override_rate_limit: bool,
    rate_limit: RateLimitForm,
    override_auth: bool,
    auth: AuthForm,
    access_list: Option<ShortId>,
    override_headers: bool,
    request_headers: String,
    response_headers: String,
    override_timeouts: bool,
    timeouts: TimeoutsForm,
    override_retry: bool,
    retry: RetryForm,
    override_body_limit: bool,
    max_body_size: String,
    strip_prefix: bool,
    path_regex: String,
    replacement: String,
    add_prefix: String,
    mirror: bool,
    mirror_servers: Vec<String>,
    mirror_percent: String,
    mirror_max_body_size: String,
}

impl RouteForm {
    fn new(route: &Route) -> Self {
        let ip_filter = route.ip_filter.clone().unwrap_or_default();
        let mirror = route.mirror.clone().unwrap_or_default();
        Self {
            path: route.path.clone(),
            response: ResponseForm::new(route.response.as_ref()),
            servers: route.servers.iter().map(format_server_line).collect(),
            override_ip_filter: route.ip_filter.is_some(),
            allow: format_cidr_list(&ip_filter.allow),
            deny: format_cidr_list(&ip_filter.deny),
            override_rate_limit: route.rate_limit.is_some(),
            rate_limit: RateLimitForm::new(&route.rate_limit.unwrap_or_default()),
            override_auth: route.auth.is_some(),
            auth: AuthForm::new(&route.auth.clone().unwrap_or_default()),
            access_list: route.access_list,
            override_headers: route.headers.is_some(),
            request_headers: route
                .headers
                .as_ref()
                .map(|rules| format_header_rules(&rules.request))
                .unwrap_or_default(),
            response_headers: route
                .headers
                .as_ref()
                .map(|rules| format_header_rules(&rules.response))
                .unwrap_or_default(),
            override_timeouts: route.timeouts.is_some(),
            timeouts: TimeoutsForm::new(&route.timeouts.unwrap_or_default()),
            override_retry: route.retry.is_some(),
            retry: RetryForm::new(&route.retry.clone().unwrap_or_default()),
            override_body_limit: route.max_body_size.is_some(),
            max_body_size: route.max_body_size.unwrap_or_default().to_string(),
            strip_prefix: route.rewrite.strip_prefix,
            path_regex: route
                .rewrite
                .regex
                .as_ref()
                .map(ToString::to_string)
                .unwrap_or_default(),
            replacement: route.rewrite.replacement.clone(),
            add_prefix: route.rewrite.add_prefix.clone(),
            mirror: route.mirror.is_some(),
            mirror_servers: mirror.servers.iter().map(format_server_line).collect(),
            mirror_percent: mirror.percent.to_string(),
            mirror_max_body_size: mirror.max_body_size.to_string(),
        }
    }

    fn empty() -> Self {
        Self {
            path: "/".into(),
            response: ResponseForm::new(None),
            servers: Vec::new(),
            override_ip_filter: false,
            allow: String::new(),
            deny: String::new(),
            override_rate_limit: false,
            rate_limit: RateLimitForm::new(&RateLimit::default()),
            override_auth: false,
            auth: AuthForm::new(&Default::default()),
            access_list: None,
            override_headers: false,
            request_headers: String::new(),
            response_headers: String::new(),
            override_timeouts: false,
            timeouts: TimeoutsForm::new(&UpstreamTimeouts::default()),
            override_retry: false,
            retry: RetryForm::new(&RetryPolicy::default()),
            override_body_limit: false,
            max_body_size: "0".into(),
            strip_prefix: true,
            path_regex: String::new(),
            replacement: String::new(),
            add_prefix: String::new(),
            mirror: false,
            mirror_servers: Vec::new(),
            mirror_percent: "100".into(),
            mirror_max_body_size: DEFAULT_MIRROR_BODY_SIZE.to_string(),
        }
    }
}

/// What answers the requests of a route.
#[derive(Clone, Copy, Default, PartialEq)]
enum RouteKind {
    #[default]
    Proxy,
    Redirect,
    Status,
}

const ROUTE_KINDS: [(RouteKind, &str, &str); 3] = [
    (RouteKind::Proxy, "proxy", "http_form.route_kind_proxy"),
    (
        RouteKind::Redirect,
        "redirect",
        "http_form.route_kind_redirect",
    ),
    (RouteKind::Status, "status", "http_form.route_kind_status"),
];

/// The route type and the fields of the fixed response types.
#[derive(Clone, PartialEq)]
struct ResponseForm {
    kind: RouteKind,
    redirect_target: String,
    redirect_status: RedirectStatus,
    preserve_path: bool,
    fixed_status: u16,
    fixed_body: String,
}

impl ResponseForm {
    fn new(response: Option<&FixedResponse>) -> Self {
        let form = Self {
            kind: RouteKind::Proxy,
            redirect_target: String::new(),
            redirect_status: RedirectStatus::default(),
            preserve_path: true,
            fixed_status: 404,
            fixed_body: String::new(),
        };
        match response {
            None => form,
            Some(FixedResponse::Redirect(redirect)) => Self {
                kind: RouteKind::Redirect,
                redirect_target: redirect.target.clone(),
                redirect_status: redirect.status,
                preserve_path: redirect.preserve_path,
                ..form
            },
            Some(FixedResponse::Status(status)) => Self {
                kind: RouteKind::Status,
                fixed_status: status.status,
                fixed_body: status.body.clone(),
                ..form
            },
        }
    }

    /// The fixed response of the route. `None` for a route that proxies to its servers.
    fn parse(&self, locale: Locale) -> Result<Option<FixedResponse>, String> {
        let response = match self.kind {
            RouteKind::Proxy => return Ok(None),
            RouteKind::Redirect => FixedResponse::Redirect(FixedRedirect {
                target: self.redirect_target.trim().to_string(),
                status: self.redirect_status,
                preserve_path: self.preserve_path,
            }),
            RouteKind::Status => FixedResponse::Status(FixedStatus {
                status: self.fixed_status,
                body: self.fixed_body.clone(),
            }),
        };
        response
            .validate()
            .map_err(|err| locale.error_message(&err))?;
        Ok(Some(response))
    }
}

#[derive(Clone, PartialEq)]
struct RateLimitForm {
    requests: String,
    per: RatePeriod,
    burst: String,
}

impl RateLimitForm {
    fn new(limit: &RateLimit) -> Self {
        Self {
            requests: limit.requests.to_string(),
            per: limit.per,
            burst: limit.burst.to_string(),
        }
    }
}

#[function_component(HttpProxyConfig)]
pub fn http_proxy_config(props: &Props) -> Html {
    let locale = use_locale();
    let form = use_state(|| ProxyForm::new(&props.proxy));
    let upstream = use_upstream_form(props.proxy.load_balancing, props.proxy.health_check.clone());
    let circuit_breaker = use_state(|| CircuitBreakerForm::new(&props.proxy.circuit_breaker));
    let client_certs = use_client_certs();
    let access_lists = use_access_lists();
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

    let errors = use_entry_errors(
        get_proxy(locale, &form, &routes, &upstream, &circuit_breaker),
        props.onchanged.clone(),
    );

    html! {
        <>
            { toggle(state_input(&form, checked, |form, value| form.upgrade_insecure = value), form.upgrade_insecure, locale.t("http_form.upgrade_insecure"), "my-6") }

            <label class={LABEL_CLASS}>{locale.t("http_form.vhosts")}</label>
            <input type="text" autocapitalize="off" value={form.vhosts.clone()} onchange={state_input(&form, text, |form, value| form.vhosts = value)} class={INPUT_CLASS} placeholder="example.com" />
            { error_view(errors.get("vhosts")) }
            <p class={HINT_CLASS}>{locale.t("http_form.vhosts_hint")}</p>

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
            ) }
            { error_view(errors.get("headers")) }
            <p class={HINT_CLASS}>{locale.t("http_form.header_rules_hint")}</p>

            <label class={SECTION_CLASS}>{locale.t("http_form.redirects")}</label>
            <textarea rows="3" autocapitalize="off" spellcheck="false" placeholder={"301 ^example\\.com/old/(.*)$ https://example.com/new/${1}"} value={form.redirects.clone()} onchange={state_input(&form, text_area, |form, value| form.redirects = value)} class={classes!(INPUT_CLASS, "font-mono")} />
            { error_view(errors.get("redirects")) }
            <p class={HINT_CLASS}>{locale.t("http_form.redirects_hint")}</p>

            <label class={SECTION_CLASS}>{locale.t("http_form.compression")}</label>
            { compression_view(locale, &form) }
            { error_view(errors.get("compression")) }
            <p class={HINT_CLASS}>{locale.t("http_form.compression_hint")}</p>

            <label class={SECTION_CLASS}>{locale.t("http_form.cache")}</label>
            { cache_view(locale, &form) }
            { error_view(errors.get("cache")) }
            <p class={HINT_CLASS}>{locale.t("http_form.cache_hint")}</p>

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
            { sticky_view(locale, &form) }
            { error_view(errors.get("sticky")) }
            <p class={HINT_CLASS}>{locale.t("http_form.sticky_hint")}</p>

            { upstream_form_view(locale, &upstream, &errors, true) }

            { circuit_breaker_view(locale, &circuit_breaker, &errors, "proxy_form.circuit_breaker_hint") }

            <label class={SECTION_CLASS}>{locale.t("http_form.routes")}</label>
            <p class={HINT_CLASS}>{locale.t("http_form.routes_hint")}</p>

            { routes.iter().enumerate().map(|(i, route)| {
                route_view(locale, &routes, i, route, &access_lists, errors.get(&format!("routes_{i}")))
            }).collect::<Html>() }
        </>
    }
}

fn route_view(
    locale: Locale,
    routes: &UseStateHandle<Vec<RouteForm>>,
    index: usize,
    route: &RouteForm,
    access_lists: &[AccessListEntry],
    error: Option<&String>,
) -> Html {
    html! {
        <div class="mt-2 bg-white dark:text-neutral-200 dark:bg-neutral-800 shadow-sm p-5 border border-neutral-300 dark:border-neutral-700 rounded-md">
            <label class="block mb-2 text-sm font-medium text-neutral-900 dark:text-neutral-200">{locale.t("http_form.path")}</label>
            <input type="text" autocapitalize="off" placeholder="/" onchange={route_input(routes, index, text, |route, value| route.path = value)} value={route.path.clone()} class={INPUT_CLASS} />

            { select_field(
                locale.t("http_form.route_kind"),
                route_input(routes, index, route_kind, |route, value| route.response.kind = value),
                super::port_config::option_list(locale, &ROUTE_KINDS, route.response.kind),
                None,
            ) }

            if route.response.kind == RouteKind::Proxy {
                <label class={LABEL_CLASS}>{locale.t("http_form.target")}</label>
                <textarea rows="2" autocapitalize="off" spellcheck="false" placeholder={"https://a.example.com/backend\nhttps://b.example.com/backend"} value={route.servers.join("\n")} onchange={route_input(routes, index, text_area, |route, value| route.servers = server_lines(&value))} class={INPUT_CLASS} />
                <p class={HINT_CLASS}>{locale.t("http_form.target_hint")}</p>

                { route_rewrite_view(locale, routes, index, route) }
            } else {
                { route_response_view(locale, routes, index, &route.response) }
            }

            { route_access_list_view(locale, routes, index, route, access_lists) }
            if route.access_list.is_none() {
                { route_ip_filter_view(locale, routes, index, route) }
            }
            { route_rate_limit_view(locale, routes, index, route) }
            if route.access_list.is_none() {
                { route_auth_view(locale, routes, index, route) }
            }
            { route_headers_view(locale, routes, index, route) }
            if route.response.kind == RouteKind::Proxy {
                { route_timeouts_view(locale, routes, index, route) }
                { route_retry_view(locale, routes, index, route) }
                { route_body_limit_view(locale, routes, index, route) }
                { route_mirror_view(locale, routes, index, route) }
            }
            { error_view(error) }

            { list_buttons(routes, index, RouteForm::empty) }
        </div>
    }
}

/// The fields of a redirect route or a fixed status route.
fn route_response_view(
    locale: Locale,
    routes: &UseStateHandle<Vec<RouteForm>>,
    index: usize,
    response: &ResponseForm,
) -> Html {
    let redirect_statuses = RedirectStatus::ALL
        .iter()
        .map(|status| code_option(status.code(), response.redirect_status.code()))
        .collect::<Html>();
    let fixed_statuses = FIXED_STATUSES
        .iter()
        .map(|&status| code_option(status, response.fixed_status))
        .collect::<Html>();
    html! {
        <>
            if response.kind == RouteKind::Redirect {
                <label class={LABEL_CLASS}>{locale.t("http_form.redirect_target")}</label>
                <input type="text" autocapitalize="off" placeholder="https://example.com" value={response.redirect_target.clone()} onchange={route_input(routes, index, text, |route, value| route.response.redirect_target = value)} class={INPUT_CLASS} />
                { select_field(
                    locale.t("http_form.redirect_status"),
                    route_input(routes, index, redirect_status, |route, value| route.response.redirect_status = value),
                    redirect_statuses,
                    None,
                ) }
                <div>
                    { toggle(route_input(routes, index, checked, |route, value| route.response.preserve_path = value), response.preserve_path, locale.t("http_form.preserve_path"), "mt-4") }
                </div>
            } else {
                { select_field(
                    locale.t("http_form.fixed_status"),
                    route_input(routes, index, status_code, |route, value| route.response.fixed_status = value),
                    fixed_statuses,
                    None,
                ) }
                <label class={LABEL_CLASS}>{locale.t("http_form.fixed_body")}</label>
                <textarea rows="3" spellcheck="false" value={response.fixed_body.clone()} onchange={route_input(routes, index, text_area, |route, value| route.response.fixed_body = value)} class={INPUT_CLASS} />
            }
            <p class={HINT_CLASS}>{locale.t("http_form.fixed_response_hint")}</p>
        </>
    }
}

fn code_option(code: u16, selected: u16) -> Html {
    html! {
        <option selected={code == selected} value={code.to_string()}>{code.to_string()}</option>
    }
}

/// Writes a server as its URL, followed by its weight when the weight is not the default.
fn format_server_line(server: &Server) -> String {
    if server.weight == DEFAULT_WEIGHT {
        server.url.to_string()
    } else {
        format!("{} {}", server.url, server.weight)
    }
}

/// Reads a server line: the URL, and an optional weight after a space. An SRV URL has no
/// weight, because the SRV records give the weights.
fn parse_server_line(locale: Locale, line: &str) -> Result<Server, String> {
    let mut parts = line.split_whitespace();
    let url = ServerUrl::from_str(parts.next().unwrap_or_default())
        .map_err(|err| locale.error_message(&err))?;
    let rest = parts.collect::<Vec<_>>();
    let weight = match (url.srv_name().is_some(), rest.as_slice()) {
        (_, []) => DEFAULT_WEIGHT,
        (true, _) => return Err(locale.t("proxy_form.srv_weight").into()),
        (false, [value]) => parse_weight(locale, value)?,
        (false, _) => parse_weight(locale, &rest.join(" "))?,
    };
    Ok(Server { url, weight })
}

/// Reads a server weight from 0 to 65535.
pub(super) fn parse_weight(locale: Locale, value: &str) -> Result<u16, String> {
    let value = value.trim();
    value
        .parse()
        .map_err(|_| locale.tf("proxy_form.invalid_weight", &[("value", value)]))
}

/// Reads one upstream server URL from each line and skips the empty lines.
fn server_lines(text: &str) -> Vec<String> {
    text.lines()
        .map(str::trim)
        .filter(|line| !line.is_empty())
        .map(str::to_string)
        .collect()
}

/// The buttons that add a new item after the item at the index and remove the item. The last
/// item cannot be removed.
pub(super) fn list_buttons<T: Clone + 'static>(
    items: &UseStateHandle<Vec<T>>,
    index: usize,
    new_item: fn() -> T,
) -> Html {
    let add_onclick = {
        let items = items.clone();
        Callback::from(move |_: MouseEvent| {
            let mut list = (*items).clone();
            list.insert(index + 1, new_item());
            items.set(list);
        })
    };

    let remove_onclick = {
        let items = items.clone();
        Callback::from(move |_: MouseEvent| {
            if items.len() > 1 {
                let mut list = (*items).clone();
                list.remove(index);
                items.set(list);
            }
        })
    };

    html! {
        <div class="flex justify-end rounded-md mt-4 sm:ml-auto" role="group">
            <button type="button" onclick={add_onclick} class={classes!(BUTTON_CLASS, "rounded-l-lg")}>
                <img src="/assets/icons/add.svg" class="w-4 h-4" />
            </button>
            <button type="button" onclick={remove_onclick} disabled={items.len() <= 1} class={classes!(BUTTON_CLASS, "border-l-0", "rounded-r-lg")}>
                <img src="/assets/icons/remove.svg" class="w-4 h-4" />
            </button>
        </div>
    }
}

/// The load balancing policy select element and the health check inputs of a proxy. `with_path`
/// shows the health check path input of an HTTP proxy.
pub(super) fn upstream_form_view(
    locale: Locale,
    form: &UseStateHandle<UpstreamForm>,
    errors: &HashMap<String, String>,
    with_path: bool,
) -> Html {
    let options = html! {
        { for LoadBalancing::ALL.iter().map(|value| html! {
            <option value={value.as_str()} selected={*value == form.load_balancing}>{locale.t(&format!("load_balancing.{}", value.as_str()))}</option>
        }) }
    };
    html! {
        <>
            { select_field(
                locale.t("proxy_form.load_balancing"),
                state_input(form, load_balancing, |form, value| form.load_balancing = value),
                options,
                Some(locale.t("proxy_form.load_balancing_hint")),
            ) }
            <div class="grid grid-cols-1 sm:grid-cols-2 gap-x-4">
                <div>
                    <label class={LABEL_CLASS}>{locale.t("proxy_form.max_fails")}</label>
                    <input type="number" min="0" value={form.max_fails.clone()} onchange={state_input(form, text, |form, value| form.max_fails = value)} class={INPUT_CLASS} />
                </div>
                <div>{ seconds_input(locale.t("proxy_form.fail_timeout"), &form.fail_timeout, 1, state_input(form, text, |form, value| form.fail_timeout = value)) }</div>
            </div>
            <p class={HINT_CLASS}>{locale.t("proxy_form.health_check_hint")}</p>
            <div class="grid grid-cols-1 sm:grid-cols-2 gap-x-4">
                <div>{ seconds_input(locale.t("proxy_form.check_interval"), &form.interval, 0, state_input(form, text, |form, value| form.interval = value)) }</div>
                <div>{ seconds_input(locale.t("proxy_form.check_timeout"), &form.timeout, 1, state_input(form, text, |form, value| form.timeout = value)) }</div>
            </div>
            if with_path {
                <label class={LABEL_CLASS}>{locale.t("proxy_form.health_check_path")}</label>
                <input type="text" autocapitalize="off" placeholder="/health" value={form.path.clone()} onchange={state_input(form, text, |form, value| form.path = value)} class={INPUT_CLASS} />
            }
            { error_view(errors.get(UPSTREAM_KEY)) }
            <p class={HINT_CLASS}>{locale.t(if with_path { "proxy_form.active_check_http_hint" } else { "proxy_form.active_check_hint" })}</p>
        </>
    }
}

/// The toggle of a route setting that replaces the proxy setting. The form and the hint show
/// when the toggle is on.
fn route_override_view(
    locale: Locale,
    onchange: Callback<Event>,
    enabled: bool,
    label_key: &'static str,
    hint_key: &'static str,
    form: Html,
) -> Html {
    html! {
        <>
            <div>
                { toggle(onchange, enabled, locale.t(label_key), "mt-6") }
            </div>
            if enabled {
                { form }
                <p class={HINT_CLASS}>{locale.t(hint_key)}</p>
            }
        </>
    }
}

fn route_access_list_view(
    locale: Locale,
    routes: &UseStateHandle<Vec<RouteForm>>,
    index: usize,
    route: &RouteForm,
    access_lists: &[AccessListEntry],
) -> Html {
    access_list_view(
        locale,
        route_input(routes, index, select_value, |route, value| {
            route.access_list = parse_optional_id(&value)
        }),
        route.access_list,
        access_lists,
        "http_form.route_access_list_hint",
    )
}

fn route_ip_filter_view(
    locale: Locale,
    routes: &UseStateHandle<Vec<RouteForm>>,
    index: usize,
    route: &RouteForm,
) -> Html {
    route_override_view(
        locale,
        route_input(routes, index, checked, |route, value| {
            route.override_ip_filter = value
        }),
        route.override_ip_filter,
        "http_form.override_ip_filter",
        "http_form.route_ip_filter_hint",
        html! {
            <>
                <label class={LABEL_CLASS}>{locale.t("http_form.allow")}</label>
                <input type="text" autocapitalize="off" placeholder="192.168.0.0/16" value={route.allow.clone()} onchange={route_input(routes, index, text, |route, value| route.allow = value)} class={INPUT_CLASS} />

                <label class={LABEL_CLASS}>{locale.t("http_form.deny")}</label>
                <input type="text" autocapitalize="off" placeholder="203.0.113.0/24" value={route.deny.clone()} onchange={route_input(routes, index, text, |route, value| route.deny = value)} class={INPUT_CLASS} />
            </>
        },
    )
}

fn route_rate_limit_view(
    locale: Locale,
    routes: &UseStateHandle<Vec<RouteForm>>,
    index: usize,
    route: &RouteForm,
) -> Html {
    route_override_view(
        locale,
        route_input(routes, index, checked, |route, value| {
            route.override_rate_limit = value
        }),
        route.override_rate_limit,
        "http_form.override_rate_limit",
        "http_form.route_rate_limit_hint",
        rate_limit_view(
            locale,
            &route.rate_limit,
            route_input(routes, index, text, |route, value| {
                route.rate_limit.requests = value
            }),
            route_input(routes, index, period, |route, value| {
                route.rate_limit.per = value
            }),
            route_input(routes, index, text, |route, value| {
                route.rate_limit.burst = value
            }),
        ),
    )
}

fn route_auth_view(
    locale: Locale,
    routes: &UseStateHandle<Vec<RouteForm>>,
    index: usize,
    route: &RouteForm,
) -> Html {
    route_override_view(
        locale,
        route_input(routes, index, checked, |route, value| {
            route.override_auth = value
        }),
        route.override_auth,
        "http_form.override_auth",
        "http_form.route_auth_hint",
        html! {
            <AuthConfig form={route.auth.clone()} onchange={item_update(routes, index, |route, value| route.auth = value)} />
        },
    )
}

fn route_headers_view(
    locale: Locale,
    routes: &UseStateHandle<Vec<RouteForm>>,
    index: usize,
    route: &RouteForm,
) -> Html {
    route_override_view(
        locale,
        route_input(routes, index, checked, |route, value| {
            route.override_headers = value
        }),
        route.override_headers,
        "http_form.override_headers",
        "http_form.route_headers_hint",
        header_rules_view(
            locale,
            &route.request_headers,
            &route.response_headers,
            route_input(routes, index, text_area, |route, value| {
                route.request_headers = value
            }),
            route_input(routes, index, text_area, |route, value| {
                route.response_headers = value
            }),
        ),
    )
}

fn route_timeouts_view(
    locale: Locale,
    routes: &UseStateHandle<Vec<RouteForm>>,
    index: usize,
    route: &RouteForm,
) -> Html {
    route_override_view(
        locale,
        route_input(routes, index, checked, |route, value| {
            route.override_timeouts = value
        }),
        route.override_timeouts,
        "http_form.override_timeouts",
        "http_form.route_timeouts_hint",
        timeouts_view(
            locale,
            &route.timeouts,
            route_input(routes, index, text, |route, value| {
                route.timeouts.connect = value
            }),
            route_input(routes, index, text, |route, value| {
                route.timeouts.request = value
            }),
        ),
    )
}

fn route_retry_view(
    locale: Locale,
    routes: &UseStateHandle<Vec<RouteForm>>,
    index: usize,
    route: &RouteForm,
) -> Html {
    route_override_view(
        locale,
        route_input(routes, index, checked, |route, value| {
            route.override_retry = value
        }),
        route.override_retry,
        "http_form.override_retry",
        "http_form.route_retry_hint",
        retry_view(
            locale,
            &route.retry,
            item_update(routes, index, |route, value| route.retry = value),
        ),
    )
}

/// The path rewrite of a route: the route path toggle, the regex, its replacement and the prefix.
fn route_rewrite_view(
    locale: Locale,
    routes: &UseStateHandle<Vec<RouteForm>>,
    index: usize,
    route: &RouteForm,
) -> Html {
    html! {
        <>
            <div>
                { toggle(route_input(routes, index, checked, |route, value| route.strip_prefix = value), route.strip_prefix, locale.t("http_form.strip_prefix"), "mt-4") }
            </div>
            <div class="grid grid-cols-1 sm:grid-cols-3 gap-x-4">
                <div>
                    <label class={LABEL_CLASS}>{locale.t("http_form.path_regex")}</label>
                    <input type="text" autocapitalize="off" spellcheck="false" placeholder="^/items/([0-9]+)$" value={route.path_regex.clone()} onchange={route_input(routes, index, text, |route, value| route.path_regex = value)} class={classes!(INPUT_CLASS, "font-mono")} />
                </div>
                <div>
                    <label class={LABEL_CLASS}>{locale.t("http_form.replacement")}</label>
                    <input type="text" autocapitalize="off" spellcheck="false" placeholder="/item/${1}" value={route.replacement.clone()} onchange={route_input(routes, index, text, |route, value| route.replacement = value)} class={classes!(INPUT_CLASS, "font-mono")} />
                </div>
                <div>
                    <label class={LABEL_CLASS}>{locale.t("http_form.add_prefix")}</label>
                    <input type="text" autocapitalize="off" spellcheck="false" placeholder="/v2" value={route.add_prefix.clone()} onchange={route_input(routes, index, text, |route, value| route.add_prefix = value)} class={INPUT_CLASS} />
                </div>
            </div>
            <p class={HINT_CLASS}>{locale.t("http_form.path_rewrite_hint")}</p>
        </>
    }
}

fn route_body_limit_view(
    locale: Locale,
    routes: &UseStateHandle<Vec<RouteForm>>,
    index: usize,
    route: &RouteForm,
) -> Html {
    route_override_view(
        locale,
        route_input(routes, index, checked, |route, value| {
            route.override_body_limit = value
        }),
        route.override_body_limit,
        "http_form.override_body_limit",
        "http_form.route_body_limit_hint",
        body_limit_input(
            locale,
            &route.max_body_size,
            route_input(routes, index, text, |route, value| {
                route.max_body_size = value
            }),
        ),
    )
}

/// The mirror servers of a route, the share of the copied requests and the copy body limit.
fn route_mirror_view(
    locale: Locale,
    routes: &UseStateHandle<Vec<RouteForm>>,
    index: usize,
    route: &RouteForm,
) -> Html {
    route_override_view(
        locale,
        route_input(routes, index, checked, |route, value| route.mirror = value),
        route.mirror,
        "http_form.enable_mirror",
        "http_form.mirror_hint",
        html! {
            <>
                <label class={LABEL_CLASS}>{locale.t("http_form.mirror_servers")}</label>
                <textarea rows="2" autocapitalize="off" spellcheck="false" placeholder={"http://shadow.example.com/backend"} value={route.mirror_servers.join("\n")} onchange={route_input(routes, index, text_area, |route, value| route.mirror_servers = server_lines(&value))} class={INPUT_CLASS} />
                <div class="grid grid-cols-1 sm:grid-cols-2 gap-x-4">
                    <div>
                        <label class={LABEL_CLASS}>{locale.t("http_form.mirror_percent")}</label>
                        <input type="number" min="1" max="100" value={route.mirror_percent.clone()} onchange={route_input(routes, index, text, |route, value| route.mirror_percent = value)} class={INPUT_CLASS} />
                    </div>
                    <div>{ body_limit_input(locale, &route.mirror_max_body_size, route_input(routes, index, text, |route, value| route.mirror_max_body_size = value)) }</div>
                </div>
            </>
        },
    )
}

/// The labeled number input of a request body limit in bytes.
fn body_limit_input(locale: Locale, value: &str, onchange: Callback<Event>) -> Html {
    html! {
        <>
            <label class={LABEL_CLASS}>{locale.t("http_form.max_body_size")}</label>
            <input type="number" min="0" value={value.to_string()} {onchange} class={INPUT_CLASS} />
        </>
    }
}

/// The attempts, the replay body limit and the failures of a retry policy. Each change emits the
/// changed form to `onchange`.
fn retry_view(locale: Locale, form: &RetryForm, onchange: Callback<RetryForm>) -> Html {
    let update = |apply: fn(&mut RetryForm, &Event)| {
        let (form, onchange) = (form.clone(), onchange.clone());
        Callback::from(move |event: Event| {
            let mut next = form.clone();
            apply(&mut next, &event);
            onchange.emit(next);
        })
    };
    html! {
        <>
            <div class="grid grid-cols-1 sm:grid-cols-2 gap-x-4">
                <div>
                    <label class={LABEL_CLASS}>{locale.t("http_form.retry_attempts")}</label>
                    <input type="number" min="1" max={MAX_RETRY_ATTEMPTS.to_string()} value={form.attempts.clone()} onchange={update(|form, event| form.attempts = text(event))} class={INPUT_CLASS} />
                </div>
                <div>
                    <label class={LABEL_CLASS}>{locale.t("http_form.replay_body_limit")}</label>
                    <input type="number" min="0" value={form.replay_body_limit.clone()} onchange={update(|form, event| form.replay_body_limit = text(event))} class={INPUT_CLASS} />
                </div>
            </div>
            <label class={LABEL_CLASS}>{locale.t("http_form.retry_on")}</label>
            <div class="flex flex-wrap gap-x-6">
                { for RetryOn::ALL.into_iter().map(|reason| toggle(
                    retry_on_input(form, &onchange, reason),
                    form.retry_on.contains(&reason),
                    locale.t(retry_on_key(reason)),
                    "mt-2",
                )) }
            </div>
        </>
    }
}

fn retry_on_key(reason: RetryOn) -> &'static str {
    match reason {
        RetryOn::Connect => "http_form.retry_on_connect",
        RetryOn::Timeout => "http_form.retry_on_timeout",
        RetryOn::Http502 => "http_form.retry_on_http_502",
        RetryOn::Http503 => "http_form.retry_on_http_503",
        RetryOn::Http504 => "http_form.retry_on_http_504",
    }
}

fn retry_on_input(
    form: &RetryForm,
    onchange: &Callback<RetryForm>,
    reason: RetryOn,
) -> Callback<Event> {
    let (form, onchange) = (form.clone(), onchange.clone());
    Callback::from(move |event: Event| {
        let mut next = form.clone();
        next.retry_on = toggled(&form.retry_on, reason, checked(&event));
        onchange.emit(next);
    })
}

/// The failures in the order of [`RetryOn::ALL`], with `reason` added when `on` is true and
/// removed otherwise.
fn toggled(current: &[RetryOn], reason: RetryOn, on: bool) -> Vec<RetryOn> {
    RetryOn::ALL
        .into_iter()
        .filter(|value| {
            if *value == reason {
                on
            } else {
                current.contains(value)
            }
        })
        .collect()
}

fn timeouts_view(
    locale: Locale,
    form: &TimeoutsForm,
    on_connect: Callback<Event>,
    on_request: Callback<Event>,
) -> Html {
    html! {
        <div class="grid grid-cols-1 sm:grid-cols-2 gap-x-4">
            <div>{ seconds_input(locale.t("proxy_form.connect_timeout"), &form.connect, 1, on_connect) }</div>
            <div>{ seconds_input(locale.t("proxy_form.request_timeout"), &form.request, 0, on_request) }</div>
        </div>
    }
}

/// A labeled number input for a timeout in whole seconds.
pub(super) fn seconds_input(
    label: &'static str,
    value: &str,
    min: u64,
    onchange: Callback<Event>,
) -> Html {
    html! {
        <>
            <label class={LABEL_CLASS}>{label}</label>
            <input type="number" min={min.to_string()} max={MAX_TIMEOUT_SECS.to_string()} value={value.to_string()} {onchange} class={INPUT_CLASS} />
        </>
    }
}

fn header_rules_view(
    locale: Locale,
    request: &str,
    response: &str,
    on_request: Callback<Event>,
    on_response: Callback<Event>,
) -> Html {
    html! {
        <div class="grid grid-cols-1 sm:grid-cols-2 gap-x-4">
            <div>
                <label class={LABEL_CLASS}>{locale.t("http_form.request_headers")}</label>
                <textarea rows="4" autocapitalize="off" spellcheck="false" placeholder={"set X-Request-Id: {request_id}\nremove X-Debug"} value={request.to_string()} onchange={on_request} class={classes!(INPUT_CLASS, "font-mono")} />
            </div>
            <div>
                <label class={LABEL_CLASS}>{locale.t("http_form.response_headers")}</label>
                <textarea rows="4" autocapitalize="off" spellcheck="false" placeholder={"set X-Frame-Options: DENY\nremove Server"} value={response.to_string()} onchange={on_response} class={classes!(INPUT_CLASS, "font-mono")} />
            </div>
        </div>
    }
}

fn compression_view(locale: Locale, form: &UseStateHandle<ProxyForm>) -> Html {
    html! {
        <>
            <div class="flex flex-wrap gap-x-6">
                { for CompressionAlgorithm::ALL.into_iter().map(|algorithm| toggle(
                    algorithm_input(form, algorithm),
                    form.compression.algorithms.contains(&algorithm),
                    algorithm.label(),
                    "mt-4",
                )) }
            </div>
            <div class="grid grid-cols-1 sm:grid-cols-3 gap-x-4">
                <div>
                    <label class={LABEL_CLASS}>{locale.t("http_form.min_size")}</label>
                    <input type="number" min="0" value={form.compression.min_size.clone()} onchange={state_input(form, text, |form, value| form.compression.min_size = value)} class={INPUT_CLASS} />
                </div>
                <div class="sm:col-span-2">
                    <label class={LABEL_CLASS}>{locale.t("http_form.media_types")}</label>
                    <input type="text" autocapitalize="off" placeholder="text/*, application/json" value={form.compression.mime_types.clone()} onchange={state_input(form, text, |form, value| form.compression.mime_types = value)} class={INPUT_CLASS} />
                </div>
            </div>
        </>
    }
}

fn cache_view(locale: Locale, form: &UseStateHandle<ProxyForm>) -> Html {
    html! {
        <>
            <div>
                { toggle(state_input(form, checked, |form, value| form.cache.enabled = value), form.cache.enabled, locale.t("http_form.enable_cache"), "mt-4") }
            </div>
            if form.cache.enabled {
                <div class="grid grid-cols-1 sm:grid-cols-3 gap-x-4">
                    <div>
                        <label class={LABEL_CLASS}>{locale.t("http_form.memory_limit")}</label>
                        <input type="number" min="1" value={form.cache.max_size.clone()} onchange={state_input(form, text, |form, value| form.cache.max_size = value)} class={INPUT_CLASS} />
                    </div>
                    <div>
                        <label class={LABEL_CLASS}>{locale.t("http_form.max_response_size")}</label>
                        <input type="number" min="1" value={form.cache.max_entry_size.clone()} onchange={state_input(form, text, |form, value| form.cache.max_entry_size = value)} class={INPUT_CLASS} />
                    </div>
                    <div>
                        <label class={LABEL_CLASS}>{locale.t("http_form.default_ttl")}</label>
                        <input type="number" min="0" value={form.cache.default_ttl.clone()} onchange={state_input(form, text, |form, value| form.cache.default_ttl = value)} class={INPUT_CLASS} />
                    </div>
                </div>
            }
        </>
    }
}

fn sticky_view(locale: Locale, form: &UseStateHandle<ProxyForm>) -> Html {
    html! {
        <>
            <div>
                { toggle(state_input(form, checked, |form, value| form.sticky.enabled = value), form.sticky.enabled, locale.t("http_form.enable_sticky_cookie"), "mt-4") }
            </div>
            if form.sticky.enabled {
                <div class="grid grid-cols-1 sm:grid-cols-2 gap-x-4">
                    <div>
                        <label class={LABEL_CLASS}>{locale.t("http_form.sticky_cookie_name")}</label>
                        <input type="text" autocapitalize="off" spellcheck="false" value={form.sticky.name.clone()} onchange={state_input(form, text, |form, value| form.sticky.name = value)} class={INPUT_CLASS} />
                    </div>
                    <div>{ seconds_input(locale.t("http_form.sticky_max_age"), &form.sticky.max_age, 0, state_input(form, text, |form, value| form.sticky.max_age = value)) }</div>
                </div>
            }
        </>
    }
}

/// Adds the algorithm to the end of the preference order, or removes it.
fn algorithm_input(
    form: &UseStateHandle<ProxyForm>,
    algorithm: CompressionAlgorithm,
) -> Callback<Event> {
    let form = form.clone();
    Callback::from(move |event: Event| {
        let mut next = (*form).clone();
        let algorithms = &mut next.compression.algorithms;
        algorithms.retain(|value| *value != algorithm);
        if checked(&event) {
            algorithms.push(algorithm);
        }
        form.set(next);
    })
}

fn rate_limit_view(
    locale: Locale,
    limit: &RateLimitForm,
    on_requests: Callback<Event>,
    on_per: Callback<Event>,
    on_burst: Callback<Event>,
) -> Html {
    html! {
        <div class="grid grid-cols-1 sm:grid-cols-3 gap-x-4">
            <div>
                <label class={LABEL_CLASS}>{locale.t("http_form.requests")}</label>
                <input type="number" min="0" value={limit.requests.clone()} onchange={on_requests} class={INPUT_CLASS} />
            </div>
            <div>
                <label class={LABEL_CLASS}>{locale.t("http_form.per")}</label>
                <select onchange={on_per} class={INPUT_CLASS}>
                    { for RatePeriod::ALL.iter().map(|value| html! {
                        <option value={value.as_str()} selected={*value == limit.per}>{period_label(locale, *value)}</option>
                    }) }
                </select>
            </div>
            <div>
                <label class={LABEL_CLASS}>{locale.t("http_form.burst")}</label>
                <input type="number" min="0" value={limit.burst.clone()} onchange={on_burst} class={INPUT_CLASS} />
            </div>
        </div>
    }
}

/// The `period.<value>` keys name every rate period.
fn period_label(locale: Locale, period: RatePeriod) -> String {
    locale.t(&format!("period.{}", period.as_str())).to_string()
}

/// Loads the client certificates that have a private key.
#[hook]
pub fn use_client_certs() -> UseStateHandle<Vec<CertInfo>> {
    use_loaded_list(get_cert_list, is_upstream_client_cert)
}

/// Loads the access lists.
#[hook]
fn use_access_lists() -> UseStateHandle<Vec<AccessListEntry>> {
    use_loaded_list(get_access_lists, |_| true)
}

/// Loads a list once with `load` and keeps the items that `keep` accepts. A failed load keeps the
/// list empty.
#[hook]
fn use_loaded_list<T, F>(load: fn() -> F, keep: fn(&T) -> bool) -> UseStateHandle<Vec<T>>
where
    T: 'static,
    F: std::future::Future<Output = Result<Vec<T>, gloo_net::Error>> + 'static,
{
    let items = use_state(Vec::<T>::new);
    use_effect_with((), {
        let items = items.clone();
        move |_| {
            wasm_bindgen_futures::spawn_local(async move {
                if let Ok(list) = load().await {
                    items.set(list.into_iter().filter(keep).collect());
                }
            });
        }
    });
    items
}

async fn get_access_lists() -> Result<Vec<AccessListEntry>, gloo_net::Error> {
    Request::get(&format!("{API_ENDPOINT}/access_lists"))
        .send()
        .await?
        .json()
        .await
}

/// The select element of the access list of a proxy or a route. A selected list that `lists` does
/// not have shows its id.
fn access_list_view(
    locale: Locale,
    onchange: Callback<Event>,
    selected: Option<ShortId>,
    lists: &[AccessListEntry],
    hint_key: &'static str,
) -> Html {
    let unknown = selected.filter(|id| lists.iter().all(|entry| entry.id != *id));
    let options = html! {
        <>
            <option value="" selected={selected.is_none()}>{locale.t("http_form.access_list_none")}</option>
            { for lists.iter().map(|entry| html! {
                <option value={entry.id.to_string()} selected={selected == Some(entry.id)}>{entry.list.name.clone()}</option>
            }) }
            if let Some(id) = unknown {
                <option value={id.to_string()} selected=true>{id.to_string()}</option>
            }
        </>
    };
    select_field(
        locale.t("http_form.access_list"),
        onchange,
        options,
        Some(locale.t(hint_key)),
    )
}

fn is_upstream_client_cert(cert: &CertInfo) -> bool {
    cert.kind == CertKind::Client && cert.has_private_key
}

/// The select element of the client certificate that a proxy sends to its upstream servers.
pub fn client_cert_view(
    locale: Locale,
    onchange: Callback<Event>,
    selected: Option<ShortId>,
    certs: &[CertInfo],
) -> Html {
    let options = html! {
        <>
            <option value="" selected={selected.is_none()}>{locale.t("proxy_form.client_cert_none")}</option>
            { for certs.iter().map(|cert| html! {
                <option value={cert.id.to_string()} selected={selected == Some(cert.id)}>{client_cert_label(cert)}</option>
            }) }
        </>
    };
    select_field(
        locale.t("proxy_form.client_cert"),
        onchange,
        options,
        Some(locale.t("proxy_form.client_cert_hint")),
    )
}

/// Reads the value of a select element of an optional id, such as the client certificate. The
/// empty value is "None", because an empty string parses as the zero ID.
pub fn parse_optional_id(value: &str) -> Option<ShortId> {
    if value.is_empty() {
        return None;
    }
    value.parse().ok()
}

pub fn client_cert_label(cert: &CertInfo) -> String {
    let names = cert
        .san
        .iter()
        .map(ToString::to_string)
        .collect::<Vec<_>>()
        .join(", ");
    format!("{names} ({})", cert.id)
}

pub(super) fn toggle(
    onchange: Callback<Event>,
    checked: bool,
    label: &'static str,
    margin: &'static str,
) -> Html {
    html! {
        <label class={classes!("relative", "inline-flex", "items-center", "cursor-pointer", margin)}>
            <input {onchange} type="checkbox" {checked} class="sr-only peer" />
            <div class={TOGGLE_CLASS}></div>
            <span class="ml-3 text-sm font-medium text-neutral-900 dark:text-neutral-200">{label}</span>
        </label>
    }
}

pub fn error_view(error: Option<&String>) -> Html {
    match error {
        Some(error) => html! { <p class={ERROR_CLASS}>{error.clone()}</p> },
        None => html! {},
    }
}

/// A labeled select element with an optional hint below it.
pub fn select_field(
    label: &str,
    onchange: Callback<Event>,
    options: Html,
    hint: Option<&str>,
) -> Html {
    html! {
        <>
            <label class={LABEL_CLASS}>{label.to_string()}</label>
            <select {onchange} class={INPUT_CLASS}>
                { options }
            </select>
            if let Some(hint) = hint {
                <p class={HINT_CLASS}>{hint.to_string()}</p>
            }
        </>
    }
}

/// Returns a change handler of a select element that stores the parsed value in the state.
pub fn select_setter<T: 'static>(
    state: &UseStateHandle<T>,
    parse: fn(&str) -> T,
) -> Callback<Event> {
    let state = state.clone();
    Callback::from(move |event: Event| {
        let target: HtmlSelectElement = event.target().unwrap_throw().dyn_into().unwrap_throw();
        state.set(parse(&target.value()));
    })
}

pub(super) fn input_element(event: &Event) -> HtmlInputElement {
    event.target().unwrap_throw().dyn_into().unwrap_throw()
}

/// Returns a change handler of an input element that stores its value in the state.
pub(super) fn text_setter(state: &UseStateHandle<String>) -> Callback<Event> {
    let state = state.clone();
    Callback::from(move |event: Event| state.set(text(&event)))
}

/// Emits `entry` to `onchanged` when it differs from the last emitted entry. Returns the errors
/// of `entry`.
#[hook]
pub(super) fn use_entry_errors<T>(
    entry: Result<T, HashMap<String, String>>,
    onchanged: Callback<Result<T, HashMap<String, String>>>,
) -> HashMap<String, String>
where
    T: Clone + PartialEq + 'static,
{
    let prev_entry = use_state::<Result<T, HashMap<String, String>>, _>(|| Err(Default::default()));
    if entry != *prev_entry {
        prev_entry.set(entry.clone());
        onchanged.emit(entry.clone());
    }
    entry.err().unwrap_or_default()
}

/// A timeout input of a TCP or UDP proxy in whole seconds, with its error and its hint.
pub(super) fn timeout_field_view(
    locale: Locale,
    label_key: &'static str,
    hint_key: &'static str,
    value: &UseStateHandle<String>,
    error: Option<&String>,
) -> Html {
    html! {
        <>
            { seconds_input(locale.t(label_key), value, 1, text_setter(value)) }
            { error_view(error) }
            <p class={HINT_CLASS}>{locale.t(hint_key)}</p>
        </>
    }
}

fn select_value(event: &Event) -> String {
    let select: HtmlSelectElement = event.target().unwrap_throw().dyn_into().unwrap_throw();
    select.value()
}

pub(super) fn text(event: &Event) -> String {
    input_element(event).value()
}

fn text_area(event: &Event) -> String {
    let area: HtmlTextAreaElement = event.target().unwrap_throw().dyn_into().unwrap_throw();
    area.value()
}

pub(super) fn checked(event: &Event) -> bool {
    input_element(event).checked()
}

/// Returns the option whose name is the value of the select element, or the default option.
fn select_option<T: Copy + Default>(event: &Event, options: &[T], name: fn(&T) -> &str) -> T {
    let value = select_value(event);
    options
        .iter()
        .find(|option| name(option) == value)
        .copied()
        .unwrap_or_default()
}

fn load_balancing(event: &Event) -> LoadBalancing {
    select_option(event, &LoadBalancing::ALL, LoadBalancing::as_str)
}

fn period(event: &Event) -> RatePeriod {
    select_option(event, &RatePeriod::ALL, |period| period.as_str())
}

fn route_kind(event: &Event) -> RouteKind {
    super::port_config::find_option(&ROUTE_KINDS, &select_value(event))
}

/// The status code of a select element whose options are only valid codes.
fn status_code(event: &Event) -> u16 {
    select_value(event).parse().unwrap_or_default()
}

fn redirect_status(event: &Event) -> RedirectStatus {
    RedirectStatus::try_from(status_code(event)).unwrap_or_default()
}

fn state_update<T, V>(state: &UseStateHandle<T>, update: fn(&mut T, V)) -> Callback<V>
where
    T: Clone + 'static,
    V: 'static,
{
    let state = state.clone();
    Callback::from(move |value: V| {
        let mut next = (*state).clone();
        update(&mut next, value);
        state.set(next);
    })
}

pub(super) fn state_input<T, V>(
    state: &UseStateHandle<T>,
    read: fn(&Event) -> V,
    update: fn(&mut T, V),
) -> Callback<Event>
where
    T: Clone + 'static,
    V: 'static,
{
    state_update(state, update).reform(move |event: Event| read(&event))
}

/// Returns a handler that updates the item at the index of a list state.
pub(super) fn item_update<T, V>(
    items: &UseStateHandle<Vec<T>>,
    index: usize,
    update: fn(&mut T, V),
) -> Callback<V>
where
    T: Clone + 'static,
    V: 'static,
{
    let items = items.clone();
    Callback::from(move |value: V| {
        let mut list = (*items).clone();
        if let Some(item) = list.get_mut(index) {
            update(item, value);
            items.set(list);
        }
    })
}

fn route_input<V: 'static>(
    routes: &UseStateHandle<Vec<RouteForm>>,
    index: usize,
    read: fn(&Event) -> V,
    update: fn(&mut RouteForm, V),
) -> Callback<Event> {
    item_update(routes, index, update).reform(move |event: Event| read(&event))
}

fn get_proxy(
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
    })
}

/// The IP filter and the authentication of the proxy. The access list of the proxy replaces both.
fn parse_proxy_access(
    locale: Locale,
    form: &ProxyForm,
    errors: &mut HashMap<String, String>,
) -> (IpFilter, AuthPolicy) {
    if form.access_list.is_some() {
        return Default::default();
    }
    let ip_filter = IpFilter {
        allow: or_error(
            translated(locale, parse_cidr_list(&form.allow)),
            "allow",
            errors,
        ),
        deny: or_error(
            translated(locale, parse_cidr_list(&form.deny)),
            "deny",
            errors,
        ),
    };
    (ip_filter, or_error(form.auth.parse(locale), "auth", errors))
}

/// The largest timeout that the forms accept, one day.
pub(super) const MAX_TIMEOUT_SECS: u64 = 86_400;

/// Shows a timeout in whole seconds. A timeout below one second shows as 1, so it does not read
/// as a disabled limit.
pub(super) fn format_seconds(timeout: Duration) -> String {
    if timeout.is_zero() {
        return "0".into();
    }
    timeout.as_secs().max(1).to_string()
}

/// Parses a timeout in whole seconds from `min` to [`MAX_TIMEOUT_SECS`].
pub(super) fn parse_seconds(
    locale: Locale,
    value: &str,
    name_key: &str,
    min: u64,
) -> Result<Duration, String> {
    value
        .trim()
        .parse::<u64>()
        .ok()
        .filter(|secs| (min..=MAX_TIMEOUT_SECS).contains(secs))
        .map(Duration::from_secs)
        .ok_or_else(|| {
            locale.tf(
                "proxy_form.seconds_range",
                &[
                    ("name", locale.t(name_key)),
                    ("min", &min.to_string()),
                    ("max", &MAX_TIMEOUT_SECS.to_string()),
                ],
            )
        })
}

fn parse_retry(
    locale: Locale,
    form: &RetryForm,
    key: &str,
    errors: &mut HashMap<String, String>,
) -> RetryPolicy {
    let attempts = form
        .attempts
        .trim()
        .parse::<u8>()
        .ok()
        .filter(|attempts| (1..=MAX_RETRY_ATTEMPTS).contains(attempts))
        .ok_or_else(|| locale.error_message(&Error::InvalidRetryAttempts));
    RetryPolicy {
        attempts: or_error(attempts, key, errors),
        retry_on: form.retry_on.clone(),
        replay_body_limit: or_error(
            parse_size(
                locale,
                &form.replay_body_limit,
                "http_form.replay_body_limit_name",
            ),
            key,
            errors,
        ),
    }
}

/// Reads the sticky cookie. A `Max-Age` of 0 seconds is a cookie without `Max-Age`.
fn parse_sticky(
    locale: Locale,
    form: &StickyForm,
    key: &str,
    errors: &mut HashMap<String, String>,
) -> StickyCookie {
    let max_age = or_error(
        parse_seconds(locale, &form.max_age, "http_form.sticky_max_age_name", 0),
        key,
        errors,
    );
    let sticky = StickyCookie {
        enabled: form.enabled,
        name: form.name.trim().to_string(),
        max_age: (!max_age.is_zero()).then_some(max_age),
    };
    if let Err(err) = sticky.validate() {
        errors
            .entry(key.to_string())
            .or_insert_with(|| locale.error_message(&err));
    }
    sticky
}

fn parse_timeouts(
    locale: Locale,
    form: &TimeoutsForm,
    key: &str,
    errors: &mut HashMap<String, String>,
) -> UpstreamTimeouts {
    UpstreamTimeouts {
        connect: or_error(
            parse_seconds(locale, &form.connect, "proxy_form.connect_timeout_name", 1),
            key,
            errors,
        ),
        request: or_error(
            parse_seconds(locale, &form.request, "proxy_form.request_timeout_name", 0),
            key,
            errors,
        ),
    }
}

fn parse_cache(
    locale: Locale,
    form: &CacheForm,
    key: &str,
    errors: &mut HashMap<String, String>,
) -> CacheConfig {
    let cache = CacheConfig {
        enabled: form.enabled,
        max_size: or_error(
            parse_size(locale, &form.max_size, "http_form.memory_limit_name"),
            key,
            errors,
        ),
        max_entry_size: or_error(
            parse_size(
                locale,
                &form.max_entry_size,
                "http_form.max_response_size_name",
            ),
            key,
            errors,
        ),
        default_ttl: Duration::from_secs(or_error(
            parse_size(locale, &form.default_ttl, "http_form.default_ttl_name"),
            key,
            errors,
        )),
    };
    if let Err(err) = cache.validate() {
        errors.insert(key.to_string(), locale.error_message(&err));
    }
    cache
}

fn parse_compression(
    locale: Locale,
    form: &CompressionForm,
    key: &str,
    errors: &mut HashMap<String, String>,
) -> Compression {
    Compression {
        algorithms: form.algorithms.clone(),
        min_size: or_error(
            parse_size(locale, &form.min_size, "http_form.min_size_name"),
            key,
            errors,
        ),
        mime_types: or_error(
            translated(locale, parse_mime_list(&form.mime_types)),
            key,
            errors,
        ),
    }
}

/// Replaces an API error with its message in the selected language.
fn translated<T>(locale: Locale, result: Result<T, Error>) -> Result<T, String> {
    result.map_err(|err| locale.error_message(&err))
}

fn parse_size(locale: Locale, value: &str, name_key: &str) -> Result<u64, String> {
    let value = value.trim();
    if value.is_empty() {
        return Ok(0);
    }
    value
        .parse()
        .map_err(|_| locale.tf("http_form.whole_number", &[("name", locale.t(name_key))]))
}

/// Returns the parsed value, or records the error under `key` and returns the default value.
pub(super) fn or_error<T: Default>(
    result: Result<T, String>,
    key: &str,
    errors: &mut HashMap<String, String>,
) -> T {
    result.unwrap_or_else(|err| {
        errors.insert(key.to_string(), err);
        T::default()
    })
}

fn parse_header_rules_form(
    locale: Locale,
    request: &str,
    response: &str,
    key: &str,
    errors: &mut HashMap<String, String>,
) -> HeaderRules {
    HeaderRules {
        request: or_error(
            parse_lines(
                locale,
                request,
                "http_form.request_headers_error",
                HeaderRule::parse_line,
            ),
            key,
            errors,
        ),
        response: or_error(
            parse_lines(
                locale,
                response,
                "http_form.response_headers_error",
                HeaderRule::parse_line,
            ),
            key,
            errors,
        ),
    }
}

/// Parses one item per line and skips empty lines and lines that start with `#`. The translated
/// error message names the line.
fn parse_lines<T>(
    locale: Locale,
    text: &str,
    error_key: &str,
    parse: fn(&str) -> Result<T, Error>,
) -> Result<Vec<T>, String> {
    text.lines()
        .enumerate()
        .filter(|(_, line)| {
            let line = line.trim();
            !line.is_empty() && !line.starts_with('#')
        })
        .map(|(index, line)| {
            parse(line).map_err(|err| {
                locale.tf(
                    error_key,
                    &[
                        ("line", &(index + 1).to_string()),
                        ("error", &locale.error_message(&err)),
                    ],
                )
            })
        })
        .collect()
}

fn parse_rate_limit(
    locale: Locale,
    form: &RateLimitForm,
    key: &str,
    errors: &mut HashMap<String, String>,
) -> RateLimit {
    RateLimit {
        requests: or_error(
            parse_count(locale, &form.requests, "http_form.requests"),
            key,
            errors,
        ),
        per: form.per,
        burst: or_error(
            parse_count(locale, &form.burst, "http_form.burst"),
            key,
            errors,
        ),
    }
}

fn parse_count(locale: Locale, value: &str, name_key: &str) -> Result<u32, String> {
    let value = value.trim();
    if value.is_empty() {
        return Ok(0);
    }
    value.parse().map_err(|_| {
        locale.tf(
            "http_form.count_range",
            &[("name", locale.t(name_key)), ("max", &u32::MAX.to_string())],
        )
    })
}

fn parse_vhosts(
    locale: Locale,
    vhosts: &str,
    errors: &mut HashMap<String, String>,
) -> Vec<VirtualHost> {
    parse_comma_list(locale, vhosts, "vhosts", errors, VirtualHost::from_str)
}

/// Parses a comma-separated list and skips empty items. Records the translated error of an invalid item under `key`.
pub(super) fn parse_comma_list<T>(
    locale: Locale,
    text: &str,
    key: &str,
    errors: &mut HashMap<String, String>,
    parse: impl Fn(&str) -> Result<T, Error>,
) -> Vec<T> {
    let mut items = Vec::new();
    for item in text.split(',').map(str::trim).filter(|s| !s.is_empty()) {
        match parse(item) {
            Ok(item) => items.push(item),
            Err(err) => {
                errors.insert(key.to_string(), locale.error_message(&err));
            }
        }
    }
    items
}

fn parse_routes(
    locale: Locale,
    routes: &[RouteForm],
    errors: &mut HashMap<String, String>,
) -> Vec<Route> {
    routes
        .iter()
        .enumerate()
        .filter_map(|(i, route)| parse_route(locale, route, &format!("routes_{i}"), errors))
        .collect()
}

fn parse_route(
    locale: Locale,
    route: &RouteForm,
    key: &str,
    errors: &mut HashMap<String, String>,
) -> Option<Route> {
    if !route.path.starts_with('/') {
        errors.insert(
            key.into(),
            locale.t("http_form.path_must_start_with_slash").into(),
        );
        return None;
    }
    // The access list of the route replaces its IP filter and its authentication.
    let own_access = route.access_list.is_none();
    let ip_filter = (own_access && route.override_ip_filter).then(|| IpFilter {
        allow: or_error(
            translated(locale, parse_cidr_list(&route.allow)),
            key,
            errors,
        ),
        deny: or_error(
            translated(locale, parse_cidr_list(&route.deny)),
            key,
            errors,
        ),
    });
    let rate_limit = route
        .override_rate_limit
        .then(|| parse_rate_limit(locale, &route.rate_limit, key, errors));
    let auth = (own_access && route.override_auth)
        .then(|| or_error(route.auth.parse(locale), key, errors));
    let headers = route.override_headers.then(|| {
        parse_header_rules_form(
            locale,
            &route.request_headers,
            &route.response_headers,
            key,
            errors,
        )
    });
    let target = match route.response.parse(locale) {
        Ok(Some(response)) => Route {
            response: Some(response),
            ..Default::default()
        },
        Ok(None) => parse_upstream_route(locale, route, key, errors),
        Err(err) => {
            errors.insert(key.into(), err);
            Route::default()
        }
    };
    Some(Route {
        path: route.path.clone(),
        ip_filter,
        rate_limit,
        auth,
        access_list: route.access_list,
        headers,
        ..target
    })
}

/// Reads the servers and the upstream settings of a route that proxies to its servers.
fn parse_upstream_route(
    locale: Locale,
    route: &RouteForm,
    key: &str,
    errors: &mut HashMap<String, String>,
) -> Route {
    let servers = parse_server_lines(locale, &route.servers, key, errors);
    if servers.is_empty() {
        errors
            .entry(key.to_string())
            .or_insert_with(|| locale.t("http_form.servers_required").into());
    }
    let timeouts = route
        .override_timeouts
        .then(|| parse_timeouts(locale, &route.timeouts, key, errors));
    let retry = route
        .override_retry
        .then(|| parse_retry(locale, &route.retry, key, errors));
    let max_body_size = route.override_body_limit.then(|| {
        let size = parse_size(locale, &route.max_body_size, "http_form.max_body_size_name");
        or_error(size, key, errors)
    });
    Route {
        servers,
        timeouts,
        retry,
        max_body_size,
        rewrite: parse_rewrite(locale, route, key, errors),
        mirror: parse_mirror(locale, route, key, errors),
        ..Default::default()
    }
}

/// Reads one server from each line. An invalid line records its error under the key.
fn parse_server_lines(
    locale: Locale,
    lines: &[String],
    key: &str,
    errors: &mut HashMap<String, String>,
) -> Vec<Server> {
    lines
        .iter()
        .filter_map(|line| match parse_server_line(locale, line) {
            Ok(server) => Some(server),
            Err(err) => {
                errors.insert(key.into(), err);
                None
            }
        })
        .collect()
}

/// Reads the mirror of a route. `None` when the mirror is off.
fn parse_mirror(
    locale: Locale,
    route: &RouteForm,
    key: &str,
    errors: &mut HashMap<String, String>,
) -> Option<Mirror> {
    if !route.mirror {
        return None;
    }
    let servers = parse_server_lines(locale, &route.mirror_servers, key, errors);
    let percent = parse_percent(
        locale,
        &route.mirror_percent,
        "http_form.mirror_percent_name",
    );
    let percent = or_error(percent, key, errors);
    let size = parse_size(
        locale,
        &route.mirror_max_body_size,
        "http_form.max_body_size_name",
    );
    let max_body_size = or_error(size, key, errors);
    Some(Mirror {
        servers,
        percent,
        max_body_size,
    })
}

/// Reads the path rewrite of a route. An empty regex is no regex.
fn parse_rewrite(
    locale: Locale,
    route: &RouteForm,
    key: &str,
    errors: &mut HashMap<String, String>,
) -> PathRewrite {
    let pattern = route.path_regex.trim();
    let regex = if pattern.is_empty() {
        None
    } else {
        let regex = pattern.parse::<PathRegex>().map(Some);
        or_error(translated(locale, regex), key, errors)
    };
    let rewrite = PathRewrite {
        strip_prefix: route.strip_prefix,
        regex,
        replacement: route.replacement.clone(),
        add_prefix: route.add_prefix.trim().to_string(),
    };
    if let Err(err) = rewrite.validate() {
        errors
            .entry(key.to_string())
            .or_insert_with(|| locale.error_message(&err));
    }
    rewrite
}

#[cfg(test)]
mod tests {
    use super::*;

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
        };
        let form = UpstreamForm::new(LoadBalancing::First, &health_check);
        let mut errors = HashMap::new();
        assert_eq!(
            form.parse(Locale::En, true, &mut errors),
            (LoadBalancing::First, health_check)
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
}
