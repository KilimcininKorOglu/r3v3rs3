+++
title = "Configuration"
description = "Configuration"
weight = 0
+++

# Configuration & Storage

r3v3rs3 stores all server configuration, reverse proxy rules, ports, access lists, ACME settings, user accounts, and deployment platform data in a unified SQLite database, `r3v3rs3.db`, located in the configuration directory (default `/etc/r3v3rs3`).

- **Unified SQLite Database (`r3v3rs3.db`)**: Atomic transactions and WAL mode ensure high concurrency, immediate durability, and rock-solid reliability across restarts.
- **Master Encryption Key (`r3v3rs3.key`)**: Sensitive credentials (DNS API keys, Git tokens, environment secrets) are sealed with AES-256-GCM using `r3v3rs3.key` stored with strict `0600` file permissions.
- **Disk File Persistence**: Certificate PEM files remain on disk under `certs/<kind>/<id>/*.pem` (with relative paths tracked in `r3v3rs3.db`). External JSON datasets (`cdn-ranges.json`, `app-store.json`, `engines.json`) remain plain files on disk for easy catalog sync.
- **Log Isolation (`log.db`)**: High-throughput tracing and request logs live in a dedicated `log.db` database to prevent write contention and bloat in `r3v3rs3.db`.
- **Automatic Legacy Migration**: If legacy configuration files (`config.toml`, `ports.toml`, `proxies.toml`, `access_lists.toml`, `acme.toml`, `dns-credentials.toml`, `accounts.toml`, `notifications.json`, or `platform.db` / `platform.key`) exist in the configuration directory, r3v3rs3 automatically migrates their contents into `r3v3rs3.db` on startup and safely renames the original files to `*.migrated`, guaranteeing zero data loss.

The settings page of the panel manages configuration options while the server runs, applying updates immediately without requiring a process restart. The exception is `worker_mode`, which changes process clustering architecture: it can be configured in settings or CLI and takes effect on restart.

## Configuration Backup & Restore

r3v3rs3 can export its entire configuration (settings, ports, proxies, access lists, ACME configs, and storage files) to a single JSON backup file and restore it onto another server:

- **Secrets masked**: Sensitive secrets such as webhook tokens, proxy and access list password hashes, and ACME provider API keys are masked or redacted in the backup file so that backups can be stored or transmitted safely.
- **Accounts excluded**: User accounts are strictly excluded from the configuration backup and restore process. When restoring a backup on a server, existing admin credentials remain unchanged.

### WebUI

Open the **Backups** page (`/backups`) and select the **Configuration Backup** tab:
- Click **Export Backup** to download the `r3v3rs3-config-backup-<timestamp>.json` file.
- Click **Restore Backup** and select a valid backup JSON file. A confirmation dialog will summarize the action before applying the configuration.

### CLI

The `r3` CLI provides dedicated commands:

```bash
# Export configuration backup to a file
$ r3 config backup -o r3v3rs3-backup.json

# Export backup to stdout (e.g. for piping or remote backup scripts)
$ r3 config backup

# Restore configuration from a file
$ r3 config restore -f r3v3rs3-backup.json

# Restore configuration from standard input
$ cat r3v3rs3-backup.json | r3 config restore -f -
```

### REST API

- `GET /api/config/backup`: Returns the complete `ConfigBackup` object. Requires `admin` permission.
- `POST /api/config/restore`: Restores the configuration from a `ConfigBackup` JSON payload. Hot-reloads all ports, proxies, access lists, and settings immediately. Requires `admin` permission and records an entry in the audit log.

## Disaster Recovery & Encrypted Platform Snapshots (S3)

Unlike standard JSON configuration backups, **Disaster Recovery Snapshots** (`.r3bundle`) capture the full operational state of r3v3rs3 and its Coolify-like deployment platform in a single, AES-256-GCM encrypted bundle:

- **Complete State Included**: Contains `r3v3rs3.db` (unified SQLite database with all settings, ports, proxies, access lists, ACME certificates, user accounts, apps, deployments, databases, and environment variables), `r3v3rs3.key`, cluster sealing keys (`keys/`), TLS certificates and private keys (`certs/`), and legacy backup files if present.
- **AES-256-GCM Encryption**: Encrypted with an optional user-defined passphrase using 10,000 rounds of salted SHA-256 key derivation. Unencrypted data never leaves the server memory.
- **Automatic S3 Replication**: When remote S3 storage is configured in database backup settings, disaster recovery backups are automatically uploaded to `{prefix}/disaster-recovery/` in the remote bucket.
- **Single-Command Disaster Recovery**: Allows restoring the entire system onto a completely fresh server with a single CLI command.

### WebUI

Open the **Backups** page (`/backups`) and select the **Platform Disaster Recovery** tab:
- Enter an optional encryption **Passphrase** and check **Upload to remote S3 storage**.
- Click **Create Encrypted Snapshot** to create a point-in-time `.r3bundle`.
- Download, restore, or delete existing snapshots from the table using action icon buttons.
- Click **Upload & Restore .r3bundle** to upload an existing bundle file and restore the entire server state with confirmation.

### Remote Administration CLI

The `r3` CLI provides full remote management parity for disaster recovery:

```bash
# List all full disaster recovery snapshots
$ r3 backup dr list

# Create an encrypted snapshot and replicate to remote S3
$ r3 backup dr create --passphrase "my-secret-passphrase"

# Restore platform state from a snapshot ID
$ r3 backup dr restore <id> --passphrase "my-secret-passphrase"

# Download a snapshot (.r3bundle)
$ r3 backup dr download <id> -o backup.r3bundle

# Restore directly from a local bundle file
$ r3 backup dr restore-file -f backup.r3bundle --passphrase "my-secret-passphrase"
```

### CLI (Single-Command Restoration on a New Server)

On a new server after installing `r3v3rs3`:

```bash
# Restore entire platform state from an encrypted bundle
$ r3v3rs3 disaster-recovery restore --file backup.r3bundle --passphrase "my-secret-passphrase"

# Start the server with all apps, certs, and accounts intact
$ r3v3rs3 start
```

To create a disaster recovery backup via CLI:

```bash
# Create backup bundle with passphrase
$ r3v3rs3 disaster-recovery backup --output /var/backups/r3v3rs3.r3bundle --passphrase "my-secret-passphrase"
```

### REST API

- `GET /api/disaster-recovery/backups`: Lists all disaster recovery snapshots with sizes, timestamps, and S3 status.
- `POST /api/disaster-recovery/backups`: Creates a new encrypted snapshot with optional passphrase and S3 replication.
- `GET /api/disaster-recovery/backups/{id}/download`: Downloads the `.r3bundle` file.
- `DELETE /api/disaster-recovery/backups/{id}`: Deletes a disaster recovery backup.
- `POST /api/disaster-recovery/backups/{id}/restore`: Restores from a stored snapshot ID.
- `POST /api/disaster-recovery/restore-upload`: Restores from an uploaded `.r3bundle` payload with `X-Passphrase` header.

# Ports

A proxy needs a port. Add one in the "Ports" section.

r3v3rs3 supports six types of ports:

- HTTP
- HTTPS (HTTP over TLS)
- HTTP over QUIC (HTTP/3)
- TCP
- TCP over TLS
- UDP

The `listen` address of a port starts with `/ip4/<address>` or `/ip6/<address>`, followed by `/tcp/<port>` (TCP), `/tcp/<port>/tls` (TCP over TLS), `/tcp/<port>/http` (HTTP), `/tcp/<port>/https` (HTTPS), `/udp/<port>` (UDP) or `/udp/<port>/quic/http` (HTTP over QUIC). `name` is an optional name of the port. Turn off "Active" (`active = false`) to stop a port or a proxy without deleting it.

## Server Names

HTTPS, HTTP over QUIC and TCP over TLS ports have a "Server Names" field. r3v3rs3 selects the server certificate from the SNI (Server Name Indication) of the client. When the client sends no SNI, r3v3rs3 selects a valid server certificate that has all the server names. When the list is empty, r3v3rs3 selects the first valid server certificate.

```toml
[my-port]
listen = "/ip4/0.0.0.0/tcp/443/https"
tls_termination = { server_names = ["example.com", "*.example.com"] }
```

## TLS Client Authentication

HTTPS, HTTP over QUIC and TCP over TLS ports can verify the certificate of the client (mutual TLS). Select the mode in "Client Authentication":

| Mode     | Behavior                                                                                                                 |
|----------|--------------------------------------------------------------------------------------------------------------------------|
| Off      | The port does not ask for a client certificate. This is the default.                                                     |
| Optional | The port asks for a certificate but also accepts clients without one. A certificate that the client sends must be valid. |
| Required | The TLS handshake fails when the client sends no valid certificate.                                                      |

In Optional and Required mode, select one or more root certificates in "Client CA Certificates". A client certificate must be signed by one of them. The system root certificates are not used. r3v3rs3 rejects a port config without a root certificate, or with a certificate that is not a root certificate. A root certificate that a port uses cannot be deleted.

When the client authentication config becomes invalid, for example after the root certificate is removed from the configuration directory, the port closes every connection and the port list shows "TLS Error".

On HTTPS and HTTP over QUIC ports, header rules can send the verified certificate to the upstream server with the `{client_cert_subject}` and `{client_cert_fingerprint}` variables. See "Header Rules".

```toml
[my-port]
listen = "/ip4/0.0.0.0/tcp/443/https"
tls_termination = { server_names = ["example.com"], client_auth = "required", client_ca_certs = ["a1b2c3d"] }
```

### Route-Level Client Certificate Requirement

When an HTTPS or HTTP over QUIC port uses `Optional` client authentication, individual routes can enforce client certificate verification with `require_client_cert = true`. Requests matching a route with `require_client_cert = true` without a verified client certificate are immediately rejected with `403 Forbidden`. This enables hosting public routes (such as `/`, `/docs`) and mTLS-restricted routes (such as `/admin`, `/internal`) on the very same port:

```toml
[my-app]
protocol = "http"
vhosts = ["app.example.com"]
routes = [
  { path = "/public", servers = [{ url = "http://127.0.0.1:8080/public" }] },
  { path = "/admin", servers = [{ url = "http://127.0.0.1:8080/admin" }], require_client_cert = true },
]
```

### TLS 1.3 0-RTT Early Data & Anti-Replay Mitigation

For mobile and high-latency clients, r3v3rs3 supports TLS 1.3 0-RTT Early Data on TLS-terminated ports (`early_data = true`). When enabled, returning clients with a valid session resumption ticket can send application data in their initial ClientHello flight, reducing round-trip connection latency to zero:

