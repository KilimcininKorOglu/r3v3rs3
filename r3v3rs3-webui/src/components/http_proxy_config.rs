use super::auth_config::{AuthConfig, AuthForm};
use crate::i18n::use_locale;
use r3v3rs3_api::cache::CacheConfig;
use r3v3rs3_api::cidr::{format_cidr_list, parse_cidr_list};
use r3v3rs3_api::client_ip::ClientIpConfig;
use r3v3rs3_api::compression::{
    format_mime_list, parse_mime_list, Compression, CompressionAlgorithm,
};
use r3v3rs3_api::error::Error;
use r3v3rs3_api::header_rules::{format_header_rules, HeaderRule, HeaderRules};
use r3v3rs3_api::i18n::Locale;
use r3v3rs3_api::policy::{IpFilter, RateLimit, RatePeriod};
use r3v3rs3_api::proxy::{HttpProxy, Route, Server, ServerUrl};
use r3v3rs3_api::vhost::VirtualHost;
use std::collections::HashMap;
use std::str::FromStr;
use std::time::Duration;
use wasm_bindgen::{JsCast, UnwrapThrowExt};
use web_sys::{HtmlInputElement, HtmlSelectElement, HtmlTextAreaElement};
use yew::prelude::*;

pub(super) const LABEL_CLASS: &str =
    "block mt-4 mb-2 text-sm font-medium text-neutral-900 dark:text-neutral-200";
const SECTION_CLASS: &str = "block mt-6 text-sm font-medium text-neutral-900 dark:text-neutral-200";
pub(super) const INPUT_CLASS: &str = "bg-neutral-50 dark:text-neutral-200 dark:bg-neutral-800 dark:border-neutral-600 border border-neutral-300 text-neutral-900 text-sm rounded-lg focus:ring-blue-500 focus:border-blue-500 block w-full p-2.5";
pub(super) const HINT_CLASS: &str = "mt-2 text-sm text-neutral-500 dark:text-neutral-400";
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
        }
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
}

