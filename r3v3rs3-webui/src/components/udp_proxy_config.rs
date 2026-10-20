use super::http_proxy_config::{
    HINT_CLASS, INPUT_CLASS, LABEL_CLASS, UpstreamForm, error_view, format_seconds, or_error,
    parse_seconds, text_setter, timeout_field_view, upstream_form_view, use_entry_errors,
    use_upstream_form,
};
use super::tcp_proxy_config::{ServerForm, parse_servers, servers_view, use_server_forms};
use crate::i18n::use_locale;
use r3v3rs3_api::i18n::Locale;
use r3v3rs3_api::proxy::UdpProxy;
use r3v3rs3_api::upstream::{MAX_DATAGRAM_SIZE, MIN_DATAGRAM_SIZE};
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
    let max_datagram_size = use_state(|| props.proxy.max_datagram_size.to_string());
    let upstream = use_upstream_form(props.proxy.load_balancing, props.proxy.health_check.clone());

    let entry = get_proxy(
        locale,
        &upstream_servers,
        &idle_timeout,
        &max_datagram_size,
        &upstream,
    );
    let errors = use_entry_errors(entry, props.onchanged.clone());

    html! {
        <>
            { servers_view(locale, &upstream_servers, &errors, false) }

            { upstream_form_view(locale, &upstream, &errors, false) }

            { timeout_field_view(
                locale,
                "proxy_form.session_idle_timeout",
                "proxy_form.session_idle_timeout_hint",
                &idle_timeout,
                errors.get("session_idle_timeout"),
            ) }

            { datagram_size_field_view(
                locale,
                &max_datagram_size,
                errors.get("max_datagram_size"),
            ) }
        </>
    }
}

fn datagram_size_field_view(
    locale: Locale,
    value: &UseStateHandle<String>,
    error: Option<&String>,
) -> Html {
    let onchange = text_setter(value);
    html! {
        <>
            <label class={LABEL_CLASS}>{locale.t("proxy_form.max_datagram_size")}</label>
            <input
                type="number"
                min={MIN_DATAGRAM_SIZE.to_string()}
                max={MAX_DATAGRAM_SIZE.to_string()}
                value={(**value).clone()}
                {onchange}
                class={INPUT_CLASS}
            />
            { error_view(error) }
            <p class={HINT_CLASS}>{locale.t("proxy_form.max_datagram_size_hint")}</p>
        </>
    }
}

fn parse_datagram_size(locale: Locale, value: &str) -> Result<usize, String> {
    value
        .trim()
        .parse::<usize>()
        .ok()
        .filter(|&size| (MIN_DATAGRAM_SIZE..=MAX_DATAGRAM_SIZE).contains(&size))
        .ok_or_else(|| {
            locale.tf(
                "proxy_form.datagram_size_range",
                &[
                    ("min", &MIN_DATAGRAM_SIZE.to_string()),
                    ("max", &MAX_DATAGRAM_SIZE.to_string()),
                ],
            )
        })
}

fn get_proxy(
    locale: Locale,
    servers: &[ServerForm],
    idle_timeout: &str,
    max_datagram_size_str: &str,
    upstream: &UpstreamForm,
) -> Result<UdpProxy, HashMap<String, String>> {
    let mut errors = HashMap::new();
    let upstream_servers = parse_servers(locale, servers, "udp", &mut errors);
    let (load_balancing, health_check) = upstream.parse(locale, false, &mut errors);
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
    let max_datagram_size = or_error(
        parse_datagram_size(locale, max_datagram_size_str),
        "max_datagram_size",
        &mut errors,
    );
    if !errors.is_empty() {
        return Err(errors);
    }
    Ok(UdpProxy {
        upstream_servers,
        session_idle_timeout,
        max_datagram_size,
        load_balancing,
        health_check,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::components::tcp_proxy_config::tests::{first_server_policy, server};
    use r3v3rs3_api::upstream::LoadBalancing;
    use std::time::Duration;

    #[test]
    fn get_proxy_builds_udp_servers_the_idle_timeout_and_the_policy() {
        let upstream = first_server_policy();
        let servers = [server("127.0.0.1", 53, false)];
        let proxy = get_proxy(Locale::En, &servers, "30", "4096", &upstream).unwrap();
        assert_eq!(
            proxy.upstream_servers[0].addr.to_string(),
            "/ip4/127.0.0.1/udp/53"
        );
        assert_eq!(proxy.session_idle_timeout, Duration::from_secs(30));
        assert_eq!(proxy.max_datagram_size, 4096);
        assert_eq!(proxy.load_balancing, LoadBalancing::First);

        let errors =
            get_proxy(Locale::Tr, &[server("", 53, false)], "0", "100", &upstream).unwrap_err();
        assert_eq!(
            errors.get("upstream_servers_0").map(String::as_str),
            Some(Locale::Tr.t("proxy_form.invalid_server"))
        );
        assert!(errors.contains_key("session_idle_timeout"));
        assert!(errors.contains_key("max_datagram_size"));
    }
}
