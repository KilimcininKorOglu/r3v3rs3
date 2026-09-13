+++
title = "Service Discovery"
description = "Service Discovery"
weight = 0
+++

# Service Discovery

A discovery provider reads proxy definitions from an external system and adds them to the proxy list. When a definition changes, r3v3rs3 updates the routes without a restart.

## Discovered Proxies

- A discovered proxy is read-only. The admin API returns `proxy_read_only` for an update or a delete. The WebUI shows the proxy with its source and without the edit actions. To change the proxy, change its definition in the source.
- r3v3rs3 does not save discovered proxies to `proxies.toml`. The provider sends them again after a restart.
- The id of a discovered proxy comes from the provider and the key of the definition. The proxy keeps its id after a restart, so the logs and the status keep the same id.
- A provider does not open ports. A definition names existing ports in `ports`, by port name or by port id. A port name must select exactly one port, and the port must accept the protocol of the proxy. When you rename or delete a port, r3v3rs3 resolves the port names again.
- A discovered TCP proxy cannot use a TCP port that another TCP proxy uses.
- When a provider loses its connection, the proxies of its last read stay active.

## Provider Status

The proxy list shows the state of each provider, the number of proxies that it added and the definitions that did not become a proxy. The admin API returns the same data at `GET /api/discovery`.

| State | Meaning |
|---|---|
| `connecting` | The provider has not read its resources yet. |
| `running` | The provider has read its resources and watches them for changes. |
| `error` | The provider cannot read its resources. The proxies of its last read stay active. |

An issue names the resource and the reason, for example `web-1: http.app: port not found: https`.

# Labels

Docker labels, Consul tags and key-value entries use the same keys. Each key has this form:

```text
r3v3rs3.<protocol>.<name>.<field>=<value>
```

- `<protocol>` is `http`, `tcp` or `udp`.
- `<name>` identifies the proxy in the resource. One resource can define more than one proxy.
- `<field>` is a field of the proxy model of the admin API. A dot separates nested fields.
- r3v3rs3 skips labels without the `r3v3rs3.` prefix. An unknown `r3v3rs3.` label or an unknown field is an issue, and the proxy is not added.

## Proxy Keys

| Key | Value |
|---|---|
| `r3v3rs3.enable` | `true` selects the resource when the provider does not read every resource. |
| `ports` | Required. Port names or port ids, separated by commas. |
| `name` | The name in the proxy list. The default is `<name>`. |
| `active` | `false` adds the proxy as inactive. The default is `true`. |
| `port` | The port of the upstream server on the address of the resource. |
| `scheme` | `http` or `https` for `port` of an HTTP proxy. The default is `http`. |

## Values

- A boolean is `true` or `false`. A number is a decimal number.
- A duration uses units, for example `500ms`, `30s` or `5m`.
- A list is a comma-separated value: `vhosts=app.example.com,www.example.com`.
- A list of groups uses the keys `0`, `1`, `2` and so on: `routes.0.path=/`, `routes.1.path=/api`. The numbers set the order.
- An empty value is no value.
- A comma-separated value is not available inside `auth` and header rules, because their fields are text. Use numbered keys there: `auth.users.0.username=alice`.

## HTTP Proxies

`port` and `scheme` define one route to `/` with one server. They cannot be used together with `routes`.

```text
r3v3rs3.http.app.ports=https
r3v3rs3.http.app.vhosts=app.example.com
r3v3rs3.http.app.port=8080
```

A route can also use `port` and `scheme`. They cannot be used together with `servers` of the same route.

```text
r3v3rs3.http.app.ports=http,https
r3v3rs3.http.app.vhosts=app.example.com
r3v3rs3.http.app.routes.0.path=/
r3v3rs3.http.app.routes.0.port=8080
r3v3rs3.http.app.routes.1.path=/api
r3v3rs3.http.app.routes.1.port=9443
r3v3rs3.http.app.routes.1.scheme=https
```

A server can have an explicit URL:

```text
r3v3rs3.http.app.routes.0.servers.0.url=http://10.0.0.5:8080
r3v3rs3.http.app.routes.0.servers.1.url=http://10.0.0.6:8080
r3v3rs3.http.app.load_balancing=round_robin
```

The other fields use the names of the admin API:

```text
r3v3rs3.http.app.upgrade_insecure=true
r3v3rs3.http.app.ip_filter.allow=10.0.0.0/8,192.168.0.0/16
r3v3rs3.http.app.rate_limit.requests=100
r3v3rs3.http.app.rate_limit.per=minute
r3v3rs3.http.app.rate_limit.burst=20
r3v3rs3.http.app.timeouts.connect=3s
r3v3rs3.http.app.timeouts.request=30s
r3v3rs3.http.app.health_check.interval=10s
r3v3rs3.http.app.health_check.path=/healthz
r3v3rs3.http.app.compression.algorithms=br,gzip
r3v3rs3.http.app.cache.enabled=true
r3v3rs3.http.app.cache.default_ttl=1m
r3v3rs3.http.app.headers.response.0.action=set
r3v3rs3.http.app.headers.response.0.name=X-Frame-Options
r3v3rs3.http.app.headers.response.0.value=DENY
r3v3rs3.http.app.auth.type=basic
r3v3rs3.http.app.auth.realm=Staff
r3v3rs3.http.app.auth.users.0.username=alice
r3v3rs3.http.app.auth.users.0.password=change-me
```

