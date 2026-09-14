//! Proxies from Ingress resources. Each host of an Ingress becomes one HTTP proxy, and each path of
//! the host becomes one route to the ready endpoints of its Service. The `r3v3rs3.io/<field>`
//! annotations set the other fields of the proxies.

use crate::certs::Cert;
use crate::discovery::{labels, Built, ProxyDefinition, ProxyGroups};
use k8s_openapi::api::core::v1::{Secret, Service, ServicePort};
use k8s_openapi::api::discovery::v1::EndpointSlice;
use k8s_openapi::api::networking::v1::{Ingress, IngressBackend, IngressServiceBackend};
use k8s_openapi::apimachinery::pkg::apis::meta::v1::ObjectMeta;
use r3v3rs3_api::cert::CertKind;
use r3v3rs3_api::discovery::{DiscoveryProvider, DiscoverySource};
use r3v3rs3_api::proxy::{ProxyKind, Route, Server};
use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::net::IpAddr;
use std::sync::Arc;

const PROVIDER: DiscoveryProvider = DiscoveryProvider::Kubernetes;
pub const ANNOTATION_PREFIX: &str = "r3v3rs3.io/";
const CLASS_ANNOTATION: &str = "kubernetes.io/ingress.class";
const SERVICE_NAME_LABEL: &str = "kubernetes.io/service-name";
const TLS_SECRET_TYPE: &str = "kubernetes.io/tls";
/// The fields that the rules of an Ingress set, so an annotation cannot set them.
const RULE_FIELDS: [&str; 4] = ["routes", "vhosts", "port", "scheme"];
/// The label name of the proxy of the rules without a host and of the default backend. A host
/// cannot be `*`.
const ANY_HOST: &str = "*";
/// An HTTP proxy needs routes, so the labels carry one route that the Ingress routes replace.
const PLACEHOLDER_ROUTE: (&str, &str) = ("routes.0.servers.0.url", "http://127.0.0.1/");

#[derive(Debug, Clone, Default)]
pub struct Settings {
    /// Empty selects every Ingress.
    pub ingress_class: String,
    /// The ports of an Ingress without the `r3v3rs3.io/ports` annotation.
    pub ports: Vec<String>,
}

/// The resources that the proxies use.
#[derive(Debug, Default)]
pub struct Resources {
    pub ingresses: Vec<Arc<Ingress>>,
    pub services: Vec<Arc<Service>>,
    pub slices: Vec<Arc<EndpointSlice>>,
    pub secrets: Vec<Arc<Secret>>,
}

/// Builds the proxies of the selected Ingress resources, and the certificates of their TLS
/// secrets.
pub fn build(resources: &Resources, settings: &Settings) -> Built {
    let cluster = Cluster::new(resources);
    let mut ingresses = resources
        .ingresses
        .iter()
        .filter(|ingress| selects_class(ingress, &settings.ingress_class))
        .collect::<Vec<_>>();
    ingresses.sort_by_key(|ingress| object_key(&ingress.metadata));

    let mut built = Built::default();
    let mut groups = ProxyGroups::new(PROVIDER);
    let mut secrets = BTreeSet::new();
    for ingress in ingresses {
        add_ingress(&mut built, &mut groups, &cluster, settings, ingress);
        secrets.extend(tls_secrets(ingress));
    }
    built.proxies = groups.into_proxies();
    for (namespace, name) in secrets {
        add_secret(&mut built, &cluster, &namespace, &name);
    }
    built
}

fn selects_class(ingress: &Ingress, class: &str) -> bool {
    if class.is_empty() {
        return true;
    }
    let spec_class = ingress
        .spec
        .as_ref()
        .and_then(|spec| spec.ingress_class_name.as_deref());
    let annotation = ingress
        .metadata
        .annotations
        .as_ref()
        .and_then(|annotations| annotations.get(CLASS_ANNOTATION))
        .map(String::as_str);
    spec_class.or(annotation) == Some(class)
}

