use super::auth_config::{AuthConfig, AuthForm};
use crate::i18n::use_locale;
use crate::pages::cert_list::get_cert_list;
use r3v3rs3_api::cache::CacheConfig;
use r3v3rs3_api::cert::{CertInfo, CertKind};
use r3v3rs3_api::cidr::{format_cidr_list, parse_cidr_list};
use r3v3rs3_api::client_ip::ClientIpConfig;
use r3v3rs3_api::compression::{
    format_mime_list, parse_mime_list, Compression, CompressionAlgorithm,
};
use r3v3rs3_api::error::Error;
use r3v3rs3_api::header_rules::{format_header_rules, HeaderRule, HeaderRules};
use r3v3rs3_api::i18n::Locale;
use r3v3rs3_api::id::ShortId;
use r3v3rs3_api::policy::{IpFilter, RateLimit, RatePeriod};
use r3v3rs3_api::proxy::{HttpProxy, Route, Server, ServerUrl};
use r3v3rs3_api::upstream::{
    CircuitBreaker, HealthCheck, LoadBalancing, UpstreamTimeouts, DEFAULT_WEIGHT,
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
    request_headers: String,
    response_headers: String,
    compression: CompressionForm,
    cache: CacheForm,
    h2c: bool,
    client_cert: Option<ShortId>,
    timeouts: TimeoutsForm,
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
            request_headers: format_header_rules(&proxy.headers.request),
            response_headers: format_header_rules(&proxy.headers.response),
            compression: CompressionForm::new(&proxy.compression),
            cache: CacheForm::new(&proxy.cache),
            h2c: proxy.h2c,
            client_cert: proxy.client_cert,
            timeouts: TimeoutsForm::new(&proxy.timeouts),
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
    servers: Vec<String>,
    override_ip_filter: bool,
    allow: String,
    deny: String,
    override_rate_limit: bool,
    rate_limit: RateLimitForm,
    override_auth: bool,
    auth: AuthForm,
    override_headers: bool,
    request_headers: String,
    response_headers: String,
    override_timeouts: bool,
    timeouts: TimeoutsForm,
}

impl RouteForm {
    fn new(route: &Route) -> Self {
        let ip_filter = route.ip_filter.clone().unwrap_or_default();
        Self {
            path: route.path.clone(),
            servers: route.servers.iter().map(format_server_line).collect(),
            override_ip_filter: route.ip_filter.is_some(),
            allow: format_cidr_list(&ip_filter.allow),
            deny: format_cidr_list(&ip_filter.deny),
            override_rate_limit: route.rate_limit.is_some(),
            rate_limit: RateLimitForm::new(&route.rate_limit.unwrap_or_default()),
            override_auth: route.auth.is_some(),
            auth: AuthForm::new(&route.auth.clone().unwrap_or_default()),
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
        }
    }

    fn empty() -> Self {
        Self {
            path: "/".into(),
            servers: Vec::new(),
            override_ip_filter: false,
            allow: String::new(),
            deny: String::new(),
            override_rate_limit: false,
            rate_limit: RateLimitForm::new(&RateLimit::default()),
            override_auth: false,
            auth: AuthForm::new(&Default::default()),
            override_headers: false,
            request_headers: String::new(),
            response_headers: String::new(),
            override_timeouts: false,
            timeouts: TimeoutsForm::new(&UpstreamTimeouts::default()),
        }
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

            <label class={LABEL_CLASS}>{locale.t("http_form.allow")}</label>
            <input type="text" autocapitalize="off" value={form.allow.clone()} onchange={state_input(&form, text, |form, value| form.allow = value)} class={INPUT_CLASS} placeholder="192.168.0.0/16" />
            { error_view(errors.get("allow")) }
            <p class={HINT_CLASS}>{locale.t("http_form.allow_hint")}</p>

            <label class={LABEL_CLASS}>{locale.t("http_form.deny")}</label>
            <input type="text" autocapitalize="off" value={form.deny.clone()} onchange={state_input(&form, text, |form, value| form.deny = value)} class={INPUT_CLASS} placeholder="203.0.113.0/24" />
            { error_view(errors.get("deny")) }
            <p class={HINT_CLASS}>{locale.t("http_form.deny_hint")}</p>

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

            <label class={SECTION_CLASS}>{locale.t("http_form.authentication")}</label>
            <AuthConfig form={form.auth.clone()} onchange={state_update(&form, |form, value| form.auth = value)} />
            { error_view(errors.get("auth")) }

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
                state_input(&form, select_value, |form, value| form.client_cert = parse_client_cert(&value)),
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

            { upstream_form_view(locale, &upstream, &errors, true) }

            { circuit_breaker_view(locale, &circuit_breaker, &errors, "proxy_form.circuit_breaker_hint") }

            <label class={SECTION_CLASS}>{locale.t("http_form.routes")}</label>
            <p class={HINT_CLASS}>{locale.t("http_form.routes_hint")}</p>

            { routes.iter().enumerate().map(|(i, route)| {
                route_view(locale, &routes, i, route, errors.get(&format!("routes_{i}")))
            }).collect::<Html>() }
        </>
    }
}

