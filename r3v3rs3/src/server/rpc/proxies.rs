use super::RpcMethod;
use crate::accounts::{Caller, Permission};
use crate::audit::AuditRecord;
use crate::proxy::tls::upstream_client_config;
use crate::server::credentials::{mask_proxy, restore_secrets, seal};
use crate::server::state::ServerState;
use r3v3rs3_api::audit::AuditAction;
use r3v3rs3_api::error::Error;
use r3v3rs3_api::id::ShortId;
use r3v3rs3_api::policy::AuthPolicy;
use r3v3rs3_api::priority::StreamPriorityConfig;
use r3v3rs3_api::proxy::{HttpProxy, Proxy, ProxyEntry, ProxyKind, ProxyStatus};
use r3v3rs3_api::security_headers::{
    SecurityHeadersAudit, SecurityProfile, analyze_response_rules, apply_profile_to_rules,
};
use r3v3rs3_api::subject_name::SubjectName;
use r3v3rs3_api::vhost::VirtualHost;

pub struct GetProxyList;

#[async_trait::async_trait]
impl RpcMethod for GetProxyList {
    type Output = Vec<ProxyEntry>;

    async fn call(self, state: &mut ServerState) -> Result<Self::Output, Error> {
        let entries = state
            .proxies
            .contexts()
            .map(|ctx| {
                let mut entry = masked(ctx.entry.clone());
                entry.status = Some(proxy_status(state, ctx));
                entry
            })
            .collect();
        Ok(entries)
    }

    fn restrict(mut output: Self::Output, caller: &Caller) -> Self::Output {
        output.retain(|entry| caller.can_see(entry.id));
        output
    }
}

pub struct GetProxy {
    pub id: ShortId,
}

#[async_trait::async_trait]
impl RpcMethod for GetProxy {
    type Output = ProxyEntry;

    async fn call(self, state: &mut ServerState) -> Result<Self::Output, Error> {
        state
            .proxies
            .get(self.id)
            .map(|ctx| {
                let mut entry = masked(ctx.entry.clone());
                entry.status = Some(proxy_status(state, ctx));
                entry
            })
            .ok_or(Error::IdNotFound {
                id: self.id.to_string(),
            })
    }

    fn proxy_scope(&self) -> Option<ShortId> {
        Some(self.id)
    }
}

pub struct GetProxyStatus {
    pub id: ShortId,
}

#[async_trait::async_trait]
impl RpcMethod for GetProxyStatus {
    type Output = ProxyStatus;

    async fn call(self, state: &mut ServerState) -> Result<Self::Output, Error> {
        state
            .proxies
            .get(self.id)
            .map(|ctx| proxy_status(state, ctx))
            .ok_or(Error::IdNotFound {
                id: self.id.to_string(),
            })
    }

    fn proxy_scope(&self) -> Option<ShortId> {
        Some(self.id)
    }
}

pub(crate) fn proxy_status(
    state: &ServerState,
    ctx: &crate::server::proxy_list::ProxyContext,
) -> ProxyStatus {
    ProxyStatus {
        state: ctx.status.state,
        upstreams: state.registries.groups.snapshot(ctx.entry.id),
        srv: match &ctx.entry.proxy.kind {
            ProxyKind::Http(http) => state.registries.srv.snapshot(&http.srv_names()),
            _ => Vec::new(),
        },
    }
}

pub struct PurgeProxyCache {
    pub id: ShortId,
    pub path: Option<String>,
}

#[async_trait::async_trait]
impl RpcMethod for PurgeProxyCache {
    type Output = ();
    const PERMISSION: Permission = Permission::EditProxies;

    async fn call(self, state: &mut ServerState) -> Result<Self::Output, Error> {
        if state.proxies.get(self.id).is_none() {
            return Err(Error::IdNotFound {
                id: self.id.to_string(),
            });
        }
        if let Some(path) = &self.path
            && !path.trim().is_empty()
        {
            state.registries.caches.purge_path(self.id, path);
            return Ok(());
        }
        let at = state.registries.caches.purge(self.id);
        state.storage.purge_shared_cache(self.id, at).await
    }

    fn proxy_scope(&self) -> Option<ShortId> {
        Some(self.id)
    }

    fn audit(&self) -> Option<AuditRecord> {
        Some(AuditRecord::new(AuditAction::PurgeProxyCache).id(self.id))
    }
}

pub struct DeleteProxy {
    pub id: ShortId,
}

#[async_trait::async_trait]
impl RpcMethod for DeleteProxy {
    type Output = ();
    const MUTATES: bool = true;
    const PERMISSION: Permission = Permission::EditProxies;

