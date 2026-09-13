use crate::components::http_proxy_config::{
    client_cert_label, parse_client_cert, use_client_certs,
};
use crate::{auth::use_ensure_auth, i18n::use_locale, API_ENDPOINT};
use gloo_net::http::{Request, RequestBuilder, Response};
use r3v3rs3_api::{
    app::{AdminConfig, AppConfig, LogConfig},
    cdn::{CdnRangesSource, CdnStatus},
    cert::CertInfo,
    discovery::{DiscoveryConfig, DockerDiscoveryConfig, Endpoint},
    error::ErrorMessage,
    i18n::Locale,
    id::ShortId,
};
use serde::{de::DeserializeOwned, Serialize};
use serde_json::json;
use std::{collections::HashMap, net::SocketAddr};
use time::{format_description::well_known::Rfc3339, OffsetDateTime};
use wasm_bindgen::{JsCast, UnwrapThrowExt};
use web_sys::{HtmlInputElement, HtmlSelectElement};
use yew::prelude::*;

const INPUT_CLASS: &str = "bg-neutral-50 dark:text-neutral-200 dark:bg-neutral-800 dark:border-neutral-600 border border-neutral-300 text-neutral-900 text-sm rounded-lg focus:ring-blue-500 focus:border-blue-500 block w-full p-2.5";
const LABEL_CLASS: &str =
    "block mt-4 mb-2 text-sm font-medium text-neutral-900 dark:text-neutral-200";

#[derive(Clone, PartialEq, Default)]
struct Fields {
    background_task_interval: String,
    session_expiry: String,
    max_login_attempts: String,
    login_attempts_reset: String,
    database_log_retention: String,
    http_challenge_addr: String,
    dns_challenge_resolver: String,
    docker_enabled: bool,
    docker_endpoint: String,
    docker_network: String,
    docker_exposed_by_default: bool,
    docker_client_cert: Option<ShortId>,
}

impl Fields {
    fn from_config(config: &AppConfig) -> Self {
        let value = serde_json::to_value(config).unwrap_or_default();
        let text = |ptr: &str| {
            value
                .pointer(ptr)
                .map(|v| match v {
                    serde_json::Value::String(s) => s.clone(),
                    other => other.to_string(),
                })
                .unwrap_or_default()
        };
        let docker = &config.discovery.docker;
        Self {
            docker_enabled: docker.enabled,
            docker_endpoint: docker.endpoint.clone(),
            docker_network: docker.network.clone(),
            docker_exposed_by_default: docker.exposed_by_default,
            docker_client_cert: docker.client_cert,
            background_task_interval: text("/background_task_interval"),
            session_expiry: text("/admin/session_expiry"),
            max_login_attempts: text("/admin/max_login_attempts"),
            login_attempts_reset: text("/admin/login_attempts_reset"),
            database_log_retention: text("/log/database_log_retention"),
            http_challenge_addr: text("/http_challenge_addr"),
            dns_challenge_resolver: text("/dns_challenge_resolver"),
        }
    }
}

