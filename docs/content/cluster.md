+++
title = "Cluster"
description = "High availability with etcd or Consul"
weight = 0
+++

# Cluster

Several r3v3rs3 nodes can share one state in etcd or in the key-value store of Consul. Each node serves traffic with the same ports, proxies, certificates, ACME entries, admin accounts and settings. A change on one node reaches the other nodes without a restart.

## Architecture

- `config.toml` of each node holds the `[cluster]` section. The admin API and the WebUI do not change it, and the store does not hold it.
- The store holds the rest of the state: the settings, the ports, the proxies, the certificates, the ACME entries, the admin accounts and the CDN IP ranges. A node with the cluster on does not read `ports.toml`, `proxies.toml`, `acme.toml`, `accounts.toml` or the certificate files.
- Each node watches the store and applies every change. A change through the admin API of a node goes to the store first. A write that finds a newer value fails with `409 cluster_write_conflict`, and the node gets the newer value from the store.
- The nodes share the admin sessions, the proxy sessions, the rate limit counts and, when `share_cache` is on, the cached responses.
- One node is the leader. Only the leader orders ACME certificates, removes expired certificates, saves the downloaded CDN IP ranges to the store and removes the expired sessions and shared responses. The leader runs these tasks when it takes the lead and then at each `background_task_interval`.
- A load balancer in front of the nodes can send each request to any node.

The **Settings** page of the WebUI shows the state, the role and the applied revision of the node. `GET /api/cluster/status` returns the same data.

| State | Meaning |
|---|---|
| `disabled` | The node does not use a cluster store. |
| `syncing` | The node reads the store for the first time. |
| `synced` | The node applies the changes of the store. |
| `degraded` | The node lost the store. It serves the last applied state and rejects changes. |

## Set Up a Cluster

1. Create an encryption key file. Copy the same file to every node.

   ```bash
   $ r3v3rs3 cluster keygen /etc/r3v3rs3/cluster.key
   ```

   The command writes a new file with mode `0600` and does not replace an existing file. Keep a copy of the key file in a safe place. When every copy of the key is lost, nobody can decrypt the data in the store.

2. Add the `[cluster]` section to `config.toml` on every node. Only `node_name` differs between the nodes.

   ```toml
   [cluster]
   enabled = true
   backend = "etcd"
   endpoints = ["https://10.0.0.1:2379", "https://10.0.0.2:2379", "https://10.0.0.3:2379"]
   username = "r3v3rs3"
   password = "<etcd password>"
   node_name = "proxy-1"
   encryption_key_files = ["/etc/r3v3rs3/cluster.key"]
   tls = { ca_file = "/etc/r3v3rs3/etcd-ca.pem" }
   ```

3. Copy the files of one node into the store. The prefix of the store must be empty.

   ```bash
   $ r3v3rs3 cluster import --config-dir /etc/r3v3rs3
   ```

   The import writes the `schema` key last. When an import stops before the end, remove the keys below the prefix and run the import again.

4. Start every node with `r3v3rs3 start`. A node without the `schema` key in the store does not start and asks for `r3v3rs3 cluster import`. `r3v3rs3 add-user` writes the account to the store after the import.

## Settings

Only `config.toml` sets these fields.

| Field | Default | Description |
|---|---|---|
| `enabled` | `false` | Turns the cluster on. |
| `backend` | `etcd` | `etcd` or `consul`. |
| `endpoints` | none | The HTTP API addresses of the store: `http://<host>:<port>`, `https://<host>:<port>` or `unix://<path>`. A connection error selects the next address. |
| `username`, `password` | empty | The etcd user. An empty user sends no credentials. |
| `token` | none | The Consul ACL token. |
| `datacenter` | empty | The Consul datacenter. Empty uses the datacenter of the agent. |
| `prefix` | `r3v3rs3` | Every key of the cluster starts with `<prefix>/v1/`. Several clusters can share one store with different prefixes. |
| `node_name` | none | The name of the node. It is required, and each node needs a different name. |
| `tls` | none | `ca_file` verifies the store. Without it, the system root certificates verify the store. `cert_file` and `key_file` send a client certificate. |
| `encryption_key_files` | none | The key files. The first key encrypts. Every key decrypts. |
| `lock_ttl` | `15s` | The TTL of the leader lock and of the node presence. etcd needs at least `1s`, Consul at least `10s`. |
| `startup_timeout` | `30s` | The longest wait for the store when the node starts. |
| `rate_limit_sync_interval` | `1s` | How often a node publishes its rate limit counts. The minimum is `100ms`. |
| `share_cache` | `false` | Stores cached responses in the store for the other nodes. |
| `cache_max_value_size` | `1048576` | The largest cached response in bytes that a node stores for the other nodes. |

The admin API does not return `password` or `token`.

## Keys in the Store

Every key starts with `<prefix>/v1/`.

| Key | Content | Encrypted |
|---|---|---|
| `schema` | The version of the data layout. The import writes it last. | no |
| `state/config` | The settings. | yes |
| `state/ports/<id>` | The ports. | no |
| `state/proxies/<id>` | The proxies. | yes |
| `state/certs/<kind>/<id>` | The certificates and their private keys. | yes |
| `state/acme/<id>` | The ACME entries with the account keys and the DNS provider credentials. | yes |
| `state/accounts/<hex name>` | The admin accounts. | yes |
| `state/cdn` | The CDN IP ranges. | no |
| `state/challenges/http/<hex token>`, `state/challenges/tls-alpn/<hex domain>` | The ACME challenges that every node serves. The ACME server publishes their values. | no |
| `state/cache-purges/<proxy id>` | The time of the last cache purge of a proxy. | no |
| `lock/leader` | The leader lock, attached to the lease of the leader. | yes |
| `nodes/<hex name>` | The presence of a node, attached to its lease. | no |
| `acks/<hex name>` | The digest of the challenges that a node serves. | no |
| `sessions/<scope>/<SHA-256 of the token>` | The admin and proxy sessions. The store never holds a token. | yes |
| `ratelimit/<hex name>` | The rate limit counts of a node, with client IP addresses. | yes |
| `cache/<proxy id>/<SHA-256 of the cache key>` | A shared cached response. | yes |

