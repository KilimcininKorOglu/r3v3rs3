//! The `inet r3v3rs3_tproxy` table and the policy route that send a port range to one socket
//! and bring the replies of a transparent upstream back to r3v3rs3.
//!
//! A range port listens on its first port with `IP_TRANSPARENT`; the prerouting chain diverts
//! every port of the range to it with `tproxy`, which keeps the original destination for the
//! socket to read. A transparent port's upstream socket sends from the client address with
//! [`UPSTREAM_MARK`]; the connection mark that the output chain sets marks the replies with
//! [`DIVERT_MARK`], and the policy route delivers them to the local socket. The divert mark must
//! differ from the upstream mark, or the upstream packet itself is routed to `lo`.

use crate::firewall::run_nft;
use r3v3rs3_api::firewall::Transport;
use r3v3rs3_api::multiaddr::{Multiaddr, PortSpan};
use std::{fmt::Write as _, net::IpAddr};
use tokio::process::Command;

/// The name of the table in the `inet` family.
pub const TABLE: &str = "r3v3rs3_tproxy";
/// The mark of a diverted packet and of a reply to a transparent upstream socket.
pub const DIVERT_MARK: u32 = 0x7233;
/// The `SO_MARK` of a transparent upstream socket.
pub const UPSTREAM_MARK: u32 = 0x7234;
/// The routing table that delivers marked packets locally.
pub const ROUTE_TABLE: u32 = 7233;

/// A TCP or UDP port that needs the table: a range, a transparent port, or both.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TproxyPort {
    pub ip: IpAddr,
    pub transport: Transport,
    pub span: PortSpan,
    pub transparent: bool,
}

impl TproxyPort {
    /// The entry of a listening address, or `None` when the port needs neither a divert rule nor
    /// a return route.
    pub fn of(listen: &Multiaddr, transparent: bool) -> Option<Self> {
        if listen.is_http() || listen.is_quic() || !(transparent || listen.is_range()) {
            return None;
        }
        Some(Self {
            ip: listen.ip_addr().ok()?,
            transport: match listen.is_udp() {
                true => Transport::Udp,
                false => Transport::Tcp,
            },
            span: listen.port_range().ok()?,
            transparent,
        })
    }

    /// The prerouting rule that diverts the range to its first port.
    fn divert_rule(&self) -> Option<String> {
        if self.span.port_count() < 2 {
            return None;
        }
        let (family, proto) = match self.ip {
            IpAddr::V4(_) => ("ip", "ipv4"),
            IpAddr::V6(_) => ("ip6", "ipv6"),
        };
        let (matcher, target) = match self.ip.is_unspecified() {
            true => (format!("meta nfproto {proto}"), String::new()),
            false => (format!("{family} daddr {}", self.ip), bracket(self.ip)),
        };
        Some(format!(
            "    {matcher} {} dport {} tproxy {family} to {target}:{} meta mark set {DIVERT_MARK:#x} accept\n",
            self.transport.as_str(),
            self.span,
            self.span.first,
        ))
    }
}

fn bracket(ip: IpAddr) -> String {
    match ip {
        IpAddr::V4(ip) => ip.to_string(),
        IpAddr::V6(ip) => format!("[{ip}]"),
    }
}

/// The nft script that replaces the table for `ports`, or removes it when no port needs it.
pub fn render(ports: &[TproxyPort]) -> String {
    let mut script = format!("table inet {TABLE}\ndelete table inet {TABLE}\n");
    if ports.is_empty() {
        return script;
    }
    let transparent = ports.iter().any(|p| p.transparent);
    let _ = write!(
        script,
        "table inet {TABLE} {{\n  chain prerouting {{\n    type filter hook prerouting priority mangle; policy accept;\n"
    );
    if transparent {
        let _ = writeln!(
            script,
            "    ct direction reply ct mark {DIVERT_MARK:#x} meta mark set {DIVERT_MARK:#x} accept"
        );
    }
    ports
        .iter()
        .filter_map(TproxyPort::divert_rule)
        .for_each(|rule| script.push_str(&rule));
    script.push_str("  }\n");
    if transparent {
        let _ = write!(
            script,
            "  chain output {{\n    type route hook output priority mangle; policy accept;\n    meta mark {UPSTREAM_MARK:#x} ct mark set {DIVERT_MARK:#x}\n    ct direction reply ct mark {DIVERT_MARK:#x} meta mark set {DIVERT_MARK:#x}\n  }}\n"
        );
    }
    script.push_str("}\n");
    script
}

