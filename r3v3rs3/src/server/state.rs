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
use crate::accounts::{AccountDirectory, Caller};
use crate::audit::{AuditLog, AuditRecord, AuditStore, DAY_MS};
use crate::certs::acme::{AcmeEntry, AcmeOrder, AcmeTarget};
use crate::certs::alpn::{challenge_config, ChallengeCerts, TlsAlpnChallenge};
use crate::certs::challenges::ServedChallenges;
use crate::clock::unix_ms;
use crate::cluster::layout::StateKind;
use crate::config::storage::Storage;
use crate::discovery::DiscoverySnapshot;
use crate::kv::http::ApiClient;
use crate::log::DatabaseLayer;
use crate::notify::{certificate_notifications, Notification, Notifier};
use crate::proxy::http::SessionService;
use crate::proxy::tls::upstream_client_config;
use crate::sessions::{self, SessionBackend};
use crate::{
    command::ServerCommand,
    proxy::{PortContext, PortContextKind, ProxyRegistries},
};
use quinn::Incoming;
use r3v3rs3_api::access_list::{apply_access_lists, AccessListEntry};
use r3v3rs3_api::app::{AppConfig, AppInfo};
use r3v3rs3_api::cluster::ClusterStatus;
use r3v3rs3_api::discovery::{DiscoveryProvider, DiscoveryState, DiscoveryStatus};
use r3v3rs3_api::error::Error;
use r3v3rs3_api::event::ServerEvent;
use r3v3rs3_api::id::ShortId;
use r3v3rs3_api::port::PortEntry;
use r3v3rs3_api::proxy::{ProxyEntry, ProxyKind};
use rand::seq::SliceRandom;
use std::collections::HashSet;
use std::net::SocketAddr;
use std::str;
use std::sync::Arc;
use std::time::{Instant, SystemTime};
use tokio::select;
use tokio::{
    net::TcpStream,
    sync::{broadcast, mpsc, watch},
};
use tokio_rustls::rustls::ServerConfig;
use tracing::{error, info, span, warn, Instrument, Level};
use x509_parser::time::ASN1Time;

pub struct ServerState {
    pub proxies: ProxyList,
    /// The access lists with the hashes of their secrets.
    pub access_lists: Vec<AccessListEntry>,
    pub certs: CertList,
    pub acmes: AcmeList,
    pub ports: PortList,
    pub storage: Arc<dyn Storage>,
    sessions: Arc<SessionService>,
    pub registries: ProxyRegistries,
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
    cluster: ClusterStatus,
    /// Whether this node runs the leader tasks. A server without a cluster always does.
    leader: bool,
    session_backend: Arc<dyn SessionBackend>,
    /// The accounts without their secrets, for the admin API and the proxy authentication.
    accounts: watch::Sender<Arc<AccountDirectory>>,
    audit: Arc<AuditLog>,
    /// The id that the running RPC method generated, for its audit log entry.
    created_id: Option<ShortId>,
    /// The day of the last audit log cleanup, in days after the Unix epoch.
    audit_cleaned_day: u64,
    /// Sends the webhook notifications in the background.
    notifier: Notifier,
    /// The keys of the certificate events that the webhook got.
    sent_notifications: HashSet<String>,
}

pub enum Received {
    Tcp(usize, TcpStream),
    Udp(usize, usize, SocketAddr, Vec<u8>),
    Quic(usize, Box<Incoming>),
}

/// Logs a write that the server cannot report to a caller.
fn log_save_error(result: Result<(), Error>) {
    if let Err(err) = result {
        error!(%err, "failed to save the server state");
    }
}

/// The proxies that the storage keeps, with sealed credentials. Discovered proxies are not saved,
/// so a source in the storage comes from a manual edit and is removed.
fn manual_proxies(mut proxies: Vec<ProxyEntry>) -> Vec<ProxyEntry> {
    proxies.retain(|entry| !entry.is_discovered());
    for entry in &mut proxies {
        if let Err(err) = super::credentials::seal_proxy(&mut entry.proxy) {
            error!(id = %entry.id, %err, "invalid proxy credentials");
        }
    }
    proxies
}

/// Gives an HTTP proxy the IP filters and the authentication of its access lists.
fn with_access_lists(mut entry: ProxyEntry, lists: &[AccessListEntry]) -> ProxyEntry {
    if let ProxyKind::Http(http) = &mut entry.proxy.kind {
        for list in apply_access_lists(http, lists) {
            warn!(proxy = %entry.id, access_list = %list, "the access list does not exist, so the proxy rejects every client");
        }
    }
    entry
}

