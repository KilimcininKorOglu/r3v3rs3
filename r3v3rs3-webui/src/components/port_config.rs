use super::http_proxy_config::{
    HINT_CLASS, INPUT_CLASS, LABEL_CLASS, checked, error_view, format_seconds, parse_comma_list,
    parse_seconds, seconds_input, select_field, select_setter, state_input, text, toggle,
};
use crate::API_ENDPOINT;
use crate::i18n::use_locale;
use crate::pages::cert_list::get_cert_list;
use gloo_net::http::Request;
use r3v3rs3_api::{
    app::{Http2Limits, Http3Limits},
    cert::{CertInfo, CertKind},
    cidr::{format_cidr_list, parse_cidr_list},
    error::Error,
    i18n::Locale,
    id::ShortId,
    multiaddr::Multiaddr,
    port::{NetworkInterface, Port, PortOptions},
    proxy_protocol::{ProxyProtocolAccept, ProxyProtocolReceive},
    subject_name::SubjectName,
    tls::{ClientAuthMode, TlsTermination},
};
use std::{
    collections::HashMap,
    net::{Ipv4Addr, Ipv6Addr},
    str::FromStr,
};
use wasm_bindgen::{JsCast, UnwrapThrowExt};
use web_sys::{HtmlInputElement, HtmlSelectElement};
use yew::prelude::*;

#[derive(Properties, PartialEq)]
pub struct Props {
    #[prop_or_else(create_default_port)]
    pub port: Port,
    pub onchanged: Callback<Result<Port, HashMap<String, String>>>,
}

fn create_default_port() -> Port {
    Port {
        active: true,
        name: String::new(),
        listen: "/ip4/0.0.0.0/tcp/8080/http".parse().unwrap(),
        opts: Default::default(),
    }
}

const PROTOCOLS: &[(&str, &str)] = &[
    ("http", "HTTP"),
    ("https", "HTTPS"),
    ("http3", "HTTP over QUIC (HTTP/3)"),
    ("tcp", "TCP"),
    ("tls", "TCP over TLS"),
    ("udp", "UDP"),
];

const CLIENT_AUTH_MODES: [(ClientAuthMode, &str, &str); 3] = [
    (ClientAuthMode::Off, "off", "ports.client_auth_off"),
    (
        ClientAuthMode::Optional,
        "optional",
        "ports.client_auth_optional",
    ),
    (
        ClientAuthMode::Required,
        "required",
        "ports.client_auth_required",
    ),
];

const PROXY_PROTOCOL_ACCEPT: [(ProxyProtocolAccept, &str, &str); 3] = [
    (ProxyProtocolAccept::Any, "any", "ports.proxy_protocol_any"),
    (ProxyProtocolAccept::V1, "v1", "ports.proxy_protocol_v1"),
    (ProxyProtocolAccept::V2, "v2", "ports.proxy_protocol_v2"),
];

