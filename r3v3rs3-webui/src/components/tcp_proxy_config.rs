use super::http_proxy_config::{
    client_cert_view, error_view, input_element, item_update, parse_client_cert, select_setter,
    toggle, use_client_certs, INPUT_CLASS, LABEL_CLASS,
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

#[derive(Clone, PartialEq, Debug)]
struct ServerForm {
    host: String,
    port: u16,
    tls: bool,
}

impl ServerForm {
    fn new(server: &UpstreamServer) -> Self {
        Self {
            host: server.addr.host().unwrap_or_default(),
            port: server.addr.port().unwrap_or(0),
            tls: server.addr.is_tls(),
        }
    }

    fn example() -> Self {
        Self {
            host: "example.com".into(),
            port: 8080,
            tls: false,
        }
    }

    /// Builds the address of the server, e.g. `/dns/example.com/tcp/443/tls`. `None` when the
    /// host or the port is invalid.
    fn addr(&self) -> Option<Multiaddr> {
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
        format!("{host}/tcp/{}{tls}", self.port).parse().ok()
    }
}

#[function_component(TcpProxyConfig)]
pub fn tcp_proxy_config(props: &Props) -> Html {
    let locale = use_locale();
    let upstream_servers = use_state(|| {
        let servers = props
            .proxy
            .upstream_servers
            .iter()
            .map(ServerForm::new)
            .collect::<Vec<_>>();
        if servers.is_empty() {
            vec![ServerForm::example()]
        } else {
            servers
        }
    });
    let client_cert = use_state(|| props.proxy.client_cert);
    let client_certs = use_client_certs();

    let prev_entry =
        use_state::<Result<TcpProxy, HashMap<String, String>>, _>(|| Err(Default::default()));
    let entry = get_proxy(locale, &upstream_servers, *client_cert);

    if entry != *prev_entry {
        prev_entry.set(entry.clone());
        props.onchanged.emit(entry.clone());
    }
    let errors = entry.err().unwrap_or_default();

    html! {
        <>
            <label class={LABEL_CLASS}>{locale.t("proxy_form.upstream_server")}</label>

            { upstream_servers.iter().enumerate().map(|(i, server)| {
                server_view(locale, &upstream_servers, i, server, errors.get(&server_key(i)))
            }).collect::<Html>() }

            { client_cert_view(
                locale,
                select_setter(&client_cert, parse_client_cert),
                *client_cert,
                &client_certs,
            ) }
        </>
    }
}

fn server_view(
    locale: Locale,
    servers: &UseStateHandle<Vec<ServerForm>>,
    index: usize,
    server: &ServerForm,
    error: Option<&String>,
) -> Html {
    let host_onchange = item_update(servers, index, |server, event: Event| {
        server.host = input_element(&event).value();
    });
    let port_onchange = item_update(servers, index, |server, event: Event| {
        server.port = input_element(&event).value().parse().unwrap_or(0);
    });
    let tls_onchange = item_update(servers, index, |server, event: Event| {
        server.tls = input_element(&event).checked();
    });

    html! {
        <div class="mt-2 bg-white shadow-sm p-5 border border-neutral-300 dark:border-neutral-600 dark:bg-neutral-800 rounded-md">
            <label class="block mb-2 text-sm font-medium text-neutral-900 dark:text-neutral-200">{locale.t("proxy_form.host")}</label>
            <input type="text" autocapitalize="off" placeholder="example.com" onchange={host_onchange} value={server.host.clone()} class={INPUT_CLASS} />

            <label class={LABEL_CLASS}>{locale.t("common.port")}</label>
            <input type="number" placeholder="8080" onchange={port_onchange} value={server.port.to_string()} max="65535" min="1" class={INPUT_CLASS} />

            <div>
                { toggle(tls_onchange, server.tls, locale.t("proxy_form.tls"), "mt-4") }
            </div>
            { error_view(error) }
        </div>
    }
}

fn server_key(index: usize) -> String {
    format!("upstream_servers_{index}")
}

fn get_proxy(
    locale: Locale,
    servers: &[ServerForm],
    client_cert: Option<ShortId>,
) -> Result<TcpProxy, HashMap<String, String>> {
    let mut errors = HashMap::new();
    let mut upstream_servers = Vec::new();
    for (i, server) in servers.iter().enumerate() {
        match server.addr() {
            Some(addr) => upstream_servers.push(UpstreamServer { addr }),
            None => {
                errors.insert(server_key(i), locale.t("proxy_form.invalid_server").into());
            }
        }
    }

    if errors.is_empty() {
        Ok(TcpProxy {
            upstream_servers,
            client_cert,
        })
    } else {
        Err(errors)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn server(host: &str, port: u16, tls: bool) -> ServerForm {
        ServerForm {
            host: host.into(),
            port,
            tls,
        }
    }

    #[test]
    fn server_form_builds_the_address() {
        let cases = [
            (
                server("example.com", 443, true),
                "/dns/example.com/tcp/443/tls",
            ),
            (server("127.0.0.1", 8080, false), "/ip4/127.0.0.1/tcp/8080"),
            (server("::1", 22, false), "/ip6/::1/tcp/22"),
        ];
        for (form, expected) in cases {
            let addr = form.addr().unwrap();
            assert_eq!(addr.to_string(), expected);
            assert_eq!(ServerForm::new(&UpstreamServer { addr }), form);
        }
        assert_eq!(server(" ", 443, false).addr(), None);
        assert_eq!(server("example.com", 0, false).addr(), None);
    }

    #[test]
    fn parse_client_cert_reads_none_as_no_certificate() {
        assert_eq!(parse_client_cert(""), None);
        assert_eq!(parse_client_cert("a1b2c3d"), "a1b2c3d".parse().ok());
    }

    #[test]
    fn get_proxy_keeps_the_client_cert() {
        let id = "a1b2c3d".parse().unwrap();
        let proxy = get_proxy(Locale::En, &[server("example.com", 443, true)], Some(id)).unwrap();
        assert_eq!(proxy.client_cert, Some(id));

        let errors = get_proxy(Locale::En, &[server("", 443, true)], None).unwrap_err();
        assert!(errors.contains_key("upstream_servers_0"));
    }
}