#[derive(Clone, PartialEq)]
enum Notice {
    Success(&'static str),
    Failed(String),
}

#[function_component(Settings)]
pub fn settings() -> Html {
    use_ensure_auth();
    let locale = use_locale();

    let fields = use_state(|| Option::<Fields>::None);
    let notice = use_state(|| Option::<Notice>::None);
    let is_loading = use_state(|| false);
    let client_certs = use_client_certs();

    let fields_cloned = fields.clone();
    use_effect_with((), move |_| {
        wasm_bindgen_futures::spawn_local(async move {
            if let Ok(config) = get_config().await {
                fields_cloned.set(Some(Fields::from_config(&config)));
            }
        });
    });

    let Some(current) = (*fields).clone() else {
        return html! {};
    };
    let parsed = parse_fields(locale, &current);

    let onsubmit = {
        let parsed = parsed.clone();
        let notice = notice.clone();
        let is_loading = is_loading.clone();
        Callback::from(move |event: SubmitEvent| {
            event.prevent_default();
            let Ok(config) = parsed.clone() else {
                return;
            };
            if *is_loading {
                return;
            }
            is_loading.set(true);
            let notice = notice.clone();
            let is_loading = is_loading.clone();
            wasm_bindgen_futures::spawn_local(async move {
                notice.set(Some(match update_config(locale, &config).await {
                    Ok(()) => Notice::Success(locale.t("settings.saved")),
                    Err(message) => Notice::Failed(message),
                }));
                is_loading.set(false);
            });
        })
    };

    let errors = parsed.clone().err().unwrap_or_default();

    html! {
        <>
        <form {onsubmit} class="bg-white dark:bg-neutral-800 shadow-sm p-5 border border-neutral-300 dark:border-neutral-700 rounded-md">
            { notice_view(&notice) }

            <h2 class="text-lg font-semibold text-neutral-900 dark:text-neutral-200">{locale.t("settings.admin")}</h2>
            { text_field(&fields, &errors, locale.t("settings.session_expiry"), "session_expiry", "1h", |f| &mut f.session_expiry) }
            { text_field(&fields, &errors, locale.t("settings.max_login_attempts"), "max_login_attempts", "10", |f| &mut f.max_login_attempts) }
            <p class="mt-2 text-sm text-neutral-500 dark:text-neutral-400">{locale.t("settings.max_login_attempts_hint")}</p>
            { text_field(&fields, &errors, locale.t("settings.login_attempts_reset"), "login_attempts_reset", "15m", |f| &mut f.login_attempts_reset) }
            <p class="mt-2 text-sm text-neutral-500 dark:text-neutral-400">{locale.t("settings.login_attempts_reset_hint")}</p>

            <h2 class="mt-6 text-lg font-semibold text-neutral-900 dark:text-neutral-200">{locale.t("settings.server")}</h2>
            { text_field(&fields, &errors, locale.t("settings.background_task_interval"), "background_task_interval", "1h", |f| &mut f.background_task_interval) }
            { text_field(&fields, &errors, locale.t("settings.http_challenge_addr"), "http_challenge_addr", "0.0.0.0:80", |f| &mut f.http_challenge_addr) }
            { text_field(&fields, &errors, locale.t("settings.dns_challenge_resolver"), "dns_challenge_resolver", "1.1.1.1:53", |f| &mut f.dns_challenge_resolver) }
            <p class="mt-2 text-sm text-neutral-500 dark:text-neutral-400">{locale.t("settings.dns_challenge_resolver_hint")}</p>
            { text_field(&fields, &errors, locale.t("settings.database_log_retention"), "database_log_retention", "3months", |f| &mut f.database_log_retention) }
            <p class="mt-2 text-sm text-neutral-500 dark:text-neutral-400">{locale.t("settings.duration_hint")}</p>

            { docker_section(locale, &fields, &errors, &client_certs) }

            <div class="flex flex-col mt-4 sm:flex-row sm:items-center sm:justify-end">
                <button type="submit" disabled={parsed.is_err() || *is_loading} class="inline-flex justify-center items-center text-neutral-500 bg-neutral-50 dark:text-neutral-200 dark:bg-neutral-800 border border-neutral-300 dark:border-neutral-600 focus:outline-none hover:bg-neutral-100 hover:dark:bg-neutral-900 focus:ring-4 focus:ring-neutral-200 dark:focus:ring-neutral-600 font-medium rounded-lg text-sm px-4 py-2">
                    {locale.t("settings.save")}
                </button>
            </div>
        </form>
        <CdnStatusCard />
        </>
    }
}

fn notice_view(notice: &UseStateHandle<Option<Notice>>) -> Html {
    match &**notice {
        Some(Notice::Success(message)) => html! {
            <div class="bg-green-100 border border-green-400 text-green-700 dark:bg-green-950 dark:border-green-800 dark:text-green-300 px-4 py-3 rounded relative mb-4" role="status">
                <span class="block sm:inline">{*message}</span>
            </div>
        },
        Some(Notice::Failed(message)) => html! {
            <div class="bg-red-100 border border-red-400 text-red-700 dark:bg-red-950 dark:border-red-800 dark:text-red-300 px-4 py-3 rounded relative mb-4" role="alert">
                <span class="block sm:inline">{message}</span>
            </div>
        },
        None => html! {},
    }
}

fn text_field(
    fields: &UseStateHandle<Option<Fields>>,
    errors: &HashMap<String, String>,
    label: &'static str,
    key: &'static str,
    placeholder: &'static str,
    select: fn(&mut Fields) -> &mut String,
) -> Html {
    let (value, onchange) = field_binding(fields, select, HtmlInputElement::value);
    html! {
        <>
            <label class={LABEL_CLASS}>{label}</label>
            <input type="text" autocapitalize="off" {value} {onchange} class={INPUT_CLASS} placeholder={placeholder} />
            if let Some(err) = errors.get(key) {
                <p class="mt-2 text-sm text-red-600 dark:text-red-500">{err}</p>
            }
        </>
    }
}

const HINT_CLASS: &str = "mt-2 text-sm text-neutral-500 dark:text-neutral-400";

fn docker_section(
    locale: Locale,
    fields: &UseStateHandle<Option<Fields>>,
    errors: &HashMap<String, String>,
    certs: &[CertInfo],
) -> Html {
    let selected = (**fields).as_ref().and_then(|f| f.docker_client_cert);
    let fields_cloned = fields.clone();
    let on_client_cert = Callback::from(move |event: Event| {
        let target: HtmlSelectElement = event.target().unwrap_throw().dyn_into().unwrap_throw();
        let mut updated = (*fields_cloned).clone().unwrap_or_default();
        updated.docker_client_cert = parse_client_cert(&target.value());
        fields_cloned.set(Some(updated));
    });
    html! {
        <>
            <h2 class="mt-6 text-lg font-semibold text-neutral-900 dark:text-neutral-200">{locale.t("settings.docker_title")}</h2>
            { checkbox_field(fields, locale.t("settings.docker_enabled"), |f| &mut f.docker_enabled) }
            { text_field(fields, errors, locale.t("settings.docker_endpoint"), "docker_endpoint", "unix:///var/run/docker.sock", |f| &mut f.docker_endpoint) }
            <p class={HINT_CLASS}>{locale.t("settings.docker_endpoint_hint")}</p>
            { text_field(fields, errors, locale.t("settings.docker_network"), "docker_network", "proxy", |f| &mut f.docker_network) }
            <p class={HINT_CLASS}>{locale.t("settings.docker_network_hint")}</p>
            { checkbox_field(fields, locale.t("settings.docker_exposed_by_default"), |f| &mut f.docker_exposed_by_default) }
            <p class={HINT_CLASS}>{locale.t("settings.docker_exposed_by_default_hint")}</p>
            <label class={LABEL_CLASS}>{locale.t("settings.docker_client_cert")}</label>
            <select onchange={on_client_cert} class={INPUT_CLASS}>
                <option value="" selected={selected.is_none()}>{locale.t("proxy_form.client_cert_none")}</option>
                { for certs.iter().map(|cert| html! {
                    <option value={cert.id.to_string()} selected={selected == Some(cert.id)}>{client_cert_label(cert)}</option>
                }) }
            </select>
            <p class={HINT_CLASS}>{locale.t("settings.docker_client_cert_hint")}</p>
        </>
    }
}

/// The current value of a field and the callback that stores the new value of its input element.
fn field_binding<T: Clone + 'static>(
    fields: &UseStateHandle<Option<Fields>>,
    select: fn(&mut Fields) -> &mut T,
    read: fn(&HtmlInputElement) -> T,
) -> (T, Callback<Event>) {
    let mut current = (**fields).clone().unwrap_or_default();
    let value = select(&mut current).clone();
    let fields = fields.clone();
    let onchange = Callback::from(move |event: Event| {
        let target: HtmlInputElement = event.target().unwrap_throw().dyn_into().unwrap_throw();
        let mut updated = (*fields).clone().unwrap_or_default();
        *select(&mut updated) = read(&target);
        fields.set(Some(updated));
    });
    (value, onchange)
}