fn object_key(meta: &ObjectMeta) -> (String, String) {
    (
        meta.namespace.clone().unwrap_or_default(),
        meta.name.clone().unwrap_or_default(),
    )
}

fn object_ref(meta: &ObjectMeta) -> Option<(&str, &str)> {
    Some((meta.namespace.as_deref()?, meta.name.as_deref()?))
}

/// The Services, the EndpointSlices and the TLS secrets by namespace and name.
struct Cluster<'a> {
    services: HashMap<(&'a str, &'a str), &'a Service>,
    /// The slices of each Service.
    slices: HashMap<(&'a str, &'a str), Vec<&'a EndpointSlice>>,
    secrets: HashMap<(&'a str, &'a str), &'a Secret>,
}

impl<'a> Cluster<'a> {
    fn new(resources: &'a Resources) -> Self {
        let services = resources
            .services
            .iter()
            .filter_map(|service| Some((object_ref(&service.metadata)?, service.as_ref())))
            .collect();
        let mut slices = HashMap::<_, Vec<_>>::new();
        for slice in &resources.slices {
            let service = slice
                .metadata
                .labels
                .as_ref()
                .and_then(|labels| labels.get(SERVICE_NAME_LABEL));
            if let (Some(namespace), Some(service)) = (slice.metadata.namespace.as_deref(), service)
            {
                slices
                    .entry((namespace, service.as_str()))
                    .or_default()
                    .push(slice.as_ref());
            }
        }
        let secrets = resources
            .secrets
            .iter()
            .filter(|secret| secret.type_.as_deref() == Some(TLS_SECRET_TYPE))
            .filter_map(|secret| Some((object_ref(&secret.metadata)?, secret.as_ref())))
            .collect();
        Self {
            services,
            slices,
            secrets,
        }
    }

    /// The servers of the ready endpoints of a Service port.
    fn servers(
        &self,
        namespace: &str,
        backend: &IngressServiceBackend,
    ) -> Result<Vec<Server>, String> {
        let name = backend.name.as_str();
        let service = self
            .services
            .get(&(namespace, name))
            .ok_or_else(|| format!("service not found: {namespace}/{name}"))?;
        let port = service_port(service, backend)?;
        let scheme = if is_https(port) { "https" } else { "http" };
        let mut urls = BTreeSet::new();
        for slice in self.slices.get(&(namespace, name)).into_iter().flatten() {
            let Some(number) = endpoint_port(slice, port) else {
                continue;
            };
            urls.extend(ready_addresses(slice).map(|host| server_url(scheme, host, number)));
        }
        if urls.is_empty() {
            return Err(format!("service {namespace}/{name} has no ready endpoint"));
        }
        urls.into_iter()
            .map(|url| {
                url.parse()
                    .map(|url| Server { url })
                    .map_err(|err| format!("invalid endpoint address {url}: {err}"))
            })
            .collect()
    }

    /// The certificate and the private key of a TLS secret.
    fn certificate(&self, namespace: &str, name: &str) -> Result<Cert, String> {
        let secret = self
            .secrets
            .get(&(namespace, name))
            .ok_or("the TLS secret is not found")?;
        let item = |key: &str| {
            secret
                .data
                .as_ref()
                .and_then(|data| data.get(key))
                .map(|bytes| bytes.0.clone())
                .ok_or_else(|| format!("the secret has no {key}"))
        };
        let cert = Cert::new(CertKind::Server, item("tls.crt")?, Some(item("tls.key")?))
            .map_err(|err| err.to_string())?;
        cert.certified_key().map_err(|err| err.to_string())?;
        Ok(cert)
    }
}

/// The port of the Service that the backend selects by number or by name.
fn service_port<'a>(
    service: &'a Service,
    backend: &IngressServiceBackend,
) -> Result<&'a ServicePort, String> {
    let ports = service
        .spec
        .as_ref()
        .and_then(|spec| spec.ports.as_deref())
        .unwrap_or_default();
    let wanted = backend.port.as_ref();
    let number = wanted.and_then(|port| port.number);
    let name = wanted.and_then(|port| port.name.as_deref());
    let found = match (number, name) {
        (Some(number), _) => ports.iter().find(|port| port.port == number),
        (None, Some(name)) => ports.iter().find(|port| port.name.as_deref() == Some(name)),
        (None, None) => None,
    };
    found.ok_or_else(|| {
        let port = number.map_or_else(|| name.unwrap_or_default().to_string(), |n| n.to_string());
        format!("service {} has no port {port}", backend.name)
    })
}

