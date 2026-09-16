#![allow(dead_code)]

pub mod cluster;
pub mod dns;
pub mod e2e;
pub mod kv;

use futures::Future;
use hickory_resolver::{config::LookupIpStrategy, system_conf::read_system_conf, AsyncResolver};
use net2::{TcpBuilder, UdpBuilder};
use r3v3rs3::{
    accounts::Caller,
    cdn::CdnRanges,
    certs::{acme::AcmeEntry, Cert},
    command::ServerCommand,
    config::{new_appinfo, storage::Storage},
    server::{
        rpc::{discovery::GetDiscoveryStatus, ErasedRpcMethod, RpcMethod, RpcWrapper},
        Server, ServerChannels,
    },
};
use r3v3rs3_api::{
    app::AppConfig,
    auth::{Account, LoginMethod, LoginRequest, LoginResponse, Role},
    discovery::DiscoveryStatus,
    error::Error,
    id::ShortId,
    multiaddr::Multiaddr,
    policy::IpFilter,
    port::{Port, PortEntry},
    proxy::{HttpProxy, Proxy, ProxyEntry, ProxyKind, Route, Server as UpstreamUrl},
};
use std::{
    collections::{BTreeSet, HashMap},
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
    // The server writes its audit log in the log directory, so the tests use a temporary one.
    let log_dir = std::env::temp_dir().join(format!("r3v3rs3-test-logs-{}", std::process::id()));
    std::fs::create_dir_all(&log_dir)?;
    let app_info = new_appinfo(Path::new("."), &log_dir);
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
    let listening = || async move { Ok(tokio::net::TcpStream::connect(addr).await.is_ok()) };
    wait_until(
        listening,
        &format!("server did not start listening on {addr}"),
    )
    .await
}

/// Checks every 100 ms until `ready` returns true. Fails with `failure` after 50 checks.
pub async fn wait_until<F, Fut>(mut ready: F, failure: &str) -> anyhow::Result<()>
where
    F: FnMut() -> Fut,
    Fut: std::future::Future<Output = anyhow::Result<bool>>,
{
    for _ in 0..50 {
        if ready().await? {
            return Ok(());
        }
        tokio::time::sleep(std::time::Duration::from_millis(100)).await;
    }
    anyhow::bail!("{failure}")
}

/// Calls an RPC method through the server command channel and returns its result.
pub async fn call<M>(
    channels: &mut ServerChannels,
    method: M,
) -> anyhow::Result<Result<M::Output, Error>>
where
    M: RpcMethod + 'static,
{
    call_as(channels, Caller::system(), method).await
}

/// Calls an RPC method for an account and returns its result.
pub async fn call_as<M>(
    channels: &mut ServerChannels,
    caller: Caller,
    method: M,
) -> anyhow::Result<Result<M::Output, Error>>
where
    M: RpcMethod + 'static,
{
    let arg = Box::new(RpcWrapper::new(method)) as Box<dyn ErasedRpcMethod>;
    channels
        .command
        .send(ServerCommand::CallMethod { id: 1, arg, caller })
        .await?;
    let callback = channels
        .callback
        .recv()
        .await
        .ok_or_else(|| anyhow::anyhow!("callback channel closed"))?;
    Ok(callback.result.and_then(|value| {
        value
            .downcast::<M::Output>()
            .map(|value| *value)
            .map_err(|_| Error::FailedToInvokeRpc)
    }))
}

/// Reads the discovery statuses until `done` accepts them.
pub async fn wait_for_discovery(
    channels: &mut ServerChannels,
    done: impl Fn(&[DiscoveryStatus]) -> bool,
) -> anyhow::Result<Vec<DiscoveryStatus>> {
    wait_for_rpc(channels, || GetDiscoveryStatus, |statuses| done(statuses)).await
}