fn checkbox_field(
    fields: &UseStateHandle<Option<Fields>>,
    label: &'static str,
    select: fn(&mut Fields) -> &mut bool,
) -> Html {
    let (checked, onchange) = field_binding(fields, select, HtmlInputElement::checked);
    html! {
        <label class="flex items-center mt-4 text-sm font-medium text-neutral-900 dark:text-neutral-200">
            <input type="checkbox" {checked} {onchange} class="w-4 h-4 mr-2 rounded" />
            {label}
        </label>
    }
}

fn parse_docker(
    locale: Locale,
    fields: &Fields,
    errors: &mut HashMap<String, String>,
) -> DockerDiscoveryConfig {
    let endpoint = fields.docker_endpoint.trim();
    if endpoint.parse::<Endpoint>().is_err() {
        errors.insert(
            "docker_endpoint".into(),
            locale.t("settings.invalid_endpoint").into(),
        );
    }
    DockerDiscoveryConfig {
        enabled: fields.docker_enabled,
        endpoint: endpoint.to_string(),
        client_cert: fields.docker_client_cert,
        network: fields.docker_network.trim().to_string(),
        exposed_by_default: fields.docker_exposed_by_default,
    }
}

/// Parses one part of the config, or records `message` under `key`.
fn parse_part<T: DeserializeOwned>(
    errors: &mut HashMap<String, String>,
    key: &str,
    message: &str,
    value: serde_json::Value,
) -> Option<T> {
    serde_json::from_value::<T>(value)
        .map_err(|_| errors.insert(key.to_string(), message.to_string()))
        .ok()
}

