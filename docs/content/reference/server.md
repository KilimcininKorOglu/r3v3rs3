+++
title = "Server Operation"
description = "Worker processes, upgrades, HTTP limits, stream priority and metrics"
weight = 9
+++

## Worker Processes

By default the same process that runs the admin API also serves every port. With `worker_mode = "workers"` in `config.toml`, r3v3rs3 starts one worker process that serves the ports while the master process keeps the storage, the admin API, the WebUI and the deployment platform. A crash of the worker does not take the admin API down, and the master restarts the worker with a backoff.

```toml
worker_mode = "workers"
```

The master sends the current configuration and the descriptors of the listening sockets to the worker over a private Unix socket, so the worker needs no access to the configuration directory. The status that the worker reports, its process id, its uptime and the number of its open connections, appears in the WebUI settings page and on `GET /api/workers`. A worker that stopped answering stays listed without a status.

## Zero-Downtime Upgrade

`kill -USR2 <pid>` hands the listeners to a new process. The old process starts the new binary, waits until the new process loads its state, passes the descriptors of the listening sockets over a Unix socket, and then drains its open connections and coordinates active platform background tasks (database dumps, deployments, and scheduled commands) up to `shutdown_graceful_timeout` (30 seconds by default) before it exits with status 0. During shutdown, cancellation tokens notify schedulers and running deployments to checkpoint and clean up safely, preventing broken container states or corrupt backup files.

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

RFC 9218 signals priority using the `Priority` HTTP header with urgency (`u=0..7`) and an incremental flag (`i`). A valid `Priority` header of the client takes precedence over the rules below. As RFC 9218 requires, r3v3rs3 ignores a member with an out-of-range value or an unexpected type (for example `u=8` or `i=1`); a header without a valid member counts as absent.

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
- **Adaptive Concurrency (`adaptive_concurrency`)**: Limits each client to 32 concurrent background streams (`u >= 5`). A further background request of that client gets `429 Too Many Requests` with `Retry-After: 1`; other clients are not affected. Behind a trusted proxy the client is the address that the client IP settings resolve.
- **Custom MIME and Path Rules (`custom_rules`)**: Custom glob patterns (e.g. `/api/critical/*`, `image/svg+xml`) that map specific endpoints to designated urgency levels. A rule with an urgency outside 0 to 7 is refused with `invalid_priority_urgency`.

CLI commands:
```sh
# View proxy stream priority configuration
r3 proxy get-stream-priority <PROXY_ID>

# Enable adaptive priority with header injection
r3 proxy update-stream-priority <PROXY_ID> --enable true --inject-header true --adaptive-concurrency true

# View global stream priority and backpressure statistics
r3 traffic get-priority-stats
```

MCP tools: `get_proxy_stream_priority`, `update_proxy_stream_priority`, `get_traffic_priority_stats`.

### WebUI Management & Telemetry

Stream prioritization can be managed directly from the WebUI:
- **Proxy Configuration**: Under proxy advanced settings, enable *Stream Priority (RFC 9218)* and configure the *Inject Priority Header* and *Adaptive Concurrency* options.
- **Port Settings**: Enable *Stream Priority Optimization* on any HTTP/2 or HTTP/3 listening port.
- **Live Traffic Telemetry**: The `/traffic` page includes a dedicated *Stream Priority* tab displaying active multiplexed streams, priority classification breakdown (Critical, Normal, Background), adaptive backpressure pacing counters, and visual percentage distribution bars.

## Metrics

`GET /api/metrics` returns the counters of the running process in the Prometheus text format (`text/plain; version=0.0.4`):

- `r3v3rs3_http_requests_total`, `r3v3rs3_h3_requests_total`: the proxied requests across TCP/TLS and QUIC, labeled by proxy ID (`proxy="..."`) and route (`route="..."`).
- `r3v3rs3_rx_bytes_total`, `r3v3rs3_tx_bytes_total`: total network throughput in payload stream bytes received and transmitted to clients across all transfer encodings.
- `r3v3rs3_http_responses_by_status_total`: HTTP responses partitioned by status class (`2xx`, `3xx`, `4xx`, `5xx`).
- `r3v3rs3_upstream_errors_total`: the requests that were answered without a successful upstream response, labeled by proxy ID (`proxy="..."`) and route (`route="..."`).
- `r3v3rs3_upstream_errors_by_kind_total`: upstream errors partitioned by failure kind (`timeout`, `connect`, `circuit_open`, `no_route`, `other`).
- `r3v3rs3_http_request_duration_seconds`: request duration latency histogram labeled by proxy ID (`proxy="..."`) and route (`route="..."`) with standard buckets (`0.005s` to `10s`), sum, and count.
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

