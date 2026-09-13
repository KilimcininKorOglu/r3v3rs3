use super::acme_list::AcmeList;
use super::acme_schedule::AcmeSchedule;
use super::cert_list::CertList;
use super::discovery::{self, DiscoveryRegistry, DiscoveryTasks};
use super::proxy_list::ProxyList;
use super::quic::QuicListenerPool;
use super::rpc::proxies::validate_proxy;
use super::udp::UdpListenerPool;
use super::{port_list::PortList, rpc::RpcCallback, tcp::TcpListenerPool};
use crate::certs::acme::AcmeOrder;
use crate::config::storage::Storage;
use crate::discovery::{docker, http::ApiClient, DiscoverySnapshot};
use crate::log::DatabaseLayer;
use crate::proxy::http::SessionService;
use crate::proxy::tls::upstream_client_config;
use crate::{
    command::ServerCommand,
    proxy::{PortContext, PortContextKind},
};
use hyper::service::service_fn;
use hyper::Response;
use hyper_util::rt::{TokioExecutor, TokioIo};
use hyper_util::server::conn::auto;
use quinn::Incoming;
use r3v3rs3_api::app::{AppConfig, AppInfo};
use r3v3rs3_api::discovery::{DiscoveryProvider, DiscoveryState, DiscoveryStatus};
use r3v3rs3_api::error::Error;
use r3v3rs3_api::event::ServerEvent;
use r3v3rs3_api::id::ShortId;
use r3v3rs3_api::proxy::ProxyEntry;
use rand::seq::SliceRandom;
use std::collections::HashSet;
use std::convert::Infallible;
use std::net::SocketAddr;
use std::str;
use std::time::Instant;
use std::{collections::HashMap, sync::Arc};
use tokio::io::AsyncBufReadExt;
use tokio::select;
use tokio::{
    io::BufStream,
    net::TcpStream,
    sync::{broadcast, mpsc},
};
use tracing::{error, info, span, warn, Instrument, Level};
use x509_parser::time::ASN1Time;

pub struct ServerState {
    pub proxies: ProxyList,
    pub certs: CertList,
    pub acmes: AcmeList,
    pub ports: PortList,
    pub storage: Arc<dyn Storage>,
    sessions: Arc<SessionService>,
    config: AppConfig,
    tcp_pool: TcpListenerPool,
    udp_pool: UdpListenerPool,
    quic_pool: QuicListenerPool,
    http_challenges: HashMap<String, String>,
    acme_schedule: AcmeSchedule,
    command_sender: mpsc::Sender<ServerCommand>,
    br_sender: broadcast::Sender<ServerEvent>,
    callback_sender: mpsc::Sender<RpcCallback>,
    broadcast_events: bool,
    discovery: DiscoveryRegistry,
    discovery_tasks: DiscoveryTasks,
}

pub enum Received {
    Tcp(usize, TcpStream),
    Udp(usize, usize, SocketAddr, Vec<u8>),
    Quic(usize, Box<Incoming>),
}

