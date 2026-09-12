use r3v3rs3_api::cidr::{format_cidr_list, parse_cidr_list};
use r3v3rs3_api::client_ip::ClientIpConfig;
use r3v3rs3_api::policy::{IpFilter, RateLimit, RatePeriod};
use r3v3rs3_api::proxy::{HttpProxy, Route, Server, ServerUrl};
use r3v3rs3_api::vhost::VirtualHost;
use std::collections::HashMap;
use std::str::FromStr;
use wasm_bindgen::{JsCast, UnwrapThrowExt};
use web_sys::{HtmlInputElement, HtmlSelectElement};
use yew::prelude::*;

const LABEL_CLASS: &str =
    "block mt-4 mb-2 text-sm font-medium text-neutral-900 dark:text-neutral-200";
const SECTION_CLASS: &str = "block mt-6 text-sm font-medium text-neutral-900 dark:text-neutral-200";
const INPUT_CLASS: &str = "bg-neutral-50 dark:text-neutral-200 dark:bg-neutral-800 dark:border-neutral-600 border border-neutral-300 text-neutral-900 text-sm rounded-lg focus:ring-blue-500 focus:border-blue-500 block w-full p-2.5";
const HINT_CLASS: &str = "mt-2 text-sm text-neutral-500";
const ERROR_CLASS: &str = "mt-2 text-sm text-red-600 dark:text-red-500";
const TOGGLE_CLASS: &str = "w-9 h-5 bg-neutral-200 dark:bg-neutral-600 peer-focus:outline-none peer-focus:ring-4 peer-focus:ring-blue-300 rounded-full peer peer-checked:after:translate-x-full peer-checked:after:border-white after:content-[''] after:absolute after:top-[2px] after:left-[2px] after:bg-white after:border-neutral-300 after:border after:rounded-full after:h-4 after:w-4 after:transition-all peer-checked:bg-blue-600";
const BUTTON_CLASS: &str = "inline-flex items-center px-4 py-2 text-sm font-medium text-neutral-500 dark:text-neutral-200 bg-white dark:bg-neutral-800 border border-neutral-300 dark:border-neutral-700 hover:bg-neutral-100 hover:dark:bg-neutral-900 focus:z-10 focus:ring-4 focus:ring-neutral-200 dark:focus:ring-neutral-600";

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
    let entry = get_proxy(&form, &routes);
    if entry != *prev_entry {
        prev_entry.set(entry.clone());
        props.onchanged.emit(entry.clone());
    }
    let errors = entry.err().unwrap_or_default();

    html! {
        <>
            { toggle(state_input(&form, checked, |form, value| form.upgrade_insecure = value), form.upgrade_insecure, "Automatically Redirect HTTP to HTTPS", "my-6") }

            <label class={LABEL_CLASS}>{"Virtual Hosts"}</label>
            <input type="text" autocapitalize="off" value={form.vhosts.clone()} onchange={state_input(&form, text, |form, value| form.vhosts = value)} class={INPUT_CLASS} placeholder="example.com" />
            { error_view(errors.get("vhosts")) }
            <p class={HINT_CLASS}>{"You can use commas to list multiple names and regex patterns, e.g, example.com, *.test.example.com, ^([a-z]+\\.)+example\\.com$ ."}</p>

            { toggle(state_input(&form, checked, |form, value| form.trust_cdn = value), form.trust_cdn, "Trust Client IP Headers from Known CDNs", "mt-6") }
            <p class={HINT_CLASS}>{"Reads the real client IP from Cloudflare, Fastly, Amazon CloudFront, Bunny CDN, Gcore, KeyCDN, Imperva and Google Cloud Load Balancing edge servers."}</p>

            <label class={LABEL_CLASS}>{"Trusted Proxies"}</label>
            <input type="text" autocapitalize="off" value={form.trusted_proxies.clone()} onchange={state_input(&form, text, |form, value| form.trusted_proxies = value)} class={INPUT_CLASS} placeholder="10.0.0.0/8, 192.168.1.10" />
            { error_view(errors.get("trusted_proxies")) }
            <p class={HINT_CLASS}>{"IP addresses or CIDR blocks of load balancers in front of r3v3rs3. Their X-Forwarded-For header is trusted."}</p>

            <label class={LABEL_CLASS}>{"Allowed IP Addresses"}</label>
            <input type="text" autocapitalize="off" value={form.allow.clone()} onchange={state_input(&form, text, |form, value| form.allow = value)} class={INPUT_CLASS} placeholder="192.168.0.0/16" />
            { error_view(errors.get("allow")) }
            <p class={HINT_CLASS}>{"When set, only clients in these IP addresses or CIDR blocks can access the proxy."}</p>

            <label class={LABEL_CLASS}>{"Denied IP Addresses"}</label>
            <input type="text" autocapitalize="off" value={form.deny.clone()} onchange={state_input(&form, text, |form, value| form.deny = value)} class={INPUT_CLASS} placeholder="203.0.113.0/24" />
            { error_view(errors.get("deny")) }
            <p class={HINT_CLASS}>{"Clients in these IP addresses or CIDR blocks receive 403 Forbidden. Denied addresses take precedence over allowed addresses."}</p>

            <label class={SECTION_CLASS}>{"Rate Limit"}</label>
            { rate_limit_view(
                &form.rate_limit,
                state_input(&form, text, |form, value| form.rate_limit.requests = value),
                state_input(&form, period, |form, value| form.rate_limit.per = value),
                state_input(&form, text, |form, value| form.rate_limit.burst = value),
            ) }
            { error_view(errors.get("rate_limit")) }
            <p class={HINT_CLASS}>{"Requests allowed for each client IP address. Clients over the limit receive 429 Too Many Requests. Set Requests to 0 to disable the limit. Set Burst to 0 to use the Requests value."}</p>

            <label class={SECTION_CLASS}>{"Routes"}</label>

            { routes.iter().enumerate().map(|(i, route)| {
                route_view(&routes, i, route, errors.get(&format!("routes_{i}")))
            }).collect::<Html>() }
        </>
    }
}

