use super::http_proxy_config::{
    format_seconds, or_error, parse_seconds, timeout_field_view, use_entry_errors,
};
use super::tcp_proxy_config::{parse_servers, servers_view, use_server_forms, ServerForm};
use crate::i18n::use_locale;
use r3v3rs3_api::i18n::Locale;
use r3v3rs3_api::proxy::UdpProxy;
use std::collections::HashMap;
use yew::prelude::*;

#[derive(Properties, PartialEq)]
pub struct Props {
    #[prop_or_default]
    pub proxy: UdpProxy,
    pub onchanged: Callback<Result<UdpProxy, HashMap<String, String>>>,
}

#[function_component(UdpProxyConfig)]
pub fn udp_proxy_config(props: &Props) -> Html {
    let locale = use_locale();
    let upstream_servers = use_server_forms(&props.proxy.upstream_servers);
    let idle_timeout = use_state(|| format_seconds(props.proxy.session_idle_timeout));

    let entry = get_proxy(locale, &upstream_servers, &idle_timeout);
    let errors = use_entry_errors(entry, props.onchanged.clone());

    html! {
        <>
            { servers_view(locale, &upstream_servers, &errors, false) }

            { timeout_field_view(
                locale,
                "proxy_form.session_idle_timeout",
                "proxy_form.session_idle_timeout_hint",
                &idle_timeout,
                errors.get("session_idle_timeout"),
            ) }
        </>
    }
}

fn get_proxy(
    locale: Locale,
    servers: &[ServerForm],
    idle_timeout: &str,
) -> Result<UdpProxy, HashMap<String, String>> {
    let mut errors = HashMap::new();
    let upstream_servers = parse_servers(locale, servers, "udp", &mut errors);
    let session_idle_timeout = or_error(
        parse_seconds(
            locale,
            idle_timeout,
            "proxy_form.session_idle_timeout_name",
            1,
        ),
        "session_idle_timeout",
        &mut errors,
    );
    if !errors.is_empty() {
        return Err(errors);
    }
    Ok(UdpProxy {
        upstream_servers,
        session_idle_timeout,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::components::tcp_proxy_config::tests::server;
    use std::time::Duration;

    #[test]
    fn get_proxy_builds_udp_servers_and_the_idle_timeout() {
        let proxy = get_proxy(Locale::En, &[server("127.0.0.1", 53, false)], "30").unwrap();
        assert_eq!(
            proxy.upstream_servers[0].addr.to_string(),
            "/ip4/127.0.0.1/udp/53"
        );
        assert_eq!(proxy.session_idle_timeout, Duration::from_secs(30));

        let errors = get_proxy(Locale::Tr, &[server("", 53, false)], "0").unwrap_err();
        assert_eq!(
            errors.get("upstream_servers_0").map(String::as_str),
            Some(Locale::Tr.t("proxy_form.invalid_server"))
        );
        assert!(errors.contains_key("session_idle_timeout"));
    }
}