impl ServerState {
    pub async fn new(
        storage: impl Storage,
        command_sender: mpsc::Sender<ServerCommand>,
        callback_sender: mpsc::Sender<RpcCallback>,
        br_sender: broadcast::Sender<ServerEvent>,
    ) -> Self {
        let config = storage.load_app_config().await;
        let _ = br_sender.send(ServerEvent::AppConfigUpdated {
            config: config.clone(),
        });

        if let Some(ranges) = storage.load_cdn_ranges().await {
            crate::cdn::install(ranges);
        }

        let certs = storage.load_certs().await;
        let acmes = storage.load_acmes().await;
        let mut proxies = storage.load_proxies().await;
        // Discovered proxies are not saved. A source in the file comes from a manual edit.
        proxies.retain(|entry| !entry.is_discovered());
        for entry in &mut proxies {
            if let Err(err) = super::credentials::seal_proxy(&mut entry.proxy) {
                error!(id = %entry.id, %err, "invalid proxy credentials");
            }
        }

        let mut ports = PortList::default();
        for entry in storage.load_ports().await {
            match PortContext::new(entry) {
                Ok(ctx) => {
                    ports.update(ctx);
                }
                Err(err) => {
                    error!(%err, "failed to create proxy state");
                }
            };
        }

        let storage: Arc<dyn Storage> = Arc::new(storage);
        let sessions = Arc::new(SessionService::new(storage.clone(), config.admin));
        let mut this = Self {
            proxies: proxies.into_iter().collect(),
            certs: CertList::new(certs).await,
            acmes: acmes.into_iter().collect(),
            ports,
            storage,
            sessions,
            config,
            tcp_pool: TcpListenerPool::new(),
            udp_pool: UdpListenerPool::new(),
            quic_pool: QuicListenerPool::new(),
            http_challenges: HashMap::new(),
            acme_schedule: AcmeSchedule::default(),
            command_sender,
            br_sender,
            callback_sender,
            broadcast_events: false,
            discovery: DiscoveryRegistry::default(),
            discovery_tasks: DiscoveryTasks::default(),
        };

        this.update_ports().await;
        this.update_certs().await;
        this.update_proxies().await;
        this.update_acmes().await;
        this.reload_proxies().await;
        for provider in DiscoveryProvider::ALL {
            this.restart_discovery(provider).await;
        }
        this
    }

    pub async fn handle_command(&mut self, cmd: ServerCommand) {
        match cmd {
            ServerCommand::AddCert { cert } => {
                self.certs.add(cert.clone());
                self.update_certs().await;
                self.reload_proxies().await;
                self.storage.save_cert(&cert).await;
            }
            ServerCommand::SetBroadcastEvents { enabled } => {
                self.broadcast_events = enabled;
            }
            ServerCommand::AddAcmeOrders { orders } => {
                self.continue_http_challenges(orders).await;
            }
            ServerCommand::AcmeOrderFinished { id, succeeded } => {
                self.acme_schedule.finish(id, succeeded, Instant::now());
                if self.acme_schedule.is_idle() {
                    self.stop_http_challenges().await;
                }
            }
            ServerCommand::CallMethod { id, mut arg } => {
                let result = arg.call(self).await;
                let _ = self.callback_sender.send(RpcCallback { id, result }).await;
            }
            ServerCommand::SetCdnRanges { ranges } => {
                if crate::cdn::install(ranges.clone()) {
                    self.storage.save_cdn_ranges(&ranges).await;
                }
            }
            ServerCommand::SetDiscovery { snapshot } => {
                self.set_discovery(snapshot).await;
            }
        }
    }

    /// Stops the task of the provider, removes its proxies and starts it again with the current
    /// settings.
    async fn restart_discovery(&mut self, provider: DiscoveryProvider) {
        if self.discovery_tasks.stop(provider) || self.discovery.contains(provider) {
            self.clear_discovery(provider).await;
        }
        let client = match self.discovery_client(provider) {
            Ok(Some(client)) => client,
            Ok(None) => return,
            Err(err) => return self.set_discovery_error(provider, err.to_string()),
        };
        let config = self.config.discovery.docker.clone();
        let command = self.command_sender.clone();
        self.discovery_tasks.start(provider, move |generation| {
            docker::spawn(&config, client, command, generation)
        });
    }

    /// The API client of an enabled provider.
    fn discovery_client(&self, provider: DiscoveryProvider) -> Result<Option<ApiClient>, Error> {
        let docker = &self.config.discovery.docker;
        if provider != DiscoveryProvider::Docker || !docker.enabled {
            return Ok(None);
        }
        let endpoint = discovery::parse_endpoint(&docker.endpoint)?;
        let tls = upstream_client_config(&self.certs, docker.client_cert)?;
        Ok(Some(ApiClient::new(endpoint, Arc::new(tls))))
    }

    fn set_discovery_error(&mut self, provider: DiscoveryProvider, error: String) {
        error!(%provider, %error, "failed to start service discovery");
        self.discovery.update(DiscoverySnapshot {
            provider,
            generation: 0,
            state: DiscoveryState::Error,
            error: Some(error),
            proxies: Some(Vec::new()),
            issues: Vec::new(),
        });
        self.publish_discovery();
    }