impl RouteForm {
    fn new(route: &Route) -> Self {
        let ip_filter = route.ip_filter.clone().unwrap_or_default();
        Self {
            path: route.path.clone(),
            servers: route
                .servers
                .iter()
                .map(|server| server.url.to_string())
                .collect(),
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

    let prev_entry =
        use_state::<Result<HttpProxy, HashMap<String, String>>, _>(|| Err(Default::default()));
    let entry = get_proxy(locale, &form, &routes);
    if entry != *prev_entry {
        prev_entry.set(entry.clone());
        props.onchanged.emit(entry.clone());
    }
    let errors = entry.err().unwrap_or_default();

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

            <label class={SECTION_CLASS}>{locale.t("http_form.routes")}</label>

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
    let add_onclick = {
        let routes = routes.clone();
        Callback::from(move |_| {
            let mut list = (*routes).clone();
            list.insert(index + 1, RouteForm::empty());
            routes.set(list);
        })
    };

    let remove_onclick = {
        let routes = routes.clone();
        Callback::from(move |_| {
            if routes.len() > 1 {
                let mut list = (*routes).clone();
                list.remove(index);
                routes.set(list);
            }
        })
    };

    html! {
        <div class="mt-2 bg-white dark:text-neutral-200 dark:bg-neutral-800 shadow-sm p-5 border border-neutral-300 dark:border-neutral-700 rounded-md">
            <label class="block mb-2 text-sm font-medium text-neutral-900 dark:text-neutral-200">{locale.t("http_form.path")}</label>
            <input type="text" autocapitalize="off" placeholder="/" onchange={route_input(routes, index, text, |route, value| route.path = value)} value={route.path.clone()} class={INPUT_CLASS} />

            <label class={LABEL_CLASS}>{locale.t("http_form.target")}</label>
            <input type="url" placeholder="https://example.com/backend" value={route.servers.join("\n")} onchange={route_input(routes, index, text, |route, value| route.servers = value.split('\n').map(str::to_string).collect())} class={INPUT_CLASS} />

            { route_ip_filter_view(locale, routes, index, route) }
            { route_rate_limit_view(locale, routes, index, route) }
            { route_auth_view(locale, routes, index, route) }
            { route_headers_view(locale, routes, index, route) }
            { error_view(error) }

            <div class="flex justify-end rounded-md mt-4 sm:ml-auto" role="group">
                <button type="button" onclick={add_onclick} class={classes!(BUTTON_CLASS, "rounded-l-lg")}>
                    <img src="/assets/icons/add.svg" class="w-4 h-4" />
                </button>
                <button type="button" onclick={remove_onclick} disabled={routes.len() <= 1} class={classes!(BUTTON_CLASS, "border-l-0", "rounded-r-lg")}>
                    <img src="/assets/icons/remove.svg" class="w-4 h-4" />
                </button>
            </div>
        </div>
    }
}

fn route_ip_filter_view(
    locale: Locale,
    routes: &UseStateHandle<Vec<RouteForm>>,
    index: usize,
    route: &RouteForm,
) -> Html {
    html! {
        <>
            <div>
                { toggle(route_input(routes, index, checked, |route, value| route.override_ip_filter = value), route.override_ip_filter, locale.t("http_form.override_ip_filter"), "mt-6") }
            </div>
            if route.override_ip_filter {
                <label class={LABEL_CLASS}>{locale.t("http_form.allow")}</label>
                <input type="text" autocapitalize="off" placeholder="192.168.0.0/16" value={route.allow.clone()} onchange={route_input(routes, index, text, |route, value| route.allow = value)} class={INPUT_CLASS} />

                <label class={LABEL_CLASS}>{locale.t("http_form.deny")}</label>
                <input type="text" autocapitalize="off" placeholder="203.0.113.0/24" value={route.deny.clone()} onchange={route_input(routes, index, text, |route, value| route.deny = value)} class={INPUT_CLASS} />
                <p class={HINT_CLASS}>{locale.t("http_form.route_ip_filter_hint")}</p>
            }
        </>
    }
}

fn route_rate_limit_view(
    locale: Locale,
    routes: &UseStateHandle<Vec<RouteForm>>,
    index: usize,
    route: &RouteForm,
) -> Html {
    html! {
        <>
            <div>
                { toggle(route_input(routes, index, checked, |route, value| route.override_rate_limit = value), route.override_rate_limit, locale.t("http_form.override_rate_limit"), "mt-6") }
            </div>
            if route.override_rate_limit {
                { rate_limit_view(
                    locale,
                    &route.rate_limit,
                    route_input(routes, index, text, |route, value| route.rate_limit.requests = value),
                    route_input(routes, index, period, |route, value| route.rate_limit.per = value),
                    route_input(routes, index, text, |route, value| route.rate_limit.burst = value),
                ) }
                <p class={HINT_CLASS}>{locale.t("http_form.route_rate_limit_hint")}</p>
            }
        </>
    }
}

fn route_auth_view(
    locale: Locale,
    routes: &UseStateHandle<Vec<RouteForm>>,
    index: usize,
    route: &RouteForm,
) -> Html {
    html! {
        <>
            <div>
                { toggle(route_input(routes, index, checked, |route, value| route.override_auth = value), route.override_auth, locale.t("http_form.override_auth"), "mt-6") }
            </div>
            if route.override_auth {
                <AuthConfig form={route.auth.clone()} onchange={route_update(routes, index, |route, value| route.auth = value)} />
                <p class={HINT_CLASS}>{locale.t("http_form.route_auth_hint")}</p>
            }
        </>
    }
}

fn route_headers_view(
    locale: Locale,
    routes: &UseStateHandle<Vec<RouteForm>>,
    index: usize,
    route: &RouteForm,
) -> Html {
    html! {
        <>
            <div>
                { toggle(route_input(routes, index, checked, |route, value| route.override_headers = value), route.override_headers, locale.t("http_form.override_headers"), "mt-6") }
            </div>
            if route.override_headers {
                { header_rules_view(
                    locale,
                    &route.request_headers,
                    &route.response_headers,
                    route_input(routes, index, text_area, |route, value| route.request_headers = value),
                    route_input(routes, index, text_area, |route, value| route.response_headers = value),
                ) }
                <p class={HINT_CLASS}>{locale.t("http_form.route_headers_hint")}</p>
            }
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

fn toggle(
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

fn error_view(error: Option<&String>) -> Html {
    match error {
        Some(error) => html! { <p class={ERROR_CLASS}>{error.clone()}</p> },
        None => html! {},
    }
}

fn input_element(event: &Event) -> HtmlInputElement {
    event.target().unwrap_throw().dyn_into().unwrap_throw()
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

fn period(event: &Event) -> RatePeriod {
    let select: HtmlSelectElement = event.target().unwrap_throw().dyn_into().unwrap_throw();
    let value = select.value();
    RatePeriod::ALL
        .into_iter()
        .find(|period| period.as_str() == value)
        .unwrap_or_default()
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

fn route_update<V: 'static>(
    routes: &UseStateHandle<Vec<RouteForm>>,
    index: usize,
    update: fn(&mut RouteForm, V),
) -> Callback<V> {
    let routes = routes.clone();
    Callback::from(move |value: V| {
        let mut list = (*routes).clone();
        if let Some(route) = list.get_mut(index) {
            update(route, value);
            routes.set(list);
        }
    })
}

fn route_input<V: 'static>(
    routes: &UseStateHandle<Vec<RouteForm>>,
    index: usize,
    read: fn(&Event) -> V,
    update: fn(&mut RouteForm, V),
) -> Callback<Event> {
    route_update(routes, index, update).reform(move |event: Event| read(&event))
}

fn get_proxy(
    locale: Locale,
    form: &ProxyForm,
    routes: &[RouteForm],
) -> Result<HttpProxy, HashMap<String, String>> {
    let mut errors = HashMap::new();
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
    })
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
fn or_error<T: Default>(
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
    let mut hosts = Vec::new();
    for host in vhosts
        .split(',')
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
    {
        match VirtualHost::from_str(&host) {
            Ok(host) => hosts.push(host),
            Err(err) => {
                errors.insert("vhosts".into(), locale.error_message(&err));
            }
        }
    }
    hosts
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
        .filter_map(|url| match ServerUrl::from_str(url) {
            Ok(url) => Some(Server { url }),
            Err(err) => {
                errors.insert(key.into(), locale.error_message(&err));
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
    (!servers.is_empty()).then(|| Route {
        path: route.path.clone(),
        servers,
        ip_filter,
        rate_limit,
        auth,
        headers,
    })
}
