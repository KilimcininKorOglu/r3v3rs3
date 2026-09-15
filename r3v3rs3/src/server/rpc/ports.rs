use super::RpcMethod;
use crate::proxy::{tls::validate_client_auth, PortContext};
use crate::server::state::ServerState;
use network_interface::NetworkInterfaceConfig;
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
        if state.ports.delete(self.id) {
            state.update_ports().await;
            state.reload_proxies().await;
            Ok(())
        } else {
            Err(Error::IdNotFound {
                id: self.id.to_string(),
            })
        }
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
            state.update_port(PortContext::new(entry)?).await;
            Ok(())
        }
    }
}

/// Checks the parts of the port config that depend on the certificates.
fn validate_port(port: &Port, state: &ServerState) -> Result<(), Error> {
    match &port.opts.tls_termination {
        Some(tls) => validate_client_auth(tls, &state.certs),
        None => Ok(()),
    }
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
            state.update_port(PortContext::new(self.entry)?).await;
            Ok(())
        } else {
            Err(Error::IdNotFound {
                id: self.entry.id.to_string(),
            })
        }
    }
}

pub struct ResetPort {
    pub id: ShortId,
}

#[async_trait::async_trait]
impl RpcMethod for ResetPort {
    type Output = ();

    async fn call(self, state: &mut ServerState) -> Result<Self::Output, Error> {
        if state.ports.reset(self.id) {
            Ok(())
        } else {
            Err(Error::IdNotFound {
                id: self.id.to_string(),
            })
        }
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
