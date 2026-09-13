use super::http_proxy_config::{
    error_view, parse_comma_list, select_field, select_setter, HINT_CLASS, INPUT_CLASS, LABEL_CLASS,
};
use crate::i18n::use_locale;
use crate::pages::cert_list::get_cert_list;
use crate::API_ENDPOINT;
use gloo_net::http::Request;
use r3v3rs3_api::{
    cert::{CertInfo, CertKind},
    error::Error,
    i18n::Locale,
    id::ShortId,
    multiaddr::Multiaddr,
    port::{NetworkInterface, Port, PortOptions},
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
}

/// The state of the TLS section of the form.
struct TlsFields {
    server_names: UseStateHandle<String>,
    client_auth: UseStateHandle<ClientAuthMode>,
    client_ca_certs: UseStateHandle<Vec<ShortId>>,
    root_certs: UseStateHandle<Vec<CertInfo>>,
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
    let active_cloned = active.clone();
    let active_onchange = Callback::from(move |event: Event| {
        let target: HtmlInputElement = event.target().unwrap_throw().dyn_into().unwrap_throw();
        active_cloned.set(target.checked());
    });

    let interfaces = use_state(|| vec![interface.clone()]);
    let interfaces_cloned = interfaces.clone();
    let interface_cloned = interface.clone();
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
                if !list.contains(&interface_cloned) {
                    list.push(interface_cloned);
                }
                interfaces_cloned.set(list);
            }
        });
    });

    let protocol = match (udp, tls, http, quic) {
        (_, _, true, true) => "http3",
        (true, _, _, _) => "udp",
        (false, true, true, _) => "https",
        (false, true, false, _) => "tls",
        (false, false, true, _) => "http",
        (false, false, false, _) => "tcp",
    };

    let name = use_state(|| props.port.name.clone());
    let name_onchange = Callback::from({
        let name = name.clone();
        move |event: Event| {
            let target: HtmlInputElement = event.target().unwrap_throw().dyn_into().unwrap_throw();
            name.set(target.value());
        }
    });

    let protocol = use_state(|| protocol.to_string());
    let protocol_onchange = Callback::from({
        let protocol = protocol.clone();
        move |event: Event| {
            let target: HtmlSelectElement = event.target().unwrap_throw().dyn_into().unwrap_throw();
            protocol.set(target.value());
        }
    });

    let interface = use_state(|| interface);
    let interface_onchange = Callback::from({
        let interface = interface.clone();
        move |event: Event| {
            let target: HtmlSelectElement = event.target().unwrap_throw().dyn_into().unwrap_throw();
            interface.set(target.value());
        }
    });

    let port = use_state(|| port);
    let port_onchange = Callback::from({
        let port = port.clone();
        move |event: Event| {
            let target: HtmlInputElement = event.target().unwrap_throw().dyn_into().unwrap_throw();
            port.set(target.value().parse().unwrap_or(1));
        }
    });

    let tls_termination = props.port.opts.tls_termination.clone().unwrap_or_default();
    let tls_fields = use_tls_fields(&tls_termination);

    let form = PortForm {
        active: *active,
        name: name.to_string(),
        protocol: protocol.to_string(),
        interface: interface.to_string(),
        port: *port,
        server_names: tls_fields.server_names.to_string(),
        client_auth: *tls_fields.client_auth,
        client_ca_certs: (*tls_fields.client_ca_certs).clone(),
    };
    let prev_entry =
        use_state::<Result<Port, HashMap<String, String>>, _>(|| Err(Default::default()));
    let entry = get_port(locale, &form, &tls_termination);
    if entry != *prev_entry {
        prev_entry.set(entry.clone());
        props.onchanged.emit(entry.clone());
    }
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
                { interfaces.iter().map(|value| {
                    html! {
                        <option selected={&*interface == value} value={value.clone()}>{value}</option>
                    }
                }).collect::<Html>() }
            </select>
            { error_view(errors.get("interface")) }

            <label class="block mt-4 mb-2 text-sm font-medium text-neutral-900 dark:text-neutral-200">{locale.t("common.port")}</label>
            <input type="number" placeholder="8080" onchange={port_onchange} value={port.to_string()} max="65535" min="1" class="bg-neutral-50 dark:text-neutral-200 dark:bg-neutral-800 dark:border-neutral-600 border border-neutral-300 text-neutral-900 text-sm rounded-lg focus:ring-blue-500 focus:border-blue-500 block w-full p-2.5" />

            <label class="block mt-4 mb-2 text-sm font-medium text-neutral-900 dark:text-neutral-200">{locale.t("common.protocol")}</label>
            <select onchange={protocol_onchange} class="bg-neutral-50 dark:text-neutral-200 dark:bg-neutral-800 dark:border-neutral-600 border border-neutral-300 text-neutral-900 text-sm rounded-lg focus:ring-blue-500 focus:border-blue-500 block w-full p-2.5">
                { PROTOCOLS.iter().map(|(value, label)| {
                    html! {
                        <option selected={&*protocol == value} value={*value}>{protocol_label(locale, value, label)}</option>
                    }
                }).collect::<Html>() }
            </select>

            if is_tls_protocol(&protocol) {
                { tls_view(locale, &tls_fields, &errors) }
            }
        </>
    }
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
    let options = CLIENT_AUTH_MODES
        .iter()
        .map(|(mode, value, key)| {
            html! {
                <option selected={**client_auth == *mode} value={*value}>{locale.t(key)}</option>
            }
        })
        .collect::<Html>();
    select_field(
        locale.t("ports.client_auth"),
        select_setter(client_auth, parse_client_auth),
        options,
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
    CLIENT_AUTH_MODES
        .iter()
        .find(|(_, name, _)| *name == value)
        .map(|(mode, _, _)| *mode)
        .unwrap_or_default()
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

    match listen {
        Some(listen) if errors.is_empty() => Ok(Port {
            active: form.active,
            name: form.name.trim().to_string(),
            listen,
            opts: PortOptions {
                tls_termination: Some(tls_termination).filter(|_| is_tls_protocol(&form.protocol)),
            },
        }),
        _ => Err(errors),
    }
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
