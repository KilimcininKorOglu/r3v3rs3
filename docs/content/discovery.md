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
