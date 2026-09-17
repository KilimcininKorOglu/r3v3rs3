use crate::API_ENDPOINT;
use crate::components::http_proxy_config::HttpProxyConfig;
use crate::components::tcp_proxy_config::TcpProxyConfig;
use crate::components::udp_proxy_config::UdpProxyConfig;
use crate::i18n::use_locale;
use crate::store::PortStore;
use gloo_net::http::Request;
use r3v3rs3_api::i18n::Locale;
use r3v3rs3_api::id::ShortId;
use r3v3rs3_api::proxy::{HttpProxy, ProxyKind, TcpProxy, UdpProxy};
use r3v3rs3_api::{port::PortEntry, proxy::Proxy};
use std::collections::HashMap;
use wasm_bindgen::{JsCast, UnwrapThrowExt};
use web_sys::{HtmlInputElement, HtmlSelectElement};
use yew::prelude::*;
use yewdux::prelude::*;

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum ProxyProtocol {
    Http,
    Tcp,
    Udp,
}

impl ProxyProtocol {
    fn label(self, locale: Locale) -> &'static str {
        match self {
            ProxyProtocol::Http => "HTTP / HTTPS",
            ProxyProtocol::Tcp => locale.t("protocol.tcp_tls"),
            ProxyProtocol::Udp => "UDP",
        }
    }
}

const PROTOCOLS: &[ProxyProtocol] = &[ProxyProtocol::Http, ProxyProtocol::Tcp, ProxyProtocol::Udp];

#[derive(Properties, PartialEq)]
pub struct Props {
    #[prop_or_default]
    pub proxy: Proxy,
    pub onchanged: Callback<Result<Proxy, HashMap<String, String>>>,
}

#[function_component(ProxyConfig)]
pub fn proxy_config(props: &Props) -> Html {
    let locale = use_locale();
    let ports = use_ports();

    let active = use_state(|| props.proxy.active);
    let active_cloned = active.clone();
    let active_onchange = Callback::from(move |event: Event| {
        let target: HtmlInputElement = event.target().unwrap_throw().dyn_into().unwrap_throw();
        active_cloned.set(target.checked());
    });

    let name = use_state(|| props.proxy.name.clone());
    let name_onchange = Callback::from({
        let name = name.clone();
        move |event: Event| {
            let target: HtmlInputElement = event.target().unwrap_throw().dyn_into().unwrap_throw();
            name.set(target.value());
        }
    });

    let protocol = use_state(|| protocol_of(&props.proxy.kind));
    let protocol_onchange = Callback::from({
        let protocol = protocol.clone();
        move |event: Event| {
            let target: HtmlSelectElement = event.target().unwrap_throw().dyn_into().unwrap_throw();
            if let Ok(index) = target.value().parse::<usize>() {
                protocol.set(PROTOCOLS[index]);
            }
        }
    });

    let bound_ports = use_state(|| props.proxy.ports.clone());

    let kinds = use_kind_states();
    let http_proxy_onchanged = kinds.http_callback();
    let tcp_proxy_onchanged = kinds.tcp_callback();
    let udp_proxy_onchanged = kinds.udp_callback();

    let compatible_ports = compatible_ports(&ports.entries, *protocol);

    let prev_entry =
        use_state::<Result<Proxy, HashMap<String, String>>, _>(|| Err(Default::default()));
    let entry = get_site(
        *active,
        &name,
        &bound_ports,
        kinds.of(*protocol),
        &compatible_ports,
    );

    if entry != *prev_entry {
        prev_entry.set(entry.clone());
        props.onchanged.emit(entry);
    }

    let http_proxy = http_proxy_of(&props.proxy.kind);
    let tcp_proxy = tcp_proxy_of(&props.proxy.kind);
    let udp_proxy = udp_proxy_of(&props.proxy.kind);

    html! {
        <>
            <label class="relative inline-flex items-center cursor-pointer mb-6">
                <input onchange={active_onchange} type="checkbox" checked={*active} class="sr-only peer" />
                <div class="shrink-0 w-9 h-5 bg-neutral-200 dark:bg-neutral-600 peer-focus:outline-none peer-focus:ring-4 peer-focus:ring-blue-300 rounded-full peer peer-checked:after:translate-x-full peer-checked:after:border-white after:content-[''] after:absolute after:top-[2px] after:left-[2px] after:bg-white after:border-neutral-300 after:border after:rounded-full after:h-4 after:w-4 after:transition-all peer-checked:bg-blue-600"></div>
                <span class="ml-3 text-sm font-medium text-neutral-900 dark:text-neutral-200">{locale.t("common.active")}</span>
            </label>

            <label class="block mb-2 text-sm font-medium text-neutral-900 dark:text-neutral-200">{locale.t("common.friendly_name")}</label>
            <input type="text" value={name.to_string()} onchange={name_onchange} class="bg-neutral-50 dark:text-neutral-200 dark:bg-neutral-800 dark:border-neutral-600 border border-neutral-300 text-neutral-900 text-sm rounded-lg focus:ring-blue-500 focus:border-blue-500 block w-full p-2.5" placeholder={locale.t("common.friendly_name_placeholder")} />

            <label class="block mt-4 mb-2 text-sm font-medium text-neutral-900 dark:text-neutral-200">{locale.t("common.protocol")}</label>
            <select onchange={protocol_onchange} class="bg-neutral-50 dark:text-neutral-200 dark:bg-neutral-800 dark:border-neutral-600 border border-neutral-300 text-neutral-900 text-sm rounded-lg focus:ring-blue-500 focus:border-blue-500 block w-full p-2.5">
                { PROTOCOLS.iter().enumerate().map(|(i, item)| {
                    html! {
                        <option selected={&*protocol == item} value={i.to_string()}>{item.label(locale)}</option>
                    }
                }).collect::<Html>() }
            </select>

            <label class="block mt-4 mb-2 text-sm font-medium text-neutral-900 dark:text-neutral-200">{locale.t("common.ports")}</label>
            <ul class="h-32 pb-3 overflow-y-auto text-sm text-neutral-700 bg-neutral-50 dark:text-neutral-200 dark:bg-neutral-800 dark:border-neutral-600 border border-neutral-300 rounded-lg">
                { compatible_ports.into_iter().map(|entry| port_item(entry, &bound_ports)).collect::<Html>() }
            </ul>

            if *protocol == ProxyProtocol::Http {
                <HttpProxyConfig onchanged={http_proxy_onchanged} proxy={http_proxy} />
            } else if *protocol == ProxyProtocol::Tcp {
                <TcpProxyConfig onchanged={tcp_proxy_onchanged} proxy={tcp_proxy} />
            } else {
                <UdpProxyConfig onchanged={udp_proxy_onchanged} proxy={udp_proxy} />
            }
        </>
    }
}

