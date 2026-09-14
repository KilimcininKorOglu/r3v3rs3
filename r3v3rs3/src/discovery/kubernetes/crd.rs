//! Proxies from `R3v3rs3Proxy` custom resources. The spec holds the fields of the proxy model, and
//! the label parser reads it. The `service` of an HTTP route, or of a TCP or UDP proxy, names a
//! Service whose ready endpoints become the upstream servers.

use super::cluster::{object_key, Cluster, Resources};
use super::PROVIDER;
use crate::discovery::{labels, Built, ProxyDefinition, ProxyGroups};
use k8s_openapi::api::networking::v1::{IngressServiceBackend, ServiceBackendPort};
use k8s_openapi::apimachinery::pkg::apis::meta::v1::ObjectMeta;
use k8s_openapi::apimachinery::pkg::util::intstr::IntOrString;
use k8s_openapi::NamespaceResourceScope;
use kube::Resource;
use r3v3rs3_api::proxy::ProxyKind;
use serde::{Deserialize, Serialize};
use serde_json::{json, Map, Value};
use std::borrow::Cow;

pub const GROUP: &str = "r3v3rs3.io";
pub const VERSION: &str = "v1";
pub const KIND: &str = "R3v3rs3Proxy";
pub const PLURAL: &str = "r3v3rs3proxies";
/// The protocols of a proxy. The first protocol is the default.
pub const PROTOCOLS: [&str; 3] = ["http", "tcp", "udp"];
/// The label name of the proxy. One resource defines one proxy.
const PROXY_NAME: &str = "proxy";
/// The parser reads a route only with servers, so a route with a `service` carries this server
/// until the endpoints of the Service replace it.
const PLACEHOLDER_URL: &str = "http://127.0.0.1/";

/// A proxy that a namespaced `r3v3rs3.io/v1` resource defines.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct R3v3rs3Proxy {
    #[serde(default)]
    pub metadata: ObjectMeta,
    #[serde(default)]
    pub spec: Value,
}

impl Resource for R3v3rs3Proxy {
    type DynamicType = ();
    type Scope = NamespaceResourceScope;

    fn kind(_: &()) -> Cow<'_, str> {
        KIND.into()
    }

    fn group(_: &()) -> Cow<'_, str> {
        GROUP.into()
    }

    fn version(_: &()) -> Cow<'_, str> {
        VERSION.into()
    }

    fn plural(_: &()) -> Cow<'_, str> {
        PLURAL.into()
    }

    fn meta(&self) -> &ObjectMeta {
        &self.metadata
    }

    fn meta_mut(&mut self) -> &mut ObjectMeta {
        &mut self.metadata
    }
}

/// A resource and the Services that its proxy uses.
struct Target<'a> {
    cluster: &'a Cluster<'a>,
    namespace: String,
    name: String,
    resource: String,
    group: String,
}

/// Builds the proxy of each R3v3rs3Proxy resource. The ports of the settings apply to a resource
/// without `ports`.
pub fn build(resources: &Resources, ports: &[String]) -> Built {
    let cluster = Cluster::new(resources);
    let mut proxies = resources.proxies.iter().collect::<Vec<_>>();
    proxies.sort_by_key(|proxy| object_key(&proxy.metadata));

    let mut built = Built::default();
    let mut groups = ProxyGroups::new(PROVIDER);
    for proxy in proxies {
        let (namespace, name) = object_key(&proxy.metadata);
        let target = Target {
            cluster: &cluster,
            resource: format!("r3v3rs3proxy {namespace}/{name}"),
            group: format!("crd/{namespace}/{name}"),
            namespace,
            name,
        };
        if let Err(message) = add_proxy(&mut built, &mut groups, &target, ports, &proxy.spec) {
            built.issue(&target.resource, message);
        }
    }
    built.proxies = groups.into_proxies();
    built
}

fn add_proxy(
    built: &mut Built,
    groups: &mut ProxyGroups,
    target: &Target,
    ports: &[String],
    spec: &Value,
) -> Result<(), String> {
    let mut spec = spec
        .as_object()
        .cloned()
        .ok_or("the spec is not an object")?;
    let protocol = take_protocol(&mut spec)?;
    let name = format!("{}/{}", target.namespace, target.name);
    spec.entry("name").or_insert_with(|| json!(name));
    if !ports.is_empty() {
        spec.entry("ports").or_insert_with(|| json!(ports));
    }
    let services = if protocol == PROTOCOLS[0] {
        route_services(&mut spec)?
    } else {
        stream_servers(built, target, &mut spec, protocol)?;
        Vec::new()
    };

    let mut pairs = Vec::new();
    let prefix = format!("{}.{protocol}.{PROXY_NAME}", labels::PREFIX);
    flatten(&prefix, &Value::Object(spec), &mut pairs)?;
    let mut parsed = labels::parse(pairs.iter().map(|(k, v)| (k.as_str(), v.as_str())), None);
    for definition in &mut parsed.definitions {
        set_route_servers(built, target, definition, &services);
    }
    built.add_parsed(groups, &target.group, &target.resource, parsed);
    Ok(())
}

