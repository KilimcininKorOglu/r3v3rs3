+++
title = "Configuration"
description = "Configuration"
weight = 0
+++

# Ports

Before configuring a proxy, you need to bind a port to listen on. You can do this in the "Ports" section.

r3v3rs3 supports six types of ports:

- HTTP
- HTTPS (HTTP over TLS)
- HTTP over QUIC (HTTP/3)
- TCP
- TCP over TLS
- UDP

## Server Names

HTTPS, HTTP over QUIC and TCP over TLS ports have a "Server Names" field. r3v3rs3 selects the server certificate from the SNI (Server Name Indication) of the client. When the client sends no SNI, r3v3rs3 selects a valid server certificate that has all the server names. When the list is empty, r3v3rs3 selects the first valid server certificate.

```toml
[my-port]
listen = "/ip4/0.0.0.0/tcp/443/https"
tls_termination = { server_names = ["example.com", "*.example.com"] }
```

## TLS Client Authentication

HTTPS, HTTP over QUIC and TCP over TLS ports can verify the certificate of the client (mutual TLS). Select the mode in "Client Authentication":

| Mode | Behavior |
|---|---|
| Off | The port does not ask for a client certificate. This is the default. |
| Optional | The port asks for a certificate but also accepts clients without one. A certificate that the client sends must be valid. |
| Required | The TLS handshake fails when the client sends no valid certificate. |

In Optional and Required mode, select one or more root certificates in "Client CA Certificates". A client certificate must be signed by one of them. The system root certificates are not used. r3v3rs3 rejects a port config without a root certificate, or with a certificate that is not a root certificate. A root certificate that a port uses cannot be deleted.

When the client authentication config becomes invalid, for example after the root certificate is removed from the configuration directory, the port closes every connection and the port list shows "TLS Error".

On HTTPS and HTTP over QUIC ports, header rules can send the verified certificate to the upstream server with the `{client_cert_subject}` and `{client_cert_fingerprint}` variables. See "Header Rules".

```toml
[my-port]
listen = "/ip4/0.0.0.0/tcp/443/https"
tls_termination = { server_names = ["example.com"], client_auth = "required", client_ca_certs = ["a1b2c3d"] }
```

## PROXY Protocol

A load balancer in front of r3v3rs3 can send the client address in a PROXY protocol header (version 1 or 2). TCP, TCP over TLS, HTTP and HTTPS ports can read this header. UDP and HTTP over QUIC ports cannot.

Enable "Receive PROXY Protocol" and list the IP addresses or CIDR blocks of the load balancers in "Trusted Load Balancers":

- A connection from a trusted address must start with a valid header. r3v3rs3 closes the connection when the header is invalid, when "Accepted Versions" does not include its version, or when the header does not arrive in "Header Timeout". The default timeout is 5 seconds.
- r3v3rs3 does not read a header from other addresses. The client address of such a connection is the peer address.
- A version 2 `LOCAL` header and a version 1 `UNKNOWN` header keep the peer address. Load balancers send them in health checks.

r3v3rs3 reads the header before the TLS handshake. The address from the header is the client address of the connection. IP filters, rate limits, the client IP hash, the `Forwarded` and `X-Forwarded-For` headers and the access log use it. The access log also writes the peer address in the `peer` field.

An ACME HTTP-01 challenge request that starts with a PROXY protocol header does not receive the challenge response. Use the DNS-01 challenge for a certificate that such a load balancer serves.

```toml
[my-port]
listen = "/ip4/0.0.0.0/tcp/443/https"
tls_termination = { server_names = ["example.com"] }
proxy_protocol = { trusted = ["10.0.0.0/8"], accept = "v2", timeout = "5s" }
```

`accept` is `any` (the default), `v1` or `v2`.

## Resetting a Port

Changing the port configuration does not affect existing connections. Old connections will continue to use the old configuration. To forcibly close existing connections, you can reset the port.

# Proxies

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

## UDP Sessions

A UDP proxy opens one session for each client address. The session has its own socket to the upstream server, so the upstream server sees a different source port for each client. r3v3rs3 sends the replies of the upstream server back to the client from the listening port.

A session closes when no packet passes in either direction for `session_idle_timeout` (default `60s`). It also closes when the upstream socket reports an error, or when the upstream servers or the idle timeout of the port change. The next packet of the client opens a new session. A port keeps at most 10,000 sessions. When the limit is reached, r3v3rs3 drops the packets of new clients.

## Upstream Timeouts

r3v3rs3 limits the time that it waits for an upstream server. A timeout is a duration such as `500ms`, `10s` or `1m`.