/// Calls the method until its output passes the check.
pub async fn wait_for_rpc<M>(
    channels: &mut ServerChannels,
    method: impl Fn() -> M,
    done: impl Fn(&M::Output) -> bool,
) -> anyhow::Result<M::Output>
where
    M: RpcMethod + 'static,
    M::Output: std::fmt::Debug,
{
    let mut output = call(channels, method()).await??;
    for _ in 0..100 {
        if done(&output) {
            return Ok(output);
        }
        tokio::time::sleep(std::time::Duration::from_millis(100)).await;
        output = call(channels, method()).await??;
    }
    anyhow::bail!("unexpected output: {output:?}")
}

/// Sends requests until the proxy answers with the expected status.
pub async fn wait_for_status(url: &str, expected: u16) -> anyhow::Result<String> {
    wait_for_host_status(url, None, expected).await
}

/// Sends requests with the optional `Host` header until `accept` accepts the status and the body
/// of a response. Returns the body, or the last status and body in the error.
async fn wait_for_response(
    url: &str,
    host: Option<&str>,
    attempts: usize,
    accept: impl Fn(u16, &str) -> bool,
) -> anyhow::Result<String> {
    let client = reqwest::Client::new();
    let mut last = None;
    for _ in 0..attempts {
        let mut request = client.get(url);
        if let Some(host) = host {
            request = request.header(reqwest::header::HOST, host);
        }
        if let Ok(res) = request.send().await {
            let status = res.status().as_u16();
            let body = res.text().await?;
            if accept(status, &body) {
                return Ok(body);
            }
            last = Some((status, body));
        }
        tokio::time::sleep(std::time::Duration::from_millis(100)).await;
    }
    anyhow::bail!("{url} (host {host:?}) did not answer as expected, last response: {last:?}")
}

/// Sends requests with the `Host` header until the proxy answers with the expected status.
pub async fn wait_for_host_status(
    url: &str,
    host: Option<&str>,
    expected: u16,
) -> anyhow::Result<String> {
    wait_for_response(url, host, 50, |status, _| status == expected).await
}

/// Sends requests with the `Host` header until the proxy answers with the expected body.
pub async fn wait_for_host_body(url: &str, host: &str, expected: &str) -> anyhow::Result<()> {
    wait_for_response(url, Some(host), 100, |_, body| body == expected).await?;
    Ok(())
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
    pub access_lists: Vec<r3v3rs3_api::access_list::AccessListEntry>,
    pub certs: HashMap<ShortId, Arc<Cert>>,
    pub acems: HashMap<ShortId, AcmeEntry>,
    pub accounts: HashMap<String, Account>,
    pub cdn_ranges: Option<CdnRanges>,
    /// The error of `ensure_writable`.
    pub write_error: Option<Error>,
    /// The error of every save and delete.
    pub save_error: Option<Error>,
}

fn fail_with(error: &Option<Error>) -> Result<(), Error> {
    error.clone().map_or(Ok(()), Err)
}

/// An account that keeps the password itself instead of a hash.
fn plain_account(password: &str, role: Role, proxies: Option<BTreeSet<ShortId>>) -> Account {
    Account {
        password: password.to_string(),
        role,
        proxies,
        ..Default::default()
    }
}

impl TestStorage {
    pub fn builder() -> TestStorageBuilder {
        TestStorageBuilder::new()
    }

    /// Applies `change` unless a save error is set.
    async fn save(&self, change: impl FnOnce(&mut Inner) + Send) -> Result<(), Error> {
        let mut inner = self.inner.lock().await;
        fail_with(&inner.save_error)?;
        change(&mut inner);
        Ok(())
    }

    pub async fn set_write_error(&self, error: Option<Error>) {
        self.inner.lock().await.write_error = error;
    }

    pub async fn set_save_error(&self, error: Option<Error>) {
        self.inner.lock().await.save_error = error;
    }
}

#[async_trait::async_trait]
impl Storage for TestStorage {
    async fn ensure_writable(&self) -> Result<(), Error> {
        fail_with(&self.inner.lock().await.write_error)
    }