fn take_protocol(spec: &mut Map<String, Value>) -> Result<&'static str, String> {
    match spec.remove("protocol") {
        None => Ok(PROTOCOLS[0]),
        Some(Value::String(protocol)) => PROTOCOLS
            .into_iter()
            .find(|known| *known == protocol)
            .ok_or_else(|| format!("unknown protocol: {protocol}")),
        Some(other) => Err(format!("unknown protocol: {other}")),
    }
}

/// Replaces the `service` of each HTTP route with a placeholder server. Returns the Service of
/// each route by route index.
fn route_services(
    spec: &mut Map<String, Value>,
) -> Result<Vec<(usize, IngressServiceBackend)>, String> {
    let Some(Value::Array(routes)) = spec.get_mut("routes") else {
        return Ok(Vec::new());
    };
    let mut services = Vec::new();
    for (index, route) in routes.iter_mut().enumerate() {
        let Some(route) = route.as_object_mut() else {
            continue;
        };
        let Some(service) = route.remove("service") else {
            continue;
        };
        if route.contains_key("servers") {
            return Err(format!(
                "routes.{index}: service cannot be used together with servers"
            ));
        }
        let backend =
            service_backend(service).map_err(|err| format!("routes.{index}.service: {err}"))?;
        services.push((index, backend));
        route.insert("servers".into(), json!([{ "url": PLACEHOLDER_URL }]));
    }
    Ok(services)
}