fn is_https(port: &ServicePort) -> bool {
    port.app_protocol.as_deref() == Some("https") || port.name.as_deref() == Some("https")
}

/// The endpoint port with the name of the Service port. A Service port without a name matches an
/// endpoint port without a name.
fn endpoint_port(slice: &EndpointSlice, port: &ServicePort) -> Option<u16> {
    let name = port.name.as_deref().unwrap_or_default();
    slice
        .ports
        .iter()
        .flatten()
        .find(|endpoint| endpoint.name.as_deref().unwrap_or_default() == name)
        .and_then(|endpoint| endpoint.port)
        .and_then(|number| u16::try_from(number).ok())
}

/// The addresses of the ready endpoints. An endpoint without the ready condition is ready.
fn ready_addresses(slice: &EndpointSlice) -> impl Iterator<Item = &str> {
    slice
        .endpoints
        .iter()
        .filter(|endpoint| {
            endpoint
                .conditions
                .as_ref()
                .and_then(|conditions| conditions.ready)
                != Some(false)
        })
        .flat_map(|endpoint| endpoint.addresses.iter().map(String::as_str))
}

fn server_url(scheme: &str, host: &str, port: u16) -> String {
    match host.parse::<IpAddr>() {
        Ok(IpAddr::V6(ip)) => format!("{scheme}://[{ip}]:{port}"),
        _ => format!("{scheme}://{host}:{port}"),
    }
}

/// A path of a rule, or the default backend.
struct PathRule<'a> {
    path: Option<&'a str>,
    path_type: Option<&'a str>,
    backend: &'a IngressBackend,
}

/// An Ingress and the resources that its rules use.
struct Target<'a> {
    cluster: &'a Cluster<'a>,
    namespace: String,
    resource: String,
}

fn add_ingress(
    built: &mut Built,
    groups: &mut ProxyGroups,
    cluster: &Cluster,
    settings: &Settings,
    ingress: &Ingress,
) {
    let (namespace, name) = object_key(&ingress.metadata);
    let target = Target {
        cluster,
        resource: format!("ingress {namespace}/{name}"),
        namespace,
    };
    let Some(fields) = annotation_fields(built, &target.resource, ingress, settings) else {
        return;
    };
    let group = format!("ingress/{}/{name}", target.namespace);
    for (host, rules) in host_rules(ingress) {
        let routes = rules
            .into_iter()
            .filter_map(|rule| route(built, &target, host, rule))
            .collect::<Vec<_>>();
        if routes.is_empty() {
            continue;
        }
        let display_name = format!("{}/{name} {host}", target.namespace);
        let result =
            definition(host, display_name.trim_end(), &fields, routes).and_then(|definition| {
                groups
                    .add(&group, &target.resource, definition)
                    .map_err(|message| vec![message])
            });
        for message in result.err().into_iter().flatten() {
            built.issue(&target.resource, message);
        }
    }
}

/// The proxy fields of the `r3v3rs3.io/` annotations. The ports of the settings apply when no
/// annotation sets the ports. `None` when the Ingress has no ports.
fn annotation_fields(
    built: &mut Built,
    resource: &str,
    ingress: &Ingress,
    settings: &Settings,
) -> Option<Vec<(String, String)>> {
    let mut fields = Vec::new();
    for (key, value) in ingress.metadata.annotations.iter().flatten() {
        let Some(field) = key.strip_prefix(ANNOTATION_PREFIX) else {
            continue;
        };
        let top = field.split('.').next().unwrap_or_default();
        if RULE_FIELDS.contains(&top) {
            built.issue(resource, format!("the Ingress rules set this field: {key}"));
            continue;
        }
        fields.push((field.to_string(), value.clone()));
    }
    if !fields.iter().any(|(field, _)| field == "ports") {
        if settings.ports.is_empty() {
            built.issue(resource, format!("{ANNOTATION_PREFIX}ports is required"));
            return None;
        }
        fields.push(("ports".to_string(), settings.ports.join(",")));
    }
    Some(fields)
}