## Encryption

A node encrypts each value with AES-256-GCM before it writes the value. The KV key of the value is the associated data, so a value that somebody moves to another key does not decrypt. An encrypted value starts with the id of its key: the first 8 bytes of the SHA-256 digest of the key.

To replace a key:

1. Create a new key file with `r3v3rs3 cluster keygen`.
2. Put the new file first in `encryption_key_files` and keep the old file in the list. Do this on every node and restart the nodes, because a node reads the key files when it starts.
3. Run `r3v3rs3 cluster rekey --config-dir <dir>` on one node. The command encrypts every value again with the first key and reports the values that it changed.
4. Remove the old file from `encryption_key_files` on every node and restart the nodes.

Protect the store too. The encryption hides the values, but the key names show the ids of the ports, proxies and certificates.

## etcd Permissions

The user of the nodes needs read and write access to the keys below the prefix:

```bash
$ etcdctl role add r3v3rs3
$ etcdctl role grant-permission --prefix=true r3v3rs3 readwrite r3v3rs3/
$ etcdctl user add r3v3rs3
$ etcdctl user grant-role r3v3rs3 r3v3rs3
```

With another `prefix`, grant the permission on `<prefix>/`. The nodes renew the authentication token when etcd revokes it.

## Consul Permissions

The token of the nodes needs write access to the keys below the prefix and the permission to create sessions:

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

The `make test-cluster-e2e` target runs the nodes with exactly these permissions.

## Leader and ACME

The leader holds the `lock/leader` key with a lease of `lock_ttl`. It renews the lease three times in each `lock_ttl`. A store call of the leader waits one third of `lock_ttl` at most, so the leader gives up the lead before its lease can end in the store. When the leader stops, it releases the lock, and another node takes the lead at its next check.

For an HTTP-01 or TLS-ALPN-01 challenge, the leader writes the challenge to the store. Every node serves it. Each node writes the digest of the challenges that it serves to `acks/`. The leader waits up to 10 seconds until every present node reports the new challenges, and then asks the ACME server to validate.

## Failure Modes

- A node checks the store three times in each `lock_ttl`. When the node has no successful read for `lock_ttl`, its state becomes `degraded`.
- A degraded node serves traffic with the last applied state. The admin API rejects changes with `503 cluster_unavailable`, and every page of the WebUI shows a warning.
- The leader gives up the lead when its store calls fail, so a degraded node does not lead.
- A new login and a new proxy session need the store. Without the store, they fail with `503`.
- When the store is back, the node reads the whole state again and becomes `synced`. The node does not write its local state to the store.
- A node that cannot reach the store when it starts stops after `startup_timeout`.

## Sessions

The admin sessions and the sessions of the proxy authentication are in the store, so a session that one node starts is valid on every node. A logout removes the session for every node. A node reads the store for each request that carries a session. Without the store, a proxy session is not valid, and the admin API answers `503 cluster_unavailable`.

## Rate Limit Accuracy

A node does not read the store for each request. Each node counts the requests of each client and publishes the counts of the 2048 busiest clients of each limit at each `rate_limit_sync_interval`. A node adds the recent counts of the other nodes to its own counts. Counts that are older than three intervals, or older than 2 seconds when that is longer, do not count.

- A limit whose period is at least 10 times `rate_limit_sync_interval` uses the counts of every node. The counts of the other nodes are up to one interval old. A burst that reaches every node at the same time can pass up to `(N − 1) × burst` more requests than the limit, where `N` is the number of nodes.
- A limit with a shorter period divides the limit between the present nodes. Each node allows `ceil(limit / N)` requests. A load balancer that sends a client to one node gives the client less than the limit.
- Without the store, each node uses the divided limit with the last known number of nodes.

## Shared Cache

With `share_cache = true`, a node queues each cached response that stays fresh for at least 60 seconds for the store. The node does not wait for the write. On a local miss, a node reads the store for at most 100 milliseconds.

- A response stays on its node when its encoded size is larger than `cache_max_value_size`, 1 MiB for etcd or 350 KiB for Consul, whichever is smaller. The body is stored in base64, so the stored size is about 4/3 of the body size. The node does not split a response into several keys.
- A purge of the cache of a proxy writes `state/cache-purges/<proxy id>` and deletes the shared responses of the proxy. Every node purges its local cache after the change.
- A shared response stays in the store for one hour after it becomes stale, so a node can revalidate it. The leader removes the expired responses at each `background_task_interval`. The removal reads and decrypts every shared response, so its cost grows with the number of stored responses.
- Each stored response is a write to the store. The store keeps every write in its history until a compaction, so a busy cache increases the size of etcd between compactions.

## Clocks

The nodes compare Unix times for the session expiry, the rate limit windows, the freshness of the rate limit counts and the expiry of shared responses. Keep the clocks of the nodes in sync with NTP. A clock that is ahead or behind by seconds changes the rate limit decisions and the cache expiry.