fn route_view(
    locale: Locale,
    routes: &UseStateHandle<Vec<RouteForm>>,
    index: usize,
    route: &RouteForm,
    error: Option<&String>,
) -> Html {
    html! {
        <div class="mt-2 bg-white dark:text-neutral-200 dark:bg-neutral-800 shadow-sm p-5 border border-neutral-300 dark:border-neutral-700 rounded-md">
            <label class="block mb-2 text-sm font-medium text-neutral-900 dark:text-neutral-200">{locale.t("http_form.path")}</label>
            <input type="text" autocapitalize="off" placeholder="/" onchange={route_input(routes, index, text, |route, value| route.path = value)} value={route.path.clone()} class={INPUT_CLASS} />

            <label class={LABEL_CLASS}>{locale.t("http_form.target")}</label>
            <textarea rows="2" autocapitalize="off" spellcheck="false" placeholder={"https://a.example.com/backend\nhttps://b.example.com/backend"} value={route.servers.join("\n")} onchange={route_input(routes, index, text_area, |route, value| route.servers = server_lines(&value))} class={INPUT_CLASS} />
            <p class={HINT_CLASS}>{locale.t("http_form.target_hint")}</p>

            { route_ip_filter_view(locale, routes, index, route) }
            { route_rate_limit_view(locale, routes, index, route) }
            { route_auth_view(locale, routes, index, route) }
            { route_headers_view(locale, routes, index, route) }
            { route_timeouts_view(locale, routes, index, route) }
            { error_view(error) }

            { list_buttons(routes, index, RouteForm::empty) }
        </div>
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

/// Reads a server line: the URL, and an optional weight after a space.
fn parse_server_line(locale: Locale, line: &str) -> Result<Server, String> {
    let mut parts = line.split_whitespace();
    let url = ServerUrl::from_str(parts.next().unwrap_or_default())
        .map_err(|err| locale.error_message(&err))?;
    let rest = parts.collect::<Vec<_>>();
    let weight = match rest.as_slice() {
        [] => DEFAULT_WEIGHT,
        [value] => parse_weight(locale, value)?,
        _ => parse_weight(locale, &rest.join(" "))?,
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
    let certs = use_state(Vec::<CertInfo>::new);
    use_effect_with((), {
        let certs = certs.clone();
        move |_| {
            wasm_bindgen_futures::spawn_local(async move {
                if let Ok(list) = get_cert_list().await {
                    certs.set(list.into_iter().filter(is_upstream_client_cert).collect());
                }
            });
        }
    });
    certs
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

/// Reads the value of the client certificate select element. The empty value is "None", because
/// an empty string parses as the zero ID.
pub fn parse_client_cert(value: &str) -> Option<ShortId> {
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

fn text(event: &Event) -> String {
    input_element(event).value()
}

fn text_area(event: &Event) -> String {
    let area: HtmlTextAreaElement = event.target().unwrap_throw().dyn_into().unwrap_throw();
    area.value()
}

fn checked(event: &Event) -> bool {
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

fn state_input<T, V>(
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
    let ip_filter = IpFilter {
        allow: or_error(
            translated(locale, parse_cidr_list(&form.allow)),
            "allow",
            &mut errors,
        ),
        deny: or_error(
            translated(locale, parse_cidr_list(&form.deny)),
            "deny",
            &mut errors,
        ),
    };
    let rate_limit = parse_rate_limit(locale, &form.rate_limit, "rate_limit", &mut errors);
    let auth = or_error(form.auth.parse(locale), "auth", &mut errors);
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
        headers,
        compression,
        cache,
        h2c: form.h2c,
        client_cert: form.client_cert,
        timeouts,
        load_balancing,
        health_check,
        circuit_breaker,
    })
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
            parse_rules(locale, request, "http_form.request_headers_error"),
            key,
            errors,
        ),
        response: or_error(
            parse_rules(locale, response, "http_form.response_headers_error"),
            key,
            errors,
        ),
    }
}

/// Parses one rule per line like `parse_header_rules`, with a translated error message.
fn parse_rules(locale: Locale, text: &str, error_key: &str) -> Result<Vec<HeaderRule>, String> {
    text.lines()
        .enumerate()
        .filter(|(_, line)| {
            let line = line.trim();
            !line.is_empty() && !line.starts_with('#')
        })
        .map(|(index, line)| {
            HeaderRule::parse_line(line).map_err(|err| {
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
    let servers = route
        .servers
        .iter()
        .filter_map(|line| match parse_server_line(locale, line) {
            Ok(server) => Some(server),
            Err(err) => {
                errors.insert(key.into(), err);
                None
            }
        })
        .collect::<Vec<_>>();
    let ip_filter = route.override_ip_filter.then(|| IpFilter {
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
    let auth = route
        .override_auth
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
    let timeouts = route
        .override_timeouts
        .then(|| parse_timeouts(locale, &route.timeouts, key, errors));
    (!servers.is_empty()).then(|| Route {
        path: route.path.clone(),
        servers,
        ip_filter,
        rate_limit,
        auth,
        headers,
        timeouts,
    })
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
