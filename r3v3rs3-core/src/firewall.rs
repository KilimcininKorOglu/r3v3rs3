//! The `inet r3v3rs3` nftables table: the input chain drops every packet except replies,
//! loopback, ICMP and the ports of its two sets. r3v3rs3 replaces the whole table in one
//! `nft -f -` transaction, so a failed update keeps the previous table. Only the input hook is
//! filtered: Docker's NAT and FORWARD rules, and the ports that Docker publishes, stay untouched.

use r3v3rs3_api::firewall::{HostPort, Transport};
use std::{collections::BTreeSet, fmt::Write as _, path::Path, process::Stdio};
use tokio::{io::AsyncWriteExt, process::Command};

/// The name of the table in the `inet` family.
pub const TABLE: &str = "r3v3rs3";

/// The port that OpenSSH listens on without a `Port` line.
const SSH_DEFAULT_PORT: u16 = 22;

/// The ports that the table opens.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct FirewallRules {
    pub ports: BTreeSet<HostPort>,
}

impl FirewallRules {
    /// The rules with `ports` and every SSH port, which the table never closes.
    pub fn new(ports: impl IntoIterator<Item = HostPort>, ssh: &[u16]) -> Self {
        let ssh = ssh
            .iter()
            .chain([&SSH_DEFAULT_PORT])
            .map(|port| HostPort::single(Transport::Tcp, *port));
        Self {
            ports: ports.into_iter().chain(ssh).collect(),
        }
    }

    fn elements(&self, transport: Transport) -> String {
        merge(self.ports.iter().filter(|p| p.transport == transport))
            .iter()
            .map(|(first, last)| match first == last {
                true => first.to_string(),
                false => format!("{first}-{last}"),
            })
            .collect::<Vec<_>>()
            .join(", ")
    }
}

/// Joins overlapping and adjacent ranges, because an interval set refuses overlapping elements.
fn merge<'a>(ports: impl Iterator<Item = &'a HostPort>) -> Vec<(u16, u16)> {
    let mut ranges: Vec<(u16, u16)> = ports.map(|p| (p.first, p.last)).collect();
    ranges.sort_unstable();
    let mut merged: Vec<(u16, u16)> = Vec::with_capacity(ranges.len());
    for (first, last) in ranges {
        match merged.last_mut() {
            Some(prev) if u32::from(first) <= u32::from(prev.1) + 1 => prev.1 = prev.1.max(last),
            _ => merged.push((first, last)),
        }
    }
    merged
}

/// The nft script that replaces the table with `rules`. The first two lines create the table if
/// it is missing and then delete it, so the script works on a host with or without the table.
pub fn render(rules: &FirewallRules) -> String {
    let mut script = format!("table inet {TABLE}\ndelete table inet {TABLE}\n");
    let _ = writeln!(script, "table inet {TABLE} {{");
    for (name, transport) in [("tcp_ports", Transport::Tcp), ("udp_ports", Transport::Udp)] {
        let _ = write!(
            script,
            "  set {name} {{\n    type inet_service; flags interval;\n"
        );
        let elements = rules.elements(transport);
        if !elements.is_empty() {
            let _ = writeln!(script, "    elements = {{ {elements} }}");
        }
        script.push_str("  }\n");
    }
    script.push_str(concat!(
        "  chain input {\n",
        "    type filter hook input priority 0; policy drop;\n",
        "    ct state established,related accept\n",
        "    iif lo accept\n",
        "    meta l4proto { icmp, ipv6-icmp } accept\n",
        "    tcp dport @tcp_ports accept\n",
        "    udp dport @udp_ports accept\n",
        "  }\n",
        "}\n",
    ));
    script
}

/// The script that removes the table, for a firewall that was turned off.
pub fn render_removal() -> String {
    format!("table inet {TABLE}\ndelete table inet {TABLE}\n")
}

