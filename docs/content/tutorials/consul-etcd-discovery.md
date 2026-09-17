+++
title = "Proxies from Consul and etcd"
description = "Build proxies from the Consul catalog, the Consul key-value store and etcd keys"
weight = 13
+++

# Proxies from Consul and etcd

A service that registers itself in Consul, or a deploy job that writes etcd keys, can carry its own proxy definition. r3v3rs3 reads it and builds the proxy without a restart and without a click in the WebUI.

This guide uses all three sources:

- The Consul catalog, where the tags of a service instance define the proxy.
- The Consul key-value store, for a proxy that no service registers.
- The etcd keys, with the same layout.

You need a port from [Getting Started](@/tutorials/getting-started.md). This guide uses an HTTP port named `web`. A provider never opens a port; it only names a port that already exists.

## Step 1: Turn On the Provider

Open **Settings** and fill in **Consul Service Discovery**, or edit `config.toml`:

```toml
[discovery.consul]
enabled = true
address = "http://127.0.0.1:8500"
token = "<ACL token>"
catalog = true
kv = true
prefix = "r3v3rs3"
exposed_by_default = false
```

`exposed_by_default = false` reads only the services that carry the tag `r3v3rs3.enable=true`. With `true` every service that has `r3v3rs3.` tags is read.

The token needs three permissions:

```hcl
service_prefix "" { policy = "read" }
node_prefix "" { policy = "read" }
key_prefix "r3v3rs3/" { policy = "read" }
```

The catalog needs `service:read` and `node:read`, and the key-value store needs `key:read` on the prefix. Give a read-only token; r3v3rs3 never writes to Consul.

The provider state appears in the proxy list and at `GET /api/discovery`:

```bash
$ curl -s -b cookies.txt http://localhost:46492/api/discovery
[{"provider":"consul","state":"running","proxies":0,"updated_at":1789647725}]
```

`connecting` means the first read is not finished. `running` means the provider watches for changes. `error` means it cannot read; the proxies of the last read stay active.

A change of the settings restarts the provider. r3v3rs3 itself does not restart.

## Step 2: Define a Proxy in the Catalog

Register a service with `r3v3rs3.` tags:

```json
{
  "Name": "whoami",
  "ID": "whoami-1",
  "Address": "10.0.0.5",
  "Port": 8080,
  "Tags": [
    "r3v3rs3.enable=true",
    "r3v3rs3.http.whoami.ports=web",
    "r3v3rs3.http.whoami.vhosts=whoami.example.com"
  ],
  "Check": { "HTTP": "http://10.0.0.5:8080/", "Interval": "10s" }
}
```

```bash
$ consul services register whoami.json
```

Within about one second the proxy list holds a new proxy:

```bash
$ curl -s -b cookies.txt http://localhost:46492/api/proxies | jq -c '.[] | {id, name, source}'
{"id":"cfr-kpm","name":"whoami","source":{"provider":"consul","resource":"whoami"}}
```

