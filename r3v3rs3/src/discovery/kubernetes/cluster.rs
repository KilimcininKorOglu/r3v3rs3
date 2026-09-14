//! The watched resources, and the Services, EndpointSlices and TLS secrets that the proxies of the
//! Kubernetes provider use.

use super::crd::R3v3rs3Proxy;
use crate::certs::Cert;
use k8s_openapi::api::core::v1::{Secret, Service, ServicePort};
use k8s_openapi::api::discovery::v1::EndpointSlice;
use k8s_openapi::api::networking::v1::{Ingress, IngressServiceBackend};
use k8s_openapi::apimachinery::pkg::apis::meta::v1::ObjectMeta;
use r3v3rs3_api::cert::CertKind;
use r3v3rs3_api::proxy::Server;
use std::collections::{BTreeSet, HashMap};
use std::net::IpAddr;
use std::sync::Arc;

pub const SERVICE_NAME_LABEL: &str = "kubernetes.io/service-name";
pub const TLS_SECRET_TYPE: &str = "kubernetes.io/tls";

/// The resources that the proxies use.
#[derive(Debug, Default)]
pub struct Resources {
    pub ingresses: Vec<Arc<Ingress>>,
    pub proxies: Vec<Arc<R3v3rs3Proxy>>,
    pub services: Vec<Arc<Service>>,
    pub slices: Vec<Arc<EndpointSlice>>,
    pub secrets: Vec<Arc<Secret>>,
}

pub fn object_key(meta: &ObjectMeta) -> (String, String) {
    (
        meta.namespace.clone().unwrap_or_default(),
        meta.name.clone().unwrap_or_default(),
    )
}

fn object_ref(meta: &ObjectMeta) -> Option<(&str, &str)> {
    Some((meta.namespace.as_deref()?, meta.name.as_deref()?))
}

/// The ready endpoints of a Service port.
#[derive(Debug, PartialEq, Eq)]
pub struct Endpoints {
    /// The Service port uses HTTPS.
    pub https: bool,
    /// The host and the port of each ready endpoint.
    pub addresses: BTreeSet<(String, u16)>,
}

/// The Services, the EndpointSlices and the TLS secrets by namespace and name.
pub struct Cluster<'a> {
    services: HashMap<(&'a str, &'a str), &'a Service>,
    /// The slices of each Service.
    slices: HashMap<(&'a str, &'a str), Vec<&'a EndpointSlice>>,
    secrets: HashMap<(&'a str, &'a str), &'a Secret>,
}

impl<'a> Cluster<'a> {
    pub fn new(resources: &'a Resources) -> Self {
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

    /// The ready endpoints of the Service port that the backend selects.
    pub fn endpoints(
        &self,
        namespace: &str,
        backend: &IngressServiceBackend,
    ) -> Result<Endpoints, String> {
        let name = backend.name.as_str();
        let service = self
            .services
            .get(&(namespace, name))
            .ok_or_else(|| format!("service not found: {namespace}/{name}"))?;
        let port = service_port(service, backend)?;
        let mut addresses = BTreeSet::new();
        for slice in self.slices.get(&(namespace, name)).into_iter().flatten() {
            let Some(number) = endpoint_port(slice, port) else {
                continue;
            };
            addresses.extend(ready_addresses(slice).map(|host| (host.to_string(), number)));
        }
        if addresses.is_empty() {
            return Err(format!("service {namespace}/{name} has no ready endpoint"));
        }
        Ok(Endpoints {
            https: is_https(port),
            addresses,
        })
    }

    /// The HTTP servers of the ready endpoints of a Service port.
    pub fn servers(
        &self,
        namespace: &str,
        backend: &IngressServiceBackend,
    ) -> Result<Vec<Server>, String> {
        let endpoints = self.endpoints(namespace, backend)?;
        let scheme = if endpoints.https { "https" } else { "http" };
        endpoints
            .addresses
            .iter()
            .map(|(host, port)| {
                let url = server_url(scheme, host, *port);
                url.parse()
                    .map(|url| Server { url })
                    .map_err(|err| format!("invalid endpoint address {url}: {err}"))
            })
            .collect()
    }

    /// The certificate and the private key of a TLS secret.
    pub fn certificate(&self, namespace: &str, name: &str) -> Result<Cert, String> {
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

/// The resources and the result helpers of the provider tests.
#[cfg(test)]
pub mod fixtures {
    use super::*;
    use crate::discovery::{Built, DiscoveredProxy};
    use r3v3rs3_api::proxy::{HttpProxy, ProxyKind, Route};
    use serde::de::DeserializeOwned;
    use serde_json::{json, Value};

    pub fn object<T: DeserializeOwned>(value: Value) -> Arc<T> {
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

    /// The Service `app` with two ready endpoints and one endpoint that is not ready, the HTTPS
    /// Service `api`, and the Service `idle` without a ready endpoint.
    pub fn cluster_resources() -> Resources {
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

    pub fn http(proxy: &DiscoveredProxy) -> &HttpProxy {
        let ProxyKind::Http(http) = &proxy.definition.kind else {
            panic!("expected an HTTP proxy");
        };
        http
    }

    pub fn urls(route: &Route) -> Vec<String> {
        route.servers.iter().map(|s| s.url.to_string()).collect()
    }

    pub fn messages(built: &Built) -> Vec<String> {
        built
            .issues
            .iter()
            .map(|issue| format!("{}: {}", issue.resource, issue.message))
            .collect()
    }
}
