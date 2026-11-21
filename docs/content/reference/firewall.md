+++
title = "Host Firewall"
description = "The nftables table that r3v3rs3 keeps on the host"
weight = 8
+++

## Host firewall

With `[firewall] enabled = true` (or `R3V3RS3_FIREWALL_ENABLED=true`), r3v3rs3 keeps one nftables table, `inet r3v3rs3`, on the server. It is off by default. The table covers IPv4 and IPv6 and drops every incoming connection except:

- replies to connections the host opened, the loopback interface, and ICMP and ICMPv6;
- the SSH ports of `/etc/ssh/sshd_config` and `sshd_config.d/*.conf`, and port 22 always;
- every port that listens on an address other than `127.0.0.1` or `::1` (TCP, or UDP for UDP and QUIC ports);
- the `host_ports` of the running apps on this server (see the platform page);
- `extra_ports`, such as `["udp/51820", "tcp/8000-8010"]`.

```toml
[firewall]
enabled = true
extra_ports = ["udp/51820"]
```

The settings page, the MCP tool `update_firewall` and `r3 security firewall update --enabled true --extra-ports udp/51820` change the same two settings.

r3v3rs3 replaces the table in one `nft -f` transaction each time a port, an app or the setting changes, so a half-written table never exists. Turning the setting off removes the table. The table filters only the `input` hook; the Docker NAT and forward rules stay as they are. Agent targets keep the same table on their host with the `host_ports` of their apps.

- The server needs `CAP_NET_ADMIN`; the unit that `install.sh` writes gives it.
- r3v3rs3 must be the only firewall that drops traffic. nftables runs the input chain of every table, so a drop in a firewalld, ufw or `iptables-persistent` table still blocks a port this table opens. Stop the other tool first (`systemctl disable --now firewalld`, `ufw disable`, or `systemctl disable --now netfilter-persistent`). The **Host Firewall** card of the settings page, `GET /api/firewall`, the MCP tool `get_firewall_status` and `r3 security firewall get-status` list each such table as a warning, and the port list shows **Firewall open, another firewall found**. While the table is on, r3v3rs3 reads these warnings again every minute, so a warning goes away within a minute after you stop the other tool. ufw counts only when `/etc/ufw/ufw.conf` has `ENABLED=yes`.
- A container that publishes a port with Docker (`-p 8080:80`) is reached through the forward hook, so this table does not block it.

Before you stop the previous firewall, read its rules outside the input chain (`iptables-save` and `nft list ruleset`): `OUTPUT`, `FORWARD` and the `nat` and `raw` tables. A cloud image or a VPN can depend on them, for example an output rule for the metadata service of the provider or a NAT rule of a VPN. Stopping the tool removes them too, and r3v3rs3 does not write them again. Keep them in one of two ways:

- Keep the tool, but remove its `INPUT` rules from its saved rules (for example `/etc/iptables/rules.v4` of `iptables-persistent`), so only r3v3rs3 filters incoming traffic.
- Move them into a separate nftables table of your own. Do not move a `FORWARD` reject rule: in a table of its own it blocks the containers that Docker publishes.

When the host loads its rules with `nftables.service`, the stock unit stops with `nft flush ruleset`, which also deletes the r3v3rs3 table and the Docker tables. Change its `ExecStop` so it deletes only your own table, for example `nft delete table inet custom`.

Rollback: set `enabled = false`, or run `nft delete table inet r3v3rs3` on the host, then start the previous firewall again. Save its rules first (`nft list ruleset > /root/ruleset.nft`) and restore them with `nft -f /root/ruleset.nft`.