/// Runs `script` with `nft -f -`.
pub async fn run_nft(script: &str) -> anyhow::Result<()> {
    let mut child = Command::new("nft")
        .args(["-f", "-"])
        .stdin(Stdio::piped())
        .stdout(Stdio::null())
        .stderr(Stdio::piped())
        .kill_on_drop(true)
        .spawn()
        .map_err(|err| anyhow::anyhow!("cannot run nft: {err}"))?;
    let mut stdin = child
        .stdin
        .take()
        .ok_or_else(|| anyhow::anyhow!("nft has no stdin"))?;
    stdin.write_all(script.as_bytes()).await?;
    drop(stdin);
    let output = child.wait_with_output().await?;
    anyhow::ensure!(
        output.status.success(),
        "nft failed: {}",
        String::from_utf8_lossy(&output.stderr).trim()
    );
    Ok(())
}

/// The SSH ports of `sshd_config` and the files of its `sshd_config.d` directory.
pub fn ssh_ports(dir: &Path) -> Vec<u16> {
    let mut texts = vec![std::fs::read_to_string(dir.join("sshd_config")).unwrap_or_default()];
    if let Ok(entries) = std::fs::read_dir(dir.join("sshd_config.d")) {
        texts.extend(
            entries
                .flatten()
                .filter(|e| e.path().extension().is_some_and(|ext| ext == "conf"))
                .filter_map(|e| std::fs::read_to_string(e.path()).ok()),
        );
    }
    texts
        .iter()
        .flat_map(|text| parse_ssh_ports(text))
        .collect()
}

/// The `Port` lines of an sshd configuration. The keyword is case-insensitive and a `#` starts
/// a comment, as in sshd_config(5).
pub fn parse_ssh_ports(text: &str) -> Vec<u16> {
    text.lines()
        .filter_map(|line| {
            let line = line.split('#').next().unwrap_or_default();
            let mut words = line.split_whitespace();
            let keyword = words.next()?;
            let value = words.next()?;
            keyword
                .eq_ignore_ascii_case("port")
                .then(|| value.parse::<u16>().ok())
                .flatten()
        })
        .filter(|port| *port != 0)
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn port(text: &str) -> HostPort {
        text.parse().unwrap()
    }

    /// The table must keep a host reachable: replies, loopback and ICMP (path MTU discovery
    /// needs ICMP type 3 code 4) pass, every SSH port stays open, and the default is drop.
    #[test]
    fn the_table_keeps_replies_loopback_icmp_and_ssh_open() {
        let script = render(&FirewallRules::new([], &[2222]));
        for line in [
            "type filter hook input priority 0; policy drop;",
            "ct state established,related accept",
            "iif lo accept",
            "meta l4proto { icmp, ipv6-icmp } accept",
            "tcp dport @tcp_ports accept",
            "udp dport @udp_ports accept",
            "elements = { 22, 2222 }",
        ] {
            assert!(script.contains(line), "{line}\n{script}");
        }
        // Docker forwards published ports through the forward hook, which the table must not
        // filter.
        assert!(!script.contains("hook forward"), "{script}");
    }

    #[test]
    fn each_protocol_fills_its_own_set_with_merged_ranges() {
        let rules = FirewallRules::new(
            [
                port("tcp/443"),
                port("udp/49160-49240"),
                port("udp/49200-49300"),
                port("udp/3478"),
                port("udp/3479"),
                port("tcp/80"),
            ],
            &[],
        );
        let script = render(&rules);
        let udp = script.split("set udp_ports").nth(1).unwrap();
        assert!(
            udp.contains("elements = { 3478-3479, 49160-49300 }"),
            "{script}"
        );
        let tcp = script.split("set udp_ports").next().unwrap();
        assert!(tcp.contains("elements = { 22, 80, 443 }"), "{script}");
    }

    /// A rerun must replace the table, not add a second chain, so the script deletes the old
    /// table in the same transaction.
    #[test]
    fn the_script_replaces_the_table_in_one_transaction() {
        let script = render(&FirewallRules::default());
        assert!(script.starts_with("table inet r3v3rs3\ndelete table inet r3v3rs3\n"));
        assert_eq!(script.matches("chain input").count(), 1);
    }

    #[test]
    fn ssh_ports_come_from_port_lines_only() {
        let text = "# Port 2200\nPort 2222\n  port 2333 # second\nListenAddress 0.0.0.0:22\nPort 0\nPort x\nPortX 9\n";
        assert_eq!(parse_ssh_ports(text), vec![2222, 2333]);
    }
}