    /// Removes the proxy from the proxy lists of the accounts after the deletion.
    async fn call(self, state: &mut ServerState) -> Result<Self::Output, Error> {
        ensure_manual(state, self.id)?;
        let previous = proxy_entries(state);
        state.proxies.delete(self.id)?;
        state.commit_proxies(previous).await?;
        state.revoke_proxy(self.id).await
    }

    fn proxy_scope(&self) -> Option<ShortId> {
        Some(self.id)
    }

    fn audit(&self) -> Option<AuditRecord> {
        Some(AuditRecord::new(AuditAction::DeleteProxy).id(self.id))
    }
}

pub struct AddProxy {
    pub entry: Proxy,
    /// The account whose proxy list gets the new proxy.
    pub owner: Option<String>,
}

#[async_trait::async_trait]
impl RpcMethod for AddProxy {
    type Output = ();
    const MUTATES: bool = true;
    const PERMISSION: Permission = Permission::EditProxies;

    async fn call(self, state: &mut ServerState) -> Result<Self::Output, Error> {
        self.call_with_caller(state, &Caller::system()).await
    }

    async fn call_with_caller(
        self,
        state: &mut ServerState,
        caller: &Caller,
    ) -> Result<Self::Output, Error> {
        validate_proxy_rules(&self.entry, None, caller, state)?;
        let proxy = seal(self.entry).await?;
        let id = state.generate_id();
        // The account gets the id first, so a failed account save adds no proxy that its creator
        // cannot see.
        if let Some(owner) = &self.owner {
            state.grant_proxy(owner, id).await?;
        }
        let previous = proxy_entries(state);
        if state.proxies.set((id, proxy).into()) {
            state.commit_proxies(previous).await?;
        }
        Ok(())
    }

    /// The server adds the generated id to the entry.
    fn audit(&self) -> Option<AuditRecord> {
        Some(AuditRecord::new(AuditAction::AddProxy).summary(self.entry.name.clone()))
    }
}

pub struct UpdateProxy {
    pub entry: ProxyEntry,
}

#[async_trait::async_trait]
impl RpcMethod for UpdateProxy {
    type Output = ();
    const MUTATES: bool = true;
    const PERMISSION: Permission = Permission::EditProxies;

    async fn call(self, state: &mut ServerState) -> Result<Self::Output, Error> {
        self.call_with_caller(state, &Caller::system()).await
    }

    /// A user or a token without a new secret keeps its current hash.
    async fn call_with_caller(
        self,
        state: &mut ServerState,
        caller: &Caller,
    ) -> Result<Self::Output, Error> {
        ensure_manual(state, self.entry.id)?;
        let mut proxy = self.entry.proxy;
        if let Some(ctx) = state.proxies.get(self.entry.id) {
            restore_secrets(&mut proxy, &ctx.entry.proxy);
        }
        validate_proxy_rules(&proxy, Some(self.entry.id), caller, state)?;
        let proxy = seal(proxy).await?;
        let previous = proxy_entries(state);
        if state.proxies.set((self.entry.id, proxy).into()) {
            state.commit_proxies(previous).await?;
        }
        Ok(())
    }

    fn proxy_scope(&self) -> Option<ShortId> {
        Some(self.entry.id)
    }

    fn audit(&self) -> Option<AuditRecord> {
        let record = AuditRecord::new(AuditAction::UpdateProxy).id(self.entry.id);
        Some(record.summary(self.entry.proxy.name.clone()))
    }
}

pub enum StreamPriorityMutation {
    Replace(StreamPriorityConfig),
    Patch {
        enabled: bool,
        inject_header: Option<bool>,
        adaptive_concurrency: Option<bool>,
    },
}

impl StreamPriorityMutation {
    fn apply(self, priority: &mut StreamPriorityConfig) {
        match self {
            Self::Replace(config) => {
                *priority = config;
            }
            Self::Patch {
                enabled,
                inject_header,
                adaptive_concurrency,
            } => {
                priority.enabled = enabled;
                if let Some(inject) = inject_header {
                    priority.inject_header = inject;
                }
                if let Some(adaptive) = adaptive_concurrency {
                    priority.adaptive_concurrency = adaptive;
                }
            }
        }
    }
}

pub struct UpdateProxyStreamPriority {
    pub id: ShortId,
    pub mutation: StreamPriorityMutation,
}

#[async_trait::async_trait]
impl RpcMethod for UpdateProxyStreamPriority {
    type Output = ();
    const MUTATES: bool = true;
    const PERMISSION: Permission = Permission::EditProxies;

    async fn call(self, state: &mut ServerState) -> Result<Self::Output, Error> {
        self.call_with_caller(state, &Caller::system()).await
    }

