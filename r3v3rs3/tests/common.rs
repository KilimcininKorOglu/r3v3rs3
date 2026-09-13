#![allow(dead_code)]

use futures::Future;
use hickory_resolver::{config::LookupIpStrategy, system_conf::read_system_conf, AsyncResolver};
use net2::{TcpBuilder, UdpBuilder};
use r3v3rs3::{
    cdn::CdnRanges,
    certs::{acme::AcmeEntry, Cert},
    config::{new_appinfo, storage::Storage},
    server::{Server, ServerChannels},
};
use r3v3rs3_api::{
    app::AppConfig,
    auth::{Account, LoginMethod, LoginRequest, LoginResponse},
    error::Error,
    id::ShortId,
    multiaddr::Multiaddr,
    policy::IpFilter,
    port::{Port, PortEntry},
    proxy::{HttpProxy, Proxy, ProxyEntry, ProxyKind, Route, Server as UpstreamUrl},
};
use std::{
    collections::HashMap,
    net::{SocketAddr, ToSocketAddrs},
    path::Path,
    sync::Arc,
};
use tokio::sync::Mutex;
use url::Url;

pub async fn with_server<S, F, O>(s: S, func: F) -> anyhow::Result<()>
where
    S: Storage,
    F: FnOnce(ServerChannels) -> O,
    O: Future<Output = anyhow::Result<()>> + Send + 'static,
{
    let app_info = new_appinfo(Path::new("."), Path::new("."));
    let (server, channels) = Server::new(app_info, s).await;
    let event_send = channels.event.clone();
    let task = tokio::spawn(server.start());
    func(channels).await?;
    event_send.send(r3v3rs3_api::event::ServerEvent::Shutdown)?;
    task.await??;
    Ok(())
}

/// Waits until a TCP listener accepts connections on the address.
pub async fn wait_for_listener(addr: SocketAddr) -> anyhow::Result<()> {
    for _ in 0..50 {
        if tokio::net::TcpStream::connect(addr).await.is_ok() {
            return Ok(());
        }
        tokio::time::sleep(std::time::Duration::from_millis(100)).await;
    }
    anyhow::bail!("server did not start listening on {addr}")
}

#[derive(Debug, Default, Clone)]
pub struct TestStorage {
    inner: Arc<Mutex<Inner>>,
}

#[derive(Debug, Default)]
struct Inner {
    pub config: AppConfig,
    pub ports: Vec<PortEntry>,
    pub proxies: Vec<ProxyEntry>,
    pub certs: HashMap<ShortId, Arc<Cert>>,
    pub acems: HashMap<ShortId, AcmeEntry>,
    pub accounts: HashMap<String, String>,
    pub cdn_ranges: Option<CdnRanges>,
}

impl TestStorage {
    pub fn builder() -> TestStorageBuilder {
        TestStorageBuilder::new()
    }
}

#[async_trait::async_trait]
impl Storage for TestStorage {
    async fn save_app_config(&self, config: &AppConfig) {
        self.inner.lock().await.config.clone_from(config);
    }

    async fn load_app_config(&self) -> AppConfig {
        self.inner.lock().await.config.clone()
    }

    async fn save_ports(&self, entries: &[PortEntry]) {
        self.inner.lock().await.ports = entries.to_vec();
    }

    async fn load_ports(&self) -> Vec<PortEntry> {
        self.inner.lock().await.ports.clone()
    }

    async fn load_proxies(&self) -> Vec<ProxyEntry> {
        self.inner.lock().await.proxies.clone()
    }

    async fn save_proxies(&self, proxies: &[ProxyEntry]) {
        self.inner.lock().await.proxies = proxies.to_vec();
    }

    async fn save_cert(&self, cert: &Cert) {
        self.inner
            .lock()
            .await
            .certs
            .insert(cert.id(), Arc::new(cert.clone()));
    }

    async fn save_acme(&self, acme: &AcmeEntry) {
        self.inner
            .lock()
            .await
            .acems
            .insert(acme.id(), acme.clone());
    }

    async fn delete_acme(&self, id: ShortId) {
        self.inner.lock().await.acems.remove(&id);
    }

    async fn delete_cert(&self, id: ShortId) {
        self.inner.lock().await.certs.remove(&id);
    }