- **RFC 8470 Safe Methods Only**: Per RFC 8470 Section 5.2, safe HTTP methods (`GET`, `HEAD`, `OPTIONS`, `TRACE`) are allowed in early data. State-changing requests (`POST`, `PUT`, `DELETE`, `PATCH`) sent in early data are rejected with HTTP `425 Too Early` and include a `Retry-After: 0` header, instructing clients to retry once the TLS handshake finishes.
- **In-Memory Anti-Replay Buffer**: Sliding-window anti-replay cache records 0-RTT request identifiers within a 60-second window. Any detected replay attempt is rejected with `425 Too Early`.
- **Upstream Forwarding**: Upstream requests arriving via 0-RTT carry an injected `Early-Data: 1` header. Any client-sent `Early-Data` header on standard connections is stripped to prevent spoofing.

```toml
[my-port]
listen = "/ip4/0.0.0.0/tcp/443/https"
tls_termination = { server_names = ["example.com"], early_data = true }
```

Via CLI or MCP:
```bash
r3 port set-early-data <PORT_ID> --enable
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

### DNS SRV Servers

A server URL with the `http+srv` or `https+srv` scheme takes its servers from the DNS SRV records of its host name. r3v3rs3 resolves the name, and the route gets one server per SRV target with the scheme before `+srv`, the target `host:port` and the path and query of the URL. Consul, Kubernetes headless services and other service registries publish such records. The URL has no port, because the SRV records give the ports.

```toml
[my-api]
protocol = "http"
routes = [{ path = "/", servers = [{ url = "http+srv://_http._tcp.api.service.consul/v1" }] }]
```

With the SRV records `_http._tcp.api.service.consul. 30 IN SRV 0 5 8080 api-1.node.consul.` and `... 0 1 8080 api-2.node.consul.`, the route sends the requests to `http://api-1.node.consul:8080/v1` and `http://api-2.node.consul:8080/v1` with the weights `5` and `1`.