    async fn call_with_caller(
        self,
        state: &mut ServerState,
        caller: &Caller,
    ) -> Result<Self::Output, Error> {
        ensure_manual(state, self.id)?;
        let ctx = state
            .proxies
            .get(self.id)
            .ok_or(Error::IdNotFound {
                id: self.id.to_string(),
            })?;
        let mut proxy = ctx.entry.proxy.clone();
        match &mut proxy.kind {
            ProxyKind::Http(http) => self.mutation.apply(&mut http.stream_priority),
            _ => return Err(Error::NotHttpProxy { id: self.id }),
        }
        validate_proxy_rules(&proxy, Some(self.id), caller, state)?;
        let proxy = seal(proxy).await?;
        let previous = proxy_entries(state);
        if state.proxies.set((self.id, proxy).into()) {
            state.commit_proxies(previous).await?;
        }
        Ok(())
    }

    fn proxy_scope(&self) -> Option<ShortId> {
        Some(self.id)
    }

    fn audit(&self) -> Option<AuditRecord> {
        let record = AuditRecord::new(AuditAction::UpdateProxy).id(self.id);
        Some(record.summary("update_proxy_stream_priority"))
    }
}

pub struct ApplyProxySecurityHeadersProfile {
    pub id: ShortId,
    pub profile: SecurityProfile,
}

#[async_trait::async_trait]
impl RpcMethod for ApplyProxySecurityHeadersProfile {
    type Output = SecurityHeadersAudit;
    const MUTATES: bool = true;
    const PERMISSION: Permission = Permission::EditProxies;

    async fn call(self, state: &mut ServerState) -> Result<Self::Output, Error> {
        self.call_with_caller(state, &Caller::system()).await
    }

    async fn call_with_caller(
        self,
        state: &mut ServerState,
        caller: &Caller,
    ) -> Result<Self::Output, Error> {
        ensure_manual(state, self.id)?;
        let ctx = state
            .proxies
            .get(self.id)
            .ok_or(Error::IdNotFound {
                id: self.id.to_string(),
            })?;
        let mut proxy = ctx.entry.proxy.clone();
        let audit = match &mut proxy.kind {
            ProxyKind::Http(http) => {
                let updated = apply_profile_to_rules(&http.headers.response, self.profile);
                http.headers.response = updated;
                analyze_response_rules(&http.headers.response)
            }
            _ => return Err(Error::NotHttpProxy { id: self.id }),
        };
        validate_proxy_rules(&proxy, Some(self.id), caller, state)?;
        let proxy = seal(proxy).await?;
        let previous = proxy_entries(state);
        if state.proxies.set((self.id, proxy).into()) {
            state.commit_proxies(previous).await?;
        }
        Ok(audit)
    }

    fn proxy_scope(&self) -> Option<ShortId> {
        Some(self.id)
    }

    fn audit(&self) -> Option<AuditRecord> {
        let record = AuditRecord::new(AuditAction::UpdateProxy).id(self.id);
        Some(record.summary("apply_proxy_security_headers_profile"))
    }
}

/// The entry without the password hashes and the token digests.
fn masked(mut entry: ProxyEntry) -> ProxyEntry {
    mask_proxy(&mut entry.proxy);
    entry
}

fn proxy_entries(state: &ServerState) -> Vec<ProxyEntry> {
    state.proxies.entries().cloned().collect()
}

/// Service discovery owns a discovered proxy, so the API cannot change or delete it.
fn ensure_manual(state: &ServerState, id: ShortId) -> Result<(), Error> {
    match state.proxies.get(id) {
        Some(ctx) if ctx.entry.is_discovered() => Err(Error::ProxyReadOnly { id }),
        _ => Ok(()),
    }
}

/// Rejects a proxy that uses an access list that does not exist. A discovered proxy skips this
/// check, and its routes with a missing access list reject every client.
fn ensure_access_lists(proxy: &Proxy, state: &ServerState) -> Result<(), Error> {
    let missing = proxy
        .kind
        .access_lists()
        .into_iter()
        .find(|id| !state.access_lists.iter().any(|list| list.id == *id));
    match missing {
        Some(id) => Err(Error::AccessListNotFound { id }),
        None => Ok(()),
    }
}

fn validate_proxy_rules(
    proxy: &Proxy,
    proxy_id: Option<ShortId>,
    caller: &Caller,
    state: &ServerState,
) -> Result<(), Error> {
    validate_proxy(proxy, state)?;
    ensure_access_lists(proxy, state)?;
    ensure_client_cert_allowed(proxy, caller)?;
    ensure_forward_auth_allowed(proxy, caller)?;
    ensure_no_unseen_host_overlap(proxy, proxy_id, caller, state)?;
    ensure_no_unseen_port_conflict(proxy, proxy_id, caller, state)
}