    async fn load_acmes(&self) -> Vec<AcmeEntry> {
        self.inner.lock().await.acems.values().cloned().collect()
    }

    async fn load_certs(&self) -> Vec<Arc<Cert>> {
        self.inner.lock().await.certs.values().cloned().collect()
    }

    async fn add_account(&self, name: &str, password: &str, _totp: bool) -> Result<Account, Error> {
        self.inner
            .lock()
            .await
            .accounts
            .insert(name.to_string(), password.to_string());
        Ok(Account {
            password: password.to_string(),
            totp: None,
        })
    }

    async fn verify_account(&self, request: LoginRequest) -> Result<LoginResponse, Error> {
        let password = match request.method {
            LoginMethod::Password { password } => password,
            _ => return Err(Error::InvalidLoginCredentials),
        };
        let inner = self.inner.lock().await;
        if let Some(p) = inner.accounts.get(&request.username) {
            if *p == password {
                return Ok(LoginResponse::Success);
            }
        }
        Err(Error::InvalidLoginCredentials)
    }

    async fn save_cdn_ranges(&self, ranges: &CdnRanges) {
        self.inner.lock().await.cdn_ranges = Some(ranges.clone());
    }

    async fn load_cdn_ranges(&self) -> Option<CdnRanges> {
        self.inner.lock().await.cdn_ranges.clone()
    }
}

#[derive(Debug, Default)]
pub struct TestStorageBuilder {
    inner: Inner,
}

impl TestStorageBuilder {
    pub fn new() -> Self {
        Self {
            inner: Inner::default(),
        }
    }

    pub fn config(mut self, config: AppConfig) -> Self {
        self.inner.config = config;
        self
    }

    pub fn ports(mut self, ports: Vec<PortEntry>) -> Self {
        self.inner.ports = ports;
        self
    }

    pub fn proxies(mut self, proxies: Vec<ProxyEntry>) -> Self {
        self.inner.proxies = proxies;
        self
    }

    pub fn certs(mut self, certs: HashMap<ShortId, Arc<Cert>>) -> Self {
        self.inner.certs = certs;
        self
    }

    pub fn acems(mut self, acems: HashMap<ShortId, AcmeEntry>) -> Self {
        self.inner.acems = acems;
        self
    }

    pub fn accounts(mut self, accounts: HashMap<String, String>) -> Self {
        self.inner.accounts = accounts;
        self
    }

    pub fn build(self) -> TestStorage {
        TestStorage {
            inner: Arc::new(Mutex::new(self.inner)),
        }
    }
}

pub fn port_entry(id: &str, listen: Multiaddr) -> PortEntry {
    PortEntry {
        id: id.parse().unwrap(),
        port: Port {
            active: true,
            name: String::new(),
            listen,
            opts: Default::default(),
        },
    }
}

pub fn http_port_entry(id: &str, port: &TestPort) -> PortEntry {
    port_entry(id, port.multiaddr_http())
}

/// Signs in to the admin API as `admin` with the password `secret` and returns the session cookie.
pub async fn admin_session_cookie(addr: SocketAddr) -> anyhow::Result<String> {
    let res = reqwest::Client::new()
        .post(format!("http://{addr}/api/login"))
        .json(&LoginRequest {
            username: "admin".to_string(),
            method: LoginMethod::Password {
                password: "secret".to_string(),
            },
            insecure: true,
        })
        .send()
        .await?
        .error_for_status()?;
    let cookie = res
        .headers()
        .get(reqwest::header::SET_COOKIE)
        .and_then(|value| value.to_str().ok())
        .and_then(|value| value.split(';').next())
        .ok_or_else(|| anyhow::anyhow!("login response has no session cookie"))?;
    Ok(cookie.to_string())
}

pub fn proxy_entry(id: &str, port_id: &str, kind: ProxyKind) -> ProxyEntry {
    ProxyEntry {
        id: id.parse().unwrap(),
        proxy: Proxy {
            ports: vec![port_id.parse().unwrap()],
            kind,
            ..Default::default()
        },
    }
}

pub fn http_proxy_entry(id: &str, port_id: &str, http: HttpProxy) -> ProxyEntry {
    proxy_entry(id, port_id, ProxyKind::Http(Box::new(http)))
}

