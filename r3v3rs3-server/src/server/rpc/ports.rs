use super::RpcMethod;
use crate::accounts::Permission;
use crate::audit::AuditRecord;
use crate::proxy::{PortContext, tls::validate_client_auth};
use crate::server::state::ServerState;
use network_interface::NetworkInterfaceConfig;
use r3v3rs3_api::audit::AuditAction;
use r3v3rs3_api::error::Error;
use r3v3rs3_api::id::ShortId;
use r3v3rs3_api::multiaddr::Multiaddr;
use r3v3rs3_api::port::{NetworkAddr, NetworkInterface, Port, PortEntry, PortStatus};

pub struct GetPortList;

impl RpcMethod for GetPortList {
    type Output = Vec<PortEntry>;

    async fn call(self, state: &mut ServerState) -> Result<Self::Output, Error> {
        let entries = state
            .ports
            .as_slice()
            .iter()
            .map(|port| {
                let mut entry = port.entry().clone();
                entry.status = Some(state.port_status(port));
                entry
            })
            .collect();
        Ok(entries)
    }
}

pub struct GetPort {
    pub id: ShortId,
}

impl RpcMethod for GetPort {
    type Output = PortEntry;

    async fn call(self, state: &mut ServerState) -> Result<Self::Output, Error> {
        state
            .ports
            .get(self.id)
            .map(|port| {
                let mut entry = port.entry().clone();
                entry.status = Some(state.port_status(port));
                entry
            })
            .ok_or(Error::IdNotFound {
                id: self.id.to_string(),
            })
    }
}

pub struct GetPortStatus {
    pub id: ShortId,
}

impl RpcMethod for GetPortStatus {
    type Output = PortStatus;

    async fn call(self, state: &mut ServerState) -> Result<Self::Output, Error> {
        state
            .ports
            .get(self.id)
            .map(|port| state.port_status(port))
            .ok_or(Error::IdNotFound {
                id: self.id.to_string(),
            })
    }
}

pub struct DeletePort {
    pub id: ShortId,
}

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

impl RpcMethod for AddPort {
    type Output = ();
    const MUTATES: bool = true;

    async fn call(self, state: &mut ServerState) -> Result<Self::Output, Error> {
        let entry: PortEntry = (state.generate_id(), self.entry).into();
        if state.ports.get(entry.id).is_some() {
            Err(Error::IdAlreadyExists { id: entry.id })
        } else {
            validate_port(&entry, state)?;
            let ctx = PortContext::new(entry, &state.config().http2, &state.config().http3)?;
            save_port(state, &ctx).await?;
            state.update_port(ctx).await
        }
    }

    /// The server adds the generated id to the entry.
    fn audit(&self) -> Option<AuditRecord> {
        Some(AuditRecord::new(AuditAction::AddPort).summary(port_summary(&self.entry)))
    }
}

/// Checks the port config, including the parts that depend on the certificates and on the
/// other ports.
fn validate_port(entry: &PortEntry, state: &ServerState) -> Result<(), Error> {
    let port = &entry.port;
    port.validate()?;
    let others = state.ports.as_slice().iter().map(|ctx| ctx.entry());
    let others = others
        .filter(|other| other.id != entry.id)
        .map(|o| &o.port.listen);
    let ssh = crate::firewall::ssh_ports(std::path::Path::new("/etc/ssh"));
    check_range_overlap(&port.listen, others, &ssh)?;
    check_transparent_upstreams(entry, state)?;
    if let Some(limits) = &port.opts.http2_limits {
        limits.validate()?;
    }
    if let Some(limits) = &port.opts.http3_limits {
        limits.validate()?;
    }
    match &port.opts.tls_termination {
        Some(tls) => validate_client_auth(tls, &state.certs),
        None => Ok(()),
    }
}

/// A transparent port sends from the client address, which the kernel refuses toward a
/// loopback address (measured: `EINVAL`), so no proxy of the port may have a loopback upstream.
fn check_transparent_upstreams(entry: &PortEntry, state: &ServerState) -> Result<(), Error> {
    if !entry.port.opts.transparent {
        return Ok(());
    }
    let upstreams = state
        .proxies
        .entries()
        .filter(|proxy| proxy.proxy.ports.contains(&entry.id))
        .flat_map(|proxy| proxy.proxy.kind.stream_upstreams());
    match upstreams
        .into_iter()
        .find(|up| up.ip_addr().is_ok_and(|ip| ip.is_loopback()))
    {
        Some(up) => Err(Error::TransparentLoopbackUpstream {
            addr: up.to_string(),
        }),
        None => Ok(()),
    }
}