fn ensure_forward_auth_allowed(proxy: &Proxy, caller: &Caller) -> Result<(), Error> {
    let ProxyKind::Http(http) = &proxy.kind else {
        return Ok(());
    };
    let AuthPolicy::Forward(forward) = &http.auth else {
        return Ok(());
    };
    let host = forward.url.0.host_str().unwrap_or_default();
    if crate::proxy::http::auth::forward::is_cloud_metadata_host(host) {
        return Err(Error::Forbidden);
    }
    if caller.proxies.is_some() && crate::proxy::http::auth::forward::is_loopback_host(host) {
        return Err(Error::Forbidden);
    }
    Ok(())
}

/// Checks the upstream timeouts and the health check, and that the upstream client certificate of
/// the proxy is a client certificate with a private key.
pub fn validate_proxy(proxy: &Proxy, state: &ServerState) -> Result<(), Error> {
    proxy.kind.validate_upstream()?;
    upstream_client_config(&state.certs, proxy.kind.client_cert()).map(|_| ())
}

fn ensure_client_cert_allowed(proxy: &Proxy, caller: &Caller) -> Result<(), Error> {
    if proxy.kind.client_cert().is_some() {
        caller.authorize(Permission::Edit)?;
    }
    Ok(())
}

fn subject_names_overlap(a: &SubjectName, b: &SubjectName) -> bool {
    match (a, b) {
        (SubjectName::DnsName(n1), SubjectName::DnsName(n2)) => n1.eq_ignore_ascii_case(n2),
        (SubjectName::IPAddress(ip1), SubjectName::IPAddress(ip2)) => ip1 == ip2,
        (SubjectName::WildcardDnsName(w1), SubjectName::WildcardDnsName(w2)) => {
            w1.eq_ignore_ascii_case(w2)
        }
        (SubjectName::WildcardDnsName(_), SubjectName::DnsName(d)) => a.test(d),
        (SubjectName::DnsName(d), SubjectName::WildcardDnsName(_)) => b.test(d),
        _ => false,
    }
}

fn vhosts_overlap(a: &VirtualHost, b: &VirtualHost) -> bool {
    match (a, b) {
        (VirtualHost::SubjectName(s1), VirtualHost::SubjectName(s2)) => {
            subject_names_overlap(s1, s2)
        }
        (VirtualHost::Regex(r), VirtualHost::SubjectName(s))
        | (VirtualHost::SubjectName(s), VirtualHost::Regex(r)) => match s {
            SubjectName::DnsName(name) => r.is_match(name),
            SubjectName::IPAddress(ip) => r.is_match(&ip.to_string()),
            SubjectName::WildcardDnsName(name) => {
                r.as_str() == name || r.is_match(&format!("sub.{name}"))
            }
        },
        (VirtualHost::Regex(r1), VirtualHost::Regex(r2)) => r1.as_str() == r2.as_str(),
    }
}

fn has_vhost_overlap(vhosts_a: &[VirtualHost], vhosts_b: &[VirtualHost]) -> bool {
    for a in vhosts_a {
        for b in vhosts_b {
            if vhosts_overlap(a, b) {
                return true;
            }
        }
    }
    false
}

fn http_proxies_overlap(a: &HttpProxy, b: &HttpProxy) -> bool {
    if a.vhosts.is_empty() || b.vhosts.is_empty() {
        return true;
    }
    has_vhost_overlap(&a.vhosts, &b.vhosts)
}

fn shares_any_port(a: &[ShortId], b: &[ShortId]) -> bool {
    a.iter().any(|p| b.contains(p))
}

fn ensure_no_unseen_host_overlap(
    proxy: &Proxy,
    proxy_id: Option<ShortId>,
    caller: &Caller,
    state: &ServerState,
) -> Result<(), Error> {
    let Some(allowed) = &caller.proxies else {
        return Ok(());
    };
    let ProxyKind::Http(http) = &proxy.kind else {
        return Ok(());
    };
    for unseen in state.proxies.entries() {
        if Some(unseen.id) == proxy_id || allowed.contains(&unseen.id) {
            continue;
        }
        let ProxyKind::Http(unseen_http) = &unseen.proxy.kind else {
            continue;
        };
        if shares_any_port(&proxy.ports, &unseen.proxy.ports)
            && http_proxies_overlap(http, unseen_http)
        {
            return Err(Error::Forbidden);
        }
    }
    Ok(())
}

fn ensure_no_unseen_port_conflict(
    proxy: &Proxy,
    proxy_id: Option<ShortId>,
    caller: &Caller,
    state: &ServerState,
) -> Result<(), Error> {
    let Some(allowed) = &caller.proxies else {
        return Ok(());
    };
    let ProxyKind::Tcp(_) = &proxy.kind else {
        return Ok(());
    };
    for unseen in state.proxies.entries() {
        if Some(unseen.id) == proxy_id || allowed.contains(&unseen.id) {
            continue;
        }
        if shares_any_port(&proxy.ports, &unseen.proxy.ports) {
            return Err(Error::Forbidden);
        }
    }
    Ok(())
}
