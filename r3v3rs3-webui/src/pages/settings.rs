use crate::components::http_proxy_config::{
    client_cert_label, parse_optional_id, use_client_certs,
};
use crate::components::worker_status::WorkerStatusCard;
use crate::{API_ENDPOINT, auth::use_ensure_auth, dialog, i18n::use_locale};
use gloo_net::http::{Request, RequestBuilder, Response};
use r3v3rs3_api::{
    acme::webhook_url_allowed,
    app::{
        AcmeExecConfig, AdminConfig, AppConfig, DISPLAY_TIMEZONES, Http2Limits, Http3Limits,
        LogConfig, McpClientConfig, McpConfig, NotificationConfig, UpgradeConfig, UpgradeQuery,
        UpgradeStage, UpgradeStageResponse, UpgradeStarted, WebhookConfig,
    },
    auth::BannedIpEntry,
    cdn::{CdnRangesSource, CdnStatus},
    cert::CertInfo,
    discovery::{DiscoveryConfig, DockerDiscoveryConfig, Endpoint, EtcdDiscoveryConfig},
    error::ErrorMessage,
    git_connection::PublicUrl,
    i18n::Locale,
    id::ShortId,
    platform::PlatformConfig,
};
use serde::{Serialize, de::DeserializeOwned};
use serde_json::json;
use std::{collections::HashMap, net::SocketAddr};
use wasm_bindgen::{JsCast, UnwrapThrowExt};
use web_sys::{HtmlInputElement, HtmlSelectElement, HtmlTextAreaElement};
use yew::prelude::*;
use yew_router::prelude::use_location;

const INPUT_CLASS: &str = "bg-neutral-50 dark:text-neutral-200 dark:bg-neutral-800 dark:border-neutral-600 border border-neutral-300 text-neutral-900 text-sm rounded-lg focus:ring-blue-500 focus:border-blue-500 block w-full p-2.5";
const LABEL_CLASS: &str =
    "block mt-4 mb-2 text-sm font-medium text-neutral-900 dark:text-neutral-200";

#[derive(Clone, PartialEq, Default)]
struct Fields {
    /// Empty means that no public address is set.
    public_url: String,
    background_task_interval: String,
    shutdown_graceful_timeout: String,
    upgrade_check_interval: String,
    session_expiry: String,
    max_login_attempts: String,
    login_attempts_reset: String,
    display_timezone: String,
    database_log_retention: String,
    audit_log_retention: String,
    cert_expiry_warning: String,
    /// Empty means that no webhook is set.
    webhook_url: String,
    /// A new token. Empty keeps the saved token.
    webhook_token: String,
    webhook_token_set: bool,
    webhook_clear_token: bool,
    /// Empty means the default timeout.
    webhook_timeout: String,
    http_challenge_addr: String,
    tls_alpn_challenge_addr: String,
    dns_challenge_resolver: String,
    upstream_dns_resolver: String,
    docker_enabled: bool,
    docker_endpoint: String,
    docker_network: String,
    docker_exposed_by_default: bool,
    docker_client_cert: Option<ShortId>,
    etcd_enabled: bool,
    /// Comma-separated endpoints.
    etcd_endpoints: String,
    etcd_username: String,
    /// A new password. Empty keeps the saved password.
    etcd_password: String,
    etcd_password_set: bool,
    etcd_clear_password: bool,
    etcd_prefix: String,
    etcd_client_cert: Option<ShortId>,
    http2_max_header_list_size: String,
    http2_max_pending_accept_reset_streams: String,
    http3_max_concurrent_bidi_streams: String,
    http3_max_concurrent_uni_streams: String,
    platform_enabled: bool,
    platform_docker: String,
    /// Comma-separated port names or ids.
    platform_proxy_ports: String,
    /// The settings of the platform that this form does not edit.
    platform_acme: Option<ShortId>,
    platform_agent_port: Option<u16>,
    platform_agent_host: Option<String>,
    mcp_enabled: bool,
    /// One client per line: `id, name, redirect_uri[, redirect_uri...]`.
    mcp_clients: String,
    /// One absolute path per line.
    acme_exec_programs: String,
    acme_exec_timeout: String,
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
        let etcd = &config.discovery.etcd;
        let webhook = config.notifications.webhook.as_ref();
        let platform = &config.platform;
        Self {
            public_url: config
                .public_url
                .as_ref()
                .map(ToString::to_string)
                .unwrap_or_default(),
            webhook_url: webhook.map(|w| w.url.clone()).unwrap_or_default(),
            webhook_token: String::new(),
            webhook_token_set: webhook.is_some_and(|w| w.token_set),
            webhook_clear_token: false,
            webhook_timeout: text("/notifications/webhook/timeout"),
            docker_enabled: docker.enabled,
            docker_endpoint: docker.endpoint.clone(),
            docker_network: docker.network.clone(),
            docker_exposed_by_default: docker.exposed_by_default,
            docker_client_cert: docker.client_cert,
            etcd_enabled: etcd.enabled,
            etcd_endpoints: etcd.endpoints.join(","),
            etcd_username: etcd.username.clone(),
            etcd_password: String::new(),
            etcd_password_set: etcd.password_set,
            etcd_clear_password: false,
            etcd_prefix: etcd.prefix.clone(),
            etcd_client_cert: etcd.client_cert,
            background_task_interval: text("/background_task_interval"),
            shutdown_graceful_timeout: text("/shutdown_graceful_timeout"),
            upgrade_check_interval: text("/upgrade/check_interval"),
            session_expiry: text("/admin/session_expiry"),
            max_login_attempts: text("/admin/max_login_attempts"),
            login_attempts_reset: text("/admin/login_attempts_reset"),
            display_timezone: text("/admin/display_timezone"),
            database_log_retention: text("/log/database_log_retention"),
            audit_log_retention: text("/log/audit_log_retention"),
            cert_expiry_warning: text("/notifications/cert_expiry_warning"),
            http_challenge_addr: text("/http_challenge_addr"),
            tls_alpn_challenge_addr: text("/tls_alpn_challenge_addr"),
            dns_challenge_resolver: text("/dns_challenge_resolver"),
            upstream_dns_resolver: text("/upstream_dns_resolver"),
            http2_max_header_list_size: text("/http2/max_header_list_size"),
            http2_max_pending_accept_reset_streams: text("/http2/max_pending_accept_reset_streams"),
            http3_max_concurrent_bidi_streams: text("/http3/max_concurrent_bidi_streams"),
            http3_max_concurrent_uni_streams: text("/http3/max_concurrent_uni_streams"),
            platform_enabled: platform.enabled,
            platform_docker: platform.docker.clone(),
            platform_proxy_ports: platform.proxy_ports.join(","),
            platform_acme: platform.acme,
            platform_agent_port: platform.agent_port,
            platform_agent_host: platform.agent_host.clone(),
            mcp_enabled: config.mcp.enabled,
            mcp_clients: config
                .mcp
                .clients
                .iter()
                .map(|client| {
                    format!(
                        "{}, {}, {}",
                        client.id,
                        client.name,
                        client.redirect_uris.join(", ")
                    )
                })
                .collect::<Vec<_>>()
                .join("\n"),
            acme_exec_programs: config
                .acme_exec
                .programs
                .iter()
                .map(|p| p.display().to_string())
                .collect::<Vec<_>>()
                .join("\n"),
            acme_exec_timeout: text("/acme_exec/timeout"),
        }
    }
}

#[derive(Clone, PartialEq)]
enum Notice {
    Success(String),
    Failed(String),
}

/// Scrolls the upgrade card into the view when the page opens through the link of the
/// update banner, after the form state has loaded.
fn scroll_to_upgrade_card(card: &NodeRef, loaded: bool, wanted: bool) {
    if !loaded || !wanted {
        return;
    }
    if let Some(element) = card.cast::<web_sys::Element>() {
        element.scroll_into_view();
    }
}

