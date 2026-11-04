use super::common::*;
use r3v3rs3_api::error::Error;
use r3v3rs3_api::i18n::Locale;
use r3v3rs3_api::upstream::{
    CircuitBreaker, HealthCheck, LoadBalancing, MAX_RETRY_ATTEMPTS, RetryOn, RetryPolicy,
    StickyCookie, UpstreamTimeouts,
};
use std::collections::HashMap;
use std::time::Duration;
use yew::prelude::*;

/// The largest timeout that the forms accept, one day.
pub const MAX_TIMEOUT_SECS: u64 = 86_400;

/// Shows a timeout in whole seconds. A timeout below one second shows as 1, so it does not read
/// as a disabled limit.
pub fn format_seconds(timeout: Duration) -> String {
    if timeout.is_zero() {
        return "0".into();
    }
    timeout.as_secs().max(1).to_string()
}

/// Parses a timeout in whole seconds from `min` to [`MAX_TIMEOUT_SECS`].
pub fn parse_seconds(
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

/// A labeled number input for a timeout in whole seconds.
pub fn seconds_input(
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

/// A timeout input of a TCP or UDP proxy in whole seconds, with its error and its hint.
pub fn timeout_field_view(
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

/// Reads a server weight from 0 to 65535.
pub fn parse_weight(locale: Locale, value: &str) -> Result<u16, String> {
    let value = value.trim();
    value
        .parse()
        .map_err(|_| locale.tf("proxy_form.invalid_weight", &[("value", value)]))
}

#[derive(Clone, PartialEq, Default)]
pub struct TimeoutsForm {
    pub connect: String,
    pub request: String,
}

impl TimeoutsForm {
    pub fn new(timeouts: &UpstreamTimeouts) -> Self {
        Self {
            connect: format_seconds(timeouts.connect),
            request: format_seconds(timeouts.request),
        }
    }
}

pub fn timeouts_view(
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

pub fn parse_timeouts(
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

/// The errors of an [`UpstreamForm`] use this key.
pub const UPSTREAM_KEY: &str = "upstream";

/// The load balancing policy and the health check of a proxy.
#[derive(Clone, PartialEq)]
pub struct UpstreamForm {
    pub load_balancing: LoadBalancing,
    pub max_fails: String,
    pub fail_timeout: String,
    pub interval: String,
    pub timeout: String,
    pub path: String,
    pub is_grpc: bool,
    pub grpc_service: String,
}

impl UpstreamForm {
    pub fn new(load_balancing: LoadBalancing, health_check: &HealthCheck) -> Self {
        Self {
            load_balancing,
            max_fails: health_check.max_fails.to_string(),
            fail_timeout: format_seconds(health_check.fail_timeout),
            interval: format_seconds(health_check.interval),
            timeout: format_seconds(health_check.timeout),
            path: health_check.path.clone(),
            is_grpc: health_check.grpc_service.is_some(),
            grpc_service: health_check.grpc_service.clone().unwrap_or_default(),
        }
    }

    /// Records an error under [`UPSTREAM_KEY`]. `http` allows a health check path.
    pub fn parse(
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
        let grpc_service = if self.is_grpc {
            Some(self.grpc_service.trim().to_string())
        } else {
            None
        };
        let health_check = HealthCheck {
            max_fails: or_error(
                parse_count(locale, &self.max_fails, "proxy_form.max_fails"),
                UPSTREAM_KEY,
                errors,
            ),
            fail_timeout,
            interval,
            timeout,
            path: if self.is_grpc {
                String::new()
            } else {
                self.path.trim().to_string()
            },
            grpc_service,
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
pub fn use_upstream_form(
    load_balancing: LoadBalancing,
    health_check: HealthCheck,
) -> UseStateHandle<UpstreamForm> {
    use_state(move || UpstreamForm::new(load_balancing, &health_check))
}

pub fn health_check_target_input(locale: Locale, form: &UseStateHandle<UpstreamForm>) -> Html {
    html! {
        <>
            <div class="mt-4">
                <label class={LABEL_CLASS}>{locale.t("proxy_form.health_check_protocol")}</label>
                <div class="flex items-center space-x-6 mt-1 mb-2">
                    <label class="inline-flex items-center text-sm font-medium text-neutral-300 cursor-pointer">
                        <input
                            type="radio"
                            name="health_check_proto"
                            checked={!form.is_grpc}
                            onchange={state_input(form, |_| (), |form, _| form.is_grpc = false)}
                            class="text-blue-500 focus:ring-blue-400 mr-2"
                        />
                        {locale.t("proxy_form.protocol_http")}
                    </label>
                    <label class="inline-flex items-center text-sm font-medium text-neutral-300 cursor-pointer">
                        <input
                            type="radio"
                            name="health_check_proto"
                            checked={form.is_grpc}
                            onchange={state_input(form, |_| (), |form, _| form.is_grpc = true)}
                            class="text-blue-500 focus:ring-blue-400 mr-2"
                        />
                        {locale.t("proxy_form.protocol_grpc")}
                    </label>
                </div>
            </div>
            if !form.is_grpc {
                <div>
                    <label class={LABEL_CLASS}>{locale.t("proxy_form.health_check_path")}</label>
                    <input type="text" autocapitalize="off" placeholder="/health" value={form.path.clone()} onchange={state_input(form, text, |form, value| form.path = value)} class={INPUT_CLASS} />
                </div>
            } else {
                <div>
                    <label class={LABEL_CLASS}>{locale.t("proxy_form.health_check_grpc_service")}</label>
                    <input type="text" autocapitalize="off" placeholder="helloworld.Greeter" value={form.grpc_service.clone()} onchange={state_input(form, text, |form, value| form.grpc_service = value)} class={INPUT_CLASS} />
                    <p class={HINT_CLASS}>{locale.t("proxy_form.health_check_grpc_service_hint")}</p>
                </div>
            }
        </>
    }
}

/// The load balancing policy select element and the health check inputs of a proxy. `with_path`
/// shows the health check path input of an HTTP proxy.
pub fn upstream_form_view(
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
                { health_check_target_input(locale, form) }
            }
            { error_view(errors.get(UPSTREAM_KEY)) }
            <p class={HINT_CLASS}>{locale.t(if with_path { "proxy_form.active_check_http_hint" } else { "proxy_form.active_check_hint" })}</p>
        </>
    }
}

/// The errors of a [`CircuitBreakerForm`] use this key.
pub const CIRCUIT_BREAKER_KEY: &str = "circuit_breaker";

/// The circuit breaker of an HTTP or TCP proxy.
#[derive(Clone, PartialEq, Debug)]
pub struct CircuitBreakerForm {
    pub enabled: bool,
    pub failure_ratio: String,
    pub min_requests: String,
    pub window: String,
    pub open_duration: String,
}

impl CircuitBreakerForm {
    pub fn new(breaker: &CircuitBreaker) -> Self {
        Self {
            enabled: breaker.enabled,
            failure_ratio: breaker.failure_ratio.to_string(),
            min_requests: breaker.min_requests.to_string(),
            window: format_seconds(breaker.window),
            open_duration: format_seconds(breaker.open_duration),
        }
    }

    /// Records an error under [`CIRCUIT_BREAKER_KEY`].
    pub fn parse(&self, locale: Locale, errors: &mut HashMap<String, String>) -> CircuitBreaker {
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

/// The circuit breaker toggle and its inputs. `hint_key` explains the breaker for the protocol.
pub fn circuit_breaker_view(
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

#[derive(Clone, PartialEq, Default)]
pub struct RetryForm {
    pub attempts: String,
    pub retry_on: Vec<RetryOn>,
    pub replay_body_limit: String,
}

impl RetryForm {
    pub fn new(retry: &RetryPolicy) -> Self {
        Self {
            attempts: retry.attempts.to_string(),
            retry_on: retry.retry_on.clone(),
            replay_body_limit: retry.replay_body_limit.to_string(),
        }
    }
}

/// The attempts, the replay body limit and the failures of a retry policy. Each change emits the
/// changed form to `onchange`.
pub fn retry_view(locale: Locale, form: &RetryForm, onchange: Callback<RetryForm>) -> Html {
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

pub fn retry_on_key(reason: RetryOn) -> &'static str {
    match reason {
        RetryOn::Connect => "http_form.retry_on_connect",
        RetryOn::Timeout => "http_form.retry_on_timeout",
        RetryOn::Http502 => "http_form.retry_on_http_502",
        RetryOn::Http503 => "http_form.retry_on_http_503",
        RetryOn::Http504 => "http_form.retry_on_http_504",
    }
}

pub fn retry_on_input(
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
pub fn toggled(current: &[RetryOn], reason: RetryOn, on: bool) -> Vec<RetryOn> {
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

pub fn parse_retry(
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

#[derive(Clone, PartialEq, Default)]
pub struct StickyForm {
    pub enabled: bool,
    pub name: String,
    /// `0` is a cookie without `Max-Age`.
    pub max_age: String,
}

impl StickyForm {
    pub fn new(sticky: &StickyCookie) -> Self {
        Self {
            enabled: sticky.enabled,
            name: sticky.name.clone(),
            max_age: sticky.max_age.map_or_else(|| "0".into(), format_seconds),
        }
    }
}

pub fn sticky_view(
    locale: Locale,
    form: &StickyForm,
    on_toggle: Callback<Event>,
    on_name: Callback<Event>,
    on_max_age: Callback<Event>,
) -> Html {
    html! {
        <>
            <div>
                { toggle(on_toggle, form.enabled, locale.t("http_form.enable_sticky_cookie"), "mt-4") }
            </div>
            if form.enabled {
                <div class="grid grid-cols-1 sm:grid-cols-2 gap-x-4">
                    <div>
                        <label class={LABEL_CLASS}>{locale.t("http_form.sticky_cookie_name")}</label>
                        <input type="text" autocapitalize="off" spellcheck="false" value={form.name.clone()} onchange={on_name} class={INPUT_CLASS} />
                    </div>
                    <div>{ seconds_input(locale.t("http_form.sticky_max_age"), &form.max_age, 0, on_max_age) }</div>
                </div>
            }
        </>
    }
}

/// Reads the sticky cookie. A `Max-Age` of 0 seconds is a cookie without `Max-Age`.
pub fn parse_sticky(
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

pub fn load_balancing(event: &Event) -> LoadBalancing {
    select_option(event, &LoadBalancing::ALL, LoadBalancing::as_str)
}