    async fn clear_discovery(&mut self, provider: DiscoveryProvider) {
        self.discovery.remove(provider);
        if self
            .proxies
            .replace_discovered(provider, Vec::new())
            .changed
        {
            self.publish_proxies();
            self.reload_proxies().await;
        }
        self.publish_discovery();
    }

    async fn set_discovery(&mut self, snapshot: DiscoverySnapshot) {
        if !self
            .discovery_tasks
            .accepts(snapshot.provider, snapshot.generation)
        {
            return;
        }
        let provider = snapshot.provider;
        self.discovery.update(snapshot);
        if self.apply_discovery(provider).await {
            self.publish_proxies();
            self.reload_proxies().await;
        }
        self.publish_discovery();
    }

    /// Builds the proxies of a provider from its latest snapshot and the current ports. Returns
    /// true when the proxy list changed.
    async fn apply_discovery(&mut self, provider: DiscoveryProvider) -> bool {
        let ports = self.ports.entries().cloned().collect::<Vec<_>>();
        let proxies = self.discovery.proxies(provider);
        let built = discovery::build(
            provider,
            &proxies,
            &ports,
            self.ids_not_owned_by(provider),
            |proxy| validate_proxy(proxy, self),
        );
        let mut issues = built.issues;
        let (entries, seal_issues) = self.discovery.seal(provider, built.entries).await;
        issues.extend(seal_issues);
        let update = self.proxies.replace_discovered(provider, entries.clone());
        issues.extend(discovery::conflict_issues(&entries, &update.skipped));
        let added = entries.len() - update.skipped.len();
        if !issues.is_empty() {
            warn!(%provider, issues = issues.len(), "some discovered proxies were not added");
        }
        self.discovery.set_result(provider, added, issues);
        update.changed
    }

    /// Rebuilds the proxies of every provider, because a port change can resolve a port name
    /// differently. Returns true when the proxy list changed.
    async fn refresh_discovery(&mut self) -> bool {
        let providers = self.discovery.providers();
        let mut changed = false;
        for provider in &providers {
            changed |= self.apply_discovery(*provider).await;
        }
        if !providers.is_empty() {
            self.publish_discovery();
        }
        changed
    }

    fn ids_not_owned_by(&self, provider: DiscoveryProvider) -> HashSet<ShortId> {
        let other_proxies = self
            .proxies
            .entries()
            .filter(|entry| entry.source.as_ref().map(|source| source.provider) != Some(provider));
        self.acmes
            .entries()
            .map(|acme| acme.id)
            .chain(self.ports.entries().map(|port| port.id))
            .chain(other_proxies.map(|entry| entry.id))
            .collect()
    }

    fn publish_discovery(&self) {
        let _ = self.br_sender.send(ServerEvent::DiscoveryStatusUpdated {
            entries: self.discovery.statuses(),
        });
    }

    pub fn discovery_statuses(&self) -> Vec<DiscoveryStatus> {
        self.discovery.statuses()
    }

    pub fn has_active_listeners(&self) -> bool {
        self.tcp_pool.has_active_listeners()
            || self.udp_pool.has_active_listeners()
            || self.quic_pool.has_active_listeners()
    }

    pub async fn select(&mut self) -> Option<Received> {
        select! {
            Some((index, stream)) = self.tcp_pool.select() => {
                Some(Received::Tcp(index, stream))
            }
            Some((index, config_index, addr, data)) = self.udp_pool.select() => {
                Some(Received::Udp(index, config_index, addr, data))
            }
            Some((index, stream)) = self.quic_pool.select() => {
                Some(Received::Quic(index, Box::new(stream)))
            }
            else => None
        }
    }

