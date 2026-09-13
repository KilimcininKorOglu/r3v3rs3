use super::RpcMethod;
use crate::proxy::tls::upstream_client_config;
use crate::server::credentials::seal;
use crate::server::state::ServerState;
use r3v3rs3_api::error::Error;
use r3v3rs3_api::id::ShortId;
use r3v3rs3_api::proxy::{Proxy, ProxyEntry, ProxyStatus};

pub struct GetProxyList;

#[async_trait::async_trait]
impl RpcMethod for GetProxyList {
    type Output = Vec<ProxyEntry>;

    async fn call(self, state: &mut ServerState) -> Result<Self::Output, Error> {
        Ok(state.proxies.entries().cloned().collect())
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
            .map(|ctx| ctx.entry.clone())
            .ok_or(Error::IdNotFound {
                id: self.id.to_string(),
            })
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
            .map(|ctx| ctx.status)
            .ok_or(Error::IdNotFound {
                id: self.id.to_string(),
            })
    }
}

pub struct PurgeProxyCache {
    pub id: ShortId,
}

#[async_trait::async_trait]
impl RpcMethod for PurgeProxyCache {
    type Output = ();

    async fn call(self, state: &mut ServerState) -> Result<Self::Output, Error> {
        if state.proxies.get(self.id).is_none() {
            return Err(Error::IdNotFound {
                id: self.id.to_string(),
            });
        }
        crate::proxy::http::cache::purge(self.id);
        Ok(())
    }
}

pub struct DeleteProxy {
    pub id: ShortId,
}

#[async_trait::async_trait]
impl RpcMethod for DeleteProxy {
    type Output = ();

    async fn call(self, state: &mut ServerState) -> Result<Self::Output, Error> {
        state.proxies.delete(self.id)?;
        state.update_proxies().await;
        state.reload_proxies().await;
        Ok(())
    }
}

pub struct AddProxy {
    pub entry: Proxy,
}

#[async_trait::async_trait]
impl RpcMethod for AddProxy {
    type Output = ();

    async fn call(self, state: &mut ServerState) -> Result<Self::Output, Error> {
        validate_proxy(&self.entry, state)?;
        let proxy = seal(self.entry).await?;
        if state.proxies.set((state.generate_id(), proxy).into()) {
            state.update_proxies().await;
            state.reload_proxies().await;
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

    async fn call(self, state: &mut ServerState) -> Result<Self::Output, Error> {
        validate_proxy(&self.entry.proxy, state)?;
        let proxy = seal(self.entry.proxy).await?;
        if state.proxies.set((self.entry.id, proxy).into()) {
            state.update_proxies().await;
            state.reload_proxies().await;
        }
        Ok(())
    }
}

/// Checks the upstream timeouts and the health check, and that the upstream client certificate of
/// the proxy is a client certificate with a private key.
fn validate_proxy(proxy: &Proxy, state: &ServerState) -> Result<(), Error> {
    proxy.kind.validate_upstream()?;
    upstream_client_config(&state.certs, proxy.kind.client_cert()).map(|_| ())
}
