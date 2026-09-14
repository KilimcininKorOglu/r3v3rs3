use super::http_proxy_config::{
    client_cert_view, error_view, format_seconds, input_element, item_update, list_buttons,
    or_error, parse_client_cert, parse_seconds, parse_weight, select_setter, timeout_field_view,
    toggle, upstream_form_view, use_client_certs, use_entry_errors, use_upstream_form,
    UpstreamForm, INPUT_CLASS, LABEL_CLASS,
};
use crate::i18n::use_locale;
use r3v3rs3_api::i18n::Locale;
use r3v3rs3_api::id::ShortId;
use r3v3rs3_api::multiaddr::Multiaddr;
use r3v3rs3_api::port::UpstreamServer;
use r3v3rs3_api::proxy::TcpProxy;
use std::collections::HashMap;
use std::net::{Ipv4Addr, Ipv6Addr};
use yew::prelude::*;

#[derive(Properties, PartialEq)]
pub struct Props {
    #[prop_or_default]
    pub proxy: TcpProxy,
    pub onchanged: Callback<Result<TcpProxy, HashMap<String, String>>>,
}

/// The form of one upstream server of a TCP or UDP proxy.
#[derive(Clone, PartialEq, Debug)]
pub(super) struct ServerForm {
    host: String,
    port: u16,
    tls: bool,
    weight: String,
}

impl ServerForm {
    fn new(server: &UpstreamServer) -> Self {
        Self {
            host: server.addr.host().unwrap_or_default(),
            port: server.addr.port().unwrap_or(0),
            tls: server.addr.is_tls(),
            weight: server.weight.to_string(),
        }
    }

    fn example() -> Self {
        Self {
            host: "example.com".into(),
            port: 8080,
            tls: false,
            weight: "1".into(),
        }
    }

    /// Builds the address of the server for the transport protocol, e.g.
    /// `/dns/example.com/tcp/443/tls`. `None` when the host or the port is invalid.
    fn addr(&self, protocol: &str) -> Option<Multiaddr> {
        let host = self.host.trim();
        if host.is_empty() || self.port == 0 {
            return None;
        }
        let host = if let Ok(addr) = host.parse::<Ipv4Addr>() {
            format!("/ip4/{addr}")
        } else if let Ok(addr) = host.parse::<Ipv6Addr>() {
            format!("/ip6/{addr}")
        } else {
            format!("/dns/{host}")
        };
        let tls = if self.tls { "/tls" } else { "" };
        format!("{host}/{protocol}/{}{tls}", self.port).parse().ok()
    }
}

/// Returns the state of the server forms of a proxy, with an example server for a new proxy.
#[hook]
pub(super) fn use_server_forms(servers: &[UpstreamServer]) -> UseStateHandle<Vec<ServerForm>> {
    let forms = servers.iter().map(ServerForm::new).collect::<Vec<_>>();
    use_state(move || {
        if forms.is_empty() {
            vec![ServerForm::example()]
        } else {
            forms
        }
    })
}

#[function_component(TcpProxyConfig)]
pub fn tcp_proxy_config(props: &Props) -> Html {
    let locale = use_locale();
    let upstream_servers = use_server_forms(&props.proxy.upstream_servers);
    let client_cert = use_state(|| props.proxy.client_cert);
    let client_certs = use_client_certs();
    let connect_timeout = use_state(|| format_seconds(props.proxy.connect_timeout));
    let upstream = use_upstream_form(props.proxy.load_balancing, props.proxy.health_check.clone());

    let entry = get_proxy(
        locale,
        &upstream_servers,
        *client_cert,
        &connect_timeout,
        &upstream,
    );
    let errors = use_entry_errors(entry, props.onchanged.clone());

    html! {
        <>
            { servers_view(locale, &upstream_servers, &errors, true) }

            { upstream_form_view(locale, &upstream, &errors, false) }

            { client_cert_view(
                locale,
                select_setter(&client_cert, parse_client_cert),
                *client_cert,
                &client_certs,
            ) }

            { timeout_field_view(
                locale,
                "proxy_form.connect_timeout",
                "proxy_form.connect_timeout_hint",
                &connect_timeout,
                errors.get("connect_timeout"),
            ) }
        </>
    }
}