- `timeouts.connect` of an HTTP / HTTPS proxy and `connect_timeout` of a TCP / TCP over TLS proxy limit the DNS lookup, the TCP connection and the TLS handshake of a new upstream connection. The default is `10s`.
- `timeouts.request` of an HTTP / HTTPS proxy limits the time from the start of a request until the response headers arrive, including a new connection. The default is `60s`, and `0s` disables the limit. The response body, WebSocket and other upgraded connections have no limit.
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
- `random` selects a random server.
- `first` uses the first healthy server. The other servers are backups.
- `client_ip_hash` sends each client IP address to the same server while that server is healthy. An HTTP proxy uses the resolved client IP (see [Client IP](#client-ip)). When a server is unhealthy, drained or removed, only its own clients move to the other servers.

Each server has a `weight` from `0` to `65535` (default `1`). `round_robin` uses each server as often as its weight and spreads the turns of a server over the cycle, as the smooth weighted round robin of nginx does. For example, weights `3` and `1` send three of every four requests to the first server. `random` selects a server with a chance that is proportional to its weight. `client_ip_hash` gives each server a share of the client addresses that is proportional to its weight. `first` ignores the weight. A server with `weight = 0` gets no new traffic, also when every other server is unhealthy, so you can take a server out of service without removing it. Its open TCP connections and UDP sessions stay. At least one server of each proxy or HTTP route must have a weight above `0`.

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

- An HTTP proxy with `health_check.path` sends `GET` to the path from the root of each server. A 2xx or 3xx status passes the check. The path must start with `/`, and only an HTTP proxy uses it.
- An HTTP proxy without a path and a TCP proxy open a TCP connection to each server.
- A UDP proxy resolves the host name of each server.

`health_check.timeout` (default `5s`) limits each check. A server that fails the check is unhealthy until a check passes, also when `max_fails = 0`. A successful request does not end this state.

The health of the servers stays after a configuration reload while the servers, their weights, the policy and the health check settings do not change.

The status API (`GET /api/proxies/{id}/status`) lists the health of each upstream server in `upstreams`: the address, the `weight`, `healthy`, the consecutive `failures`, and `last_error`. The proxy list of the WebUI shows the number of healthy servers and refreshes the statuses every 10 seconds. The title of the number lists the unhealthy servers with their last errors.

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

`sticky` keeps each client of an HTTP proxy on one upstream server with a cookie. It is off by default.

- The first response to a client sets the cookie `sticky.name` (default `r3v3rs3_affinity`). Its value is the HMAC-SHA256 signature of the proxy, the route and the server URL, so a client cannot select a server with a forged value. A value that does not match a server of the route is ignored.
- A request with a valid cookie goes to its server while the server is healthy and its circuit is not open. A server with `weight = 0` keeps its sticky clients, so a drained server finishes the sessions that it has. Otherwise `load_balancing` selects the server, and the response sets a new cookie. A retry to another server also sets a new cookie.
- r3v3rs3 removes the cookie from the request, so the upstream server does not receive it.
- The cookie has the `Path` of the route, `HttpOnly` and `SameSite=Lax`. On HTTPS and HTTP/3 it also has `Secure`. `sticky.max_age` sets `Max-Age`. Without `max_age`, the cookie ends with the browser session.
- The name must be a cookie token of RFC 6265: visible ASCII characters without spaces and separators.
- r3v3rs3 creates a new signature key at each start. After a restart, the old cookies are invalid, and each client gets a server from `load_balancing` again.
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
2. The rightmost address in `X-Forwarded-For` that is not a trusted proxy or a known CDN edge.

r3v3rs3 then sends the resolved address to the upstream server in `X-Real-IP` and keeps the incoming `X-Forwarded-For` and `Forwarded` chains. For an untrusted peer, r3v3rs3 removes `Forwarded`, `X-Forwarded-For`, `X-Real-IP`, `CF-Connecting-IP`, `True-Client-IP`, `CloudFront-Viewer-Address`, `Fastly-Client-IP` and `Incap-Client-IP`, because the client can forge them.

The CDN IP ranges are compiled into the binary and downloaded again every day. The last downloaded list is saved to `cdn-ranges.json` in the configuration directory. If a download fails, r3v3rs3 keeps the last known list. The "Settings" section shows the list status and has a "Refresh Now" button.

Akamai does not publish its edge IP ranges. Add your Akamai Site Shield ranges to "Trusted Proxies" instead.

## IP Filter

You can allow or deny clients by IP address for each HTTP / HTTPS proxy. r3v3rs3 checks the client IP that the "Client IP" section resolves, so the filter also works behind a CDN or a trusted proxy.

- **Denied IP Addresses**: Clients in these IP addresses or CIDR blocks receive `403 Forbidden`.
- **Allowed IP Addresses**: When the list is not empty, only clients in these IP addresses or CIDR blocks can access the proxy. Other clients receive `403 Forbidden`.

A denied address takes precedence over an allowed address.

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

## Authentication

You can require authentication for each HTTP / HTTPS proxy in the "Authentication" section. A route can replace the proxy authentication with "Override Authentication for This Route". Select "None" in the override to allow every client on that route.

r3v3rs3 checks authentication after the IP filter, the rate limit and the HTTPS redirect. So a browser sends the credentials on the secure connection when "Automatically Redirect HTTP to HTTPS" is on.

### Basic Auth

Clients without a valid username and password receive `401 Unauthorized` with a `WWW-Authenticate: Basic realm="..."` header. The browser then shows a login dialog.

- **Realm**: The name that the browser shows in the login dialog. Empty uses `r3v3rs3`.
- **Users**: Usernames and passwords. A username must not contain a colon.

r3v3rs3 stores each password as an argon2 hash and never saves the plain text password. The admin API does not return the hash. It returns `password_set: true` for a user with a password. Leave the password field empty to keep the current password. r3v3rs3 removes the `Authorization` header before it sends the request to the upstream server.

Argon2 takes CPU time on purpose. r3v3rs3 verifies each credential once and keeps the result in memory until the configuration changes. Use a rate limit to limit password guessing.

In `proxies.toml`, you can write a `password` instead of a `password_hash`. r3v3rs3 replaces it with a hash at startup.

```toml
[my-proxy]
protocol = "http"
vhosts = ["example.com"]
auth = { type = "basic", realm = "Staff", users = [{ username = "alice", password_hash = "$argon2id$v=19$m=19456,t=2,p=1$..." }] }
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

| Header | Value |
|---|---|
| `X-Forwarded-Method` | The client request method. |
| `X-Forwarded-Proto` | `http` or `https`. |
| `X-Forwarded-Host` | The client request host. |
| `X-Forwarded-Uri` | The client request path and query. |
| `X-Forwarded-For` | The client IP address that the "Client IP" section resolves. |

- **2xx response**: r3v3rs3 sends the request to the upstream server. It copies the headers in "Copy Response Headers" from the auth response to the upstream request. It removes these headers from the client request first, so a client cannot send them itself.
- **Other responses**: r3v3rs3 sends the auth response (status, headers and a body up to 64 KiB) to the client. So a redirect to a login page works.
- **No response within the timeout, or a connection error**: The client receives `502 Bad Gateway`.

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

| Endpoint | Method | Action |
|---|---|---|
| `.r3v3rs3/auth/login` | `GET` | Shows the sign-in form. |
| `.r3v3rs3/auth/login` | `POST` | Checks the username, the password and the TOTP code, then sets the session cookie. |
| `.r3v3rs3/auth/logout` | `POST` | Ends the session and removes the session cookie. |

The TOTP code is required only for accounts with TOTP. The session cookie `r3v3rs3_session` has the `HttpOnly` and `SameSite=Lax` attributes, and the `Secure` attribute on HTTPS and HTTP/3. It has no `Domain` attribute, and r3v3rs3 accepts a session only on the host where the client signed in. r3v3rs3 removes the session cookie before it sends the request to the upstream server.

Only an account that sees the proxy can sign in. An account with a proxy list sees only the proxies of its list. r3v3rs3 checks the account on each request, so a session ends when the account is removed, when the account changes, or when the proxy leaves the proxy list of the account. A session that started before r3v3rs3 recorded the account of each session is not valid, and the client signs in again.

The `[admin]` settings in `config.toml` apply to these sessions too:

- `session_expiry`: The lifetime of a session. The minimum is 5 minutes.
- `max_login_attempts` and `login_attempts_reset`: The limit of failed sign-ins for each client IP address and username. A blocked client receives `429 Too Many Requests` until the reset time passes.

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

## Header Rules

You can change the headers of proxied requests and responses in the "Header Rules" section. A route can replace the proxy rules with "Override Header Rules for This Route".

Write one rule per line:

| Rule | Action |
|---|---|
| `set Name: value` | Replaces every value of the header. |
| `append Name: value` | Adds a value and keeps the existing values. |
| `remove Name` | Removes the header. |

r3v3rs3 skips empty lines and lines that start with `#`.

- **Request Headers**: The rules change the request that r3v3rs3 sends to the upstream server. They run after r3v3rs3 sets `Forwarded`, `X-Forwarded-*` and `Via`, so a rule can replace these headers.
- **Response Headers**: The rules change the upstream response before r3v3rs3 sends it to the client. They do not change the responses that r3v3rs3 creates itself, like error pages, redirects and sign-in pages.

Values can use these variables. Write `{{` and `}}` for a literal brace.

| Variable | Value |
|---|---|
| `{client_ip}` | The client IP address that the "Client IP" section resolves. |
| `{host}` | The requested host name. |
| `{scheme}` | `http` or `https`. |
| `{request_id}` | A random 32-character hex ID. The request and response rules of one request use the same ID. |
| `{route}` | The path of the matched route, e.g. `/api`. |
| `{client_cert_subject}` | The subject of the verified client certificate, e.g. `CN=client.example.com`. Empty without a client certificate. |
| `{client_cert_fingerprint}` | The SHA-256 fingerprint of the verified client certificate in hex. Empty without a client certificate. |

A client can send a header with the same name itself. Use `set`, not `append`, for the client certificate headers, so the rule replaces the value of the client.

Rules cannot change `Connection`, `Content-Length`, `Host`, `Keep-Alive`, `Proxy-Connection`, `TE`, `Trailer`, `Transfer-Encoding` and `Upgrade`, because these headers control the connection and the message framing.

```toml
[my-app]
protocol = "http"
vhosts = ["app.example.com"]
headers = { request = [{ action = "set", name = "X-Request-Id", value = "{request_id}" }, { action = "remove", name = "X-Debug" }], response = [{ action = "set", name = "X-Frame-Options", value = "DENY" }, { action = "remove", name = "Server" }] }
routes = [{ path = "/", servers = [{ url = "http://127.0.0.1:9000/" }] }]
```

## Compression

You can compress proxied responses in the "Compression" section. Select one or more encodings to enable compression. Select no encoding to disable compression.

| Encoding | `Content-Encoding` | Level |
|---|---|---|
| Brotli | `br` | Quality 4 |
| Zstandard | `zstd` | Level 3 |
| Gzip | `gzip` | Level 6 |

r3v3rs3 reads the `Accept-Encoding` header of the request and uses the accepted encoding with the highest `q` value. When the client accepts several encodings with the same `q` value, r3v3rs3 uses the order of `algorithms`. In the panel, the order in which you select the encodings sets this order.

r3v3rs3 compresses a response only when all of these conditions are true:

- The status is not `1xx`, `204 No Content`, `206 Partial Content` or `304 Not Modified`.
- The upstream server did not encode the response, and the response has no `Content-Range` header.
- `Cache-Control` does not contain `no-transform`.
- The media type of `Content-Type` is in `mime_types`. `text/*` matches every text type. r3v3rs3 never compresses `text/event-stream`, because compression holds server-sent events back.
- `Content-Length` is equal to or larger than `min_size`. r3v3rs3 compresses a streamed response without `Content-Length`.

When a response meets these conditions, r3v3rs3 adds `Accept-Encoding` to `Vary`. When r3v3rs3 compresses the response, it also removes `Content-Length` and `Accept-Ranges`, and changes a strong `ETag` to a weak `ETag`. The response header rules run before compression, so a rule can set `Cache-Control: no-transform` to stop compression.

| Setting | Default |
|---|---|
| `algorithms` | Empty. Compression is disabled. |
| `min_size` | `1024` bytes |
| `mime_types` | `text/*`, `application/javascript`, `application/json`, `application/manifest+json`, `application/wasm`, `application/xml`, `application/xhtml+xml`, `application/rss+xml`, `application/atom+xml`, `image/svg+xml`, `font/otf`, `font/ttf` |

```toml
[my-app]
protocol = "http"
vhosts = ["app.example.com"]
compression = { algorithms = ["br", "zstd", "gzip"], min_size = 1024, mime_types = ["text/*", "application/json"] }
routes = [{ path = "/", servers = [{ url = "http://127.0.0.1:9000/" }] }]
```

## Cache

You can store proxied responses in memory in the "Cache" section. A stored response goes to the client without a request to the upstream server. Every route of the proxy shares one cache, and each proxy has its own cache.

| Setting | Default | Description |
|---|---|---|
| `enabled` | `false` | Enables the cache. |
| `max_size` | `67108864` (64 MiB) | The memory limit of the stored responses in bytes. When the cache is full, r3v3rs3 removes the least used responses. |
| `max_entry_size` | `1048576` (1 MiB) | r3v3rs3 does not store a response with a larger body. |
| `default_ttl` | `0s` | The lifetime of a response without `Cache-Control: max-age`, `s-maxage` or `Expires`. With `0s`, r3v3rs3 stores such a response only when it has an `ETag` or `Last-Modified` header, and revalidates it on every request. |

r3v3rs3 uses the cache only for `GET` and `HEAD` requests without `Range`, `Upgrade` and `Cache-Control: no-store`. A `HEAD` request uses the stored `GET` response. When the client sends `Cache-Control: no-cache` or `Pragma: no-cache`, r3v3rs3 sends the request to the upstream server and stores the new response. The cache key is the requested host with the path and the query of the request, so the upstream server that load balancing selects does not change the key.

r3v3rs3 stores a response only when all of these conditions are true:

- The status is `200`, `203`, `204`, `300`, `301`, `308`, `404`, `405`, `410`, `414` or `501`.
- `Cache-Control` does not contain `no-store` or `private`.
- The response has no `Set-Cookie` header.
- The response has no `Vary: *` header.
- When the request has an `Authorization` header, `Cache-Control` contains `public`, `s-maxage` or `must-revalidate`.
- The response has a lifetime or a validator, and the body is not larger than `max_entry_size`.

The lifetime comes from `s-maxage`, then `max-age`, then `Expires`, then `default_ttl`. `Cache-Control: no-cache` sets the lifetime to zero. The age of a stored response includes the `Age` header of the upstream response.

When a stored response is stale and has an `ETag` or `Last-Modified` header, r3v3rs3 sends a conditional request with `If-None-Match` or `If-Modified-Since`. When the upstream server answers `304 Not Modified`, r3v3rs3 updates the stored headers and sends the stored response. r3v3rs3 keeps a stale response with a validator for one hour after it expires. A client with a matching `If-None-Match` or `If-Modified-Since` header receives `304 Not Modified` from the cache.

r3v3rs3 stores one response for each cache key. When `Vary` names request headers, r3v3rs3 sends the stored response only to a request with the same values of these headers.

r3v3rs3 removes `Accept-Encoding` from the requests that can use the cache, so the upstream server sends unencoded responses. The "Compression" section then compresses the response for each client. Each response to these requests has an `X-Cache` header: `HIT` for a stored response and `MISS` for a response from the upstream server. A response from the cache also has an `Age` header.

To remove every stored response of a proxy, click "Purge" in the proxy list, or send `DELETE /api/proxies/{id}/cache`. The stored responses stay in memory while the cache settings do not change. A restart or a change of the cache settings removes them.

```toml
[my-app]
protocol = "http"
vhosts = ["app.example.com"]
cache = { enabled = true, max_size = 67108864, max_entry_size = 1048576, default_ttl = "5m" }
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

To enable HTTP/3 proxying, bind a QUIC port in the Ports section and select HTTP over QUIC as the protocol. Note that HTTP/3 is only supported for incoming connections—upstream connections will be downgraded to HTTP/2 or HTTP/1.1.

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

# Certificates

## Server Certificates

For TCP over TLS and HTTPS proxy, r3v3rs3 requires a server certificate. There are three ways to install a server certificate:

1. Generate a self-signed certificate
2. Import a certificate from a file (PEM format only)
3. Use [ACME](https://letsencrypt.org/how-it-works/) to automatically provision a certificate

r3v3rs3 will automatically search for a certificate from SNI (Server Name Indication) in the TLS client hello message.

## Client Certificates

A TLS server can require a client certificate to authenticate the client. There are two ways to add a client certificate in the "Client Certs" tab:

1. Generate a self-signed certificate and select "Client Certificate" as the certificate type. r3v3rs3 adds the `clientAuth` extended key usage, and the selected CA certificate signs it.
2. Import a certificate chain and its private key from files (PEM format only). A client certificate needs a private key.

A proxy sends a client certificate to its upstream servers. See "Upstream Client Certificates".

## Root Certificates

If your upstream server uses certificates not trusted by the system, you will need to add them to the root certificate store. r3v3rs3 automatically trusts all certificates signed by the root certificate, in addition to the system's root certificates.

Also, if you generate a self-signed certificate, r3v3rs3 will automatically generate a CA certificate and add it to the root certificate store.

# ACME

r3v3rs3 supports automatic certificate provisioning using [ACME](https://letsencrypt.org/docs/client-options/) (Automatic Certificate Management Environment). ACME is supported by many certificate authorities, such as Let's Encrypt, ZeroSSL, and Google Trust Services.

An ACME entry holds one or more domain names. Enter them separated by commas in the "Domain Names" field, for example `example.com, *.example.com`. The certificate contains every domain name as a Subject Alternative Name. r3v3rs3 renews the certificate automatically before it expires. If an order fails, r3v3rs3 orders again after one hour.

## Challenges

The certificate authority checks that you control each domain name with a challenge. Select the challenge in the "Challenge" field.

| Challenge | How it works | Requirements |
|---|---|---|
| HTTP-01 | The certificate authority requests `http://<domain>/.well-known/acme-challenge/<token>`, and r3v3rs3 answers. | Every domain name resolves to r3v3rs3, and TCP port 80 is open and accessible from the internet. Wildcard domain names are not possible. |
| TLS-ALPN-01 | The certificate authority opens a TLS connection to port 443 of the domain with the ALPN protocol `acme-tls/1`, and r3v3rs3 answers with a challenge certificate. | Every domain name resolves to r3v3rs3, and TCP port 443 is open and accessible from the internet. Wildcard domain names are not possible. |
| DNS-01 | r3v3rs3 creates a TXT record `_acme-challenge.<domain>` through the API of your DNS provider. | A DNS provider from the table below and an API credential that can edit the zone. |

A wildcard domain name such as `*.example.com` needs DNS-01. r3v3rs3 rejects a wildcard domain name with HTTP-01 and TLS-ALPN-01.

During a TLS-ALPN-01 challenge, every TLS port and every HTTPS port answers a client that offers only `acme-tls/1` with the challenge certificate. Other clients get the certificate of the port. A TLS port does not connect to the upstream server for a challenge connection. If no TCP or HTTP port uses the port of the "TLS-ALPN Challenge Address" setting, r3v3rs3 listens on that address until the challenges end. An HTTP port without TLS on port 443 cannot answer the challenge.

## DNS-01

For each domain name, r3v3rs3 does these steps:

1. It finds the zone of the domain name with the provider API. The longest zone that contains the name is used.
2. It creates the TXT record `_acme-challenge.<domain>` with a TTL of 60 seconds. Linode gets 300 seconds, the lowest TTL that Linode accepts. Porkbun gets no TTL, so the record has the lowest TTL of the account. Gandi gets 300 seconds, the lowest TTL that Gandi accepts. deSEC gets 3600 seconds, because deSEC rejects a TTL below the minimum TTL of the domain. Gandi, deSEC, Azure DNS and Google Cloud DNS write the whole record set of a name, so r3v3rs3 adds its values to the TXT values that are already there and removes only its own values. For `*.example.com`, the record is `_acme-challenge.example.com`, the same name as for `example.com`, so the record set holds two values.
3. It asks DNS every 5 seconds until the TXT values are visible, for at most 5 minutes. The "DNS Challenge Resolver" setting selects the DNS server. Without this setting, r3v3rs3 uses the system resolver.
4. It tells the certificate authority that the challenges are ready and waits at most 3 minutes for the validation.
5. It deletes the TXT records. It also deletes them when the order fails.

A system resolver can return cached answers. If the propagation check fails often, set "DNS Challenge Resolver" to a public resolver such as `1.1.1.1:53` or to an authoritative name server of the zone.

| DNS provider | Credentials | Required permissions |
|---|---|---|
| Cloudflare | API Token | `Zone:Read` and `DNS:Edit` for the zone. |
| Route 53 | Access Key ID, Secret Access Key | `route53:ListHostedZones` and `route53:ChangeResourceRecordSets`. Private hosted zones are skipped. |
| Azure DNS | Tenant ID, Client ID, Client Secret, Subscription ID | A service principal with the DNS Zone Contributor role on the zones. r3v3rs3 lists the DNS zones of the subscription and takes the resource group from the zone ID. |
| Google Cloud DNS | Service Account Key (JSON), Project ID | The JSON key file of a service account with the DNS Administrator role (`roles/dns.admin`) in the project of the zones. Without a Project ID, r3v3rs3 uses the project of the key. Private zones are skipped. |
| deSEC | API Token | A token of the account. A token with a scoped policy must allow writes to the `_acme-challenge` TXT record sets. |
| DigitalOcean | API Token | A token that can read domains and create and delete domain records. |
| Gandi | API Token | A personal access token that can read the domains and change their LiveDNS records. |
| Hetzner Cloud | API Token | A Hetzner Cloud project token with read and write access. The zone must be in Hetzner Cloud DNS. |
| Linode | API Token | A personal access token with read and write access to Domains. |
| Vultr | API Key | The API key of the account. |
| Porkbun | API Key, Secret API Key | "API Access" is on for the domain in the Porkbun domain management. |
| OVHcloud | API Endpoint, Application Key, Application Secret, Consumer Key | A consumer key with the rights `GET /domain/zone`, `POST /domain/zone/*` and `DELETE /domain/zone/*`. The endpoint is `ovh-eu`, `ovh-ca`, `ovh-us`, `kimsufi-eu`, `kimsufi-ca`, `soyoustart-eu` or `soyoustart-ca`. r3v3rs3 refreshes the zone after it creates the records and after it deletes them. |
| Webhook | Webhook URL, Bearer Token | A service of your own that creates and deletes the TXT records. See "DNS webhook" below. |
| Exec | Program Path | A program on the r3v3rs3 host that creates and deletes the TXT records. See "DNS exec" below. |
| RFC 2136 | DNS Server, Zone, TSIG Key Name, TSIG Algorithm, TSIG Secret | A DNS server that accepts dynamic updates, for example BIND, Knot DNS or PowerDNS. See "RFC 2136" below. |

r3v3rs3 is tested against mock servers of these APIs and against the [Pebble](https://github.com/letsencrypt/pebble) test certificate authority. It is not tested against real provider accounts.

## DNS webhook

The webhook provider sends the TXT records to a service of your own instead of the API of a DNS hosting service. For each challenge name, r3v3rs3 sends a `POST` request with this JSON body to the webhook URL:

```json
{"action": "add", "fqdn": "_acme-challenge.example.com", "values": ["<TXT value>"]}
```

- `action` is `add` before the validation and `remove` after it. r3v3rs3 also sends `remove` when an `add` request fails.
- `fqdn` is the TXT record name without the trailing dot. `values` holds every TXT value of the name.
- The service must keep the other TXT values of the name.
- With a bearer token, the request has the header `Authorization: Bearer <token>`.
- A 2xx status is a success. r3v3rs3 waits at most 30 seconds for the response.

The webhook URL must use HTTPS. HTTP is allowed only for a loopback address, for example `http://127.0.0.1:8080/acme`.

## DNS exec

The exec provider runs a program on the r3v3rs3 host for each TXT value:

```
<program> add <fqdn> <value>
<program> remove <fqdn> <value>
```

- r3v3rs3 starts the program directly, without a shell. The program gets an empty environment and no standard input.
- `fqdn` is the TXT record name without the trailing dot.
- The program must keep the other TXT values of the name.
- Exit status 0 is a success. Any other exit status is a failure, and the error message holds the start of the standard error.
- When an `add` call fails, r3v3rs3 runs `remove` for the values of the name that it added and for the failed value.
- r3v3rs3 stops the program when the timeout expires, and the call fails.

The program must be in the `[acme_exec]` section of `config.toml`:

```toml
[acme_exec]
programs = ["/usr/local/bin/r3v3rs3-dns-hook"]
timeout = "30s"
```

- Only the file sets this section. The admin API and the WebUI cannot change it. Restart r3v3rs3 after you edit it.
- The program of the provider and each entry of `programs` must be absolute paths. r3v3rs3 resolves symbolic links and compares the resolved paths.
- r3v3rs3 checks the program when you add the ACME entry and before each run.
- `timeout` is the longest run time of one call. The default is `30s`.

## RFC 2136

The RFC 2136 provider sends dynamic updates to the primary DNS server of the zone. Every message carries a TSIG signature.

- **DNS Server** is the address of the primary server as `host:port`, for example `ns1.example.com:53`. r3v3rs3 sends the messages over TCP.
- **Zone** is the zone of the names, for example `example.com`. When it is empty, r3v3rs3 asks the server for the SOA record of each TXT name and uses the owner name of that record.
- **TSIG Key Name**, **TSIG Algorithm** and **TSIG Secret** must match the key on the server. The algorithm is `hmac-sha256`, `hmac-sha384` or `hmac-sha512`. The secret is base64.
- One update adds all TXT values of a name. After the validation, a second update deletes only these values, so the other TXT values of the name stay.
- r3v3rs3 verifies the TSIG signature of every response. The clocks of r3v3rs3 and the server can differ by at most 300 seconds.

A BIND example. `tsig-keygen -a hmac-sha256 r3v3rs3` prints the key block with a new secret:

```
key "r3v3rs3" {
    algorithm hmac-sha256;
    secret "<base64 secret>";
};

zone "example.com" {
    type primary;
    file "example.com.zone";
    update-policy {
        grant r3v3rs3 name _acme-challenge.example.com. TXT;
    };
};
```

Add a `grant` rule for each challenge name. A certificate for `*.example.com` uses the `_acme-challenge.example.com` name too.

## Stored data

r3v3rs3 stores ACME entries in `acme.toml` in the configuration directory. The file contains the private key of each ACME account and the DNS provider credentials in plain text. On Unix, r3v3rs3 creates and writes the file with mode `0600`, so only the owner of the process can read it. The admin API and the WebUI never return the credentials. The ACME list shows only the provider name, for example `Let's Encrypt (DNS-01, Cloudflare)`.

Create ACME entries in the WebUI, because r3v3rs3 creates the ACME account when you add an entry. An entry in `acme.toml` looks like this:

```toml
version = "0.3.40"

[acme1]
provider = "Let's Encrypt"
renewal_days = 60
identifiers = ["example.com", "*.example.com"]
challenge_type = "dns-01"

[acme1.dns_provider]
provider = "cloudflare"
api_token = "<Cloudflare API token>"

[acme1.account]
id = "https://acme-v02.api.letsencrypt.org/acme/acct/123456789"
key_pkcs8 = "<account private key>"
directory = "https://acme-v02.api.letsencrypt.org/directory"
```

The `provider` value of `dns_provider` is `cloudflare`, `route53`, `digitalocean`, `hetzner`, `linode`, `vultr`, `gandi`, `desec`, `porkbun`, `ovh`, `azure`, `google_cloud`, `webhook`, `exec` or `rfc2136`. Route 53 uses `access_key_id` and `secret_access_key` instead of `api_token`. Porkbun uses `api_key` and `secret_api_key`. OVHcloud uses `endpoint`, `application_key`, `application_secret` and `consumer_key`. Azure DNS uses `tenant_id`, `client_id`, `client_secret` and `subscription_id`. Google Cloud DNS uses `service_account_key` and an optional `project_id`. The webhook uses `url` and an optional `token`. Exec uses `program`. RFC 2136 uses `server`, an optional `zone`, `key_name`, `key_algorithm` and `key_secret`. The Vultr API key goes in `api_token`.

# Settings

The "Settings" section of the WebUI edits the server-wide options stored in `config.toml`. Changes take effect immediately and are saved to the file.

| Setting | Default | Description |
|---|---|---|
| Session Expiry | `1h` | Lifetime of an admin session. The minimum is 5 minutes. |
| Max Login Attempts | `10` | Failed logins allowed per client IP and username. |
| Login Attempts Reset | `15m` | Wait time after the limit is reached. |
| Background Task Interval | `1h` | Interval of certificate renewal and log cleanup tasks. |
| HTTP Challenge Address | `0.0.0.0:80` | Listening address for ACME HTTP challenges. |
| TLS-ALPN Challenge Address | `0.0.0.0:443` | Listening address for ACME TLS-ALPN-01 challenges when no port uses its port. |
| DNS Challenge Resolver | empty | DNS server, for example `1.1.1.1:53`, that r3v3rs3 asks until the TXT records of a DNS-01 challenge are visible. Empty uses the system resolver. |
| Database Log Retention | `3months` | How long logs are kept in the log database. |
| Audit Log Retention | `1year` | How long the audit log keeps an entry. See [Audit Log](#audit-log). |

Durations use a human-readable format, for example `30s`, `15m`, `1h`, or `7days`.

# Configuration Files

r3v3rs3 uses TOML files for storing its configuration. The location of these files varies according to the operating system:

- Linux: `$XDG_CONFIG_HOME/r3v3rs3` or `$HOME/.config/r3v3rs3`
- macOS: `$HOME/Library/Application Support/r3v3rs3`
- Windows: `%APPDATA%\r3v3rs3\config`

You can override the default location by setting the `R3V3RS3_CONFIG_DIR` environment variable or the `--config-dir` command-line option.

If needed, these files can be edited manually. Note, however, that r3v3rs3 does not automatically detect changes made to the configuration files. To ensure any changes take effect, you must restart the server after editing a configuration file.

A node of a cluster reads only `config.toml`. The rest of its state is in etcd or Consul. See [Cluster](@/cluster.md).

# WebUI

r3v3rs3 includes a built-in WebUI. By default, it is served on localhost:46492. However, you can customize the port using the `R3V3RS3_WEBUI` environment variable or the `--webui` command-line option. If you wish to disable the WebUI, set the `R3V3RS3_NO_WEBUI=1` environment variable or use the `--no-webui` command-line option.

The flag menu in the navbar selects the WebUI language: English or Turkish. The theme menu selects the System, Light or Dark theme. The WebUI stores the selections in the `r3v3rs3_lang` and `r3v3rs3_theme` cookies. Without these cookies, the WebUI uses English and the system theme.

The error pages of r3v3rs3 and the Admin Session sign-in page read these cookies too. A browser sends the cookies only to the host of the WebUI, so the pages of a proxy on another host use English and the system theme.

# Admin API

The WebUI uses the admin API under `/api`. r3v3rs3 generates the OpenAPI document of this API from the server code, so the document lists the routes of the running version.

- OpenAPI document: `http://localhost:46492/api/openapi.json`
- Swagger UI: `http://localhost:46492/api/docs/`

Both addresses require a session. Sign in to the WebUI, then open them in the same browser. The API link in the WebUI footer opens the Swagger UI.

A script signs in with `POST /api/login` and sends the `token` cookie of the response with the next requests:

```bash
$ curl -c cookies.txt -H 'Content-Type: application/json' \
    -d '{"username":"admin","method":"password","password":"passw0rd","insecure":true}' \
    http://localhost:46492/api/login
$ curl -b cookies.txt http://localhost:46492/api/ports
```

`"insecure": true` removes the `Secure` attribute from the cookie. Use it when the admin panel uses plain HTTP.

# Audit Log

r3v3rs3 records the changes that an account makes through the WebUI or the admin API: ports, proxies, certificates, ACME entries, settings, CDN IP range refreshes and accounts. It also records each sign-in, failed sign-in and sign-out of the admin panel. The changes that r3v3rs3 makes by itself, for example a certificate renewal or a discovered proxy, are not recorded.

Each entry holds the time, the account, the client IP address, the action, the id of the changed resource and a short summary. The summary holds names, addresses and roles. It never holds a password, a token or a key.

A single server keeps the audit log in the `audit_log` table of `log.db` in the log directory. A cluster keeps the audit log encrypted in the cluster store, so each node reads the entries of every node. An entry of a cluster also names the node that recorded it.

The "Audit Log Retention" setting sets how long an entry stays. The default is `1year`. A cluster deletes the entries of a day together, after that day passes the retention.

A failed write to the audit log does not undo the change. r3v3rs3 logs the error.

The Audit Log page of the WebUI lists the entries, starting with the newest. Only an admin opens the page. The page filters the entries by account, resource and period, and shows at most 500 entries.

`GET /api/audit` returns the same entries. Only an admin account can call it. Each query parameter is optional:

| Parameter | Description |
|---|---|
| `since` | The earliest time in Unix milliseconds. The default is 31 days before `until`. |
| `until` | The latest time in Unix milliseconds. The default is the current time. |
| `username` | The account of the entries. |
| `resource_id` | The id of the changed resource, or the changed username. |
| `limit` | The most entries in the response. The default is `100` and the maximum is `500`. |

A cluster reads at most 31 days before `until`, so a longer period does not return older entries.

# Logging

r3v3rs3 logs to the standard output as its default setting. You can change this behavior by setting the `R3V3RS3_LOG`, `R3V3RS3_ACCESS_LOG` environment variable or using the `--log`, `--access-log` command-line option.

```bash
$ r3v3rs3 start --log /var/log/r3v3rs3.log --access-log /var/log/r3v3rs3-access.log
```

If you want to adjust the log level, you can do so by setting the `R3V3RS3_LOG_LEVEL`, `R3V3RS3_ACCESS_LOG_LEVEL` environment variable or using the `--log-level`, `--access-log-level` command-line option.