/// The address families whose policy route the ports need.
pub fn families(ports: &[TproxyPort]) -> (bool, bool) {
    let v4 = ports.iter().any(|p| p.ip.is_ipv4());
    let v6 = ports.iter().any(|p| p.ip.is_ipv6());
    (v4, v6)
}

/// Runs `ip` with `args` and returns its standard output.
async fn ip(args: &[&str]) -> anyhow::Result<String> {
    let output = Command::new("ip")
        .args(args)
        .kill_on_drop(true)
        .output()
        .await
        .map_err(|err| anyhow::anyhow!("cannot run ip: {err}"))?;
    anyhow::ensure!(
        output.status.success(),
        "ip {} failed: {}",
        args.join(" "),
        String::from_utf8_lossy(&output.stderr).trim()
    );
    Ok(String::from_utf8_lossy(&output.stdout).into_owned())
}

/// The `-4` or `-6` flag of a family.
fn family_flag(v6: bool) -> &'static str {
    match v6 {
        true => "-6",
        false => "-4",
    }
}

/// Adds the rule and the local route of one family; both steps can run again without harm.
async fn add_route(v6: bool) -> anyhow::Result<()> {
    let (mark, table) = (format!("{DIVERT_MARK:#x}"), ROUTE_TABLE.to_string());
    let flag = family_flag(v6);
    let rule = ["rule", "show", "fwmark", &mark, "lookup", &table];
    if ip(&[&[flag][..], &rule].concat()).await?.trim().is_empty() {
        ip(&[flag, "rule", "add", "fwmark", &mark, "lookup", &table]).await?;
    }
    let route = [
        "route", "replace", "local", "default", "dev", "lo", "table", &table,
    ];
    ip(&[&[flag][..], &route].concat()).await.map(drop)
}

/// Removes the rule and the routes of one family, if they exist.
async fn remove_route(v6: bool) -> anyhow::Result<()> {
    let (mark, table) = (format!("{DIVERT_MARK:#x}"), ROUTE_TABLE.to_string());
    let flag = family_flag(v6);
    let rule = ["rule", "show", "fwmark", &mark, "lookup", &table];
    if !ip(&[&[flag][..], &rule].concat()).await?.trim().is_empty() {
        ip(&[flag, "rule", "del", "fwmark", &mark, "lookup", &table]).await?;
    }
    ip(&[flag, "route", "flush", "table", &table])
        .await
        .map(drop)
}

/// Applies the table and the policy routes for `ports`; an empty list removes all of them.
pub async fn apply(ports: &[TproxyPort]) -> anyhow::Result<()> {
    run_nft(&render(ports)).await?;
    let (v4, v6) = families(ports);
    for (v6_family, wanted) in [(false, v4), (true, v6)] {
        match wanted {
            true => add_route(v6_family).await?,
            false => remove_route(v6_family).await?,
        }
    }
    Ok(())
}

/// The handle of the task that keeps the table and the routes. The server loop only hands over
/// the wanted ports, so a slow `nft` or `ip` never holds it up.
pub struct TproxyControl {
    wanted: tokio::sync::watch::Sender<Vec<TproxyPort>>,
    error: std::sync::Arc<std::sync::Mutex<Option<String>>>,
}

impl TproxyControl {
    pub fn spawn() -> Self {
        // The start state is "no port", so a server without ranges or transparent ports never
        // runs nft or ip, and a host without them logs no error.
        let (wanted, mut receiver) = tokio::sync::watch::channel(Vec::<TproxyPort>::new());
        let error = std::sync::Arc::new(std::sync::Mutex::new(None));
        let task_error = error.clone();
        tokio::spawn(async move {
            while receiver.changed().await.is_ok() {
                let ports = receiver.borrow_and_update().clone();
                let result = apply(&ports).await.err().map(|err| format!("{err:#}"));
                match &result {
                    Some(err) => tracing::error!(%err, "failed to update the tproxy table"),
                    None => tracing::info!(ports = ports.len(), "updated the tproxy table"),
                }
                if let Ok(mut current) = task_error.lock() {
                    *current = result;
                }
            }
        });
        Self { wanted, error }
    }