#[function_component(Settings)]
pub fn settings() -> Html {
    use_ensure_auth();
    let locale = use_locale();

    let fields = use_state(|| Option::<Fields>::None);
    let notice = use_state(|| Option::<Notice>::None);
    let test_notice = use_state(|| Option::<Notice>::None);
    let is_loading = use_state(|| false);
    let client_certs = use_client_certs();
    // The update banner links here with ?section=upgrade, so the page opens on the
    // upgrade card at the bottom instead of at the top.
    let scroll_to_upgrade = use_location()
        .and_then(|location| location.query::<UpgradeQuery>().ok())
        .map(|query| query.section == "upgrade")
        .unwrap_or(false);
    let upgrade_card_ref = use_node_ref();
    let effect_card_ref = upgrade_card_ref.clone();
    use_effect_with(
        ((*fields).is_some(), scroll_to_upgrade),
        move |(loaded, scroll)| scroll_to_upgrade_card(&effect_card_ref, *loaded, *scroll),
    );

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
        let fields = fields.clone();
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
            let fields = fields.clone();
            wasm_bindgen_futures::spawn_local(async move {
                let result = update_config(locale, &config).await;
                // The saved settings clear the typed secrets and show which secrets are set.
                if let (Ok(()), Ok(saved)) = (&result, get_config().await) {
                    fields.set(Some(Fields::from_config(&saved)));
                }
                notice.set(Some(match result {
                    Ok(()) => Notice::Success(locale.t("settings.saved").to_string()),
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

            // The public address is a server-wide setting: the Git providers send the webhook
            // requests of the apps to it, and the OAuth clients of the MCP endpoint see it as
            // the issuer.
            <h2 class="text-lg font-semibold text-neutral-900 dark:text-neutral-200">{locale.t("settings.public_url_title")}</h2>
            { text_field(&fields, &errors, locale.t("settings.public_url"), "public_url", "", |f| &mut f.public_url) }
            <p class="mt-2 text-sm text-neutral-500 dark:text-neutral-400">{locale.t("settings.public_url_hint")}</p>

            <h2 class="text-lg font-semibold text-neutral-900 dark:text-neutral-200">{locale.t("settings.admin")}</h2>
            { text_field(&fields, &errors, locale.t("settings.session_expiry"), "session_expiry", "1h", |f| &mut f.session_expiry) }
            { text_field(&fields, &errors, locale.t("settings.max_login_attempts"), "max_login_attempts", "10", |f| &mut f.max_login_attempts) }
            <p class="mt-2 text-sm text-neutral-500 dark:text-neutral-400">{locale.t("settings.max_login_attempts_hint")}</p>
            { text_field(&fields, &errors, locale.t("settings.login_attempts_reset"), "login_attempts_reset", "15m", |f| &mut f.login_attempts_reset) }
            <p class="mt-2 text-sm text-neutral-500 dark:text-neutral-400">{locale.t("settings.login_attempts_reset_hint")}</p>
            { select_field(&fields, &errors, locale.t("settings.display_timezone"), "display_timezone", &DISPLAY_TIMEZONES, |f| &mut f.display_timezone) }
            <p class="mt-2 text-sm text-neutral-500 dark:text-neutral-400">{locale.t("settings.display_timezone_hint")}</p>

            <h2 class="mt-6 text-lg font-semibold text-neutral-900 dark:text-neutral-200">{locale.t("settings.server")}</h2>
            { text_field(&fields, &errors, locale.t("settings.background_task_interval"), "background_task_interval", "1h", |f| &mut f.background_task_interval) }
            { text_field(&fields, &errors, locale.t("settings.shutdown_graceful_timeout"), "shutdown_graceful_timeout", "30s", |f| &mut f.shutdown_graceful_timeout) }
            { text_field(&fields, &errors, locale.t("settings.upgrade_check_interval"), "upgrade_check_interval", "1h", |f| &mut f.upgrade_check_interval) }
            <p class="mt-2 text-sm text-neutral-500 dark:text-neutral-400">{locale.t("settings.upgrade_check_interval_hint")}</p>
            { text_field(&fields, &errors, locale.t("settings.http_challenge_addr"), "http_challenge_addr", "0.0.0.0:80", |f| &mut f.http_challenge_addr) }
            { text_field(&fields, &errors, locale.t("settings.tls_alpn_challenge_addr"), "tls_alpn_challenge_addr", "0.0.0.0:443", |f| &mut f.tls_alpn_challenge_addr) }
            { text_field(&fields, &errors, locale.t("settings.dns_challenge_resolver"), "dns_challenge_resolver", "1.1.1.1:53", |f| &mut f.dns_challenge_resolver) }
            <p class="mt-2 text-sm text-neutral-500 dark:text-neutral-400">{locale.t("settings.dns_challenge_resolver_hint")}</p>
            { text_field(&fields, &errors, locale.t("settings.upstream_dns_resolver"), "upstream_dns_resolver", "127.0.0.1:8600", |f| &mut f.upstream_dns_resolver) }
            <p class="mt-2 text-sm text-neutral-500 dark:text-neutral-400">{locale.t("settings.upstream_dns_resolver_hint")}</p>
            { text_field(&fields, &errors, locale.t("settings.database_log_retention"), "database_log_retention", "3months", |f| &mut f.database_log_retention) }
            { text_field(&fields, &errors, locale.t("settings.audit_log_retention"), "audit_log_retention", "1year", |f| &mut f.audit_log_retention) }
            { text_field(&fields, &errors, locale.t("settings.cert_expiry_warning"), "cert_expiry_warning", "14days", |f| &mut f.cert_expiry_warning) }
            <p class="mt-2 text-sm text-neutral-500 dark:text-neutral-400">{locale.t("settings.cert_expiry_warning_hint")}</p>
            <p class="mt-2 text-sm text-neutral-500 dark:text-neutral-400">{locale.t("settings.duration_hint")}</p>

            <h2 class="mt-8 mb-2 text-lg font-semibold text-neutral-900 dark:text-neutral-100">{locale.t("settings.http_limits")}</h2>
            { text_field(&fields, &errors, locale.t("settings.http2_max_header_list_size"), "http2_max_header_list_size", "65536", |f| &mut f.http2_max_header_list_size) }
            <p class="mt-2 text-sm text-neutral-500 dark:text-neutral-400">{locale.t("settings.http2_max_header_list_size_hint")}</p>
            { text_field(&fields, &errors, locale.t("settings.http2_max_pending_accept_reset_streams"), "http2_max_pending_accept_reset_streams", "128", |f| &mut f.http2_max_pending_accept_reset_streams) }
            <p class="mt-2 text-sm text-neutral-500 dark:text-neutral-400">{locale.t("settings.http2_max_pending_accept_reset_streams_hint")}</p>
            { text_field(&fields, &errors, locale.t("settings.http3_max_concurrent_bidi_streams"), "http3_max_concurrent_bidi_streams", "100", |f| &mut f.http3_max_concurrent_bidi_streams) }
            <p class="mt-2 text-sm text-neutral-500 dark:text-neutral-400">{locale.t("settings.http3_max_concurrent_bidi_streams_hint")}</p>
            { text_field(&fields, &errors, locale.t("settings.http3_max_concurrent_uni_streams"), "http3_max_concurrent_uni_streams", "10", |f| &mut f.http3_max_concurrent_uni_streams) }
            <p class="mt-2 text-sm text-neutral-500 dark:text-neutral-400">{locale.t("settings.http3_max_concurrent_uni_streams_hint")}</p>

            { notifications_section(locale, &fields, &errors, &test_notice) }
            { platform_section(locale, &fields, &errors) }
            { mcp_section(locale, &fields, &errors) }
            { acme_exec_section(locale, &fields, &errors) }
            { docker_section(locale, &fields, &errors, &client_certs) }
            { etcd_section(locale, &fields, &errors, &client_certs) }

            <div class="flex flex-col mt-4 sm:flex-row sm:items-center sm:justify-end">
                <button type="submit" disabled={parsed.is_err() || *is_loading} class="inline-flex justify-center items-center text-neutral-500 bg-neutral-50 dark:text-neutral-200 dark:bg-neutral-800 border border-neutral-300 dark:border-neutral-600 focus:outline-none hover:bg-neutral-100 hover:dark:bg-neutral-900 focus:ring-4 focus:ring-neutral-200 dark:focus:ring-neutral-600 font-medium rounded-lg text-sm px-4 py-2">
                    {locale.t("settings.save")}
                </button>
            </div>
        </form>
        <CdnStatusCard />
        <WorkerStatusCard />
        <BannedIpsCard />
        <div ref={upgrade_card_ref}>
            <UpgradeCard />
        </div>
        </>
    }
}

fn notice_view(notice: &UseStateHandle<Option<Notice>>) -> Html {
    match &**notice {
        Some(Notice::Success(message)) => {
            success_box(html! { <span class="block sm:inline">{message}</span> })
        }
        Some(Notice::Failed(message)) => failure_box(message),
        None => html! {},
    }
}

/// A green status box around `content`.
pub fn success_box(content: Html) -> Html {
    html! {
        <div class="bg-green-100 border border-green-400 text-green-700 dark:bg-green-950 dark:border-green-800 dark:text-green-300 px-4 py-3 rounded relative mb-4" role="status">
            {content}
        </div>
    }
}

/// A red alert box with the message of a failure.
pub fn failure_box(message: &str) -> Html {
    html! {
        <div class="bg-red-100 border border-red-400 text-red-700 dark:bg-red-950 dark:border-red-800 dark:text-red-300 px-4 py-3 rounded relative mb-4" role="alert">
            <span class="block sm:inline">{message.to_string()}</span>
        </div>
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

/// A multi-line text field, one value per line.
fn textarea_field(
    fields: &UseStateHandle<Option<Fields>>,
    errors: &HashMap<String, String>,
    label: &'static str,
    key: &'static str,
    placeholder: &'static str,
    select: fn(&mut Fields) -> &mut String,
) -> Html {
    let mut current = (**fields).clone().unwrap_or_default();
    let value = select(&mut current).clone();
    let fields = fields.clone();
    let onchange = Callback::from(move |event: Event| {
        let target: HtmlTextAreaElement = event.target().unwrap_throw().dyn_into().unwrap_throw();
        let mut updated = (*fields).clone().unwrap_or_default();
        *select(&mut updated) = target.value();
        fields.set(Some(updated));
    });
    html! {
        <>
            <label class={LABEL_CLASS}>{label}</label>
            <textarea rows="4" autocapitalize="off" value={value} onchange={onchange} class={INPUT_CLASS} placeholder={placeholder} />
            if let Some(err) = errors.get(key) {
                <p class="mt-2 text-sm text-red-600 dark:text-red-500">{err}</p>
            }
        </>
    }
}

/// A select field whose options are the given values. The stored value stays selected when it
/// is not in the list, so a config.toml value survives an open page.
fn select_field(
    fields: &UseStateHandle<Option<Fields>>,
    errors: &HashMap<String, String>,
    label: &'static str,
    key: &'static str,
    options: &[&str],
    select: fn(&mut Fields) -> &mut String,
) -> Html {
    let mut current = (**fields).clone().unwrap_or_default();
    let value = select(&mut current).clone();
    let fields = fields.clone();
    let onchange = Callback::from(move |event: Event| {
        let target: HtmlSelectElement = event.target().unwrap_throw().dyn_into().unwrap_throw();
        let mut updated = (*fields).clone().unwrap_or_default();
        *select(&mut updated) = target.value();
        fields.set(Some(updated));
    });
    let extra = (!options.contains(&value.as_str())).then(|| {
        let value = value.clone();
        html! { <option value={value.clone()} selected={true}>{value}</option> }
    });
    let list: Vec<Html> = options
        .iter()
        .map(|option| {
            let option = (*option).to_owned();
            let selected = option == value;
            html! { <option value={option.clone()} selected={selected}>{option}</option> }
        })
        .collect();
    html! {
        <>
            <label class={LABEL_CLASS}>{label}</label>
            <select {onchange} class={INPUT_CLASS}>
                { extra }
                { for list }
            </select>
            if let Some(err) = errors.get(key) {
                <p class="mt-2 text-sm text-red-600 dark:text-red-500">{err}</p>
            }
        </>
    }
}

const HINT_CLASS: &str = "mt-2 text-sm text-neutral-500 dark:text-neutral-400";

fn section_title(title: &'static str) -> Html {
    html! {
        <h2 class="mt-6 text-lg font-semibold text-neutral-900 dark:text-neutral-200">{title}</h2>
    }
}

fn etcd_section(
    locale: Locale,
    fields: &UseStateHandle<Option<Fields>>,
    errors: &HashMap<String, String>,
    certs: &[CertInfo],
) -> Html {
    let password = SecretInput {
        label: "settings.etcd_password",
        saved: "settings.etcd_password_saved",
        hint: "settings.etcd_password_hint",
        clear: "settings.etcd_clear_password",
        is_set: |f| f.etcd_password_set,
        value: |f| &mut f.etcd_password,
        clear_value: |f| &mut f.etcd_clear_password,
    };
    html! {
        <>
            { section_title(locale.t("settings.etcd_title")) }
            { checkbox_field(fields, locale.t("settings.etcd_enabled"), |f| &mut f.etcd_enabled) }
            { text_field(fields, errors, locale.t("settings.etcd_endpoints"), "etcd_endpoints", "http://127.0.0.1:2379", |f| &mut f.etcd_endpoints) }
            <p class={HINT_CLASS}>{locale.t("settings.etcd_endpoints_hint")}</p>
            { text_field(fields, errors, locale.t("settings.etcd_username"), "etcd_username", "", |f| &mut f.etcd_username) }
            <p class={HINT_CLASS}>{locale.t("settings.etcd_username_hint")}</p>
            { secret_field(locale, fields, password) }
            { text_field(fields, errors, locale.t("settings.etcd_prefix"), "etcd_prefix", "r3v3rs3", |f| &mut f.etcd_prefix) }
            <p class={HINT_CLASS}>{locale.t("settings.etcd_prefix_hint")}</p>
            { client_cert_field(locale, fields, certs, "settings.etcd_client_cert", "settings.etcd_client_cert_hint", |f| &mut f.etcd_client_cert) }
        </>
    }
}

fn platform_section(
    locale: Locale,
    fields: &UseStateHandle<Option<Fields>>,
    errors: &HashMap<String, String>,
) -> Html {
    let rotate_on_click = {
        Callback::from(move |event: MouseEvent| {
            event.prevent_default();
            dialog::confirm_then(
                locale,
                locale.t("settings.rotate_key_question").to_string(),
                async move {
                    match Request::post(&format!("{API_ENDPOINT}/platform/key/rotate"))
                        .send()
                        .await
                    {
                        Ok(resp) if resp.ok() => {
                            let _ = dialog::message(
                                locale,
                                locale.t("settings.rotate_key_success"),
                                dialog::Icon::Success,
                            )
                            .await;
                        }
                        Ok(resp) => {
                            let _ =
                                dialog::message(locale, &resp.status_text(), dialog::Icon::Error)
                                    .await;
                        }
                        Err(err) => {
                            let _ = dialog::message(locale, &err.to_string(), dialog::Icon::Error)
                                .await;
                        }
                    }
                },
            );
        })
    };
    html! {
        <>
            { section_title(locale.t("settings.platform_title")) }
            { checkbox_field(fields, locale.t("settings.platform_enabled"), |f| &mut f.platform_enabled) }
            <p class={HINT_CLASS}>{locale.t("settings.platform_enabled_hint")}</p>
            { text_field(fields, errors, locale.t("settings.platform_docker"), "platform_docker", "unix:///var/run/docker.sock", |f| &mut f.platform_docker) }
            { text_field(fields, errors, locale.t("settings.platform_proxy_ports"), "platform_proxy_ports", "http,https", |f| &mut f.platform_proxy_ports) }
            <p class={HINT_CLASS}>{locale.t("settings.platform_proxy_ports_hint")}</p>
            <div class="mt-4">
                <button
                    type="button"
                    onclick={rotate_on_click}
                    class="px-3 py-1.5 rounded bg-neutral-200 dark:bg-neutral-700 text-neutral-800 dark:text-neutral-200 hover:bg-neutral-300 dark:hover:bg-neutral-600 text-sm font-medium"
                >
                    {locale.t("settings.rotate_key")}
                </button>
                <p class={HINT_CLASS}>{locale.t("settings.rotate_key_hint")}</p>
            </div>
        </>
    }
}

fn mcp_section(
    locale: Locale,
    fields: &UseStateHandle<Option<Fields>>,
    errors: &HashMap<String, String>,
) -> Html {
    html! {
        <>
            { section_title(locale.t("settings.mcp_title")) }
            { checkbox_field(fields, locale.t("settings.mcp_enabled"), |f| &mut f.mcp_enabled) }
            <p class={HINT_CLASS}>{locale.t("settings.mcp_enabled_hint")}</p>
            { textarea_field(fields, errors, locale.t("settings.mcp_clients"), "mcp_clients", "r3v3rs3-cli, r3v3rs3 CLI client, http://127.0.0.1:8765/callback", |f| &mut f.mcp_clients) }
            <p class={HINT_CLASS}>{locale.t("settings.mcp_clients_hint")}</p>
        </>
    }
}

fn acme_exec_section(
    locale: Locale,
    fields: &UseStateHandle<Option<Fields>>,
    errors: &HashMap<String, String>,
) -> Html {
    html! {
        <>
            { section_title(locale.t("settings.acme_exec_title")) }
            { textarea_field(fields, errors, locale.t("settings.acme_exec_programs"), "acme_exec_programs", "/usr/local/bin/r3v3rs3-dns-hook", |f| &mut f.acme_exec_programs) }
            <p class={HINT_CLASS}>{locale.t("settings.acme_exec_programs_hint")}</p>
            { text_field(fields, errors, locale.t("settings.acme_exec_timeout"), "acme_exec_timeout", "30s", |f| &mut f.acme_exec_timeout) }
        </>
    }
}

fn notifications_section(
    locale: Locale,
    fields: &UseStateHandle<Option<Fields>>,
    errors: &HashMap<String, String>,
    test_notice: &UseStateHandle<Option<Notice>>,
) -> Html {
    let token = SecretInput {
        label: "settings.webhook_token",
        saved: "settings.webhook_token_saved",
        hint: "settings.webhook_token_hint",
        clear: "settings.webhook_clear_token",
        is_set: |f| f.webhook_token_set,
        value: |f| &mut f.webhook_token,
        clear_value: |f| &mut f.webhook_clear_token,
    };
    html! {
        <>
            { section_title(locale.t("settings.notifications_title")) }
            { text_field(fields, errors, locale.t("settings.webhook_url"), "webhook_url", "https://hooks.example.com/r3v3rs3", |f| &mut f.webhook_url) }
            <p class={HINT_CLASS}>{locale.t("settings.webhook_url_hint")}</p>
            { secret_field(locale, fields, token) }
            { text_field(fields, errors, locale.t("settings.webhook_timeout"), "webhook_timeout", "10s", |f| &mut f.webhook_timeout) }
            <div class="mt-4">
                { notice_view(test_notice) }
                <button type="button" onclick={send_test_notification(locale, test_notice)} class="inline-flex justify-center items-center text-neutral-500 bg-neutral-50 dark:text-neutral-200 dark:bg-neutral-800 border border-neutral-300 dark:border-neutral-600 focus:outline-none hover:bg-neutral-100 hover:dark:bg-neutral-900 focus:ring-4 focus:ring-neutral-200 dark:focus:ring-neutral-600 font-medium rounded-lg text-sm px-4 py-2">
                    {locale.t("settings.notification_test")}
                </button>
            </div>
            <p class={HINT_CLASS}>{locale.t("settings.notification_test_hint")}</p>
        </>
    }
}

/// Asks the server to send a test notification to the webhook of the saved settings.
fn send_test_notification(
    locale: Locale,
    notice: &UseStateHandle<Option<Notice>>,
) -> Callback<MouseEvent> {
    let notice = notice.clone();
    Callback::from(move |_: MouseEvent| {
        let notice = notice.clone();
        wasm_bindgen_futures::spawn_local(async move {
            let url = format!("{API_ENDPOINT}/config/notifications/test");
            notice.set(Some(
                match fetch_json::<()>(locale, Request::post(&url)).await {
                    Ok(()) => {
                        Notice::Success(locale.t("settings.notification_test_sent").to_string())
                    }
                    Err(message) => Notice::Failed(message),
                },
            ));
        });
    })
}

/// The locale keys and the fields of a secret that the admin API does not return.
struct SecretInput {
    label: &'static str,
    /// The placeholder when a secret is saved.
    saved: &'static str,
    hint: &'static str,
    /// The label of the checkbox that removes the saved secret.
    clear: &'static str,
    is_set: fn(&Fields) -> bool,
    value: fn(&mut Fields) -> &mut String,
    clear_value: fn(&mut Fields) -> &mut bool,
}

fn secret_field(
    locale: Locale,
    fields: &UseStateHandle<Option<Fields>>,
    input: SecretInput,
) -> Html {
    let is_set = (**fields).as_ref().is_some_and(input.is_set);
    let placeholder = if is_set { locale.t(input.saved) } else { "" };
    html! {
        <>
            { password_field(fields, locale.t(input.label), placeholder, input.value) }
            <p class={HINT_CLASS}>{locale.t(input.hint)}</p>
            if is_set {
                { checkbox_field(fields, locale.t(input.clear), input.clear_value) }
            }
        </>
    }
}

fn client_cert_field(
    locale: Locale,
    fields: &UseStateHandle<Option<Fields>>,
    certs: &[CertInfo],
    label: &'static str,
    hint: &'static str,
    select: fn(&mut Fields) -> &mut Option<ShortId>,
) -> Html {
    html! {
        <>
            <label class={LABEL_CLASS}>{locale.t(label)}</label>
            { client_cert_select(fields, certs, locale.t("proxy_form.client_cert_none"), select) }
            <p class={HINT_CLASS}>{locale.t(hint)}</p>
        </>
    }
}

fn password_field(
    fields: &UseStateHandle<Option<Fields>>,
    label: &'static str,
    placeholder: &'static str,
    select: fn(&mut Fields) -> &mut String,
) -> Html {
    let (value, onchange) = field_binding(fields, select, HtmlInputElement::value);
    html! {
        <>
            <label class={LABEL_CLASS}>{label}</label>
            <input type="password" autocomplete="new-password" {value} {onchange} class={INPUT_CLASS} {placeholder} />
        </>
    }
}

fn client_cert_select(
    fields: &UseStateHandle<Option<Fields>>,
    certs: &[CertInfo],
    none_label: &'static str,
    select: fn(&mut Fields) -> &mut Option<ShortId>,
) -> Html {
    let mut current = (**fields).clone().unwrap_or_default();
    let selected = *select(&mut current);
    let fields = fields.clone();
    let onchange = Callback::from(move |event: Event| {
        let target: HtmlSelectElement = event.target().unwrap_throw().dyn_into().unwrap_throw();
        let mut updated = (*fields).clone().unwrap_or_default();
        *select(&mut updated) = parse_optional_id(&target.value());
        fields.set(Some(updated));
    });
    html! {
        <select {onchange} class={INPUT_CLASS}>
            <option value="" selected={selected.is_none()}>{none_label}</option>
            { for certs.iter().map(|cert| html! {
                <option value={cert.id.to_string()} selected={selected == Some(cert.id)}>{client_cert_label(cert)}</option>
            }) }
        </select>
    }
}

fn docker_section(
    locale: Locale,
    fields: &UseStateHandle<Option<Fields>>,
    errors: &HashMap<String, String>,
    certs: &[CertInfo],
) -> Html {
    html! {
        <>
            { section_title(locale.t("settings.docker_title")) }
            { checkbox_field(fields, locale.t("settings.docker_enabled"), |f| &mut f.docker_enabled) }
            { text_field(fields, errors, locale.t("settings.docker_endpoint"), "docker_endpoint", "unix:///var/run/docker.sock", |f| &mut f.docker_endpoint) }
            <p class={HINT_CLASS}>{locale.t("settings.docker_endpoint_hint")}</p>
            { text_field(fields, errors, locale.t("settings.docker_network"), "docker_network", "proxy", |f| &mut f.docker_network) }
            <p class={HINT_CLASS}>{locale.t("settings.docker_network_hint")}</p>
            { checkbox_field(fields, locale.t("settings.docker_exposed_by_default"), |f| &mut f.docker_exposed_by_default) }
            <p class={HINT_CLASS}>{locale.t("settings.docker_exposed_by_default_hint")}</p>
            { client_cert_field(locale, fields, certs, "settings.docker_client_cert", "settings.docker_client_cert_hint", |f| &mut f.docker_client_cert) }
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

/// The secret that an update sends. An empty secret removes the saved secret, and no secret
/// keeps it.
fn secret_update(typed: &str, clear: bool) -> Option<String> {
    if clear {
        return Some(String::new());
    }
    Some(typed.trim().to_string()).filter(|secret| !secret.is_empty())
}

/// The non-empty items of a comma-separated value.
fn comma_list(value: &str) -> Vec<String> {
    value
        .split(',')
        .map(str::trim)
        .filter(|item| !item.is_empty())
        .map(str::to_string)
        .collect()
}

/// Parses the MCP clients, one client per line: `id, name, redirect_uri[, redirect_uri...]`.
fn parse_mcp(locale: Locale, fields: &Fields, errors: &mut HashMap<String, String>) -> McpConfig {
    let mut clients = Vec::new();
    let lines = fields
        .mcp_clients
        .lines()
        .map(str::trim)
        .filter(|line| !line.is_empty());
    for line in lines {
        let parts = line.split(',').map(str::trim).collect::<Vec<_>>();
        if parts.len() < 3 || parts.iter().any(|part| part.is_empty()) {
            errors.insert(
                "mcp_clients".into(),
                locale.t("settings.mcp_clients_line").into(),
            );
            continue;
        }
        clients.push(McpClientConfig {
            id: parts[0].to_string(),
            name: parts[1].to_string(),
            redirect_uris: parts[2..].iter().map(|uri| uri.to_string()).collect(),
        });
    }
    McpConfig {
        enabled: fields.mcp_enabled,
        clients,
    }
}

/// Parses the allowlist of the exec DNS provider and the timeout of one program call.
fn parse_acme_exec(
    locale: Locale,
    fields: &Fields,
    errors: &mut HashMap<String, String>,
) -> AcmeExecConfig {
    let programs = fields
        .acme_exec_programs
        .lines()
        .map(str::trim)
        .filter(|line| !line.is_empty())
        .map(std::path::PathBuf::from)
        .collect();
    let timeout = parse_part::<AcmeExecConfig>(
        errors,
        "acme_exec_timeout",
        locale.t("settings.invalid_duration"),
        json!({ "timeout": fields.acme_exec_timeout.trim() }),
    );
    AcmeExecConfig {
        programs,
        timeout: timeout.map(|parsed| parsed.timeout).unwrap_or_default(),
    }
}

fn parse_etcd(
    locale: Locale,
    fields: &Fields,
    errors: &mut HashMap<String, String>,
) -> EtcdDiscoveryConfig {
    let endpoints = comma_list(&fields.etcd_endpoints);
    if endpoints.is_empty() {
        errors.insert(
            "etcd_endpoints".into(),
            locale.t("settings.etcd_endpoints_required").into(),
        );
    } else if endpoints.iter().any(|e| e.parse::<Endpoint>().is_err()) {
        errors.insert(
            "etcd_endpoints".into(),
            locale.t("settings.invalid_endpoint").into(),
        );
    }
    let prefix = fields.etcd_prefix.trim().trim_matches('/');
    if prefix.is_empty() {
        errors.insert(
            "etcd_prefix".into(),
            locale.t("settings.etcd_prefix_required").into(),
        );
    }
    let username = fields.etcd_username.trim();
    let password = secret_update(&fields.etcd_password, fields.etcd_clear_password);
    if let Some(key) = credentials_error(username, password.as_deref(), fields.etcd_password_set) {
        errors.insert("etcd_username".into(), locale.t(key).into());
    }
    EtcdDiscoveryConfig {
        enabled: fields.etcd_enabled,
        endpoints,
        client_cert: fields.etcd_client_cert,
        username: username.to_string(),
        password,
        password_set: false,
        prefix: prefix.to_string(),
    }
}

/// The message key when the user name and the password are not set together. `password` is the
/// secret that the update sends.
fn credentials_error(
    username: &str,
    password: Option<&str>,
    password_set: bool,
) -> Option<&'static str> {
    let has_password = match password {
        Some(secret) => !secret.is_empty(),
        None => password_set,
    };
    match (username.is_empty(), has_password) {
        (false, false) => Some("settings.etcd_password_required"),
        (true, true) => Some("settings.etcd_username_required"),
        _ => None,
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

/// Parses each duration field as a part of the config. `None` when one of them is invalid.
fn parse_durations<T: DeserializeOwned>(
    errors: &mut HashMap<String, String>,
    message: &str,
    durations: [(&str, &String); 2],
) -> Option<Vec<T>> {
    durations
        .into_iter()
        .map(|(key, value)| parse_part::<T>(errors, key, message, json!({ key: value.trim() })))
        .collect()
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
    let admin = parse_durations::<AdminConfig>(
        &mut errors,
        invalid_duration,
        [
            ("session_expiry", &fields.session_expiry),
            ("login_attempts_reset", &fields.login_attempts_reset),
        ],
    );
    let display_timezone = fields.display_timezone.trim().to_owned();
    if !DISPLAY_TIMEZONES.contains(&display_timezone.as_str()) {
        errors.insert(
            "display_timezone".into(),
            locale.t("settings.invalid_display_timezone").into(),
        );
    }
    let log = parse_durations::<LogConfig>(
        &mut errors,
        invalid_duration,
        [
            ("database_log_retention", &fields.database_log_retention),
            ("audit_log_retention", &fields.audit_log_retention),
        ],
    );
    let interval = parse_part::<AppConfig>(
        &mut errors,
        "background_task_interval",
        invalid_duration,
        json!({
            "background_task_interval": fields.background_task_interval.trim(),
            "shutdown_graceful_timeout": fields.shutdown_graceful_timeout.trim(),
        }),
    );
    let upgrade = parse_part::<AppConfig>(
        &mut errors,
        "upgrade_check_interval",
        invalid_duration,
        json!({
            "upgrade": {
                "check_interval": fields.upgrade_check_interval.trim(),
            },
        }),
    );
    let notifications = parse_notifications(locale, fields, &mut errors);
    let addr = parse_addr(
        locale,
        &mut errors,
        "http_challenge_addr",
        &fields.http_challenge_addr,
    );
    let tls_alpn_addr = parse_addr(
        locale,
        &mut errors,
        "tls_alpn_challenge_addr",
        &fields.tls_alpn_challenge_addr,
    );
    let docker = parse_docker(locale, fields, &mut errors);
    let etcd = parse_etcd(locale, fields, &mut errors);
    let resolver = parse_resolver(
        locale,
        &mut errors,
        "dns_challenge_resolver",
        &fields.dns_challenge_resolver,
    );
    let upstream_resolver = parse_resolver(
        locale,
        &mut errors,
        "upstream_dns_resolver",
        &fields.upstream_dns_resolver,
    );
    let http2 = parse_http2_limits(locale, fields, &mut errors);
    let http3 = parse_http3_limits(locale, fields, &mut errors);
    let public_url = parse_public_url(locale, &mut errors, &fields.public_url);
    match (
        admin,
        (log, notifications),
        interval,
        upgrade,
        (addr, tls_alpn_addr),
        (resolver, upstream_resolver),
        (http2, http3),
        public_url,
    ) {
        (
            Some(admin),
            (Some(log), Some(notifications)),
            Some(interval),
            Some(upgrade),
            (Some(addr), Some(tls_alpn_addr)),
            (Ok(resolver), Ok(upstream_resolver)),
            (Some(http2), Some(http3)),
            public_url,
        ) if errors.is_empty() => Ok(AppConfig {
            public_url: public_url.flatten(),
            background_task_interval: interval.background_task_interval,
            shutdown_graceful_timeout: interval.shutdown_graceful_timeout,
            admin: AdminConfig {
                session_expiry: admin[0].session_expiry,
                max_login_attempts,
                login_attempts_reset: admin[1].login_attempts_reset,
                display_timezone,
            },
            log: LogConfig {
                database_log_retention: log[0].database_log_retention,
                audit_log_retention: log[1].audit_log_retention,
            },
            notifications,
            http_challenge_addr: addr,
            tls_alpn_challenge_addr: tls_alpn_addr,
            dns_challenge_resolver: resolver,
            upstream_dns_resolver: upstream_resolver,
            discovery: DiscoveryConfig { docker, etcd },
            // The server keeps the worker mode of config.toml; a restart applies it.
            acme_exec: parse_acme_exec(locale, fields, &mut errors),
            platform: PlatformConfig {
                enabled: fields.platform_enabled,
                docker: fields.platform_docker.trim().to_string(),
                proxy_ports: comma_list(&fields.platform_proxy_ports),
                acme: fields.platform_acme,
                agent_port: fields.platform_agent_port,
                agent_host: fields.platform_agent_host.clone(),
            },
            worker_mode: Default::default(),
            mcp: parse_mcp(locale, fields, &mut errors),
            http2,
            http3,
            upgrade: UpgradeConfig {
                check_interval: upgrade.upgrade.check_interval,
            },
        }),
        _ => Err(errors),
    }
}

/// Parses one unsigned integer field with its range. `None` marks an invalid value.
fn parse_limit(
    locale: Locale,
    errors: &mut HashMap<String, String>,
    field: &str,
    value: &str,
    min: u64,
    max: u64,
) -> Option<u64> {
    match value.trim().parse::<u64>() {
        Ok(n) if (min..=max).contains(&n) => Some(n),
        _ => {
            let (min, max) = (min.to_string(), max.to_string());
            errors.insert(
                field.into(),
                locale.tf("settings.limits_range", &[("min", &min), ("max", &max)]),
            );
            None
        }
    }
}

fn parse_http2_limits(
    locale: Locale,
    fields: &Fields,
    errors: &mut HashMap<String, String>,
) -> Option<Http2Limits> {
    let max_header_list_size = parse_limit(
        locale,
        errors,
        "http2_max_header_list_size",
        &fields.http2_max_header_list_size,
        1024,
        1024 * 1024,
    )? as u32;
    let max_pending_accept_reset_streams = parse_limit(
        locale,
        errors,
        "http2_max_pending_accept_reset_streams",
        &fields.http2_max_pending_accept_reset_streams,
        1,
        65536,
    )? as usize;
    Some(Http2Limits {
        max_header_list_size,
        max_pending_accept_reset_streams,
    })
}

fn parse_http3_limits(
    locale: Locale,
    fields: &Fields,
    errors: &mut HashMap<String, String>,
) -> Option<Http3Limits> {
    let max_concurrent_bidi_streams = parse_limit(
        locale,
        errors,
        "http3_max_concurrent_bidi_streams",
        &fields.http3_max_concurrent_bidi_streams,
        1,
        100000,
    )? as u32;
    let max_concurrent_uni_streams = parse_limit(
        locale,
        errors,
        "http3_max_concurrent_uni_streams",
        &fields.http3_max_concurrent_uni_streams,
        1,
        100000,
    )? as u32;
    Some(Http3Limits {
        max_concurrent_bidi_streams,
        max_concurrent_uni_streams,
    })
}

fn parse_notifications(
    locale: Locale,
    fields: &Fields,
    errors: &mut HashMap<String, String>,
) -> Option<NotificationConfig> {
    let config = parse_part::<NotificationConfig>(
        errors,
        "cert_expiry_warning",
        locale.t("settings.invalid_duration"),
        json!({ "cert_expiry_warning": fields.cert_expiry_warning.trim() }),
    );
    let webhook = parse_webhook(locale, fields, errors);
    Some(NotificationConfig {
        webhook: webhook?,
        ..config?
    })
}

/// The webhook of the notifications. `Some(None)` when the URL is empty, and `None` when a field
/// is invalid.
fn parse_webhook(
    locale: Locale,
    fields: &Fields,
    errors: &mut HashMap<String, String>,
) -> Option<Option<WebhookConfig>> {
    let url = fields.webhook_url.trim();
    if url.is_empty() {
        return Some(None);
    }
    if !webhook_url_allowed(url) {
        errors.insert(
            "webhook_url".into(),
            locale.t("settings.webhook_url_invalid").into(),
        );
    }
    let timeout = fields.webhook_timeout.trim();
    let value = if timeout.is_empty() {
        json!({ "url": url })
    } else {
        json!({ "url": url, "timeout": timeout })
    };
    let mut webhook = parse_part::<WebhookConfig>(
        errors,
        "webhook_timeout",
        locale.t("settings.invalid_duration"),
        value,
    )?;
    webhook.token = secret_update(&fields.webhook_token, fields.webhook_clear_token);
    Some(Some(webhook))
}

/// Parses a required socket address, or records the error under `key`.
fn parse_addr(
    locale: Locale,
    errors: &mut HashMap<String, String>,
    key: &str,
    value: &str,
) -> Option<SocketAddr> {
    let addr = value.trim().parse::<SocketAddr>().ok();
    if addr.is_none() {
        invalid_addr(locale, errors, key);
    }
    addr
}

/// Parses an optional DNS server field, or records the error under `key`.
fn parse_resolver(
    locale: Locale,
    errors: &mut HashMap<String, String>,
    key: &str,
    value: &str,
) -> Result<Option<SocketAddr>, std::net::AddrParseError> {
    let resolver = parse_optional_addr(value);
    if resolver.is_err() {
        invalid_addr(locale, errors, key);
    }
    resolver
}

fn invalid_addr(locale: Locale, errors: &mut HashMap<String, String>, key: &str) {
    errors.insert(
        key.to_string(),
        locale.t("settings.invalid_socket_addr").into(),
    );
}

/// An empty value means that no address is set.
fn parse_optional_addr(value: &str) -> Result<Option<SocketAddr>, std::net::AddrParseError> {
    let value = value.trim();
    if value.is_empty() {
        return Ok(None);
    }
    value.parse().map(Some)
}

/// The public address of the server. An empty value means that no address is set.
fn parse_public_url(
    locale: Locale,
    errors: &mut HashMap<String, String>,
    value: &str,
) -> Option<Option<PublicUrl>> {
    let value = value.trim();
    if value.is_empty() {
        return Some(None);
    }
    match value.parse::<PublicUrl>() {
        Ok(url) => Some(Some(url)),
        Err(err) => {
            errors.insert("public_url".into(), locale.error_message(&err));
            None
        }
    }
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
    send_request(locale, request).await.map(|_| ())
}

/// Sends the request and returns a successful response. The error is the message of the failure
/// in the selected language.
pub async fn send_request(locale: Locale, request: Request) -> Result<Response, String> {
    let response = request.send().await.map_err(|err| err.to_string())?;
    if response.ok() {
        return Ok(response);
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
                        notice.set(Some(Notice::Success(
                            locale.t("settings.cdn_refreshed").to_string(),
                        )));
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
    match u64::try_from(unix_time) {
        Ok(time) => crate::format::format_time(time * 1000),
        Err(_) => unix_time.to_string(),
    }
}

async fn get_cdn_status() -> Result<CdnStatus, gloo_net::Error> {
    Request::get(&format!("{API_ENDPOINT}/cdn"))
        .send()
        .await?
        .json()
        .await
}

#[function_component(BannedIpsCard)]
fn banned_ips_card() -> Html {
    let locale = use_locale();
    let reload = use_state(|| 0);
    let list = use_state(Vec::<BannedIpEntry>::new);
    let notice = use_state(|| Option::<Notice>::None);

    let reload_val = *reload;
    let list_handle = list.clone();
    use_effect_with(reload_val, move |_| {
        wasm_bindgen_futures::spawn_local(async move {
            if let Ok(entries) = fetch_json::<Vec<BannedIpEntry>>(
                locale,
                Request::get(&format!("{API_ENDPOINT}/banned-ips")),
            )
            .await
            {
                list_handle.set(entries);
            }
        });
    });

    let unban = {
        let reload = reload.clone();
        let notice = notice.clone();
        Callback::from(move |ip: String| {
            let reload = reload.clone();
            let notice = notice.clone();
            let question = locale.tf("settings.unban_confirm", &[("ip", &ip)]);
            dialog::confirm_then(locale, question, async move {
                let req = Request::delete(&format!("{API_ENDPOINT}/banned-ips/{ip}"));
                if let Ok(built) = req.build() {
                    match send_request(locale, built).await {
                        Ok(_) => {
                            notice.set(Some(Notice::Success(
                                locale.t("settings.unbanned").to_string(),
                            )));
                            reload.set(*reload + 1);
                        }
                        Err(err) => notice.set(Some(Notice::Failed(err))),
                    }
                }
            });
        })
    };

    html! {
        <div class="mt-4 bg-white dark:bg-neutral-800 shadow-sm p-5 border border-neutral-300 dark:border-neutral-700 rounded-md">
            { notice_view(&notice) }
            <h2 class="text-lg font-semibold text-neutral-900 dark:text-neutral-200">{locale.t("settings.banned_ips")}</h2>
            <p class="mt-2 text-sm text-neutral-500 dark:text-neutral-400">{locale.t("settings.banned_ips_hint")}</p>
            if list.is_empty() {
                <p class="mt-4 text-sm text-neutral-400 dark:text-neutral-500 italic">{locale.t("settings.banned_ips_empty")}</p>
            } else {
                <div class="mt-4 overflow-x-auto">
                    <table class="w-full text-sm text-left text-neutral-500 dark:text-neutral-400">
                        <thead class="text-xs uppercase bg-neutral-50 dark:bg-neutral-750 text-neutral-700 dark:text-neutral-300">
                            <tr>
                                <th class="px-4 py-2">{"IP"}</th>
                                <th class="px-4 py-2">{locale.t("settings.failures_count")}</th>
                                <th class="px-4 py-2">{locale.t("settings.remaining_time")}</th>
                                <th class="px-4 py-2"></th>
                            </tr>
                        </thead>
                        <tbody>
                            { for list.iter().map(|entry| {
                                let ip_str = entry.ip.to_string();
                                let on_unban = {
                                    let unban = unban.clone();
                                    let ip_str = ip_str.clone();
                                    Callback::from(move |e: MouseEvent| {
                                        e.prevent_default();
                                        unban.emit(ip_str.clone());
                                    })
                                };
                                let mins = entry.remaining_seconds / 60;
                                let secs = entry.remaining_seconds % 60;
                                let time_str = format!("{mins}m {secs}s");
                                html! {
                                    <tr class="border-b dark:border-neutral-700 hover:bg-neutral-50 dark:hover:bg-neutral-750">
                                        <td class="px-4 py-2 font-mono font-medium text-neutral-900 dark:text-white">{entry.ip.to_string()}</td>
                                        <td class="px-4 py-2">{entry.failures}</td>
                                        <td class="px-4 py-2 font-mono text-amber-600 dark:text-amber-400">{time_str}</td>
                                        <td class="px-4 py-2 text-right">
                                            <button type="button" onclick={on_unban} class="text-xs text-red-600 hover:text-red-800 dark:text-red-400 font-medium">
                                                {locale.t("settings.unban")}
                                            </button>
                                        </td>
                                    </tr>
                                }
                            }) }
                        </tbody>
                    </table>
                </div>
            }
        </div>
    }
}

/// The card of the server upgrade: it installs the newest release and restarts the server
/// without dropping the connections. While the upgrade runs, a blocking dialog covers the
/// page and shows the step that runs at the moment.
#[function_component(UpgradeCard)]
fn upgrade_card() -> Html {
    let locale = use_locale();
    let notice = use_state(|| Option::<Notice>::None);
    let is_running = use_state(|| false);
    let stage = use_state(|| Option::<UpgradeStage>::None);

    let onclick = {
        let notice = notice.clone();
        let is_running = is_running.clone();
        let stage = stage.clone();
        Callback::from(move |_: MouseEvent| {
            if *is_running {
                return;
            }
            notice.set(None);
            stage.set(Some(UpgradeStage::CheckingRelease));
            is_running.set(true);
            let notice = notice.clone();
            let task_running = is_running.clone();
            let task_stage = stage.clone();
            wasm_bindgen_futures::spawn_local(async move {
                match upgrade_server(locale).await {
                    Ok(started) => {
                        notice.set(Some(Notice::Success(
                            locale.tf("settings.upgrade_started", &[("tag", started.tag.as_str())]),
                        )));
                        task_stage.set(Some(UpgradeStage::Done));
                        // The old process drains and the new one serves, so the panel loads
                        // the bundle of the new version after a short wait.
                        gloo_timers::future::TimeoutFuture::new(2000).await;
                        if let Some(window) = web_sys::window() {
                            let _ = window.location().reload();
                        }
                    }
                    Err(message) => {
                        notice.set(Some(Notice::Failed(message)));
                        task_stage.set(None);
                        task_running.set(false);
                    }
                }
            });
            let poll_stage = stage.clone();
            let poll_running = is_running.clone();
            wasm_bindgen_futures::spawn_local(async move {
                loop {
                    gloo_timers::future::TimeoutFuture::new(1000).await;
                    if !*poll_running {
                        return;
                    }
                    if let Ok(status) = upgrade_status().await {
                        poll_stage.set(status.stage);
                    }
                }
            });
        })
    };

    html! {
        <div class="mt-4 bg-white dark:bg-neutral-800 shadow-sm p-5 border border-neutral-300 dark:border-neutral-700 rounded-md">
            { notice_view(&notice) }
            <h2 class="text-lg font-semibold text-neutral-900 dark:text-neutral-200">{locale.t("settings.upgrade_title")}</h2>
            <p class="mt-2 text-sm text-neutral-500 dark:text-neutral-400">{locale.t("settings.upgrade_hint")}</p>
            <div class="flex flex-col mt-4 sm:flex-row sm:items-center sm:justify-end">
                <button type="button" {onclick} disabled={*is_running} class="inline-flex justify-center items-center text-neutral-500 bg-neutral-50 dark:text-neutral-200 dark:bg-neutral-800 border border-neutral-300 dark:border-neutral-600 focus:outline-none hover:bg-neutral-100 hover:dark:bg-neutral-900 focus:ring-4 focus:ring-neutral-200 dark:focus:ring-neutral-600 font-medium rounded-lg text-sm px-4 py-2">
                    {locale.t("settings.upgrade_button")}
                </button>
            </div>
            if *is_running {
                { upgrade_dialog(locale, *stage) }
            }
        </div>
    }
}

/// The blocking dialog of a running upgrade: it covers the page until the panel reloads.
fn upgrade_dialog(locale: Locale, stage: Option<UpgradeStage>) -> Html {
    let message = match stage {
        Some(step) => locale.t(step_label(step)).to_string(),
        None => locale.t("settings.upgrade_dialog_wait").to_string(),
    };
    html! {
        <div class="fixed inset-0 z-50 bg-black/50 flex items-center justify-center">
            <div class="bg-white dark:bg-neutral-800 p-6 border border-neutral-300 dark:border-neutral-700 rounded-md shadow-lg text-center">
                <div class="mx-auto h-10 w-10 border-4 border-neutral-300 dark:border-neutral-600 border-t-blue-600 rounded-full animate-spin" role="status"></div>
                <h3 class="mt-4 text-lg font-semibold text-neutral-900 dark:text-neutral-200">{locale.t("settings.upgrade_dialog_title")}</h3>
                <p class="mt-2 text-sm text-neutral-500 dark:text-neutral-400">{message}</p>
            </div>
        </div>
    }
}

/// The locale key of the label of an upgrade step.
fn step_label(step: UpgradeStage) -> &'static str {
    match step {
        UpgradeStage::CheckingRelease => "settings.upgrade_stage_checking_release",
        UpgradeStage::Downloading => "settings.upgrade_stage_downloading",
        UpgradeStage::Installing => "settings.upgrade_stage_installing",
        UpgradeStage::HandingOver => "settings.upgrade_stage_handing_over",
        UpgradeStage::Done => "settings.upgrade_stage_done",
    }
}

async fn refresh_cdn_ranges(locale: Locale) -> Result<CdnStatus, String> {
    fetch_json(
        locale,
        Request::post(&format!("{API_ENDPOINT}/cdn/refresh")),
    )
    .await
}

/// Asks the server to install the newest release and hand over to it.
async fn upgrade_server(locale: Locale) -> Result<UpgradeStarted, String> {
    fetch_json(locale, Request::post(&format!("{API_ENDPOINT}/upgrade"))).await
}

/// Reads the step of the upgrade that runs at the moment.
async fn upgrade_status() -> Result<UpgradeStageResponse, gloo_net::Error> {
    Request::get(&format!("{API_ENDPOINT}/upgrade/status"))
        .send()
        .await?
        .json()
        .await
}

/// Sends a request without a body and reads the JSON of the response. A failed request gives the
/// message of the admin API error.
pub async fn fetch_json<T: DeserializeOwned>(
    locale: Locale,
    request: gloo_net::http::RequestBuilder,
) -> Result<T, String> {
    let request = request.build().map_err(|err| err.to_string())?;
    send_request(locale, request)
        .await?
        .json()
        .await
        .map_err(|err| err.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_upgrade_check_interval_round_trips_and_rejects_an_invalid_duration() {
        let mut fields = Fields::from_config(&AppConfig::default());
        fields.upgrade_check_interval = "90m".into();
        let parsed = parse_fields(Locale::En, &fields).unwrap();
        assert_eq!(
            parsed.upgrade.check_interval,
            std::time::Duration::from_secs(90 * 60)
        );

        fields.upgrade_check_interval = "soon".into();
        let errors = parse_fields(Locale::En, &fields).unwrap_err();
        assert_eq!(
            errors.get("upgrade_check_interval").map(String::as_str),
            Some(Locale::En.t("settings.invalid_duration"))
        );
    }

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

    #[test]
    fn the_webhook_needs_a_url_and_its_token_is_sent_only_when_it_changes() {
        let mut config = AppConfig::default();
        config.notifications.webhook = Some(WebhookConfig {
            url: "https://hooks.example.com/".into(),
            token_set: true,
            timeout: std::time::Duration::from_secs(5),
            ..Default::default()
        });
        let fields = Fields::from_config(&config);
        let parsed = parse_fields(Locale::En, &fields).unwrap();
        if let Some(webhook) = &mut config.notifications.webhook {
            webhook.token_set = false;
        }
        assert_eq!(parsed, config);

        let typed = Fields {
            webhook_token: " new-token ".into(),
            ..fields.clone()
        };
        let webhook = parse_fields(Locale::En, &typed)
            .unwrap()
            .notifications
            .webhook;
        assert_eq!(webhook.and_then(|w| w.token).as_deref(), Some("new-token"));

        let removed = Fields {
            webhook_url: " ".into(),
            ..fields.clone()
        };
        let parsed = parse_fields(Locale::En, &removed).unwrap();
        assert_eq!(parsed.notifications.webhook, None);

        let invalid = Fields {
            webhook_url: "http://hooks.example.com/".into(),
            webhook_timeout: "soon".into(),
            ..fields
        };
        let errors = parse_fields(Locale::En, &invalid).unwrap_err();
        assert_eq!(
            errors.get("webhook_url").map(String::as_str),
            Some(Locale::En.t("settings.webhook_url_invalid"))
        );
        assert!(errors.contains_key("webhook_timeout"));
    }

    #[test]
    fn etcd_settings_round_trip_and_the_credentials_are_checked() {
        let mut config = AppConfig::default();
        config.discovery.etcd = EtcdDiscoveryConfig {
            enabled: true,
            endpoints: vec![
                "http://10.0.0.1:2379".into(),
                "https://10.0.0.2:2379".into(),
            ],
            username: "r3v3rs3".into(),
            password_set: true,
            prefix: "apps".into(),
            ..Default::default()
        };
        let fields = Fields::from_config(&config);
        let parsed = parse_fields(Locale::En, &fields).unwrap();
        config.discovery.etcd.password_set = false;
        assert_eq!(parsed, config);

        let spaced = Fields {
            etcd_endpoints: " http://a:2379 , ,http://b:2379".into(),
            etcd_password: "new".into(),
            ..fields.clone()
        };
        let parsed = parse_fields(Locale::En, &spaced).unwrap();
        assert_eq!(
            parsed.discovery.etcd.endpoints,
            ["http://a:2379", "http://b:2379"]
        );
        assert_eq!(parsed.discovery.etcd.password.as_deref(), Some("new"));

        let error_of = |fields: &Fields, key: &str| {
            parse_fields(Locale::En, fields)
                .unwrap_err()
                .get(key)
                .cloned()
        };
        let cleared = Fields {
            etcd_clear_password: true,
            ..fields.clone()
        };
        assert_eq!(
            error_of(&cleared, "etcd_username").as_deref(),
            Some(Locale::En.t("settings.etcd_password_required"))
        );
        let without_user = Fields {
            etcd_username: " ".into(),
            ..fields.clone()
        };
        assert_eq!(
            error_of(&without_user, "etcd_username").as_deref(),
            Some(Locale::En.t("settings.etcd_username_required"))
        );
        let invalid = Fields {
            etcd_endpoints: "10.0.0.1:2379".into(),
            ..fields
        };
        assert_eq!(
            error_of(&invalid, "etcd_endpoints").as_deref(),
            Some(Locale::En.t("settings.invalid_endpoint"))
        );
    }
}
