use r3v3rs3_api::cidr::{format_cidr_list, parse_cidr_list};
use r3v3rs3_api::client_ip::ClientIpConfig;
use r3v3rs3_api::proxy::{HttpProxy, Route, Server, ServerUrl};
use r3v3rs3_api::vhost::VirtualHost;
use std::collections::HashMap;
use std::str::FromStr;
use wasm_bindgen::{JsCast, UnwrapThrowExt};
use web_sys::HtmlInputElement;
use yew::prelude::*;

#[derive(Properties, PartialEq)]
pub struct Props {
    #[prop_or_default]
    pub proxy: HttpProxy,
    pub onchanged: Callback<Result<HttpProxy, HashMap<String, String>>>,
}

#[function_component(HttpProxyConfig)]
pub fn http_proxy_config(props: &Props) -> Html {
    let vhosts = use_state(|| {
        props
            .proxy
            .vhosts
            .iter()
            .map(|host| host.to_string())
            .collect::<Vec<_>>()
            .join(", ")
    });
    let vhosts_onchange = Callback::from({
        let vhosts = vhosts.clone();
        move |event: Event| {
            let target: HtmlInputElement = event.target().unwrap_throw().dyn_into().unwrap_throw();
            vhosts.set(target.value());
        }
    });

    let upgrade_insecure = use_state(|| props.proxy.upgrade_insecure);
    let upgrade_insecure_onchange = Callback::from({
        let upgrade_insecure = upgrade_insecure.clone();
        move |event: Event| {
            let target: HtmlInputElement = event.target().unwrap_throw().dyn_into().unwrap_throw();
            upgrade_insecure.set(target.checked());
        }
    });

    let trust_cdn = use_state(|| props.proxy.client_ip.trust_cdn);
    let trust_cdn_onchange = Callback::from({
        let trust_cdn = trust_cdn.clone();
        move |event: Event| {
            let target: HtmlInputElement = event.target().unwrap_throw().dyn_into().unwrap_throw();
            trust_cdn.set(target.checked());
        }
    });

    let trusted_proxies = use_state(|| format_cidr_list(&props.proxy.client_ip.trusted_proxies));
    let trusted_proxies_onchange = Callback::from({
        let trusted_proxies = trusted_proxies.clone();
        move |event: Event| {
            let target: HtmlInputElement = event.target().unwrap_throw().dyn_into().unwrap_throw();
            trusted_proxies.set(target.value());
        }
    });

    let routes = use_state(|| {
        props
            .proxy
            .routes
            .iter()
            .map(|route| {
                (
                    route.path.clone(),
                    route
                        .servers
                        .iter()
                        .map(|server| server.url.to_string())
                        .collect::<Vec<_>>(),
                )
            })
            .collect::<Vec<_>>()
    });

    if routes.is_empty() {
        routes.set(vec![("/".into(), Vec::new())]);
    }

    let prev_entry =
        use_state::<Result<HttpProxy, HashMap<String, String>>, _>(|| Err(Default::default()));
    let entry = get_proxy(
        &vhosts,
        &routes,
        *upgrade_insecure,
        *trust_cdn,
        &trusted_proxies,
    );

    if entry != *prev_entry {
        prev_entry.set(entry.clone());
        props.onchanged.emit(entry.clone());
    }

    let trusted_proxies_error = entry
        .as_ref()
        .err()
        .and_then(|errors| errors.get("trusted_proxies").cloned());

    html! {
        <>
            <label class="relative inline-flex items-center cursor-pointer my-6">
                <input onchange={upgrade_insecure_onchange} type="checkbox" checked={*upgrade_insecure} class="sr-only peer" />
                <div class="w-9 h-5 bg-neutral-200 dark:bg-neutral-600 peer-focus:outline-none peer-focus:ring-4 peer-focus:ring-blue-300 rounded-full peer peer-checked:after:translate-x-full peer-checked:after:border-white after:content-[''] after:absolute after:top-[2px] after:left-[2px] after:bg-white after:border-neutral-300 after:border after:rounded-full after:h-4 after:w-4 after:transition-all peer-checked:bg-blue-600"></div>
                <span class="ml-3 text-sm font-medium text-neutral-900 dark:text-neutral-200">{"Automatically Redirect HTTP to HTTPS"}</span>
            </label>

            <label class="block mt-4 mb-2 text-sm font-medium text-neutral-900 dark:text-neutral-200">{"Virtual Hosts"}</label>
            <input type="text" autocapitalize="off" value={vhosts.to_string()} onchange={vhosts_onchange} class="bg-neutral-50 dark:text-neutral-200 dark:bg-neutral-800 dark:border-neutral-600 border border-neutral-300 text-neutral-900 text-sm rounded-lg focus:ring-blue-500 focus:border-blue-500 block w-full p-2.5" placeholder="example.com" />
            <p class="mt-2 text-sm text-neutral-500">{"You can use commas to list multiple names and regex patterns, e.g, example.com, *.test.example.com, ^([a-z]+\\.)+example\\.com$ ."}</p>

            <label class="relative inline-flex items-center cursor-pointer mt-6">
                <input onchange={trust_cdn_onchange} type="checkbox" checked={*trust_cdn} class="sr-only peer" />
                <div class="w-9 h-5 bg-neutral-200 dark:bg-neutral-600 peer-focus:outline-none peer-focus:ring-4 peer-focus:ring-blue-300 rounded-full peer peer-checked:after:translate-x-full peer-checked:after:border-white after:content-[''] after:absolute after:top-[2px] after:left-[2px] after:bg-white after:border-neutral-300 after:border after:rounded-full after:h-4 after:w-4 after:transition-all peer-checked:bg-blue-600"></div>
                <span class="ml-3 text-sm font-medium text-neutral-900 dark:text-neutral-200">{"Trust Client IP Headers from Known CDNs"}</span>
            </label>
            <p class="mt-2 text-sm text-neutral-500">{"Reads the real client IP from Cloudflare, Fastly, Amazon CloudFront, Bunny CDN, Gcore, KeyCDN, Imperva and Google Cloud Load Balancing edge servers."}</p>

            <label class="block mt-4 mb-2 text-sm font-medium text-neutral-900 dark:text-neutral-200">{"Trusted Proxies"}</label>
            <input type="text" autocapitalize="off" value={trusted_proxies.to_string()} onchange={trusted_proxies_onchange} class="bg-neutral-50 dark:text-neutral-200 dark:bg-neutral-800 dark:border-neutral-600 border border-neutral-300 text-neutral-900 text-sm rounded-lg focus:ring-blue-500 focus:border-blue-500 block w-full p-2.5" placeholder="10.0.0.0/8, 192.168.1.10" />
            if let Some(err) = trusted_proxies_error {
                <p class="mt-2 text-sm text-red-600 dark:text-red-500">{err}</p>
            }
            <p class="mt-2 text-sm text-neutral-500">{"IP addresses or CIDR blocks of load balancers in front of r3v3rs3. Their X-Forwarded-For header is trusted."}</p>

            <label class="block mt-4 text-sm font-medium text-neutral-900 dark:text-neutral-200">{"Routes"}</label>

            { routes.iter().enumerate().map(|(i, (path, servers))| {
                let routes_len = routes.len();

                let routes_cloned = routes.clone();
                let add_onclick = Callback::from(move |_| {
                    let mut routes = (*routes_cloned).clone();
                    routes.insert(i + 1, ("/".into(), Vec::new()));
                    routes_cloned.set(routes);
                });

                let routes_cloned = routes.clone();
                let remove_onclick = Callback::from(move |_| {
                    if routes_len > 1 {
                        let mut routes = (*routes_cloned).clone();
                        routes.remove(i);
                        routes_cloned.set(routes);
                    }
                });

                let routes_cloned = routes.clone();
                let path_onchange = Callback::from(move |event: Event| {
                    let target: HtmlInputElement = event.target().unwrap_throw().dyn_into().unwrap_throw();
                    let mut routes = (*routes_cloned).clone();
                    routes[i].0 = target.value();
                    routes_cloned.set(routes);
                });

                let routes_cloned = routes.clone();
                let servers_onchange = Callback::from(move |event: Event| {
                    let target: HtmlInputElement = event.target().unwrap_throw().dyn_into().unwrap_throw();
                    let mut routes = (*routes_cloned).clone();
                    routes[i].1 = target.value().split('\n').map(|s| s.to_string()).collect();
                    routes_cloned.set(routes);
                });

                html! {
                    <div class="mt-2 bg-white dark:text-neutral-200 dark:bg-neutral-800 shadow-sm p-5 border border-neutral-300 dark:border-neutral-700 rounded-md">
                        <label class="block mb-2 text-sm font-medium text-neutral-900 dark:text-neutral-200">{"Path"}</label>
                        <input type="text" autocapitalize="off" placeholder="/" onchange={path_onchange} value={path.clone()} class="bg-neutral-50 dark:text-neutral-200 dark:bg-neutral-800 dark:border-neutral-600 border border-neutral-300 text-neutral-900 text-sm rounded-lg focus:ring-blue-500 focus:border-blue-500 block w-full p-2.5" />

                        <label class="block mt-4 mb-2 text-sm font-medium text-neutral-900 dark:text-neutral-200">{"Target"}</label>
                        <input type="url" placeholder="https://example.com/backend" value={servers.join("\n").to_string()} onchange={servers_onchange} class="bg-neutral-50 dark:text-neutral-200 dark:bg-neutral-800 dark:border-neutral-600 border border-neutral-300 text-neutral-900 text-sm rounded-lg focus:ring-blue-500 focus:border-blue-500 block w-full p-2.5" />

                        <div class="flex justify-end rounded-md mt-4 sm:ml-auto px-4 lg:px-0" role="group">
                            <button type="button" onclick={add_onclick} class="inline-flex items-center px-4 py-2 text-sm font-medium text-neutral-500 dark:text-neutral-200 bg-white dark:bg-neutral-800 border border-neutral-300 dark:border-neutral-700 rounded-l-lg hover:bg-neutral-100 hover:dark:bg-neutral-900 focus:z-10 focus:ring-4 focus:ring-neutral-200 dark:focus:ring-neutral-600">
                                <img src="/assets/icons/add.svg" class="w-4 h-4" />
                            </button>
                            <button type="button" onclick={remove_onclick} disabled={routes_len <= 1} class="inline-flex items-center px-4 py-2 text-sm font-medium text-neutral-500 dark:text-neutral-200 bg-white dark:bg-neutral-800 border border-l-0 border-neutral-300 dark:border-neutral-700 rounded-r-lg hover:bg-neutral-100 hover:dark:bg-neutral-900 focus:z-10 focus:ring-4 focus:ring-neutral-200 dark:focus:ring-neutral-600">
                                <img src="/assets/icons/remove.svg" class="w-4 h-4" />
                            </button>
                        </div>
                    </div>
                }
            }).collect::<Html>() }

        </>
    }
}