fn parse_fields(locale: Locale, fields: &Fields) -> Result<AppConfig, HashMap<String, String>> {
    let mut errors = HashMap::new();
    let invalid_duration = locale.t("settings.invalid_duration");
    let max_login_attempts = match fields.max_login_attempts.trim().parse::<u32>() {
        Ok(0) | Err(_) => {
            errors.insert(
                "max_login_attempts".into(),
                locale.t("settings.positive_integer").into(),
            );
            0
        }
        Ok(n) => n,
    };
    let admin = [
        ("session_expiry", &fields.session_expiry),
        ("login_attempts_reset", &fields.login_attempts_reset),
    ]
    .into_iter()
    .map(|(key, value)| {
        parse_part::<AdminConfig>(
            &mut errors,
            key,
            invalid_duration,
            json!({ key: value.trim() }),
        )
    })
    .collect::<Option<Vec<_>>>();
    let log = parse_part::<LogConfig>(
        &mut errors,
        "database_log_retention",
        invalid_duration,
        json!({ "database_log_retention": fields.database_log_retention.trim() }),
    );
    let interval = parse_part::<AppConfig>(
        &mut errors,
        "background_task_interval",
        invalid_duration,
        json!({ "background_task_interval": fields.background_task_interval.trim() }),
    );
    let addr = fields.http_challenge_addr.trim().parse::<SocketAddr>().ok();
    if addr.is_none() {
        errors.insert(
            "http_challenge_addr".into(),
            locale.t("settings.invalid_socket_addr").into(),
        );
    }
    let docker = parse_docker(locale, fields, &mut errors);
    let resolver = parse_optional_addr(&fields.dns_challenge_resolver);
    if resolver.is_err() {
        errors.insert(
            "dns_challenge_resolver".into(),
            locale.t("settings.invalid_socket_addr").into(),
        );
    }
    match (admin, log, interval, addr, resolver) {
        (Some(admin), Some(log), Some(interval), Some(addr), Ok(resolver)) if errors.is_empty() => {
            Ok(AppConfig {
                background_task_interval: interval.background_task_interval,
                admin: AdminConfig {
                    session_expiry: admin[0].session_expiry,
                    max_login_attempts,
                    login_attempts_reset: admin[1].login_attempts_reset,
                },
                log,
                http_challenge_addr: addr,
                dns_challenge_resolver: resolver,
                discovery: DiscoveryConfig { docker },
            })
        }
        _ => Err(errors),
    }
}

/// An empty value means that no address is set.
fn parse_optional_addr(value: &str) -> Result<Option<SocketAddr>, std::net::AddrParseError> {
    let value = value.trim();
    if value.is_empty() {
        return Ok(None);
    }
    value.parse().map(Some)
}

async fn get_config() -> Result<AppConfig, gloo_net::Error> {
    Request::get(&format!("{API_ENDPOINT}/config"))
        .send()
        .await?
        .json()
        .await
}

async fn update_config(locale: Locale, config: &AppConfig) -> Result<(), String> {
    send_json(
        locale,
        Request::put(&format!("{API_ENDPOINT}/config")),
        config,
    )
    .await
}

/// Sends `body` as JSON. The error is the message of the failure in the selected language.
pub async fn send_json(
    locale: Locale,
    builder: RequestBuilder,
    body: &impl Serialize,
) -> Result<(), String> {
    let request = builder.json(body).map_err(|err| err.to_string())?;
    let response = request.send().await.map_err(|err| err.to_string())?;
    if response.ok() {
        return Ok(());
    }
    Err(error_message(locale, response).await)
}

/// The message of an admin API error response in the selected language.
pub async fn error_message(locale: Locale, response: Response) -> String {
    match response.json::<ErrorMessage>().await {
        Ok(ErrorMessage {
            error: Some(error), ..
        }) => locale.error_message(&error),
        Ok(err) => err.message,
        Err(err) => err.to_string(),
    }
}

