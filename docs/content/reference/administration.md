+++
title = "Administration"
description = "Settings, the WebUI, the admin API, the audit log and logging"
weight = 11
+++

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
| Upstream DNS Resolver      | empty         | DNS server, for example `127.0.0.1:8600` for Consul, that answers the SRV lookups of `http+srv` and `https+srv` server URLs. Empty uses the system resolver. See [DNS SRV Servers](@/reference/proxies.md#dns-srv-servers). |
| Database Log Retention     | `3months`     | How long logs are kept in the log database.                                                                                                                                                           |
| Audit Log Retention        | `1year`       | How long the audit log keeps an entry. See [Audit Log](#audit-log).                                                                                                                                   |
| Certificate Expiry Warning | `14days`      | The certificate list marks a certificate that expires within this time, and the webhook gets a notification for it. See [Notifications](@/reference/certificates.md#notifications).                                              |
| Webhook URL                | empty         | The notification webhook. Empty sends no notification.                                                                                                                                                |
| Webhook Token              | empty         | The bearer token of the webhook requests. The admin API does not return it.                                                                                                                           |
| Webhook Timeout            | `10s`         | The longest time of one webhook request.                                                                                                                                                              |

Durations use a human-readable format, for example `30s`, `15m`, `1h`, or `7days`.


## WebUI

r3v3rs3 includes a built-in WebUI. By default, it is served on `127.0.0.1:46492`. Set another listening address, for example `0.0.0.0:46492`, with the `R3V3RS3_WEBUI` environment variable or the `--webui` command-line option. The `R3V3RS3_NO_WEBUI=1` environment variable or the `--no-webui` command-line option turns off the WebUI and the admin API.

A proxy of r3v3rs3 whose route points at the WebUI address, for example `http://127.0.0.1:46492`, serves the panel in process (see [Admin Session](@/reference/proxies.md#admin-session)). The `R3V3RS3_WEBUI_LISTEN=false` environment variable or the `--webui-listen=false` command-line option then closes the TCP listener: nothing answers on the WebUI address, so no other process of the host reaches the admin API around the protections of the proxy, and only that proxy serves the panel. Keep a way in before you turn the listener off, because a broken proxy then also locks you out of the panel. With `install.sh`, put the setting into a drop-in file, because the script writes the unit file again on every upgrade:

```sh
$ sudo mkdir -p /etc/systemd/system/r3-server.service.d
$ printf '[Service]\nEnvironment=R3V3RS3_WEBUI_LISTEN=false\n' | sudo tee /etc/systemd/system/r3-server.service.d/webui-listen.conf
$ sudo systemctl daemon-reload && sudo systemctl restart r3-server
```

A reload does not apply the setting, because the new process of a zero-downtime upgrade gets the environment of the old one. Remove the file and restart again to open the listener. The workers mode (`worker_mode = "workers"`) refuses to start without the listener, because its proxies run in other processes and reach the panel over TCP.

The menu of the WebUI is a sidebar on the left with three groups:

- **Proxy**: **Ports**, **Proxies**, **Access Lists** and **Certificates**.
- **Platform**: **Apps**, **Targets** and **Git Providers**, the [deployment platform](@/reference/platform.md). The group shows only while the platform is on, and only an account without a proxy list sees it; only an admin or an editor sees **Git Providers**.
- **Administration**: **Accounts**, **Audit Log** and **Settings**. Only an admin account sees this group.

The navbar holds the logo, the language menu, the theme menu and **Logout**. On a narrow screen the **Menu** button of the navbar replaces the sidebar and **Logout**, and opens the same menu with **Logout** at its end.

The flag menu in the navbar selects the WebUI language: English or Turkish. The theme menu selects the System, Light or Dark theme. The WebUI stores the selections in the `r3v3rs3_lang` and `r3v3rs3_theme` cookies. Without these cookies, the WebUI uses English and the system theme.

The error pages of r3v3rs3 and the Admin Session sign-in page read these cookies too. A browser sends the cookies only to the host of the WebUI, so the pages of a proxy on another host use English and the system theme.

#### Action Icon Standard

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

## Admin API

The WebUI uses the admin API under `/api`. r3v3rs3 generates the OpenAPI document of this API from the server code, so the document lists the routes of the running version.

- OpenAPI document: `http://localhost:46492/api/openapi.json`
- Swagger UI: `http://localhost:46492/api/docs/`

Both addresses require a session. Sign in to the WebUI, then open them in the same browser. The API link in the WebUI footer opens the Swagger UI.

A script signs in with `POST /api/login` and sends the `token` cookie of the response with the next requests:

```bash
$ curl -c cookies.txt -H 'Content-Type: application/json' \
    -d '{"username":"admin","method":"password","password":"passw0rd"}' \
    http://localhost:46492/api/login
$ curl -b cookies.txt http://localhost:46492/api/ports
```

The server automatically sets the `Secure` attribute on HTTPS connections and trusted reverse proxies, and leaves it off on local loopback HTTP connections.

For an account with TOTP, the first response is `"totp_required"`. Send the code in a second request with the cookie of the first one:

```bash
$ curl -b cookies.txt -c cookies.txt -H 'Content-Type: application/json' \
    -d '{"username":"admin","method":"totp","token":"123456"}' \
    http://localhost:46492/api/login
```

## Audit Log

r3v3rs3 records the changes that an account makes through the WebUI or the admin API: ports (including resets), proxies (including cache purges), access lists, certificates, ACME entries, settings, CDN IP range refreshes and accounts. For the [deployment platform](@/reference/platform.md) it records the apps, their environment variables, deployments and rollbacks, Git tokens and webhook secrets, the targets and agent enrollments, the Git provider connections, the reinstalled webhooks and the platform settings. A deployment that a signed Git provider webhook starts is also recorded. It also records each sign-in, failed sign-in and sign-out of the admin panel. The changes that r3v3rs3 makes by itself, for example a certificate renewal or a discovered proxy, are not recorded.

Each entry holds the time, the account, the client IP address, the action, the id of the changed resource and a short summary. The summary holds names, addresses and roles. It never holds a password, a token or a key.

The action of an entry is the operation id of the API operation that made the change, the same name as its MCP tool: `create_proxy`, `upload_app_file`, `run_prune`. Two actions have no API operation of their own: `login_failed` (a refused sign-in) and `enroll_agent` (an agent that enrolls itself). Entries that older releases wrote under earlier names, such as `add_proxy` or `backup_database`, move to the current names when the server opens `log.db`; an old name that covered several operations moves to the first of them.

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

## Logging

r3v3rs3 logs to the standard output as its default setting. You can change this behavior by setting the `R3V3RS3_LOG`, `R3V3RS3_ACCESS_LOG` environment variable or using the `--log`, `--access-log` command-line option.

```bash
$ r3v3rs3 start --log /var/log/r3v3rs3/r3v3rs3.log --access-log /var/log/r3v3rs3/access.log
```

A path with a directory writes into that directory, and a bare file name writes into the log directory. r3v3rs3 starts a new file every hour and adds the date and the hour to the name, for example `r3v3rs3.log.2026-09-25-10`.

The log directory holds these files and `log.db`, the database of the log page and the audit log. The `R3V3RS3_LOG_DIR` environment variable or the `--log-dir` command-line option sets it. The default is `$XDG_DATA_HOME/r3v3rs3/logs` or `$HOME/.local/share/r3v3rs3/logs`. Periodic log database cleanup runs asynchronously off the main server event loop, avoiding event loop starvation and connection latency spikes.

Set the log level with the `R3V3RS3_LOG_LEVEL`, `R3V3RS3_ACCESS_LOG_LEVEL` environment variable or the `--log-level`, `--access-log-level` command-line option. The default level is `info`, and `off` turns a log off. `R3V3RS3_LOG_FORMAT` or `--log-format` selects `text` (the default) or `json` for both logs.

## Live Access Log Streaming & Traffic Inspection

In addition to writing file logs, r3v3rs3 includes an in-memory real-time access log ring buffer and streaming engine. HTTP and WebSocket requests flowing through reverse proxies are captured with status codes, latencies, client IPs, hosts, paths, and methods.

### WebUI Traffic View

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
  - **Hidden secrets**: Every account with Read permission sees the access log, so r3v3rs3 stores `[REDACTED]` instead of a secret: the value of `Authorization`, `Proxy-Authorization`, `Cookie`, `Set-Cookie`, `X-API-Key`, `API-Key` and of every header whose name contains `password`, `secret`, `token`, `apikey`, `api_key` or `credential`, and the value of every field with such a name in the query string, a form body or a JSON body. A field name is percent-decoded before the check.
  - **Replay & cURL Console**: One-click **Copy as cURL** button generating fully escaped, production-accurate CLI commands, and a live **Replay Request** trigger to re-dispatch captured requests directly to upstream endpoints or custom URLs with live status and round-trip latency measurements.

### Admin API

The traffic endpoints are protected by session authentication and respect multi-tenant proxy scoping:

- `GET /api/traffic`: Returns recent access log entries matching optional query filters (`status`, `search`, `country`, `resource_id`, `limit`, `since_id`).
- `GET /api/traffic/live`: Streams live incoming access log entries as `text/event-stream` Server-Sent Events (SSE).
- `GET /api/traffic/geo`: Aggregates real-time geographic distribution of access traffic, returning the total count of resolved requests, top 10 ranked countries (`country`, `count`, `percentage`), and the full ISO country code count mapping.
- `GET /api/traffic/{id}`: Retrieves full inspection details for a specific access log entry by ID, including captured request and response headers, payload, content type, and WebSocket flags.
- `POST /api/traffic/replay`: Replays an HTTP or WebSocket handshake request against a specified target URL with custom headers, body, and timeout (clamped between 500ms and 30s), returning the status code, latency, error details, and response body.

### Live Connection Inspector & Stream Management Console

In addition to short-lived request logs, r3v3rs3 tracks active long-lived connections and streams in real time across reverse proxies:
- **Protocols Tracked**: WebSocket connections, HTTP Upgrades, Server-Sent Events (SSE), and raw TCP proxy sessions.
- **Real-Time Telemetry**: Tracks client address, target upstream, active uptime, bytes sent, bytes received, and instantaneous transfer rate (bytes per second).
- **Graceful Termination**: Terminate runaway or rogue connections immediately via WebUI, CLI, or MCP. WebSocket streams receive clean closure frames and TCP sockets are closed gracefully.

#### WebUI

The Live Traffic dashboard at `/traffic` includes a dedicated **Active Connections** tab:
- Displays active streams with color-coded protocol badges (purple for WebSocket, blue for TCP, emerald for SSE, indigo for HTTP Upgrade).
- Live metrics table with connection ID, client address, upstream target, uptime, bytes transferred, and speed.
- Single-click connection termination via the red delete action button with SweetAlert2 confirmation dialog.

#### CLI

Manage long-lived connections from the terminal with full parity:

```bash
# List all active long-lived connections
r3 traffic connections list

# Filter active connections by proxy ID
r3 traffic connections list --proxy prx_01j7xyz

# Terminate an active connection by ID
r3 traffic connections terminate con_01j7xyz
```

#### Model Context Protocol (MCP)

AI agents can inspect and manage active connections autonomously via MCP tools:
- `list_traffic_connections`: Lists all active long-lived connections and streams with protocol, client IP, target, duration, transferred bytes, and throughput rate. Supports optional `proxy_id` filter.
- `terminate_traffic_connection`: Immediately terminates an active connection by its unique ID. Requires the `edit` permission.

#### Admin API