pub fn http_route(path: &str, upstream: &str, ip_filter: Option<IpFilter>) -> Route {
    Route {
        path: path.into(),
        servers: vec![UpstreamUrl {
            url: upstream.parse().unwrap(),
        }],
        ip_filter,
        rate_limit: None,
        auth: None,
        headers: None,
    }
}

pub async fn alloc_tcp_port() -> Result<TestPort, std::io::Error> {
    let (conf, mut opts) = read_system_conf().unwrap_or_default();
    opts.ip_strategy = LookupIpStrategy::Ipv4AndIpv6;
    let resolver = AsyncResolver::tokio(conf, opts);

    let addr = "localhost:0".to_socket_addrs().unwrap().next().unwrap();
    let addr = SocketAddr::new(
        resolver
            .lookup_ip("localhost")
            .await
            .unwrap()
            .iter()
            .next()
            .unwrap(),
        addr.port(),
    );
    let addr = if addr.is_ipv4() {
        TcpBuilder::new_v4()?
    } else {
        TcpBuilder::new_v6()?
    }
    .reuse_address(true)?
    .bind(addr)?
    .local_addr()?;
    Ok(TestPort { addr })
}

pub async fn alloc_udp_port() -> Result<TestPort, std::io::Error> {
    let (conf, mut opts) = read_system_conf().unwrap_or_default();
    opts.ip_strategy = LookupIpStrategy::Ipv4AndIpv6;
    let resolver = AsyncResolver::tokio(conf, opts);

    let addr = "localhost:0".to_socket_addrs().unwrap().next().unwrap();
    let addr = SocketAddr::new(
        resolver
            .lookup_ip("localhost")
            .await
            .unwrap()
            .iter()
            .next()
            .unwrap(),
        addr.port(),
    );
    let addr = if addr.is_ipv4() {
        UdpBuilder::new_v4()?
    } else {
        UdpBuilder::new_v6()?
    }
    .reuse_address(true)?
    .bind(addr)?
    .local_addr()?;
    Ok(TestPort { addr })
}

pub struct TestPort {
    addr: SocketAddr,
}

impl TestPort {
    pub fn socket_addr(&self) -> SocketAddr {
        self.addr
    }

    pub fn multiaddr_http(&self) -> Multiaddr {
        let protocol = if self.addr.is_ipv4() { "ip4" } else { "ip6" };
        let addr = self.addr.ip();
        format!("/{protocol}/{addr}/tcp/{}/http", self.addr.port())
            .parse()
            .unwrap()
    }

    pub fn multiaddr_https(&self) -> Multiaddr {
        let protocol = if self.addr.is_ipv4() { "ip4" } else { "ip6" };
        let addr = self.addr.ip();
        format!("/{protocol}/{addr}/tcp/{}/https", self.addr.port())
            .parse()
            .unwrap()
    }

    pub fn multiaddr_tcp(&self) -> Multiaddr {
        let protocol = if self.addr.is_ipv4() { "ip4" } else { "ip6" };
        let addr = self.addr.ip();
        format!("/{protocol}/{addr}/tcp/{}", self.addr.port())
            .parse()
            .unwrap()
    }

    pub fn multiaddr_udp(&self) -> Multiaddr {
        let protocol = if self.addr.is_ipv4() { "ip4" } else { "ip6" };
        let addr = self.addr.ip();
        format!("/{protocol}/{addr}/udp/{}", self.addr.port())
            .parse()
            .unwrap()
    }

    pub fn multiaddr_tls(&self) -> Multiaddr {
        let protocol = if self.addr.is_ipv4() { "ip4" } else { "ip6" };
        let addr = self.addr.ip();
        format!("/{protocol}/{addr}/tcp/{}/tls", self.addr.port())
            .parse()
            .unwrap()
    }

    pub fn http_url(&self, path: &str) -> Url {
        format!("http://localhost:{}{path}", self.addr.port())
            .parse()
            .unwrap()
    }

    pub fn https_url(&self, path: &str) -> Url {
        format!("https://localhost:{}{path}", self.addr.port())
            .parse()
            .unwrap()
    }
}