#[function_component(CdnStatusCard)]
fn cdn_status_card() -> Html {
    let locale = use_locale();
    let status = use_state(|| Option::<CdnStatus>::None);
    let notice = use_state(|| Option::<Notice>::None);
    let is_loading = use_state(|| false);

    let status_cloned = status.clone();
    use_effect_with((), move |_| {
        wasm_bindgen_futures::spawn_local(async move {
            if let Ok(value) = get_cdn_status().await {
                status_cloned.set(Some(value));
            }
        });
    });

    let onclick = {
        let status = status.clone();
        let notice = notice.clone();
        let is_loading = is_loading.clone();
        Callback::from(move |_: MouseEvent| {
            if *is_loading {
                return;
            }
            is_loading.set(true);
            let status = status.clone();
            let notice = notice.clone();
            let is_loading = is_loading.clone();
            wasm_bindgen_futures::spawn_local(async move {
                match refresh_cdn_ranges(locale).await {
                    Ok(value) => {
                        status.set(Some(value));
                        notice.set(Some(Notice::Success(locale.t("settings.cdn_refreshed"))));
                    }
                    Err(message) => notice.set(Some(Notice::Failed(message))),
                }
                is_loading.set(false);
            });
        })
    };

    html! {
        <div class="mt-4 bg-white dark:bg-neutral-800 shadow-sm p-5 border border-neutral-300 dark:border-neutral-700 rounded-md">
            { notice_view(&notice) }
            <h2 class="text-lg font-semibold text-neutral-900 dark:text-neutral-200">{locale.t("settings.cdn_title")}</h2>
            <p class="mt-2 text-sm text-neutral-500 dark:text-neutral-400">{locale.t("settings.cdn_hint")}</p>
            if let Some(status) = &*status {
                { cdn_status_view(locale, status) }
            }
            <div class="flex flex-col mt-4 sm:flex-row sm:items-center sm:justify-end">
                <button type="button" {onclick} disabled={*is_loading} class="inline-flex justify-center items-center text-neutral-500 bg-neutral-50 dark:text-neutral-200 dark:bg-neutral-800 border border-neutral-300 dark:border-neutral-600 focus:outline-none hover:bg-neutral-100 hover:dark:bg-neutral-900 focus:ring-4 focus:ring-neutral-200 dark:focus:ring-neutral-600 font-medium rounded-lg text-sm px-4 py-2">
                    {locale.t("settings.refresh_now")}
                </button>
            </div>
        </div>
    }
}

fn cdn_status_view(locale: Locale, status: &CdnStatus) -> Html {
    let source = locale.t(match status.source {
        CdnRangesSource::Embedded => "settings.cdn_embedded",
        CdnRangesSource::Downloaded => "settings.cdn_downloaded",
    });
    let updated = locale.tf(
        "settings.cdn_updated",
        &[
            ("time", &format_unix_time(locale, status.updated_at)),
            ("source", source),
        ],
    );
    html! {
        <>
            <p class="mt-4 text-sm text-neutral-900 dark:text-neutral-200">{updated}</p>
            <ul class="mt-2 text-sm text-neutral-700 dark:text-neutral-300">
                { status.providers.iter().map(|provider| html! {
                    <li>{locale.tf("settings.cdn_provider_ranges", &[("provider", &provider.provider.to_string()), ("count", &provider.ranges.to_string())])}</li>
                }).collect::<Html>() }
            </ul>
            { status.errors.iter().map(|err| html! {
                <p class="mt-2 text-sm text-red-600 dark:text-red-500">{err}</p>
            }).collect::<Html>() }
        </>
    }
}

fn format_unix_time(locale: Locale, unix_time: i64) -> String {
    if unix_time <= 0 {
        return locale.t("settings.never").to_string();
    }
    OffsetDateTime::from_unix_timestamp(unix_time)
        .ok()
        .and_then(|time| time.format(&Rfc3339).ok())
        .unwrap_or_else(|| unix_time.to_string())
}

async fn get_cdn_status() -> Result<CdnStatus, gloo_net::Error> {
    Request::get(&format!("{API_ENDPOINT}/cdn"))
        .send()
        .await?
        .json()
        .await
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn docker_settings_round_trip_and_the_endpoint_is_validated() {
        let mut config = AppConfig::default();
        config.discovery.docker = DockerDiscoveryConfig {
            enabled: true,
            endpoint: "tcp://10.0.0.1:2375".into(),
            client_cert: Some("abc".parse().unwrap()),
            network: "proxy".into(),
            exposed_by_default: true,
        };
        let fields = Fields::from_config(&config);
        assert_eq!(parse_fields(Locale::En, &fields), Ok(config));

        let invalid = Fields {
            docker_endpoint: "docker.sock".into(),
            ..fields
        };
        let errors = parse_fields(Locale::En, &invalid).unwrap_err();
        assert_eq!(
            errors.get("docker_endpoint").map(String::as_str),
            Some(Locale::En.t("settings.invalid_endpoint"))
        );
    }
}

async fn refresh_cdn_ranges(locale: Locale) -> Result<CdnStatus, String> {
    let response = Request::post(&format!("{API_ENDPOINT}/cdn/refresh"))
        .send()
        .await
        .map_err(|err| err.to_string())?;
    if response.ok() {
        return response.json().await.map_err(|err| err.to_string());
    }
    Err(error_message(locale, response).await)
}
