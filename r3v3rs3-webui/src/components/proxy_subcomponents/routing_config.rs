use super::common::*;
use super::security_config::*;
use super::upstream_config::*;
use crate::components::auth_config::{AuthConfig, AuthForm};
use r3v3rs3_api::access_list::AccessListEntry;
use r3v3rs3_api::cidr::{format_cidr_list, parse_cidr_list};
use r3v3rs3_api::fixed_response::{FIXED_STATUSES, FixedRedirect, FixedResponse, FixedStatus};
use r3v3rs3_api::header_rules::{format_header_rules, parse_header_rules};
use r3v3rs3_api::i18n::Locale;
use r3v3rs3_api::id::ShortId;
use r3v3rs3_api::mirror::{DEFAULT_MIRROR_BODY_SIZE, Mirror};
use r3v3rs3_api::policy::{ConcurrencyLimit, IpFilter, RateLimit};
use r3v3rs3_api::proxy::{Route, Server, ServerUrl};
use r3v3rs3_api::redirect::RedirectStatus;
use r3v3rs3_api::rewrite::{PathRegex, PathRewrite};
use r3v3rs3_api::security_headers::{SecurityProfile, apply_profile_to_rules};
use r3v3rs3_api::upstream::{DEFAULT_WEIGHT, RetryPolicy, UpstreamTimeouts};
use r3v3rs3_api::vhost::VirtualHost;
use std::collections::HashMap;
use std::str::FromStr;
use yew::prelude::*;

#[derive(Clone, PartialEq)]
pub struct RouteForm {
    pub path: String,
    pub response: ResponseForm,
    pub servers: Vec<String>,
    pub override_ip_filter: bool,
    pub allow: String,
    pub deny: String,
    pub allow_countries: String,
    pub deny_countries: String,
    pub override_rate_limit: bool,
    pub rate_limit: RateLimitForm,
    pub override_concurrency_limit: bool,
    pub concurrency_limit: ConcurrencyLimitForm,
    pub override_auth: bool,
    pub auth: AuthForm,
    pub access_list: Option<ShortId>,
    pub override_headers: bool,
    pub request_headers: String,
    pub response_headers: String,
    pub override_timeouts: bool,
    pub timeouts: TimeoutsForm,
    pub override_retry: bool,
    pub retry: RetryForm,
    pub override_body_limit: bool,
    pub max_body_size: String,
    pub strip_prefix: bool,
    pub path_regex: String,
    pub replacement: String,
    pub add_prefix: String,
    pub mirror: bool,
    pub mirror_servers: Vec<String>,
    pub mirror_percent: String,
    pub mirror_max_body_size: String,
    pub require_client_cert: bool,
    pub override_bot_blocker: bool,
    pub bot_blocker: bool,
    pub override_grpc_web: bool,
    pub grpc_web: bool,
}

impl RouteForm {
    pub fn new(route: &Route) -> Self {
        let ip_filter = route.ip_filter.clone().unwrap_or_default();
        let mirror = route.mirror.clone().unwrap_or_default();
        Self {
            path: route.path.clone(),
            response: ResponseForm::new(route.response.as_ref()),
            servers: route.servers.iter().map(format_server_line).collect(),
            override_ip_filter: route.ip_filter.is_some(),
            allow: format_cidr_list(&ip_filter.allow),
            deny: format_cidr_list(&ip_filter.deny),
            allow_countries: ip_filter.allow_countries.join(", "),
            deny_countries: ip_filter.deny_countries.join(", "),
            override_rate_limit: route.rate_limit.is_some(),
            rate_limit: RateLimitForm::new(&route.rate_limit.unwrap_or_default()),
            override_concurrency_limit: route.concurrency_limit.is_some(),
            concurrency_limit: ConcurrencyLimitForm::new(
                &route.concurrency_limit.clone().unwrap_or_default(),
            ),
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
            require_client_cert: route.require_client_cert,
            override_bot_blocker: route.bot_blocker.is_some(),
            bot_blocker: route.bot_blocker.unwrap_or_default(),
            override_grpc_web: route.grpc_web.is_some(),
            grpc_web: route.grpc_web.unwrap_or_default(),
        }
    }

    pub fn empty() -> Self {
        Self {
            path: "/".into(),
            response: ResponseForm::new(None),
            servers: Vec::new(),
            override_ip_filter: false,
            allow: String::new(),
            deny: String::new(),
            allow_countries: String::new(),
            deny_countries: String::new(),
            override_rate_limit: false,
            rate_limit: RateLimitForm::new(&RateLimit::default()),
            override_concurrency_limit: false,
            concurrency_limit: ConcurrencyLimitForm::new(&ConcurrencyLimit::default()),
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
            require_client_cert: false,
            override_bot_blocker: false,
            bot_blocker: false,
            override_grpc_web: false,
            grpc_web: false,
        }
    }
}

/// What answers the requests of a route.
#[derive(Clone, Copy, Default, PartialEq)]
pub enum RouteKind {
    #[default]
    Proxy,
    Redirect,
    Status,
}

