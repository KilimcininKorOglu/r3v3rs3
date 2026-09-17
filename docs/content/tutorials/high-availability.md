+++
title = "High Availability"
description = "Set up three nodes behind a load balancer, step by step"
weight = 4
aliases = ["high-availability/"]
+++

# High Availability

Cluster mode is the only way to run r3v3rs3 without a single point of failure. Several nodes share one state in etcd or in the key-value store of Consul, and a load balancer sends each request to any of them. There is no other failover mechanism.

This page is a walkthrough. Follow it from the top and you get a working setup. [Cluster](@/cluster.md) is the reference page: it explains every setting, every key in the store and every failure mode.

## What You Build

```
                    clients
                       |
                 10.0.0.10 (VIP)
                       |
          +------------+------------+
          |            |            |
     10.0.0.11    10.0.0.12    10.0.0.13     r3v3rs3 nodes
          |            |            |
          +------------+------------+
                       |
          +------------+------------+
          |            |            |
     10.0.2.11    10.0.2.12    10.0.2.13     etcd or Consul
```

| Failure | Result |
|---|---|
| One r3v3rs3 node stops | keepalived moves the VIP to another node. The other nodes keep the whole state. |
| The leader stops | Another node takes the lead within `lock_ttl`. ACME orders and the background tasks continue there. |
| One store node stops | The store keeps its quorum. Nothing changes for r3v3rs3. |
| The store loses its quorum | Every node becomes `degraded`. Traffic continues with the last state, and changes are rejected. |

Three nodes are the smallest useful size. Two nodes give no quorum in the store, and the store decides the availability of the whole cluster.

## Before You Start

| Host | Address | Role |
|---|---|---|
| `proxy-1` | `10.0.0.11` | r3v3rs3 node |
| `proxy-2` | `10.0.0.12` | r3v3rs3 node |
| `proxy-3` | `10.0.0.13` | r3v3rs3 node |
| `store-1` | `10.0.2.11` | etcd or Consul |
| `store-2` | `10.0.2.12` | etcd or Consul |
| `store-3` | `10.0.2.13` | etcd or Consul |
| VIP | `10.0.0.10` | The address of your DNS records |

Requirements:

- Linux on every host. r3v3rs3 supports Linux only.
- NTP on every host. The nodes compare Unix times for the sessions, the rate limit windows and the cache expiry. See [Clocks](@/cluster.md#clocks).
- The nodes reach the store on port `2379` (etcd) or `8500` (Consul).
- The nodes reach each other with VRRP (protocol 112) when you use keepalived.
- The clients reach the VIP on your proxy ports, for example `80` and `443`.

The store holds your certificates and your passwords in encrypted form. Put it on a private network.

## Step 1: Install the Store

Install either etcd or Consul. Both work the same way for r3v3rs3.

### etcd

Run this unit on each of the three store hosts, with `<NAME>` and `<IP>` of that host:

```ini
[Unit]
Description=etcd
After=network.target

[Service]
ExecStart=/usr/local/bin/etcd \
  --name <NAME> \
  --data-dir /var/lib/etcd \
  --listen-client-urls http://<IP>:2379 \
  --advertise-client-urls http://<IP>:2379 \
  --listen-peer-urls http://<IP>:2380 \
  --initial-advertise-peer-urls http://<IP>:2380 \
  --initial-cluster store-1=http://10.0.2.11:2380,store-2=http://10.0.2.12:2380,store-3=http://10.0.2.13:2380 \
  --initial-cluster-state new
Restart=always

[Install]
WantedBy=multi-user.target
```

Check the store:

```bash
$ etcdctl --endpoints=http://10.0.2.11:2379,http://10.0.2.12:2379,http://10.0.2.13:2379 endpoint health
```

Every endpoint must report `is healthy`.

### Consul

Run this configuration on each of the three store hosts, with `<NAME>` and `<IP>` of that host:

```json
{
  "node_name": "<NAME>",
  "server": true,
  "bootstrap_expect": 3,
  "bind_addr": "<IP>",
  "client_addr": "0.0.0.0",
  "data_dir": "/var/lib/consul",
  "retry_join": ["10.0.2.11", "10.0.2.12", "10.0.2.13"],
  "acl": { "enabled": true, "default_policy": "deny" }
}
```

Bootstrap the ACL system on one host and keep the management token:

```bash
$ consul acl bootstrap
$ consul members
```

`consul members` must list three servers with the state `alive`.

## Step 2: Create the Credentials

The nodes need an account that reads and writes only the keys below their prefix.

### etcd

```bash
$ etcdctl role add r3v3rs3
$ etcdctl role grant-permission --prefix=true r3v3rs3 readwrite r3v3rs3/
$ etcdctl user add r3v3rs3 --new-user-password=<password>
$ etcdctl user grant-role r3v3rs3 r3v3rs3
```

Without `--new-user-password` the command asks for the password on the terminal. Enable the authentication of etcd after you create a `root` user, because etcd checks no permission while the authentication is off:

```bash
$ etcdctl user add root --new-user-password=<root password>
$ etcdctl user grant-role root root
$ etcdctl auth enable
```

Check that the new user cannot write outside the prefix:

```bash
$ etcdctl --user r3v3rs3:<password> put r3v3rs3/probe ok
OK
$ etcdctl --user r3v3rs3:<password> put other/probe ok
Error: etcdserver: permission denied
$ etcdctl --user r3v3rs3:<password> del r3v3rs3/probe
```

### Consul

Write the policy to `r3v3rs3.hcl`:

```hcl
key_prefix "r3v3rs3/" {
  policy = "write"
}

session_prefix "" {
  policy = "write"
}
```

```bash
$ consul acl policy create -name r3v3rs3 -rules @r3v3rs3.hcl
$ consul acl token create -description "r3v3rs3 nodes" -policy-name r3v3rs3
```

Keep the `SecretID` of the new token. It is the `token` of the nodes.

## Step 3: Install r3v3rs3 on Every Node

```bash
$ curl -fsSL https://raw.githubusercontent.com/KilimcininKorOglu/r3v3rs3/main/install.sh | sudo bash
```

The script installs the binary, creates `/etc/r3v3rs3`, asks for an admin user name and a password, and runs r3v3rs3 as a systemd service. That account goes to `/etc/r3v3rs3/accounts.toml`. A node with the cluster on does not read that file, so Step 6 creates the account of the cluster again.

Stop the service on every node until the cluster configuration is ready:

```bash
$ sudo systemctl stop r3v3rs3
```

With Docker, use the `docker-compose.yml` of the repository on each node and add `/etc/r3v3rs3` as the config volume. Keep `network_mode: host`, because the proxy ports bind on the host.

## Step 4: Create the Encryption Key

The nodes encrypt every value in the store. Create one key and copy it to every node:

```bash
$ sudo r3v3rs3 cluster keygen /etc/r3v3rs3/cluster.key
Wrote the encryption key 855523573514e205 to /etc/r3v3rs3/cluster.key.
Keep a copy of the key file. The cluster data cannot be decrypted when every key file is lost.
```

The file gets mode `0600`. The command does not replace an existing file.

Copy the same file to `proxy-2` and `proxy-3`, and keep a copy off the nodes. When every copy is lost, nobody can read the certificates, the passwords and the proxies in the store again.

## Step 5: Configure Every Node

Add the `[cluster]` section to `/etc/r3v3rs3/config.toml`. Only `node_name` differs between the nodes.

etcd:

```toml
[cluster]
enabled = true
backend = "etcd"
endpoints = ["http://10.0.2.11:2379", "http://10.0.2.12:2379", "http://10.0.2.13:2379"]
username = "r3v3rs3"
password = "<password>"
node_name = "proxy-1"
encryption_key_files = ["/etc/r3v3rs3/cluster.key"]
```

Consul:

```toml
[cluster]
enabled = true
backend = "consul"
endpoints = ["http://10.0.2.11:8500", "http://10.0.2.12:8500", "http://10.0.2.13:8500"]
token = "<SecretID>"
node_name = "proxy-1"
encryption_key_files = ["/etc/r3v3rs3/cluster.key"]
```

Use `https://` endpoints and the `tls` field in production. [Settings](@/cluster.md#settings) lists every field with its default.

## Step 6: Import the State and Add the Admin Account

Run the import once, on one node only:

```bash
$ sudo r3v3rs3 cluster import --config-dir /etc/r3v3rs3
Imported the config, 0 ports, 0 proxies, 0 access lists, 0 certificates, 0 ACME entries and 0 accounts.
```

The prefix of the store must be empty. The import writes the `schema` key last, so a stopped import leaves no usable prefix: remove the keys below the prefix and run the import again.

Add the admin account after the import. With the cluster on, the account goes to the store and every node sees it:

```bash
$ sudo r3v3rs3 add-user admin --config-dir /etc/r3v3rs3
```

The command asks for the password. It replaces the account when the name already exists in the store, so you can also use it to set a new password.

## Step 7: Start Every Node

```bash
$ sudo systemctl start r3v3rs3
```

Open the WebUI of each node and sign in. The **Settings** page shows the state, the role and the revision of the node. The same data comes from the admin API:

```bash
$ curl -b session.txt http://10.0.0.11:46492/api/cluster/status
{"state":"synced","node_name":"proxy-1","leader":false,"revision":247}
```

Every node must report `synced`, and exactly one node must report `"leader":true`. A node that reports `syncing` for a long time cannot read the store: check the credentials and the endpoints in its log.

## Step 8: Add a Health Check Route

The load balancer needs an address that answers without a login. The admin API is behind the session cookie, so it cannot serve this. Add a route that answers with a fixed status instead.

On the **Ports** page, add an HTTP port, for example `/ip4/0.0.0.0/tcp/80/http`. On the **Proxies** page, add an HTTP proxy on that port with one route:

- **Path**: `/healthz`
- **Response**: `Status`, status `200`, body `ok`

The change reaches every node through the store, so one WebUI is enough. Check each node by its own address:

```bash
$ curl -i http://10.0.0.11/healthz
HTTP/1.1 200 OK
...
ok
```

The route answers as long as the node serves traffic, also when the node is `degraded`. That is what a load balancer needs: it removes a node that stopped, not a node that lost the store.

## Step 9: Put a Load Balancer in Front

### keepalived

keepalived moves the VIP `10.0.0.10` to a node that answers. Install it on all three nodes.

`/etc/keepalived/keepalived.conf` on `proxy-1`:

```
vrrp_script check_r3v3rs3 {
    script "/usr/bin/curl -sf -o /dev/null http://127.0.0.1/healthz"
    interval 2
    timeout 2
    rise 2
    fall 2
}

vrrp_instance r3v3rs3 {
    state MASTER
    interface eth0
    virtual_router_id 51
    priority 150
    advert_int 1
    authentication {
        auth_type PASS
        auth_pass <shared secret>
    }
    virtual_ipaddress {
        10.0.0.10/24
    }
    track_script {
        check_r3v3rs3
    }
}
```

On `proxy-2` and `proxy-3`, change `state` to `BACKUP` and `priority` to `140` and `130`. Keep `virtual_router_id` and `auth_pass` the same on all three.

Check the configuration before you start the service:

```bash
$ keepalived -t -f /etc/keepalived/keepalived.conf
```

Point your DNS records at `10.0.0.10`.

One node serves every request with this setup. The other two hold the same state and take over after two failed checks, so after `interval` times `fall` seconds.

### DNS round robin

Give your record the addresses of all three nodes:

```
proxy.example.com. 60 IN A 10.0.0.11
proxy.example.com. 60 IN A 10.0.0.12
proxy.example.com. 60 IN A 10.0.0.13
```

This spreads the load over the three nodes without a VIP. It has one limit: DNS does not remove a node that stopped. A client keeps the address until the TTL ends, and some clients cache it longer. Use it when your clients retry another address, and use keepalived when they do not.

## Verify the Cluster

1. Add a proxy on one node. It appears on the other nodes within a second.
2. Stop the leader: `sudo systemctl stop r3v3rs3`. Another node reports `"leader":true` within `lock_ttl`, 15 seconds by default.
3. Check the VIP: `ip addr show eth0` on the other nodes. One of them now holds `10.0.0.10`.
4. Start the stopped node again. It reads the whole state from the store and reports `synced`.
5. Sign in on one node and open the WebUI of another node with the same browser. The session is valid there too.

## Operate the Cluster

**Add a node.** Install r3v3rs3, copy `cluster.key`, write the same `[cluster]` section with a new `node_name` and start it. Do not run the import again.

**Remove a node.** Stop the service. Its `nodes/` key ends with its lease, and the other nodes stop counting it. Remove its keepalived configuration too.

**Upgrade.** Upgrade the followers first, one at a time, and the leader last. Each node reads the state again when it starts.

**Rotate the encryption key.** Follow [Encryption](@/cluster.md#encryption). The order matters: every node reads its key files when it starts.

## What Does Not Fail Over

- A node that loses the store becomes `degraded`. It serves traffic with the last state, and the admin API rejects changes with `503 cluster_unavailable`.
- A new login and a new proxy session need the store. Without the store, they fail with `503`.
- The rate limit of a short period is divided between the nodes, so one client that reaches one node gets less than the whole limit. See [Rate Limit Accuracy](@/cluster.md#rate-limit-accuracy).
- The response cache is local unless you set `share_cache = true`. See [Shared Cache](@/cluster.md#shared-cache).

[Failure Modes](@/cluster.md#failure-modes) describes each case in detail.
