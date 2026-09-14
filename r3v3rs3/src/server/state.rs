use super::acme_list::AcmeList;
use super::acme_schedule::AcmeSchedule;
use super::cert_list::CertList;
use super::connection::{self, HttpChallenges};
use super::discovery::{self, DiscoveryRegistry, DiscoveryTasks};
use super::proxy_list::ProxyList;
use super::quic::QuicListenerPool;
use super::rpc::proxies::validate_proxy;
use super::udp::UdpListenerPool;
use super::{port_list::PortList, rpc::RpcCallback, tcp::TcpListenerPool};
use crate::certs::acme::{AcmeEntry, AcmeOrder, AcmeTarget};
use crate::certs::alpn::{challenge_config, ChallengeCerts, TlsAlpnChallenge};
use crate::config::storage::Storage;
use crate::discovery::{http::ApiClient, DiscoverySnapshot};
use crate::log::DatabaseLayer;
use crate::proxy::http::SessionService;
use crate::proxy::tls::upstream_client_config;
use crate::{
    command::ServerCommand,
    proxy::{PortContext, PortContextKind},
};
use quinn::Incoming;
use r3v3rs3_api::app::{AppConfig, AppInfo};
use r3v3rs3_api::discovery::{DiscoveryProvider, DiscoveryState, DiscoveryStatus};
use r3v3rs3_api::error::Error;
use r3v3rs3_api::event::ServerEvent;
use r3v3rs3_api::id::ShortId;
use r3v3rs3_api::proxy::ProxyEntry;
use rand::seq::SliceRandom;
use std::collections::HashSet;
use std::net::SocketAddr;
use std::str;
use std::sync::Arc;
use std::time::{Instant, SystemTime};
use tokio::select;
use tokio::{
    net::TcpStream,
    sync::{broadcast, mpsc},
};
use tokio_rustls::rustls::ServerConfig;
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
    http_challenges: HttpChallenges,
    /// The TLS config of the active TLS-ALPN-01 challenges.
    tls_alpn_challenge: Option<Arc<ServerConfig>>,
    tls_alpn_challenges: Vec<TlsAlpnChallenge>,
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

