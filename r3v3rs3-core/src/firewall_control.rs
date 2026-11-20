//! Keeps the `inet r3v3rs3` table equal to the ports of the server. The server loop only hands
//! the wanted ports to a background task, so a slow or missing `nft` never holds up the loop.

use crate::firewall::{FirewallRules, render, render_removal, run_nft, ssh_ports};
use r3v3rs3_api::firewall::{FirewallConfig, FirewallStatus, HostPort, Transport};
use r3v3rs3_api::multiaddr::Multiaddr;
use std::{
    path::PathBuf,
    sync::{Arc, Mutex},
    time::SystemTime,
};
use tokio::sync::watch;
use tracing::{error, info};

/// The environment variable that overrides `[firewall] enabled`.
pub const ENABLED_ENV: &str = "R3V3RS3_FIREWALL_ENABLED";

/// What the table should hold.
#[derive(Debug, Clone, PartialEq, Eq)]
enum Wanted {
    /// No update was asked yet.
    Nothing,
    Apply(Vec<HostPort>),
    /// The firewall was turned off: the table goes.
    Remove,
}

/// The handle of the firewall task.
pub struct FirewallControl {
    wanted: watch::Sender<Wanted>,
    status: Arc<Mutex<FirewallStatus>>,
}

impl FirewallControl {
    /// Starts the task. `ssh_dir` holds `sshd_config`.
    pub fn spawn(ssh_dir: PathBuf) -> Self {
        let (wanted, receiver) = watch::channel(Wanted::Nothing);
        let status = Arc::new(Mutex::new(FirewallStatus::default()));
        tokio::spawn(run(receiver, status.clone(), ssh_dir));
        Self { wanted, status }
    }

    /// Asks for the table of `config` and the listening addresses of the ports. `was_enabled`
    /// tells whether the previous config managed the table, so turning it off removes it.
    pub fn update<'a>(
        &self,
        config: &FirewallConfig,
        listens: impl Iterator<Item = &'a Multiaddr>,
        app_ports: &[HostPort],
        was_enabled: bool,
    ) {
        let next = match enabled(config) {
            Ok(true) => Wanted::Apply(wanted_ports(config, listens, app_ports)),
            Ok(false) if was_enabled => Wanted::Remove,
            Ok(false) => return,
            Err(err) => {
                error!(%err, "the firewall setting is invalid");
                self.set_error(err);
                return;
            }
        };
        self.wanted.send_if_modified(|current| {
            let changed = *current != next;
            *current = next;
            changed
        });
    }

    pub fn status(&self) -> FirewallStatus {
        self.status.lock().map(|s| s.clone()).unwrap_or_default()
    }

    fn set_error(&self, err: String) {
        if let Ok(mut status) = self.status.lock() {
            status.error = Some(err);
        }
    }
}

/// `[firewall] enabled`, or the value of [`ENABLED_ENV`] when it is set.
pub fn enabled(config: &FirewallConfig) -> Result<bool, String> {
    match std::env::var(ENABLED_ENV) {
        Ok(value) => parse_bool(&value)
            .ok_or_else(|| format!("{ENABLED_ENV} must be true or false, not {value:?}")),
        Err(_) => Ok(config.enabled),
    }
}

fn parse_bool(value: &str) -> Option<bool> {
    match value.trim().to_ascii_lowercase().as_str() {
        "true" | "1" | "yes" => Some(true),
        "false" | "0" | "no" => Some(false),
        _ => None,
    }
}

/// The ports that clients outside the host reach: every listening address that is not a
/// loopback address, as TCP or as UDP for UDP and QUIC ports, plus the extra ports and the
/// host ports of the running apps.
pub fn wanted_ports<'a>(
    config: &FirewallConfig,
    listens: impl Iterator<Item = &'a Multiaddr>,
    app_ports: &[HostPort],
) -> Vec<HostPort> {
    let ports = listens.filter_map(|addr| {
        let loopback = addr.ip_addr().is_ok_and(|ip| ip.is_loopback());
        let transport = match addr.is_udp() || addr.is_quic() {
            true => Transport::Udp,
            false => Transport::Tcp,
        };
        let port = addr.port().ok()?;
        (!loopback).then_some(HostPort::single(transport, port))
    });
    let mut all: Vec<HostPort> = ports
        .chain(config.extra_ports.iter().copied())
        .chain(app_ports.iter().copied())
        .collect();
    all.sort_unstable();
    all.dedup();
    all
}