pub const ROUTE_KINDS: [(RouteKind, &str, &str); 3] = [
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
pub struct ResponseForm {
    pub kind: RouteKind,
    pub redirect_target: String,
    pub redirect_status: RedirectStatus,
    pub preserve_path: bool,
    pub fixed_status: u16,
    pub fixed_body: String,
}

impl ResponseForm {
    pub fn new(response: Option<&FixedResponse>) -> Self {
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
    pub fn parse(&self, locale: Locale) -> Result<Option<FixedResponse>, String> {
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

pub fn route_kind(event: &Event) -> RouteKind {
    crate::components::port_config::find_option(&ROUTE_KINDS, &select_value(event))
}

pub fn route_input<V: 'static>(
    routes: &UseStateHandle<Vec<RouteForm>>,
    index: usize,
    read: fn(&Event) -> V,
    update: fn(&mut RouteForm, V),
) -> Callback<Event> {
    item_update(routes, index, update).reform(move |event: Event| read(&event))
}

pub fn route_view(
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
                crate::components::port_config::option_list(locale, &ROUTE_KINDS, route.response.kind),
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
            { route_bot_blocker_view(locale, routes, index, route) }
            { route_grpc_web_view(locale, routes, index, route) }
            { route_rate_limit_view(locale, routes, index, route) }
            { route_concurrency_limit_view(locale, routes, index, route) }
            if route.access_list.is_none() {
                { route_auth_view(locale, routes, index, route) }
            }
            { route_headers_view(locale, routes, index, route) }
            { route_client_cert_view(locale, routes, index, route) }
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
pub fn route_response_view(
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

pub fn code_option(code: u16, selected: u16) -> Html {
    html! {
        <option selected={code == selected} value={code.to_string()}>{code.to_string()}</option>
    }
}

/// Writes a server as its URL, followed by its weight when the weight is not the default.
pub fn format_server_line(server: &Server) -> String {
    let mut line = if server.weight == DEFAULT_WEIGHT {
        server.url.to_string()
    } else {
        format!("{} {}", server.url, server.weight)
    };
    if let Some(region) = &server.region {
        line.push_str(&format!(" region:{region}"));
    }
    if !server.countries.is_empty() {
        line.push_str(&format!(" countries:{}", server.countries.join(",")));
    }
    line
}

/// Reads a server line: the URL, and an optional weight after a space. An SRV URL has no
/// weight, because the SRV records give the weights.
pub fn parse_server_line(locale: Locale, line: &str) -> Result<Server, String> {
    let mut parts = line.split_whitespace();
    let url = ServerUrl::from_str(parts.next().unwrap_or_default())
        .map_err(|err| locale.error_message(&err))?;
    let rest = parts.collect::<Vec<_>>();
    if url.srv_name().is_some() && !rest.is_empty() {
        return Err(locale.t("proxy_form.srv_weight").into());
    }
    let mut weight = DEFAULT_WEIGHT;
    let mut weight_set = false;
    let mut region = None;
    let mut countries = Vec::new();
    for part in &rest {
        if let Some(r) = part.strip_prefix("region:") {
            region = Some(r.to_lowercase());
        } else if let Some(c) = part.strip_prefix("countries:") {
            countries = r3v3rs3_api::geoip::parse_countries(c);
        } else if !weight_set {
            weight = parse_weight(locale, part)?;
            weight_set = true;
        } else {
            return Err(locale.tf("proxy_form.invalid_weight", &[("value", *part)]));
        }
    }
    Ok(Server {
        url,
        weight,
        region,
        countries,
    })
}

/// Reads one upstream server URL from each line and skips the empty lines.
pub fn server_lines(text: &str) -> Vec<String> {
    text.lines()
        .map(str::trim)
        .filter(|line| !line.is_empty())
        .map(str::to_string)
        .collect()
}

/// The toggle of a route setting that replaces the proxy setting. The form and the hint show
/// when the toggle is on.
pub fn route_override_view(
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

pub fn route_access_list_view(
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

pub fn route_ip_filter_view(
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

                <div class="pt-2">
                    <crate::components::geoip_wizard::GeoIpRuleWizard
                        allow_countries={route.allow_countries.clone()}
                        deny_countries={route.deny_countries.clone()}
                        on_allow_change={let routes = routes.clone(); Callback::from(move |val: String| {
                            let mut list = (*routes).clone();
                            if let Some(r) = list.get_mut(index) {
                                r.allow_countries = val;
                                routes.set(list);
                            }
                        })}
                        on_deny_change={let routes = routes.clone(); Callback::from(move |val: String| {
                            let mut list = (*routes).clone();
                            if let Some(r) = list.get_mut(index) {
                                r.deny_countries = val;
                                routes.set(list);
                            }
                        })}
                    />
                </div>
            </>
        },
    )
}

pub fn route_bot_blocker_view(
    locale: Locale,
    routes: &UseStateHandle<Vec<RouteForm>>,
    index: usize,
    route: &RouteForm,
) -> Html {
    route_override_view(
        locale,
        route_input(routes, index, checked, |route, value| {
            route.override_bot_blocker = value
        }),
        route.override_bot_blocker,
        "http_form.override_bot_blocker",
        "http_form.route_bot_blocker_hint",
        html! {
            { toggle(route_input(routes, index, checked, |route, value| route.bot_blocker = value), route.bot_blocker, locale.t("http_form.bot_blocker"), "mt-4") }
        },
    )
}

pub fn route_grpc_web_view(
    locale: Locale,
    routes: &UseStateHandle<Vec<RouteForm>>,
    index: usize,
    route: &RouteForm,
) -> Html {
    route_override_view(
        locale,
        route_input(routes, index, checked, |route, value| {
            route.override_grpc_web = value
        }),
        route.override_grpc_web,
        "http_form.override_grpc_web",
        "http_form.route_grpc_web_hint",
        html! {
            { toggle(route_input(routes, index, checked, |route, value| route.grpc_web = value), route.grpc_web, locale.t("http_form.grpc_web"), "mt-4") }
        },
    )
}

pub fn route_rate_limit_view(
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

pub fn route_concurrency_limit_view(
    locale: Locale,
    routes: &UseStateHandle<Vec<RouteForm>>,
    index: usize,
    route: &RouteForm,
) -> Html {
    route_override_view(
        locale,
        route_input(routes, index, checked, |route, value| {
            route.override_concurrency_limit = value
        }),
        route.override_concurrency_limit,
        "http_form.override_concurrency_limit",
        "http_form.route_concurrency_limit_hint",
        concurrency_limit_view(
            locale,
            &route.concurrency_limit,
            route_input(routes, index, text, |route, value| {
                route.concurrency_limit.max = value
            }),
            route_input(routes, index, text, |route, value| {
                route.concurrency_limit.header = value
            }),
        ),
    )
}

pub fn route_auth_view(
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

pub fn route_headers_view(
    locale: Locale,
    routes: &UseStateHandle<Vec<RouteForm>>,
    index: usize,
    route: &RouteForm,
) -> Html {
    let on_apply_route_profile = {
        let routes = routes.clone();
        Callback::from(move |profile: SecurityProfile| {
            let mut routes_data = (*routes).clone();
            if let Some(r) = routes_data.get_mut(index) {
                let existing = parse_header_rules(&r.response_headers).unwrap_or_default();
                let updated = apply_profile_to_rules(&existing, profile);
                r.response_headers = format_header_rules(&updated);
                routes.set(routes_data);
            }
        })
    };

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
            on_apply_route_profile,
        ),
    )
}

pub fn route_client_cert_view(
    locale: Locale,
    routes: &UseStateHandle<Vec<RouteForm>>,
    index: usize,
    route: &RouteForm,
) -> Html {
    html! {
        <div class="mt-4">
            { toggle(
                route_input(routes, index, checked, |route, value| {
                    route.require_client_cert = value
                }),
                route.require_client_cert,
                locale.t("http_form.require_client_cert"),
                "",
            ) }
            <p class={HINT_CLASS}>{locale.t("http_form.require_client_cert_hint")}</p>
        </div>
    }
}

pub fn route_timeouts_view(
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

pub fn route_retry_view(
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
pub fn route_rewrite_view(
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

pub fn route_body_limit_view(
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
pub fn route_mirror_view(
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
pub fn body_limit_input(locale: Locale, value: &str, onchange: Callback<Event>) -> Html {
    html! {
        <>
            <label class={LABEL_CLASS}>{locale.t("http_form.max_body_size")}</label>
            <input type="number" min="0" value={value.to_string()} {onchange} class={INPUT_CLASS} />
        </>
    }
}

pub fn split_vhosts(vhosts: &str) -> Vec<String> {
    vhosts
        .split(',')
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(ToString::to_string)
        .collect()
}

pub fn parse_vhosts(
    locale: Locale,
    vhosts: &str,
    errors: &mut HashMap<String, String>,
) -> Vec<VirtualHost> {
    parse_comma_list(locale, vhosts, "vhosts", errors, VirtualHost::from_str)
}

pub fn parse_routes(
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

pub fn parse_route(
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
        allow_countries: parse_country_list(&route.allow_countries),
        deny_countries: parse_country_list(&route.deny_countries),
    });
    let rate_limit = route
        .override_rate_limit
        .then(|| parse_rate_limit(locale, &route.rate_limit, key, errors));
    let concurrency_limit = route
        .override_concurrency_limit
        .then(|| parse_concurrency_limit(locale, &route.concurrency_limit, key, errors));
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
        concurrency_limit,
        auth,
        access_list: route.access_list,
        headers,
        require_client_cert: route.require_client_cert,
        bot_blocker: route.override_bot_blocker.then_some(route.bot_blocker),
        grpc_web: route.override_grpc_web.then_some(route.grpc_web),
        ..target
    })
}

/// Reads the servers and the upstream settings of a route that proxies to its servers.
pub fn parse_upstream_route(
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
pub fn parse_server_lines(
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
pub fn parse_mirror(
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
pub fn parse_rewrite(
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