type KindResult = Result<ProxyKind, HashMap<String, String>>;

/// The edited proxy of each protocol. The form keeps all three, so a protocol change does not lose
/// the values of the other two.
struct KindStates {
    http: UseStateHandle<KindResult>,
    tcp: UseStateHandle<KindResult>,
    udp: UseStateHandle<KindResult>,
}

#[hook]
fn use_kind_states() -> KindStates {
    KindStates {
        http: use_state::<KindResult, _>(|| Ok(ProxyKind::Http(Default::default()))),
        tcp: use_state::<KindResult, _>(|| Ok(ProxyKind::Tcp(Default::default()))),
        udp: use_state::<KindResult, _>(|| Ok(ProxyKind::Udp(Default::default()))),
    }
}

impl KindStates {
    fn of(&self, protocol: ProxyProtocol) -> &KindResult {
        match protocol {
            ProxyProtocol::Http => &self.http,
            ProxyProtocol::Tcp => &self.tcp,
            ProxyProtocol::Udp => &self.udp,
        }
    }

    fn http_callback(&self) -> Callback<Result<HttpProxy, HashMap<String, String>>> {
        let state = self.http.clone();
        Callback::from(move |updated: Result<HttpProxy, HashMap<String, String>>| {
            state.set(updated.map(|http| ProxyKind::Http(Box::new(http))));
        })
    }

    fn tcp_callback(&self) -> Callback<Result<TcpProxy, HashMap<String, String>>> {
        let state = self.tcp.clone();
        Callback::from(move |updated: Result<TcpProxy, HashMap<String, String>>| {
            state.set(updated.map(ProxyKind::Tcp));
        })
    }

    fn udp_callback(&self) -> Callback<Result<UdpProxy, HashMap<String, String>>> {
        let state = self.udp.clone();
        Callback::from(move |updated: Result<UdpProxy, HashMap<String, String>>| {
            state.set(updated.map(ProxyKind::Udp));
        })
    }
}