    async fn save_app_config(&self, config: &AppConfig) -> Result<(), Error> {
        self.save(|inner| inner.config.clone_from(config)).await
    }

    async fn load_app_config(&self) -> AppConfig {
        self.inner.lock().await.config.clone()
    }

    async fn save_ports(&self, entries: &[PortEntry]) -> Result<(), Error> {
        self.save(|inner| inner.ports = entries.to_vec()).await
    }

    async fn load_ports(&self) -> Vec<PortEntry> {
        self.inner.lock().await.ports.clone()
    }

    async fn load_proxies(&self) -> Vec<ProxyEntry> {
        self.inner.lock().await.proxies.clone()
    }

    async fn save_proxies(&self, proxies: &[ProxyEntry]) -> Result<(), Error> {
        self.save(|inner| inner.proxies = proxies.to_vec()).await
    }

    async fn load_access_lists(&self) -> Vec<r3v3rs3_api::access_list::AccessListEntry> {
        self.inner.lock().await.access_lists.clone()
    }

    async fn save_access_lists(
        &self,
        lists: &[r3v3rs3_api::access_list::AccessListEntry],
    ) -> Result<(), Error> {
        self.save(|inner| inner.access_lists = lists.to_vec()).await
    }

    async fn save_cert(&self, cert: &Cert) -> Result<(), Error> {
        let cert = Arc::new(cert.clone());
        self.save(|inner| {
            inner.certs.insert(cert.id(), cert);
        })
        .await
    }

    async fn save_acme(&self, acme: &AcmeEntry) -> Result<(), Error> {
        self.save(|inner| {
            inner.acems.insert(acme.id(), acme.clone());
        })
        .await
    }

    async fn delete_acme(&self, id: ShortId) -> Result<(), Error> {
        self.save(|inner| {
            inner.acems.remove(&id);
        })
        .await
    }

    async fn delete_cert(&self, id: ShortId) -> Result<(), Error> {
        self.save(|inner| {
            inner.certs.remove(&id);
        })
        .await
    }

    async fn load_acmes(&self) -> Vec<AcmeEntry> {
        self.inner.lock().await.acems.values().cloned().collect()
    }

    async fn load_certs(&self) -> Vec<Arc<Cert>> {
        self.inner.lock().await.certs.values().cloned().collect()
    }

    async fn add_account(
        &self,
        name: &str,
        password: &str,
        _totp: bool,
        role: Role,
    ) -> Result<Account, Error> {
        let account = plain_account(password, role, None);
        self.inner
            .lock()
            .await
            .accounts
            .insert(name.to_string(), account.clone());
        Ok(account)
    }

    async fn verify_account(&self, request: LoginRequest) -> Result<LoginResponse, Error> {
        let password = match request.method {
            LoginMethod::Password { password } => password,
            _ => return Err(Error::InvalidLoginCredentials),
        };
        let inner = self.inner.lock().await;
        if let Some(account) = inner.accounts.get(&request.username) {
            if password_matches(&account.password, &password) {
                return Ok(LoginResponse::Success);
            }
        }
        Err(Error::InvalidLoginCredentials)
    }

    async fn load_accounts(&self) -> Result<HashMap<String, Account>, Error> {
        Ok(self.inner.lock().await.accounts.clone())
    }

    async fn save_accounts(&self, accounts: &HashMap<String, Account>) -> Result<(), Error> {
        self.save(|inner| inner.accounts = accounts.clone()).await
    }