/// The server forms of a proxy. `with_tls` shows the TLS toggle of each server.
pub(super) fn servers_view(
    locale: Locale,
    servers: &UseStateHandle<Vec<ServerForm>>,
    errors: &HashMap<String, String>,
    with_tls: bool,
) -> Html {
    html! {
        <>
            <label class={LABEL_CLASS}>{locale.t("proxy_form.upstream_server")}</label>
            { for (0..servers.len()).map(|index| server_view(locale, servers, index, errors.get(&server_key(index)), with_tls)) }
        </>
    }
}

fn server_view(
    locale: Locale,
    servers: &UseStateHandle<Vec<ServerForm>>,
    index: usize,
    error: Option<&String>,
    with_tls: bool,
) -> Html {
    let Some(server) = servers.get(index) else {
        return html! {};
    };
    let host_onchange = item_update(servers, index, |server, event: Event| {
        server.host = input_element(&event).value();
    });
    let port_onchange = item_update(servers, index, |server, event: Event| {
        server.port = input_element(&event).value().parse().unwrap_or(0);
    });
    let tls_onchange = item_update(servers, index, |server, event: Event| {
        server.tls = input_element(&event).checked();
    });
    let weight_onchange = item_update(servers, index, |server, event: Event| {
        server.weight = input_element(&event).value();
    });

    html! {
        <div class="mt-2 bg-white shadow-sm p-5 border border-neutral-300 dark:border-neutral-600 dark:bg-neutral-800 rounded-md">
            <label class="block mb-2 text-sm font-medium text-neutral-900 dark:text-neutral-200">{locale.t("proxy_form.host")}</label>
            <input type="text" autocapitalize="off" placeholder="example.com" onchange={host_onchange} value={server.host.clone()} class={INPUT_CLASS} />

            <label class={LABEL_CLASS}>{locale.t("common.port")}</label>
            <input type="number" placeholder="8080" onchange={port_onchange} value={server.port.to_string()} max="65535" min="1" class={INPUT_CLASS} />

            <label class={LABEL_CLASS}>{locale.t("proxy_form.weight")}</label>
            <input type="number" placeholder="1" onchange={weight_onchange} value={server.weight.clone()} max="65535" min="0" class={INPUT_CLASS} />

            if with_tls {
                <div>
                    { toggle(tls_onchange, server.tls, locale.t("proxy_form.tls"), "mt-4") }
                </div>
            }
            { error_view(error) }

            { list_buttons(servers, index, ServerForm::example) }
        </div>
    }
}

fn server_key(index: usize) -> String {
    format!("upstream_servers_{index}")
}

/// Builds the upstream servers for the transport protocol. Records an error for each invalid
/// server.
pub(super) fn parse_servers(
    locale: Locale,
    servers: &[ServerForm],
    protocol: &str,
    errors: &mut HashMap<String, String>,
) -> Vec<UpstreamServer> {
    let mut upstream_servers = Vec::new();
    for (i, server) in servers.iter().enumerate() {
        match parse_server(locale, server, protocol) {
            Ok(server) => upstream_servers.push(server),
            Err(err) => {
                errors.insert(server_key(i), err);
            }
        }
    }
    upstream_servers
}

fn parse_server(
    locale: Locale,
    server: &ServerForm,
    protocol: &str,
) -> Result<UpstreamServer, String> {
    let addr = server
        .addr(protocol)
        .ok_or_else(|| locale.t("proxy_form.invalid_server").to_string())?;
    let weight = parse_weight(locale, &server.weight)?;
    Ok(UpstreamServer { addr, weight })
}