fn route_view(
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
            <label class="block mb-2 text-sm font-medium text-neutral-900 dark:text-neutral-200">{"Path"}</label>
            <input type="text" autocapitalize="off" placeholder="/" onchange={route_input(routes, index, text, |route, value| route.path = value)} value={route.path.clone()} class={INPUT_CLASS} />

            <label class={LABEL_CLASS}>{"Target"}</label>
            <input type="url" placeholder="https://example.com/backend" value={route.servers.join("\n")} onchange={route_input(routes, index, text, |route, value| route.servers = value.split('\n').map(str::to_string).collect())} class={INPUT_CLASS} />

            { route_ip_filter_view(routes, index, route) }
            { route_rate_limit_view(routes, index, route) }
            { error_view(error) }

            <div class="flex justify-end rounded-md mt-4 sm:ml-auto px-4 lg:px-0" role="group">
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
    routes: &UseStateHandle<Vec<RouteForm>>,
    index: usize,
    route: &RouteForm,
) -> Html {
    html! {
        <>
            <div>
                { toggle(route_input(routes, index, checked, |route, value| route.override_ip_filter = value), route.override_ip_filter, "Override IP Filter for This Route", "mt-6") }
            </div>
            if route.override_ip_filter {
                <label class={LABEL_CLASS}>{"Allowed IP Addresses"}</label>
                <input type="text" autocapitalize="off" placeholder="192.168.0.0/16" value={route.allow.clone()} onchange={route_input(routes, index, text, |route, value| route.allow = value)} class={INPUT_CLASS} />

                <label class={LABEL_CLASS}>{"Denied IP Addresses"}</label>
                <input type="text" autocapitalize="off" placeholder="203.0.113.0/24" value={route.deny.clone()} onchange={route_input(routes, index, text, |route, value| route.deny = value)} class={INPUT_CLASS} />
                <p class={HINT_CLASS}>{"This route uses these lists instead of the proxy lists. Leave both lists empty to allow every client."}</p>
            }
        </>
    }
}

fn route_rate_limit_view(
    routes: &UseStateHandle<Vec<RouteForm>>,
    index: usize,
    route: &RouteForm,
) -> Html {
    html! {
        <>
            <div>
                { toggle(route_input(routes, index, checked, |route, value| route.override_rate_limit = value), route.override_rate_limit, "Override Rate Limit for This Route", "mt-6") }
            </div>
            if route.override_rate_limit {
                { rate_limit_view(
                    &route.rate_limit,
                    route_input(routes, index, text, |route, value| route.rate_limit.requests = value),
                    route_input(routes, index, period, |route, value| route.rate_limit.per = value),
                    route_input(routes, index, text, |route, value| route.rate_limit.burst = value),
                ) }
                <p class={HINT_CLASS}>{"This route uses its own counter instead of the proxy counter. Set Requests to 0 to disable the limit for this route."}</p>
            }
        </>
    }
}