/// The account directory of the storage. `None` when the storage cannot load the accounts.
async fn load_account_directory(storage: &dyn Storage) -> Option<AccountDirectory> {
    match storage.load_accounts().await {
        Ok(accounts) => Some(AccountDirectory::new(&accounts)),
        Err(err) => {
            error!(%err, "failed to load the accounts");
            None
        }
    }
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
        storage: Arc<dyn Storage>,
        audit_store: Arc<dyn AuditStore>,
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
        let proxies = manual_proxies(storage.load_proxies().await);
        let access_lists = storage.load_access_lists().await;
        let sent_notifications = storage.load_sent_notifications().await;

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

        // Without the accounts the directory is empty, so no account passes until a reload.
        let directory = load_account_directory(storage.as_ref()).await;
        let accounts = watch::Sender::new(Arc::new(directory.unwrap_or_default()));
        let session_backend = storage.clone().session_backend();
        let sessions = Arc::new(SessionService::new(
            storage.clone(),
            session_backend.clone(),
            accounts.subscribe(),
            config.admin,
        ));
        let leader = !config.cluster.enabled;
        let audit = Arc::new(AuditLog::new(audit_store, config.cluster.node_name.clone()));
        let mut this = Self {
            proxies: proxies.into_iter().collect(),
            access_lists,
            certs: CertList::new(certs).await,
            acmes: acmes.into_iter().collect(),
            ports,
            storage,
            sessions,
            registries: ProxyRegistries::default(),
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
            cluster: ClusterStatus::default(),
            leader,
            session_backend,
            accounts,
            audit,
            created_id: None,
            audit_cleaned_day: 0,
            notifier: Notifier::spawn(),
            sent_notifications,
        };

        if let Some(exchange) = this.storage.clone().rate_count_exchange() {
            let interval = this.config.cluster.rate_limit_sync_interval;
            this.registries.limiters.start_sharing(exchange, interval);
        }
        if let Some(store) = this.storage.clone().shared_cache_store() {
            this.registries.caches.start_sharing(store);
        }
        log_save_error(this.update_ports().await);
        this.update_certs().await;
        log_save_error(this.update_proxies().await);
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
                self.add_issued_cert(cert).await;
            }
            ServerCommand::SetBroadcastEvents { enabled } => {
                self.broadcast_events = enabled;
            }
            ServerCommand::AddAcmeOrders { orders } => {
                self.continue_http_challenges(orders).await;
            }
            ServerCommand::AcmeOrderFinished { target, error } => {
                self.finish_acme_order(target, error).await;
            }
            ServerCommand::CallMethod {
                id,
                mut arg,
                caller,
            } => {
                let result = self.call_method(&caller, arg.as_mut()).await;
                let _ = self.callback_sender.send(RpcCallback { id, result }).await;
            }
            ServerCommand::SetCdnRanges { ranges } => {
                // In a cluster the other nodes read the ranges of the leader from the store.
                if self.leader && crate::cdn::install(ranges.clone()) {
                    log_save_error(self.storage.save_cdn_ranges(&ranges).await);
                }
            }
            ServerCommand::SetDiscovery { snapshot } => {
                self.set_discovery(snapshot).await;
            }
            ServerCommand::ClusterChanged { kinds } => {
                self.apply_cluster_changes(kinds).await;
            }
            ServerCommand::SetClusterStatus { status } => {
                self.cluster = ClusterStatus {
                    leader: self.leader,
                    ..status
                };
                self.publish_cluster_status();
            }
            ServerCommand::SetLeader { leader } => {
                self.set_leader(leader).await;
            }
        }
    }

    fn publish_cluster_status(&self) {
        let status = self.cluster.clone();
        let _ = self
            .br_sender
            .send(ServerEvent::ClusterStatusUpdated { status });
    }

    /// A new leader runs the leader tasks at once, so it does not wait for the next background
    /// run.
    async fn set_leader(&mut self, leader: bool) {
        self.leader = leader;
        self.cluster.leader = leader;
        self.publish_cluster_status();
        if leader {
            // The previous leader can have sent notifications that this node did not send.
            self.sent_notifications = self.storage.load_sent_notifications().await;
            self.run_leader_tasks().await;
        }
    }

    /// The tasks that only one node of a cluster runs.
    async fn run_leader_tasks(&mut self) {
        self.start_http_challenges().await;
        self.cleanup_audit_log().await;
        self.remove_expired_certs().await;
        self.notify_certificates().await;
        let expiry = sessions::expiry(&self.config.admin);
        if let Err(err) = self.session_backend.remove_expired(expiry).await {
            error!(%err, "failed to remove the expired sessions");
        }
        self.storage.remove_expired_shared_responses().await;
    }

    pub fn cluster_status(&self) -> ClusterStatus {
        self.cluster.clone()
    }

    pub fn session_backend(&self) -> Arc<dyn SessionBackend> {
        self.session_backend.clone()
    }

    pub fn audit_log(&self) -> Arc<AuditLog> {
        self.audit.clone()
    }

    /// Deletes the audit log entries after the retention, once a day.
    async fn cleanup_audit_log(&mut self) {
        let now = unix_ms();
        let today = now / DAY_MS;
        if self.audit_cleaned_day == today {
            return;
        }
        let retention = self.config.log.audit_log_retention.as_millis();
        let retention = u64::try_from(retention).unwrap_or(u64::MAX);
        match self
            .audit
            .remove_before(now.saturating_sub(retention))
            .await
        {
            Ok(()) => self.audit_cleaned_day = today,
            Err(err) => error!("failed to remove the old audit log entries: {err:#}"),
        }
    }

    /// Sends a webhook notification for each certificate that expires within the warning time or
    /// has expired. Each event of a certificate is sent once.
    async fn notify_certificates(&mut self) {
        let Some(webhook) = self.config.notifications.webhook.clone() else {
            return;
        };
        let certs = self
            .certs
            .iter()
            .map(|cert| cert.info())
            .collect::<Vec<_>>();
        let now = i64::try_from(unix_ms() / 1000).unwrap_or(i64::MAX);
        let (notifications, sent) = certificate_notifications(
            &certs,
            &self.sent_notifications,
            now,
            self.config.notifications.cert_expiry_warning,
            &self.config.cluster.node_name,
        );
        for notification in notifications {
            self.notifier.send(&webhook, notification);
        }
        if sent != self.sent_notifications {
            log_save_error(self.storage.save_sent_notifications(&sent).await);
            self.sent_notifications = sent;
        }
    }

    /// Ends an ACME order. A failed order waits before the next attempt, and the leader sends a
    /// webhook notification about it.
    async fn finish_acme_order(&mut self, target: AcmeTarget, error: Option<String>) {
        self.acme_schedule
            .finish(&target, error.is_none(), Instant::now());
        let webhook = self
            .config
            .notifications
            .webhook
            .as_ref()
            .filter(|_| self.leader);
        if let (Some(webhook), Some(error)) = (webhook, error) {
            let node = &self.config.cluster.node_name;
            let notification = Notification::acme_order_failed(node, &target, error);
            self.notifier.send(webhook, notification);
        }
        if self.acme_schedule.is_idle() {
            self.stop_http_challenges().await;
        }
    }

    /// The accounts without their secrets. The value changes when an account changes.
    pub fn account_directory(&self) -> watch::Receiver<Arc<AccountDirectory>> {
        self.accounts.subscribe()
    }

    /// Reads the parts of the state that the cluster store changed again, and applies them.
    async fn apply_cluster_changes(&mut self, kinds: Vec<StateKind>) {
        for kind in kinds {
            self.reload_kind(kind).await;
        }
    }

    async fn reload_kind(&mut self, kind: StateKind) {
        match kind {
            StateKind::Config => self.reload_config().await,
            StateKind::Certs => self.reload_certs().await,
            StateKind::Acmes => self.reload_acmes().await,
            StateKind::Ports => self.reload_ports().await,
            StateKind::Proxies => self.reload_manual_proxies().await,
            StateKind::AccessLists => self.reload_access_lists().await,
            StateKind::Cdn => self.reload_cdn_ranges().await,
            StateKind::Challenges => self.reload_challenges().await,
            StateKind::CachePurges => self.reload_cache_purges().await,
            StateKind::Accounts => self.reload_accounts().await,
        }
    }

    /// Adds a new proxy to the proxy list of the account that created it. An account without a
    /// proxy list already sees every proxy.
    pub async fn grant_proxy(
        &mut self,
        username: &str,
        proxy: r3v3rs3_api::id::ShortId,
    ) -> Result<(), Error> {
        self.edit_accounts(|accounts| {
            let proxies = accounts
                .get_mut(username)
                .and_then(|account| account.proxies.as_mut());
            Ok(proxies.is_some_and(|proxies| proxies.insert(proxy)))
        })
        .await
    }

    /// Removes a deleted proxy from the proxy lists of the accounts.
    pub async fn revoke_proxy(&mut self, proxy: r3v3rs3_api::id::ShortId) -> Result<(), Error> {
        self.edit_accounts(|accounts| {
            Ok(accounts
                .values_mut()
                .filter_map(|account| account.proxies.as_mut())
                .fold(false, |changed, proxies| proxies.remove(&proxy) || changed))
        })
        .await
    }

    /// Saves the accounts when `edit` changes them, and reads the directory again. An error of
    /// `edit` saves nothing.
    pub async fn edit_accounts(
        &mut self,
        edit: impl FnOnce(
                &mut std::collections::HashMap<String, r3v3rs3_api::auth::Account>,
            ) -> Result<bool, Error>
            + Send,
    ) -> Result<(), Error> {
        let mut accounts = self.storage.load_accounts().await?;
        if edit(&mut accounts)? {
            self.storage.save_accounts(&accounts).await?;
            self.reload_accounts().await;
        }
        Ok(())
    }

    /// Reads the accounts again. A failed read keeps the directory.
    pub async fn reload_accounts(&mut self) {
        let Some(directory) = load_account_directory(self.storage.as_ref()).await else {
            return;
        };
        self.accounts.send_if_modified(|current| {
            if **current == directory {
                return false;
            }
            *current = Arc::new(directory);
            true
        });
    }

    async fn reload_config(&mut self) {
        let config = self.storage.load_app_config().await;
        if config != self.config {
            self.apply_config(config).await;
        }
    }

    async fn reload_certs(&mut self) {
        let certs = self.storage.load_certs().await;
        if self.certs.replace_stored(certs) {
            self.update_certs().await;
            self.reload_proxies().await;
        }
    }

    async fn reload_acmes(&mut self) {
        self.acmes = self.storage.load_acmes().await.into_iter().collect();
        self.update_acmes().await;
    }

    async fn reload_ports(&mut self) {
        let entries = self.storage.load_ports().await;
        let ids = entries.iter().map(|entry| entry.id).collect::<HashSet<_>>();
        let removed = self
            .ports
            .entries()
            .map(|entry| entry.id)
            .filter(|id| !ids.contains(id))
            .collect::<Vec<_>>();
        let mut changed = false;
        for id in removed {
            changed |= self.ports.delete(id);
        }
        for entry in entries {
            match PortContext::new(entry) {
                Ok(ctx) => changed |= self.ports.update(ctx),
                Err(err) => error!(%err, "failed to create proxy state"),
            }
        }
        if changed {
            log_save_error(self.update_ports().await);
            self.reload_proxies().await;
        }
    }

    async fn reload_manual_proxies(&mut self) {
        let entries = manual_proxies(self.storage.load_proxies().await);
        if self.proxies.replace_manual(entries) {
            self.publish_proxies();
            self.reload_proxies().await;
        }
    }

    async fn reload_cdn_ranges(&mut self) {
        if let Some(ranges) = self.storage.load_cdn_ranges().await {
            crate::cdn::install(ranges);
        }
    }

    /// Serves the challenges of the store, and tells the leader that this node serves them.
    async fn reload_challenges(&mut self) {
        let Some(challenges) = self.storage.load_challenges().await else {
            return;
        };
        self.set_challenges(challenges.clone()).await;
        log_save_error(self.storage.ack_challenges(&challenges).await);
    }

    /// Purges the caches that another node purged.
    async fn reload_cache_purges(&mut self) {
        let purges = self.storage.load_cache_purges().await;
        self.registries.caches.apply_purges(purges);
    }

    /// Adds a certificate that an ACME order issued. The server uses the certificate even when the
    /// storage cannot save it.
    async fn add_issued_cert(&mut self, cert: Arc<crate::certs::Cert>) {
        log_save_error(self.storage.save_cert(&cert).await);
        self.certs.add(cert);
        self.update_certs().await;
        self.reload_proxies().await;
    }

    /// Runs an RPC method when the role of the caller allows it. A method that changes the stored
    /// state fails before it changes anything when the storage cannot write. A successful call
    /// adds the audit log entry of the method.
    async fn call_method(
        &mut self,
        caller: &Caller,
        method: &mut dyn super::rpc::ErasedRpcMethod,
    ) -> Result<Box<dyn std::any::Any + Send + Sync>, Error> {
        caller.authorize(method.permission())?;
        if let Some(proxy) = method.proxy_scope() {
            caller.ensure_visible(proxy)?;
        }
        if method.mutates() {
            self.storage.ensure_writable().await?;
        }
        let record = method.audit();
        self.created_id = None;
        let output = method.call(self, caller).await?;
        if let Some(record) = record {
            self.record_audit(caller, record);
        }
        Ok(output)
    }

    /// Records a change of an account. The calls of the admin API itself have no account, so they
    /// are not recorded.
    fn record_audit(&self, caller: &Caller, mut record: AuditRecord) {
        if caller.username.is_empty() {
            return;
        }
        if record.resource_id.is_none() {
            record.resource_id = self.created_id.map(|id| id.to_string());
        }
        self.audit.record(&caller.username, caller.client, record);
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

    /// Applies the port list to the listeners. The callers save the ports before they change the
    /// list. An error means that the proxies without the removed ports were not saved.
    pub async fn update_ports(&mut self) -> Result<(), Error> {
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
        let saved = if self.proxies.remove_incompatible_ports(&entries) {
            self.update_proxies().await
        } else {
            Ok(())
        };
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
        saved
    }

    /// Saves the port list after `edit` changes a copy of it. The callers save before they change
    /// the list, so a port list that cannot be saved is not applied.
    pub async fn save_ports_with(
        &self,
        edit: impl FnOnce(&mut Vec<PortEntry>) + Send,
    ) -> Result<(), Error> {
        let mut entries = self.ports.entries().cloned().collect::<Vec<_>>();
        edit(&mut entries);
        self.storage.save_ports(&entries).await
    }

    /// Saves the manual proxies and sends the proxy list to the event subscribers.
    pub async fn update_proxies(&mut self) -> Result<(), Error> {
        let manual = self
            .proxies
            .entries()
            .filter(|entry| !entry.is_discovered())
            .cloned()
            .collect::<Vec<_>>();
        self.storage.save_proxies(&manual).await?;
        self.publish_proxies();
        Ok(())
    }

    /// Saves a changed proxy list and applies it to the ports. When the storage fails, the list
    /// returns to `previous`.
    pub async fn commit_proxies(&mut self, previous: Vec<ProxyEntry>) -> Result<(), Error> {
        let saved = self.update_proxies().await;
        if saved.is_err() {
            self.proxies = previous.into_iter().collect();
            self.publish_proxies();
        }
        self.reload_proxies().await;
        saved
    }

    /// Saves the access lists, and applies them to the proxies after the storage accepts them.
    pub async fn commit_access_lists(&mut self, lists: Vec<AccessListEntry>) -> Result<(), Error> {
        self.storage.save_access_lists(&lists).await?;
        self.access_lists = lists;
        self.reload_proxies().await;
        Ok(())
    }

    async fn reload_access_lists(&mut self) {
        let lists = self.storage.load_access_lists().await;
        if lists != self.access_lists {
            self.access_lists = lists;
            self.reload_proxies().await;
        }
    }

    /// Sends the proxy list without the password hashes and the token digests.
    fn publish_proxies(&self) {
        let entries = self
            .proxies
            .entries()
            .cloned()
            .map(|mut entry| {
                super::credentials::mask_proxy(&mut entry.proxy);
                entry
            })
            .collect::<Vec<_>>();
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

    pub async fn update_port(&mut self, ctx: PortContext) -> Result<(), Error> {
        if !self.ports.update(ctx) {
            return Ok(());
        }
        let saved = self.update_ports().await;
        self.reload_proxies().await;
        saved
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
                .map(|entry| with_access_lists(entry.clone(), &self.access_lists))
                .collect();
            let span = span!(Level::INFO, "port", resource_id = ctx.entry.id.to_string());
            if let Err(err) = ctx
                .setup(
                    &ports,
                    &self.certs,
                    proxies,
                    &self.sessions,
                    &self.registries,
                )
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

        self.reload_proxies().await;
        if self.leader {
            self.run_leader_tasks().await;
        }
    }

    async fn remove_expired_certs(&mut self) {
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
            // The other nodes of a cluster remove the certificate after the store change.
            log_save_error(self.storage.delete_cert(*id).await);
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

    /// Orders the due certificates. In a cluster only the leader orders, and a node that takes the
    /// lead orders the due certificates at once.
    async fn start_http_challenges(&mut self) {
        if !self.leader {
            return;
        }
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
        let acme_exec = self.config.acme_exec.clone();
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
                    .request(&target, dns_resolver, &acme_exec)
                    .instrument(span.clone())
                    .await
                {
                    Ok(request) => orders.push(request),
                    Err(err) => {
                        span.in_scope(|| error!("failed to request challenge: {}", err));
                        let _ = command
                            .send(ServerCommand::AcmeOrderFinished {
                                target,
                                error: Some(format!("{err:#}")),
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
        let challenges = ServedChallenges::default();
        log_save_error(self.storage.save_challenges(&challenges).await);
        self.set_challenges(challenges).await;
    }

    async fn continue_http_challenges(&mut self, orders: Vec<AcmeOrder>) {
        let challenges = self.serve_challenges(&orders).await;

        let command = self.command_sender.clone();
        let storage = self.storage.clone();
        tokio::task::spawn(async move {
            // The ACME server can validate a challenge on any node of a cluster.
            storage.wait_for_challenges(&challenges).await;
            for mut order in orders {
                let span = span!(
                    Level::INFO,
                    "acme",
                    resource_id = order.target.acme_id.to_string()
                );
                let error = match order.start_challenge().instrument(span.clone()).await {
                    Ok(cert) => {
                        span.in_scope(|| {
                            info!(id = cert.id().to_string(), "acme request completed");
                        });
                        let _ = command
                            .send(ServerCommand::AddCert {
                                cert: Arc::new(cert),
                            })
                            .await;
                        None
                    }
                    Err(err) => {
                        span.in_scope(|| error!(%err, "failed to start challenge"));
                        Some(format!("{err:#}"))
                    }
                };
                let _ = command
                    .send(ServerCommand::AcmeOrderFinished {
                        target: order.target,
                        error,
                    })
                    .await;
            }
        });
    }

    /// Serves the HTTP-01 and TLS-ALPN-01 challenges of the orders, and returns every served
    /// challenge. Orders of an earlier batch can still be running, so their challenges stay served.
    async fn serve_challenges(&mut self, orders: &[AcmeOrder]) -> ServedChallenges {
        let mut challenges = ServedChallenges {
            http: (*self.http_challenges).clone(),
            tls_alpn: self.tls_alpn_challenges.clone(),
        };
        challenges.http.extend(
            orders
                .iter()
                .flat_map(|order| order.http_challenges.clone()),
        );
        challenges.tls_alpn.extend(
            orders
                .iter()
                .flat_map(|order| order.tls_alpn_challenges.clone()),
        );
        log_save_error(self.storage.save_challenges(&challenges).await);
        self.set_challenges(challenges.clone()).await;
        challenges
    }

    async fn set_challenges(&mut self, challenges: ServedChallenges) {
        let previous = self.challenge_addrs();
        self.http_challenges = Arc::new(challenges.http);
        self.tls_alpn_challenges = challenges.tls_alpn;
        self.tls_alpn_challenge = tls_alpn_config(&self.tls_alpn_challenges);
        let addrs = self.challenge_addrs();
        // DNS-01 orders need no listener.
        if !addrs.is_empty() || addrs != previous {
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
        config.keep_file_only(&self.config);
        discovery::validate_config(&config.discovery, &self.certs)?;
        config.notifications.validate()?;
        self.storage.save_app_config(&config).await?;
        self.apply_config(config).await;
        Ok(())
    }

    /// Applies a config that the storage already keeps.
    async fn apply_config(&mut self, config: AppConfig) {
        let changed = discovery::changed_providers(&self.config.discovery, &config.discovery);
        self.config.discovery.clone_from(&config.discovery);
        for provider in changed {
            self.restart_discovery(provider).await;
        }
        self.config.clone_from(&config);
        self.sessions.set_config(config.admin);
        let _ = self.br_sender.send(ServerEvent::AppConfigUpdated {
            config: Box::new(config.masked()),
        });
    }

    /// A new id that no ACME entry, port, proxy or access list uses. The audit log entry of the
    /// running RPC method gets the id.
    pub fn generate_id(&mut self) -> ShortId {
        const TABLE: &[u8] = b"bcdfghjklmnpqrstvwxyz";

        let used_ids = self
            .acmes
            .entries()
            .map(|acme| acme.id)
            .chain(self.ports.entries().map(|port| port.id))
            .chain(self.proxies.entries().map(|site| site.id))
            .chain(self.access_lists.iter().map(|list| list.id))
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
                self.created_id = Some(id);
                return id;
            }
        }
    }

    pub async fn shutdown(self) {
        std::mem::drop(self.quic_pool);
    }
}
