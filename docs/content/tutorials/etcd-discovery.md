+++
title = "Proxies from etcd"
description = "Build proxies from the keys of an etcd cluster"
weight = 13
+++

# Proxies from etcd

A deploy job that writes etcd keys can carry its own proxy definition. r3v3rs3 reads the keys under a prefix and builds the proxies without a restart and without a click in the WebUI, and a changed key updates its proxy within about one second.

You need a port from [Getting Started](@/tutorials/getting-started.md). This guide uses an HTTP port named `web`. A provider never opens a port; it only names a port that already exists.

## Step 1: Turn On the Provider

Open **Settings** and fill in **etcd Service Discovery**, or edit `config.toml`:

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

The provider state appears in the proxy list and at `GET /api/discovery`:

```bash
$ curl -s -b cookies.txt http://localhost:46492/api/discovery
[{"provider":"etcd","state":"running","proxies":0,"updated_at":1789647725}]
```

`connecting` means the first read is not finished. `running` means the provider watches for changes. `error` means it cannot read; the proxies of the last read stay active.

A change of the settings restarts the provider. r3v3rs3 itself does not restart.

## Step 2: Write a Proxy

A key under the prefix is a label, with `/` instead of `.`:

```bash
$ etcdctl put r3v3rs3/http/etcdapp/ports web
$ etcdctl put r3v3rs3/http/etcdapp/vhosts etcd.example.com
$ etcdctl put r3v3rs3/http/etcdapp/routes/0/servers/0/url http://10.0.0.9:8080/
```

Within about one second the proxy list holds a new proxy:

```bash
$ curl -s -b cookies.txt http://localhost:46492/api/proxies | jq -c '.[] | {id, name, source}'
{"id":"cfr-kpm","name":"etcdapp","source":{"provider":"etcd","resource":"http/etcdapp"}}
```

The key set sets `ports`, `vhosts` and one server URL. The key names are the fields of the proxy model of the admin API; see [Labels](@/discovery.md#labels) for the complete list.

r3v3rs3 reads every proxy under the prefix, so a key set does not need `r3v3rs3.enable`. A key has no address, so `port` and `scheme` are not available; write the server URL. A part of a key cannot contain a `.`. Such a key is an issue.

The watch stream reports every change. Change the server URL and the proxy follows within about one second:

```bash
$ etcdctl put r3v3rs3/http/etcdapp/routes/0/servers/0/url http://10.0.0.10:8080/
```

The password of etcd does not come back from the admin API. `GET /api/config` returns `password_set: true`. A `PUT /api/config` without `password` keeps the saved password, and an empty string removes it. `config.toml` holds the password as plain text, so allow only the r3v3rs3 user to read the config directory.

## Step 3: Read the Issues

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

## Step 4: Know What You Cannot Do

A discovered proxy is read-only:

```bash
$ curl -s -b cookies.txt -X DELETE http://localhost:46492/api/proxies/cfr-kpm
{"message":"proxy is managed by service discovery and cannot be changed: cfr-kpm","error":{"message":"proxy_read_only","id":"cfr-kpm"}}
```

The WebUI shows the proxy with its source and without the edit actions. To change it, change the key.

- r3v3rs3 does not save discovered proxies to `proxies.toml`. The provider sends them again after a restart.
- The id comes from the provider and the key of the definition, so it survives a restart.
- A provider does not open ports. A port name must select exactly one port, and the port must accept the protocol of the proxy.
- A plain text password or token in a key stays plain text in the source. Prefer `password_hash` and `token_hash`.

## Reference

- [Labels](@/discovery.md#labels): every key, every value type and the numbered lists.
- [etcd](@/discovery.md#etcd): the settings and their defaults.
- [ACME Certificates](@/discovery.md#acme-certificates): a certificate for a discovered proxy.

## Next Steps

- [Proxies from Docker](@/tutorials/docker-discovery.md): the same keys as container labels.