fn rate_limit_view(
    limit: &RateLimitForm,
    on_requests: Callback<Event>,
    on_per: Callback<Event>,
    on_burst: Callback<Event>,
) -> Html {
    html! {
        <div class="grid grid-cols-3 gap-4">
            <div>
                <label class={LABEL_CLASS}>{"Requests"}</label>
                <input type="number" min="0" value={limit.requests.clone()} onchange={on_requests} class={INPUT_CLASS} />
            </div>
            <div>
                <label class={LABEL_CLASS}>{"Per"}</label>
                <select onchange={on_per} class={INPUT_CLASS}>
                    { for RatePeriod::ALL.iter().map(|value| html! {
                        <option value={value.as_str()} selected={*value == limit.per}>{value.as_str()}</option>
                    }) }
                </select>
            </div>
            <div>
                <label class={LABEL_CLASS}>{"Burst"}</label>
                <input type="number" min="0" value={limit.burst.clone()} onchange={on_burst} class={INPUT_CLASS} />
            </div>
        </div>
    }
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

fn state_input<T, V>(
    state: &UseStateHandle<T>,
    read: fn(&Event) -> V,
    update: fn(&mut T, V),
) -> Callback<Event>
where
    T: Clone + 'static,
    V: 'static,
{
    let state = state.clone();
    Callback::from(move |event: Event| {
        let mut value = (*state).clone();
        update(&mut value, read(&event));
        state.set(value);
    })
}

fn route_input<V: 'static>(
    routes: &UseStateHandle<Vec<RouteForm>>,
    index: usize,
    read: fn(&Event) -> V,
    update: fn(&mut RouteForm, V),
) -> Callback<Event> {
    let routes = routes.clone();
    Callback::from(move |event: Event| {
        let mut list = (*routes).clone();
        if let Some(route) = list.get_mut(index) {
            update(route, read(&event));
            routes.set(list);
        }
    })
}

fn get_proxy(form: &ProxyForm, routes: &[RouteForm]) -> Result<HttpProxy, HashMap<String, String>> {
    let mut errors = HashMap::new();
    let vhosts = parse_vhosts(&form.vhosts, &mut errors);
    let routes = parse_routes(routes, &mut errors);
    let trusted_proxies = or_error(
        parse_cidr_list(&form.trusted_proxies),
        "trusted_proxies",
        &mut errors,
    );
    let ip_filter = IpFilter {
        allow: or_error(parse_cidr_list(&form.allow), "allow", &mut errors),
        deny: or_error(parse_cidr_list(&form.deny), "deny", &mut errors),
    };
    let rate_limit = parse_rate_limit(&form.rate_limit, "rate_limit", &mut errors);

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
    })
}

/// Returns the parsed value, or records the error under `key` and returns the default value.
fn or_error<T: Default, E: ToString>(
    result: Result<T, E>,
    key: &str,
    errors: &mut HashMap<String, String>,
) -> T {
    result.unwrap_or_else(|err| {
        errors.insert(key.to_string(), err.to_string());
        T::default()
    })
}

fn parse_rate_limit(
    form: &RateLimitForm,
    key: &str,
    errors: &mut HashMap<String, String>,
) -> RateLimit {
    RateLimit {
        requests: or_error(parse_count(&form.requests, "Requests"), key, errors),
        per: form.per,
        burst: or_error(parse_count(&form.burst, "Burst"), key, errors),
    }
}

fn parse_count(value: &str, name: &str) -> Result<u32, String> {
    let value = value.trim();
    if value.is_empty() {
        return Ok(0);
    }
    value
        .parse()
        .map_err(|_| format!("{name} must be a whole number from 0 to {}", u32::MAX))
}

fn parse_vhosts(vhosts: &str, errors: &mut HashMap<String, String>) -> Vec<VirtualHost> {
    let mut hosts = Vec::new();
    for host in vhosts
        .split(',')
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
    {
        match VirtualHost::from_str(&host) {
            Ok(host) => hosts.push(host),
            Err(err) => {
                errors.insert("vhosts".into(), err.to_string());
            }
        }
    }
    hosts
}

fn parse_routes(routes: &[RouteForm], errors: &mut HashMap<String, String>) -> Vec<Route> {
    routes
        .iter()
        .enumerate()
        .filter_map(|(i, route)| parse_route(route, &format!("routes_{i}"), errors))
        .collect()
}

fn parse_route(
    route: &RouteForm,
    key: &str,
    errors: &mut HashMap<String, String>,
) -> Option<Route> {
    if !route.path.starts_with('/') {
        errors.insert(key.into(), "Path must start with /".into());
        return None;
    }
    let servers = route
        .servers
        .iter()
        .filter_map(|url| match ServerUrl::from_str(url) {
            Ok(url) => Some(Server { url }),
            Err(err) => {
                errors.insert(key.into(), err.to_string());
                None
            }
        })
        .collect::<Vec<_>>();
    let ip_filter = route.override_ip_filter.then(|| IpFilter {
        allow: or_error(parse_cidr_list(&route.allow), key, errors),
        deny: or_error(parse_cidr_list(&route.deny), key, errors),
    });
    let rate_limit = route
        .override_rate_limit
        .then(|| parse_rate_limit(&route.rate_limit, key, errors));
    (!servers.is_empty()).then(|| Route {
        path: route.path.clone(),
        servers,
        ip_filter,
        rate_limit,
    })
}