/// Replaces the `service` of a TCP or UDP proxy with the upstream servers of its ready endpoints.
fn stream_servers(
    built: &mut Built,
    target: &Target,
    spec: &mut Map<String, Value>,
    transport: &str,
) -> Result<(), String> {
    let Some(service) = spec.remove("service") else {
        return Ok(());
    };
    if spec.contains_key("upstream_servers") {
        return Err("service cannot be used together with upstream_servers".into());
    }
    let backend = service_backend(service).map_err(|err| format!("service: {err}"))?;
    match target.cluster.endpoints(&target.namespace, &backend) {
        Ok(endpoints) => {
            let servers = endpoints
                .addresses
                .iter()
                .map(|(host, port)| json!({ "addr": labels::stream_addr(host, transport, *port) }))
                .collect();
            spec.insert("upstream_servers".into(), Value::Array(servers));
        }
        Err(message) => built.issue(&target.resource, format!("service: {message}")),
    }
    Ok(())
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct ServiceRef {
    name: String,
    port: IntOrString,
}

/// The backend of a `service: {name, port}` value. The port is a number or a port name.
fn service_backend(value: Value) -> Result<IngressServiceBackend, String> {
    let service = serde_json::from_value::<ServiceRef>(value).map_err(|err| err.to_string())?;
    let port = match service.port {
        IntOrString::Int(number) => ServiceBackendPort {
            number: Some(number),
            name: None,
        },
        IntOrString::String(name) => ServiceBackendPort {
            name: Some(name),
            number: None,
        },
    };
    Ok(IngressServiceBackend {
        name: service.name,
        port: Some(port),
    })
}

/// Sets the servers of the routes with a Service. A Service without ready endpoints leaves its
/// route without servers, so its requests do not reach another route.
fn set_route_servers(
    built: &mut Built,
    target: &Target,
    definition: &mut ProxyDefinition,
    services: &[(usize, IngressServiceBackend)],
) {
    let ProxyKind::Http(http) = &mut definition.kind else {
        return;
    };
    for (index, backend) in services {
        let Some(route) = http.routes.get_mut(*index) else {
            continue;
        };
        route.servers = target
            .cluster
            .servers(&target.namespace, backend)
            .unwrap_or_else(|message| {
                built.issue(&target.resource, format!("routes.{index}: {message}"));
                Vec::new()
            });
    }
}

/// Adds one label for each value of the tree. A list uses the keys 0, 1, 2 and so on.
fn flatten(path: &str, value: &Value, pairs: &mut Vec<(String, String)>) -> Result<(), String> {
    match value {
        Value::Null => {}
        Value::String(text) => pairs.push((path.to_string(), text.clone())),
        Value::Bool(_) | Value::Number(_) => pairs.push((path.to_string(), value.to_string())),
        Value::Array(items) => {
            for (index, item) in items.iter().enumerate() {
                flatten(&format!("{path}.{index}"), item, pairs)?;
            }
        }
        Value::Object(map) => {
            for (key, item) in map {
                if key.is_empty() || key.contains('.') {
                    return Err(format!("a key cannot be empty or contain a dot: {key:?}"));
                }
                flatten(&format!("{path}.{key}"), item, pairs)?;
            }
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::super::cluster::fixtures::{cluster_resources, http, messages, object, urls};
    use super::*;
    use k8s_openapi::api::core::v1::{Secret, Service};
    use k8s_openapi::api::discovery::v1::EndpointSlice;
    use k8s_openapi::api::networking::v1::Ingress;
    use k8s_openapi::apiextensions_apiserver::pkg::apis::apiextensions::v1::CustomResourceDefinition;

    fn proxy(name: &str, spec: Value) -> std::sync::Arc<R3v3rs3Proxy> {
        object(json!({
            "apiVersion": "r3v3rs3.io/v1", "kind": KIND,
            "metadata": {"namespace": "default", "name": name},
            "spec": spec,
        }))
    }

    fn build_proxies(proxies: Vec<std::sync::Arc<R3v3rs3Proxy>>, ports: &[&str]) -> Built {
        let resources = Resources {
            proxies,
            ..cluster_resources()
        };
        let ports = ports
            .iter()
            .map(|port| port.to_string())
            .collect::<Vec<_>>();
        build(&resources, &ports)
    }

    #[test]
    fn a_spec_defines_the_proxy_and_services_become_servers() {
        let built = build_proxies(
            vec![
                proxy(
                    "app",
                    json!({
                        "vhosts": ["app.example.com", "www.example.com"],
                        "headers": {"response": [{"action": "set", "name": "X-Tags", "value": "a, b"}]},
                        "routes": [
                            {"path": "/", "service": {"name": "app", "port": "http"}},
                            {"path": "/static", "servers": [{"url": "http://10.0.0.9:8080"}]},
                            {"path": "/idle", "service": {"name": "idle", "port": 80}},
                        ],
                    }),
                ),
                proxy(
                    "db",
                    json!({"protocol": "tcp", "ports": ["postgres"], "name": "Database",
                           "service": {"name": "app", "port": 80}}),
                ),
            ],
            &["web"],
        );
        let keys = built
            .proxies
            .iter()
            .map(|p| p.key.as_str())
            .collect::<Vec<_>>();
        assert_eq!(
            keys,
            ["crd/default/app/http.proxy", "crd/default/db/tcp.proxy"]
        );

        let app = &built.proxies[0];
        assert_eq!(app.source.resource, "r3v3rs3proxy default/app");
        assert_eq!(app.definition.name, "default/app");
        assert_eq!(app.definition.ports, ["web"]);
        let app_http = http(app);
        assert_eq!(app_http.vhosts.len(), 2);
        assert_eq!(app_http.headers.response[0].to_string(), "set X-Tags: a, b");
        assert_eq!(
            urls(&app_http.routes[0]),
            ["http://10.0.0.1:8080/", "http://[fd00::3]:8080/"]
        );
        assert_eq!(urls(&app_http.routes[1]), ["http://10.0.0.9:8080/"]);
        assert!(app_http.routes[2].servers.is_empty());

        let db = &built.proxies[1];
        assert_eq!(db.definition.name, "Database");
        assert_eq!(db.definition.ports, ["postgres"]);
        let ProxyKind::Tcp(tcp) = &db.definition.kind else {
            panic!("expected a TCP proxy");
        };
        let addrs = tcp
            .upstream_servers
            .iter()
            .map(|server| server.addr.to_string())
            .collect::<Vec<_>>();
        assert_eq!(addrs, ["/ip4/10.0.0.1/tcp/8080", "/ip6/fd00::3/tcp/8080"]);

        assert_eq!(
            messages(&built),
            ["r3v3rs3proxy default/app: routes.2: service default/idle has no ready endpoint"]
        );
    }

    #[test]
    fn invalid_specs_are_issues() {
        let built = build_proxies(
            vec![
                proxy("a-protocol", json!({"protocol": "quic", "ports": ["web"]})),
                proxy(
                    "b-both",
                    json!({"ports": ["web"], "routes": [
                        {"service": {"name": "app", "port": 80}, "servers": [{"url": "http://10.0.0.9/"}]},
                    ]}),
                ),
                proxy(
                    "c-port",
                    json!({"ports": ["web"], "routes": [{"service": {"name": "app", "port": 80, "scheme": "https"}}]}),
                ),
                proxy(
                    "d-dot",
                    json!({"ports": ["web"], "routes": [{"servers": [{"url": "http://10.0.0.9/"}]}], "cache": {"a.b": 1}}),
                ),
                proxy(
                    "e-unknown",
                    json!({"ports": ["web"], "routes": [{"servers": [{"url": "http://10.0.0.9/"}]}], "timeout": "5s"}),
                ),
                proxy(
                    "f-ports",
                    json!({"routes": [{"servers": [{"url": "http://10.0.0.9/"}]}]}),
                ),
                proxy(
                    "g-gone",
                    json!({"protocol": "udp", "ports": ["dns"], "service": {"name": "gone", "port": 53}}),
                ),
            ],
            &[],
        );
        assert_eq!(built.proxies.len(), 1);
        assert_eq!(built.proxies[0].key, "crd/default/g-gone/udp.proxy");
        assert_eq!(
            messages(&built),
            [
                "r3v3rs3proxy default/a-protocol: unknown protocol: quic",
                "r3v3rs3proxy default/b-both: routes.0: service cannot be used together with servers",
                "r3v3rs3proxy default/c-port: routes.0.service: unknown field `scheme`, expected `name` or `port`",
                "r3v3rs3proxy default/d-dot: a key cannot be empty or contain a dot: \"a.b\"",
                "r3v3rs3proxy default/e-unknown: http.proxy: unknown keys: timeout",
                "r3v3rs3proxy default/f-ports: http.proxy: ports is required",
                "r3v3rs3proxy default/g-gone: service: service not found: default/gone",
            ]
        );
    }

    /// The manifests install the resource that the provider reads, and allow every resource that
    /// the provider watches.
    #[test]
    fn the_manifests_match_the_provider() {
        let crd_yaml = include_str!("../../../../deploy/kubernetes/crd.yaml");
        let crd = serde_saphyr::from_str::<CustomResourceDefinition>(crd_yaml).unwrap();
        assert_eq!(
            crd.metadata.name.as_deref(),
            Some("r3v3rs3proxies.r3v3rs3.io")
        );
        assert_eq!(crd.spec.group, GROUP);
        assert_eq!(crd.spec.names.kind, KIND);
        assert_eq!(crd.spec.names.plural, PLURAL);
        assert_eq!(crd.spec.scope, "Namespaced");
        assert_eq!(crd.spec.versions.len(), 1);
        let version = &crd.spec.versions[0];
        assert_eq!(version.name, VERSION);
        assert!(version.served && version.storage);

        let schema = serde_saphyr::from_str::<Value>(crd_yaml).unwrap();
        let spec = schema
            .pointer("/spec/versions/0/schema/openAPIV3Schema/properties/spec")
            .unwrap();
        assert_eq!(spec["x-kubernetes-preserve-unknown-fields"], true);
        assert_eq!(spec["properties"]["protocol"]["enum"], json!(PROTOCOLS));

        let rbac = serde_saphyr::from_multiple::<Value>(include_str!(
            "../../../../deploy/kubernetes/rbac.yaml"
        ))
        .unwrap();
        let role = rbac
            .iter()
            .find(|document| document["kind"] == "ClusterRole")
            .unwrap();
        let allowed = |group: &str, plural: &str| {
            role["rules"].as_array().unwrap().iter().any(|rule| {
                rule["apiGroups"]
                    .as_array()
                    .unwrap()
                    .contains(&json!(group))
                    && rule["resources"]
                        .as_array()
                        .unwrap()
                        .contains(&json!(plural))
                    && ["list", "watch"]
                        .iter()
                        .all(|verb| rule["verbs"].as_array().unwrap().contains(&json!(verb)))
            })
        };
        let watched = [
            (Ingress::group(&()), Ingress::plural(&())),
            (R3v3rs3Proxy::group(&()), R3v3rs3Proxy::plural(&())),
            (Service::group(&()), Service::plural(&())),
            (EndpointSlice::group(&()), EndpointSlice::plural(&())),
            (Secret::group(&()), Secret::plural(&())),
        ];
        for (group, plural) in watched {
            assert!(allowed(&group, &plural), "{group}/{plural}");
        }
    }
}