/// The TLS config that answers the TLS-ALPN-01 challenges. `None` without a challenge.
fn tls_alpn_config(challenges: &[TlsAlpnChallenge]) -> Option<Arc<ServerConfig>> {
    if challenges.is_empty() {
        return None;
    }
    match ChallengeCerts::new(challenges) {
        Ok(certs) => Some(challenge_config(Arc::new(certs))),
        Err(err) => {
            error!(%err, "failed to build the TLS-ALPN-01 challenge certificates");
            None
        }
    }
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
            config: Box::new(config.masked()),
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
            http_challenges: HttpChallenges::default(),
            tls_alpn_challenge: None,
            tls_alpn_challenges: Vec::new(),
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
            ServerCommand::AcmeOrderFinished { target, succeeded } => {
                self.acme_schedule
                    .finish(&target, succeeded, Instant::now());
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
        if !discovery::is_enabled(&self.config.discovery, provider) {
            return;
        }
        let client = match self.discovery_client(provider) {
            Ok(client) => client,
            Err(err) => return self.set_discovery_error(provider, err.to_string()),
        };
        let config = self.config.discovery.clone();
        let command = self.command_sender.clone();
        self.discovery_tasks.start(provider, move |generation| {
            crate::discovery::spawn(provider, &config, client, command, generation)
        });
    }

    /// The API client of an enabled provider that reads an HTTP API.
    fn discovery_client(&self, provider: DiscoveryProvider) -> Result<Option<ApiClient>, Error> {
        let Some((endpoints, client_cert)) =
            discovery::api_settings(&self.config.discovery, provider)
        else {
            return Ok(None);
        };
        let endpoints = endpoints
            .into_iter()
            .map(discovery::parse_endpoint)
            .collect::<Result<Vec<_>, _>>()?;
        let tls = upstream_client_config(&self.certs, client_cert)?;
        Ok(Some(ApiClient::new(endpoints, Arc::new(tls))))
    }

    fn set_discovery_error(&mut self, provider: DiscoveryProvider, error: String) {
        error!(%provider, %error, "failed to start service discovery");
        self.discovery.update(DiscoverySnapshot {
            provider,
            generation: 0,
            state: DiscoveryState::Error,
            error: Some(error),
            proxies: Some(Vec::new()),
            certs: Vec::new(),
            issues: Vec::new(),
        });
        self.publish_discovery();
    }

    async fn clear_discovery(&mut self, provider: DiscoveryProvider) {
        self.discovery.remove(provider);
        let certs_changed = self.apply_discovery_certs(provider).await;
        let proxies_changed = self
            .proxies
            .replace_discovered(provider, Vec::new())
            .changed;
        self.finish_discovery_update(proxies_changed, certs_changed)
            .await;
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
        let certs_changed = self.apply_discovery_certs(provider).await;
        let proxies_changed = self.apply_discovery(provider).await;
        self.finish_discovery_update(proxies_changed, certs_changed)
            .await;
        self.start_http_challenges().await;
    }

    /// Sends the changed lists to the event subscribers and sets up the ports again.
    async fn finish_discovery_update(&mut self, proxies_changed: bool, certs_changed: bool) {
        if proxies_changed {
            self.publish_proxies();
        }
        if proxies_changed || certs_changed {
            self.reload_proxies().await;
        }
        self.publish_discovery();
    }

    /// Replaces the certificates of a provider with the certificates of its latest snapshot.
    /// Returns true when the certificate list changed.
    async fn apply_discovery_certs(&mut self, provider: DiscoveryProvider) -> bool {
        let certs = self.discovery.certs(provider);
        let changed = self.certs.replace_discovered(provider, certs);
        if changed {
            self.update_certs().await;
        }
        changed
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
        let (acme_targets, acme_issues) =
            discovery::acme_targets(&built.acme, &update.skipped, &self.acmes);
        issues.extend(acme_issues);
        let added = entries.len() - update.skipped.len();
        if !issues.is_empty() {
            warn!(%provider, issues = issues.len(), "some discovered proxies were not added");
        }
        self.discovery
            .set_result(provider, added, issues, acme_targets);
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

    /// Starts the connection without waiting for the client, so a slow client does not delay the
    /// server loop.
    pub fn handle_tcp_connection(&self, index: usize, stream: TcpStream) {
        let starter = self
            .ports
            .as_slice()
            .get(index)
            .and_then(PortContext::starter);
        let challenge = self.tls_alpn_challenge.clone();
        let tls_alpn_port = self.config.tls_alpn_challenge_addr.port();
        connection::accept(stream, &self.http_challenges, move |stream| match starter {
            Some(starter) => starter.start(stream, challenge),
            None => connection::serve_reserved(stream, challenge, tls_alpn_port),
        });
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
        // The ACME targets of the discovered proxies depend on the entries.
        if self.refresh_discovery().await {
            self.publish_proxies();
            self.reload_proxies().await;
        }
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
        let ordered = self
            .acmes
            .entries()
            .map(AcmeEntry::target)
            .chain(self.discovery.acme_targets())
            .collect::<HashSet<_>>();
        let removing_items = self.certs.expired_acme_certs(&ordered, ASN1Time::now());
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

    /// The targets to order with their entries and their renewal times: the targets of the active
    /// entries, and the discovered targets that no valid certificate covers without an own
    /// certificate.
    fn acme_renewals(&self) -> Vec<(&AcmeEntry, AcmeTarget, Option<SystemTime>)> {
        let mut renewals = self
            .acmes
            .entries()
            .filter(|entry| entry.acme.config.active)
            .map(|entry| (entry, entry.target(), entry.next_renewal(&self.certs)))
            .collect::<Vec<_>>();
        for target in self.discovery.acme_targets() {
            let entry = self.acmes.get(target.acme_id);
            let Some(entry) = entry.filter(|entry| entry.acme.config.active) else {
                continue;
            };
            if renewals.iter().any(|(_, known, _)| *known == target) {
                continue;
            }
            let next_renewal = entry.next_renewal_for(&target, &self.certs);
            if next_renewal.is_none() && self.certs.covers(&target.identifiers) {
                continue;
            }
            renewals.push((entry, target, next_renewal));
        }
        renewals
    }

    async fn start_http_challenges(&mut self) {
        let now = Instant::now();
        let targets = self
            .acme_renewals()
            .into_iter()
            .filter(|(_, target, next_renewal)| {
                self.acme_schedule.is_due(target, *next_renewal, now)
            })
            .map(|(entry, target, _)| (entry.clone(), target))
            .collect::<Vec<_>>();

        if targets.is_empty() {
            return;
        }
        for (_, target) in &targets {
            self.acme_schedule.start(target.clone());
        }

        let dns_resolver = self.config.dns_challenge_resolver;
        let command = self.command_sender.clone();
        tokio::task::spawn(async move {
            let mut orders = Vec::new();
            for (entry, target) in targets {
                let span = span!(Level::INFO, "acme", resource_id = entry.id.to_string());
                span.in_scope(|| {
                    info!(
                        provider = entry.acme.config.provider,
                        identifiers = ?target.identifiers,
                        "starting acme request"
                    );
                });
                match entry
                    .request(&target, dns_resolver)
                    .instrument(span.clone())
                    .await
                {
                    Ok(request) => orders.push(request),
                    Err(err) => {
                        span.in_scope(|| error!("failed to request challenge: {}", err));
                        let _ = command
                            .send(ServerCommand::AcmeOrderFinished {
                                target,
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
        self.http_challenges = HttpChallenges::default();
        self.tls_alpn_challenges.clear();
        self.tls_alpn_challenge = None;
        self.tcp_pool.set_challenge_addrs(Vec::new());
        self.tcp_pool.update(self.ports.as_mut_slice()).await;
    }

    async fn continue_http_challenges(&mut self, orders: Vec<AcmeOrder>) {
        self.serve_challenges(&orders).await;

        let command = self.command_sender.clone();
        tokio::task::spawn(async move {
            for mut order in orders {
                let span = span!(
                    Level::INFO,
                    "acme",
                    resource_id = order.target.acme_id.to_string()
                );
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
                        target: order.target,
                        succeeded,
                    })
                    .await;
            }
        });
    }

    /// Serves the HTTP-01 and TLS-ALPN-01 challenges of the orders. Orders of an earlier batch can
    /// still be running, so their challenges stay served.
    async fn serve_challenges(&mut self, orders: &[AcmeOrder]) {
        Arc::make_mut(&mut self.http_challenges).extend(
            orders
                .iter()
                .flat_map(|order| order.http_challenges.clone()),
        );
        self.tls_alpn_challenges.extend(
            orders
                .iter()
                .flat_map(|order| order.tls_alpn_challenges.clone()),
        );
        self.tls_alpn_challenge = tls_alpn_config(&self.tls_alpn_challenges);
        let addrs = self.challenge_addrs();
        // DNS-01 orders need no listener.
        if !addrs.is_empty() {
            self.tcp_pool.set_challenge_addrs(addrs);
            self.tcp_pool.update(self.ports.as_mut_slice()).await;
        }
    }

    /// The listening addresses of the active challenges.
    fn challenge_addrs(&self) -> Vec<SocketAddr> {
        let http = (!self.http_challenges.is_empty()).then_some(self.config.http_challenge_addr);
        let tls_alpn = self
            .tls_alpn_challenge
            .as_ref()
            .map(|_| self.config.tls_alpn_challenge_addr);
        http.into_iter().chain(tls_alpn).collect()
    }

    pub fn config(&self) -> &AppConfig {
        &self.config
    }

    pub async fn set_config(&mut self, mut config: AppConfig) -> Result<(), Error> {
        config.keep_secrets(&self.config);
        discovery::validate_config(&config.discovery, &self.certs)?;
        let changed = discovery::changed_providers(&self.config.discovery, &config.discovery);
        self.config.discovery.clone_from(&config.discovery);
        for provider in changed {
            self.restart_discovery(provider).await;
        }
        self.config.clone_from(&config);
        self.sessions.set_config(config.admin);
        self.storage.save_app_config(&config).await;
        let _ = self.br_sender.send(ServerEvent::AppConfigUpdated {
            config: Box::new(config.masked()),
        });
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