    pub async fn handle_tcp_connection(&mut self, index: usize, stream: TcpStream) {
        let mut stream = BufStream::new(stream);

        if !self.http_challenges.is_empty() {
            if let Some(body) = self.handle_http_challenge(&mut stream).await {
                tokio::task::spawn(async move {
                    let stream = TokioIo::new(BufStream::new(stream));
                    if let Err(err) = auto::Builder::new(TokioExecutor::new())
                        .serve_connection(
                            stream,
                            service_fn(|_| {
                                let body = body.clone();
                                async move { Ok::<_, Infallible>(Response::new(body)) }
                            }),
                        )
                        .await
                    {
                        error!("Error serving connection: {:?}", err);
                    }
                });
                return;
            }
        }

        if index < self.ports.as_slice().len() {
            let state = &mut self.ports.as_mut_slice()[index];
            match state.kind_mut() {
                PortContextKind::Tcp(tcp) => {
                    tcp.start_proxy(stream);
                }
                PortContextKind::Http(http) => {
                    http.start_proxy(stream);
                }
                _ => (),
            }
        }
    }

    pub async fn handle_udp_packet(
        &mut self,
        index: usize,
        config_index: usize,
        addr: SocketAddr,
        data: Vec<u8>,
    ) {
        let Some(listener) = self.udp_pool.socket(index) else {
            return;
        };
        let port = self.ports.as_mut_slice().get_mut(config_index);
        if let Some(PortContextKind::Udp(udp)) = port.map(PortContext::kind_mut) {
            udp.forward(addr, &data, &listener).await;
        }
    }

    pub async fn handle_quic_connection(&mut self, index: usize, stream: Incoming) {
        if index < self.ports.as_slice().len() {
            let state = &mut self.ports.as_mut_slice()[index];
            if let PortContextKind::Http3(http) = state.kind_mut() {
                http.start_quic_proxy(stream);
            }
        }
    }

    pub async fn update_ports(&mut self) {
        let entries = self.ports.entries().cloned().collect::<Vec<_>>();
        self.tcp_pool
            .remove_unused_ports(self.ports.as_slice())
            .await;
        self.udp_pool
            .remove_unused_ports(self.ports.as_slice())
            .await;
        self.quic_pool
            .remove_unused_ports(self.ports.as_slice())
            .await;
        self.tcp_pool.update(self.ports.as_mut_slice()).await;
        self.udp_pool.update(self.ports.as_mut_slice()).await;
        self.quic_pool.update(self.ports.as_mut_slice()).await;
        self.storage.save_ports(&entries).await;
        if self.proxies.remove_incompatible_ports(&entries) {
            self.update_proxies().await;
        }
        if self.refresh_discovery().await {
            self.publish_proxies();
        }
        let _ = self
            .br_sender
            .send(ServerEvent::PortTableUpdated { entries });
        if self.broadcast_events {
            for (entry, ctx) in self.ports.entries().cloned().zip(self.ports.as_slice()) {
                let _ = self.br_sender.send(ServerEvent::PortStatusUpdated {
                    id: entry.id,
                    status: *ctx.status(),
                });
            }
        }
    }

    /// Saves the manual proxies and sends the proxy list to the event subscribers.
    pub async fn update_proxies(&mut self) {
        let manual = self
            .proxies
            .entries()
            .filter(|entry| !entry.is_discovered())
            .cloned()
            .collect::<Vec<_>>();
        self.storage.save_proxies(&manual).await;
        self.publish_proxies();
    }

    fn publish_proxies(&self) {
        let entries = self.proxies.entries().cloned().collect::<Vec<_>>();
        let _ = self.br_sender.send(ServerEvent::ProxiesUpdated { entries });
        if self.broadcast_events {
            for ctx in self.proxies.contexts() {
                let _ = self.br_sender.send(ServerEvent::ProxyStatusUpdated {
                    id: ctx.entry.id,
                    status: ctx.status.clone(),
                });
            }
        }
    }

    pub async fn update_certs(&mut self) {
        let _ = self.br_sender.send(ServerEvent::CertsUpdated {
            entries: self.certs.iter().map(|item| item.info()).collect(),
        });
    }