r3v3rs3 replaces plain text passwords and tokens with hashes before it uses the proxy. The source still holds the plain text value, so prefer `password_hash` and `token_hash` in labels.

## TCP and UDP Proxies

`port` defines one upstream server on the address of the resource. It cannot be used together with `upstream_servers`.

```text
r3v3rs3.tcp.db.ports=postgres
r3v3rs3.tcp.db.port=5432

r3v3rs3.udp.dns.ports=dns
r3v3rs3.udp.dns.upstream_servers.0.addr=/ip4/10.0.0.53/udp/53
r3v3rs3.udp.dns.session_idle_timeout=30s
```

# Docker

The Docker provider reads the labels of the running containers through the Docker Engine API. It follows the container events, so a started, stopped or changed container updates the proxies within about one second.

## Settings

Fill in "Docker Service Discovery" in "Settings", or edit `config.toml`:

```toml
[discovery.docker]
enabled = true
endpoint = "unix:///var/run/docker.sock"
network = "proxy"
exposed_by_default = false
```

| Setting | Meaning |
|---|---|
| `enabled` | Starts the provider. |
| `endpoint` | `unix://<path>`, `tcp://<host>:<port>`, `http://<host>:<port>` or `https://<host>:<port>`. The default is `unix:///var/run/docker.sock`. Windows does not support `unix://`. |
| `client_cert` | The id of a client certificate that r3v3rs3 sends to an `https` endpoint. A system root certificate or a root certificate in r3v3rs3 must sign the server certificate. |
| `network` | The Docker network of the upstream addresses. Leave it empty when each container is on one network. |
| `exposed_by_default` | `true` reads every container that has `r3v3rs3.` labels. `false` reads only the containers with `r3v3rs3.enable=true`. |

A change of the settings restarts the provider without a server restart. A certificate that the provider uses cannot be deleted.

## Containers

- The upstream address of a container is its IP address on the selected network. A container with the `host` network mode uses `127.0.0.1`.
- A container whose health check fails is skipped and shown as an issue. A container whose health check has not passed yet is skipped.
- The replicas of a Compose service share their proxies. Each replica adds its servers to the routes of the proxy, so the proxy balances the load across the replicas. The replicas must define the same number of routes.
- The key of a proxy is the Compose project, the Compose service and the proxy name. A container outside Compose uses its container name.
- r3v3rs3 reads the container list again every five minutes without events.

## Compose Example

```yaml
services:
  r3v3rs3:
    image: ghcr.io/kilimcininkoroglu/r3v3rs3:latest
    volumes:
      - r3v3rs3-config:/root/.config/r3v3rs3
      - /var/run/docker.sock:/var/run/docker.sock:ro
    networks: [proxy]
    ports:
      - 80:80
      - 127.0.0.1:46492:46492

  whoami:
    image: traefik/whoami
    networks: [proxy]
    labels:
      r3v3rs3.enable: "true"
      r3v3rs3.http.whoami.ports: http
      r3v3rs3.http.whoami.vhosts: whoami.example.com
      r3v3rs3.http.whoami.port: "80"

networks:
  proxy:
    name: proxy

volumes:
  r3v3rs3-config:
```

Add a port with the name `http` in "Ports", enable the provider with the network `proxy`, and start the stack. Compose adds the project name to a network name unless the network sets `name`.

> Access to the Docker socket gives full control of the Docker host, which equals root access. The `:ro` option applies only to the socket file and does not make the API read-only. When the admin panel or the host is reachable from untrusted networks, connect r3v3rs3 to a Docker socket proxy that allows only `GET /containers/json` and `GET /events`.

# Consul

The Consul provider reads the `r3v3rs3.*` tags of the services in the catalog and the keys under a prefix in the key-value store. It follows the changes with blocking queries, so a registered, removed or failing service instance and a changed key update the proxies within about one second.

## Settings

Fill in "Consul Service Discovery" in "Settings", or edit `config.toml`:

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

| Setting | Meaning |
|---|---|
| `enabled` | Starts the provider. |
| `address` | The HTTP API of a Consul agent: `http://<host>:<port>`, `https://<host>:<port>` or `unix://<path>`. The default is `http://127.0.0.1:8500`. |
| `client_cert` | The id of a client certificate that r3v3rs3 sends to an `https` address. |
| `token` | The ACL token. The catalog needs `service:read` and `node:read`, and the key-value store needs `key:read` on the prefix. |
| `datacenter` | The datacenter to read. Leave it empty to read the datacenter of the agent. |
| `catalog` | Reads the tags of the services in the catalog. The default is `true`. |
| `kv` | Reads the keys under `prefix`. The default is `true`. |
| `prefix` | The key prefix. The default is `r3v3rs3`. |
| `exposed_by_default` | `true` reads every service that has `r3v3rs3.` tags. `false` reads only the services with the `r3v3rs3.enable=true` tag. |