/// The paths of each host. The rules without a host and the default backend use the empty host.
fn host_rules(ingress: &Ingress) -> BTreeMap<&str, Vec<PathRule<'_>>> {
    let mut hosts = BTreeMap::<&str, Vec<PathRule>>::new();
    let Some(spec) = &ingress.spec else {
        return hosts;
    };
    for rule in spec.rules.iter().flatten() {
        let paths = rule
            .http
            .iter()
            .flat_map(|http| &http.paths)
            .map(|path| PathRule {
                path: path.path.as_deref(),
                path_type: Some(path.path_type.as_str()),
                backend: &path.backend,
            });
        hosts
            .entry(rule.host.as_deref().unwrap_or_default())
            .or_default()
            .extend(paths);
    }
    if let Some(backend) = &spec.default_backend {
        hosts.entry("").or_default().push(PathRule {
            path: None,
            path_type: None,
            backend,
        });
    }
    hosts
}

/// The route of a path. A backend without a ready endpoint keeps its route without servers, so
/// its requests do not reach another route.
fn route(built: &mut Built, target: &Target, host: &str, rule: PathRule) -> Option<Route> {
    let path = rule.path.filter(|path| !path.is_empty()).unwrap_or("/");
    let resource = &target.resource;
    if rule.path_type == Some("Exact") {
        let message = format!("{host}{path}: the Exact path type is not available, use Prefix");
        built.issue(resource, message);
        return None;
    }
    let Some(service) = &rule.backend.service else {
        built.issue(
            resource,
            format!("{host}{path}: only a Service backend is available"),
        );
        return None;
    };
    let servers = target
        .cluster
        .servers(&target.namespace, service)
        .unwrap_or_else(|message| {
            built.issue(resource, format!("{host}{path}: {message}"));
            Vec::new()
        });
    Some(Route {
        path: path.to_string(),
        servers,
        ip_filter: None,
        rate_limit: None,
        auth: None,
        headers: None,
        timeouts: None,
    })
}

/// The proxy of a host with the fields of the annotations and the routes of the rules.
fn definition(
    host: &str,
    name: &str,
    fields: &[(String, String)],
    routes: Vec<Route>,
) -> Result<ProxyDefinition, Vec<String>> {
    let proxy = if host.is_empty() {
        ANY_HOST.to_string()
    } else {
        host.replace('.', "_")
    };
    let prefix = format!("{}.http.{proxy}", labels::PREFIX);
    let mut pairs = vec![(
        format!("{prefix}.{}", PLACEHOLDER_ROUTE.0),
        PLACEHOLDER_ROUTE.1.to_string(),
    )];
    if !fields.iter().any(|(field, _)| field == "name") {
        pairs.push((format!("{prefix}.name"), name.to_string()));
    }
    pairs.extend(
        fields
            .iter()
            .map(|(field, value)| (format!("{prefix}.{field}"), value.clone())),
    );
    let parsed = labels::parse(pairs.iter().map(|(k, v)| (k.as_str(), v.as_str())), None);
    if !parsed.issues.is_empty() {
        return Err(parsed.issues);
    }
    let mut definition = parsed
        .definitions
        .into_iter()
        .next()
        .ok_or_else(|| vec![format!("http.{proxy}: the proxy has no fields")])?;
    let ProxyKind::Http(http) = &mut definition.kind else {
        return Err(vec![format!(
            "http.{proxy}: the proxy is not an HTTP proxy"
        )]);
    };
    http.routes = routes;
    if !host.is_empty() {
        let vhost = host
            .parse()
            .map_err(|err| vec![format!("http.{proxy}: {err}")])?;
        http.vhosts = vec![vhost];
    }
    Ok(definition)
}