    pub async fn update_acmes(&mut self) {
        let _ = self.br_sender.send(ServerEvent::AcmeUpdated {
            entries: self
                .acmes
                .entries()
                .map(|acme| acme.info(&self.certs))
                .collect(),
        });
        self.start_http_challenges().await;
    }

    pub async fn update_port(&mut self, ctx: PortContext) {
        if self.ports.update(ctx) {
            self.update_ports().await;
            self.reload_proxies().await;
        }
    }

    async fn handle_http_challenge(&mut self, stream: &mut BufStream<TcpStream>) -> Option<String> {
        const HTTP_CHALLENGE_HEADER: &[u8] = b"GET /.well-known/acme-challenge/";
        if let Ok(buf) = stream.fill_buf().await {
            if buf.starts_with(HTTP_CHALLENGE_HEADER) {
                return buf[HTTP_CHALLENGE_HEADER.len()..]
                    .split(|&b| b == b' ')
                    .next()
                    .and_then(|line| {
                        let key = std::str::from_utf8(line).unwrap_or("");
                        self.http_challenges.get(key).cloned()
                    });
            }
        }
        None
    }

    pub async fn reload_proxies(&mut self) {
        let ports = self.ports.entries().cloned().collect::<Vec<_>>();
        for ctx in self.ports.as_mut_slice() {
            let proxies = self
                .proxies
                .entries()
                .filter(|entry: &&ProxyEntry| {
                    entry.proxy.active && entry.proxy.ports.contains(&ctx.entry.id)
                })
                .cloned()
                .collect();
            let span = span!(Level::INFO, "port", resource_id = ctx.entry.id.to_string());
            if let Err(err) = ctx
                .setup(&ports, &self.certs, proxies, &self.sessions)
                .instrument(span.clone())
                .await
            {
                span.in_scope(|| {
                    error!(%err, "failed to setup port");
                });
            }
        }
    }

    pub async fn run_background_tasks(&mut self, app_info: &AppInfo) {
        if let Err(err) = self.cleanup_old_logs(app_info).await {
            error!(%err, "failed to cleanup old logs");
        }

        self.start_http_challenges().await;
        self.reload_proxies().await;
        self.remove_expired_certs();
    }

    fn remove_expired_certs(&mut self) {
        let mut removing_items = Vec::new();
        for acme in self.acmes.entries() {
            let certs = self.certs.find_certs_by_acme(acme.id);
            let mut expired = certs
                .iter()
                .filter(|cert| cert.not_after < ASN1Time::now())
                .map(|cert| cert.id)
                .collect::<Vec<_>>();
            if expired.len() >= certs.len() {
                expired.pop();
            }
            removing_items.append(&mut expired);
        }
        for id in &removing_items {
            if let Err(err) = self.certs.delete(*id) {
                error!(%err, "failed to delete cert");
            }
        }
        if !removing_items.is_empty() {
            let _ = self.br_sender.send(ServerEvent::CertsUpdated {
                entries: self.certs.iter().map(|item| item.info()).collect(),
            });
        }
    }

    async fn cleanup_old_logs(&mut self, app_info: &AppInfo) -> anyhow::Result<()> {
        let path = app_info.log_path.join("log.db");
        let database = DatabaseLayer::new(&path, tracing::level_filters::LevelFilter::OFF).await?;
        database
            .cleanup(self.config.log.database_log_retention)
            .await?;
        Ok(())
    }