async fn run(
    mut receiver: watch::Receiver<Wanted>,
    status: Arc<Mutex<FirewallStatus>>,
    ssh_dir: PathBuf,
) {
    while receiver.changed().await.is_ok() {
        let wanted = receiver.borrow_and_update().clone();
        let (script, enabled, ports) = match &wanted {
            Wanted::Nothing => continue,
            Wanted::Remove => (render_removal(), false, Vec::new()),
            Wanted::Apply(ports) => {
                let rules = FirewallRules::new(ports.iter().copied(), &ssh_ports(&ssh_dir));
                (render(&rules), true, rules.ports.into_iter().collect())
            }
        };
        let result = run_nft(&script).await;
        match &result {
            Ok(()) => info!(enabled, ports = ports.len(), "updated the firewall table"),
            Err(err) => error!(%err, "failed to update the firewall table"),
        }
        let warnings = match enabled {
            true => crate::firewall_conflicts::detect().await,
            false => Vec::new(),
        };
        if let Ok(mut current) = status.lock() {
            *current = next_status(&current, enabled, ports, result);
            current.warnings = warnings;
        }
    }
}

/// A failed update keeps the ports of the table that nft left in place.
fn next_status(
    current: &FirewallStatus,
    enabled: bool,
    ports: Vec<HostPort>,
    result: anyhow::Result<()>,
) -> FirewallStatus {
    let updated_at = SystemTime::now()
        .duration_since(SystemTime::UNIX_EPOCH)
        .ok()
        .map(|d| d.as_millis() as u64);
    match result {
        Ok(()) => FirewallStatus {
            enabled,
            ports,
            warnings: current.warnings.clone(),
            error: None,
            updated_at,
        },
        Err(err) => FirewallStatus {
            error: Some(format!("{err:#}")),
            updated_at,
            ..current.clone()
        },
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn addr(text: &str) -> Multiaddr {
        text.parse().unwrap()
    }

    /// Only addresses that clients outside the host reach need a hole: a loopback listener
    /// stays closed, and UDP and QUIC listeners open UDP ports.
    #[test]
    fn only_outside_listeners_and_extra_ports_are_opened() {
        let listens = [
            addr("/ip4/0.0.0.0/tcp/443/https"),
            addr("/ip6/::/udp/443/quic/http"),
            addr("/ip4/0.0.0.0/udp/3478"),
            addr("/ip4/10.0.0.5/tcp/5349/tls"),
            addr("/ip4/127.0.0.1/tcp/8080/http"),
            addr("/ip6/::1/tcp/9090"),
        ];
        let config = FirewallConfig {
            enabled: true,
            extra_ports: vec!["udp/51820".parse().unwrap(), "tcp/443".parse().unwrap()],
        };
        // A TURN server on the host network needs its relay range open.
        let app = ["udp/49160-49240".parse().unwrap()];
        let ports: Vec<String> = wanted_ports(&config, listens.iter(), &app)
            .iter()
            .map(ToString::to_string)
            .collect();
        assert_eq!(
            ports,
            [
                "tcp/443",
                "tcp/5349",
                "udp/443",
                "udp/3478",
                "udp/49160-49240",
                "udp/51820"
            ]
        );
    }

    #[test]
    fn only_clear_words_turn_the_firewall_on_or_off() {
        for on in ["true", "1", "yes", " TRUE "] {
            assert_eq!(parse_bool(on), Some(true), "{on:?}");
        }
        for off in ["false", "0", "no"] {
            assert_eq!(parse_bool(off), Some(false), "{off:?}");
        }
        for bad in ["", "on", "enabled", "2"] {
            assert_eq!(parse_bool(bad), None, "{bad:?}");
        }
    }

    /// A failed nft run leaves the previous table, so the status must keep its ports and say
    /// why the update failed.
    #[test]
    fn a_failed_update_keeps_the_ports_of_the_previous_table() {
        let open: Vec<HostPort> = vec!["tcp/22".parse().unwrap()];
        let applied = next_status(&FirewallStatus::default(), true, open.clone(), Ok(()));
        assert_eq!(applied.ports, open);
        assert!(applied.error.is_none());
        let failed = next_status(
            &applied,
            true,
            vec!["tcp/80".parse().unwrap()],
            Err(anyhow::anyhow!("nft failed: Operation not permitted")),
        );
        assert_eq!(failed.ports, open);
        assert!(failed.enabled);
        assert!(failed.error.unwrap().contains("Operation not permitted"));
    }
}