fn get_proxy(
    locale: Locale,
    servers: &[ServerForm],
    client_cert: Option<ShortId>,
    connect_timeout: &str,
    upstream: &UpstreamForm,
) -> Result<TcpProxy, HashMap<String, String>> {
    let mut errors = HashMap::new();
    let upstream_servers = parse_servers(locale, servers, "tcp", &mut errors);
    let (load_balancing, health_check) = upstream.parse(locale, false, &mut errors);
    let connect_timeout = or_error(
        parse_seconds(
            locale,
            connect_timeout,
            "proxy_form.connect_timeout_name",
            1,
        ),
        "connect_timeout",
        &mut errors,
    );
    if !errors.is_empty() {
        return Err(errors);
    }
    Ok(TcpProxy {
        upstream_servers,
        client_cert,
        connect_timeout,
        load_balancing,
        health_check,
    })
}

#[cfg(test)]
pub(super) mod tests {
    use super::*;
    use r3v3rs3_api::upstream::{HealthCheck, LoadBalancing};
    use std::time::Duration;

    pub(in crate::components) fn first_server_policy() -> UpstreamForm {
        UpstreamForm::new(LoadBalancing::First, &HealthCheck::default())
    }

    pub(in crate::components) fn server(host: &str, port: u16, tls: bool) -> ServerForm {
        ServerForm {
            host: host.into(),
            port,
            tls,
            weight: "1".into(),
        }
    }

    #[test]
    fn server_form_builds_the_address() {
        let cases = [
            (
                server("example.com", 443, true),
                "tcp",
                "/dns/example.com/tcp/443/tls",
            ),
            (
                server("127.0.0.1", 8080, false),
                "tcp",
                "/ip4/127.0.0.1/tcp/8080",
            ),
            (server("::1", 53, false), "udp", "/ip6/::1/udp/53"),
        ];
        for (form, protocol, expected) in cases {
            let addr = form.addr(protocol).unwrap();
            assert_eq!(addr.to_string(), expected);
            assert_eq!(ServerForm::new(&UpstreamServer::new(addr)), form);
        }
        assert_eq!(server(" ", 443, false).addr("tcp"), None);
        assert_eq!(server("example.com", 0, false).addr("tcp"), None);
    }

    #[test]
    fn parse_client_cert_reads_none_as_no_certificate() {
        assert_eq!(parse_client_cert(""), None);
        assert_eq!(parse_client_cert("a1b2c3d"), "a1b2c3d".parse().ok());
    }

    #[test]
    fn get_proxy_keeps_the_client_cert_the_connect_timeout_and_the_policy() {
        let id = "a1b2c3d".parse().unwrap();
        let servers = [server("example.com", 443, true)];
        let upstream = first_server_policy();
        let proxy = get_proxy(Locale::En, &servers, Some(id), "3", &upstream).unwrap();
        assert_eq!(proxy.client_cert, Some(id));
        assert_eq!(proxy.connect_timeout, Duration::from_secs(3));
        assert_eq!(proxy.load_balancing, LoadBalancing::First);

        let invalid = [server("", 443, true)];
        let errors = get_proxy(Locale::En, &invalid, None, "3", &upstream).unwrap_err();
        assert!(errors.contains_key("upstream_servers_0"));

        let errors = get_proxy(Locale::En, &servers, None, "0", &upstream).unwrap_err();
        assert!(errors.contains_key("connect_timeout"));
    }

    #[test]
    fn server_forms_carry_the_weight() {
        let upstream = first_server_policy();
        let with_weight = |weight: &str| ServerForm {
            weight: weight.into(),
            ..server("example.com", 443, true)
        };
        let proxy = get_proxy(Locale::En, &[with_weight(" 0 ")], None, "3", &upstream).unwrap();
        assert_eq!(proxy.upstream_servers[0].weight, 0);
        assert_eq!(
            ServerForm::new(&proxy.upstream_servers[0]),
            with_weight("0")
        );

        let errors = get_proxy(Locale::Tr, &[with_weight("x")], None, "3", &upstream).unwrap_err();
        assert_eq!(
            errors.get("upstream_servers_0"),
            Some(&Locale::Tr.tf("proxy_form.invalid_weight", &[("value", "x")]))
        );
    }
}
