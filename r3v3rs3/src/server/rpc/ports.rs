use super::RpcMethod;
use crate::accounts::Permission;
use crate::audit::AuditRecord;
use crate::proxy::{PortContext, tls::validate_client_auth};
use crate::server::state::ServerState;
use network_interface::NetworkInterfaceConfig;
use r3v3rs3_api::audit::AuditAction;
use r3v3rs3_api::error::Error;
use r3v3rs3_api::id::ShortId;
use r3v3rs3_api::port::{NetworkAddr, NetworkInterface, Port, PortEntry, PortStatus};

pub struct GetPortList;

#[async_trait::async_trait]
impl RpcMethod for GetPortList {
    type Output = Vec<PortEntry>;

    async fn call(self, state: &mut ServerState) -> Result<Self::Output, Error> {
        Ok(state.ports.entries().cloned().collect())
    }
}

pub struct GetPort {
    pub id: ShortId,
}

#[async_trait::async_trait]
impl RpcMethod for GetPort {
    type Output = PortEntry;

    async fn call(self, state: &mut ServerState) -> Result<Self::Output, Error> {
        state
            .ports
            .get(self.id)
            .map(|port| port.entry().clone())
            .ok_or(Error::IdNotFound {
                id: self.id.to_string(),
            })
    }
}

pub struct GetPortStatus {
    pub id: ShortId,
}

#[async_trait::async_trait]
impl RpcMethod for GetPortStatus {
    type Output = PortStatus;

    async fn call(self, state: &mut ServerState) -> Result<Self::Output, Error> {
        state
            .ports
            .get(self.id)
            .map(|port| *port.status())
            .ok_or(Error::IdNotFound {
                id: self.id.to_string(),
            })
    }
}

pub struct DeletePort {
    pub id: ShortId,
}

#[async_trait::async_trait]
impl RpcMethod for DeletePort {
    type Output = ();
    const MUTATES: bool = true;

    async fn call(self, state: &mut ServerState) -> Result<Self::Output, Error> {
        if state.ports.get(self.id).is_none() {
            return Err(Error::IdNotFound {
                id: self.id.to_string(),
            });
        }
        let id = self.id;
        state
            .save_ports_with(|entries| entries.retain(|entry| entry.id != id))
            .await?;
        state.ports.delete(self.id);
        let saved = state.update_ports().await;
        state.reload_proxies().await;
        saved
    }

    fn audit(&self) -> Option<AuditRecord> {
        Some(AuditRecord::new(AuditAction::DeletePort).id(self.id))
    }
}

pub struct AddPort {
    pub entry: Port,
}

#[async_trait::async_trait]
impl RpcMethod for AddPort {
    type Output = ();
    const MUTATES: bool = true;

    async fn call(self, state: &mut ServerState) -> Result<Self::Output, Error> {
        let entry: PortEntry = (state.generate_id(), self.entry).into();
        if state.ports.get(entry.id).is_some() {
            Err(Error::IdAlreadyExists { id: entry.id })
        } else {
            validate_port(&entry.port, state)?;
            let ctx = PortContext::new(entry)?;
            save_port(state, &ctx).await?;
            state.update_port(ctx).await
        }
    }

    /// The server adds the generated id to the entry.
    fn audit(&self) -> Option<AuditRecord> {
        Some(AuditRecord::new(AuditAction::AddPort).summary(port_summary(&self.entry)))
    }
}

/// Checks the parts of the port config that depend on the certificates.
fn validate_port(port: &Port, state: &ServerState) -> Result<(), Error> {
    match &port.opts.tls_termination {
        Some(tls) => validate_client_auth(tls, &state.certs),
        None => Ok(()),
    }
}

/// The name and the listening address of a port for the audit log.
fn port_summary(port: &Port) -> String {
    format!("{} {}", port.name, port.listen).trim().to_string()
}

/// Saves the port list with the port of `ctx` added or replaced.
async fn save_port(state: &ServerState, ctx: &PortContext) -> Result<(), Error> {
    let entry = ctx.entry();
    state
        .save_ports_with(|entries| {
            match entries.iter_mut().find(|current| current.id == entry.id) {
                Some(current) => current.clone_from(entry),
                None => entries.push(entry.clone()),
            }
        })
        .await
}

pub struct UpdatePort {
    pub entry: PortEntry,
}

#[async_trait::async_trait]
impl RpcMethod for UpdatePort {
    type Output = ();
    const MUTATES: bool = true;

    async fn call(self, state: &mut ServerState) -> Result<Self::Output, Error> {
        if state.ports.get(self.entry.id).is_some() {
            validate_port(&self.entry.port, state)?;
            let ctx = PortContext::new(self.entry)?;
            save_port(state, &ctx).await?;
            state.update_port(ctx).await
        } else {
            Err(Error::IdNotFound {
                id: self.entry.id.to_string(),
            })
        }
    }

    fn audit(&self) -> Option<AuditRecord> {
        let record = AuditRecord::new(AuditAction::UpdatePort).id(self.entry.id);
        Some(record.summary(port_summary(&self.entry.port)))
    }
}

pub struct ResetPort {
    pub id: ShortId,
}

#[async_trait::async_trait]
impl RpcMethod for ResetPort {
    type Output = ();
    const PERMISSION: Permission = Permission::Edit;

    async fn call(self, state: &mut ServerState) -> Result<Self::Output, Error> {
        if state.ports.reset(self.id) {
            Ok(())
        } else {
            Err(Error::IdNotFound {
                id: self.id.to_string(),
            })
        }
    }

    fn audit(&self) -> Option<AuditRecord> {
        Some(AuditRecord::new(AuditAction::ResetPort).id(self.id))
    }
}

pub struct GetNetworkInterfaceList;

#[async_trait::async_trait]
impl RpcMethod for GetNetworkInterfaceList {
    type Output = Vec<NetworkInterface>;

    async fn call(self, _state: &mut ServerState) -> Result<Self::Output, Error> {
        Ok(network_interface::NetworkInterface::show()
            .map_err(|_| Error::FailedToListNetworkInterfaces)?
            .into_iter()
            .map(|iface| {
                let addrs = iface
                    .addr
                    .into_iter()
                    .map(|net| NetworkAddr {
                        ip: net.ip(),
                        mask: net.netmask(),
                    })
                    .collect::<Vec<_>>();
                NetworkInterface {
                    name: iface.name,
                    addrs,
                    mac: iface.mac_addr,
                }
            })
            .collect())
    }
}