The proxy has no `routes` tag, so it uses the address and the port of the service instance as its single upstream server. The tag keys are the fields of the proxy model of the admin API; see [Labels](@/discovery.md#labels) for the complete list.

A `r3v3rs3.` tag without `=` is an issue. r3v3rs3 reads only the instances that pass their health checks, so a service whose check fails loses its proxy.

## Step 3: Watch the Instances Balance the Load

Register a second instance of the same service on another address:

```bash
$ consul services register whoami-2.json
```

The instances share the proxy. Each one adds its server to the routes:

```bash
$ curl -s -b cookies.txt http://localhost:46492/api/proxies/cfr-kpm/status
{"state":"active","upstreams":[
  {"addr":"http://10.0.0.5:8080/","weight":1,"healthy":true,"failures":0},
  {"addr":"http://10.0.0.6:8080/","weight":1,"healthy":true,"failures":0}]}
```

Round robin then answers `10.0.0.5, 10.0.0.6, 10.0.0.5, 10.0.0.6`. Deregister one instance and its server leaves the list within about one second, with no configuration change on your side.

That is the point of the catalog: scale the service and the proxy follows.

## Step 4: A Proxy Without a Service

A service that nobody registers, for example a machine outside Consul, needs the key-value store. A key under the prefix is a label, with `/` instead of `.`:

```bash
$ consul kv put r3v3rs3/http/kvapp/ports web
$ consul kv put r3v3rs3/http/kvapp/vhosts kv.example.com
$ consul kv put r3v3rs3/http/kvapp/routes/0/servers/0/url http://10.0.0.9:8080/
```

r3v3rs3 reads every proxy under the prefix, so a key set does not need `r3v3rs3.enable`. A key has no address, so `port` and `scheme` are not available; write the server URL.

A part of a key cannot contain a `.`. Such a key is an issue.

## Step 5: The Same Keys in etcd

etcd uses the same layout. Fill in **etcd Service Discovery** in **Settings**, or edit `config.toml`:

```toml
[discovery.etcd]
enabled = true
endpoints = ["http://10.0.0.1:2379", "http://10.0.0.2:2379"]
username = "r3v3rs3"
password = "<password>"
prefix = "r3v3rs3"
```

r3v3rs3 connects to the next address when a connection fails. With etcd authentication, create a read-only user for the prefix:

```bash
$ etcdctl role add r3v3rs3-reader
$ etcdctl role grant-permission r3v3rs3-reader --prefix=true read r3v3rs3/
$ etcdctl user add r3v3rs3
$ etcdctl user grant-role r3v3rs3 r3v3rs3-reader
```

Then write the proxy:

```bash
$ etcdctl put r3v3rs3/http/etcdapp/ports web
$ etcdctl put r3v3rs3/http/etcdapp/vhosts etcd.example.com
$ etcdctl put r3v3rs3/http/etcdapp/routes/0/servers/0/url http://10.0.0.9:8080/
```

The proxy answers within about one second:

```bash
$ curl -s -b cookies.txt http://localhost:46492/api/discovery
[{"provider":"consul","state":"running","proxies":2,"updated_at":1789647747},
 {"provider":"etcd","state":"running","proxies":1,"updated_at":1789647767}]
```

Neither the token of Consul nor the password of etcd comes back from the admin API. `GET /api/config` returns `token_set: true` and `password_set: true`. A `PUT /api/config` without the field keeps the saved value, and an empty string removes it. `config.toml` holds both as plain text, so allow only the r3v3rs3 user to read the config directory.

## Step 6: Read the Issues

A definition that does not become a proxy is an issue. The proxy list and `GET /api/discovery` name the resource and the reason:

```bash
$ etcdctl put r3v3rs3/http/badapp/ports nosuchport
$ etcdctl put r3v3rs3/http/badapp/vhosts bad.example.com
```

```json
{"provider":"etcd","state":"running","proxies":1,
 "issues":[{"resource":"r3v3rs3","message":"http.badapp: missing field `routes`"}]}
```

Add the missing field and the message names the next problem:

```json
{"issues":[{"resource":"r3v3rs3/http/badapp","message":"http.badapp: port not found: nosuchport"}]}
```

Read the issues after every change. A proxy with an issue is not added, and nothing else tells you that the definition is wrong.

## Step 7: Know What You Cannot Do

A discovered proxy is read-only:

```bash
$ curl -s -b cookies.txt -X DELETE http://localhost:46492/api/proxies/cfr-kpm
{"message":"proxy is managed by service discovery and cannot be changed: cfr-kpm","error":{"message":"proxy_read_only","id":"cfr-kpm"}}
```

The WebUI shows the proxy with its source and without the edit actions. To change it, change the tag or the key.

- r3v3rs3 does not save discovered proxies to `proxies.toml`. The provider sends them again after a restart.
- The id comes from the provider and the key of the definition, so it survives a restart.
- A provider does not open ports. A port name must select exactly one port, and the port must accept the protocol of the proxy.
- A plain text password or token in a tag or a key stays plain text in the source. Prefer `password_hash` and `token_hash`.

## Reference

- [Labels](@/discovery.md#labels): every key, every value type and the numbered lists.
- [Consul](@/discovery.md#consul) and [etcd](@/discovery.md#etcd): the settings and their defaults.
- [ACME Certificates](@/discovery.md#acme-certificates): a certificate for a discovered proxy.

## Next Steps

- [Proxies from Docker](@/tutorials/docker-discovery.md): the same keys as container labels.
- [r3v3rs3 on Kubernetes](@/tutorials/kubernetes.md): Ingress resources and the R3v3rs3Proxy resource.
- [High Availability](@/tutorials/high-availability.md): the same Consul or etcd as the cluster store.