/// Protocol names are the same in every language, except the ones that contain words.
fn protocol_label(locale: Locale, value: &str, label: &'static str) -> &'static str {
    match value {
        "http3" => locale.t("protocol.http3"),
        "tls" => locale.t("protocol.tls"),
        _ => label,
    }
}

fn is_tls_protocol(protocol: &str) -> bool {
    matches!(protocol, "tls" | "https" | "http3")
}

/// UDP and QUIC ports cannot read a PROXY protocol header.
fn supports_proxy_protocol(protocol: &str) -> bool {
    matches!(protocol, "tcp" | "tls" | "http" | "https")
}

/// HTTP ports speak HTTP/2 on TCP, TLS and QUIC.
fn supports_http2(protocol: &str) -> bool {
    matches!(protocol, "http" | "https" | "http3")
}

/// Only a QUIC port speaks HTTP/3.
fn supports_http3(protocol: &str) -> bool {
    protocol == "http3"
}

#[derive(Clone, PartialEq)]
struct PortForm {
    active: bool,
    name: String,
    protocol: String,
    interface: String,
    port: u16,
    server_names: String,
    client_auth: ClientAuthMode,
    client_ca_certs: Vec<ShortId>,
    proxy_protocol: ProxyProtocolForm,
    http2_limits: Http2LimitsForm,
    http3_limits: Http3LimitsForm,
}

#[derive(Clone, PartialEq)]
struct Http2LimitsForm {
    enabled: bool,
    max_header_list_size: String,
    max_pending_accept_reset_streams: String,
}

impl Http2LimitsForm {
    fn new(config: Option<&Http2Limits>) -> Self {
        let default = Http2Limits::default();
        let saved = config.unwrap_or(&default);
        Self {
            enabled: config.is_some(),
            max_header_list_size: saved.max_header_list_size.to_string(),
            max_pending_accept_reset_streams: saved.max_pending_accept_reset_streams.to_string(),
        }
    }
}

#[derive(Clone, PartialEq)]
struct Http3LimitsForm {
    enabled: bool,
    max_concurrent_bidi_streams: String,
    max_concurrent_uni_streams: String,
}

impl Http3LimitsForm {
    fn new(config: Option<&Http3Limits>) -> Self {
        let default = Http3Limits::default();
        let saved = config.unwrap_or(&default);
        Self {
            enabled: config.is_some(),
            max_concurrent_bidi_streams: saved.max_concurrent_bidi_streams.to_string(),
            max_concurrent_uni_streams: saved.max_concurrent_uni_streams.to_string(),
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
struct ProxyProtocolForm {
    enabled: bool,
    accept: ProxyProtocolAccept,
    trusted: String,
    timeout: String,
}

impl ProxyProtocolForm {
    fn new(config: Option<&ProxyProtocolReceive>) -> Self {
        let default = ProxyProtocolReceive::default();
        let saved = config.unwrap_or(&default);
        Self {
            enabled: config.is_some(),
            accept: saved.accept,
            trusted: format_cidr_list(&saved.trusted),
            timeout: format_seconds(saved.timeout),
        }
    }
}

/// The state of the TLS section of the form.
struct TlsFields {
    server_names: UseStateHandle<String>,
    client_auth: UseStateHandle<ClientAuthMode>,
    client_ca_certs: UseStateHandle<Vec<ShortId>>,
    root_certs: UseStateHandle<Vec<CertInfo>>,
}

/// Every local IP address, plus the two wildcard addresses and the address of the port.
#[hook]
fn use_interfaces(interface: &str) -> UseStateHandle<Vec<String>> {
    let interfaces = use_state(|| vec![interface.to_string()]);
    let interfaces_cloned = interfaces.clone();
    let interface = interface.to_string();
    use_effect_with((), move |_| {
        wasm_bindgen_futures::spawn_local(async move {
            if let Ok(entry) = get_interfaces().await {
                let mut list = vec!["0.0.0.0".into(), "::".into()]
                    .into_iter()
                    .chain(
                        entry
                            .into_iter()
                            .flat_map(|ifs| ifs.addrs)
                            .map(|addr| addr.ip.to_string()),
                    )
                    .collect::<Vec<_>>();
                if !list.contains(&interface) {
                    list.push(interface);
                }
                interfaces_cloned.set(list);
            }
        });
    });
    interfaces
}

/// The protocol name of the listen address, as the form shows it.
fn protocol_name(udp: bool, tls: bool, http: bool, quic: bool) -> &'static str {
    match (udp, tls, http, quic) {
        (_, _, true, true) => "http3",
        (true, _, _, _) => "udp",
        (false, true, true, _) => "https",
        (false, true, false, _) => "tls",
        (false, false, true, _) => "http",
        (false, false, false, _) => "tcp",
    }
}

#[function_component(PortConfig)]
pub fn port_config(props: &Props) -> Html {
    let locale = use_locale();
    let stack = &props.port.listen;
    let tls = stack.is_tls();
    let http = stack.is_http();
    let udp = stack.is_udp();
    let quic = stack.is_quic();
    let interface = stack.host().unwrap();
    let port = stack.port().unwrap();

    let active = use_state(|| props.port.active);
    let active_onchange = checkbox_onchange(active.clone());

    let interfaces = use_interfaces(&interface);
    let protocol = protocol_name(udp, tls, http, quic);

    let name = use_state(|| props.port.name.clone());
    let name_onchange = input_onchange(name.clone());

    let protocol = use_state(|| protocol.to_string());
    let protocol_onchange = select_onchange(protocol.clone());

    let interface = use_state(|| interface);
    let interface_onchange = select_onchange(interface.clone());

    let port = use_state(|| port);
    let port_onchange = port_onchange(port.clone());

    let tls_termination = props.port.opts.tls_termination.clone().unwrap_or_default();
    let tls_fields = use_tls_fields(&tls_termination);
    let proxy_protocol =
        use_state(|| ProxyProtocolForm::new(props.port.opts.proxy_protocol.as_ref()));
    let http2_limits = use_state(|| Http2LimitsForm::new(props.port.opts.http2_limits.as_ref()));
    let http3_limits = use_state(|| Http3LimitsForm::new(props.port.opts.http3_limits.as_ref()));

    let form = PortForm {
        active: *active,
        name: name.to_string(),
        protocol: protocol.to_string(),
        interface: interface.to_string(),
        port: *port,
        server_names: tls_fields.server_names.to_string(),
        client_auth: *tls_fields.client_auth,
        client_ca_certs: (*tls_fields.client_ca_certs).clone(),
        proxy_protocol: (*proxy_protocol).clone(),
        http2_limits: (*http2_limits).clone(),
        http3_limits: (*http3_limits).clone(),
    };
    let prev_entry =
        use_state::<Result<Port, HashMap<String, String>>, _>(|| Err(Default::default()));
    let entry = get_port(locale, &form, &tls_termination);
    emit_change(&props.onchanged, &prev_entry, &entry);
    let errors = entry.err().unwrap_or_default();

    html! {
        <>
            <label class="relative inline-flex items-center cursor-pointer mb-6">
                <input onchange={active_onchange} type="checkbox" checked={*active} class="sr-only peer" />
                <div class="shrink-0 w-9 h-5 bg-neutral-200 dark:bg-neutral-600 peer-focus:outline-none peer-focus:ring-4 peer-focus:ring-blue-300 rounded-full peer peer-checked:after:translate-x-full peer-checked:after:border-white after:content-[''] after:absolute after:top-[2px] after:left-[2px] after:bg-white after:border-neutral-300 after:border after:rounded-full after:h-4 after:w-4 after:transition-all peer-checked:bg-blue-600"></div>
                <span class="ml-3 text-sm font-medium text-neutral-900 dark:text-neutral-200">{locale.t("common.active")}</span>
            </label>

            <label class="block mb-2 text-sm font-medium text-neutral-900 dark:text-neutral-200">{locale.t("common.friendly_name")}</label>
            <input type="text" value={name.to_string()} onchange={name_onchange} class="bg-neutral-50 dark:text-neutral-200 dark:bg-neutral-800 dark:border-neutral-600 border border-neutral-300 text-neutral-900 text-sm rounded-lg focus:ring-blue-500 focus:border-blue-500 block w-full p-2.5" placeholder={locale.t("common.friendly_name_placeholder")} />

            <label class="block mt-4 mb-2 text-sm font-medium text-neutral-900 dark:text-neutral-200">{locale.t("ports.interface")}</label>
            <select onchange={interface_onchange} class="bg-neutral-50 dark:text-neutral-200 dark:bg-neutral-800 dark:border-neutral-600 border border-neutral-300 text-neutral-900 text-sm rounded-lg focus:ring-blue-500 focus:border-blue-500 block w-full p-2.5">
                { interface_options(&interfaces, &interface) }
            </select>
            { error_view(errors.get("interface")) }

            <label class="block mt-4 mb-2 text-sm font-medium text-neutral-900 dark:text-neutral-200">{locale.t("common.port")}</label>
            <input type="number" placeholder="8080" onchange={port_onchange} value={port.to_string()} max="65535" min="1" class="bg-neutral-50 dark:text-neutral-200 dark:bg-neutral-800 dark:border-neutral-600 border border-neutral-300 text-neutral-900 text-sm rounded-lg focus:ring-blue-500 focus:border-blue-500 block w-full p-2.5" />

            <label class="block mt-4 mb-2 text-sm font-medium text-neutral-900 dark:text-neutral-200">{locale.t("common.protocol")}</label>
            <select onchange={protocol_onchange} class="bg-neutral-50 dark:text-neutral-200 dark:bg-neutral-800 dark:border-neutral-600 border border-neutral-300 text-neutral-900 text-sm rounded-lg focus:ring-blue-500 focus:border-blue-500 block w-full p-2.5">
                { protocol_options(locale, &protocol) }
            </select>

            { protocol_view(
                locale,
                &protocol,
                &tls_fields,
                &proxy_protocol,
                &http2_limits,
                &http3_limits,
                &errors,
            ) }
        </>
    }
}

fn accept_option(event: &Event) -> ProxyProtocolAccept {
    let target: HtmlSelectElement = event.target().unwrap_throw().dyn_into().unwrap_throw();
    find_option(&PROXY_PROTOCOL_ACCEPT, &target.value())
}

fn proxy_protocol_view(
    locale: Locale,
    state: &UseStateHandle<ProxyProtocolForm>,
    errors: &HashMap<String, String>,
) -> Html {
    let enabled_onchange = state_input(state, checked, |form, enabled| form.enabled = enabled);
    html! {
        <>
            <div class="mt-6">
                { toggle(enabled_onchange, state.enabled, locale.t("ports.proxy_protocol"), "") }
            </div>
            <p class={HINT_CLASS}>{locale.t("ports.proxy_protocol_hint")}</p>
            if state.enabled {
                { proxy_protocol_fields_view(locale, state, errors) }
            }
        </>
    }
}

fn proxy_protocol_fields_view(
    locale: Locale,
    state: &UseStateHandle<ProxyProtocolForm>,
    errors: &HashMap<String, String>,
) -> Html {
    let accept_onchange = state_input(state, accept_option, |form, accept| form.accept = accept);
    let trusted_onchange = state_input(state, text, |form, trusted| form.trusted = trusted);
    let timeout_onchange = state_input(state, text, |form, timeout| form.timeout = timeout);
    html! {
        <>
            <label class={LABEL_CLASS}>{locale.t("ports.proxy_protocol_trusted")}</label>
            <input type="text" autocapitalize="off" value={state.trusted.clone()} onchange={trusted_onchange} class={INPUT_CLASS} placeholder="10.0.0.0/8, 192.168.1.10" />
            { error_view(errors.get("proxy_protocol_trusted")) }
            <p class={HINT_CLASS}>{locale.t("ports.proxy_protocol_trusted_hint")}</p>

            { select_field(
                locale.t("ports.proxy_protocol_accept"),
                accept_onchange,
                option_list(locale, &PROXY_PROTOCOL_ACCEPT, state.accept),
                None,
            ) }

            { seconds_input(locale.t("ports.proxy_protocol_timeout"), &state.timeout, 1, timeout_onchange) }
            { error_view(errors.get("proxy_protocol_timeout")) }
        </>
    }
}

/// The options of a select field. Each option is a value, its form name and its locale key.
pub(super) fn option_list<T: Copy + PartialEq>(
    locale: Locale,
    options: &[(T, &str, &str)],
    selected: T,
) -> Html {
    options
        .iter()
        .map(|(value, name, key)| {
            html! {
                <option selected={*value == selected} value={name.to_string()}>{locale.t(key)}</option>
            }
        })
        .collect()
}

/// The value of the option with the form name. The default value for an unknown name.
pub(super) fn find_option<T: Copy + Default>(options: &[(T, &str, &str)], name: &str) -> T {
    options
        .iter()
        .find(|(_, option, _)| *option == name)
        .map(|(value, _, _)| *value)
        .unwrap_or_default()
}

/// Returns the state of the TLS section from the saved TLS config and loads the root certificates.
#[hook]
fn use_tls_fields(tls_termination: &TlsTermination) -> TlsFields {
    let fields = TlsFields {
        server_names: use_state(|| tls_termination.server_names.join(", ")),
        client_auth: use_state(|| tls_termination.client_auth),
        client_ca_certs: use_state(|| tls_termination.client_ca_certs.clone()),
        root_certs: use_state(Vec::<CertInfo>::new),
    };
    use_effect_with((), {
        let root_certs = fields.root_certs.clone();
        move |_| {
            wasm_bindgen_futures::spawn_local(async move {
                if let Ok(list) = get_cert_list().await {
                    root_certs.set(
                        list.into_iter()
                            .filter(|cert| cert.kind == CertKind::Root)
                            .collect(),
                    );
                }
            });
        }
    });
    fields
}

/// The server names and the client authentication of a TLS port.
fn tls_view(locale: Locale, fields: &TlsFields, errors: &HashMap<String, String>) -> Html {
    let server_names_onchange = Callback::from({
        let server_names = fields.server_names.clone();
        move |event: Event| {
            let target: HtmlInputElement = event.target().unwrap_throw().dyn_into().unwrap_throw();
            server_names.set(target.value());
        }
    });
    html! {
        <>
            <label class={LABEL_CLASS}>{locale.t("ports.server_names")}</label>
            <input type="text" autocapitalize="off" value={fields.server_names.to_string()} onchange={server_names_onchange} class={INPUT_CLASS} placeholder="example.com, *.example.com" />
            { error_view(errors.get("server_names")) }
            <p class={HINT_CLASS}>{locale.t("ports.server_names_hint")}</p>

            { client_auth_view(locale, &fields.client_auth) }
            if !fields.client_auth.is_off() {
                { client_ca_certs_view(locale, &fields.client_ca_certs, &fields.root_certs) }
                { error_view(errors.get("client_ca_certs")) }
            }
        </>
    }
}

fn client_auth_view(locale: Locale, client_auth: &UseStateHandle<ClientAuthMode>) -> Html {
    select_field(
        locale.t("ports.client_auth"),
        select_setter(client_auth, parse_client_auth),
        option_list(locale, &CLIENT_AUTH_MODES, **client_auth),
        Some(locale.t("ports.client_auth_hint")),
    )
}

fn client_ca_certs_view(
    locale: Locale,
    selected: &UseStateHandle<Vec<ShortId>>,
    root_certs: &[CertInfo],
) -> Html {
    html! {
        <>
            <label class={LABEL_CLASS}>{locale.t("ports.client_ca_certs")}</label>
            if root_certs.is_empty() {
                <p class={HINT_CLASS}>{locale.t("ports.no_root_certs")}</p>
            }
            { root_certs.iter().map(|cert| {
                let id = cert.id;
                let onchange = Callback::from({
                    let selected = selected.clone();
                    move |event: Event| {
                        let target: HtmlInputElement = event.target().unwrap_throw().dyn_into().unwrap_throw();
                        selected.set(toggle_id(&selected, id, target.checked()));
                    }
                });
                html! {
                    <label class="flex items-center gap-2 mb-1 text-sm text-neutral-900 dark:text-neutral-200">
                        <input type="checkbox" checked={selected.contains(&id)} {onchange} class="w-4 h-4" />
                        <span>{format!("{} ({})", cert.issuer, id)}</span>
                    </label>
                }
            }).collect::<Html>() }
            <p class={HINT_CLASS}>{locale.t("ports.client_ca_certs_hint")}</p>
        </>
    }
}

fn parse_client_auth(value: &str) -> ClientAuthMode {
    find_option(&CLIENT_AUTH_MODES, value)
}

/// Adds the ID to the list or removes it from the list.
fn toggle_id(list: &[ShortId], id: ShortId, checked: bool) -> Vec<ShortId> {
    let mut list = list
        .iter()
        .copied()
        .filter(|item| *item != id)
        .collect::<Vec<_>>();
    if checked {
        list.push(id);
    }
    list
}

/// Builds the port from the form. `tls_termination` is the saved TLS config; the form replaces only its fields.
fn get_port(
    locale: Locale,
    form: &PortForm,
    tls_termination: &TlsTermination,
) -> Result<Port, HashMap<String, String>> {
    let mut errors = HashMap::new();
    let listen = listen_addr(locale, form, &mut errors);
    let mut tls_termination = tls_termination.clone();
    tls_termination.server_names = parse_comma_list(
        locale,
        &form.server_names,
        "server_names",
        &mut errors,
        |name| SubjectName::from_str(name).map(|_| name.to_string()),
    );
    tls_termination.client_auth = form.client_auth;
    tls_termination.client_ca_certs = client_ca_certs(locale, form, &mut errors);
    let proxy_protocol = proxy_protocol(locale, form, &mut errors);
    let http2_limits = http2_limits(locale, form, &mut errors);
    let http3_limits = http3_limits(locale, form, &mut errors);

    match listen {
        Some(listen) if errors.is_empty() => Ok(Port {
            active: form.active,
            name: form.name.trim().to_string(),
            listen,
            opts: PortOptions {
                tls_termination: is_tls_protocol(&form.protocol).then_some(tls_termination),
                proxy_protocol,
                http2_limits,
                http3_limits,
            },
        }),
        _ => Err(errors),
    }
}

/// Returns the HTTP/2 limit override of an enabled section on a port that speaks HTTP/2.
fn http2_limits(
    locale: Locale,
    form: &PortForm,
    errors: &mut HashMap<String, String>,
) -> Option<Http2Limits> {
    let section = &form.http2_limits;
    if !section.enabled || !supports_http2(&form.protocol) {
        return None;
    }
    let max_header_list_size = parse_limit_field(
        locale,
        errors,
        "http2_max_header_list_size",
        &section.max_header_list_size,
        1024,
        1024 * 1024,
    )? as u32;
    let max_pending_accept_reset_streams = parse_limit_field(
        locale,
        errors,
        "http2_max_pending_accept_reset_streams",
        &section.max_pending_accept_reset_streams,
        1,
        65536,
    )? as usize;
    Some(Http2Limits {
        max_header_list_size,
        max_pending_accept_reset_streams,
    })
}

/// Returns the HTTP/3 limit override of an enabled section on a QUIC port.
fn http3_limits(
    locale: Locale,
    form: &PortForm,
    errors: &mut HashMap<String, String>,
) -> Option<Http3Limits> {
    let section = &form.http3_limits;
    if !section.enabled || !supports_http3(&form.protocol) {
        return None;
    }
    let max_concurrent_bidi_streams = parse_limit_field(
        locale,
        errors,
        "http3_max_concurrent_bidi_streams",
        &section.max_concurrent_bidi_streams,
        1,
        100000,
    )? as u32;
    let max_concurrent_uni_streams = parse_limit_field(
        locale,
        errors,
        "http3_max_concurrent_uni_streams",
        &section.max_concurrent_uni_streams,
        1,
        100000,
    )? as u32;
    Some(Http3Limits {
        max_concurrent_bidi_streams,
        max_concurrent_uni_streams,
    })
}

/// Parses one limit field with its range. `None` marks an invalid value with the error set.
fn parse_limit_field(
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

/// Returns the PROXY protocol config of an enabled section on a port that supports it.
fn proxy_protocol(
    locale: Locale,
    form: &PortForm,
    errors: &mut HashMap<String, String>,
) -> Option<ProxyProtocolReceive> {
    let section = &form.proxy_protocol;
    if !section.enabled || !supports_proxy_protocol(&form.protocol) {
        return None;
    }
    let trusted = parse_cidr_list(&section.trusted)
        .and_then(|trusted| {
            if trusted.is_empty() {
                Err(Error::ProxyProtocolTrustedMissing)
            } else {
                Ok(trusted)
            }
        })
        .unwrap_or_else(|err| {
            errors.insert("proxy_protocol_trusted".into(), locale.error_message(&err));
            Vec::new()
        });
    let timeout = parse_seconds(
        locale,
        &section.timeout,
        "ports.proxy_protocol_timeout_name",
        1,
    )
    .unwrap_or_else(|err| {
        errors.insert("proxy_protocol_timeout".into(), err);
        Default::default()
    });
    Some(ProxyProtocolReceive {
        accept: section.accept,
        trusted,
        timeout,
    })
}

/// Returns the selected root certificates. `Off` keeps no certificate.
fn client_ca_certs(
    locale: Locale,
    form: &PortForm,
    errors: &mut HashMap<String, String>,
) -> Vec<ShortId> {
    if form.client_auth.is_off() || !is_tls_protocol(&form.protocol) {
        return Vec::new();
    }
    if form.client_ca_certs.is_empty() {
        errors.insert(
            "client_ca_certs".into(),
            locale.error_message(&Error::ClientCaCertsMissing),
        );
    }
    form.client_ca_certs.clone()
}

fn listen_addr(
    locale: Locale,
    form: &PortForm,
    errors: &mut HashMap<String, String>,
) -> Option<Multiaddr> {
    let interface = form.interface.trim();
    let host = if interface.is_empty() {
        errors.insert(
            "interface".into(),
            locale.t("ports.interface_required").into(),
        );
        return None;
    } else if let Ok(ip) = interface.parse::<Ipv4Addr>() {
        format!("/ip4/{ip}")
    } else if let Ok(ip) = interface.parse::<Ipv6Addr>() {
        format!("/ip6/{ip}")
    } else {
        format!("/dns/{interface}")
    };

    let port = form.port;
    let transport = match form.protocol.as_str() {
        "tcp" => format!("/tcp/{port}"),
        "tls" => format!("/tcp/{port}/tls"),
        "http" => format!("/tcp/{port}/http"),
        "https" => format!("/tcp/{port}/https"),
        "udp" => format!("/udp/{port}"),
        "http3" => format!("/udp/{port}/quic/http"),
        _ => String::new(),
    };

    let parsed = format!("{host}{transport}")
        .parse()
        .ok()
        .filter(|_| !transport.is_empty());
    if parsed.is_none() {
        errors.insert("interface".into(), locale.t("ports.invalid_address").into());
    }
    parsed
}

async fn get_interfaces() -> Result<Vec<NetworkInterface>, gloo_net::Error> {
    Request::get(&format!("{API_ENDPOINT}/ports/interfaces"))
        .send()
        .await?
        .json()
        .await
}

fn interface_options(interfaces: &[String], selected: &str) -> Html {
    interfaces
        .iter()
        .map(|value| {
            html! {
                <option selected={selected == value} value={value.clone()}>{value}</option>
            }
        })
        .collect()
}

fn protocol_options(locale: Locale, selected: &str) -> Html {
    PROTOCOLS
        .iter()
        .map(|(value, label)| {
            html! {
                <option selected={selected == *value} value={*value}>{protocol_label(locale, value, label)}</option>
            }
        })
        .collect()
}

/// Emits the entry when it differs from the entry of the last render.
fn emit_change(
    onchanged: &Callback<Result<Port, HashMap<String, String>>>,
    prev_entry: &UseStateHandle<Result<Port, HashMap<String, String>>>,
    entry: &Result<Port, HashMap<String, String>>,
) {
    if *entry != **prev_entry {
        prev_entry.set(entry.clone());
        onchanged.emit(entry.clone());
    }
}

/// The fields of the protocol: the TLS fields, the PROXY protocol fields and the limit overrides.
fn protocol_view(
    locale: Locale,
    protocol: &str,
    tls_fields: &TlsFields,
    proxy_protocol: &UseStateHandle<ProxyProtocolForm>,
    http2_limits: &UseStateHandle<Http2LimitsForm>,
    http3_limits: &UseStateHandle<Http3LimitsForm>,
    errors: &HashMap<String, String>,
) -> Html {
    html! {
        <>
            if is_tls_protocol(protocol) {
                { tls_view(locale, tls_fields, errors) }
            }
            if supports_proxy_protocol(protocol) {
                { proxy_protocol_view(locale, proxy_protocol, errors) }
            }
            if supports_http2(protocol) {
                { http2_limits_view(locale, http2_limits, errors) }
            }
            if supports_http3(protocol) {
                { http3_limits_view(locale, http3_limits, errors) }
            }
        </>
    }
}

fn http2_limits_view(
    locale: Locale,
    state: &UseStateHandle<Http2LimitsForm>,
    errors: &HashMap<String, String>,
) -> Html {
    let enabled_onchange = state_input(state, checked, |form, enabled| form.enabled = enabled);
    let header_list_onchange =
        state_input(state, text, |form, value| form.max_header_list_size = value);
    let reset_streams_onchange = state_input(state, text, |form, value| {
        form.max_pending_accept_reset_streams = value
    });
    html! {
        <>
            <div class="mt-6">
                { toggle(enabled_onchange, state.enabled, locale.t("ports.http2_limits"), "") }
            </div>
            <p class={HINT_CLASS}>{locale.t("ports.http_limits_hint")}</p>
            if state.enabled {
                <label class={LABEL_CLASS}>{locale.t("settings.http2_max_header_list_size")}</label>
                <input type="text" autocapitalize="off" value={state.max_header_list_size.clone()} onchange={header_list_onchange} class={INPUT_CLASS} placeholder="65536" />
                { error_view(errors.get("http2_max_header_list_size")) }
                <p class={HINT_CLASS}>{locale.t("settings.http2_max_header_list_size_hint")}</p>

                <label class={LABEL_CLASS}>{locale.t("settings.http2_max_pending_accept_reset_streams")}</label>
                <input type="text" autocapitalize="off" value={state.max_pending_accept_reset_streams.clone()} onchange={reset_streams_onchange} class={INPUT_CLASS} placeholder="128" />
                { error_view(errors.get("http2_max_pending_accept_reset_streams")) }
                <p class={HINT_CLASS}>{locale.t("settings.http2_max_pending_accept_reset_streams_hint")}</p>
            }
        </>
    }
}

fn http3_limits_view(
    locale: Locale,
    state: &UseStateHandle<Http3LimitsForm>,
    errors: &HashMap<String, String>,
) -> Html {
    let enabled_onchange = state_input(state, checked, |form, enabled| form.enabled = enabled);
    let bidi_onchange = state_input(state, text, |form, value| {
        form.max_concurrent_bidi_streams = value
    });
    let uni_onchange = state_input(state, text, |form, value| {
        form.max_concurrent_uni_streams = value
    });
    html! {
        <>
            <div class="mt-6">
                { toggle(enabled_onchange, state.enabled, locale.t("ports.http3_limits"), "") }
            </div>
            <p class={HINT_CLASS}>{locale.t("ports.http_limits_hint")}</p>
            if state.enabled {
                <label class={LABEL_CLASS}>{locale.t("settings.http3_max_concurrent_bidi_streams")}</label>
                <input type="text" autocapitalize="off" value={state.max_concurrent_bidi_streams.clone()} onchange={bidi_onchange} class={INPUT_CLASS} placeholder="100" />
                { error_view(errors.get("http3_max_concurrent_bidi_streams")) }
                <p class={HINT_CLASS}>{locale.t("settings.http3_max_concurrent_bidi_streams_hint")}</p>

                <label class={LABEL_CLASS}>{locale.t("settings.http3_max_concurrent_uni_streams")}</label>
                <input type="text" autocapitalize="off" value={state.max_concurrent_uni_streams.clone()} onchange={uni_onchange} class={INPUT_CLASS} placeholder="10" />
                { error_view(errors.get("http3_max_concurrent_uni_streams")) }
                <p class={HINT_CLASS}>{locale.t("settings.http3_max_concurrent_uni_streams_hint")}</p>
            }
        </>
    }
}

/// Writes the checked state of a checkbox to the state handle.
fn checkbox_onchange(state: UseStateHandle<bool>) -> Callback<Event> {
    Callback::from(move |event: Event| {
        let target: HtmlInputElement = event.target().unwrap_throw().dyn_into().unwrap_throw();
        state.set(target.checked());
    })
}

/// Writes the value of a text input to the state handle.
fn input_onchange(state: UseStateHandle<String>) -> Callback<Event> {
    Callback::from(move |event: Event| {
        let target: HtmlInputElement = event.target().unwrap_throw().dyn_into().unwrap_throw();
        state.set(target.value());
    })
}

/// Writes the value of a select to the state handle.
fn select_onchange(state: UseStateHandle<String>) -> Callback<Event> {
    Callback::from(move |event: Event| {
        let target: HtmlSelectElement = event.target().unwrap_throw().dyn_into().unwrap_throw();
        state.set(target.value());
    })
}

/// Writes the port number of a text input to the state handle. An invalid number becomes 1.
fn port_onchange(state: UseStateHandle<u16>) -> Callback<Event> {
    Callback::from(move |event: Event| {
        let target: HtmlInputElement = event.target().unwrap_throw().dyn_into().unwrap_throw();
        state.set(target.value().parse().unwrap_or(1));
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn form(protocol: &str, server_names: &str) -> PortForm {
        PortForm {
            active: true,
            name: " web ".into(),
            protocol: protocol.into(),
            interface: "0.0.0.0".into(),
            port: 443,
            server_names: server_names.into(),
            client_auth: ClientAuthMode::Off,
            client_ca_certs: Vec::new(),
            proxy_protocol: ProxyProtocolForm::new(None),
            http2_limits: Http2LimitsForm::new(None),
            http3_limits: Http3LimitsForm::new(None),
        }
    }

    fn build(form: &PortForm) -> Result<Port, HashMap<String, String>> {
        get_port(Locale::En, form, &TlsTermination::default())
    }

    #[test]
    fn tls_port_keeps_server_names() {
        let port = build(&form("https", "example.com, *.example.com")).unwrap();
        assert_eq!(port.name, "web");
        assert_eq!(port.listen.to_string(), "/ip4/0.0.0.0/tcp/443/https");
        assert_eq!(
            port.opts.tls_termination.unwrap().server_names,
            vec!["example.com".to_string(), "*.example.com".to_string()]
        );
    }

    #[test]
    fn plain_port_has_no_tls_termination() {
        let port = build(&form("tcp", "example.com")).unwrap();
        assert_eq!(port.opts.tls_termination, None);
    }

    #[test]
    fn invalid_server_name_is_an_error() {
        let errors = build(&form("tls", "exa mple.com")).unwrap_err();
        assert!(errors.contains_key("server_names"));
    }

    #[test]
    fn empty_interface_is_an_error() {
        let mut form = form("http", "");
        form.interface = " ".into();
        let errors = build(&form).unwrap_err();
        assert_eq!(
            errors["interface"],
            Locale::En.t("ports.interface_required")
        );
    }

    #[test]
    fn client_auth_needs_a_root_certificate() {
        let root: ShortId = "abc".parse().unwrap();
        let mut form = form("https", "");
        form.client_auth = parse_client_auth("required");
        let errors = build(&form).unwrap_err();
        assert_eq!(
            errors["client_ca_certs"],
            Locale::En.error_message(&Error::ClientCaCertsMissing)
        );

        form.client_ca_certs = vec![root];
        let tls = build(&form).unwrap().opts.tls_termination.unwrap();
        assert_eq!(tls.client_auth, ClientAuthMode::Required);
        assert_eq!(tls.client_ca_certs, vec![root]);

        form.client_auth = parse_client_auth("off");
        let tls = build(&form).unwrap().opts.tls_termination.unwrap();
        assert_eq!(tls.client_auth, ClientAuthMode::Off);
        assert!(tls.client_ca_certs.is_empty());
    }

    #[test]
    fn proxy_protocol_is_kept_only_on_supported_ports() {
        let mut form = form("https", "");
        form.proxy_protocol = ProxyProtocolForm {
            enabled: true,
            accept: find_option(&PROXY_PROTOCOL_ACCEPT, "v2"),
            trusted: "10.0.0.0/8, 192.168.1.10".into(),
            timeout: "3".into(),
        };
        let config = build(&form).unwrap().opts.proxy_protocol.unwrap();
        assert_eq!(config.accept, ProxyProtocolAccept::V2);
        assert_eq!(
            format_cidr_list(&config.trusted),
            "10.0.0.0/8, 192.168.1.10/32"
        );
        assert_eq!(config.timeout, std::time::Duration::from_secs(3));
        assert_eq!(
            ProxyProtocolForm::new(Some(&config)),
            ProxyProtocolForm {
                trusted: format_cidr_list(&config.trusted),
                ..form.proxy_protocol.clone()
            }
        );

        form.protocol = "udp".into();
        assert_eq!(build(&form).unwrap().opts.proxy_protocol, None);
        form.protocol = "tcp".into();
        form.proxy_protocol.enabled = false;
        assert_eq!(build(&form).unwrap().opts.proxy_protocol, None);
    }

    #[test]
    fn proxy_protocol_needs_trusted_addresses_and_a_timeout() {
        let mut form = form("http", "");
        form.proxy_protocol.enabled = true;
        let errors = build(&form).unwrap_err();
        assert_eq!(
            errors["proxy_protocol_trusted"],
            Locale::En.error_message(&Error::ProxyProtocolTrustedMissing)
        );

        form.proxy_protocol.trusted = "10.0.0.0/33".into();
        form.proxy_protocol.timeout = "0".into();
        let errors = build(&form).unwrap_err();
        assert!(errors.contains_key("proxy_protocol_trusted"));
        assert!(errors.contains_key("proxy_protocol_timeout"));
        assert_eq!(
            find_option(&PROXY_PROTOCOL_ACCEPT, "unknown"),
            ProxyProtocolAccept::Any
        );
    }

    #[test]
    fn toggle_id_adds_and_removes_the_id() {
        let a: ShortId = "abc".parse().unwrap();
        let b: ShortId = "def".parse().unwrap();
        assert_eq!(toggle_id(&[a], b, true), vec![a, b]);
        assert_eq!(toggle_id(&[a, b], a, false), vec![b]);
        assert_eq!(toggle_id(&[a], a, true), vec![a]);
        assert_eq!(parse_client_auth("optional"), ClientAuthMode::Optional);
        assert_eq!(parse_client_auth("unknown"), ClientAuthMode::Off);
    }
}