fn get_proxy(
    vhosts: &str,
    routes: &[(String, Vec<String>)],
    upgrade_insecure: bool,
    trust_cdn: bool,
    trusted_proxies: &str,
) -> Result<HttpProxy, HashMap<String, String>> {
    let mut errors = HashMap::new();
    let hosts = parse_vhosts(vhosts, &mut errors);
    let parsed_routes = parse_routes(routes, &mut errors);
    let trusted_proxies = match parse_cidr_list(trusted_proxies) {
        Ok(nets) => nets,
        Err(err) => {
            errors.insert("trusted_proxies".into(), err.to_string());
            Vec::new()
        }
    };

    if errors.is_empty() {
        Ok(HttpProxy {
            vhosts: hosts,
            routes: parsed_routes,
            upgrade_insecure,
            client_ip: ClientIpConfig {
                trust_cdn,
                trusted_proxies,
            },
        })
    } else {
        Err(errors)
    }
}

fn parse_vhosts(vhosts: &str, errors: &mut HashMap<String, String>) -> Vec<VirtualHost> {
    let mut hosts = Vec::new();
    for host in vhosts
        .split(',')
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
    {
        match VirtualHost::from_str(&host) {
            Ok(host) => hosts.push(host),
            Err(err) => {
                errors.insert("vhosts".into(), err.to_string());
            }
        }
    }
    hosts
}

fn parse_routes(
    routes: &[(String, Vec<String>)],
    errors: &mut HashMap<String, String>,
) -> Vec<Route> {
    let mut parsed_routes = Vec::new();
    for (i, (path, servers)) in routes.iter().enumerate() {
        if !path.starts_with('/') {
            errors.insert(format!("routes_{}", i), "Path must start with /".into());
            continue;
        }
        let mut urls = Vec::new();
        for url in servers {
            match ServerUrl::from_str(url) {
                Ok(url) => urls.push(Server { url }),
                Err(err) => {
                    errors.insert(format!("routes_{}", i), err.to_string());
                }
            }
        }
        if !urls.is_empty() {
            parsed_routes.push(Route {
                path: path.clone(),
                servers: urls,
            });
        }
    }
    parsed_routes
}