    async fn start_http_challenges(&mut self) {
        let now = Instant::now();
        let entries = self
            .acmes
            .entries()
            .filter(|entry| entry.acme.config.active)
            .filter(|entry| {
                self.acme_schedule
                    .is_due(entry.id, entry.next_renewal(&self.certs), now)
            })
            .cloned()
            .collect::<Vec<_>>();

        if entries.is_empty() {
            return;
        }
        for entry in &entries {
            self.acme_schedule.start(entry.id);
        }

        let dns_resolver = self.config.dns_challenge_resolver;
        let command = self.command_sender.clone();
        tokio::task::spawn(async move {
            let mut orders = Vec::new();
            for entry in entries {
                let span = span!(Level::INFO, "acme", resource_id = entry.id.to_string());
                span.in_scope(|| {
                    info!(
                        provider = entry.acme.config.provider,
                        identifiers = ?entry.acme.identifiers,
                        "starting acme request"
                    );
                });
                match entry.request(dns_resolver).instrument(span.clone()).await {
                    Ok(request) => orders.push(request),
                    Err(err) => {
                        span.in_scope(|| error!("failed to request challenge: {}", err));
                        let _ = command
                            .send(ServerCommand::AcmeOrderFinished {
                                id: entry.id,
                                succeeded: false,
                            })
                            .await;
                    }
                }
            }
            if !orders.is_empty() {
                let _ = command.send(ServerCommand::AddAcmeOrders { orders }).await;
            }
        });
    }

    async fn stop_http_challenges(&mut self) {
        self.http_challenges.clear();
        self.tcp_pool.set_http_challenge_addr(None);
        self.tcp_pool.update(self.ports.as_mut_slice()).await;
    }

    async fn continue_http_challenges(&mut self, orders: Vec<AcmeOrder>) {
        // Orders of an earlier batch can still be running, so their challenges stay served.
        self.http_challenges
            .extend(orders.iter().flat_map(|req| req.http_challenges.clone()));
        // DNS-01 orders need no listener.
        if !self.http_challenges.is_empty() {
            self.tcp_pool
                .set_http_challenge_addr(Some(self.config.http_challenge_addr));
            self.tcp_pool.update(self.ports.as_mut_slice()).await;
        }

        let command = self.command_sender.clone();
        tokio::task::spawn(async move {
            for mut order in orders {
                let span = span!(Level::INFO, "acme", resource_id = order.id.to_string());
                let succeeded = match order.start_challenge().instrument(span.clone()).await {
                    Ok(cert) => {
                        span.in_scope(|| {
                            info!(id = cert.id().to_string(), "acme request completed");
                        });
                        let _ = command
                            .send(ServerCommand::AddCert {
                                cert: Arc::new(cert),
                            })
                            .await;
                        true
                    }
                    Err(err) => {
                        span.in_scope(|| error!(%err, "failed to start challenge"));
                        false
                    }
                };
                let _ = command
                    .send(ServerCommand::AcmeOrderFinished {
                        id: order.id,
                        succeeded,
                    })
                    .await;
            }
        });
    }

    pub fn config(&self) -> &AppConfig {
        &self.config
    }

    pub async fn set_config(&mut self, config: AppConfig) -> Result<(), Error> {
        discovery::validate_config(&config.discovery, &self.certs)?;
        let changed = discovery::changed_providers(&self.config.discovery, &config.discovery);
        self.config.discovery.clone_from(&config.discovery);
        for provider in changed {
            self.restart_discovery(provider).await;
        }
        self.config.clone_from(&config);
        self.sessions.set_config(config.admin);
        self.storage.save_app_config(&config).await;
        let _ = self
            .br_sender
            .send(ServerEvent::AppConfigUpdated { config });
        Ok(())
    }

    pub fn generate_id(&self) -> ShortId {
        const TABLE: &[u8] = b"bcdfghjklmnpqrstvwxyz";

        let used_ids = self
            .acmes
            .entries()
            .map(|acme| acme.id)
            .chain(self.ports.entries().map(|port| port.id))
            .chain(self.proxies.entries().map(|site| site.id))
            .collect::<HashSet<_>>();

        let mut rng = rand::thread_rng();
        let mut id = [b'a'; 6];
        loop {
            for c in &mut id {
                *c = *TABLE.choose(&mut rng).unwrap();
            }
            let id = format!(
                "{}-{}",
                str::from_utf8(&id[..3]).unwrap(),
                str::from_utf8(&id[3..]).unwrap()
            )
            .parse()
            .unwrap();
            if !used_ids.contains(&id) {
                return id;
            }
        }
    }

    pub async fn shutdown(self) {
        std::mem::drop(self.quic_pool);
    }
}