fn tls_secrets(ingress: &Ingress) -> Vec<(String, String)> {
    let namespace = ingress.metadata.namespace.clone().unwrap_or_default();
    ingress
        .spec
        .iter()
        .flat_map(|spec| spec.tls.iter().flatten())
        .filter_map(|tls| Some((namespace.clone(), tls.secret_name.clone()?)))
        .collect()
}

fn add_secret(built: &mut Built, cluster: &Cluster, namespace: &str, name: &str) {
    let resource = format!("secret {namespace}/{name}");
    match cluster.certificate(namespace, name) {
        Ok(mut cert) => {
            cert.source = Some(DiscoverySource {
                provider: PROVIDER,
                resource,
            });
            built.certs.push(Arc::new(cert));
        }
        Err(message) => built.issue(&resource, message),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use r3v3rs3_api::subject_name::SubjectName;
    use serde::de::DeserializeOwned;
    use serde_json::{json, Value};

    fn object<T: DeserializeOwned>(value: Value) -> Arc<T> {
        Arc::new(serde_json::from_value(value).unwrap())
    }

    fn service(name: &str, ports: Value) -> Arc<Service> {
        object(json!({
            "apiVersion": "v1", "kind": "Service",
            "metadata": {"namespace": "default", "name": name},
            "spec": {"ports": ports},
        }))
    }

    fn slice(service: &str, ports: Value, endpoints: Value) -> Arc<EndpointSlice> {
        object(json!({
            "apiVersion": "discovery.k8s.io/v1", "kind": "EndpointSlice",
            "metadata": {"namespace": "default", "name": format!("{service}-abc"), "labels": {SERVICE_NAME_LABEL: service}},
            "addressType": "IPv4", "ports": ports, "endpoints": endpoints,
        }))
    }

    fn ingress(name: &str, annotations: Value, spec: Value) -> Arc<Ingress> {
        object(json!({
            "apiVersion": "networking.k8s.io/v1", "kind": "Ingress",
            "metadata": {"namespace": "default", "name": name, "annotations": annotations},
            "spec": spec,
        }))
    }

    fn backend(service: &str, port: Value) -> Value {
        json!({"service": {"name": service, "port": port}})
    }

    fn http(proxy: &crate::discovery::DiscoveredProxy) -> &r3v3rs3_api::proxy::HttpProxy {
        let ProxyKind::Http(http) = &proxy.definition.kind else {
            panic!("expected an HTTP proxy");
        };
        http
    }

    fn urls(route: &Route) -> Vec<String> {
        route.servers.iter().map(|s| s.url.to_string()).collect()
    }

    fn messages(built: &Built) -> Vec<String> {
        built
            .issues
            .iter()
            .map(|issue| format!("{}: {}", issue.resource, issue.message))
            .collect()
    }

    fn cluster_resources() -> Resources {
        Resources {
            services: vec![
                service("app", json!([{"name": "http", "port": 80}])),
                service(
                    "api",
                    json!([{"name": "tls", "port": 8443, "appProtocol": "https"}]),
                ),
                service("idle", json!([{"port": 80}])),
            ],
            slices: vec![
                slice(
                    "app",
                    json!([{"name": "http", "port": 8080}]),
                    json!([
                        {"addresses": ["10.0.0.1"], "conditions": {"ready": true}},
                        {"addresses": ["10.0.0.2"], "conditions": {"ready": false}},
                        {"addresses": ["fd00::3"]},
                    ]),
                ),
                slice(
                    "api",
                    json!([{"name": "tls", "port": 9443}]),
                    json!([{"addresses": ["10.0.0.4"]}]),
                ),
                slice(
                    "idle",
                    json!([{"port": 8080}]),
                    json!([{"addresses": ["10.0.0.5"], "conditions": {"ready": false}}]),
                ),
            ],
            ..Default::default()
        }
    }

    fn settings() -> Settings {
        Settings {
            ingress_class: "r3v3rs3".into(),
            ports: vec![],
        }
    }

    #[test]
    fn each_host_becomes_a_proxy_with_the_ready_endpoints() {
        let mut resources = cluster_resources();
        let rules = json!([
            {"host": "app.example.com", "http": {"paths": [
                {"path": "/", "pathType": "Prefix", "backend": backend("app", json!({"name": "http"}))},
                {"path": "/api", "pathType": "ImplementationSpecific", "backend": backend("api", json!({"number": 8443}))},
                {"path": "/exact", "pathType": "Exact", "backend": backend("app", json!({"name": "http"}))},
            ]}},
            {"http": {"paths": [
                {"path": "/status", "pathType": "Prefix", "backend": backend("app", json!({"number": 80}))},
            ]}},
        ]);
        resources.ingresses = vec![
            ingress(
                "app",
                json!({
                    "r3v3rs3.io/ports": "web",
                    "r3v3rs3.io/upgrade_insecure": "false",
                    "r3v3rs3.io/vhosts": "other.example.com",
                }),
                json!({"ingressClassName": "r3v3rs3", "rules": rules,
                       "defaultBackend": backend("app", json!({"name": "http"}))}),
            ),
            ingress(
                "legacy",
                json!({"kubernetes.io/ingress.class": "r3v3rs3", "r3v3rs3.io/ports": "web", "r3v3rs3.io/name": "Legacy"}),
                json!({"rules": [{"host": "legacy.example.com", "http": {"paths": [
                    {"path": "/", "pathType": "Prefix", "backend": backend("app", json!({"name": "http"}))},
                ]}}]}),
            ),
            ingress(
                "nginx",
                json!({"r3v3rs3.io/ports": "web"}),
                json!({"ingressClassName": "nginx", "defaultBackend": backend("app", json!({"name": "http"}))}),
            ),
        ];
        let built = build(&resources, &settings());
        let keys = built
            .proxies
            .iter()
            .map(|p| p.key.as_str())
            .collect::<Vec<_>>();
        assert_eq!(
            keys,
            [
                "ingress/default/app/http.*",
                "ingress/default/app/http.app_example_com",
                "ingress/default/legacy/http.legacy_example_com",
            ]
        );

        let any = &built.proxies[0];
        assert_eq!(any.definition.name, "default/app");
        assert_eq!(any.definition.ports, ["web"]);
        assert!(http(any).vhosts.is_empty());
        let paths = http(any)
            .routes
            .iter()
            .map(|r| r.path.as_str())
            .collect::<Vec<_>>();
        assert_eq!(paths, ["/status", "/"]);

        let host = &built.proxies[1];
        assert_eq!(host.source.resource, "ingress default/app");
        assert_eq!(host.definition.name, "default/app app.example.com");
        let host_http = http(host);
        assert!(!host_http.upgrade_insecure);
        assert_eq!(host_http.vhosts[0].to_string(), "app.example.com");
        assert_eq!(host_http.routes.len(), 2);
        assert_eq!(
            urls(&host_http.routes[0]),
            ["http://10.0.0.1:8080/", "http://[fd00::3]:8080/"]
        );
        assert_eq!(host_http.routes[1].path, "/api");
        assert_eq!(urls(&host_http.routes[1]), ["https://10.0.0.4:9443/"]);
        assert_eq!(built.proxies[2].definition.name, "Legacy");

        assert_eq!(
            messages(&built),
            [
                "ingress default/app: the Ingress rules set this field: r3v3rs3.io/vhosts",
                "ingress default/app: app.example.com/exact: the Exact path type is not available, use Prefix",
            ]
        );
        assert!(built.certs.is_empty());
    }

    #[test]
    fn missing_backends_and_ports_are_issues() {
        let mut resources = cluster_resources();
        let paths = json!([
            {"path": "/idle", "pathType": "Prefix", "backend": backend("idle", json!({"number": 80}))},
            {"path": "/gone", "pathType": "Prefix", "backend": backend("gone", json!({"number": 80}))},
            {"path": "/port", "pathType": "Prefix", "backend": backend("app", json!({"name": "grpc"}))},
            {"path": "/bucket", "pathType": "Prefix", "backend": {"resource": {"kind": "Bucket", "name": "assets"}}},
        ]);
        resources.ingresses = vec![
            ingress(
                "broken",
                json!({}),
                json!({"rules": [{"host": "b.example.com", "http": {"paths": paths}}]}),
            ),
            ingress(
                "typo",
                json!({"r3v3rs3.io/timeout": "5s"}),
                json!({"defaultBackend": backend("app", json!({"name": "http"}))}),
            ),
        ];
        let settings = Settings {
            ports: vec!["web".into()],
            ..Default::default()
        };
        let built = build(&resources, &settings);
        assert_eq!(built.proxies.len(), 1);
        let proxy = http(&built.proxies[0]);
        assert_eq!(built.proxies[0].definition.ports, ["web"]);
        let routes = proxy
            .routes
            .iter()
            .map(|route| (route.path.as_str(), route.servers.len()))
            .collect::<Vec<_>>();
        assert_eq!(routes, [("/idle", 0), ("/gone", 0), ("/port", 0)]);
        assert_eq!(
            messages(&built),
            [
                "ingress default/broken: b.example.com/idle: service default/idle has no ready endpoint",
                "ingress default/broken: b.example.com/gone: service not found: default/gone",
                "ingress default/broken: b.example.com/port: service app has no port grpc",
                "ingress default/broken: b.example.com/bucket: only a Service backend is available",
                "ingress default/typo: http.*: unknown keys: timeout",
            ]
        );

        let without_ports = build(&resources, &Settings::default());
        assert!(without_ports.proxies.is_empty());
        assert_eq!(
            messages(&without_ports)[0],
            "ingress default/broken: r3v3rs3.io/ports is required"
        );
    }

    #[test]
    fn tls_secrets_of_the_selected_ingresses_become_certificates() {
        let ca = Cert::new_ca().unwrap();
        let san = [SubjectName::from_str("app.example.com").unwrap()];
        let cert = Cert::new_self_signed(&san, &ca).unwrap();
        let data = |cert: &Cert| {
            json!({
                "tls.crt": base64_encode(&cert.pem_chain),
                "tls.key": base64_encode(cert.pem_key.as_deref().unwrap()),
            })
        };
        let secret = |name: &str, kind: &str, data: Value| -> Arc<Secret> {
            object(json!({
                "apiVersion": "v1", "kind": "Secret",
                "metadata": {"namespace": "default", "name": name},
                "type": kind, "data": data,
            }))
        };
        let mut resources = cluster_resources();
        resources.secrets = vec![
            secret("app-tls", TLS_SECRET_TYPE, data(&cert)),
            secret("opaque", "Opaque", data(&cert)),
            secret("unused-tls", TLS_SECRET_TYPE, data(&cert)),
        ];
        let tls = json!([{"secretName": "app-tls"}, {"secretName": "opaque"}, {"hosts": ["x.example.com"]}]);
        resources.ingresses = vec![ingress(
            "app",
            json!({"r3v3rs3.io/ports": "web"}),
            json!({"ingressClassName": "r3v3rs3", "tls": tls, "defaultBackend": backend("app", json!({"name": "http"}))}),
        )];
        let built = build(&resources, &settings());
        assert_eq!(built.certs.len(), 1);
        assert_eq!(built.certs[0].id, cert.id);
        assert_eq!(
            built.certs[0].source,
            Some(DiscoverySource {
                provider: PROVIDER,
                resource: "secret default/app-tls".into(),
            })
        );
        assert_eq!(
            messages(&built),
            ["secret default/opaque: the TLS secret is not found"]
        );
    }

    fn base64_encode(bytes: &[u8]) -> String {
        use base64::prelude::{Engine as _, BASE64_STANDARD};
        BASE64_STANDARD.encode(bytes)
    }

    use std::str::FromStr;
}