The admin API does not return the token. `GET /api/config` returns `token_set: true` when a token is saved. A `PUT /api/config` without `token` keeps the saved token, and `"token": ""` removes it. `config.toml` holds the token as plain text, so allow only the r3v3rs3 user to read the config directory.

## Catalog Services

- A tag has the form `<key>=<value>`, for example `r3v3rs3.http.app.ports=https`. A `r3v3rs3.` tag without `=` is an issue.
- r3v3rs3 reads only the instances that pass their health checks. A selected service without a passing instance is an issue, and its proxies are removed.
- The upstream address of an instance is the service address. An instance without a service address uses the node address.
- A proxy without `port`, `routes` and `upstream_servers` uses the port of the service. A route without `port` and `servers` also uses the port of the service.
- The instances of a service share its proxies. Each instance adds its servers to the routes of the proxy, so the proxy balances the load across the instances.

Register a service with the Consul agent, for example with `consul services register whoami.json`:

```json
{
  "Service": {
    "Name": "whoami",
    "Port": 8080,
    "Tags": [
      "r3v3rs3.enable=true",
      "r3v3rs3.http.whoami.ports=http",
      "r3v3rs3.http.whoami.vhosts=whoami.example.com"
    ],
    "Check": { "HTTP": "http://localhost:8080/", "Interval": "10s" }
  }
}
```

## Key-Value Store

- A key under the prefix is a label. With the prefix `r3v3rs3`, the key `r3v3rs3/http/app/ports` is the label `r3v3rs3.http.app.ports`.
- r3v3rs3 reads every proxy under the prefix, so the keys do not need `r3v3rs3.enable`.
- A key has no address, so `port` is not available. Use `routes.<n>.servers.<n>.url` or `upstream_servers.<n>.addr`.
- A part of a key cannot contain a `.`. Such a key is an issue.

```bash
consul kv put r3v3rs3/http/app/ports https
consul kv put r3v3rs3/http/app/vhosts app.example.com
consul kv put r3v3rs3/http/app/routes/0/servers/0/url http://10.0.0.5:8080
```

# etcd

The etcd provider reads the keys under a prefix through the etcd v3 HTTP API. It follows the changes with a watch stream, so a changed key updates the proxies within about one second.

## Settings

Fill in "etcd Service Discovery" in "Settings", or edit `config.toml`:

```toml
[discovery.etcd]
enabled = true
endpoints = ["http://10.0.0.1:2379", "http://10.0.0.2:2379"]
username = "r3v3rs3"
password = "<password>"
prefix = "r3v3rs3"
```

| Setting | Meaning |
|---|---|
| `enabled` | Starts the provider. |
| `endpoints` | The HTTP API addresses of the cluster members: `http://<host>:<port>`, `https://<host>:<port>` or `unix://<path>`. When a connection fails, r3v3rs3 connects to the next address. The default is `http://127.0.0.1:2379`. |
| `client_cert` | The id of a client certificate that r3v3rs3 sends to an `https` endpoint. |
| `username` | The user of etcd authentication. Leave it empty when authentication is off. |
| `password` | The password of the user. Set it together with `username`. |
| `prefix` | The key prefix. The default is `r3v3rs3`. |

The admin API does not return the password. `GET /api/config` returns `password_set: true` when a password is saved. A `PUT /api/config` without `password` keeps the saved password, and `"password": ""` removes it. `config.toml` holds the password as plain text, so allow only the r3v3rs3 user to read the config directory.

With authentication, r3v3rs3 requests a token with the user name and the password. When etcd rejects an expired token, r3v3rs3 requests a new token and sends the request again. The user needs a role with the read permission on the prefix:

```bash
etcdctl role add r3v3rs3-reader
etcdctl role grant-permission r3v3rs3-reader --prefix=true read r3v3rs3/
etcdctl user add r3v3rs3
etcdctl user grant-role r3v3rs3 r3v3rs3-reader
```

## Keys

- The keys use the layout of the Consul key-value store. With the prefix `r3v3rs3`, the key `r3v3rs3/http/app/ports` is the label `r3v3rs3.http.app.ports`.
- r3v3rs3 reads every proxy under the prefix, so the keys do not need `r3v3rs3.enable`.
- A key has no address, so `port` is not available. Use `routes.<n>.servers.<n>.url` or `upstream_servers.<n>.addr`.
- A part of a key cannot contain a `.`. Such a key is an issue.
- When etcd compacts the revision that the watch stream needs, r3v3rs3 reads every key again.

```bash
etcdctl put r3v3rs3/http/app/ports https
etcdctl put r3v3rs3/http/app/vhosts app.example.com
etcdctl put r3v3rs3/http/app/routes/0/servers/0/url http://10.0.0.5:8080
```