/// Loads the ports once and returns the store that holds them.
#[hook]
fn use_ports() -> std::rc::Rc<PortStore> {
    let (ports, dispatcher) = use_store::<PortStore>();
    let ports_cloned = ports.clone();
    use_effect_with((), move |_| {
        wasm_bindgen_futures::spawn_local(async move {
            if let Ok(res) = get_ports().await {
                dispatcher.set(PortStore {
                    entries: res,
                    loaded: true,
                    ..(*ports_cloned).clone()
                });
            }
        });
    });
    ports
}

fn protocol_of(kind: &ProxyKind) -> ProxyProtocol {
    match kind {
        ProxyKind::Http(_) => ProxyProtocol::Http,
        ProxyKind::Tcp(_) => ProxyProtocol::Tcp,
        ProxyKind::Udp(_) => ProxyProtocol::Udp,
    }
}

/// The ports that can carry a proxy of the protocol.
fn compatible_ports(entries: &[PortEntry], protocol: ProxyProtocol) -> Vec<PortEntry> {
    entries
        .iter()
        .filter(|entry| match protocol {
            ProxyProtocol::Http => entry.port.listen.is_http(),
            ProxyProtocol::Tcp => !entry.port.listen.is_udp() && !entry.port.listen.is_http(),
            ProxyProtocol::Udp => entry.port.listen.is_udp() && !entry.port.listen.is_http(),
        })
        .cloned()
        .collect()
}

fn http_proxy_of(kind: &ProxyKind) -> HttpProxy {
    match kind {
        ProxyKind::Http(http_proxy) => HttpProxy::clone(http_proxy),
        _ => Default::default(),
    }
}

fn tcp_proxy_of(kind: &ProxyKind) -> TcpProxy {
    match kind {
        ProxyKind::Tcp(tcp_proxy) => tcp_proxy.clone(),
        _ => Default::default(),
    }
}

fn udp_proxy_of(kind: &ProxyKind) -> UdpProxy {
    match kind {
        ProxyKind::Udp(udp_proxy) => udp_proxy.clone(),
        _ => Default::default(),
    }
}

/// One checkbox of the port list. It adds the port to the bound ports or removes it.
fn port_item(entry: PortEntry, bound_ports: &UseStateHandle<Vec<ShortId>>) -> Html {
    let id = entry.id;
    let onchange = {
        let bound_ports = bound_ports.clone();
        Callback::from(move |event: Event| {
            let target: HtmlInputElement = event.target().unwrap_throw().dyn_into().unwrap_throw();
            let mut ports = (*bound_ports).clone();
            if target.checked() {
                if !ports.contains(&id) {
                    ports.push(id);
                }
            } else {
                ports.retain(|&bound| bound != id);
            }
            bound_ports.set(ports);
        })
    };
    html! {
        <li>
            <div class="flex items-center pl-2 rounded hover:bg-neutral-100 dark:hover:bg-neutral-900">
                <input {onchange} id={id.to_string()} type="checkbox" checked={bound_ports.contains(&id)} class="w-4 h-4 text-blue-600 bg-neutral-100 border-neutral-300 dark:bg-neutral-700 dark:border-neutral-600 rounded focus:ring-blue-500 focus:ring-2" />
                <label for={id.to_string()} class="w-full py-2 ml-2 text-sm font-medium text-neutral-900 dark:text-neutral-200 rounded">{entry.port.listen.to_string()}</label>
            </div>
        </li>
    }
}

fn get_site(
    active: bool,
    name: &str,
    ports: &[ShortId],
    kind: &Result<ProxyKind, HashMap<String, String>>,
    compatible_ports: &[PortEntry],
) -> Result<Proxy, HashMap<String, String>> {
    let mut errors = HashMap::new();
    let mut ports = ports.to_vec();
    ports.retain(|&id| compatible_ports.iter().any(|entry| entry.id == id));
    ports.sort();
    ports.dedup();

    let kind = match kind {
        Ok(kind) => kind,
        Err(err) => {
            errors.extend(err.clone());
            return Err(errors);
        }
    };

    if errors.is_empty() {
        Ok(Proxy {
            active,
            name: name.trim().to_string(),
            ports,
            kind: kind.clone(),
        })
    } else {
        Err(errors)
    }
}

async fn get_ports() -> Result<Vec<PortEntry>, gloo_net::Error> {
    Request::get(&format!("{API_ENDPOINT}/ports"))
        .send()
        .await?
        .json()
        .await
}
