+++
title = "Proxies"
description = "Routing, load balancing, security, caching and every other proxy option"
weight = 3
+++

r3v3rs3 supports three types of proxies:

- HTTP / HTTPS
- TCP / TCP over TLS
- UDP

Multiple ports can be bound to a proxy. However, it's not possible to bind TCP / TCP over TLS ports to an HTTP / HTTPS proxy and vice versa.

## Routing

When several HTTP / HTTPS proxies share a port, r3v3rs3 compares every route of every proxy on the port and sends the request to the most specific route. The order of the proxies and the routes does not change the result.

1. The host of the request selects the proxies first. A virtual host that is equal to the host (`app.example.com` or an IP address) wins over a wildcard (`*.example.com`). A wildcard wins over a regex pattern. A regex pattern wins over a proxy without virtual hosts, which accepts every host.
2. Of the routes with the same host match, the route with the longest path wins. The path matches whole segments, so `/api` matches `/api` and `/api/users` but not `/apiv2`.
3. When two routes have the same host match and the same path, the first route wins.

For example, with the routes below, `GET /api/users` goes to `http://api:8080/` and `GET /about` goes to `http://web:3000/`. A request for `app.example.com` goes to the `my-app` proxy, also when its path is `/api`.

```toml
[my-default]
protocol = "http"
routes = [
  { path = "/", servers = [{ url = "http://web:3000/" }] },
  { path = "/api", servers = [{ url = "http://api:8080/" }] },
]

[my-app]
protocol = "http"
vhosts = ["app.example.com"]
routes = [{ path = "/", servers = [{ url = "http://app:9000/" }] }]
```

### DNS SRV Servers

A server URL with the `http+srv` or `https+srv` scheme takes its servers from the DNS SRV records of its host name. r3v3rs3 resolves the name, and the route gets one server per SRV target with the scheme before `+srv`, the target `host:port` and the path and query of the URL. Consul, Kubernetes headless services and other service registries publish such records. The URL has no port, because the SRV records give the ports.

```toml
[my-api]
protocol = "http"
routes = [{ path = "/", servers = [{ url = "http+srv://_http._tcp.api.service.consul/v1" }] }]
```

With the SRV records `_http._tcp.api.service.consul. 30 IN SRV 0 5 8080 api-1.node.consul.` and `... 0 1 8080 api-2.node.consul.`, the route sends the requests to `http://api-1.node.consul:8080/v1` and `http://api-2.node.consul:8080/v1` with the weights `5` and `1`.

- The records with the lowest priority value get the requests. As RFC 2782 requires, a record with a higher value is a backup: it gets requests only when every server with a lower value is unhealthy or failed the request.
- The SRV weight of a record is the `weight` of its server. A weight of `0` becomes `1`, because a server with weight `0` gets no traffic in r3v3rs3. A `weight` on the SRV server itself is not allowed.
- r3v3rs3 resolves the name again when the TTL of the answer expires, at the earliest after 5 seconds. When the targets change, the route gets the new servers without a restart. The health of the servers that stay is kept. A failed lookup, for example a SERVFAIL answer or a timeout, keeps the last targets and is retried after 5 seconds. An answer that the name does not exist (NXDOMAIN) or has no SRV records removes the targets, because it is a valid answer (RFC 8767); the next lookup follows the negative TTL of the answer, at the earliest after 5 seconds.
- A route whose SRV name has no target answers with 502. The other servers of the route, if any, still get the requests.
- The "Upstream DNS Resolver" setting (see [Settings](@/reference/administration.md)) selects the DNS server of the SRV lookups, for example `127.0.0.1:8600` for the Consul DNS interface. Without the setting, r3v3rs3 uses the system resolver. The host names of the targets are resolved by the system resolver when r3v3rs3 connects.

### Rule Matching Dry Run

Before saving proxy changes, you can test how incoming requests will be routed using the built-in dry-run testing tool. In the WebUI proxy form, the **Rule Matching Dry Run** card lets you enter a sample URL, HTTP method and optional client IP. Clicking **Simulate Request** tests the unsaved rules in memory:

- Identifies which route or redirect rule matches the request (or explains why no route matched).
- Shows the resolved upstream URL after path rewrite rules (`strip_prefix`, `add_prefix`, regex replacement).
- Checks client IP filtering (`allow` / `deny` lists) to indicate whether the request would be blocked.
- Notes any authentication requirement (Basic Auth, Bearer, Forward Auth, or Session).

The admin API exposes this tool at `POST /api/proxies/dry-run`:

```bash
curl -X POST http://localhost:8080/api/proxies/dry-run \
  -H "Cookie: r3v3rs3_session=..." \
  -H "Content-Type: application/json" \
  -d '{"proxy": {"routes": [{"path": "/api", "servers": [{"url": "http://127.0.0.1:8080"}]}]}, "url": "https://example.com/api/users", "method": "GET"}'
```

## HTTPS Redirect

"Automatically Redirect HTTP to HTTPS" (`upgrade_insecure`, default `true`) answers a plain HTTP request with `301` to the same host and path on the HTTPS port of the proxy. A proxy without an HTTPS port does not redirect.

## Path Rewrite

By default, r3v3rs3 removes the route path from the request path and adds the rest to the path of the server URL. For example, a route with `path = "/api"` and the server `http://api:8080/v1/` sends `GET /api/users` to `http://api:8080/v1/users`. The `rewrite` table of a route changes the path in this order:

1. `strip_prefix = false` keeps the route path, so the same request goes to `http://api:8080/v1/api/users`.
2. `regex` replaces its first match in the path with `replacement`. The path starts with `/`. `${1}` or `${name}` inserts a capture group. A path without a match stays unchanged.
3. `add_prefix` adds a path, such as `/v2`, before the path.

r3v3rs3 keeps the query string. Authentication and the cache use the path of the client request. r3v3rs3 rejects an invalid regex, and an `add_prefix` that does not start with `/` or that contains `?` or `#`.

With the routes below, `GET /api/users` goes to `http://api:8080/v2/users` and `GET /items/42` goes to `http://shop:9000/item/42`.

```toml
[my-shop]
protocol = "http"
routes = [
  { path = "/api", servers = [{ url = "http://api:8080/" }], rewrite = { add_prefix = "/v2" } },
  { path = "/items", servers = [{ url = "http://shop:9000/" }], rewrite = { strip_prefix = false, regex = "^/items/([0-9]+)$", replacement = "/item/${1}" } },
]
```

## Redirect Rules

`redirects` of an HTTP / HTTPS proxy answers a request with a redirect, so the request does not reach an upstream server. Each rule has a `regex`, a `target` and a `status`:

- `regex` matches the host without the port, the path and the query of the request, e.g. `example.com/old/page?id=1`.
- `target` is the `Location` header of the response. `${1}` or `${name}` inserts a capture group.
- `status` is `301`, `302` (default), `307` or `308`.

The first matching rule answers. r3v3rs3 applies the client IP filter, the rate limit and the HTTPS redirect of `upgrade_insecure` before the rules, and authentication after the rules. A target that does not form a valid header value does not match, and r3v3rs3 logs a warning. r3v3rs3 rejects another status, and a target that is empty or that contains a control character.

In the WebUI, write one rule on each line as `status regex target`. There, the regex and the target cannot contain spaces. Use `\s` in the regex and `%20` in the target.

```toml
[my-site]
protocol = "http"
vhosts = ["example.com", "www.example.com"]
redirects = [
  { regex = "^www\\.example\\.com/(.*)$", target = "https://example.com/${1}", status = 301 },
  { regex = "^example\\.com/blog/([0-9]+)$", target = "/posts/${1}" },
]
routes = [{ path = "/", servers = [{ url = "http://127.0.0.1:3000/" }] }]
```

## Fixed Responses

A route of an HTTP / HTTPS proxy can answer every request itself with `response`, instead of sending the request to `servers`. A route has either `servers` or `response`. Use a fixed response for a redirection host or a 404 host.

- `type = "redirect"` answers with a redirect to `target`. `status` is `301`, `302` (default), `307` or `308`. `preserve_path` (default `true`) appends the path and the query of the request to `target`, so `GET /a?b=1` goes to `https://example.com/a?b=1`.
- `type = "status"` answers with `status` and an optional plain text `body` of at most 4096 bytes. `status` is `200`, `400`, `403`, `404`, `410`, `429`, `451`, `500`, `502` or `503`.

r3v3rs3 applies the client IP filter, the rate limit, the HTTPS redirect of `upgrade_insecure`, the redirect rules and authentication before the fixed response. r3v3rs3 rejects a route with both `servers` and `response`, and an invalid target, status or body.

In the WebUI, select the type of each route. The new proxy page offers the "Redirection host" and "404 host" templates. Service discovery labels set the same fields, e.g. `r3v3rs3.http.old.routes.0.response.type=redirect` and `r3v3rs3.http.old.routes.0.response.target=https://example.com`. A route with `response` gets no default server from the container port.

```toml
[old-domain]
protocol = "http"
vhosts = ["old.example.com"]
routes = [{ path = "/", response = { type = "redirect", target = "https://example.com", status = 301 } }]

[catch-all]
protocol = "http"
routes = [{ path = "/", response = { type = "status", status = 404, body = "Not found" } }]
```

## Maintenance Mode

An HTTP / HTTPS proxy can be placed into maintenance mode at any time without altering or losing its existing route and upstream server definitions. When maintenance mode is active, r3v3rs3 immediately responds to all incoming requests with a designated status code (default `503 Service Unavailable`) and an optional `Retry-After` header, bypassing all upstream targets.