An admin session cookie or a valid MCP / API Bearer token authorizes the request, so a Prometheus job can scrape it with `bearer_token` while the WebUI uses its cookie. An account with a proxy list gets the series with a `proxy` label (`r3v3rs3_http_requests_total`, `r3v3rs3_upstream_errors_total`, `r3v3rs3_http_request_duration_seconds`) only for its own proxies; the series without a `proxy` label stay the totals of the process. The counters count one process: with worker mode, the worker serves the ports and keeps its own counters, which `GET /api/workers` reports separately.

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

### Liveness & Readiness Probes

r3v3rs3 provides unauthenticated health probe endpoints designed for container orchestrators (Kubernetes, Docker Swarm, Nomad), load balancers, and systemd watchdogs:

- `GET /healthz`: Liveness probe. Returns HTTP `200 OK` with `{"status":"ok"}` whenever the admin HTTP server is running. Used by orchestrators to know when to restart a stalled container.
- `GET /readyz`: Readiness probe. Verifies that the internal server event loop is responsive and that database storage is connected and operational. Returns HTTP `200 OK` with `{"status":"ready"}` when ready to receive traffic, or HTTP `503 Service Unavailable` with `{"status":"not_ready","message":"..."}` during startup, database failure, or internal deadlock.

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
  - Automatically identifies probe patterns such as `/.env`, `/.git`, `/.aws`, `/wp-admin`, `/wp-login.php`, `/xmlrpc.php`, `/phpmyadmin`, and `/config.json`. A pattern matches only the path and the query of a request, never its host, so a host such as `phpmyadmin.example.com` does not match `/phpmyadmin`. A pattern matches whole path segments only: it ends at the end of the path or before `/`, `.`, `?`, `&` or `#`. `/.git` matches `/.git/config` and `/wp-login` matches `/wp-login.php`, but `/.git` does not match `/.github`. Each matching request counts as one violation, like a blocked status code.
  - Accumulates repeated client status violations (`404`, `403`) across a sliding time window (default: 60 seconds). Only the responses that r3v3rs3 creates itself count: an unknown host, a refused IP filter, a failed sign-in or another route policy. A `403` or `404` from an upstream application never counts, so the login page of an app does not quarantine its users.
  - Instantly quarantines offending client IPs when violations reach the threshold (default: 5 violations).
- **Allowed Addresses**:
  - The addresses and CIDR networks in `allowed_ips` are never quarantined, for example the network of the administrators. Adding an address that is in quarantine lifts its quarantine at once, and a manual quarantine of an allowed address fails with `quarantine_allowed_ip`.
- **Drop Enforcement on Every Port**:
  - Quarantined IP connections are immediately dropped at the TCP listener level before HTTP request parsing or TLS handshakes, consuming near-zero server CPU and memory.
  - On a UDP port, the packets of a quarantined IP are dropped and its open session closes, so the upstream server neither gets them nor answers.
  - On a TCP port that reads PROXY protocol headers, the source address of the header is the client: a connection whose header names a quarantined IP closes before it reaches the upstream server.
- **Webhook Alerting**:
  - Automatically triggers `security.threat_detected` webhook notifications upon IP quarantine.
- **Management & WebUI Controls**:
  - View all quarantined IPs, violation counts, and remaining quarantine times in the **Security > Quarantined IPs** tab.
  - Manually quarantine or release any IP address with a single click.
  - On the server, `sudo r3-server unblock <ip> --config-dir /etc/r3v3rs3` releases an address from the quarantine and the sign-in ban without a panel session (see [Release a Blocked Address](@/tutorials/install-linux.md#release-a-blocked-address)).
  - Change the threshold, the window, the duration, the blocked status codes, the scanner paths and the allowed addresses in the **Auto-Quarantine Settings** card of the same tab.

```toml
[auto_quarantine]
enabled = true
max_violations = 5
window_seconds = 60
ban_duration_seconds = 3600
block_status_codes = [404, 403]
allowed_ips = ["198.51.100.0/24"]
```

#### Admin API

- `GET /api/security/quarantine`: Lists all active quarantined IP entries.
- `POST /api/security/quarantine`: Manually places an IP address into quarantine (`{"ip": "...", "reason": "...", "duration_seconds": 3600}`).
- `DELETE /api/security/quarantine/{ip}`: Releases an IP address from quarantine early.
- `GET /api/security/quarantine/config`: Retrieves current auto-quarantine configuration.
- `PUT /api/security/quarantine/config`: Updates and persists auto-quarantine configuration.