    async fn save_cdn_ranges(&self, ranges: &CdnRanges) -> Result<(), Error> {
        self.save(|inner| inner.cdn_ranges = Some(ranges.clone()))
            .await
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

    pub fn access_lists(mut self, lists: Vec<r3v3rs3_api::access_list::AccessListEntry>) -> Self {
        self.inner.access_lists = lists;
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

    /// Admin accounts with their passwords.
    pub fn accounts(mut self, accounts: HashMap<String, String>) -> Self {
        self.inner.accounts = accounts
            .into_iter()
            .map(|(name, password)| {
                let account = Account {
                    password,
                    ..Default::default()
                };
                (name, account)
            })
            .collect();
        self
    }

    /// Adds an account with a role and an optional proxy list.
    pub fn account(
        mut self,
        name: &str,
        password: &str,
        role: Role,
        proxies: Option<BTreeSet<ShortId>>,
    ) -> Self {
        let account = plain_account(password, role, proxies);
        self.inner.accounts.insert(name.to_string(), account);
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

/// The builder stores plain passwords, and the admin API stores Argon2 hashes.
fn password_matches(stored: &str, password: &str) -> bool {
    match argon2::PasswordHash::new(stored) {
        Ok(hash) => argon2::PasswordVerifier::verify_password(
            &argon2::Argon2::default(),
            password.as_bytes(),
            &hash,
        )
        .is_ok(),
        Err(_) => stored == password,
    }
}

/// Signs in to the admin API as `admin` with the password `secret` and returns the session cookie.
pub async fn admin_session_cookie(addr: SocketAddr) -> anyhow::Result<String> {
    session_cookie(addr, "admin", "secret").await
}

/// Signs in to the admin API and returns the session cookie.
pub async fn session_cookie(
    addr: SocketAddr,
    username: &str,
    password: &str,
) -> anyhow::Result<String> {
    let res = reqwest::Client::new()
        .post(format!("http://{addr}/api/login"))
        .json(&LoginRequest {
            username: username.to_string(),
            method: LoginMethod::Password {
                password: password.to_string(),
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

/// Sends a request to the admin API and returns the status and the body.
pub async fn send(
    addr: SocketAddr,
    method: reqwest::Method,
    path: &str,
    cookie: &str,
    body: Option<serde_json::Value>,
) -> anyhow::Result<(u16, String)> {
    let mut request = reqwest::Client::new()
        .request(method, format!("http://{addr}{path}"))
        .header(reqwest::header::COOKIE, cookie);
    if let Some(body) = body {
        request = request.json(&body);
    }
    let response = request.send().await?;
    Ok((response.status().as_u16(), response.text().await?))
}

/// Signs in again after the rate limit of the sign-in endpoint allows another request.
pub async fn login_when_allowed(
    addr: SocketAddr,
    username: &str,
    password: &str,
) -> anyhow::Result<String> {
    tokio::time::timeout(std::time::Duration::from_secs(10), async {
        loop {
            match session_cookie(addr, username, password).await {
                Err(err) if err.to_string().contains("429") => {
                    tokio::time::sleep(std::time::Duration::from_millis(500)).await;
                }
                result => return result,
            }
        }
    })
    .await
    .map_err(|_| anyhow::anyhow!("the sign-in stayed rate limited"))?
}

pub fn proxy_entry(id: &str, port_id: &str, kind: ProxyKind) -> ProxyEntry {
    ProxyEntry {
        id: id.parse().unwrap(),
        proxy: Proxy {
            ports: vec![port_id.parse().unwrap()],
            kind,
            ..Default::default()
        },
        source: None,
    }
}

pub fn http_proxy_entry(id: &str, port_id: &str, http: HttpProxy) -> ProxyEntry {
    proxy_entry(id, port_id, ProxyKind::Http(Box::new(http)))
}

pub fn http_route(path: &str, upstream: &str, ip_filter: Option<IpFilter>) -> Route {
    Route {
        path: path.into(),
        servers: vec![UpstreamUrl::new(upstream.parse().unwrap())],
        ip_filter,
        ..Default::default()
    }
}

/// Serves the axum app on a new local port and returns the root URL of the server.
pub async fn serve_http_upstream(app: axum::Router) -> anyhow::Result<Url> {
    let port = alloc_tcp_port().await?;
    let listener = tokio::net::TcpListener::bind(port.socket_addr()).await?;
    tokio::spawn(std::future::IntoFuture::into_future(axum::serve(
        listener, app,
    )));
    Ok(port.http_url("/"))
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