- `enabled` (boolean, default `false`): Toggles maintenance mode on or off.
- `status` (integer, default `503`): HTTP status code returned to clients (100–599).
- `retry_after` (optional integer): Number of seconds in the `Retry-After` header to inform clients and search crawlers when to check back.
- `body` (optional string): Custom HTML or plain text response body. When left blank, r3v3rs3 serves a built-in responsive dark-themed maintenance page (`text/html; charset=utf-8`).
- `error_page` (boolean, default `false`): Answers with the page of `status` from the [error page theme](#centralized-error-page-themes-hub) of the proxy instead of `body`, in the language and color theme of the visitor. The proxy form shows it as **Use the error page theme**. The proxy of a [stopped platform app](@/reference/platform.md#stop-start-and-restart) uses it with status `503`.

In the WebUI:
- The **Proxies** list table includes a quick one-click action link (**Enter maintenance** / **Exit maintenance**) on every HTTP proxy, switching status immediately while updating the badge to amber **Maintenance**.
- The proxy editing page provides a dedicated **Maintenance Mode** card to customize the status code, retry interval, and custom response body.

```toml
[my-app]
protocol = "http"
vhosts = ["app.example.com"]
maintenance = { enabled = true, status = 503, retry_after = 300, body = "System upgrade in progress. Back shortly." }
routes = [{ path = "/", servers = [{ url = "http://127.0.0.1:8080/" }] }]
```

## Custom Error Pages

An HTTP / HTTPS proxy can define custom HTML error pages for standard HTTP error statuses (400 to 599), such as `404 Not Found` or `502 Bad Gateway`. When an upstream server responds with that status or r3v3rs3 generates a gateway error (such as when an upstream server is unreachable or no route matches), r3v3rs3 intercepts the error and returns the custom HTML page with `Content-Type: text/html; charset=utf-8` and `Cache-Control: no-store`.

- `status` (integer, 400–599): The HTTP error status code.
- `body` (string, max 64 KB): The custom HTML content for the error page.

### Visual Error Pages Designer & Live Preview

In the WebUI, the **Custom Error Pages** section on the HTTP proxy configuration page provides an interactive designer:

- **Ready-Made HTML/CSS Template Library**: Instantly apply responsive, dark-mode friendly, self-contained templates:
  - **Minimalist Dark**: Sleek dark background with radial glow, status badge, and clean action buttons.
  - **Modern Glassmorphism**: Frosted glass card with vibrant gradient typography and elegant styling.
  - **Developer Terminal**: CLI console with simulated HTTP headers, diagnostics, and blinking cursor.
  - **Illustration Card**: Centered card featuring clean inline vector SVG graphics and friendly messaging.
  - **Maintenance Notice**: Amber maintenance alert with gear illustration and auto-refresh notices.
- **Real-Time Live Preview**: Render pages inside an isolated, sandboxed preview iframe updated live as you type.
- **Three View Modes**: Switch between **HTML/CSS Code** editor, **Live Preview**, or **Split View** (side-by-side editing and live rendering).
- **Responsive Viewport Controls**: Preview designs across **Desktop (100%)**, **Tablet (768px)**, and **Mobile (375px)** device widths.
- **Quick Status Presets**: Single-click buttons to set common error codes (`404`, `502`, `503`, `500`, `403`, `429`).
- **Live Size Counter**: Displays current payload byte size against the 64 KB limit.

```toml
[my-app]
protocol = "http"
vhosts = ["app.example.com"]
error_page_theme = "a1b2c3d4"
hide_branding = true
error_pages = [
  { status = 404, body = "<h1>Page Not Found</h1><p>Sorry, the requested page does not exist on {{host}}.</p>" },
  { status = 502, body = "<h1>Service Unavailable</h1><p>Our backend service is currently down. (Trace: {{request_id}})</p>" },
]
routes = [{ path = "/", servers = [{ url = "http://127.0.0.1:8080/" }] }]
```

### Centralized Error Page Themes Hub

Instead of defining custom HTML pages repeatedly per proxy, r3v3rs3 provides a centralized **Error Page Themes** repository managed platform-wide under **Proxies > Error Page Themes** in the WebUI:

- **Built-in Themes**: Ships with ready-to-use modern themes (`Minimalist Dark`, `Modern Glassmorphism`, `Developer Terminal`, `Illustration Card`, `Maintenance Notice`) pre-populated across all standard error status codes (`400`, `401`, `403`, `404`, `405`, `408`, `429`, `500`, `502`, `503`, `504`).
- **Interactive Visual Editor Modal**: Create and customize themes directly in the WebUI with live responsive preview viewports (Desktop, Tablet, Mobile), HTML code editor, and status code templates.
- **Custom Theme Management**: Add new custom themes with unique IDs and custom designs, or edit existing templates (`+ New Theme` in WebUI, or `r3 error-page-theme create` / `r3 error-page-theme update`).
- **Proxy Theme Association**: Assign an error page theme to any proxy with `error_page_theme = "short_id"` via WebUI, CLI (`r3 proxy update <id> ...`), or REST API.
- **Granular Per-Proxy Overrides**: Any proxy-specific `error_pages` entry takes precedence over the assigned theme for that specific status code.
- **Suppress Software Branding**: Set `hide_branding = true` on the HTTP proxy to suppress the "Powered by r3v3rs3" link on default error pages.
- **Reset to Defaults**: Built-in themes can be restored to default templates anytime via WebUI, CLI (`r3 error-page-theme reset`), or MCP (`reset_error_page_themes`).

### Dynamic Template Variables

Error page templates support dynamic runtime variable substitution. Before returning an error page to the client, r3v3rs3 replaces the following tokens:

| Variable | Description | Example |
|---|---|---|
| `{{status}}` | HTTP status code | `502` |
| `{{message}}` | Standard status phrase or error description | `Bad Gateway` |
| `{{request_id}}` | Unique tracing identifier for the request | `184f9a0b123` |
| `{{host}}` | Requested Host header or server name | `app.example.com` |
| `{{client_ip}}` | Client remote IP address (or trusted CDN proxy IP) | `203.0.113.195` |

## UDP Sessions

A UDP proxy opens one session for each client address. The session has its own socket to the upstream server, so the upstream server sees a different source port for each client. r3v3rs3 sends the replies of the upstream server back to the client from the listening port.

A session closes when no packet passes in either direction for `session_idle_timeout` (default `60s`). It also closes when the upstream socket reports an error, or when the upstream servers or the idle timeout of the port change. The next packet of the client opens a new session. A port keeps at most 10,000 sessions. When the limit is reached, r3v3rs3 drops the packets of new clients.

`max_datagram_size` sets the maximum payload size in bytes for UDP packets (between 512 and 65527, default `65527`). A packet above this size is dropped in both directions, because a truncated packet would reach the other side as corrupt data.

## Upstream Timeouts

r3v3rs3 limits the time that it waits for an upstream server. A timeout is a duration such as `500ms`, `10s` or `1m`.

- `timeouts.connect` of an HTTP / HTTPS proxy and `connect_timeout` of a TCP / TCP over TLS proxy limit the DNS lookup, the TCP connection and the TLS handshake of a new upstream connection. The default is `10s`.
- `timeouts.request` of an HTTP / HTTPS proxy limits the time from the start of a request until the response headers arrive, including a new connection. The default is `60s`, and `0s` disables the limit. The response body, WebSocket and other upgraded connections have no limit. A route that points at the WebUI address of r3v3rs3 itself has no request limit either: the panel runs in the same process, and a backup, a restore or another long job of the panel answers when it ends.
- A route can replace the proxy timeouts with its own `timeouts`. A value that the route does not set uses its default, not the proxy value.
- `session_idle_timeout` of a UDP proxy closes an idle client session. See "UDP Sessions".

When a timeout expires, an HTTP client receives 504 Gateway Timeout, and a TCP client connection closes. r3v3rs3 rejects a connect timeout or a session idle timeout of zero.

```toml
[my-app]
protocol = "http"
vhosts = ["app.example.com"]
timeouts = { connect = "5s", request = "30s" }
routes = [
  { path = "/", servers = [{ url = "http://127.0.0.1:9000/" }] },
  { path = "/reports", servers = [{ url = "http://127.0.0.1:9001/" }], timeouts = { connect = "5s", request = "5m" } },
]

[my-database]
protocol = "tcp"
upstream_servers = [{ addr = "/ip4/127.0.0.1/tcp/5432" }]
connect_timeout = "3s"
```

## Request Body Size

`max_body_size` of an HTTP / HTTPS proxy limits the request body in bytes. The default is `0`, and `0` disables the limit. A route can replace the proxy value with its own `max_body_size`, and `0` in a route disables the limit for that route.

r3v3rs3 checks the `Content-Length` header before authentication, so a larger request receives 413 Payload Too Large and does not reach an upstream server. A body without `Content-Length`, such as a chunked body, is counted while r3v3rs3 sends it to the upstream server. When the body passes the limit before the upstream server answers, r3v3rs3 stops the upstream request and the client receives 413. The upstream server can receive the start of such a body. The limit applies to HTTP/1.1, HTTP/2 and HTTP/3 requests.

```toml
[uploads]
protocol = "http"
vhosts = ["files.example.com"]
max_body_size = 1048576
routes = [
  { path = "/", servers = [{ url = "http://127.0.0.1:9000/" }] },
  { path = "/upload", servers = [{ url = "http://127.0.0.1:9000/" }], max_body_size = 104857600 },
]
```

## Traffic Mirroring

`mirror` of a route sends a copy of the route requests to other servers, e.g. to test a new version with real traffic. r3v3rs3 drops the responses of the mirror servers. It does not retry a copy, does not count a copy in the health checks or the circuit breaker, and does not wait for a copy. So the client receives the response of the route server as before.

- `servers` lists the mirror servers. Every server receives each copy, and the weight has no effect. The path of a copy follows the same rules as the path for the route servers, including `rewrite`.
- `percent` is the share of the requests that r3v3rs3 copies, from `1` to `100` (default).
- `max_body_size` is the largest request body in bytes that r3v3rs3 copies. The default is `65536`. A request with a longer body is not copied. r3v3rs3 keeps at most this many bytes in memory for each copy.

r3v3rs3 sends a copy after it reads the whole request body. A body that fails or that the client does not finish sends no copy. A response from the cache, an upgrade request such as WebSocket, and a request that arrives while 64 copies of the route are in progress are not copied. A copy has the method and the headers of the request after the header rules. So it also carries the credentials that authentication does not remove, such as cookies. Use a mirror server that you trust with this data. r3v3rs3 rejects a `percent` outside `1` to `100`.

```toml
[my-api]
protocol = "http"
vhosts = ["api.example.com"]
routes = [
  { path = "/", servers = [{ url = "http://127.0.0.1:9000/" }], mirror = { servers = [{ url = "http://127.0.0.1:9100/" }], percent = 10 } },
]
```

## Load Balancing and Health Checks

A proxy or an HTTP route with more than one upstream server spreads the traffic with `load_balancing`:

- `round_robin` (default) uses the servers in turn.
- `least_conn` routes new requests or connections to the server with the fewest active connections, taking server weights into account.
- `random` selects a random server.
- `first` uses the first healthy server. The other servers are backups.
- `client_ip_hash` sends each client IP address to the same server while that server is healthy. An HTTP proxy uses the resolved client IP (see [Client IP](#client-ip)). When a server is unhealthy, drained or removed, only its own clients move to the other servers.
- `geo_proximity` routes traffic to the upstream server closest to the client's geographical location using MaxMind GeoIP country and regional presets (e.g. `eu`, `na`, `mena`, `apac`, `turkey`). If the closest server is unhealthy or circuit-broken, traffic automatically fails over to the next closest or global fallback servers.
- `latency` routes traffic to the healthy upstream server with the lowest measured round-trip response latency.

Each server has a `weight` from `0` to `65535` (default `1`). `round_robin` uses each server as often as its weight and spreads the turns of a server over the cycle, as the smooth weighted round robin of nginx does. `least_conn` orders servers by active connections divided by weight (`active / weight`). For example, weights `3` and `1` send three of every four requests to the first server. `random` selects a server with a chance that is proportional to its weight. `client_ip_hash` gives each server a share of the client addresses that is proportional to its weight. `first` ignores the weight. A server with `weight = 0` gets no new traffic, also when every other server is unhealthy, so you can take a server out of service without removing it. Its open TCP connections and UDP sessions stay. At least one server of each proxy or HTTP route must have a weight above `0`.

An HTTP proxy selects a server for each request, a TCP proxy for each connection, and a UDP proxy for each client session. The servers of each HTTP route are a separate group.

When the connection of a TCP proxy to the selected server fails or its connect timeout expires, r3v3rs3 tries the next server once.

An HTTP proxy sends a failed request again to the next server with its retry policy. `retry.attempts` (default `2`) is the number of tries of one request, including the first try, from `1` to `10`. `attempts = 1` disables retries. `retry.retry_on` (default `["connect"]`) lists the failures that start a retry:

- `connect`: the connection fails or the connect timeout expires. The server received nothing, so r3v3rs3 retries every method.
- `timeout`: the request timeout expires.
- `http_502`, `http_503`, `http_504`: the server answers with this status.

`timeout`, `http_502`, `http_503` and `http_504` retry only the idempotent methods of RFC 9110: `GET`, `HEAD`, `OPTIONS`, `TRACE`, `PUT` and `DELETE`. The request timeout applies to each try. A retry goes to the next server in the order of the policy and skips a server whose circuit is open. After the last try, the client receives the last response or error.

A request with a body is retried only when the length of the body is known, e.g. from `Content-Length`, and is at most `retry.replay_body_limit` bytes (default `0`). r3v3rs3 keeps such a body in memory. With `0`, only requests without a body are retried. An upgrade request, such as a WebSocket request, is not retried. A route can replace the retry policy of the proxy with its own `retry`.

The passive health check counts the consecutive failures of each server. A failure is a failed connection or a request without a response. After `health_check.max_fails` failures (default `1`), the server is unhealthy for `health_check.fail_timeout` (default `30s`). An unhealthy server gets new traffic only after the healthy servers. When every server is unhealthy, r3v3rs3 still sends the traffic to them in the same order. A success resets the failure count. `max_fails = 0` disables the check. An HTTP response with an error status such as 500 is a success, because the server answered.

The active health check runs when `health_check.interval` is above `0s` (default `0s`, disabled). Every interval, r3v3rs3 checks each server:

- An HTTP proxy with `health_check.grpc_service` probes upstream servers using the standard gRPC Health Checking Protocol (`grpc.health.v1.Health/Check`) over HTTP/2 (`h2c` prior knowledge or TLS ALPN `h2`). Leave `grpc_service = ""` to verify overall server health, or specify a service name (e.g. `grpc_service = "helloworld.Greeter"`). Only `SERVING` status passes the check; `NOT_SERVING`, `UNKNOWN`, `SERVICE_UNKNOWN`, or any non-zero gRPC error code marks the server unhealthy.
- An HTTP proxy with `health_check.path` sends `GET` to the path from the root of each server. A 2xx or 3xx status passes the check. The path must start with `/`, and only an HTTP proxy uses it.
- An HTTP proxy without a path or `grpc_service`, and a TCP proxy, open a TCP connection to each server.
- A UDP proxy resolves the host name of each server.

You can also run ad-hoc gRPC health checks directly:
- **CLI**: `r3 proxy check-grpc-health <PROXY_ID> [--service <SERVICE>]`
- **MCP Tool**: `check_proxy_grpc_health` (parameters: `proxy_id` or `target_url`, `service`)
- **REST API**: `POST /api/proxies/{id}/grpc/health` or `POST /api/grpc/health`

`health_check.timeout` (default `5s`) limits each check. A server that fails the check is unhealthy until a check passes, also when `max_fails = 0`. A successful request does not end this state.

The health of the servers stays after a configuration reload while the servers, their weights, the policy and the health check settings do not change.

The status API (`GET /api/proxies/{id}/status`) lists the health of each upstream server in `upstreams`: the address, the `weight`, `healthy`, the consecutive `failures`, and `last_error`. The proxy list of the WebUI shows the number of healthy servers and refreshes the statuses every 10 seconds. The title of the number lists the unhealthy servers with their last errors.

An HTTP proxy with [DNS SRV servers](#dns-srv-servers) also lists each SRV name in `srv`: the `name`, the `targets` (`host:port`) of the last successful lookup, the `error` of the last lookup when it failed, and `refreshed_at`, the time of the last successful lookup in seconds since the Unix epoch. The proxy list of the WebUI shows a line for each SRV name whose lookup failed.

```toml
[my-app]
protocol = "http"
vhosts = ["app.example.com"]
load_balancing = "round_robin"
health_check = { max_fails = 3, fail_timeout = "10s", interval = "10s", timeout = "2s", path = "/health" }
retry = { attempts = 3, retry_on = ["connect", "http_503"], replay_body_limit = 65536 }
routes = [
  { path = "/", servers = [{ url = "http://10.0.0.1:9000/" }, { url = "http://10.0.0.2:9000/", weight = 3 }] },
]

[my-database]
protocol = "tcp"
load_balancing = "first"
upstream_servers = [
  { addr = "/ip4/10.0.0.1/tcp/5432" },
  { addr = "/ip4/10.0.0.2/tcp/5432" },
]
```

### DNS-Based Upstream Load Balancing

When an upstream server uses a hostname that resolves to multiple IP addresses (e.g. multi-record A/AAAA round-robin DNS, Docker Swarm tasks, Kubernetes headless services, or AWS Route53):

- **Automatic Round Robin**: r3v3rs3 rotates across all resolved IP addresses in round-robin order for subsequent connections or requests.
- **DNS TTL Caching**: The resolved IP addresses are cached in memory according to the DNS record TTL (`valid_until`), eliminating unnecessary DNS lookups on every request while honoring DNS expiration.
- **Dynamic Re-resolution**: When the DNS TTL expires, r3v3rs3 automatically queries DNS again. Added, updated, or removed backend IP addresses are discovered dynamically without restarting the server or reloading the proxy configuration.
- **Failover**: For HTTP and HTTPS upstreams, if the selected IP fails to connect, r3v3rs3 automatically falls back to the remaining resolved IPs in rotation order.
- **Protocol Support**: DNS round-robin load balancing and TTL caching apply to HTTP/HTTPS upstream pools, TCP proxies, and UDP proxy sessions.
- **Negative Caching**: If a DNS query fails (e.g. NXDOMAIN or DNS server timeout), r3v3rs3 caches the failure for 5 seconds to prevent DNS flood loops.

## Circuit Breaker

The circuit breaker stops the traffic to a failing upstream server of an HTTP or TCP proxy for a time. It is off by default. Each server of each HTTP route has its own circuit. UDP proxies have no circuit breaker.

- An HTTP request fails when the connection fails, the connect or request timeout expires, or the server answers 502, 503 or 504. A TCP connection fails when it cannot connect.
- r3v3rs3 counts the requests of each server in windows of `circuit_breaker.window` (default `10s`). When a window has at least `circuit_breaker.min_requests` requests (default `20`) and at least `circuit_breaker.failure_ratio` percent of them failed (default `50`), the circuit opens.
- An open circuit gets no traffic until `circuit_breaker.open_duration` expires (default `30s`). Then the circuit is half-open, and one trial request goes to the server. A successful trial closes the circuit. A failed trial opens it again.
- When every server of an HTTP route has an open circuit, the client receives 503 Service Unavailable without a request to a server. A TCP client connection closes.

The passive health check still counts a 5xx response as a success, because the server answered. The status API shows `"circuit": "open"` or `"circuit": "half_open"` for a server whose circuit is not closed. The proxy list of the WebUI counts such a server as not healthy and names the circuit state in the title of the number.

```toml
[my-app]
protocol = "http"
vhosts = ["app.example.com"]
circuit_breaker = { enabled = true, failure_ratio = 50, min_requests = 20, window = "10s", open_duration = "30s" }
routes = [
  { path = "/", servers = [{ url = "http://10.0.0.1:9000/" }, { url = "http://10.0.0.2:9000/" }] },
]
```

## Sticky Sessions

`sticky` keeps each client of an HTTP proxy on one upstream server with a cookie. It is off by default. An individual HTTP route can also configure or override sticky sessions with `route.sticky`.

- The first response to a client sets the cookie `sticky.name` (default `r3v3rs3_affinity`). Its value is the HMAC-SHA256 signature of the proxy, the route and the server URL, so a client cannot select a server with a forged value. A value that does not match a server of the route is ignored.
- A request with a valid cookie goes to its server while the server is healthy and its circuit is not open. A server with `weight = 0` keeps its sticky clients, so a drained server finishes the sessions that it has. Otherwise `load_balancing` selects the server, and the response sets a new cookie. A retry to another server also sets a new cookie.
- r3v3rs3 removes the cookie from the request, so the upstream server does not receive it.
- The cookie has the `Path` of the route, `HttpOnly` and `SameSite=Lax`. On HTTPS and HTTP/3 it also has `Secure`. `sticky.max_age` sets `Max-Age`. Without `max_age`, the cookie ends with the browser session.
- The name must be a cookie token of RFC 6265: visible ASCII characters without spaces and separators.
- The signature key is derived from the master encryption key in `r3v3rs3.key`. Sticky cookies remain valid across process restarts, zero-downtime upgrades, and worker respawns.
- A response from the cache does not set the cookie.

A TCP or UDP proxy, and a client that does not keep cookies, can use `load_balancing = "client_ip_hash"` instead.

```toml
[my-app]
protocol = "http"
vhosts = ["app.example.com"]
sticky = { enabled = true, name = "app_server", max_age = "1h" }
routes = [
  { path = "/", servers = [{ url = "http://10.0.0.1:9000/" }, { url = "http://10.0.0.2:9000/" }] },
]
```

## Client IP

Behind a CDN or a load balancer, the TCP peer of r3v3rs3 is the edge server, not the visitor. r3v3rs3 resolves the real client IP only when the peer is trusted:

- **Known CDNs**: Cloudflare, Fastly, Amazon CloudFront, Bunny CDN, Gcore, KeyCDN, Imperva and Google Cloud Load Balancing. This is enabled by default. Turn off "Trust Client IP Headers from Known CDNs" in the proxy settings to disable it.
- **Trusted Proxies**: IP addresses or CIDR blocks that you add to the proxy, for example a local load balancer.

For a trusted peer, r3v3rs3 reads the client IP in this order:

1. The provider header: `CF-Connecting-IP` for Cloudflare, `X-Real-IP` for Bunny CDN, `CloudFront-Viewer-Address` for Amazon CloudFront.
2. The rightmost address in `X-Forwarded-For` that is not a trusted proxy or a known CDN edge. The entries left of it come from the client. r3v3rs3 never reads the RFC 7239 `Forwarded` header, because a trusted proxy that does not write it passes on the value that the client sent, and the client could then name any address.

r3v3rs3 then sends the resolved address to the upstream server in `X-Real-IP` and appends the peer to the incoming `X-Forwarded-For` chain. It builds the `Forwarded` header from that chain and drops an incoming `Forwarded` header, because a trusted proxy that does not write it passes on the value of the client. For an untrusted peer, r3v3rs3 removes `Forwarded`, `X-Forwarded-For`, `X-Real-IP`, `CF-Connecting-IP`, `True-Client-IP`, `CloudFront-Viewer-Address`, `Fastly-Client-IP` and `Incap-Client-IP`, because the client can forge them.

The CDN IP ranges are compiled into the binary and downloaded again every day. The last downloaded list is saved to `cdn-ranges.json` in the configuration directory. If a download fails, r3v3rs3 keeps the last known list. The "Settings" section shows the list status and has a "Refresh Now" button.

Akamai does not publish its edge IP ranges. Add your Akamai Site Shield ranges to "Trusted Proxies" instead.

```toml
[my-app]
protocol = "http"
client_ip = { trust_cdn = true, trusted_proxies = ["10.0.0.0/8"] }
```

## IP Filter

You can allow or deny clients by IP address for each HTTP / HTTPS proxy. r3v3rs3 checks the client IP that the "Client IP" section resolves, so the filter also works behind a CDN or a trusted proxy.

- **Denied IP Addresses**: Clients in these IP addresses or CIDR blocks receive `403 Forbidden`.
- **Allowed IP Addresses**: When the list is not empty, only clients in these IP addresses or CIDR blocks can access the proxy. Other clients receive `403 Forbidden`.
- **Allowed Countries**: Comma-separated ISO 3166-1 alpha-2 country codes (e.g. `TR`, `US`, `DE`). Requires a MaxMind GeoIP database configured in Security Center (`[geoip] database = "/path/to/GeoLite2-Country.mmdb"`).
- **Denied Countries**: Comma-separated ISO country codes. Denied countries take precedence over allowed countries.

A denied address or country takes precedence over an allowed one. Whitelisted IP CIDRs take precedence to prevent locking out local or VPN addresses.

### GeoIP Rule Wizard & Regional Presets

In the WebUI, configuring country-based access is streamlined through the visual **GeoIP Rule Wizard** card, available across proxy settings, route overrides, and access lists:

- **Enforcement Modes**:
  - `Disabled (Off)`: Country-based filtering is inactive. Incoming requests are evaluated purely against IP address rules.
  - `Allowlist (Only Selected)`: Only traffic originating from specified countries is admitted; all other client countries receive `403 Forbidden`.
  - `Denylist (Block Selected)`: Traffic originating from specified countries is immediately dropped with `403 Forbidden`; all other client countries are allowed.
  - `Advanced (Allow & Deny)`: Allows defining both permitted and blocked lists simultaneously (denied countries take precedence).
- **One-Click Regional Presets**:
  - Instantly load standard country groups: **Turkey** (`TR`), **European Union** (EU 27 member states), **North America** (`US`, `CA`, `MX`), **G7 Nations**, **DACH** (`DE`, `AT`, `CH`), **APAC**, **LATAM**, **MENA**, **Nordic**, or **Clear All**.
- **Visual Search & Tag Chips**:
  - Search any country by ISO alpha-2 code, English name, or Turkish name with real-time autocompletion.
  - Selected countries appear as removable tag chips showing their ISO code and localized country name.
- **Direct CSV Fallback**:
  - A collapsible text box allows pasting or copying comma-separated ISO country codes directly for bulk imports.

A route can replace the proxy lists with "Override IP Filter for This Route". The route then uses only its own lists. If both route lists are empty, the route allows every client.

```toml
[my-proxy]
protocol = "http"
vhosts = ["example.com"]
ip_filter = { allow = ["192.168.0.0/16"], deny = ["192.168.10.0/24"] }
routes = [
  { path = "/", servers = [{ url = "http://127.0.0.1:8080/" }] },
  { path = "/public", servers = [{ url = "http://127.0.0.1:8080/public" }], ip_filter = {} },
]
```

### Automated GeoIP Database Updates & MaxMind Licensing

r3v3rs3 includes an autonomous, zero-downtime updater for MaxMind GeoLite2-Country databases with SHA256 integrity verification, safe archive extraction, and lock-free in-memory hot reloading via atomic pointer swaps (`ArcSwapOption`).

- **MaxMind Account & License Integration**: Configure your MaxMind Account ID and License Key in Settings or via API/CLI/MCP. Secrets are safely sealed and masked in responses.
- **Configurable Background Schedule**: Set `auto_update = true` and `update_interval_days = 7` (1–90 days). The leader node checks for new releases every 12 hours in the background without impacting proxy throughput.
- **Outbound Webhook Alerts**:
  - `system.geoip_updated`: Dispatched when a new database is downloaded, verified, and hot-swapped into memory.
  - `system.geoip_update_failed`: Dispatched with error details if the download, checksum verification, or decompression fails.
- **100% Operational Parity across WebUI, CLI, and MCP**:
  - **WebUI**: Dedicated "GeoIP" tab in Security Center (`/security`) displaying loaded state, database path, file size, build epoch date, license configuration status, and an immediate "Update Database Now" button.
  - **CLI**:
    - `r3 security geoip get-status`: View loaded database status, build timestamp, size, and auto-update schedule.
    - `r3 security geoip update-database`: Download and hot-reload the latest GeoLite2-Country MMDB.
    - `r3 security geoip update-config --license-key <KEY> --auto-update true --interval-days 7`: Update GeoIP settings.
  - **MCP Tools**: `get_geoip_status`, `update_geoip_database`, `update_geoip_config`.
  - **REST API**:
    - `GET /api/security/geoip/status`
    - `PUT /api/security/geoip/config`
    - `POST /api/security/geoip/update`

## Bot Blocker (AI Scraper & Crawler Protection)

You can protect your website or application from automated AI scrapers, LLM training crawlers, and aggressive data miners with a single switch.

When **Block AI Scrapers & Bots** (`bot_blocker = true`) is enabled on an HTTP / HTTPS proxy, r3v3rs3 inspects the `User-Agent` header of every incoming request using a zero-allocation streaming matcher. If the agent matches known AI scrapers or crawlers, the request is immediately dropped with `403 Forbidden` (`error.bot_blocked`) before reaching your upstream servers.

### Blocked AI Agents & Crawlers

The built-in blocklist includes all major commercial AI data extractors and scrapers:
- **OpenAI**: `GPTBot`, `ChatGPT-User`, `OAI-SearchBot`
- **Anthropic**: `ClaudeBot`, `Claude-Web`, `anthropic-ai`
- **ByteDance / TikTok**: `Bytespider`
- **Common Crawl**: `CCBot`
- **Perplexity AI**: `PerplexityBot`
- **Cohere**: `cohere-ai`
- **Diffbot**: `Diffbot`
- **Meta**: `FacebookBot`, `Meta-ExternalAgent`
- **Google**: `Google-Extended`
- **Amazon**: `Amazonbot`
- **General Scrapers**: `Scrapy`, `SemrushBot`

### Route Overrides

A route can override the proxy setting using "Override Bot Blocker for This Route":
- Set to disabled (`false`) on public API or webhook endpoints to allow automated clients.
- Set to enabled (`true`) on private content or sensitive HTML routes.

```toml
[my-proxy]
protocol = "http"
vhosts = ["example.com"]
bot_blocker = true
routes = [
  { path = "/", servers = [{ url = "http://127.0.0.1:8080/" }] },
  { path = "/api/public", servers = [{ url = "http://127.0.0.1:8080/api" }], bot_blocker = false },
]
```

## gRPC & gRPC-Web Proxy

r3v3rs3 natively proxies HTTP/2 gRPC traffic, and provides automated **gRPC-Web proxy translation** (`grpc_web = true`) for browser clients connecting to backend gRPC services.

Web browsers cannot initiate native HTTP/2 arbitrary framing or inspect HTTP response trailers directly. When `grpc_web` is enabled:
- **CORS Preflight Automation**: Automatically answers browser `OPTIONS` preflight requests with necessary `Access-Control-Allow-Origin`, `Access-Control-Allow-Methods`, `Access-Control-Allow-Headers` (`content-type,x-grpc-web,x-user-agent`) and `Access-Control-Expose-Headers` (`grpc-status,grpc-message`) headers.
- **Request Transpilation**: Converts incoming browser `application/grpc-web` and `application/grpc-web+proto` requests to standard `application/grpc` and ensures `TE: trailers` is sent upstream.
- **Trailer Framing**: Automatically encodes upstream gRPC trailers (`grpc-status`, `grpc-message`) into standard gRPC-Web trailer frames (flag byte `0x80` + 4-byte length + header payload) at the end of the HTTP response body stream.
- **HTTP/2 Prior Knowledge (h2c)**: Automatically enables HTTP/2 prior knowledge when connecting to plain HTTP gRPC backends.

`grpc_web` can be configured globally on an HTTP proxy or overridden per route:

```toml
[grpc-service]
protocol = "https"
vhosts = ["grpc.example.com"]
grpc_web = true
routes = [
  { path = "/helloworld.Greeter/", servers = [{ url = "http://127.0.0.1:50051/" }] },
]
```

### gRPC Server Reflection & Interactive Test Console

r3v3rs3 includes built-in support for **gRPC Server Reflection** (`grpc.reflection.v1` and `grpc.reflection.v1alpha`) and an interactive **Test Console** directly inside the WebUI and Admin REST API.

#### Features
- **Automated Service Discovery**: Query upstream gRPC servers to discover registered services, methods, request message schemas, and response message types without requiring local `.proto` files.
- **Interactive Method Invocation**: Execute unary gRPC requests with custom JSON payloads and metadata headers (e.g. `authorization: Bearer ...`).
- **Detailed Inspection**: View response status code (`OK (0)`, `NOT_FOUND (5)`, `UNAVAILABLE (14)`), status messages, round-trip latency in milliseconds, formatted response JSON, and gRPC response headers and trailers.
- **Embedded & Standalone**: Test directly from an existing proxy's configuration panel or via standalone endpoints.

#### Admin API Endpoints
- `POST /api/proxies/{id}/grpc/reflect`: Discovers services from the upstream server of the given proxy ID.
- `POST /api/proxies/{id}/grpc/invoke`: Executes a unary RPC on the upstream server of the given proxy ID.
- A `target_url` in a proxy request must be one of the upstream servers of that proxy. Any other address is refused with `403 forbidden`, so these endpoints cannot reach an address outside the proxy.
- `POST /api/grpc/reflect`: Standalone service reflection targeting any reachable gRPC server (`{"target_url": "http://127.0.0.1:50051"}`).
- `POST /api/grpc/invoke`: Standalone RPC invocation with target URL, service, method, payload, and metadata.
- The standalone endpoints (`/api/grpc/reflect`, `/api/grpc/invoke`, `/api/grpc/health`) accept any address, so only admin accounts may call them. Editors use the proxy endpoints (`/api/proxies/{id}/grpc/...`), whose target must be one of the proxy's upstream servers.

## Rate Limit

You can limit the request rate of each client IP address for each HTTP / HTTPS proxy. r3v3rs3 counts requests by the client IP that the "Client IP" section resolves.

- **Requests**: Requests allowed in each period. `0` disables the limit.
- **Per**: The period: second, minute or hour.
- **Burst**: Requests that a client can send at once before the limit applies. `0` uses the "Requests" value.

A client over the limit receives `429 Too Many Requests` with a `Retry-After` header.

A route can replace the proxy limit with "Override Rate Limit for This Route". Routes without an override share one counter for each client. A route override with `0` requests disables the limit for that route.

r3v3rs3 keeps the counters in memory. A configuration change keeps the counters unless the limit itself changes. A restart resets the counters.

```toml
[my-proxy]
protocol = "http"
vhosts = ["example.com"]
rate_limit = { requests = 10, per = "second", burst = 20 }
routes = [
  { path = "/", servers = [{ url = "http://127.0.0.1:8080/" }] },
  { path = "/login", servers = [{ url = "http://127.0.0.1:8080/login" }], rate_limit = { requests = 5, per = "minute" } },
]
```

## Concurrency Limit

You can limit the number of simultaneous (in-flight) requests for each HTTP / HTTPS proxy. While rate limiting restricts requests over a time window, concurrency limiting protects upstreams from being overloaded by slow or parallel requests.

- **Max Concurrent Requests**: Maximum simultaneous active requests allowed. `0` disables concurrency limiting.
- **Header (optional)**: Group active requests by a request header (such as `x-api-key`, `authorization`, or `x-client-id`) instead of client IP. When empty, requests are grouped by the client IP address resolved by the "Client IP" section.

A client exceeding the concurrency limit immediately receives `429 Too Many Requests` with a `Retry-After: 1` header.

A route can replace the proxy limit with "Override Concurrency Limit for This Route". A route override with `0` max concurrent requests disables concurrency limiting for that route.

```toml
[my-proxy]
protocol = "http"
vhosts = ["example.com"]
concurrency_limit = { max = 20 }
routes = [
  { path = "/", servers = [{ url = "http://127.0.0.1:8080/" }] },
  { path = "/api", servers = [{ url = "http://127.0.0.1:8080/api" }], concurrency_limit = { max = 5, header = "x-api-key" } },
  { path = "/health", servers = [{ url = "http://127.0.0.1:8080/health" }], concurrency_limit = { max = 0 } },
]
```

## Authentication

You can require authentication for each HTTP / HTTPS proxy in the "Authentication" section. A route can replace the proxy authentication with "Override Authentication for This Route". Select "None" in the override to allow every client on that route.

r3v3rs3 checks authentication after the IP filter, the rate limit and the HTTPS redirect. So a browser sends the credentials on the secure connection when "Automatically Redirect HTTP to HTTPS" is on.

### Basic Auth

Clients without a valid username and password receive `401 Unauthorized` with a `WWW-Authenticate: Basic realm="..."` header. The browser then shows a login dialog.

- **Realm**: The name that the browser shows in the login dialog. Empty uses `r3v3rs3`.
- **Users**: Usernames and passwords configured directly on the proxy. A username must not contain a colon.
- **User List File (`user_file`)**: An optional absolute path to a text file on disk containing credentials (`username:password` or `username:$argon2id$...`), one entry per line (empty lines and lines starting with `#` are skipped). This is convenient for static sites or external user lists. Plaintext passwords are verified using constant-time comparison, and Argon2 hashes are supported. When the file's modification time changes, r3v3rs3 automatically hot-reloads it without restarting the proxy.

r3v3rs3 stores each password as an argon2 hash and never saves the plain text password. The admin API does not return the hash. It returns `password_set: true` for a user with a password. Leave the password field empty to keep the current password. r3v3rs3 removes the `Authorization` header before it sends the request to the upstream server.

Argon2 takes CPU time on purpose. r3v3rs3 verifies each credential once and keeps the result in memory until the configuration changes. Use a rate limit to limit password guessing.

In `proxies.toml`, you can write a `password` instead of a `password_hash`. r3v3rs3 replaces it with a hash at startup. You can also specify `user_file`:

```toml
[my-proxy]
protocol = "http"
vhosts = ["example.com"]
auth = { type = "basic", realm = "Staff", user_file = "/etc/r3v3rs3/users.txt" }
routes = [
  { path = "/", servers = [{ url = "http://127.0.0.1:8080/" }] },
  { path = "/health", servers = [{ url = "http://127.0.0.1:8080/health" }], auth = { type = "none" } },
]
```

### Bearer Token

Clients must send `Authorization: Bearer <token>` with one of the tokens of the proxy. Clients without a token receive `401 Unauthorized` with `WWW-Authenticate: Bearer realm="r3v3rs3"`. Clients with a wrong token receive the same response with `error="invalid_token"`.

- **Name**: A label that identifies the token.
- **Token**: A random value of at least 16 characters. For example, create one with `openssl rand -hex 32`.

r3v3rs3 stores the SHA-256 digest of each token and never saves the plain text token. It compares the digests in constant time. The admin API does not return the digest. It returns `token_set: true` for a token with a value. Leave the token field empty to keep the current token. r3v3rs3 removes the `Authorization` header before it sends the request to the upstream server, so the upstream server cannot receive its own bearer token on a route with bearer authentication.

In `proxies.toml`, you can write a `token` instead of a `token_hash`. r3v3rs3 replaces it with a digest at startup.

```toml
[my-api]
protocol = "http"
vhosts = ["api.example.com"]
auth = { type = "bearer", tokens = [{ name = "ci", token_hash = "<sha-256 hex digest>" }] }
routes = [{ path = "/", servers = [{ url = "http://127.0.0.1:9000/" }] }]
```

### Forward Auth

r3v3rs3 asks an external service, for example oauth2-proxy or Authelia, whether to allow each request. This works like `auth_request` in nginx.

For each client request, r3v3rs3 sends a `GET` request to the auth URL. The auth request carries the client request headers without the connection headers and `Host`, and these headers:

| Header               | Value                                                        |
|----------------------|--------------------------------------------------------------|
| `X-Forwarded-Method` | The client request method.                                   |
| `X-Forwarded-Proto`  | `http` or `https`.                                           |
| `X-Forwarded-Host`   | The client request host.                                     |
| `X-Forwarded-Uri`    | The client request path and query.                           |
| `X-Forwarded-For`    | The client IP address that the "Client IP" section resolves. |

- **2xx response**: r3v3rs3 sends the request to the upstream server. It copies the headers in "Copy Response Headers" from the auth response to the upstream request. It removes these headers from the client request first, so a client cannot send them itself.
- **Other responses**: r3v3rs3 sends the auth response (status, headers and a body up to 64 KiB) to the client. So a redirect to a login page works.
- **No response within `timeout` (default `10s`), or a connection error**: The client receives `502 Bad Gateway`.

The auth request trusts the same root certificates as the upstream requests. It sends the client certificate of the proxy, see "Upstream Client Certificates".

```toml
[my-app]
protocol = "http"
vhosts = ["app.example.com"]
auth = { type = "forward", url = "http://127.0.0.1:4180/oauth2/auth", response_headers = ["X-Auth-Request-User"], timeout = "10s" }
routes = [{ path = "/", servers = [{ url = "http://127.0.0.1:9000/" }] }]
```

### Admin Session

Clients sign in with an r3v3rs3 panel account, the same account that you use for the admin panel. Use it to protect a web application that has no authentication of its own.

- A `GET` or `HEAD` request without a session receives `302 Found` to the sign-in page. After the sign-in, r3v3rs3 redirects the browser back to the requested path.
- Other requests without a session receive `401 Unauthorized`.

Each route with this authentication serves these endpoints below its path. For the route `/`, the sign-in page is `/.r3v3rs3/auth/login`. For the route `/admin`, it is `/admin/.r3v3rs3/auth/login`.

| Endpoint               | Method | Action                                                                             |
|------------------------|--------|------------------------------------------------------------------------------------|
| `.r3v3rs3/auth/login`  | `GET`  | Shows the sign-in form.                                                            |
| `.r3v3rs3/auth/login`  | `POST` | Checks the username, the password and the TOTP code, then sets the session cookie. |
| `.r3v3rs3/auth/logout` | `POST` | Ends the session and removes the session cookie.                                   |

The TOTP code is required only for accounts with TOTP. The session cookie `r3v3rs3_session` has the `HttpOnly` and `SameSite=Lax` attributes, and the `Secure` attribute on HTTPS and HTTP/3. It has no `Domain` attribute, and r3v3rs3 accepts a session only on the host where the client signed in. r3v3rs3 removes the session cookie before it sends the request to the upstream server.

Only an account that sees the proxy can sign in. An account with a proxy list sees only the proxies of its list. r3v3rs3 checks the account on each request, so a session ends when the account is removed, when the account changes, or when the proxy leaves the proxy list of the account. A session that started before r3v3rs3 recorded the account of each session is not valid, and the client signs in again.

The `[admin]` settings in `config.toml` apply to these sessions too:

- `session_expiry`: The lifetime of a session. The minimum is 5 minutes.
- `max_login_attempts` and `login_attempts_reset`: The limit of failed sign-ins for each client IP address and username. A blocked client receives `429 Too Many Requests` until the reset time passes.

A proxy of r3v3rs3 whose route points at the admin address hands the request to the panel in process, without a TCP connection. The panel then gets the client address, the HTTPS state and the `Host` that the proxy resolved, including the [client IP](#client-ip) settings of the proxy, and no client can forge them. This proxy needs no setting below.

The `trusted_proxies` setting of `[admin]` (**Trusted Proxies** on the **Settings** page) lists the addresses or CIDR blocks of other reverse proxies in front of the panel, for example an nginx or a tunnel that connects to the admin address over TCP. The panel takes the client address of the audit log, of the rate limits and of the MCP token ban from the rightmost `X-Forwarded-For` entry that is not one of these addresses (the entries left of it come from the client; no other header such as `X-Real-IP` is read), and the HTTPS state of the session cookie from the `X-Forwarded-Proto` header, only when the request comes from one of these addresses. Otherwise it records the address of the connection. A loopback address is not trusted on its own, because any process of the host can connect from there. The workers mode (`worker_mode = "workers"`) serves the ports in another process, so its proxy connects to the panel over TCP; enter `127.0.0.1/32` in that mode. The list of CDN address ranges is a separate setting of each proxy and does not apply to the panel.

r3v3rs3 keeps the sessions in memory, so a restart signs every client out. To add a sign-out button to your application:

```html
<form method="post" action="/.r3v3rs3/auth/logout"><button>Sign Out</button></form>
```

```toml
[my-app]
protocol = "http"
vhosts = ["app.example.com"]
auth = { type = "session" }
routes = [{ path = "/", servers = [{ url = "http://127.0.0.1:9000/" }] }]
```

### Built-in OIDC / OAuth 2.0

Protect applications directly using OpenID Connect and OAuth 2.0 identity providers (Google, GitHub, Keycloak, Authentik, Okta, GitLab, etc.) without requiring an external proxy like oauth2-proxy or Authelia.

- An unauthenticated client accessing a protected path is automatically redirected to the provider's sign-in page via `/.r3v3rs3/auth/oidc/login`.
- After signing in, the provider redirects back to `/.r3v3rs3/auth/oidc/callback`. r3v3rs3 exchanges the authorization code for tokens, verifies identity, validates the email allowlist, sets an encrypted session cookie (`r3v3rs3_oidc`), and redirects the user back to the original request URL.
- On authorized upstream requests, r3v3rs3 removes the session cookie and injects `X-Forwarded-User` and `X-Forwarded-Email` headers containing the verified identity.
- Users can sign out at any time by sending a `POST` request to `/.r3v3rs3/auth/oidc/logout` (protected against cross-site requests).

| Field               | Description                                                                                                                                                                                                        |
|---------------------|--------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------|
| `issuer_url`        | The OIDC provider issuer URL (e.g., `https://accounts.google.com` or `https://auth.example.com/realms/master`). Standard authorization and token endpoints are resolved automatically if not specified explicitly. |
| `authorization_url` | Optional explicit authorization endpoint URL.                                                                                                                                                                      |
| `token_url`         | Optional explicit token endpoint URL.                                                                                                                                                                              |
| `userinfo_url`      | Optional explicit userinfo endpoint URL (used if identity claims are not in the ID token).                                                                                                                         |
| `client_id`         | OAuth 2.0 Client ID registered with the provider.                                                                                                                                                                  |
| `client_secret`     | OAuth 2.0 Client Secret (stored sealed at rest).                                                                                                                                                                   |
| `scopes`            | OAuth 2.0 scopes to request (defaults to `["openid", "email", "profile"]`).                                                                                                                                        |
| `allowed_emails`    | List of allowed email addresses or domain wildcards (e.g., `["admin@example.com", "*@company.com"]`). If empty, any user authenticated by the provider is permitted.                                               |

```toml
[internal-dashboard]
protocol = "http"
vhosts = ["dashboard.example.com"]
auth = { type = "oidc", issuer_url = "https://accounts.google.com", client_id = "your-client-id.apps.googleusercontent.com", client_secret = "your-secret", allowed_emails = ["*@mycompany.com"] }
routes = [{ path = "/", servers = [{ url = "http://127.0.0.1:8080/" }] }]
```


## Access Lists

An access list holds an IP filter and an authentication with a name. Several proxies and routes can use one access list. A change of the list applies to each of them without a restart.

The "Access Lists" page of the WebUI adds, changes and deletes the lists. The "Access List" field of an HTTP / HTTPS proxy selects a list for the proxy, and the "Access List" field of a route selects a list for the route. In the admin API and in `proxies.toml`, `access_list` sets the list of a proxy or of a route.

- The list of a proxy replaces the "IP Filter" and the "Authentication" of the proxy.
- The list of a route replaces the IP filter and the authentication of the route.
- A route without its own list or override uses the settings of the proxy, including the list of the proxy.
- A proxy or a route with a list cannot also set its own IP filter or authentication. The admin API answers `400 access_list_conflict`.
- A proxy that names an unknown list gets `400 access_list_not_found`. When the list is missing at runtime, the proxy or the route answers every client with `403 Forbidden`.
- A list that a proxy or a route uses cannot be deleted. The admin API answers `400 access_list_in_use`.

`access_lists.toml` in the configuration directory holds the lists. r3v3rs3 writes the file with mode `0600`, because it holds the password hashes and the token digests. The admin API does not return the hashes, like the proxy authentication.

```toml
# access_lists.toml
[office]
name = "Office"
ip_filter = { allow = ["192.168.0.0/16"] }
auth = { type = "basic", realm = "Office", users = [{ username = "alice", password_hash = "$argon2id$v=19$m=19456,t=2,p=1$..." }] }
```

```toml
# proxies.toml
[my-app]
protocol = "http"
vhosts = ["app.example.com"]
access_list = "office"
routes = [
  { path = "/", servers = [{ url = "http://127.0.0.1:9000/" }] },
  { path = "/health", servers = [{ url = "http://127.0.0.1:9000/health" }], ip_filter = {}, auth = { type = "none" } },
]
```

| Endpoint                        | Action                                                                         |
|---------------------------------|--------------------------------------------------------------------------------|
| `GET /api/access_lists`         | Lists the access lists.                                                        |
| `POST /api/access_lists`        | Adds an access list.                                                           |
| `PUT /api/access_lists/{id}`    | Changes an access list. A user or a token without a new secret keeps its hash. |
| `DELETE /api/access_lists/{id}` | Deletes an access list.                                                        |

Every account reads the lists. Only an admin or an editor without a proxy list changes them. See [Accounts](@/reference/accounts.md).

## Header Rules

You can change the headers of proxied requests and responses in the "Header Rules" section. A route can replace the proxy rules with "Override Header Rules for This Route".

Write one rule per line:

| Rule                 | Action                                      |
|----------------------|---------------------------------------------|
| `set Name: value`    | Replaces every value of the header.         |
| `append Name: value` | Adds a value and keeps the existing values. |
| `remove Name`        | Removes the header.                         |

r3v3rs3 skips empty lines and lines that start with `#`.

- **Request Headers**: The rules change the request that r3v3rs3 sends to the upstream server. They run after r3v3rs3 sets `Forwarded`, `X-Forwarded-*` and `Via`, so a rule can replace these headers.
- **Response Headers**: The rules change the upstream response before r3v3rs3 sends it to the client. They do not change the responses that r3v3rs3 creates itself, like error pages, redirects and sign-in pages.

Values can use these variables. Write `{{` and `}}` for a literal brace.

| Variable                    | Value                                                                                                             |
|-----------------------------|-------------------------------------------------------------------------------------------------------------------|
| `{client_ip}`               | The client IP address that the "Client IP" section resolves.                                                      |
| `{host}`                    | The requested host name.                                                                                          |
| `{scheme}`                  | `http` or `https`.                                                                                                |
| `{request_id}`              | A random 32-character hex ID. The request and response rules of one request use the same ID.                      |
| `{route}`                   | The path of the matched route, e.g. `/api`.                                                                       |
| `{client_cert_subject}`     | The subject of the verified client certificate, e.g. `CN=client.example.com`. Empty without a client certificate. |
| `{client_cert_fingerprint}` | The SHA-256 fingerprint of the verified client certificate in hex. Empty without a client certificate.            |

A client can send a header with the same name itself. Use `set`, not `append`, for the client certificate headers, so the rule replaces the value of the client.

Rules cannot change `Connection`, `Content-Length`, `Host`, `Keep-Alive`, `Proxy-Connection`, `TE`, `Trailer`, `Transfer-Encoding` and `Upgrade`, because these headers control the connection and the message framing.

### Host Header

By default, r3v3rs3 sends the address of the upstream server as the `Host` header, for example `127.0.0.1:8080`. Turn on "Keep the Client Host Header" (`preserve_host = true`) to send the host name that the client asked for instead. An application that compares the `Origin` header with the `Host` header, for example in a CSRF check, needs this setting. An upstream server that speaks HTTP/2 (an HTTPS server that selects h2, or a plain HTTP server with h2c) receives the client host as the `:authority` of the request, because HTTP/2 has no separate `Host` header. The connection and the TLS certificate check still use the address of the upstream server. The proxies of the deployment platform apps always keep the client host. A discovered proxy sets it with the `r3v3rs3.http.<name>.preserve_host=true` label.

```toml
[my-app]
protocol = "http"
vhosts = ["app.example.com"]
preserve_host = true
routes = [{ path = "/", servers = [{ url = "http://127.0.0.1:9000/" }] }]
```

```toml
[my-app]
protocol = "http"
vhosts = ["app.example.com"]
headers = { request = [{ action = "set", name = "X-Request-Id", value = "{request_id}" }, { action = "remove", name = "X-Debug" }], response = [{ action = "set", name = "X-Frame-Options", value = "DENY" }, { action = "remove", name = "Server" }] }
routes = [{ path = "/", servers = [{ url = "http://127.0.0.1:9000/" }] }]
```

### W3C Trace Context & Correlation IDs

r3v3rs3 implements distributed tracing propagation following the W3C Trace Context recommendation:

- **W3C Traceparent Extraction & Forwarding**: When an incoming request includes a valid `traceparent` header (format `00-{trace_id}-{parent_id}-{flags}`), r3v3rs3 extracts the trace ID and generates a new child span ID before forwarding `traceparent` to the upstream service.
- **Automatic Generation**: When `traceparent` is absent or malformed, r3v3rs3 generates a new valid W3C traceparent header (`00-{trace_id}-{span_id}-01`).
- **Tracestate Propagation**: Inbound `tracestate` headers are preserved and forwarded without modification.
- **Request ID Synchronization**: The correlation ID (`{request_id}`) matches the inbound `X-Request-Id` or the 32-hex trace ID. If `X-Request-Id` is missing from the inbound request, r3v3rs3 injects `X-Request-Id` using the trace ID into upstream request headers.
- **Error Response Correlation**: All error responses generated by r3v3rs3 (such as 502 Bad Gateway, 504 Gateway Timeout, or 404 Not Found) attach the `X-Request-Id` HTTP header and display the correlation ID in both default and custom error pages for user debugging and log tracing.

### Security Header Hardening Profiles

r3v3rs3 includes built-in OWASP security header profiles and an automated scoring engine (A+ to F) to harden proxy responses with one click, via the WebUI, MCP, or the CLI:

- **Strict (`strict`)**: Full defense-in-depth profile for high-security applications (`max-age=63072000; includeSubDomains; preload`, `X-Content-Type-Options: nosniff`, `X-Frame-Options: DENY`, `Referrer-Policy: no-referrer`, restrictive `Permissions-Policy`, strict `Content-Security-Policy`, and isolation headers `Cross-Origin-Opener-Policy: same-origin` and `Cross-Origin-Resource-Policy: same-origin`).
- **Balanced (`balanced`)**: Recommended profile for typical CMS and multi-page web applications (`max-age=31536000; includeSubDomains`, `X-Frame-Options: SAMEORIGIN`, `Referrer-Policy: strict-origin-when-cross-origin`, permissive CSP supporting inline styles and common CDNs).
- **API Only (`api_only`)**: Optimized for REST, JSON, and gRPC endpoints (`X-Frame-Options: DENY`, `Cache-Control: no-store, max-age=0`, restrictive CSP, `Referrer-Policy: no-referrer`).
- **SPA Frontend (`spa_frontend`)**: Tailored for Vite, React, Vue, Svelte, and modern Single Page Applications with worker, blob, and WebSocket support.

#### CLI Commands

```bash
# List available security profiles
r3 proxy security-headers list-profiles

# Audit security headers of a proxy and inspect score and recommendations
r3 proxy security-headers get <PROXY_ID>

# Apply an OWASP security hardening profile to a proxy
r3 proxy security-headers apply-profile <PROXY_ID> strict
```

#### MCP Tools

AI agents can inspect and apply security profiles using `list_security_header_profiles`, `get_proxy_security_headers`, and `apply_proxy_security_header_profile`.

## Compression

You can compress proxied responses in the "Compression" section. Select one or more encodings to enable compression. Select no encoding to disable compression.

| Encoding             | `Content-Encoding` | Default Level | Adaptive Levels (< 8 KB / 8–64 KB / > 64 KB or Stream) |
|----------------------|--------------------|---------------|--------------------------------------------------------|
| Brotli               | `br`               | Quality 4     | Quality 5 / Quality 4 / Quality 3                      |
| Zstandard            | `zstd`             | Level 3       | Level 4 / Level 3 / Level 1 (Fastest)                  |
| Gzip                 | `gzip`             | Level 6       | Level 9 (Best) / Level 6 / Level 1 (Fastest)           |
| Zstandard Dictionary | `dcz`              | Level 3       | Level 4 / Level 3 / Level 1 (Fastest)                  |
| Brotli Dictionary    | `dcb`              | Quality 4     | Quality 5 / Quality 4 / Quality 3                      |

r3v3rs3 reads the `Accept-Encoding` header of the request and uses the accepted encoding with the highest `q` value. When the client accepts several encodings with the same `q` value, r3v3rs3 uses the order of `algorithms`. In the panel, the order in which you select the encodings sets this order.

When `adaptive_level` is enabled (default `true`), r3v3rs3 tunes compression quality based on response size: small responses (< 8 KB) receive higher compression ratios where CPU cost is negligible, while large or streamed responses use maximum throughput levels to prevent worker thread saturation. In addition, the minimum size threshold is dynamically lowered to 256 bytes for text, JSON, and SVG payloads.

### Shared Compression Dictionaries (RFC 9842)

r3v3rs3 supports Compression Dictionary Transport (RFC 9842) for Chromium and compatible modern HTTP clients:
- **Serving Dictionaries**: Dictionaries configured on an HTTP proxy are directly served by r3v3rs3 at their configured `path` with `Content-Type: application/octet-stream`, `Use-As-Dictionary: match="<match_pattern>", id="<id>"`, `Cache-Control: public, max-age=31536000, immutable`, and `ETag: "<sha256>"`.
- **Content Negotiation & Encoding**: When an incoming request includes `Available-Dictionary` (or `Sec-Available-Dictionary`) matching a configured dictionary's SHA-256 hash and path, and the client accepts `dcz` or `dcb`, r3v3rs3 compresses the response against the dictionary. The response carries `Content-Encoding: dcz` (or `dcb`) and `Vary: Available-Dictionary, Accept-Encoding`. The body starts with the fixed header of the encoding and the SHA-256 digest of the dictionary, so the client can check that it decodes with the right dictionary. RFC 9842 allows dictionary compression only in a secure context, so requests over plain HTTP get the standard algorithms.
- **Bandwidth Reduction**: By eliminating static JSON schemas, recurring object keys, and repetitive HTML wrappers, shared dictionary compression delivers 80% to 95% bandwidth savings on recurring API traffic.
- **Management**: Compression dictionaries can be added, inspected, and removed via the WebUI Proxy Configuration page, the API (`/api/proxies/{id}/dictionaries` and `/api/proxies/{id}/dictionaries/{dict_id}`), MCP tools (`list_proxy_compression_dictionaries`, `get_proxy_compression_dictionary`, `create_proxy_compression_dictionary`, `delete_proxy_compression_dictionary`), or the CLI (`r3 proxy dictionary list/get/create/delete`). The server picks the id and computes the hash of a new dictionary, and refuses it on a TCP or UDP proxy.

r3v3rs3 compresses a response only when all of these conditions are true:

- The status is not `1xx`, `204 No Content`, `206 Partial Content` or `304 Not Modified`.
- The upstream server did not encode the response, and the response has no `Content-Range` header.
- `Cache-Control` does not contain `no-transform`.
- The media type of `Content-Type` is in `mime_types` and is not an inherently pre-compressed format (such as PNG, JPEG, WebP, AVIF, MP4, WOFF2, or ZIP archives). r3v3rs3 never compresses `text/event-stream`, because compression holds server-sent events back.
- `Content-Length` is equal to or larger than `min_size` (or the adaptive threshold). r3v3rs3 compresses a streamed response without `Content-Length`.

When a response meets these conditions, r3v3rs3 adds `Accept-Encoding` to `Vary`. When r3v3rs3 compresses the response, it also removes `Content-Length` and `Accept-Ranges`, and changes a strong `ETag` to a weak `ETag`. The response header rules run before compression, so a rule can set `Cache-Control: no-transform` to stop compression.

| Setting          | Default                                                                                                                                                                                                                                     | Description                                                                                                                                                                                                              |
|------------------|---------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------|--------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------|
| `algorithms`     | Empty. Compression is disabled.                                                                                                                                                                                                             | List of compression algorithms (`br`, `zstd`, `gzip`) in preferred order.                                                                                                                                                |
| `min_size`       | `1024` bytes                                                                                                                                                                                                                                | Minimum body size required before compression is applied.                                                                                                                                                                |
| `mime_types`     | `text/*`, `application/javascript`, `application/json`, `application/manifest+json`, `application/wasm`, `application/xml`, `application/xhtml+xml`, `application/rss+xml`, `application/atom+xml`, `image/svg+xml`, `font/otf`, `font/ttf` | Media types eligible for compression. Incompressible formats (PNG, JPEG, WebP, AVIF, WOFF2, ZIP) are automatically excluded.                                                                                             |
| `adaptive_level` | `true`                                                                                                                                                                                                                                      | Dynamically adapts compression levels based on response payload size (small payloads use higher ratios, large/streaming payloads use fastest throughput), and lowers min-size threshold down to 256 bytes for text/JSON. |
| `precompressed`  | `true`                                                                                                                                                                                                                                      | Negotiates and serves pre-compressed static assets (`.zst`, `.br`, `.gz`) directly with zero CPU overhead.                                                                                                               |

```toml
[my-app]
protocol = "http"
vhosts = ["app.example.com"]
compression = { algorithms = ["br", "zstd", "gzip"], min_size = 1024, mime_types = ["text/*", "application/json"], adaptive_level = true, precompressed = true }
routes = [{ path = "/", servers = [{ url = "http://127.0.0.1:9000/" }] }]
```

## Cache

You can store proxied responses in memory in the "Cache" section. A stored response goes to the client without a request to the upstream server. Every route of the proxy shares one cache, and each proxy has its own cache.

| Setting                  | Default             | Description                                                                                                                                                                                                                |
|--------------------------|---------------------|----------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------|
| `enabled`                | `false`             | Enables the cache.                                                                                                                                                                                                         |
| `max_size`               | `67108864` (64 MiB) | The memory limit of the stored responses in bytes. When the cache is full, r3v3rs3 removes the least used responses.                                                                                                       |
| `max_entry_size`         | `1048576` (1 MiB)   | r3v3rs3 does not store a response with a larger body.                                                                                                                                                                      |
| `default_ttl`            | `0s`                | The lifetime of a response without `Cache-Control: max-age`, `s-maxage` or `Expires`. With `0s`, r3v3rs3 stores such a response only when it has an `ETag` or `Last-Modified` header, and revalidates it on every request. |
| `stale_while_revalidate` | `0s`                | Window during which a stale response is served immediately (`X-Cache: STALE`) while r3v3rs3 revalidates it asynchronously in the background. Upstream `stale-while-revalidate` in `Cache-Control` takes precedence.        |

r3v3rs3 uses the cache only for `GET` and `HEAD` requests without `Range`, `Upgrade` and `Cache-Control: no-store`. A `HEAD` request uses the stored `GET` response. When the client sends `Cache-Control: no-cache` or `Pragma: no-cache`, r3v3rs3 sends the request to the upstream server and stores the new response. The cache key is the requested host with the path and the query of the request, so the upstream server that load balancing selects does not change the key.

r3v3rs3 stores a response only when all of these conditions are true:

- The status is `200`, `203`, `204`, `300`, `301`, `308`, `404`, `405`, `410`, `414` or `501`.
- `Cache-Control` does not contain `no-store` or `private`.
- The response has no `Set-Cookie` header.
- The response has no `Vary: *` header.
- When the request has an `Authorization` header, `Cache-Control` contains `public`, `s-maxage` or `must-revalidate`.
- The response has a lifetime or a validator, and the body is not larger than `max_entry_size`.
- The body arrives complete: it holds the `Content-Length` bytes, or a chunked body ends with its last chunk. A body that the upstream server cuts is not stored.

The lifetime comes from `s-maxage`, then `max-age`, then `Expires`, then `default_ttl`. `Cache-Control: no-cache` sets the lifetime to zero. The age of a stored response includes the `Age` header of the upstream response.

When a stored response is stale and `stale_while_revalidate` (or upstream `stale-while-revalidate` header directive) has not elapsed, r3v3rs3 immediately responds with the stale cached content and sets `X-Cache: STALE`, while spawning a non-blocking background revalidation to refresh the cache. In-flight background revalidations are locked per cache key to prevent dogpiling.

When a stored response is stale and outside the `stale_while_revalidate` window, if it has an `ETag` or `Last-Modified` header, r3v3rs3 sends a conditional request with `If-None-Match` or `If-Modified-Since`. When the upstream server answers `304 Not Modified`, r3v3rs3 updates the stored headers and sends the stored response. r3v3rs3 keeps a stale response with a validator for one hour after it expires. A client with a matching `If-None-Match` or `If-Modified-Since` header receives `304 Not Modified` from the cache.

r3v3rs3 stores one response for each cache key. When `Vary` names request headers, r3v3rs3 sends the stored response only to a request with the same values of these headers.

r3v3rs3 removes `Accept-Encoding` from the requests that can use the cache, so the upstream server sends unencoded responses. The "Compression" section then compresses the response for each client. Each response to these requests has an `X-Cache` header: `HIT` for a stored response, `MISS` for a response from the upstream server, and `STALE` when served during background revalidation. A response from the cache also has an `Age` header.

To remove stored responses:
- Send `DELETE /api/proxies/{id}/cache` (or `DELETE /api/proxies/{id}/cache?path=/api/v1/` to purge only entries whose path starts with the given prefix).
- Send `POST /api/proxies/{id}/cache/purge` with optional JSON body `{"path": "/prefix"}`.
- Send an HTTP `PURGE` request directly to the proxy URL (e.g. `curl -X PURGE https://app.example.com/data.json`). The proxy accepts it only when the route has an authentication that the request passed, or when the client address is a loopback address. A request through a trusted proxy or CDN does not count as trusted, because the trusted proxy forwards every internet client; other clients get `403 Forbidden`.
- Or click "Purge" in the proxy list of the WebUI.

The stored responses stay in memory while the cache settings do not change. A restart or a change of the cache settings removes them.

```toml
[my-app]
protocol = "http"
vhosts = ["app.example.com"]
cache = { enabled = true, max_size = 67108864, max_entry_size = 1048576, default_ttl = "5m", stale_while_revalidate = "1m" }
routes = [{ path = "/", servers = [{ url = "http://127.0.0.1:9000/" }] }]
```

## HTTP/2

r3v3rs3 supports HTTP/2 for HTTP and HTTPS proxies in both upstream and downstream connections.

Downstream, HTTP/2 is automatically negotiated if the client supports it. Most web browsers use HTTP/2 only over TLS, because they need ALPN (Application-Layer Protocol Negotiation) to learn that the server supports HTTP/2.

Upstream, r3v3rs3 offers `h2` and `http/1.1` with ALPN to HTTPS servers and uses the protocol that the server selects. Plain HTTP servers receive HTTP/1.1, because a plain connection cannot negotiate the protocol. Set `h2c = true` on a proxy whose plain HTTP servers accept HTTP/2 with prior knowledge (h2c). WebSocket and other upgrade requests always use HTTP/1.1. The proxies of a port that use the same client certificate and the same connect timeout share the upstream connections, so one HTTP/2 upstream connection carries the requests of many clients.

```toml
[my-app]
protocol = "http"
vhosts = ["app.example.com"]
h2c = true
routes = [{ path = "/", servers = [{ url = "http://127.0.0.1:9000/" }] }]
```

## WebSocket

r3v3rs3 supports WebSocket (and HTTP upgrading) for HTTP and HTTPS proxies. You don't need to do anything special to enable WebSocket support.

## HTTP/3

To enable HTTP/3 proxying, bind a QUIC port in the Ports section and select HTTP over QUIC as the protocol. HTTP/3 is only supported for incoming connections. Upstream connections use HTTP/2 or HTTP/1.1.

r3v3rs3 replaces the `Alt-Svc` header of each response with the HTTPS and QUIC ports of the proxy, so a browser learns about HTTP/3 from its first HTTPS response.

WebTransport is not supported.

## Upstream Client Certificates

An upstream server can require a client certificate (mutual TLS). Select the certificate in the "Client Certificate" field of an HTTP / HTTPS proxy or a TCP / TCP over TLS proxy. The list shows the client certificates that have a private key. See "Client Certificates".

- The proxy sends the certificate to every TLS upstream server that asks for one. A plain HTTP or plain TCP upstream server does not use it.
- On an HTTP / HTTPS proxy, the forward auth request sends the same certificate.
- r3v3rs3 rejects a proxy config whose certificate does not exist, is not a client certificate or has no private key. A client certificate that a proxy uses cannot be deleted.
- When the certificate becomes invalid, for example after it is removed from the configuration directory, the proxy does not connect without it. An HTTP / HTTPS proxy returns `502 Bad Gateway`, and a TCP proxy closes the connection.

On a TCP proxy, turn on "Connect with TLS" for an upstream server that expects TLS. The address of such a server ends with `/tls`.

```toml
[my-app]
protocol = "http"
vhosts = ["app.example.com"]
client_cert = "a1b2c3d"
routes = [{ path = "/", servers = [{ url = "https://10.0.0.5:8443/" }] }]

[my-database]
protocol = "tcp"
client_cert = "a1b2c3d"
upstream_servers = [{ addr = "/dns/db.internal/tcp/5433/tls" }]
```

## Sending PROXY Protocol

A TCP / TCP over TLS proxy opens a new connection to its upstream server, so without a PROXY protocol header the upstream server sees the address of r3v3rs3 as the client: `127.0.0.1` for a platform app on the same server. A server that needs the real client address, for example a TURN server behind a TLS port of r3v3rs3, must read PROXY protocol.

A TCP / TCP over TLS proxy can send the client address to its upstream servers. Select the version in "Send PROXY Protocol". Each upstream connection then starts with a PROXY protocol header, before the TLS handshake of a TLS upstream server.

- The source address is the client address of the connection. When the port receives PROXY protocol, it is the address from that header. See "PROXY Protocol".
- The destination address is the address that the client connected to.
- When the two addresses have different families, both are written as IPv6 addresses. An IPv4 address becomes an IPv4-mapped IPv6 address.
- The active health check sends a version 2 `LOCAL` header.

Enable it only when every upstream server reads the header, because a server that does not read it receives the header as data. HTTP / HTTPS proxies do not send PROXY protocol. Use the `Forwarded` and `X-Forwarded-For` headers instead.

```toml
[my-mail]
protocol = "tcp"
proxy_protocol = "v2"
upstream_servers = [{ addr = "/dns/mail.internal/tcp/25" }]
```
