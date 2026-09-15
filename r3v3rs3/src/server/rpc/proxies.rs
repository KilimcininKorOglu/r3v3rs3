use super::RpcMethod;
use crate::accounts::{Caller, Permission};
use crate::proxy::tls::upstream_client_config;
use crate::server::credentials::{mask_proxy, restore_secrets, seal};
use crate::server::state::ServerState;
use r3v3rs3_api::error::Error;
use r3v3rs3_api::id::ShortId;
use r3v3rs3_api::proxy::{Proxy, ProxyEntry, ProxyStatus};

pub struct GetProxyList;

#[async_trait::async_trait]
impl RpcMethod for GetProxyList {
    type Output = Vec<ProxyEntry>;

    async fn call(self, state: &mut ServerState) -> Result<Self::Output, Error> {
        Ok(state.proxies.entries().cloned().map(masked).collect())
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
            .map(|ctx| masked(ctx.entry.clone()))
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
            .map(|ctx| ProxyStatus {
                state: ctx.status.state,
                upstreams: state.registries.groups.snapshot(self.id),
            })
            .ok_or(Error::IdNotFound {
                id: self.id.to_string(),
            })
    }

    fn proxy_scope(&self) -> Option<ShortId> {
        Some(self.id)
    }
}

pub struct PurgeProxyCache {
    pub id: ShortId,
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
        let at = state.registries.caches.purge(self.id);
        state.storage.purge_shared_cache(self.id, at).await
    }

    fn proxy_scope(&self) -> Option<ShortId> {
        Some(self.id)
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
        validate_proxy(&self.entry, state)?;
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
}

pub struct UpdateProxy {
    pub entry: ProxyEntry,
}

#[async_trait::async_trait]
impl RpcMethod for UpdateProxy {
    type Output = ();
    const MUTATES: bool = true;
    const PERMISSION: Permission = Permission::EditProxies;

    /// A user or a token without a new secret keeps its current hash.
    async fn call(self, state: &mut ServerState) -> Result<Self::Output, Error> {
        ensure_manual(state, self.entry.id)?;
        let mut proxy = self.entry.proxy;
        if let Some(ctx) = state.proxies.get(self.entry.id) {
            restore_secrets(&mut proxy, &ctx.entry.proxy);
        }
        validate_proxy(&proxy, state)?;
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

/// Checks the upstream timeouts and the health check, and that the upstream client certificate of
/// the proxy is a client certificate with a private key.
pub fn validate_proxy(proxy: &Proxy, state: &ServerState) -> Result<(), Error> {
    proxy.kind.validate_upstream()?;
    upstream_client_config(&state.certs, proxy.kind.client_cert()).map(|_| ())
}