    /// Asks for the table of `ports`; an unchanged list does nothing. An empty list after a
    /// non-empty one removes the table and the routes.
    pub fn update(&self, ports: Vec<TproxyPort>) {
        self.wanted.send_if_modified(|current| {
            let changed = *current != ports;
            *current = ports;
            changed
        });
    }

    /// The error of the last update, if it failed.
    pub fn error(&self) -> Option<String> {
        self.error.lock().ok().and_then(|e| e.clone())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn port(listen: &str, transparent: bool) -> Option<TproxyPort> {
        TproxyPort::of(&listen.parse().unwrap(), transparent)
    }

    /// Only a range or a transparent TCP or UDP port needs the table; HTTP and QUIC never do.
    #[test]
    fn only_ranges_and_transparent_ports_need_the_table() {
        assert!(port("/ip4/0.0.0.0/udp/3478", false).is_none());
        assert!(port("/ip4/0.0.0.0/tcp/443/https", false).is_none());
        assert!(port("/ip4/0.0.0.0/udp/443/quic/http", true).is_none());
        assert!(port("/ip4/0.0.0.0/udp/49160-49240", false).is_some());
        assert!(port("/ip4/0.0.0.0/udp/3478", true).is_some());
    }

    /// The rules that the spike proved: a range goes to its first port with the divert mark,
    /// and a transparent port adds the connection-mark return path. Source: the Linux TPROXY
    /// documentation and the measured spike in the plan.
    #[test]
    fn a_range_is_diverted_to_its_first_port() {
        let ports = [
            port("/ip4/0.0.0.0/udp/49160-49240", false).unwrap(),
            port("/ip6/2001:db8::5/tcp/8000-8010", false).unwrap(),
        ];
        let script = render(&ports);
        for line in [
            "type filter hook prerouting priority mangle; policy accept;",
            "meta nfproto ipv4 udp dport 49160-49240 tproxy ip to :49160 meta mark set 0x7233 accept",
            "ip6 daddr 2001:db8::5 tcp dport 8000-8010 tproxy ip6 to [2001:db8::5]:8000 meta mark set 0x7233 accept",
        ] {
            assert!(script.contains(line), "{line}\n{script}");
        }
        assert!(!script.contains("chain output"), "{script}");
        assert!(!script.contains("ct mark"), "{script}");
    }

    /// A transparent upstream's packets carry the upstream mark, which must differ from the
    /// divert mark: the same mark sends the upstream packet to `lo` (measured in the spike).
    #[test]
    fn a_transparent_port_gets_the_return_path() {
        let script = render(&[port("/ip4/0.0.0.0/udp/3478", true).unwrap()]);
        for line in [
            "ct direction reply ct mark 0x7233 meta mark set 0x7233 accept",
            "type route hook output priority mangle; policy accept;",
            "meta mark 0x7234 ct mark set 0x7233",
            "ct direction reply ct mark 0x7233 meta mark set 0x7233",
        ] {
            assert!(script.contains(line), "{line}\n{script}");
        }
        assert_ne!(DIVERT_MARK, UPSTREAM_MARK);
        // One transparent port is not a range, so nothing is diverted.
        assert!(!script.contains(" tproxy ip"), "{script}");
    }

    /// The script replaces the table in one transaction, and without ports it only deletes it.
    #[test]
    fn the_script_replaces_or_removes_the_table() {
        let empty = render(&[]);
        assert_eq!(
            empty,
            "table inet r3v3rs3_tproxy\ndelete table inet r3v3rs3_tproxy\n"
        );
        let script = render(&[port("/ip4/0.0.0.0/udp/1-2", false).unwrap()]);
        assert!(script.starts_with(&empty));
        assert_eq!(script.matches("chain prerouting").count(), 1);
    }

    #[test]
    fn each_family_gets_its_route_only_when_a_port_uses_it() {
        let v4 = port("/ip4/0.0.0.0/udp/1-2", false).unwrap();
        let v6 = port("/ip6/::/udp/1-2", false).unwrap();
        assert_eq!(families(&[]), (false, false));
        assert_eq!(families(std::slice::from_ref(&v4)), (true, false));
        assert_eq!(families(&[v4, v6]), (true, true));
    }
}