- Only the records with the lowest priority value are used. The records with a higher value are backups in DNS, and r3v3rs3 does not use them.
- The SRV weight of a record is the `weight` of its server. A weight of `0` becomes `1`, because a server with weight `0` gets no traffic in r3v3rs3. A `weight` on the SRV server itself is not allowed.
- r3v3rs3 resolves the name again when the TTL of the answer expires, at the earliest after 5 seconds. When the targets change, the route gets the new servers without a restart. The health of the servers that stay is kept. A failed lookup keeps the last targets and is retried after 5 seconds.
- A route whose SRV name has no target answers with 502. The other servers of the route, if any, still get the requests.
- The "Upstream DNS Resolver" setting (see [Settings](#settings)) selects the DNS server of the SRV lookups, for example `127.0.0.1:8600` for the Consul DNS interface. Without the setting, r3v3rs3 uses the system resolver. The host names of the targets are resolved by the system resolver when r3v3rs3 connects.

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
- **Custom Theme Management**: Add new custom themes with unique IDs and custom designs, or edit existing templates (`+ New Theme` in WebUI, or `r3 error-page-theme add` / `r3 error-page-theme update`).
- **Proxy Theme Association**: Assign an error page theme to any proxy with `error_page_theme = "short_id"` via WebUI, CLI (`r3 proxy update <id> ...`), or REST API.
- **Granular Per-Proxy Overrides**: Any proxy-specific `error_pages` entry takes precedence over the assigned theme for that specific status code.
- **Reset to Defaults**: Built-in themes can be restored to default templates anytime via WebUI, CLI (`r3 error-page-theme reset-defaults`), or MCP (`reset_error_page_themes`).

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

`max_datagram_size` sets the maximum payload size in bytes for UDP packets (between 512 and 65527, default `65527`). Packets exceeding this size are truncated.

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
- **CLI**: `r3 proxy grpc-health <PROXY_ID> [--service <SERVICE>]`
- **MCP Tool**: `check_grpc_health` (parameters: `proxy_id` or `target_url`, `service`)
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
2. The rightmost address in `X-Forwarded-For` and the RFC 7239 `Forwarded` header that is not a trusted proxy or a known CDN edge. The `for=` elements of `Forwarded`, including quoted IPv6 addresses with a port, join the same chain.

r3v3rs3 then sends the resolved address to the upstream server in `X-Real-IP` and keeps the incoming `X-Forwarded-For` and `Forwarded` chains. For an untrusted peer, r3v3rs3 removes `Forwarded`, `X-Forwarded-For`, `X-Real-IP`, `CF-Connecting-IP`, `True-Client-IP`, `CloudFront-Viewer-Address`, `Fastly-Client-IP` and `Incap-Client-IP`, because the client can forge them.

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
    - `r3 security geoip status`: View loaded database status, build timestamp, size, and auto-update schedule.
    - `r3 security geoip update`: Download and hot-reload the latest GeoLite2-Country MMDB.
    - `r3 security geoip config`: Display GeoIP configuration.
    - `r3 security geoip set-config --license-key <KEY> --auto-update true --interval-days 7`: Update GeoIP settings.
  - **MCP Tools**: `get_geoip_status`, `update_geoip_database`, `get_geoip_config`, `update_geoip_config`.
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
- `POST /api/grpc/reflect`: Standalone service reflection targeting any reachable gRPC server (`{"target_url": "http://127.0.0.1:50051"}`).
- `POST /api/grpc/invoke`: Standalone RPC invocation with target URL, service, method, payload, and metadata.

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
- Users can sign out at any time by navigating to `/.r3v3rs3/auth/oidc/logout`.

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

Every account reads the lists. Only an admin or an editor without a proxy list changes them. See [Accounts](@/accounts.md).

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

```toml
[my-app]
protocol = "http"
vhosts = ["app.example.com"]
headers = { request = [{ action = "set", name = "X-Request-Id", value = "{request_id}" }, { action = "remove", name = "X-Debug" }], response = [{ action = "set", name = "X-Frame-Options", value = "DENY" }, { action = "remove", name = "Server" }] }
routes = [{ path = "/", servers = [{ url = "http://127.0.0.1:9000/" }] }]
```

### Security Header Hardening Profiles

r3v3rs3 includes built-in OWASP security header profiles and an automated scoring engine (A+ to F) to harden proxy responses with one click, via the WebUI, MCP, or the CLI:

- **Strict (`strict`)**: Full defense-in-depth profile for high-security applications (`max-age=63072000; includeSubDomains; preload`, `X-Content-Type-Options: nosniff`, `X-Frame-Options: DENY`, `Referrer-Policy: no-referrer`, restrictive `Permissions-Policy`, strict `Content-Security-Policy`, and isolation headers `Cross-Origin-Opener-Policy: same-origin` and `Cross-Origin-Resource-Policy: same-origin`).
- **Balanced (`balanced`)**: Recommended profile for typical CMS and multi-page web applications (`max-age=31536000; includeSubDomains`, `X-Frame-Options: SAMEORIGIN`, `Referrer-Policy: strict-origin-when-cross-origin`, permissive CSP supporting inline styles and common CDNs).
- **API Only (`api_only`)**: Optimized for REST, JSON, and gRPC endpoints (`X-Frame-Options: DENY`, `Cache-Control: no-store, max-age=0`, restrictive CSP, `Referrer-Policy: no-referrer`).
- **SPA Frontend (`spa_frontend`)**: Tailored for Vite, React, Vue, Svelte, and modern Single Page Applications with worker, blob, and WebSocket support.

#### CLI Commands

```bash
# List available security profiles
r3 proxy security-headers profiles

# Audit security headers of a proxy and inspect score and recommendations
r3 proxy security-headers audit <PROXY_ID>

# Apply an OWASP security hardening profile to a proxy
r3 proxy security-headers apply <PROXY_ID> strict
```

#### MCP Tools

AI agents can inspect and apply security profiles using `list_security_header_profiles`, `analyze_proxy_security_headers`, and `apply_security_header_profile`.

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

### Shared Compression Dictionaries (RFC 9651 / RFC 9652)

r3v3rs3 supports Compression Dictionary Transport (RFC 9651 Structured Fields and RFC 9652) for Chromium and compatible modern HTTP clients:
- **Serving Dictionaries**: Dictionaries configured on an HTTP proxy are directly served by r3v3rs3 at their configured `path` with `Content-Type: application/octet-stream`, `Use-As-Dictionary: match="<match_pattern>", id="<id>"`, `Cache-Control: public, max-age=31536000, immutable`, and `ETag: "<sha256>"`.
- **Content Negotiation & Encoding**: When an incoming request includes `Available-Dictionary` (or `Sec-Available-Dictionary`) matching a configured dictionary's SHA-256 hash and path, and the client accepts `dcz` or `dcb`, r3v3rs3 compresses the response against the dictionary. The response carries `Content-Encoding: dcz` (or `dcb`), `Content-Dictionary: :<sha256>:`, and `Vary: Available-Dictionary, Accept-Encoding`.
- **Bandwidth Reduction**: By eliminating static JSON schemas, recurring object keys, and repetitive HTML wrappers, shared dictionary compression delivers 80% to 95% bandwidth savings on recurring API traffic.
- **Management**: Compression dictionaries can be added, inspected, and removed via the WebUI Proxy Configuration page, MCP tools (`list_compression_dictionaries`, `get_compression_dictionary`, `add_compression_dictionary`, `delete_compression_dictionary`), or the CLI (`r3 proxy dictionary list/get/add/delete`).

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

The lifetime comes from `s-maxage`, then `max-age`, then `Expires`, then `default_ttl`. `Cache-Control: no-cache` sets the lifetime to zero. The age of a stored response includes the `Age` header of the upstream response.

When a stored response is stale and `stale_while_revalidate` (or upstream `stale-while-revalidate` header directive) has not elapsed, r3v3rs3 immediately responds with the stale cached content and sets `X-Cache: STALE`, while spawning a non-blocking background revalidation to refresh the cache. In-flight background revalidations are locked per cache key to prevent dogpiling.

When a stored response is stale and outside the `stale_while_revalidate` window, if it has an `ETag` or `Last-Modified` header, r3v3rs3 sends a conditional request with `If-None-Match` or `If-Modified-Since`. When the upstream server answers `304 Not Modified`, r3v3rs3 updates the stored headers and sends the stored response. r3v3rs3 keeps a stale response with a validator for one hour after it expires. A client with a matching `If-None-Match` or `If-Modified-Since` header receives `304 Not Modified` from the cache.

r3v3rs3 stores one response for each cache key. When `Vary` names request headers, r3v3rs3 sends the stored response only to a request with the same values of these headers.

r3v3rs3 removes `Accept-Encoding` from the requests that can use the cache, so the upstream server sends unencoded responses. The "Compression" section then compresses the response for each client. Each response to these requests has an `X-Cache` header: `HIT` for a stored response, `MISS` for a response from the upstream server, and `STALE` when served during background revalidation. A response from the cache also has an `Age` header.

To remove stored responses:
- Send `DELETE /api/proxies/{id}/cache` (or `DELETE /api/proxies/{id}/cache?path=/api/v1/` to purge only entries whose path starts with the given prefix).
- Send `POST /api/proxies/{id}/cache/purge` with optional JSON body `{"path": "/prefix"}`.
- Send an HTTP `PURGE` request directly to the proxy URL (e.g. `curl -X PURGE https://app.example.com/data.json`).
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

A TCP over TLS or HTTPS proxy needs a server certificate. There are three ways to install one:

1. Generate a self-signed certificate
2. Import a certificate from a file (PEM format only)
3. Use [ACME](https://letsencrypt.org/how-it-works/) to automatically provision a certificate

r3v3rs3 selects the certificate from the SNI (Server Name Indication) in the TLS client hello message. With **Wildcard SSL Auto-Sharing**, an existing wildcard certificate (such as `*.example.com`) automatically covers all matching subdomains across proxies and platform applications with zero manual certificate selection. The admin API also exposes `/api/certs/coverage` to report coverage and `/api/certs/verify_cname` to verify public DNS CNAME delegation for external custom domains.

## Client Certificates

A TLS server can require a client certificate to authenticate the client. There are two ways to add a client certificate in the "Client Certs" tab:

1. Generate a self-signed certificate and select "Client Certificate" as the certificate type. r3v3rs3 adds the `clientAuth` extended key usage, and the selected CA certificate signs it.
2. Import a certificate chain and its private key from files (PEM format only). A client certificate needs a private key.

A proxy sends a client certificate to its upstream servers. See "Upstream Client Certificates".

## Root Certificates

If your upstream server uses certificates that the system does not trust, add them to the root certificate store. r3v3rs3 trusts every certificate that the root certificate signs, in addition to the system root certificates.

A generated self-signed certificate also gets a CA certificate, and r3v3rs3 adds that CA certificate to the root certificate store.

## Expiry Warnings

The certificate list marks a certificate that expires within "Certificate Expiry Warning" (default `14days`) with "Expiring soon", and an expired certificate with "Expired".

## Notifications

r3v3rs3 sends a JSON `POST` request to the notification webhook for these events:

| Event                  | When                                                                                                  |
|------------------------|-------------------------------------------------------------------------------------------------------|
| `certificate_expiring` | A certificate expires within "Certificate Expiry Warning".                                            |
| `certificate_expired`  | A certificate has expired.                                                                            |
| `acme_order_failed`    | An ACME order failed. Each failed order sends an event.                                               |
| `test`                 | An admin sent a test notification.                                                                    |
| `deployment_started`   | A deployment of the [deployment platform](@/platform.md) started.                                     |
| `deployment_running`   | A deployment ended, and its container runs.                                                           |
| `deployment_failed`    | A deployment failed. A restart of r3v3rs3 also fails each unfinished deployment and sends this event. |
| `agent_online`         | An agent connected to the master. A new connection that replaces an open one sends no event.          |
| `agent_offline`        | The connection of an agent ended. A deleted or re-enrolled target sends no event.                     |
| `app_alert`            | An app exceeded its configured CPU or memory alert threshold.                                         |

```json
{"event": "certificate_expiring", "time": 1757894400, "node": "proxy-1", "certificate": {"id": "a1b2c3d", "san": ["example.com"], "not_after": 1759104000}}
{"event": "deployment_failed", "time": 1757894400, "deployment": {"id": "kfd-mzq", "app": "bxv-tpw", "app_name": "shop", "trigger": "webhook"}, "error": "the health check failed"}
{"event": "agent_offline", "time": 1757894400, "target": {"id": "fzn-txd", "name": "edge"}}
{"event": "app_alert", "time": 1757894400, "alert": {"app": "bxv-tpw", "app_name": "shop", "metric": "cpu", "current_value": "85.0%", "threshold": "80%"}}
```

- `time` is the Unix time in seconds. `certificate` names the certificate of a certificate event. `acme` holds the `id` and the `identifiers` of the ACME entry, and `error` describes the failure of an `acme_order_failed` event.
- `deployment` names the deployment, its app and its trigger (`manual`, `webhook`, `rollback` or `api`). `error` holds the message of a `deployment_failed` event. `target` names the agent target of an agent event. `alert` holds the app id, name, triggered metric (`cpu`, `memory_bytes` or `memory_percent`), current value and threshold of an `app_alert` event. Repeated alerts for the same app and metric are rate-limited with a 5-minute cooldown.
- With a token, the request has the header `Authorization: Bearer <token>`. The admin API does not return the token.
- A 2xx status is a success. r3v3rs3 sends a failed request at most three times: again after 1 second, and again 2 seconds later. "Webhook Timeout" (default `10s`) limits each try.
- r3v3rs3 checks the certificates at each "Background Task Interval". Each event of a certificate is sent once. A renewed certificate has a new fingerprint, so its events are sent again. r3v3rs3 keeps the sent events in `notifications.json` in the configuration directory.
- r3v3rs3 sends the notifications in the background. The queue holds at most 64 notifications. When the queue is full, r3v3rs3 drops the notification and logs an error.
- The webhook URL must use HTTPS. HTTP is allowed only for a loopback address.

Set the webhook in the "Notifications" section of the "Settings" page. "Send Test Notification" sends a `test` event to the webhook of the saved settings. `POST /api/config/notifications/test` does the same, and only an admin account can call it. The response is `400 notification_webhook_missing` without a webhook, and `502 notification_failed` when the webhook request fails.

```toml
[notifications]
cert_expiry_warning = "14days"
webhook = { url = "https://hooks.example.com/r3v3rs3", token = "<token>", timeout = "10s" }
```

`config.toml` holds the token in plain text.

## Deleting Several Certificates

Select the certificates with the checkboxes of the list, then click "Delete Selected". `POST /api/certs/delete` with the body `{"ids": [...]}` does the same for at most 200 ids. The response holds one result for each id, in the order of the request:

| Result      | Meaning                                                                     |
|-------------|-----------------------------------------------------------------------------|
| `deleted`   | r3v3rs3 deleted the certificate.                                            |
| `in_use`    | A port, a proxy or a discovery provider uses the certificate, so it stays.  |
| `read_only` | Service discovery manages the certificate, so it stays.                     |
| `not_found` | No certificate has the id.                                                  |
| `failed`    | The storage did not delete the certificate. The server log names the cause. |

An id that the request repeats gets one result.

# ACME

r3v3rs3 supports automatic certificate provisioning using [ACME](https://letsencrypt.org/docs/client-options/) (Automatic Certificate Management Environment). ACME is supported by many certificate authorities, such as Let's Encrypt, ZeroSSL, BuyPass, and Google Trust Services.

An ACME entry holds one or more domain names. Enter them separated by commas in the "Domain Names" field, for example `example.com, *.example.com`. The certificate contains every domain name as a Subject Alternative Name. r3v3rs3 renews the certificate automatically before it expires. If an order fails, r3v3rs3 orders again after one hour. The "Order again" action of an entry in the ACME tab, and `POST /api/acme/{id}/order`, order the certificates of the entry at once, without waiting for the renewal time or for the retry.

Select the certificate authority (Let's Encrypt, Google Trust Services, ZeroSSL, BuyPass, BuyPass Test, or "Custom" with its "Server URL") and enter an "Email Address". Certificate authorities that use External Account Binding (EAB), such as Google Trust Services or ZeroSSL, require the "EAB Key ID" and "EAB HMAC Key" of your account at the certificate authority. r3v3rs3 orders a new certificate "Renewal Interval (days)" (`renewal_days`, default `60`) days after the last one.

## Challenges

The certificate authority checks that you control each domain name with a challenge. Select the challenge in the "Challenge" field.

| Challenge   | How it works                                                                                                                                                      | Requirements                                                                                                                              |
|-------------|-------------------------------------------------------------------------------------------------------------------------------------------------------------------|-------------------------------------------------------------------------------------------------------------------------------------------|
| HTTP-01     | The certificate authority requests `http://<domain>/.well-known/acme-challenge/<token>`, and r3v3rs3 answers.                                                     | Every domain name resolves to r3v3rs3, and TCP port 80 is open and accessible from the internet. Wildcard domain names are not possible.  |
| TLS-ALPN-01 | The certificate authority opens a TLS connection to port 443 of the domain with the ALPN protocol `acme-tls/1`, and r3v3rs3 answers with a challenge certificate. | Every domain name resolves to r3v3rs3, and TCP port 443 is open and accessible from the internet. Wildcard domain names are not possible. |
| DNS-01      | r3v3rs3 creates a TXT record `_acme-challenge.<domain>` through the API of your DNS provider.                                                                     | A DNS provider from the table below and an API credential that can edit the zone.                                                         |

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

| DNS provider     | Credentials                                                     | Required permissions                                                                                                                                                                                                                                                                                   |
|------------------|-----------------------------------------------------------------|--------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------|
| Cloudflare       | API Token                                                       | `Zone:Read` and `DNS:Edit` for the zone.                                                                                                                                                                                                                                                               |
| Route 53         | Access Key ID, Secret Access Key                                | `route53:ListHostedZones` and `route53:ChangeResourceRecordSets`. Private hosted zones are skipped.                                                                                                                                                                                                    |
| Azure DNS        | Tenant ID, Client ID, Client Secret, Subscription ID            | A service principal with the DNS Zone Contributor role on the zones. r3v3rs3 lists the DNS zones of the subscription and takes the resource group from the zone ID.                                                                                                                                    |
| Google Cloud DNS | Service Account Key (JSON), Project ID                          | The JSON key file of a service account with the DNS Administrator role (`roles/dns.admin`) in the project of the zones. Without a Project ID, r3v3rs3 uses the project of the key. Private zones are skipped.                                                                                          |
| deSEC            | API Token                                                       | A token of the account. A token with a scoped policy must allow writes to the `_acme-challenge` TXT record sets.                                                                                                                                                                                       |
| DigitalOcean     | API Token                                                       | A token that can read domains and create and delete domain records.                                                                                                                                                                                                                                    |
| Gandi            | API Token                                                       | A personal access token that can read the domains and change their LiveDNS records.                                                                                                                                                                                                                    |
| Hetzner Cloud    | API Token                                                       | A Hetzner Cloud project token with read and write access. The zone must be in Hetzner Cloud DNS.                                                                                                                                                                                                       |
| Linode           | API Token                                                       | A personal access token with read and write access to Domains.                                                                                                                                                                                                                                         |
| Vultr            | API Key                                                         | The API key of the account.                                                                                                                                                                                                                                                                            |
| Porkbun          | API Key, Secret API Key                                         | "API Access" is on for the domain in the Porkbun domain management.                                                                                                                                                                                                                                    |
| OVHcloud         | API Endpoint, Application Key, Application Secret, Consumer Key | A consumer key with the rights `GET /domain/zone`, `POST /domain/zone/*` and `DELETE /domain/zone/*`. The endpoint is `ovh-eu`, `ovh-ca`, `ovh-us`, `kimsufi-eu`, `kimsufi-ca`, `soyoustart-eu` or `soyoustart-ca`. r3v3rs3 refreshes the zone after it creates the records and after it deletes them. |
| Webhook          | Webhook URL, Bearer Token                                       | A service of your own that creates and deletes the TXT records. See "DNS webhook" below.                                                                                                                                                                                                               |
| Exec             | Program Path                                                    | A program on the r3v3rs3 host that creates and deletes the TXT records. See "DNS exec" below.                                                                                                                                                                                                          |
| RFC 2136         | DNS Server, Zone, TSIG Key Name, TSIG Algorithm, TSIG Secret    | A DNS server that accepts dynamic updates, for example BIND, Knot DNS or PowerDNS. See "RFC 2136" below.                                                                                                                                                                                               |

r3v3rs3 is tested against mock servers of these APIs and against the [Pebble](https://github.com/letsencrypt/pebble) test certificate authority. It is not tested against real provider accounts.

### DNS-01 CNAME Delegation / Alias Mode (RFC 8555)

When issuing certificates for external clients or third-party domains, clients often cannot or should not provide their primary DNS provider API tokens to r3v3rs3. Under RFC 8555 DNS alias mode, external domains delegate their ACME DNS-01 validation to a DNS zone managed by r3v3rs3 through a static one-time CNAME record.

#### How it works

1. **Configure `dns_alias`:** When creating an ACME order with DNS-01, set `dns_alias` to your delegation base zone (e.g. `auth.example.com`) or explicit template (e.g. `_acme-challenge.{domain}.auth.example.com`).
2. **Client CNAME Setup:** The domain owner adds a single CNAME record pointing their challenge subdomain to the alias target:
   - **Source:** `_acme-challenge.client.com`
   - **Type:** `CNAME`
   - **Target:** `_acme-challenge.client.com.auth.example.com`
3. **Automated Issuance & Renewal:** During challenge execution, r3v3rs3 publishes the TXT validation record to `_acme-challenge.client.com.auth.example.com` using its own configured DNS provider (e.g. Cloudflare or Route 53). The ACME validation authority follows the CNAME from `client.com` to `auth.example.com` and issues the certificate.

Inspect CNAME instructions and verification status via CLI or MCP:
```bash
r3 acme cname-instructions <ACME_ID>
```

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

The program must be in the allowlist of the exec DNS provider. The settings page holds it in the **Exec DNS Provider** card while the server runs, and `config.toml` carries the same section:

```toml
[acme_exec]
programs = ["/usr/local/bin/r3v3rs3-dns-hook"]
timeout = "30s"
```

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

# Certificate Transparency (CT) Monitor

r3v3rs3 includes a background Certificate Transparency log monitor that inspects public CT logs for your domains to detect rogue, unauthorized, or compromised SSL certificates in real time.

## Background Monitoring & Detection

The monitor runs on a configurable schedule (default: every 6 hours) and automatically discovers domains from active server certificates and ACME configurations, or monitors custom domains configured by the administrator:

- **Auto-Discovery**: Identifies root domains and SANs from all loaded certificates and ACME orders.
- **Allowed Certificate Authorities**: Predefined trusted public CAs (Let's Encrypt, Google Trust Services, DigiCert, Sectigo, ZeroSSL, Cloudflare) or customized trusted issuer lists.
- **Rogue Detection**: Any certificate discovered in public CT logs issued by an unrecognized or unauthorized CA is immediately flagged as rogue (`is_rogue: true`) with status `investigating`.
- **Alerting & Webhooks**: Triggers the `cert.rogue_detected` webhook event with complete certificate metadata (issuer, serial number, common name, SANs, logged timestamp).

## CLI Management

Manage CT logs directly using the `r3 cert ct` command line tool:

```bash
# View monitor status and active alerts count
r3 cert ct status

# View current configuration
r3 cert ct config

# Update configuration
r3 cert ct set-config --enable true --domains "example.com,api.example.com" --interval 7200

# Trigger an immediate on-demand check
r3 cert ct check --domain example.com

# List discovered CT log records and rogue certificates
r3 cert ct list --rogue-only

# Dismiss an alert
r3 cert ct dismiss <ENTRY_ID>
```

## Model Context Protocol (MCP)

AI agents can inspect and manage CT logs autonomously via MCP tools:
- `get_ct_monitor_config`: Retrieve current monitoring configuration.
- `update_ct_monitor_config`: Update check intervals, domains, and allowed CAs.
- `get_ct_monitor_status`: Get real-time status, active alerts, and last check time.
- `list_ct_log_entries`: Query discovered records with filtering.
- `check_ct_logs_now`: Trigger an immediate log query.
- `dismiss_ct_alert`: Acknowledge or resolve an alert.

## Worker Processes

By default the same process that runs the admin API also serves every port. With `worker_mode = "workers"` in `config.toml`, r3v3rs3 starts one worker process that serves the ports while the master process keeps the storage, the admin API, the WebUI and the deployment platform. A crash of the worker does not take the admin API down, and the master restarts the worker with a backoff.

```toml
worker_mode = "workers"
```

The master sends the current configuration and the descriptors of the listening sockets to the worker over a private Unix socket, so the worker needs no access to the configuration directory. The status that the worker reports, its process id, its uptime and the number of its open connections, appears in the WebUI settings page and on `GET /api/workers`. A worker that stopped answering stays listed without a status.

## Zero-Downtime Upgrade

`kill -USR2 <pid>` hands the listeners to a new process. The old process starts the new binary, waits until the new process loads its state, passes the descriptors of the listening sockets over a Unix socket, and then drains its open connections up to `shutdown_graceful_timeout` (30 seconds by default) before it exits with status 0.

The new process reads the configuration from the same directory, so the upgrade takes effect for every setting that `config.toml` holds. With `worker_mode = "workers"` the workers keep the ports, so an upgrade hands over nothing and a rolling restart of the master happens without touching the open connections of the worker.

With systemd, `install.sh` writes the service as `Type=notify` with `ExecReload=/bin/kill -USR2 $MAINPID`, so `systemctl reload r3-server` upgrades without a restart. The server tells systemd when it is ready, and the handover moves the main process to the new one, so systemd keeps tracking the service while the old process drains.

The handover works only inside one installation: the old process starts the new binary from the same machine and passes the descriptors to it. A Docker upgrade replaces the whole container with a new image, so it cannot hand anything over and always restarts the connections. For a restart-free upgrade in Docker, run two r3v3rs3 containers behind a load balancer and roll them one at a time. A container restart still closes the open connections gracefully, up to `shutdown_graceful_timeout`.

## In-Panel Upgrade

An admin can upgrade without a shell: the "Settings" page carries a "Server Upgrade" card. Its button reads the latest GitHub release, picks the archive of this platform, verifies the sha256 digest that the release API reports, replaces the running binary, and hands the listeners to the new process over the zero-downtime upgrade above. The panel reloads onto the new version a few seconds later. While the upgrade runs, a dialog covers the panel and shows the step of the upgrade, which `GET /api/upgrade/status` reports. An upgrade that finds no newer release answers with an error, and the running server is not changed. The audit log records the upgrade.

The server also checks for a newer release on its own. The `upgrade.check_interval` setting of `config.toml`, which the "Settings" page edits, sets how often the server asks the release API (one hour by default, at least one minute). While a newer release exists, a banner on every page of the panel names it and links to the "Settings" page; the banner disappears when the server runs the newest release again.

## HTTP/2 and HTTP/3 Limits

Every server-side connection carries flood limits. The `[http2]` and `[http3]` sections of `config.toml`, which the "Settings" page edits, hold the defaults for every port:

| Setting                                  | Default | Range           | What it bounds                                                                           |
|------------------------------------------|---------|-----------------|------------------------------------------------------------------------------------------|
| `http2.max_header_list_size`             | 65536   | 1024 to 1048576 | The `HEADERS` plus CONTINUATION frames of one request, in bytes (CVE-2024-27316).        |
| `http2.max_pending_accept_reset_streams` | 128     | 1 to 65536      | The streams a client can reset before a `GOAWAY` closes the connection (CVE-2023-44487). |
| `http3.max_concurrent_bidi_streams`      | 100     | 1 to 100000     | The request streams one QUIC connection can open.                                        |
| `http3.max_concurrent_uni_streams`       | 10      | 1 to 100000     | The control streams one QUIC connection can open.                                        |

A request above the header limit gets its stream reset, and the port keeps serving new connections.

A port can override the defaults with its own `http2_limits` and `http3_limits` options, which the port form exposes for the HTTP and QUIC protocols. A section that stays off uses the limits of the server settings.

```toml
[http2]
max_header_list_size = 65536
max_pending_accept_reset_streams = 128

[http3]
max_concurrent_bidi_streams = 100
max_concurrent_uni_streams = 10
```

A change of the defaults rebuilds every port, so it takes effect without a restart. The QUIC endpoints that an upgrade adopts from another process keep the global defaults.

## Adaptive Stream Priority (RFC 9218)

r3v3rs3 implements the RFC 9218 Extensible Prioritization Scheme for HTTP to optimize multiplexed HTTP/2 and HTTP/3 connections. In high-concurrency environments, critical rendering resources (HTML documents, CSS, core JavaScript, and interactive APIs) are prioritized over bulk background downloads, images, or media streams.

### Priority Levels and Structured Fields

RFC 9218 signals priority using the `Priority` HTTP header with urgency (`u=0..7`) and an incremental flag (`i`):

| Urgency (`u`) | Class | Resource Types | Weight | Delivery Behavior |
|---|---|---|---|---|
| `0` | Critical | HTML entrypoints, critical security/checkout APIs | 256 | Delivered immediately with full bandwidth |
| `1` | High | CSS style bundles, critical JS, REST API | 128 | High concurrency, prioritized over media |
| `2` | Medium-High | Web fonts (`woff2`), critical fetches | 64 | Prioritized before visual media |
| `3` | Normal | Default HTTP requests, general API | 32 | Standard fair-share multiplexing |
| `4` | Medium-Low | Favicons, non-critical sub-resources | 16 | Delivered as capacity allows |
| `5` | Low | Images (`png`, `webp`, `avif`, `jpg`) | 8 | Incremental chunking (`i=?1`) |
| `6` | Background | Video and audio streaming (`mp4`, `webm`) | 4 | Bandwidth-paced streaming chunks |
| `7` | Lowest | Telemetry, archives, background downloads | 2 | Throttled under system congestion |

### Priority Configuration

Stream prioritization can be toggled on each HTTP proxy and overridden per port:

- **Inject Priority Header (`inject_header`)**: Forwards normalized `Priority: u=..., i` headers to upstream HTTP/2 & HTTP/3 servers so upstream schedulers can schedule multiplexed frames efficiently.
- **Adaptive Concurrency (`adaptive_concurrency`)**: Automatically tracks active streams and paces background streams (`u >= 5`) during high connection congestion to prevent bufferbloat.
- **Custom MIME and Path Rules (`custom_rules`)**: Custom glob patterns (e.g. `/api/critical/*`, `image/svg+xml`) that map specific endpoints to designated urgency levels.

CLI commands:
```sh
# View proxy stream priority configuration
r3 proxy priority <PROXY_ID>

# Enable adaptive priority with header injection
r3 proxy priority <PROXY_ID> --enable --inject-header --adaptive-concurrency

# View global stream priority and backpressure statistics
r3 traffic priority
```

MCP tools: `get_stream_priority`, `set_stream_priority`, `get_stream_priority_stats`.

### WebUI Management & Telemetry

Stream prioritization can be managed directly from the WebUI:
- **Proxy Configuration**: Under proxy advanced settings, enable *Stream Priority (RFC 9218)* and configure the *Inject Priority Header* and *Adaptive Concurrency* options.
- **Port Settings**: Enable *Stream Priority Optimization* on any HTTP/2 or HTTP/3 listening port.
- **Live Traffic Telemetry**: The `/traffic` page includes a dedicated *Stream Priority* tab displaying active multiplexed streams, priority classification breakdown (Critical, Normal, Background), adaptive backpressure pacing counters, and visual percentage distribution bars.

## Metrics

`GET /api/metrics` returns the counters of the running process in the Prometheus text format (`text/plain; version=0.0.4`):

- `r3v3rs3_http_requests_total`, `r3v3rs3_h3_requests_total`: the proxied requests across TCP/TLS and QUIC.
- `r3v3rs3_rx_bytes_total`, `r3v3rs3_tx_bytes_total`: total network throughput in bytes received and transmitted to clients.
- `r3v3rs3_http_responses_by_status_total`: HTTP responses partitioned by status class (`2xx`, `3xx`, `4xx`, `5xx`).
- `r3v3rs3_upstream_errors_total`: the requests that were answered without a successful upstream response.
- `r3v3rs3_upstream_errors_by_kind_total`: upstream errors partitioned by failure kind (`timeout`, `connect`, `circuit_open`, `no_route`, `other`).
- `r3v3rs3_http_request_duration_seconds`: request duration latency histogram with standard buckets (`0.005s` to `10s`), sum, and count.
- `r3v3rs3_http_request_duration_quantiles_seconds`: recent request duration quantiles (`p50`, `p95`, `p99`) in seconds.
- `r3v3rs3_tls_handshakes_total`, `r3v3rs3_tls_handshake_errors_total`: total completed and failed TLS handshakes.
- `r3v3rs3_tls_handshake_duration_seconds`: TLS handshake duration latency histogram with buckets (`0.001s` to `2.5s`), sum, and count.
- `r3v3rs3_tls_handshake_duration_quantiles_seconds`: recent TLS handshake duration quantiles (`p50`, `p95`, `p99`) in seconds.
- `r3v3rs3_cache_hits_total`, `r3v3rs3_cache_misses_total`, `r3v3rs3_cache_bypasses_total`: cache decisions (hits, misses, and uncacheable bypasses).
- `r3v3rs3_open_connections`: the client connection tasks that still run.
- `r3v3rs3_process_open_fds`, `r3v3rs3_process_max_fds`: open and maximum allowed file descriptors.
- `r3v3rs3_process_resident_memory_bytes`, `r3v3rs3_process_virtual_memory_bytes`: resident and virtual memory usage in bytes.
- `r3v3rs3_event_loop_lag_seconds`: current Tokio event loop latency in seconds.

Sending `Accept: application/json` returns the system self-monitoring metrics as a JSON object.

An admin session cookie or a valid MCP / API Bearer token authorizes the request, so a Prometheus job can scrape it with `bearer_token` while the WebUI uses its cookie. The counters count one process: with worker mode, the worker serves the ports and keeps its own counters, which `GET /api/workers` reports separately.

### Grafana Dashboard

A production-ready Grafana dashboard is bundled directly into r3v3rs3. You can download it directly from the endpoint `GET /api/metrics/grafana` or via the **Grafana Dashboard** button on the Live Traffic page in the WebUI.

The dashboard includes 5 visual sections:
1. **Key Performance Indicators**: Total request throughput rate, active connection count, p95 request latency, cache hit ratio, TLS handshake success rate, and Tokio runtime event loop lag.
2. **Traffic & Throughput**: Protocols comparison (HTTP/1.1 & HTTP/2 vs HTTP/3), response status classes distribution (2xx, 3xx, 4xx, 5xx), and RX/TX network bandwidth.
3. **Latency & TLS Performance**: Request latency quantiles (p50, p95, p99), TLS handshake duration quantiles, and TLS handshake volume vs failure rate.
4. **Upstream Reliability & Caching**: Upstream errors breakdown by failure kind (timeout, connect, circuit breaker, no route), and cache hit/miss/bypass rates.
5. **System Health & Resources**: Resident (RSS) and Virtual (VM) process memory consumption, open file descriptors vs limits, and event loop latency.

Example Prometheus scrape configuration:

```yaml
scrape_configs:
  - job_name: 'r3v3rs3'
    metrics_path: '/api/metrics'
    scheme: 'http'
    static_configs:
      - targets: ['127.0.0.1:46492']
    authorization:
      type: Bearer
      credentials: 'YOUR_API_TOKEN'
```

### Prometheus Anomaly Alerts & Notification Center

r3v3rs3 includes an autonomous real-time anomaly detection engine that monitors live Prometheus traffic and runtime metrics, triggers alerts, dispatches webhooks, and surfaces critical issues in the built-in **Notification Center**:

- **Real-Time Evaluation Loop**: Every 10 seconds, the server assesses recent request metrics and runtime health against configured thresholds:
  - **P99 Latency Anomaly (`max_p99_latency_ms`)**: Triggers when 99th percentile response latency exceeds the limit (default `1000ms`).
  - **5xx Error Rate Anomaly (`max_5xx_percent`)**: Triggers when the percentage of 5xx server errors in the recent evaluation window exceeds the threshold (default `5%`).
  - **Traffic Surge / Spike (`max_request_rate_rps`)**: Triggers when total incoming request throughput exceeds the configured requests-per-second limit.
  - **Event Loop Lag Anomaly (`max_event_loop_lag_ms`)**: Triggers when Tokio asynchronous runtime scheduler lag exceeds the threshold (default `150ms`).
- **WebUI Notification Center**:
  - The top navigation bar features an active notification bell icon displaying a live red badge with the count of active anomaly alerts.
  - Clicking the bell opens a flyout displaying current active anomalies, their severity (`CRITICAL` or `WARNING`), current metric values vs. thresholds, and a "Dismiss Alerts" button.
- **Webhook Dispatching & Storm Prevention**:
  - Detected anomalies dispatch webhook events (`system.high_latency`, `system.high_error_rate`, `system.traffic_spike`, `system.event_loop_lag`) to configured notification endpoints.
  - An automatic 5-minute cooldown per anomaly type prevents notification flood and alert storms during persistent incidents.
- **Configuring Thresholds in WebUI**:
  - Visit **Settings** and locate the **Anomaly Alert Thresholds** card to enable/disable automated anomaly alerts and customize detection limits.

#### Admin API

- `GET /api/system/anomalies`: Returns the current snapshot of active anomalies, recent anomaly history, live metrics, and active alert rules.
- `GET /api/system/anomalies/rules`: Returns current anomaly alert threshold rules.
- `PUT /api/system/anomalies/rules`: Updates anomaly alert threshold rules.
- `POST /api/system/anomalies/dismiss`: Dismisses all currently active anomaly alerts.

### Threat Detection & Auto-Quarantine

r3v3rs3 monitors incoming live HTTP and TCP traffic to detect scanner probes, path exploitation attempts, and repeated client error statuses without requiring external threat feed API latency:

- **Local Behavioral Threat Detection**:
  - Automatically identifies probe patterns such as `/.env`, `/.git`, `/.aws`, `/wp-admin`, `/wp-login.php`, `/xmlrpc.php`, `/phpmyadmin`, and `/config.json`.
  - Accumulates repeated client status violations (`404`, `403`, `401`) across a sliding time window (default: 60 seconds).
  - Instantly quarantines offending client IPs when violations reach the threshold (default: 5 violations).
- **TCP Drop Enforcement**:
  - Quarantined IP connections are immediately dropped at the TCP listener level before HTTP request parsing or TLS handshakes, consuming near-zero server CPU and memory.
- **Webhook Alerting**:
  - Automatically triggers `security.threat_detected` webhook notifications upon IP quarantine.
- **Management & WebUI Controls**:
  - View all quarantined IPs, violation counts, and remaining quarantine times in the **Security > Quarantined IPs** tab.
  - Manually quarantine or release any IP address with a single click.

#### Admin API

- `GET /api/security/quarantine`: Lists all active quarantined IP entries.
- `POST /api/security/quarantine`: Manually places an IP address into quarantine (`{"ip": "...", "reason": "...", "duration_seconds": 3600}`).
- `DELETE /api/security/quarantine/{ip}`: Releases an IP address from quarantine early.
- `GET /api/security/quarantine/config`: Retrieves current auto-quarantine configuration.
- `PUT /api/security/quarantine/config`: Updates and persists auto-quarantine configuration.

## MCP Server

The admin API exposes its complete tool suite over the Model Context Protocol at `/api/mcp` (Streamable HTTP, spec 2025-03-26). With **100% MCP Management Parity**, every single administrative feature, proxy route, port listener, ACME certificate, PaaS application, Docker Compose stack, one-click App Store catalog app, managed database, automated backup, agent target node, and disaster recovery snapshot is fully manageable autonomously via MCP without needing a human operator to open the WebUI.

### Autonomous MCP Tool Suite

| Category | Tools & Operations | Description |
|---|---|---|
| **System & Monitoring** | `server_status`, `get_config`, `list_audit_log`, `get_system_metrics`, `get_system_anomalies`, `list_banned_ips`, `unban_ip`, `list_quarantined_ips`, `quarantine_ip`, `unban_quarantined_ip`, `get_quarantine_config`, `update_quarantine_config`, `get_geoip_status`, `update_geoip_config`, `update_geoip_database`, `get_server_upgrade_status`, `run_server_upgrade` | Runtime version, worker processes, masked system configuration, tamper-evident audit trail, process open FDs/memory/lag, real-time anomaly alerts, fail2ban brute-force bans, threat auto-quarantine, automated GeoIP MMDB database management, and seamless live server self-upgrade execution. |
| **Ports** | `list_ports`, `get_port`, `get_port_status`, `add_port`, `update_port`, `delete_port`, `reset_port`, `list_network_interfaces` | Complete port listener lifecycle: bind addresses, TLS termination options, socket reset/reload, and host interface enumeration. |
| **Proxies** | `list_proxies`, `get_proxy`, `get_proxy_status`, `add_proxy`, `update_proxy`, `delete_proxy`, `purge_proxy_cache` | Reverse proxy configuration for HTTP, WebSocket, TCP, UDP, TLS, and HTTP/3, upstream health status, and path-selective cache purging. |
| **Certificates & ACME** | `list_certs`, `get_cert`, `check_cert_coverage`, `delete_cert`, `list_acme`, `get_acme`, `add_acme`, `delete_acme`, `request_acme`, `get_acme_cname_instructions`, `list_dns_credentials`, `get_ct_monitor_config`, `update_ct_monitor_config`, `get_ct_monitor_status`, `list_ct_log_entries`, `check_ct_logs_now`, `dismiss_ct_alert` | TLS certificate inspection, domain coverage verification, ACME automated issuance (HTTP-01, TLS-ALPN-01, DNS-01), RFC 8555 CNAME alias delegation, instant certificate orders/renewals, DNS credential management, and Certificate Transparency rogue detection. |
| **Access Lists** | `list_access_lists`, `add_access_list`, `update_access_list`, `delete_access_list` | Access control lists with CIDR IP filtering, HTTP basic authentication, and custom bearer tokens. |
| **PaaS Applications** | `list_apps`, `get_app`, `add_app`, `update_app`, `delete_app`, `deploy_app`, `get_app_logs`, `get_app_env`, `update_app_env`, `list_app_deployments`, `rollback_deployment`, `check_app_update`, `list_app_scheduled_commands`, `add_app_scheduled_command`, `update_app_scheduled_command`, `delete_app_scheduled_command`, `run_app_scheduled_command` | Autonomous PaaS container lifecycle: image/git/compose app deployment, container log streaming, environment variable injection, deployment rollbacks, scheduled cron commands management and immediate in-container execution, and Docker registry update detection. |
| **App Store Catalog** | `get_store`, `refresh_store`, `install_store_app` | Instant deployment of curated one-click templates (databases, dashboards, CMS, wikis, analytics) directly from the store into fully configured and routed applications. |
| **Managed Databases & Backups** | `list_databases`, `list_database_engines`, `create_database`, `list_database_backups`, `create_database_backup`, `delete_database_backup`, `get_backup_config`, `update_backup_config`, `test_database_backup`, `get_database_pitr_range`, `restore_database_pitr`, `archive_database_wal_segment`, `list_database_wal_segments` | Provisioning PostgreSQL, MySQL, Redis, MariaDB, and MongoDB with automated credentials, immediate backup creation, snapshot deletion, backup integrity verification, PITR point-in-time recovery with branching, WAL archive operations, and remote storage configuration. |
| **Agent Target Nodes** | `list_targets`, `add_target`, `delete_target`, `set_target_region`, `drain_target`, `undrain_target`, `generate_target_token`, `upgrade_target`, `upgrade_all_targets` | Multi-node Docker agent infrastructure, one-time enrollment token issuance, target decommissioning, geo-region assignment, traffic draining/undraining, and remote target binary upgrades. |
| **Projects & Env Groups** | `list_projects`, `create_project`, `delete_project`, `list_env_groups`, `create_env_group`, `delete_env_group`, `list_project_environments`, `create_project_environment`, `update_project_env`, `sync_project_env` | Project organization, multi-environment isolation, shared environment variable groups, and cross-environment configuration synchronization with selective key cloning. |
| **Outbound Webhooks** | `list_webhooks`, `get_webhook`, `add_webhook`, `update_webhook`, `delete_webhook`, `test_webhook` | Outbound event notification webhook dispatchers: endpoint management, HMAC-SHA256 signature secret validation, fine-grained event subscriptions, and end-to-end webhook delivery testing. |
| **Container Registries** | `list_registries`, `add_registry`, `update_registry`, `delete_registry` | Private Docker and OCI container registry credentials: credential storage, secure pull authentication, and server target endpoints. |
| **Git Connections** | `list_git_connections`, `get_git_connection`, `add_git_connection`, `delete_git_connection`, `list_git_repositories`, `list_git_branches` | Git provider connections (GitHub App, GitLab, Gitea): OAuth link management, accessible repository browsing, and remote branch enumeration. |
| **Accounts & Auth Tokens** | `list_accounts`, `create_account`, `update_account`, `delete_account`, `list_auth_tokens`, `create_auth_token`, `delete_auth_token` | User account administration: roles (admin, editor, viewer), proxy isolation scopes, password management, and persistent scoped API token issuance. |
| **Disaster Recovery** | `list_dr_backups`, `create_dr_backup` | Full system snapshot archives with optional AES-256-GCM encryption and automatic S3 replication. |

The MCP server is off by default. The settings page of the panel turns it on while the server runs and holds the clients that may sign in. `config.toml` carries the same section:

```toml
[mcp]
enabled = true

[[mcp.clients]]
id = "r3v3rs3-cli"
name = "r3v3rs3 MCP client"
redirect_uris = ["http://127.0.0.1:8765/callback"]
```

A client signs in through the OAuth 2.1 authorization code flow with PKCE: the client opens `/oauth/authorize` in the browser, the signed-in account approves the client on a consent form, and the client trades the one-time code for an access token at `/oauth/token`. The token carries the scopes of the approval: `read` sees the state, `edit` also changes proxies. The metadata documents live at `/.well-known/oauth-authorization-server` and `/.well-known/oauth-protected-resource`.

Tokens and dynamically registered clients are persisted in SQLite (`r3v3rs3.db`). A server restart or upgrade preserves the active grants, and clients seamlessly refresh their access tokens using their 30-day refresh tokens without prompting for re-authentication. Active sessions can be viewed and revoked at any time from the WebUI Settings page or via `GET /api/mcp/sessions` and `DELETE /api/mcp/sessions/{id}`.

How a client connects depends on the client. An AI host that speaks the MCP authorization flow, such as Claude Code or Cursor, needs no extra program: it adds this server as an HTTP MCP server, for example with `claude mcp add --transport http --scope user r3v3rs3 https://panel.example.com/api/mcp`, and its own `Authenticate` action of the `/mcp` command runs the same authorization flow in the browser. The command line client below serves the other cases: scripts, cron jobs and servers without a browser session, where one command signs in once and stores its tokens for every later call.

The repository ships a command line client in `r3` (`r3v3rs3-cli` crate). The installer script downloads the prebuilt binary of the newest release from GitHub, verifies its sha256 digest, and installs it:

```sh
# Linux and macOS:
curl -fsSL https://raw.githubusercontent.com/KilimcininKorOglu/r3v3rs3/main/install-cli.sh | sh
```

```powershell
# Windows (PowerShell):
irm https://raw.githubusercontent.com/KilimcininKorOglu/r3v3rs3/main/install-cli.ps1 | iex
```

On Windows the binary lands in `%LOCALAPPDATA%\Programs\r3`, which you add to your `PATH`. The script installs into `/usr/local/bin` on Linux and macOS, and `INSTALL_DIR` overrides that directory. A specific release installs with `--version vX.Y.Z`, and the archives of every platform are on the [releases page](https://github.com/KilimcininKorOglu/r3v3rs3/releases) as well.

Then:

```sh
r3 http://127.0.0.1:46492 login
r3 http://127.0.0.1:46492 platform status
```

`login` opens the consent page in the browser and stores the tokens in the config directory. A script can pass an access token directly with `--token` or the `R3V3RS3_TOKEN` environment variable. The `update` command installs a newer release over the running binary and verifies its sha256 digest against the checksums of the release.

### Full resource management

`r3` provides direct first-class commands to manage all aspects of the reverse proxy and PaaS platform:

```sh
# Ports
r3 <server> port list
r3 <server> port get <id>
r3 <server> port add <spec.json|yaml|->
r3 <server> port update <id> <spec.json|yaml|->
r3 <server> port delete <id>
r3 <server> port reset <id>

# Proxies
r3 <server> proxy list
r3 <server> proxy get <id>
r3 <server> proxy add <proxy.json|yaml|->
r3 <server> proxy update <id> <proxy.json|yaml|->
r3 <server> proxy status <id>
r3 <server> proxy purge-cache <id> [--path /static]
r3 <server> proxy delete <id>

# Certificates & ACME
r3 <server> cert list
r3 <server> cert get <id>
r3 <server> cert upload --cert server.crt --key server.key
r3 <server> cert coverage
r3 <server> cert delete <id>
r3 <server> acme list
r3 <server> acme get <id>
r3 <server> acme add <acme.json|yaml|->
r3 <server> acme update <id> <acme.json|yaml|->
r3 <server> acme order <id>
r3 <server> acme delete <id>

# Access Lists & DNS Providers
r3 <server> acl list
r3 <server> acl get <id>
r3 <server> acl add <acl.json|yaml|->
r3 <server> acl update <id> <acl.json|yaml|->
r3 <server> acl delete <id>
r3 <server> dns list
r3 <server> dns get <id>
r3 <server> dns add <dns.json|yaml|->
r3 <server> dns update <id> <dns.json|yaml|->
r3 <server> dns delete <id>

# Deployed Applications
r3 <server> app list
r3 <server> app get <id>
r3 <server> app add <app.json|yaml|->
r3 <server> app update <id> <app.json|yaml|->
r3 <server> app deploy <id>
r3 <server> app rollback <id>
r3 <server> app start <id>
r3 <server> app stop <id>
r3 <server> app restart <id>
r3 <server> app logs <id> [--tail 100]
r3 <server> app log-size <id>
r3 <server> app vacuum-logs <id>
r3 <server> app env <id>
r3 <server> app set-env <id> KEY=VAL [KEY2=VAL2...]
r3 <server> app delete <id>

# Managed Databases
r3 <server> db list
r3 <server> db get <id>
r3 <server> db create --name my-db --engine postgresql [--version 17] [--password secret]
r3 <server> db clone <id> --name my-db-clone
r3 <server> db replica <id> --name my-db-replica
r3 <server> db logs <id> [--tail 100]
r3 <server> db delete <id>

# Backups
r3 <server> backup config
r3 <server> backup list <database-id>
r3 <server> backup create <database-id>
r3 <server> backup restore <database-id> <backup-id>
r3 <server> backup delete <database-id> <backup-id>

# Multi-Node Agent Targets
r3 <server> target list
r3 <server> target get <id>
r3 <server> target add --name worker-1 --host 10.0.0.5 --port 46493
r3 <server> target drain <id>
r3 <server> target undrain <id>
r3 <server> target token
r3 <server> target delete <id>

# Projects & Environments
r3 <server> project list
r3 <server> project get <id>
r3 <server> project add --name my-project [--description "Core apps"]
r3 <server> project sync-env <project-id> --from staging --to production
r3 <server> project delete <id>

# App Store & One-Click Templates
r3 <server> store list
r3 <server> store refresh
r3 <server> store install <store-app-id> --name my-service [--domain service.example.com]

# Security, Banned & Quarantined IPs
r3 <server> security banned-ips
r3 <server> security unban <ip>
r3 <server> security quarantined-ips
r3 <server> security quarantine <ip> [--reason "Scanner probe"] [--duration-seconds 3600]
r3 <server> security unquarantine <ip>
r3 <server> security quarantine-config
r3 <server> security set-quarantine-config [--enabled true] [--max-violations 5] [--window-seconds 60] [--ban-duration-seconds 3600]

# Webhooks & Scheduled Cron Jobs
r3 <server> webhook list
r3 <server> webhook get <id>
r3 <server> webhook add <webhook.json|yaml|->
r3 <server> webhook update <id> <webhook.json|yaml|->
r3 <server> webhook test <id>
r3 <server> webhook delete <id>
r3 <server> cron list <app-id>
r3 <server> cron add <app-id> --cron "0 * * * *" --command "php artisan schedule:run" [--name hourly]
r3 <server> cron update <app-id> <job-id> [--cron "*/15 * * * *"] [--command "php artisan queue:work"] [--name queue]
r3 <server> cron run <app-id> <job-id>
r3 <server> cron delete <app-id> <job-id>

# Container Registries
r3 <server> registry list
r3 <server> registry add --server ghcr.io --username user --password token
r3 <server> registry update <id> [--server ghcr.io] [--username user] [--password token]
r3 <server> registry delete <id>

# Git Provider Connections & Repositories
r3 <server> git list
r3 <server> git get <id>
r3 <server> git add --name my-gitlab --provider gitlab --client-id <id> [--client-secret <secret>]
r3 <server> git repos <id> [--search query] [--page 1]
r3 <server> git branches <id> --repo owner/repo
r3 <server> git delete <id>

# User Accounts & Auth Tokens
r3 <server> account list
r3 <server> account create --username operator --password <pass> [--role editor]
r3 <server> account update <username> [--password <pass>] [--role admin]
r3 <server> account delete <username>
r3 <server> token list
r3 <server> token create --name ci-deploy [--scope edit]
r3 <server> token delete <id>

# Platform Status & Maintenance
r3 <server> platform status
r3 <server> platform metrics
r3 <server> platform prune
r3 <server> platform vacuum-logs [--threshold-mb 50]
r3 <server> platform rotate-key
```

### Declarative manifests (`apply -f`)

For CI/CD pipelines and automated configuration, `r3 apply -f <file>` applies a declarative YAML or JSON manifest to configure ports, proxies, and applications idempotently:

```sh
# Apply from a file
r3 http://127.0.0.1:46492 apply -f infrastructure.yaml

# Apply from stdin
cat infrastructure.yaml | r3 http://127.0.0.1:46492 apply -f -
```

An example manifest in YAML:

```yaml
ports:
  - id: http
    port:
      listen: /ip4/0.0.0.0/tcp/80
      active: true

proxies:
  - id: web
    name: "Web frontend"
    protocol: http
    ports: ["http"]
    routes:
      - host: "example.com"
        servers:
          - url: "http://127.0.0.1:3000"

apps:
  - spec:
      name: "my-app"
      domains: ["app.example.com"]
      target:
        type: image
        image: "nginx:alpine"
        port: 80
```

## Stored data

r3v3rs3 stores ACME entries in `acme.toml` in the configuration directory. The file contains the private key of each ACME account and the DNS provider credentials in plain text. On Unix, r3v3rs3 creates and writes the file with mode `0600`, so only the owner of the process can read it. The admin API and the WebUI never return the credentials. The ACME list shows only the provider name, for example `Let's Encrypt (DNS-01, Cloudflare)`.

Create ACME entries in the WebUI, because r3v3rs3 creates the ACME account when you add an entry. An entry in `acme.toml` looks like this:

```toml
version = "1.0.1"

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

## DNS Credentials

The "DNS Credentials" page below "Certificates" in the WebUI stores the credentials of the DNS providers under a name. A provider can hold several credentials, for example one Cloudflare token per account. The ACME form then picks a stored credential for the DNS-01 challenge instead of typing the provider credentials again, and every entry that uses the credential picks up a token change with it.

The admin API and the WebUI never return the stored credentials, and the list shows only the name and the provider of a credential. r3v3rs3 seals the credentials with the master key in `r3v3rs3.key` in the configuration directory and creates that key on the first credential when it does not exist yet. The credentials are encrypted and stored in `r3v3rs3.db`.

An ACME entry in `acme.toml` references a credential by its id and carries no provider of its own:

```toml
[acme1]
provider = "Let's Encrypt"
renewal_days = 60
identifiers = ["example.com", "*.example.com"]
challenge_type = "dns-01"
dns_credential = "<credential id>"
```

An entry names a `dns_provider` or a `dns_credential`, never both. Entries with an inline `dns_provider` keep working. The panel edits an entry that references a credential without showing the secrets; an edit that leaves the fields of the credential empty keeps the stored values.

A credential that an ACME entry still references cannot be deleted. Delete the reference in the entry first.

# Settings

The "Settings" section of the WebUI edits the server-wide options stored in the database (`r3v3rs3.db`). Changes take effect immediately and are persisted.

| Setting                    | Default       | Description                                                                                                                                                                                           |
|----------------------------|---------------|-------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------|
| Session Expiry             | `1h`          | Lifetime of an admin session. The minimum is 5 minutes.                                                                                                                                               |
| Display Time Zone          | `UTC`         | The IANA time zone that the panel renders the stored times in, for example Europe/Istanbul. The times stay stored as UTC.                                                                             |
| Max Login Attempts         | `10`          | Failed logins allowed per client IP and username.                                                                                                                                                     |
| Login Attempts Reset       | `15m`         | Wait time after the limit is reached.                                                                                                                                                                 |
| Background Task Interval   | `1h`          | Interval of certificate renewal and log cleanup tasks.                                                                                                                                                |
| HTTP Challenge Address     | `0.0.0.0:80`  | Listening address for ACME HTTP challenges.                                                                                                                                                           |
| TLS-ALPN Challenge Address | `0.0.0.0:443` | Listening address for ACME TLS-ALPN-01 challenges when no port uses its port.                                                                                                                         |
| DNS Challenge Resolver     | empty         | DNS server, for example `1.1.1.1:53`, that r3v3rs3 asks until the TXT records of a DNS-01 challenge are visible. Empty uses the system resolver.                                                      |
| Upstream DNS Resolver      | empty         | DNS server, for example `127.0.0.1:8600` for Consul, that answers the SRV lookups of `http+srv` and `https+srv` server URLs. Empty uses the system resolver. See [DNS SRV Servers](#dns-srv-servers). |
| Database Log Retention     | `3months`     | How long logs are kept in the log database.                                                                                                                                                           |
| Audit Log Retention        | `1year`       | How long the audit log keeps an entry. See [Audit Log](#audit-log).                                                                                                                                   |
| Certificate Expiry Warning | `14days`      | The certificate list marks a certificate that expires within this time, and the webhook gets a notification for it. See [Notifications](#notifications).                                              |
| Webhook URL                | empty         | The notification webhook. Empty sends no notification.                                                                                                                                                |
| Webhook Token              | empty         | The bearer token of the webhook requests. The admin API does not return it.                                                                                                                           |
| Webhook Timeout            | `10s`         | The longest time of one webhook request.                                                                                                                                                              |

Durations use a human-readable format, for example `30s`, `15m`, `1h`, or `7days`.

# Configuration Files

r3v3rs3 stores its configuration in TOML files in `$XDG_CONFIG_HOME/r3v3rs3` or `$HOME/.config/r3v3rs3`.

You can override the default location by setting the `R3V3RS3_CONFIG_DIR` environment variable or the `--config-dir` command-line option.

These files can be edited manually, but r3v3rs3 does not detect the change. Restart the server after editing a configuration file.

# WebUI

r3v3rs3 includes a built-in WebUI. By default, it is served on `127.0.0.1:46492`. Set another listening address, for example `0.0.0.0:46492`, with the `R3V3RS3_WEBUI` environment variable or the `--webui` command-line option. The `R3V3RS3_NO_WEBUI=1` environment variable or the `--no-webui` command-line option turns off the WebUI and the admin API.

The menu of the WebUI is a sidebar on the left with three groups:

- **Proxy**: **Ports**, **Proxies**, **Access Lists** and **Certificates**.
- **Platform**: **Apps**, **Targets** and **Git Providers**, the [deployment platform](@/platform.md). The group shows only while the platform is on, and only an account without a proxy list sees it; only an admin or an editor sees **Git Providers**.
- **Administration**: **Accounts**, **Audit Log** and **Settings**. Only an admin account sees this group.

The navbar holds the logo, the language menu, the theme menu and **Logout**. On a narrow screen the **Menu** button of the navbar replaces the sidebar and **Logout**, and opens the same menu with **Logout** at its end.

The flag menu in the navbar selects the WebUI language: English or Turkish. The theme menu selects the System, Light or Dark theme. The WebUI stores the selections in the `r3v3rs3_lang` and `r3v3rs3_theme` cookies. Without these cookies, the WebUI uses English and the system theme.

The error pages of r3v3rs3 and the Admin Session sign-in page read these cookies too. A browser sends the cookies only to the host of the WebUI, so the pages of a proxy on another host use English and the system theme.

### Action Icon Standard

Table and list rows throughout the WebUI (Proxies, Applications, Ports, Certificates, Targets, Accounts, and Projects) use compact, standardized icon buttons instead of text links. Every action button includes localized hover tooltips (`title`) and `aria-label` screen reader tags, following a unified semantic color scheme:

- **Edit / Settings**: Blue (`#2563eb`)
- **View / Inspect**: Neutral Slate (`#737373`)
- **Terminal / Shell**: Emerald Green (`#059669`)
- **Volume / Files**: Amber Folder (`#d97706`)
- **Metrics / Distribution**: Cyan Analytics (`#0891b2`)
- **Logs / Audit**: Indigo Document (`#4f46e5`)
- **Deploy / Play**: Emerald Play (`#059669`)
- **Pause / Drain**: Orange Pause (`#ea580c`)
- **Download / Export**: Neutral Download (`#525252`)
- **Restore / Re-test**: Teal Refresh (`#0d9488`)
- **Delete / Purge**: Red Trash (`#dc2626`)

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

For an account with TOTP, the first response is `"totp_required"`. Send the code in a second request with the cookie of the first one:

```bash
$ curl -b cookies.txt -c cookies.txt -H 'Content-Type: application/json' \
    -d '{"username":"admin","method":"totp","token":"123456","insecure":true}' \
    http://localhost:46492/api/login
```

# Audit Log

r3v3rs3 records the changes that an account makes through the WebUI or the admin API: ports (including resets), proxies (including cache purges), access lists, certificates, ACME entries, settings, CDN IP range refreshes and accounts. For the [deployment platform](@/platform.md) it records the apps, their environment variables, deployments and rollbacks, Git tokens and webhook secrets, the targets and agent enrollments, the Git provider connections, the reinstalled webhooks and the platform settings. A deployment that a signed Git provider webhook starts is also recorded. It also records each sign-in, failed sign-in and sign-out of the admin panel. The changes that r3v3rs3 makes by itself, for example a certificate renewal or a discovered proxy, are not recorded.

Each entry holds the time, the account, the client IP address, the action, the id of the changed resource and a short summary. The summary holds names, addresses and roles. It never holds a password, a token or a key.

A single server keeps the audit log in the `audit_log` table of `log.db` in the log directory.

The "Audit Log Retention" setting sets how long an entry stays. The default is `1year`.

A failed write to the audit log does not undo the change. r3v3rs3 logs the error.

The Audit Log page of the WebUI lists the entries, starting with the newest. Only an admin opens the page. The page filters the entries by keyword search, action, account, resource and period, and shows at most 500 entries. It provides an **Export CSV** button to download the filtered entries, as well as an in-page **Retention** selector to view and adjust how long entries are kept.

`GET /api/audit` returns the same entries. Only an admin account can call it. Each query parameter is optional:

| Parameter     | Description                                                                      |
|---------------|----------------------------------------------------------------------------------|
| `since`       | The earliest time in Unix milliseconds. The default is 31 days before `until`.   |
| `until`       | The latest time in Unix milliseconds. The default is the current time.           |
| `username`    | The account of the entries.                                                      |
| `resource_id` | The id of the changed resource, or the changed username.                         |
| `action`      | The action of the entries (for example `login`, `deploy_app`).                   |
| `search`      | Free-text search matching username, resource id, or summary.                     |
| `limit`       | The most entries in the response. The default is `100` and the maximum is `500`. |

# Logging

r3v3rs3 logs to the standard output as its default setting. You can change this behavior by setting the `R3V3RS3_LOG`, `R3V3RS3_ACCESS_LOG` environment variable or using the `--log`, `--access-log` command-line option.

```bash
$ r3v3rs3 start --log /var/log/r3v3rs3/r3v3rs3.log --access-log /var/log/r3v3rs3/access.log
```

A path with a directory writes into that directory, and a bare file name writes into the log directory. r3v3rs3 starts a new file every hour and adds the date and the hour to the name, for example `r3v3rs3.log.2026-09-25-10`.

The log directory holds these files and `log.db`, the database of the log page and the audit log. The `R3V3RS3_LOG_DIR` environment variable or the `--log-dir` command-line option sets it. The default is `$XDG_DATA_HOME/r3v3rs3/logs` or `$HOME/.local/share/r3v3rs3/logs`.

Set the log level with the `R3V3RS3_LOG_LEVEL`, `R3V3RS3_ACCESS_LOG_LEVEL` environment variable or the `--log-level`, `--access-log-level` command-line option. The default level is `info`, and `off` turns a log off. `R3V3RS3_LOG_FORMAT` or `--log-format` selects `text` (the default) or `json` for both logs.

# Live Access Log Streaming & Traffic Inspection

In addition to writing file logs, r3v3rs3 includes an in-memory real-time access log ring buffer and streaming engine. HTTP and WebSocket requests flowing through reverse proxies are captured with status codes, latencies, client IPs, hosts, paths, and methods.

## WebUI Traffic View

The panel provides an interactive live traffic dashboard accessible at `/traffic` and linked from proxy inspection pages:
- **Real-Time Stream**: Live Server-Sent Events (SSE) update the table in real time as requests arrive.
- **Color-Coded Status Inspection**: Visual badges for `2xx` (success), `3xx` (redirect), `4xx` (client error), and `5xx` (server error).
- **Latency Monitoring**: Color-coded response latencies (< 50ms green, 50–300ms amber, > 300ms red).
- **Interactive GeoIP World Map & Density Heatmap**: Incoming client IP addresses are resolved against the embedded MaxMind GeoIP database into ISO country codes. The map connects directly to the backend aggregated `GET /api/traffic/geo` endpoint, rendering historical and persistent geographic distribution with color-intensity heat styling. Hovering over a country displays its request count and percentage, and clicking any country or ranking bar instantly filters the live traffic log to requests from that country.
- **Top Countries Leaderboard**: Displays the top 10 countries ranked by traffic volume from backend distribution data with animated progress bars, request counts, and percentage shares.
- **Geographic & Text Filtering**: Filter incoming or historical logs by country, HTTP status code, URL path, client IP, host, or HTTP method.
- **Controls**: Pause/resume stream, toggle automatic pinning to latest entries, collapse/expand the Geo Map view, clear in-memory buffer, and export logs to JSON.
- **Live Request Inspector & Detail Drawer**: Clicking any row in the live traffic table opens an interactive side drawer with deep inspection tabs:
  - **Overview**: Core request metadata including client IP, GeoIP country, HTTP protocol, latency, timestamp, status, and matched proxy route.
  - **Headers**: Structured key-value table of both incoming client request headers and outgoing upstream response headers.
  - **Payload**: Inspect buffered request bodies (up to 16 KB) with a one-click toggle between syntax-formatted JSON and raw text view.
  - **Replay & cURL Console**: One-click **Copy as cURL** button generating fully escaped, production-accurate CLI commands, and a live **Replay Request** trigger to re-dispatch captured requests directly to upstream endpoints or custom URLs with live status and round-trip latency measurements.

## Admin API

The traffic endpoints are protected by session authentication and respect multi-tenant proxy scoping:

- `GET /api/traffic`: Returns recent access log entries matching optional query filters (`status`, `search`, `country`, `resource_id`, `limit`, `since_id`).
- `GET /api/traffic/live`: Streams live incoming access log entries as `text/event-stream` Server-Sent Events (SSE).
- `GET /api/traffic/geo`: Aggregates real-time geographic distribution of access traffic, returning the total count of resolved requests, top 10 ranked countries (`country`, `count`, `percentage`), and the full ISO country code count mapping.
- `GET /api/traffic/{id}`: Retrieves full inspection details for a specific access log entry by ID, including captured request and response headers, payload, content type, and WebSocket flags.
- `POST /api/traffic/replay`: Replays an HTTP or WebSocket handshake request against a specified target URL with custom headers, body, and timeout (clamped between 500ms and 30s), returning the status code, latency, error details, and response body.

## Live Connection Inspector & Stream Management Console

In addition to short-lived request logs, r3v3rs3 tracks active long-lived connections and streams in real time across reverse proxies:
- **Protocols Tracked**: WebSocket connections, HTTP Upgrades, Server-Sent Events (SSE), and raw TCP proxy sessions.
- **Real-Time Telemetry**: Tracks client address, target upstream, active uptime, bytes sent, bytes received, and instantaneous transfer rate (bytes per second).
- **Graceful Termination**: Terminate runaway or rogue connections immediately via WebUI, CLI, or MCP. WebSocket streams receive clean closure frames and TCP sockets are closed gracefully.

### WebUI

The Live Traffic dashboard at `/traffic` includes a dedicated **Active Connections** tab:
- Displays active streams with color-coded protocol badges (purple for WebSocket, blue for TCP, emerald for SSE, indigo for HTTP Upgrade).
- Live metrics table with connection ID, client address, upstream target, uptime, bytes transferred, and speed.
- Single-click connection termination via the red delete action button with SweetAlert2 confirmation dialog.

### CLI

Manage long-lived connections from the terminal with full parity:

```bash
# List all active long-lived connections
r3 traffic connections list

# Filter active connections by proxy ID
r3 traffic connections list --proxy prx_01j7xyz

# Terminate an active connection by ID
r3 traffic connections close con_01j7xyz
```

### Model Context Protocol (MCP)

AI agents can inspect and manage active connections autonomously via MCP tools:
- `list_active_connections`: Lists all active long-lived connections and streams with protocol, client IP, target, duration, transferred bytes, and throughput rate. Supports optional `proxy_id` filter.
- `terminate_active_connection`: Immediately terminates an active connection by its unique ID. Requires the `edit` permission.

### Admin API

- `GET /api/traffic/connections`: Returns the list of active long-lived connections (optional query parameter `proxy_id`).
- `DELETE /api/traffic/connections/{id}`: Terminates an active connection by its unique ID and creates an audit log record.