/// A range diverts every port it names to one socket, so it must not cover a port that
/// another port of the same protocol or SSH listens on. Two single ports are not checked here:
/// the second one reports that its address is in use, as before ranges existed.
fn check_range_overlap<'a>(
    listen: &Multiaddr,
    others: impl Iterator<Item = &'a Multiaddr>,
    ssh: &[u16],
) -> Result<(), Error> {
    let Ok(span) = listen.port_range() else {
        return Ok(());
    };
    let udp = listen.is_udp();
    let overlap = |other: &Multiaddr| {
        other.is_udp() == udp
            && (listen.is_range() || other.is_range())
            && other.port_range().is_ok_and(|o| o.overlaps(&span))
    };
    if let Some(other) = others.into_iter().find(|o| overlap(o)) {
        return Err(Error::PortRangeOverlap {
            addr: listen.to_string(),
            other: other.to_string(),
        });
    }
    match ssh
        .iter()
        .find(|p| !udp && listen.is_range() && span.contains(**p))
    {
        Some(port) => Err(Error::PortRangeOverlap {
            addr: listen.to_string(),
            other: format!("ssh/{port}"),
        }),
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

impl RpcMethod for UpdatePort {
    type Output = ();
    const MUTATES: bool = true;

    async fn call(self, state: &mut ServerState) -> Result<Self::Output, Error> {
        if state.ports.get(self.entry.id).is_some() {
            validate_port(&self.entry, state)?;
            let ctx = PortContext::new(self.entry, &state.config().http2, &state.config().http3)?;
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

#[cfg(test)]
mod tests {
    use super::*;

    fn addrs(list: &[&str]) -> Vec<Multiaddr> {
        list.iter().map(|a| a.parse().unwrap()).collect()
    }

    fn check(listen: &str, others: &[&str], ssh: &[u16]) -> Result<(), Error> {
        check_range_overlap(&listen.parse().unwrap(), addrs(others).iter(), ssh)
    }

    /// A range sends every port it names to one socket, so a port of the same protocol inside
    /// it, or a range around an existing port, would steal that port's traffic.
    #[test]
    fn a_range_must_not_cover_another_port_of_its_protocol() {
        let relay = "/ip4/0.0.0.0/udp/49160-49240";
        let overlap = |r: Result<(), Error>| matches!(r, Err(Error::PortRangeOverlap { .. }));
        assert!(overlap(check(relay, &["/ip4/0.0.0.0/udp/49200"], &[])));
        assert!(overlap(check("/ip4/0.0.0.0/udp/49200", &[relay], &[])));
        assert!(overlap(check(
            relay,
            &["/ip4/0.0.0.0/udp/49000-49160"],
            &[]
        )));
        // Another protocol, or ports outside the range, do not collide.
        assert!(check(relay, &["/ip4/0.0.0.0/tcp/49200"], &[]).is_ok());
        assert!(
            check(
                relay,
                &["/ip4/0.0.0.0/udp/49241", "/ip4/0.0.0.0/udp/3478"],
                &[]
            )
            .is_ok()
        );
        // Two single ports keep the old behaviour: the second one reports the busy address.
        assert!(check("/ip4/0.0.0.0/udp/3478", &["/ip6/::/udp/3478"], &[]).is_ok());
    }

    /// A TCP range around the SSH port would divert SSH to r3v3rs3 and lock the operator out.
    #[test]
    fn a_tcp_range_must_not_cover_ssh() {
        let overlap = |r: Result<(), Error>| matches!(r, Err(Error::PortRangeOverlap { .. }));
        assert!(overlap(check("/ip4/0.0.0.0/tcp/1-100", &[], &[22])));
        assert!(overlap(check(
            "/ip4/0.0.0.0/tcp/2200-2300",
            &[],
            &[22, 2222]
        )));
        assert!(check("/ip4/0.0.0.0/udp/1-100", &[], &[22]).is_ok());
        assert!(check("/ip4/0.0.0.0/tcp/2223-2300", &[], &[22, 2222]).is_ok());
    }
}
