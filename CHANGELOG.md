# Changelog

All notable changes to this project are documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.1.0/), and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [2.8.1] - 2026-10-28

### Added
- Stream Priority Sub-Toggles: Exposed nested `Inject Priority Header` (`inject_header`) and `Adaptive Concurrency` (`adaptive_concurrency`) controls in HTTP proxy configuration when RFC 9218 stream prioritization is enabled.
- Port-Level Stream Priority: Enabled stream priority toggle in port configuration for HTTP/2 and HTTP/3 ports, preserving options across edits.
- Live Traffic Stream Priority Telemetry: Added a dedicated `Stream Priority` tab in `/traffic` visualizing real-time RFC 9218 metrics, active in-flight multiplexed streams, urgency distribution (Critical, Normal, Background), and adaptive backpressure counters.
- Dual-Language Documentation: Updated `configuration.md` and `configuration.tr.md` documenting WebUI stream priority management and telemetry.

## [2.8.0] - 2026-10-28

### Added
- Target Region Assignment: Added geographical `Region` configuration input in the Add Agent Target form and an interactive `EditRegionModal` calling `PUT /api/targets/{id}/region` on both local and agent target rows to assign, update, or clear node regions for geo-proximity traffic steering.
- Platform Container Log Vacuum: Added an on-demand "Vacuum Logs" action button in the Disk Hygiene & Prune Policy card on the Targets page invoking `POST /api/platform/logs/vacuum` to immediately truncate active container logs across the entire platform.
- Dual-Language Documentation: Updated `platform.md` and `platform.tr.md` documenting target region management and platform-wide container log vacuum.

### Changed
- Target Actions Standardization: Standardized target row action buttons by assigning `ActionVariant::Edit` to Edit Region and `ActionVariant::Terminal` to New Token generation, maintaining unique and semantic iconography across all target actions.

## [2.7.9] - 2026-10-27

### Added
- Scheduled Command Edit Modal: Added a dedicated `EditCommandModal` in `app_commands.rs` calling `PUT /api/apps/{id}/commands/{cmd_id}` to modify task name, timing expression, shell command, and active state without recreating commands.
- Dual-Language Documentation: Synchronized `platform.md` and `platform.tr.md` documenting scheduled commands editing.

### Changed
- Action Button Standardization: Refactored scheduled commands table actions ("Output", "Run now", "Edit", "Delete") and outbound webhooks test trigger into standardized `ActionIconButton` components with localized tooltips and consistent iconography.

## [2.7.8] - 2026-10-27

### Added
- Interactive Error Page Themes Visual Editor Modal: Integrated `ErrorPagesEditor` inside a full-featured modal on `ErrorPageThemesCard`, enabling visual HTML and status-code editing directly from the Proxies Error Page Themes tab.
- Custom Error Page Theme Creation: Added "+ New Theme" modal dialog in the WebUI to create custom themes with custom IDs, names, descriptions, and status-code error templates.
- Row-Level Theme Editing: Added standardized Edit action icon buttons (`ActionVariant::Edit`) to error page theme table rows for quick theme modifications.
- Dual-Language Documentation: Synchronized `configuration.md` and `configuration.tr.md` documenting the visual editor modal and custom error page theme creation.

## [2.7.7] - 2026-10-27

### Changed
- Centralized Error Page Themes Relocation: Relocated `ErrorPageThemesCard` out of global Settings into the Proxies hub (`/proxies`) under a dedicated "Error Page Themes" tab with tabbed URL routing (`/proxies?tab=themes`), consolidating proxy-level error templates directly inside the proxy management interface.
- Streamlined Global Settings: Replaced the Error Page Themes configuration card in `/settings` with a direct shortcut banner pointing to the Proxies hub, further decluttering the settings view.
- Dual-Language Documentation: Synchronized `configuration.md` and `configuration.tr.md` in lockstep documenting the relocated Error Page Themes hub under Proxies.

## [2.7.6] - 2026-10-27

### Added
- Centralized Security Hub Console: Introduced a unified 4-tab Security Hub at `/security` (with backward-compatible `/access_lists` routing) consolidating Access Lists, Banned IPs, Quarantined IPs, and Anomaly Detection Rules into a dedicated operational interface.
- Standardized Security Action Controls: Converted Banned IPs and Quarantined IPs list controls to standardized `ActionIconButton` components with localized tooltips and aria-labels.

### Changed
- Streamlined Global Settings: Relocated Banned IPs, Quarantined IPs, and Anomaly Rules cards from `/settings` to the Centralized Security Hub, replacing them with a direct shortcut banner and keeping the global settings page focused on core server parameters.
- Sidebar Navigation: Renamed the sidebar navigation item from "Access Lists" to "Security" (`nav.security`) with shield-checkmark iconography reflecting the expanded multi-tab security capabilities.
- Dual-Language Documentation: Synchronized `configuration.md` and `configuration.tr.md` in lockstep documenting the centralized Security Hub and tabbed quarantine controls.

## [2.7.5] - 2026-10-27

### Changed
- Standardized Database Action Controls: Converted raw text links in the databases table (`databases.rs`) to standardized `ActionIconButton` components (`ActionVariant::View`, `Files`, `Terminal`, `Play`, `Restore`, `Edit`, `Delete`) with localized tooltips and aria-labels, achieving 100% visual parity across all platform lists.
- Eliminated Redundant S3 & Backup Settings: Removed redundant S3 drawer panels from `databases.rs` and the redundant global storage/schedule configuration form from `database_backups.rs`, replacing them with a direct redirect banner to the centralized Backups & Disaster Recovery center (`/backups`).
- Dual-Language Platform Documentation: Synchronized `platform.md` and `platform.tr.md` in lockstep documenting standardized database action controls and centralized storage management.

## [2.7.4] - 2026-10-27

### Changed
- Centralized Backups & Disaster Recovery Center: Relocated platform Disaster Recovery (`.r3bundle` encrypted full-platform backups) and Configuration Backup (`ConfigBackup` JSON export/import) from the global Settings page into a tabbed console on the Backups page (`/backups`), unifying database backups, S3 snapshots, system disaster recovery, and configuration management under one interface.
- Streamlined Global Settings: Cleaned up `/settings` by removing disaster recovery and configuration backup cards, refocusing the settings view strictly on core server parameters, runtime daemons, MCP sessions, GeoIP, CDN, and system upgrade.
- Standardized WebUI Action Controls: Converted Disaster Recovery table actions to standardized `ActionIconButton` components with localized tooltips and aria-labels for download, in-place restore, and deletion.

### Added
- Disaster Recovery Remote CLI Commands: Added `r3 backup dr` and `r3 dr` subcommands (`list`, `create`, `restore`, `delete`, `download`, `restore-file`) in `r3v3rs3-cli` providing 100% remote management parity for platform disaster recovery archives.
- Model Context Protocol (MCP) DR Parity: Implemented `restore_dr_backup` and `delete_dr_backup` MCP tools in Group 11, giving automated AI agents autonomous lifecycle control over platform disaster recovery snapshots.
- Dual-Language Documentation: Synchronized `platform.md`, `platform.tr.md`, `configuration.md`, and `configuration.tr.md` in lockstep documenting the centralized Backups center and CLI management workflows.

## [2.7.3] - 2026-10-27

### Changed
- WebUI Application Settings Clean-up: Purged non-configuration components (`AppMetrics`, `AppServices`, redundant terminal/file links) from the application settings page (`/apps/{id}/settings`), refocusing the settings view strictly on general configuration, Git credentials, auto-update policies, webhooks, scheduled commands, and environment variables.
- Consolidated Application Dashboard: Integrated historical metrics trend cards, sparklines, and container saturation diagnostics (`AppMetrics`) alongside full Compose architecture topology graphs and service controls (`AppServices`) directly into the dedicated application overview console (`/apps/{id}`).
- Dual-Language Documentation: Updated platform documentation (`platform.md` and `platform.tr.md`) in lockstep reflecting the enhanced application overview telemetry and streamlined settings page.

## [2.7.2] - 2026-10-27

### Fixed
- WebUI Icon and Static Asset Integrity: Completed comprehensive live crawl and disk validation across all WebUI routes (`/`, `/proxies`, `/ports`, `/certificates`, `/settings`, `/traffic`, `/audit`, `/apps`, `/databases`, `/backups`, `/targets`, `/git-connections`, `/projects`, `/store`, `/ct-logs`, `/apps/{id}`) ensuring zero missing or 404 vector assets and zero broken images.
- Automated Vector Asset Validation Test Suite: Added `every_icon_in_assets_is_valid` regression test verifying XML/SVG syntax integrity for all icons in `r3v3rs3-webui/assets/icons/`, and extended `every_referenced_asset_exists` to validate all root static assets (`favicon.ico`, `apple-touch-icon.png`, `mstile-*.png`, `safari-pinned-tab.svg`, `browserconfig.xml`, vendor bundles) referenced in `index.html`.

## [2.7.1] - 2026-10-27

### Fixed
- Missing WebUI SVG Icons: Added missing vector icon assets in `r3v3rs3-webui/assets/icons/` (`refresh.svg`, `reload.svg`, `terminal.svg`, `description.svg`, `play.svg`, `pause.svg`, `trash.svg`, `eye.svg`), resolving 404 Not Found asset errors on CT logs, application dashboard, and control actions.
- Automated Asset Presence Verification: Added automated regression test `every_referenced_asset_exists` in `r3v3rs3-webui` verifying that all SVG icons referenced across WebUI sources exist on disk before compilation.

## [2.7.0] - 2026-10-25

### Fixed
- WebUI Application Dashboard Hook Lifecycle: Converted polling effect runner in `r3v3rs3-webui/src/pages/app_dashboard.rs` into a properly annotated `#[hook]` function (`use_dashboard_poll`), ensuring Yew correctly registers the effect context and initiates live telemetry polling.
- Application Dashboard Loading Skeleton: Rendered a responsive placeholder skeleton with Back button navigation while application overview data is loading, preventing empty white view containers.

## [2.6.9] - 2026-10-25

### Fixed
- WebUI Login Error Response Handling: Corrected `send_login` in `r3v3rs3-webui/src/pages/login.rs` to inspect `!response.ok()` before deserializing. Properly parse server error payloads (`ErrorMessage`) on non-200 responses instead of attempting to deserialize them as `LoginResponse`, resolving misleading `unknown variant 'message', expected 'success' or 'totp_required'` client error.

## [2.6.8] - 2026-10-25

### Added
- Dedicated Application Dashboard & Unified Overview Console: Full-featured application management console at `/apps/{id}` providing single-pane-of-glass operational visibility and control.
- Application Navigation Tabs: Standardized tabbed navigation header across application views (Overview, Deployments, Logs, Terminal, Files, Settings) with live status badge and quick domain links.
- Real-time Application Telemetry & Health Monitoring: Live CPU utilization progress bar, memory usage versus configured limits, network I/O throughput (RX/TX), application uptime duration, and proactive CFS throttle / OOM-kill warning alerts with automated recommendations.
- Active Deployment Context Card: Highlighting active commit SHA, trigger source, author, commit message, target cluster node, and started timestamp with one-click quick actions for Deploy, Restart, Rollback, Terminal, and Live Logs.
- Multi-Service Container Topology: Live status, container names, states, and health check indications for Compose multi-service applications directly within the application dashboard.
- Live Log Preview & Log Sizing: Inline preview of recent container standard output/error with total on-disk log volume tracking and quick navigation to full logs.
- REST API Endpoint: Added `GET /api/apps/{id}/dashboard` returning comprehensive aggregated `AppDashboardSummary`.
- 100% MCP & CLI Parity: Added `get_app_dashboard` Model Context Protocol tool to `admin/mcp.rs` and `r3 app dashboard <APP_ID>` CLI command to `r3v3rs3-cli`.
- Dual-Language Documentation: Updated `platform.md` and `platform.tr.md` in lockstep documenting dedicated application dashboard capabilities, live telemetry, and CLI/MCP administration.

### Changed
- Documentation Service Alignment: Updated all documentation files and tutorials to reference the unified `r3-server` service name and `r3` CLI command syntax.

## [2.6.7] - 2026-10-25

### Added
- Automated Build Cache GC & Container Layer Retention Policy: Configurable BuildKit build cache retention limit (`build_cache_keep_storage_mb`) passing `keep-storage` parameter to Docker Engine during prune runs to retain hot compilation cache layers while discarding stale intermediates.
- Application Rollback Protection & Image Tag Retention: Configurable retained build count per application (`kept_builds_count`, default: 5) protecting historical image versions across zero-downtime rollbacks during disk hygiene runs.
- Dangling Layer Cleanup Filter: Added `dangling_layers_only` toggle restricting image cleanup to untagged intermediate layer artifacts (`<none>:<none>`) while preserving active tagged releases.
- 100% MCP & CLI Parity: Added MCP tools `get_prune_policy` and `update_prune_policy` to `admin/mcp.rs`, and CLI subcommands `r3 platform prune-policy` and `r3 platform set-prune-policy` with retention controls. Corrected `r3 platform prune` endpoint to `/api/platform/prune/run`.
- WebUI Disk Hygiene Controls: Added Build Cache Retention Limit, Kept Builds Per Application, and Dangling Layers Only inputs to the Disk Hygiene card on the Targets page with comprehensive localization.
- Dual-Language Documentation: Updated `platform.md` and `platform.tr.md` in lockstep documenting BuildKit keep-storage, rollback retention, and autonomous CLI/MCP administration.

### Fixed
- Fixed startup race condition in `stream_priority_test.rs` by adding `wait_for_listener`.

## [2.6.6] - 2026-10-25

### Added
- Certificate Transparency (CT) Log Monitor & Rogue Certificate Detection Engine: Background monitoring daemon periodically querying public CT logs (crt.sh and custom log endpoints) to track newly issued SSL/TLS certificates for managed domains.
- Rogue & Unauthorized Certificate Detection: Automated verification against trusted CA issuer lists (`allowed_issuers`), immediately detecting rogue, spoofed, or unexpected certificates and generating real-time security alerts.
- Automatic Domain Discovery: Dynamic discovery of active domain names and SANs from local server certificates and ACME orders alongside user-defined custom domains.
- Webhook Security Alerts: Integration with platform notification subsystem dispatching `cert.rogue_detected` and `cert.ct_check_failed` webhook events for immediate operational response.
- REST API Endpoints: Added `/api/certs/ct/config`, `/api/certs/ct/status`, `/api/certs/ct/entries`, `/api/certs/ct/check`, and `/api/certs/ct/dismiss` for full lifecycle CT monitoring.
- 100% MCP & CLI Parity: Added MCP tools (`get_ct_monitor_config`, `update_ct_monitor_config`, `get_ct_monitor_status`, `list_ct_log_entries`, `check_ct_logs_now`, `dismiss_ct_alert`) and CLI subcommands (`r3 cert ct status`, `config`, `set-config`, `check`, `list`, `dismiss`).
- WebUI Certificate Transparency Tab: Comprehensive dashboard under Certificates (`/certs/ct`) with status metric cards, configurable interval/trusted CA drawer, search and rogue filter bar, and alert resolution workflow.
- Dual-Language Documentation: Updated `configuration.md` and `configuration.tr.md` with CT architecture, rogue detection rules, webhook schemas, and CLI/MCP administration commands.

## [2.6.5] - 2026-10-25

### Added
- HTTP/2 & HTTP/3 Adaptive Stream Priority (RFC 9218): Extensible Prioritization Scheme for HTTP supporting the `Priority: u=urgency, i=?incremental` header across multiplexed HTTP/2 and HTTP/3 streams.
- Urgency Scheduling & Weight Mapping: Standardized 8-tier urgency hierarchy (u=0 critical to u=7 background) mapped to relative scheduling weights (256 down to 2) prioritizing critical rendering paths (HTML, CSS, JS, interactive APIs) over bulk background downloads.
- Automatic MIME & Path Heuristics: Intelligent resource prioritization engine inspecting request paths and Content-Type headers to automatically apply optimal urgency and incremental streaming flags without manual configuration.
- Custom Pattern Rules: Configurable `custom_rules` allowing administrators to override urgency and incremental flags using glob path patterns and MIME types.
- Adaptive Concurrency & Backpressure: Real-time stream controller monitoring multiplexed concurrent streams to throttle and pace background streams (`u >= 5`) during high server congestion, preventing bufferbloat.
- REST API Endpoints: Added `GET /api/proxies/{id}/priority`, `PUT /api/proxies/{id}/priority`, and `GET /api/traffic/priority` for stream priority configuration and global multiplexing metrics.
- 100% MCP & CLI Parity: Added MCP tools `get_stream_priority`, `set_stream_priority`, and `get_stream_priority_stats`, and CLI subcommands `r3 proxy priority <ID> [--enable] [--inject-header] [--adaptive-concurrency]` and `r3 traffic priority`.
- WebUI Stream Priority Management: Added "Stream Priority (RFC 9218)" configuration cards with toggles and hints to the proxy and port forms, plus global stream priority statistics.
- Dual-Language Documentation: Updated `configuration.md` and `configuration.tr.md` with RFC 9218 stream priority specifications, urgency class reference tables, and CLI/MCP administration commands.

## [2.6.4] - 2026-10-25

### Added
- TLS 1.3 0-RTT Early Data: Zero-round-trip connection latency for returning TLS 1.3 clients on TLS-terminated ports (`early_data: bool`).
- RFC 8470 Safe Method Enforcement: Strict validation restricting 0-RTT early data execution to safe HTTP methods (`GET`, `HEAD`, `OPTIONS`, `TRACE`), rejecting state-changing requests (`POST`, `PUT`, `DELETE`, `PATCH`) with HTTP `425 Too Early` and `Retry-After: 0` to prevent replay attacks.
- In-Memory Anti-Replay Mitigation: Sliding-window anti-replay buffer tracking 0-RTT request identifiers to immediately reject replayed early data packets with `425 Too Early`.
- RFC 8470 Header Management: Automatic injection of `Early-Data: 1` header on proxied requests originating from 0-RTT early data and stripping of untrusted client `Early-Data` headers on standard connections.
- 100% MCP & CLI Parity: Added MCP tool `set_port_early_data` and CLI subcommand `r3 port set-early-data <PORT_ID> [--enable]`.
- WebUI TLS Configuration: Added "TLS 1.3 0-RTT Early Data" toggle switch with localized descriptions and validation to the port configuration form.
- Dual-Language Documentation: Updated `configuration.md` and `configuration.tr.md` with TLS 1.3 0-RTT Early Data specifications, anti-replay mechanics, and administration guides.

## [2.6.3] - 2026-10-25

### Added
- Compose .env Template Resolver & Auto-Secret Generator: Automated template resolution and cryptographic secret generation engine for Docker Compose applications and App Store one-click deployments.
- Cryptographic Random Secret Generator: Generates high-entropy passwords (32-character alphanumeric) and secure keys/salts (64-character hex tokens) for empty or placeholder environment variables (`_PASSWORD`, `_SECRET`, `_KEY`, `_TOKEN`, `_HASH`, `_SALT`, `_PASSPHRASE`).
- REST API Endpoints: Added `POST /api/apps/{id}/env/generate` to inspect and automatically populate missing secrets for existing applications, and `POST /api/compose/env/generate` to resolve raw Compose `.env` templates before deployment.
- 100% MCP & CLI Parity: Added MCP tools `generate_app_env_secrets` and `resolve_compose_env_template`, and CLI subcommands `r3 app generate-env <APP_ID> [--save] [--overwrite]` and `r3 app resolve-template [--template <STR>] [--file <PATH>] [--overwrite]`.
- WebUI One-Click Secret Generation: Integrated "Auto-Generate Missing Secrets" action buttons into both the App Store installation wizard (`store_env.rs`) and application environment variables editor (`app_env.rs`).
- Dual-Language Documentation: Updated `platform.md` and `platform.tr.md` with complete Compose environment resolution specifications, placeholder patterns, REST routes, and CLI/MCP examples.

## [2.6.2] - 2026-10-25

### Added
- Multi-Region Load Balancing & Latency-Based Geo-Routing: Dynamic client steering across distributed upstream servers, replica nodes, and multi-region agent targets according to MaxMind GeoIP geographical country/region proximity and measured round-trip response latency.
- Geo-Proximity Steering Engine: New `geo_proximity` load balancing policy mapping incoming client IP addresses to regional presets (`eu`, `na`, `mena`, `apac`, `turkey`, `dach`, `latam`, `nordic`) and direct ISO country assignments, prioritizing closest nodes while preserving healthy fallback pools.
- Latency-Based Routing Engine: New `latency` load balancing policy prioritizing healthy upstream servers with the lowest measured round-trip time (`rtt_micros`) using atomic live performance counters.
- Zero-Downtime Multi-Region Failover: Automated failover protection seamlessly redirecting traffic to healthy nodes in secondary regions if the primary regional replica fails or triggers circuit breakers.
- Target Node Geographical Region Configuration: Added `region` property to platform target nodes (`TargetEntry`, `TargetRequest`, `SetTargetRegionRequest`) enabling regional grouping of agent targets and automatic inheritance during multi-region app deployments.
- REST API Endpoint: Added `PUT /api/targets/{id}/region` to set or clear target node geographical regions with full audit logging.
- 100% MCP & CLI Parity: Added MCP tool `set_target_region` (plus maintenance tools `drain_target` and `undrain_target`) and CLI subcommand `r3 target set-region <ID> [--region <REGION>]`.
- WebUI Upstream & Target Integration: Added `geo_proximity` and `latency` options to the Proxy Upstream load balancing dropdown and application replica settings, along with visual regional badges on target cards.
- Dual-Language Documentation: Updated `configuration.md` and `configuration.tr.md` with complete geo-proximity and latency load balancing specifications, failover mechanisms, and multi-region deployment guides.

## [2.6.1] - 2026-10-25

### Added
- Pre/Post-Deployment Lifecycle Hooks & Migration Runner: Automated command execution for database migrations and cache warming during deployment workflows.
- Zero-Downtime Rollback Protection: Pre-deploy hooks run in an isolated ephemeral container using the new image and environment before old containers are stopped. If the hook fails (non-zero exit code), the deployment aborts cleanly and the running service continues uninterrupted.
- Post-Deploy Cache Warmup: Post-deploy hooks execute inside the healthy container after readiness checks pass and traffic routing begins.
- 100% MCP & CLI Parity: Added `r3 app hooks <APP_ID>` CLI command (`--pre-deploy`, `--post-deploy`, `--clear`) and full MCP support on app management tools.
- WebUI Deployment Lifecycle Hooks Section: Interactive form section in application settings allowing operators to configure pre-deploy and post-deploy lifecycle commands.
- Dual-Language Documentation: Updated `platform.md` and `platform.tr.md` with complete lifecycle hooks usage guides and CLI examples.

## [2.6.0] - 2026-10-25

### Added
- Container Resource Throttling & OOMKill Diagnostic Alert: Built-in kernel cgroup and Docker engine inspection engine diagnosing CPU CFS quota throttling, memory saturation, and OOM termination risk.
- CFS Quota & Throttling Metrics: Real-time calculation of throttled period ratios (`cpu_throttled_percent`), total periods, and cumulative throttled time in milliseconds (`cpu_throttled_time_ms`).
- Memory Pressure & Allocation Fail Tracking: Continuous monitoring of container memory usage against configured limits and tracking of memory allocation failure counts (`failcnt` / `oom_kills`).
- Outbound Webhook Alerts: Added `system.oom_kill` (emergency notification when a container process is terminated by the Linux OOM killer) and `app.cpu_throttled` (early alert when CPU throttling exceeds warning threshold).
- REST API Endpoint: Added `GET /api/apps/{id}/diagnostics` returning comprehensive container resource diagnostic status, health grades, and actionable recommendations.
- 100% MCP & CLI Parity: Added `get_app_resource_diagnostics` MCP tool and `r3 app diagnostics <APP_ID>` CLI command.
- WebUI Diagnostics Section: Integrated resource health badges (Healthy, Throttled Warning, Throttled Critical, OOM Risk, OOM Killed), metric meters, and remediation guidance into the application metrics view.
- Dual-Language Documentation: Updated `platform.md` and `platform.tr.md` with complete container throttling diagnostic specifications, REST routes, CLI and MCP commands, and webhook events.

## [2.5.9] - 2026-10-25

### Added
- Live Long-Lived Connection Inspector & Stream Management Console: Comprehensive real-time tracking, metrics, and administration for active long-lived TCP sessions, WebSocket streams, and Server-Sent Events (SSE) connections.
- Bidirectional Stream Tracking Engine: Core connection registry tracking client IP, upstream target address, duration/uptime, total bytes sent and received, and instantaneous transfer rate (B/s).
- Graceful Connection Termination: Safe disconnection with protocol-aware teardown, issuing WebSocket close frames or TCP bidirectional shutdown to prevent data corruption.
- REST API Endpoints: Added `GET /api/traffic/connections` (active connection listing with protocol and duration metrics) and `DELETE /api/traffic/connections/{id}` (immediate graceful connection termination).
- 100% MCP & CLI Parity: Added `list_active_connections` and `terminate_active_connection` MCP tools, and `r3 traffic connections list` and `r3 traffic connections close <ID>` CLI subcommands.
- WebUI Active Connections Panel: Dedicated tab in the Traffic dashboard with real-time connection counters, color-coded protocol badges, live transfer metrics, and SweetAlert2 confirmation dialogs for safe termination.
- Dual-Language Documentation: Updated `configuration.md` and `configuration.tr.md` with complete connection inspector specifications, REST API routes, MCP tools, and CLI examples.

## [2.5.8] - 2026-10-24

### Added
- Dynamic Response Security Headers Wizard & One-Click OWASP Hardening: Built-in security profile engine providing one-click hardening of HTTP/HTTPS proxy responses according to OWASP standards.
- Predefined OWASP Profiles: Includes `strict` (preloaded HSTS, strict CSP, deny framing, origin isolation), `balanced` (1-year HSTS, CMS-compatible CSP, sameorigin framing), `api_only` (anti-sniffing, framing denial, cache shields), and `spa_frontend` (Vite, React, Vue, PWA optimized with worker and WebSocket directives).
- Automated Security Scoring & Grade Diagnostic: Real-time analysis of response header rules calculating security scores (0 to 100) and letter grades (A+, A, B, C, D, F) with risk attribution and remediation recommendations.
- REST API Endpoints: Added `GET /api/security/headers/profiles` (profile listing), `POST /api/security/headers/analyze` (header rule analysis), `GET /api/proxies/{id}/security-headers` (proxy security audit), and `POST /api/proxies/{id}/security-headers/apply-profile` (profile application).
- 100% MCP & CLI Parity: Added `list_security_header_profiles`, `analyze_proxy_security_headers`, and `apply_security_header_profile` MCP tools, along with `r3 proxy security-headers audit/apply/profiles` CLI commands.
- WebUI Security Wizard Panel: Integrated live security grade badge, one-click OWASP preset buttons, and expandable header audit drawer in the proxy configuration form.
- Dual-Language Documentation: Updated `configuration.md` and `configuration.tr.md` with complete security profiles, CLI and MCP commands, and score diagnostic metrics.

## [2.5.7] - 2026-10-24

### Added
- Upstream gRPC Health Checking Protocol (grpc.health.v1.Health): Native support for probing upstream gRPC servers using standard unary `grpc.health.v1.Health/Check` RPCs over HTTP/2 (`h2c` prior knowledge and TLS ALPN `h2`).
- Service-Level Health Filtering: Configurable `grpc_service` in `HealthCheck` allowing either overall server health checks (empty service string) or granular per-service health monitoring (e.g. `helloworld.Greeter`).
- Dynamic Upstream Failover: Automatic pool draining and routing exclusion for upstream servers returning `NOT_SERVING`, `UNKNOWN`, `SERVICE_UNKNOWN`, or non-zero gRPC error codes, seamlessly restoring servers upon `SERVING` status.
- REST API Endpoints: Added `POST /api/proxies/{id}/grpc/health` (proxy-level upstream check) and `POST /api/grpc/health` (standalone endpoint check).
- 100% MCP & CLI Parity: Added `check_grpc_health` MCP tool and `r3 proxy grpc-health <PROXY_ID> [--service <SERVICE>]` CLI command with colored serving status output and latency diagnostics.
- WebUI Upstream Health Check Selector: Proxy configuration form upgraded with protocol selection (HTTP GET vs gRPC Health) and optional gRPC service name input.
- Dual-Language Documentation: Updated `configuration.md` and `configuration.tr.md` with complete gRPC Health Checking specifications, configuration examples, and CLI/MCP administration commands.

## [2.5.6] - 2026-10-24

### Added
- ACME DNS-01 CNAME Delegation & Alias Mode (RFC 8555): Support for automated wildcard and single-domain certificate issuance without requiring clients or third-party domains to share their DNS provider API credentials.
- Configurable DNS Alias Target: Proxy and ACME-level `dns_alias` target configuration with dynamic pattern substitution (`{domain}` template, FQDN targets, or root alias domain).
- Automated TXT Placement & Verification: DNS-01 challenge engine publishes validation TXT records under the delegated alias domain using r3v3rs3's configured DNS provider while verifying public CNAME propagation.
- REST API Endpoint: Added `GET /api/acme/{id}/cname_instructions` returning source FQDN, target alias FQDN, and live public DNS CNAME resolution and verification status.
- 100% MCP & CLI Parity: Added `get_acme_cname_instructions` MCP tool and `r3 acme cname-instructions <ID>` CLI command for autonomous inspection and administration.
- WebUI CNAME Delegation Panel: Interactive CNAME record preview in the ACME creation form and dedicated "CNAME Instructions" dialog in the certificates list view.
- Dual-Language Documentation: Comprehensive English and Turkish documentation in `configuration.md` and `configuration.tr.md` explaining RFC 8555 DNS alias mode, customer DNS setup, and operational CLI/MCP commands.

## [2.5.5] - 2026-10-24

### Added
- Shared Compression Dictionaries (RFC 9651 / RFC 9652): Support for shared dictionary compression with Brotli (`dcb`) and Zstandard (`dcz`), enabling up to 90%+ payload reduction for repetitive API endpoints, dynamic HTML, and single-page application assets.
- Dictionary Management & Serving: Proxy-level dictionary definitions (`CompressionDictionary`) with custom matching patterns, internal serving endpoint with RFC-compliant headers (`Use-As-Dictionary`, `Cache-Control: public, max-age=31536000, immutable`, and strong SHA-256 structured field hash).
- Runtime Content Negotiation & Encoding: Full support for client dictionary advertisements via `Available-Dictionary` and `Sec-Available-Dictionary` request headers, automatic response encoding selection (`dcz` / `dcb`), `Content-Dictionary` response header, and appropriate `Vary` cache keying.
- 100% MCP & CLI Parity: Added MCP tools (`list_compression_dictionaries`, `get_compression_dictionary`, `add_compression_dictionary`, `delete_compression_dictionary`) and CLI subcommands under `r3 proxy dictionary` (`list`, `get`, `add`, `delete`).
- WebUI Compression Dictionaries Table: Proxy compression configuration editor extended with shared dictionary management, enabling adding, viewing SHA-256 hashes, and deleting dictionaries.
- Dual-Language Documentation: Comprehensive English and Turkish documentation in `configuration.md` and `configuration.tr.md` covering shared dictionaries, client negotiation, RFC 9651/9652 headers, and administrative CLI and MCP usage.

## [2.5.4] - 2026-10-24

### Added
- Centralized Error Page Themes Hub: Reusable error page templates (`ErrorPageTheme`) pool with built-in templates (`dark`, `glass`, `term`, `card`, `maint`) covering standard HTTP error status codes (400, 401, 403, 404, 405, 408, 429, 500, 502, 503, 504).
- Proxy-Level Theme Binding: Added `error_page_theme: Option<ShortId>` to `HttpProxy`, allowing reverse proxies to bind to a centralized theme while preserving granular per-proxy `error_pages` overrides.
- Dynamic Variable Substitution: Automatic runtime replacement of template tokens (`{{status}}`, `{{message}}`, `{{request_id}}`, `{{host}}`, `{{client_ip}}`) for both upstream error responses and gateway-generated errors in `ResponseRewriter`.
- 100% MCP & CLI Parity: Added MCP tools (`list_error_page_themes`, `get_error_page_theme`, `add_error_page_theme`, `update_error_page_theme`, `delete_error_page_theme`, `reset_error_page_themes`) and CLI subcommands under `r3 error-page-theme` (`list`, `get`, `add`, `update`, `delete`, `reset-defaults`).
- WebUI Error Page Themes Management: Dedicated Error Page Themes card on the Settings page with table view, reset defaults, and delete actions; theme selector dropdown and dynamic variable token helper banner in the proxy configuration editor.
- Dual-Language Documentation: Comprehensive English and Turkish documentation in `configuration.md` and `configuration.tr.md` explaining error page themes, proxy assignment, dynamic variables, MCP tools, and CLI management.

## [2.5.3] - 2026-10-23

### Added
- Automated MaxMind GeoIP MMDB Updater & License Management: Fully autonomous, zero-downtime updater for MaxMind GeoLite2-Country databases with SHA256 integrity verification, safe archive extraction, and lock-free in-memory hot reloading (`ArcSwapOption`).
- Background Periodic Update Scheduler: Configurable update schedule (`auto_update`, `update_interval_days`, default: 7 days) evaluated every 12 hours on the leader node without impacting proxy throughput.
- Outbound Webhook Alerts: Added `system.geoip_updated` and `system.geoip_update_failed` under the `System` category with event metadata and failure reporting.
- WebUI GeoIP Status Card & Controls: Live status card on Settings page displaying loaded state, database path, file size, build epoch date, license configuration status, and an immediate "Update Database Now" action button.
- 100% MCP & CLI Parity: Added MCP tools (`get_geoip_status`, `update_geoip_database`, `get_geoip_config`, `update_geoip_config`) and CLI subcommands under `r3 security geoip` (`status`, `update`, `config`, `set-config`).
- Comprehensive Documentation: Updated English and Turkish documentation with API endpoints, MCP tools table, and CLI usage.

## [2.5.2] - 2026-10-23

### Fixed
- Loopback Quarantine Bypass: Excluded loopback IP addresses (`127.0.0.1`, `::1`) from behavioral threat detection and TCP drop quarantine enforcement, preserving local health checks, worker proxies, and automated bot blocker test suites.

## [2.5.1] - 2026-10-23

### Added
- IP Reputation & Threat Intelligence Auto-Quarantine: In-memory sliding-window behavioral threat detection inspecting HTTP request paths and repeated client error status codes (404, 403, 401).
- Scanner Probe Pattern Matching: Instant automatic quarantine on scanner path probes including `/.env`, `wp-admin`, `wp-login.php`, `/.git`, `/phpmyadmin`, `/.aws`, `/xmlrpc.php`, and `/config.json`.
- Zero-Overhead TCP Drop Enforcement: Quarantined peers have their TCP connections reset/dropped at socket accept level before TLS negotiation or HTTP response generation, minimizing server resource consumption.
- Webhook Threat Notification: Dispatches `security.threat_detected` (`WebhookEvent::SecurityThreatDetected`) with offending IP, trigger reason, request path, and expiration timestamp.
- WebUI Quarantine Management Card: Added Quarantined IPs table with unquarantine action icon buttons and manual IP quarantine modal under Settings.
- 100% MCP & CLI Parity: Added MCP tools (`list_quarantined_ips`, `quarantine_ip`, `unban_quarantined_ip`, `get_quarantine_config`, `update_quarantine_config`) and CLI subcommands (`r3 security quarantined-ips`, `quarantine`, `unquarantine`, `quarantine-config`, `set-quarantine-config`).
- Comprehensive Documentation: Updated English and Turkish documentation with API endpoints, MCP tools table, and CLI usage.

## [2.5.0] - 2026-10-23

### Added
- Node File Descriptor Exhaustion Alerts: Added `/proc/self/fd` counting against `rlimit` NOFILE limits to detect open file descriptor exhaustion under high traffic load with configurable `max_fd_ratio_percent` threshold (default: 80%).
- Socket & TIME_WAIT Exhaustion Alerts: Added `/proc/net/sockstat` socket metrics parser (`socket_stats()`) and configurable `max_time_wait_sockets` threshold to detect socket and ephemeral port exhaustion.
- Outbound Webhooks: Added `system.fd_exhaustion` and `system.socket_exhaustion` webhook event types under the `System` category with full HMAC-SHA256 signature support and 15-minute anti-spam cooldown.
- WebUI Node Alert Threshold Controls: Added threshold inputs for maximum open file descriptor percentage and maximum TIME_WAIT socket count on the Targets page (`targets.rs`).
- MCP & CLI Parity: Added `get_node_alerts` and `update_node_alerts` MCP tools in `r3v3rs3/src/admin/mcp.rs`, and `r3 platform node-alerts` and `r3 platform set-node-alerts` subcommands in `r3v3rs3-cli`.
- Prometheus Metrics Expansion: Added `r3v3rs3_host_sockets_used`, `r3v3rs3_host_tcp_inuse`, and `r3v3rs3_host_tcp_tw` gauges to `/api/metrics`.

### Fixed
- Installer Database Preservation: Ensured `install.sh` never overwrites or re-initializes existing SQLite databases (`r3v3rs3.db`), preventing credential resets on upgrades.
- Legacy Service Unit Cleanup: Automatically stop and remove deprecated `r3v3rs3.service` systemd units during installation and upgrade.

## [2.4.9] - 2026-10-23

### Added
- Container Log Rotation Policy: Automatically enforce Docker container logging limits (`max-size: 50m`, `max-file: 3`) on all deployed applications and databases to prevent host disk exhaustion.
- On-Demand Log Vacuum: Added active log truncation and rotated log removal (`.1`, `.2`) via `POST /api/apps/{id}/logs/vacuum` and `POST /api/platform/logs/vacuum`.
- Log Size Inspection: Added `GET /api/apps/{id}/logs/size` reporting active container log file sizes, rotated log counts, and total byte footprint.
- Target Prune Auto-Vacuum Integration: Extended target prune policies with `prune_logs`, `log_vacuum_threshold_mb`, `container_log_max_size`, and `container_log_max_file` settings.
- WebUI Log Vacuum Controls: Added live container log size badges and an on-demand "Vacuum Logs" action button on the Application Logs page, along with container log prune configuration cards in Targets.
- 100% MCP & CLI Parity: Added MCP tools (`get_app_log_size`, `vacuum_app_logs`, `vacuum_all_logs`, `run_platform_prune`) and CLI commands (`r3 app log-size`, `r3 app vacuum-logs`, `r3 platform vacuum-logs`).
- Native Binary Renaming: Renamed CLI client binary directly to `r3` across all platforms (producing `r3` / `r3.exe`), and server binary to `r3-server` with `r3-agent` symlink.

### Changed
- Disentangled Model Context Protocol (MCP) server integration from the command-line client (`r3v3rs3-cli`), separating CLI client responsibilities from MCP endpoint hosting.
- Updated documentation and installer scripts (`install.sh`, `install-cli.sh`, `install-cli.ps1`) to use `r3` and `r3-server`.

## [2.4.8] - 2026-10-23

### Added
- 100% CLI First Autonomous Administration Parity: Expanded `r3v3rs3-cli` with first-class command sets providing 100% operational parity with the WebUI, API, and MCP server, enabling terminal users and CI/CD pipelines to manage the entire reverse proxy and PaaS stack headlessly.
- Comprehensive CLI Management Commands:
  - `port add`, `port get`, `port delete`, `port reset`: Full lifecycle management of listening sockets and port interfaces.
  - `proxy add`, `proxy get`, `proxy status`, `proxy purge-cache`, `proxy delete`: Reverse proxy routing configuration and path-selective cache invalidation.
  - `cert list`, `cert get`, `cert upload`, `cert coverage`, `cert delete`: TLS certificate inspection, multipart certificate/key upload, and domain coverage analysis.
  - `acme list`, `acme get`, `acme add`, `acme order`, `acme delete`: Automated ACME certificate order management and DNS-01 provider linking.
  - `acl list`, `acl get`, `acl add`, `acl delete`: Access control list management and IP rule configuration.
  - `dns list`, `dns get`, `dns add`, `dns delete`: DNS-01 provider credential management.
  - `app add`, `app get`, `app logs`, `app env`, `app set-env`, `app rollback`, `app start`, `app stop`, `app restart`, `app delete`: PaaS container lifecycle, streaming logs, and environment variable configuration.
  - `db list`, `db get`, `db create`, `db clone`, `db replica`, `db logs`, `db delete`: Managed database provisioning, replica/clone management, and streaming logs.
  - `backup list`, `backup create`, `backup restore`, `backup delete`, `backup config`: Database backup management and storage configuration.
  - `target list`, `target get`, `target add`, `target drain`, `target undrain`, `target token`, `target delete`: Multi-node agent target clustering and traffic draining.
  - `project list`, `project get`, `project add`, `project sync-env`, `project delete`: Project grouping and cross-environment secret synchronization.
  - `store list`, `store refresh`, `store install`: One-click App Store catalog browsing and template installation.
  - `security banned-ips`, `security unban`: Security monitoring and IP unbanning.
  - `webhook list`, `webhook get`, `webhook add`, `webhook test`, `webhook delete`: Outgoing event webhook management and ping tests.
  - `cron list`, `cron add`, `cron run`, `cron delete`: In-container scheduled command management and manual execution triggers.
  - `platform status`, `platform metrics`, `platform prune`, `platform rotate-key`: System status, hardware metrics, container disk hygiene prune, and AES encryption key rotation.
- Flexible Payload Input: Supported JSON and YAML payloads inline, from local files, or directly streamed via standard input (`-`) across all management commands.

## [2.4.7] - 2026-10-23

### Added
- Multi-Tab Container Web Terminal: Supported multiple concurrent web terminal sessions per container with isolated WebSocket connections, background session preservation across tab switches, tab close controls, and one-click shell presets (`/bin/sh`, `/bin/bash`, `/bin/zsh`, and database CLIs `psql`, `mysql`, `mariadb`, `redis-cli`, `mongosh`).
- Persistent Terminal Session Command History: Captured and persisted command history memory across active container terminal sessions with Up/Down arrow keyboard navigation in custom command input.
- Searchable Command History Modal: Added an interactive history dialog with real-time text filtering, single-click command execution, active input insertion, clipboard copying, and history purge capability.
- Quick Maintenance Command Snippet Panel: Integrated quick-action maintenance command snippets categorized by System, Files, Network, Environment, and Database for immediate container troubleshooting.

## [2.4.6] - 2026-10-23

### Added
- 100% MCP First Autonomous Administration Parity: Expanded the Model Context Protocol server (`/api/mcp`) to expose comprehensive tools covering every operational capability across the reverse proxy and PaaS stack, enabling autonomous AI agents to manage the entire platform without opening the WebUI.
- Autonomous Port & Proxy Lifecycle Tools: Added MCP tools for port creation, configuration updates, socket reload/reset, host network interface inspection, proxy health monitoring, and path-selective cache purging.
- Automated Certificate & ACME Tools: Added MCP tools for certificate coverage checking across domains, ACME request creation, deletion, and on-demand renewal triggering, plus DNS credential inspection.
- PaaS Application & Deployment Management Tools: Added MCP tools for application creation, specification updates, container deletion, on-demand deployments, live container log streaming, environment variable management, deployment history listing, and rollback to prior versions.
- One-Click App Store Installation via MCP: Added `install_store_app` tool to deploy and route catalog applications directly from the curated App Store templates with automated port and domain configuration.
- Managed Databases & Backups Tools: Added MCP tools to provision PostgreSQL, MySQL, Redis, MariaDB, and MongoDB with automated credentials, take immediate backups, delete snapshots, and manage S3/SFTP/rsync storage configuration.
- Infrastructure & Security Tools: Added MCP tools for agent target node enrollment, project and shared environment group management, brute-force IP ban inspection and unbanning, system performance metrics, and disaster recovery snapshot creation.

## [2.4.5] - 2026-10-23

### Added
- Self-Hosted App Store Assets: Hosted all 58 application official vector/raster logos and high-resolution UI screenshots directly in the dedicated `r3v3rs3-apps` repository, eliminating broken external links and third-party rate limits.
- App Store Logo Badges: Added `logo` field to `StoreApp` and rendered rounded badges in WebUI catalog cards next to application titles.

### Fixed
- Missing Download Icon in Audit View: Added `download.svg` icon to WebUI assets, resolving 404 errors when viewing the audit log export action.

## [2.4.4] - 2026-10-23

### Added
- Unified SQLite Database Architecture (`r3v3rs3.db`): Consolidated server configuration, reverse proxies, port bindings, access lists, ACME settings, DNS provider credentials, user accounts, and deployment platform operational state into a single unified SQLite database engine with WAL journaling and atomic transaction guarantees.
- Automatic Legacy Configuration Migration: Built-in startup migrator that automatically imports legacy flat files (`config.toml`, `ports.toml`, `proxies.toml`, `access_lists.toml`, `acme.toml`, `dns-credentials.toml`, `accounts.toml`, `notifications.json`, and `platform.db`) into `r3v3rs3.db` and safely renames migrated files to `*.migrated` with zero data loss.
- Master Encryption Key Hardening (`r3v3rs3.key`): Unified master encryption key under `r3v3rs3.key` with strict `0600` permissions for AES-256-GCM sealing of secrets, Git tokens, and DNS credentials, migrating legacy `platform.key` on boot.
- Preserved Disk Storage & Log Isolation: Retained certificate files on disk under `certs/<kind>/<id>/*.pem` with relative database path references, kept external catalog JSON datasets (`cdn-ranges.json`, `app-store.json`, `engines.json`) on disk, and preserved dedicated `log.db` for high-throughput tracing isolation.

## [2.4.3] - 2026-10-23

### Added
- Persistent MCP OAuth 2.1 Backend: Stored dynamic clients and issued OAuth 2.1 authorization tokens (both access and 30-day refresh tokens) in the SQLite platform database (`platform.db`), ensuring developer sessions survive server restarts and reboots.
- Automatic Background Token Rotation: Enabled remote MCP clients (such as Google Cloud Code) to rotate expired access tokens transparently in the background across restarts without prompting users for re-authentication.
- WebUI Active MCP Sessions Card: Added a live active MCP sessions inspector to the WebUI Settings page displaying client names, client IDs, authorized usernames, granted scopes, and creation/expiration timestamps with human-readable formatting.
- One-Click Session Revocation: Added immediate token revocation via standard red trash action icon button (`ActionIconButton`) with sweetalert confirmation dialog and admin permission checks (`DELETE /api/mcp/sessions/{id}`).

## [2.4.2] - 2026-10-23

### Added
- Standardized Action Icon Buttons (`ActionIconButton`): Replaced raw, text-heavy action links with compact, accessible icon buttons featuring localized hover tooltips (`title`) and `aria-label` screen reader tags across all WebUI tables.
- Semantic Role & Color Palette: Cohesive visual styling across row actions: Edit (blue pencil), View (neutral slate eye), Terminal (emerald terminal), Files (amber folder), Metrics/Traffic (cyan chart), Logs (indigo document), Deploy/Run (emerald play), Pause/Drain (orange pause), Download (neutral download), Restore/Re-test (teal refresh), and Delete (red trash).
- Unified Table Spacing & Responsiveness: Streamlined table row action containers (`gap-1.5`) across Proxies, Applications, Ports, Certificates, Database Backups, Targets, Git Connections, and Projects, saving horizontal screen real estate.

## [2.4.1] - 2026-10-23

### Added
- Upstream Live Metrics & Health Tracking: Track real-time routed requests, active in-flight connections, error counts/rates, moving average RTT latency, and throughput bytes per upstream node across HTTP and TCP reverse proxy pools.
- Per-Node Traffic Distribution Modal: Interactive WebUI modal featuring segmented proportional distribution bar, expected weight vs. actual traffic share comparisons, live health and circuit breaker status badges, and aggregate summary metric cards.
- Centralized Formatting Utilities: Consolidated byte size (`format_bytes`) and latency (`format_latency`) formatting into a unified, shared `format.rs` module across the WebUI.
- Direct Application Terminal & Files Links: Added one-click action links to every row of the Applications table for immediate access to container web terminal and volume file manager.

### Fixed
- Sidebar Icon Color Uniformity: Standardized folder and backup cloud SVG icon fills to white, ensuring crisp contrast across light and dark sidebar themes.

## [2.4.0] - 2026-10-23

### Fixed
- Local Target API Compatibility: Excluded local Docker host from mTLS link latency and jitter metrics in the platform targets API response, ensuring strict compatibility with local target schema assertions while keeping the WebUI status badge crisp.

## [2.3.9] - 2026-10-23

### Added
- Agent Latency & Link Quality Matrix: Real-time RTT latency, jitter (RFC 3550 mean absolute deviation), and packet loss tracking across multiplexed mTLS agent connections.
- Dynamic Load Balancer De-weighting: Automated link quality classification (`Excellent`, `Good`, `Degraded`, `Poor`, `Offline`) with proportional load balancer de-weighting (`10`, `10`, `5`, `1`, `0`) to reduce traffic to degraded or unstable agent nodes across round-robin and least-connections upstream pools.
- WebUI Link Quality Badges & Sparkline: Interactive, dark-mode compatible link quality badges, latency stats, and inline SVG latency sparklines on the Targets page.

### Fixed
- WebUI Dark Mode Styling: Resolved contrast issues in the notification flyout and table row hover states, and corrected the default status filter in the live traffic console.

### Changed
- CI Workflow Optimization: Enabled parent-commit test gate verification in the release pipeline and added run concurrency groups to eliminate redundant CI runs.

## [2.3.8] - 2026-10-23

### Added
- Automated Restore Drill Simulation: Deep in-memory restore simulation and schema extraction for database backups without launching auxiliary containers or affecting production databases.
- Schema & Data Integrity Analysis: Deep parsing of SQL dumps (PostgreSQL, MySQL, MariaDB) extracting declared table definitions, detecting payload data presence (`COPY` or `INSERT`), and verifying EOF completion markers; plus binary header, version, and checksum verification for Redis RDB and SQLite 3.
- Backup Drill Diagnostics & Metadata: Verified backups capture execution latency (`drill_duration_ms`), discovered table counts, table name samples, and total schema object metrics stored in backup manifests.
- WebUI Interactive Drill Inspector: Interactive diagnostics modal accessible by clicking verification badges, displaying status banners, 4-card metric summaries, discovered table chips, and a one-click **Run Restore Drill** dry-run re-tester.
- Emergency Verification Failure Alerts: Outbound webhook notification (`backup.verification_failed`) dispatched immediately when checksums, decompression, or schema integrity checks fail, alerting operators prior to disaster recovery incidents.

## [2.3.7] - 2026-10-23

### Added
- Docker Compose Architecture Visualizer: Interactive SVG architecture visualizer and service dependency graph rendered directly in the WebUI for multi-container Compose applications, featuring responsive viewBox pan/zoom controls and node filtering.
- Dependency & Relationship Edge Engine: Automatic extraction and rendering of directed bezier connections for `depends_on` (with health conditions such as `healthy`, `started`, `completed`), container network memberships, and volume mounts with mount targets and access mode flags (`ro`/`rw`).
- Real-Time Container Health & Runtime Status: Live color-coded runtime halos (healthy/running green, starting amber, exited/unhealthy red) on topology cards, with primary external traffic entrypoint badges.
- Node Inspector Flyout: Interactive slide-out drawer displaying container image tags, port mappings, environment variable counts, restart policies, container names, and complete upstream and downstream dependency chains upon clicking any topology card.
- Live Topology in Compose Editor: Real-time visual architecture preview tab in the Compose YAML editor that dynamically parses and updates the dependency diagram as operators edit Compose YAML or select starter templates.
- Compose Architecture Topology API: Added `GET /api/apps/{id}/topology` endpoint returning parsed topology nodes, relationships, and health summaries with full OpenAPI 3.1 documentation.
- Widescreen Desktop Experience: Widened desktop container to 1680px for high-density dashboard layouts, alongside expanded modal dialogs for side-by-side diffing and topology views.

## [2.3.6] - 2026-10-22

### Added
- Cross-Environment Configuration & Secret Diff Engine: High-performance comparison engine (`diff_environments`) performing in-memory evaluation of variables and secrets across project environments, classifying entries into `Identical`, `Different Value`, `Missing in Target`, or `Missing in Source`.
- Zero-Leak Cryptographic Secret Re-Sealing: Secure secret synchronization pipeline (`sync_environment_env`) that opens encrypted secrets in server-side memory and re-seals them using the target environment's cryptographic Associated Authenticated Data (`env_aad`), preventing cross-row decryption attacks without ever exposing plaintext secrets to the WebUI or API clients.
- One-Click Cross-Environment Synchronization: Support for multiple synchronization modes: `SyncMissing` (syncs missing variables without overwriting existing target values), `CloneAll` (full overwrite/clone), and `SelectedKeys` (granular single-key sync).
- WebUI Multi-Environment Manager & Diff Console: Redesigned Environments modal with horizontal navigation tabs for each environment, plus a dedicated **Compare & Sync** tab featuring interactive source/target selectors, real-time diff metric badges, side-by-side diff table, and one-click key synchronization.
- Cross-Environment Admin API Endpoints: Added `GET /api/projects/{id}/environments/{source_id}/diff/{target_id}` and `POST /api/projects/{id}/environments/sync` with full OpenAPI 3.1 documentation and audit logging (`AuditAction::SyncEnvironmentEnv`).

## [2.3.5] - 2026-10-22

### Added
- Prometheus & Runtime Anomaly Detection Engine: Autonomous evaluation loop running every 10 seconds monitoring real-time P99 latency, 5xx server error rate, traffic throughput spikes (RPS), and Tokio event loop scheduling lag.
- WebUI Notification Center: Integrated top navigation bar notification bell featuring a live count badge for active anomalies, flyout list with severity indicators (`CRITICAL` vs `WARNING`), live metric vs threshold comparisons, and one-click alert dismissal.
- Anomaly Alert Webhook Dispatcher: Automated webhook notifications dispatched on anomaly triggers (`system.high_latency`, `system.high_error_rate`, `system.traffic_spike`, `system.event_loop_lag`) with an integrated 5-minute cooldown per anomaly type to prevent notification storms.
- Anomaly Alert Threshold Configuration Card: Dedicated Settings card enabling operators to dynamically configure threshold limits and toggle automated anomaly detection without restarting the server.
- Anomaly Detection API Endpoints: Added `GET /api/system/anomalies`, `GET /api/system/anomalies/rules`, `PUT /api/system/anomalies/rules`, and `POST /api/system/anomalies/dismiss` with full OpenAPI 3.1 documentation.

## [2.3.4] - 2026-10-22

### Added
- Live Request Inspector & Detail Drawer: An interactive slide-over drawer accessible directly from the live traffic monitor (`/traffic`) displaying detailed request diagnostics across Overview, Headers, Payload, and Replay tabs.
- Full Request & Response Header Inspection: Deep inspection of parsed client request headers and upstream response headers formatted in a searchable key-value table.
- Body Buffering & Smart Payload Viewer: In-memory capture of request bodies (up to 16 KB) for non-WebSocket requests with syntax-highlighted formatted JSON and raw text toggle modes.
- Copy as cURL: One-click generation of terminal-ready, escaped cURL commands directly reproducible from captured request headers and payloads.
- Traffic Replay Engine & Console: Interactive replay runner allowing operators to re-dispatch captured HTTP or WebSocket handshake requests to target endpoints or custom URLs with live status code reporting and round-trip latency measurements.
- Traffic Inspection API Endpoints: Added `GET /api/traffic/{id}` to fetch complete access log details and `POST /api/traffic/replay` to replay requests with custom timeouts, hop-by-hop header filtering, and response body capture.

## [2.3.3] - 2026-10-22

### Added
- gRPC Server Reflection Protocol v1 & v1alpha: Automatic dynamic discovery of upstream gRPC services, methods, request/response message types, and schema definitions without requiring local proto file compilation.
- HTTP/2 gRPC Unary Invocation Engine: Native HTTP/2 client engine to invoke gRPC methods directly via dynamic JSON payload encoding and decoding, Protobuf wire serialization, gRPC message framing, custom metadata headers, and trailer status code inspection.
- gRPC API Endpoints: Added `POST /api/proxies/{id}/grpc/reflect`, `POST /api/proxies/{id}/grpc/invoke`, and standalone `POST /api/grpc/reflect` & `POST /api/grpc/invoke` endpoints with full OpenAPI 3.1 documentation.
- Interactive WebUI gRPC Test Console: Embedded Postman/gRPCui-style interactive testing tool within the HTTP proxy configuration, featuring service and method dropdown selectors, automatic sample JSON payload generation, custom metadata header editor, latency measurement, and gRPC status code display with payload inspection.

## [2.3.2] - 2026-10-22

### Added
- Automatic Wildcard SSL Sharing: Applications and reverse proxies deployed under a common root domain automatically detect and share existing wildcard certificates (`*.example.com`) with zero manual TLS configuration or redundant ACME orders.
- Domain Certificate Coverage API: Added `POST /api/certs/coverage` endpoint that analyses configured domain lists, detects matching wildcard or dedicated certificates, and identifies domains requiring external CNAME delegation.
- Asynchronous Public DNS CNAME Verification Engine: Added `GET /api/certs/verify_cname` utilizing Hickory DNS to inspect public CNAME DNS records and verify proper delegation pointing to the cluster public hostname.
- Interactive WebUI SSL Status Badges & CNAME Guidance: Visual SSL status badges rendered directly beneath the domain textarea in platform app creation and virtual host configuration in HTTP proxies, with one-click interactive CNAME verification and target copy helpers.

## [2.3.1] - 2026-10-22

### Added
- Smart Pre-Compression & Direct Static Asset Serving: Enhanced static file serving to automatically negotiate and serve pre-compressed `.zst` (Zstandard), `.br` (Brotli), and `.gz` (Gzip) assets directly from the embedded WebUI bundle with zero runtime CPU compression overhead.
- Universal Decompression Fallback: Automatic on-the-fly decompression for legacy clients requesting uncompressed responses when only pre-compressed assets are present.
- Adaptive Compression Quality Tuning: Dynamically adjusts compression levels based on response payload size:
  - Small payloads (< 8 KB): Brotli level 5, Zstandard level 4, Gzip level 9 (Best) for maximum compression ratios at negligible CPU cost.
  - Medium payloads (8 KB – 64 KB): Brotli level 4, Zstandard level 3, Gzip level 6 (Default).
  - Large or streaming payloads (> 64 KB or unmetered streams): Brotli level 3, Zstandard level 1, Gzip level 1 (Fastest) for maximum network throughput without thread saturation.
- Smart MIME Minimum Size Threshold Adaptation: Dynamically lowers the compression minimum size threshold down to 256 bytes for text, JSON, XML, and SVG API responses, while raising the threshold to 2048 bytes for WebAssembly modules.
- Incompressible Binary Format Pre-Filtering: Automatically excludes already-compressed media formats (PNG, JPEG, WebP, AVIF, MP4, WOFF, WOFF2, ZIP, TAR, GZ, ZST, 7Z, RAR) from redundant, wasteful re-compression.
- WebUI Compression Toggles: Added interactive switches for "Adaptive Quality & MIME Level" and "Serve Pre-Compressed Assets" within the HTTP Proxy Compression settings card.

## [2.3.0] - 2026-10-22

### Added
- Interactive Error Pages Designer & Live Preview: Replaced basic error page text inputs with an interactive visual designer component supporting real-time preview, code editing, and split-view modes.
- Built-in HTML/CSS Error Page Template Library: Instant one-click templates tailored for common HTTP error codes (400-599):
  - Minimalist Dark: Sleek dark theme with radiant glow, status pill badge, and return home action button.
  - Modern Glassmorphism: Frosted glass card with vibrant gradient typography and elegant styling.
  - Developer Terminal: DevOps / hacker CLI console with simulated HTTP response headers, diagnostics, and blinking cursor.
  - Illustration Card: Centered modern card featuring clean vector SVG graphics and friendly messaging.
  - Maintenance Notice: Amber maintenance alert with gear illustration and auto-refresh notices.
- Sandboxed Live Iframe Rendering: Real-time isolated preview iframe updating instantaneously as HTML/CSS code is edited.
- Multi-Viewport Device Switcher: Toggle live error page previews across Desktop (100%), Tablet (768px), and Mobile (375px) responsive widths.
- Quick HTTP Status Presets: Single-click status code selector buttons for 404, 502, 503, 500, 403, and 429.
- Real-Time Payload Size Counter: Live byte counter comparing custom error page body sizes against the 64 KB storage limit.

## [2.2.9] - 2026-10-22

### Added
- Visual GeoIP Rule Wizard Card: Replaced plain text comma-separated country code inputs across HTTP proxies, route IP overrides, and access lists with an interactive visual rule wizard card.
- Single-Click GeoIP Enforcement Modes: Intuitive mode switcher supporting "Disabled (Off)", "Allowlist (Only Selected)", "Denylist (Block Selected)", and "Advanced (Allow & Deny)" with contextual summary banners describing enforcement behavior.
- Instant Regional Country Presets: Pre-configured single-click presets to generate policies for Turkey (TR), European Union (EU 27 member states), North America (US, CA, MX), G7 Nations, DACH (DE, AT, CH), APAC, LATAM, MENA, and Nordic regions.
- Country Search Autocomplete & Filter: Search any ISO-3166-1 country by alpha-2 code, English name, or Turkish name with real-time autocompletion dropdown.
- Interactive Tag Chip Badges: Selected countries render as visual chips showing alpha-2 code and localized display name with quick one-click removal buttons.
- Collapsible Direct CSV Input Fallback: Expandable raw input fields for bulk copying and pasting comma-separated country codes.

## [2.2.8] - 2026-10-22

### Added
- Interactive GeoIP World Map & Real-Time Traffic Heatmap: Client IP addresses are resolved against MaxMind MMDB into ISO country codes across all reverse proxy handlers (HTTP/1.1, HTTP/2, and HTTP/3) with zero latency overhead.
- Interactive SVG World Map Visualizer: Vector map component displaying global request distribution with logarithmic density heatmap color styling, hover tooltips detailing country name and request metrics, and click-to-filter interaction.
- Top Countries Ranked Leaderboard: Displays top 10 countries ranked by traffic volume with animated percentage progress indicators, request counts, and quick filter toggles.
- Geographic Filtering in WebUI: Filter live streaming and historical traffic by country code via toolbar dropdown, interactive map clicking, or active filter tags.
- Admin API Traffic Geo Distribution Endpoint: `GET /api/traffic/geo` aggregates real-time geographical traffic distribution, returning total resolved requests, ranked top countries, and full ISO code count mappings respecting proxy scoping.
- Access Log Query Country Filter: `AccessLogQuery` and `/api/traffic` support filtering by country code with case-insensitive matching.

## [2.2.7] - 2026-10-22

### Added
- Advanced Host Node Disk and Network Bandwidth Threshold Alerts: Automated node-level resource threshold monitoring triggering outbound webhook events and audit logs.
- Low Free Disk Warning Alert: Dispatches `system.disk_low` webhook notifications when available host node disk space drops to or below the configured percentage threshold (default: 10%), detailing available, used, and total disk bytes.
- Bandwidth Spike Anomaly Detection: Dispatches `system.bandwidth_spike` webhook notifications when total network throughput rate exceeds the configured bytes-per-second limit, measured via `/proc/net/dev` with fallback to container network rates.
- Anti-Spam Notification Cooldown: 15-minute suppression window preventing alert fatigue during sustained high traffic or prolonged low disk conditions.
- WebUI Node Threshold Alert Controls: Interactive configuration card on the Targets page to toggle alerts, configure low disk free space percentages, and set bandwidth spike thresholds in MB/s.
- Admin API Node Alerts Endpoints: Registered `GET /api/platform/node_alerts` and `PUT /api/platform/node_alerts` in OpenAPI specs with dedicated audit logging (`AuditAction::UpdateNodeAlertRules`).
- Webhook Event Registry: Exposed `system.disk_low` and `system.bandwidth_spike` in outbound webhook subscriptions under the System category.

## [2.2.6] - 2026-10-22

### Added
- Reactive Disk-Pressure Emergency Pruning: Automated Docker resource pruning triggered whenever node disk usage meets or exceeds the configured percentage threshold (default: 85%), with a 15-minute anti-thrashing cooldown and audit logging.
- Outbound Disk Pressure Webhooks: Dispatches `system.disk_pressure` webhook event notifications detailing reclaimed disk bytes and deleted container, image, cache, and volume counts upon emergency prune execution.
- WebUI Disk Pressure Threshold Control: Configurable threshold input field (%) with explanatory hints and last emergency prune execution timestamp on the Targets page.
- Web Terminal Multi-Shell Selection: Interactive shell switcher supporting `/bin/sh`, `/bin/bash`, and `/bin/zsh` with automatic graceful fallback to `/bin/sh` on minimalist container images.
- Database CLI Console Shortcuts: Direct one-click interactive terminal shortcuts for managed databases (`psql`, `mysql`, `mariadb`, `redis-cli`, `mongosh`) with pre-applied connection credentials.
- Web Terminal Session Utilities: In-browser terminal tools to clear screen buffer and copy full session console output directly to the clipboard.

### Changed
- Streamlined Release Pipeline: Removed crates.io publish step from CI workflows; releases and distribution are strictly focused on GitHub Releases and `install.sh`.

## [2.2.5] - 2026-10-22

### Added
- Multi-Node Dynamic Active Health Checks: Proactive background polling for application replicas deployed across multiple nodes; automatically wires `health_check_path` (or active TCP connection probes every 10 seconds for multi-replica apps) into the reverse proxy engine.
- Automated Outlier Detection & Circuit Breaker: Outlier detection policy isolating failing replicas when failure ratio exceeds 50% over a 10s window for 30 seconds, automatically diverting client traffic to healthy nodes without dropped requests.
- Target Node Drain & Maintenance Mode: Ability to place agent and local targets into maintenance mode ("Drain"), seamlessly removing their containers from active proxy load balancing pools while permitting in-flight connections to drain gracefully.
- Admin API Target Drain Endpoints: `POST /api/targets/{id}/drain` and `POST /api/targets/{id}/undrain` registered in OpenAPI specifications with dedicated audit logging (`AuditAction::DrainTarget`, `AuditAction::UndrainTarget`).
- WebUI Node Drain Controls & Drained Status Badge: One-click "Drain" and "Undrain" maintenance toggles with confirmation dialogs on the Targets page, alongside a visual purple "Drained" status badge for affected nodes.

## [2.2.4] - 2026-10-21

### Added
- Centralized Backups Center: Dedicated control plane (`/backups`) consolidating database and volume backups, storage metrics, schedules, and disaster recovery across all engines in a unified management dashboard.
- Dead Man's Snitch & Heartbeat Monitoring: Automated HTTP GET ping (`r3v3rs3-heartbeat`) to external snitch monitoring endpoints (Healthchecks.io, Dead Man's Snitch, Better Stack) upon successful daily scheduled backup completion, with outbound `backup.failure` alerts on failures.
- Unified Backups Overview & Metrics: 4 real-time KPI indicators displaying total backups, cumulative storage consumed, latest backup timestamp, automated cron schedule, heartbeat status, and remote S3 backup status.
- Engine Filtering & Search: Instant text filtering by database name or backup ID, combined with database engine dropdown filters (PostgreSQL, MySQL, MariaDB, MongoDB, Redis).
- On-Demand Backup & Verification Actions: Quick modal to trigger immediate backups for any selected database, alongside in-place restore and one-click decompression integrity tests.
- Admin API Backups Endpoints: `GET /api/backups`, `GET /api/backups/overview`, and `POST /api/backups/run` registered in OpenAPI specifications.

## [2.2.3] - 2026-10-21

### Added
- Centralized Outbound Webhooks & Event Notification Center: Dedicated outbound webhook hub to stream real-time events to external monitoring and automation services (Slack, Discord, n8n, Datadog, PagerDuty, or custom HTTP endpoints).
- Granular Event Subscriptions: Support for 14 event types across 4 distinct categories (Deployments: `deploy.success`, `deploy.failure`, `deploy.pending_approval`, `app.crashed`; Databases & Backups: `backup.success`, `backup.failure`, `backup.upload_failed`, `backup.restored`; Security & Certificates: `cert.expiring`, `cert.renewed`, `cert.renewal_failed`, `security.ip_banned`; System & Nodes: `target.offline`, `system.disk_pressure`; plus `test.ping`).
- Industry-Standard Payload & Delivery Headers: Structured JSON payload schema with UUID identifiers, Unix and ISO timestamps, human-readable summaries, and standard headers (`X-R3v3rs3-Event`, `X-R3v3rs3-Delivery`, and `X-R3v3rs3-Signature: sha256=<hex>`).
- Sealed Secret Storage & HMAC-SHA256 Signing: Webhook signing secrets are encrypted at rest with AES-256-GCM via `platform.key` and re-sealed automatically upon master key rotation.
- WebUI Webhooks Dashboard: New dedicated management page under Administration (`/webhooks`) featuring categorized event selection chips ("Select All" / "Deselect All"), secret status badges, custom header configuration, and instant test ping delivery.
- Admin API Endpoints: 6 REST endpoints under `/api/webhooks` for CRUD operations and live test ping triggering.

## [2.2.2] - 2026-10-21

### Added
- Remote S3 Backup Browser & Disaster Recovery: Interactive remote backup snapshot browser directly querying external S3/R2 storage buckets using SigV4 AWS-compliant `ListObjectsV2` and `GetObject`.
- Database Disaster Recovery: Ability to restore a remote S3 backup into a newly created isolated database container or in-place into an existing database with a single click.
- Direct S3 Snapshot Downloads: Stream and download backup archives (`.dump.gz`) from remote S3 buckets through the authenticated admin API proxy.
- Admin API S3 Endpoints: `GET /databases/s3/backups` (remote snapshot listing and filtering), `GET /databases/s3/download` (direct download streaming), and `POST /databases/s3/restore` (disaster recovery and in-place restore).
- WebUI Snapshot Drawers: Integrated S3 disaster recovery browser panel in the main Databases dashboard and a remote snapshot table in the Database Backups detail view.

### Fixed
- Duplicate Update Banner: Eliminated duplicate version update notification on the dashboard by removing redundant local page rendering and retaining the global layout banner.

## [2.2.1] - 2026-10-21

### Added
- Visual Docker Compose Editor: Integrated visual Compose YAML editor with real-time YAML syntax validation, line numbering, and error gutter markers powered by client-side `serde_yaml`.
- Environment Variable & Secret Auto-Completion: Quick-insert chips and completion for environment variables (`${VAR_NAME}`) and encrypted secrets across application, project environment, and shared environment groups.
- One-Click Architectural Templates: Built-in scaffolding templates for Nginx Web Server, Node.js + PostgreSQL, Python + Redis, and WordPress + MySQL stacks.
- Automatic Service Detection: Real-time parsing of declared services under `services:`, with clickable pills to select and designate the primary routing service.
- Inline Compose Deployment Support: Apps can now be created and deployed directly from inline Compose YAML (`compose_yaml` in `AppSource::Compose`) without requiring an external Git repository.
- Interactive Syntax Preview Mode: Tokenized visual preview styling YAML keys, comments, string values, and variable interpolations.

## [2.2.0] - 2026-10-21

### Added
- Project & Environment Stacking: Hierarchical organization of platform applications and databases under Projects (e.g. "E-Commerce") and isolated deployment Environments (e.g. "Production", "Staging", "Development").
- Environment-level Shared Variables: Secure, encrypted shared environment variables and secrets defined per environment, automatically inherited by all member applications during deployment.
- Hierarchical Environment Variable Resolution: Strict 5-tier inheritance precedence during app deployment: Git config < Env groups < Environment shared variables < App-level overrides < Database connection variables.
- Dedicated WebUI Management: New "Projects" page under Platform menu for project/environment CRUD, variable management, and filtered navigation.
- Project & Environment Scoping: Added project and environment badges and filters to Apps and Databases dashboards, plus selection fields in application creation and edit forms.
- Admin API: 11 new REST endpoints under `/api/projects` for project, environment, and environment variable lifecycle management.

## [2.1.10] - 2026-10-21

### Added
- Extended Prometheus `/api/metrics` endpoint instrumentation: network bandwidth throughput counters (`r3v3rs3_rx_bytes_total`, `r3v3rs3_tx_bytes_total`), HTTP response status class counters (`r3v3rs3_http_responses_by_status_total`), TLS handshake latency histograms and quantiles (`r3v3rs3_tls_handshake_duration_seconds`, `r3v3rs3_tls_handshake_duration_quantiles_seconds`), TLS handshake error counters (`r3v3rs3_tls_handshake_errors_total`), and cache bypass tracking (`r3v3rs3_cache_bypasses_total`).
- Dedicated ready-to-import Grafana dashboard endpoint (`GET /api/metrics/grafana`) returning a comprehensive 5-section observability dashboard for throughput, HTTP status distribution, latency histograms, TLS timings, upstream errors, and system health.
- WebUI shortcut: added direct "Grafana Dashboard" download button to the Live Traffic page.

## [2.1.9] - 2026-10-21

### Added
- Real-time live access log streaming and interactive traffic inspection dashboard in WebUI with color-coded HTTP status badges (`2xx`, `3xx`, `4xx`, `5xx`), latency metrics (<50ms, 50-300ms, >300ms), and multi-criteria filters (proxy, client IP, HTTP status, search keyword).
- Live Server-Sent Events (SSE) streaming endpoint (`GET /api/traffic/live`) with real-time push of access log entries matching client filters.
- Recent in-memory circular access log buffer endpoint (`GET /api/traffic`) with query parameters and proxy scope authorization.
- Instant proxy traffic shortcut: added "View Traffic" navigation button to proxy overview page.

### Changed
- CI: fixed test-gate workflow job boolean logic to accurately detect successful Rust runs on release commits and skip redundant test execution.

## [2.1.8] - 2026-10-21

### Added
- Native gRPC and gRPC-Web proxy routing and protocol translation mode (`grpc_web`) for HTTP/HTTPS proxies, individual routes, and platform applications.
- Automated browser CORS preflight handling for gRPC-Web requests: automatically responds to `OPTIONS` preflight with required `Access-Control-Allow-Origin`, `Access-Control-Allow-Methods`, `Access-Control-Allow-Headers` (`content-type,x-grpc-web,x-user-agent`), and `Access-Control-Expose-Headers` (`grpc-status,grpc-message`).
- Browser request transpilation: converts incoming `application/grpc-web` and `application/grpc-web+proto` requests to upstream `application/grpc` and ensures `TE: trailers` is propagated.
- End-of-stream trailer framing: encodes upstream gRPC trailer metadata (`grpc-status`, `grpc-message`) into standard gRPC-Web trailer frames (flag byte `0x80` + 4-byte big-endian length + headers) appended to the HTTP response stream.
- Automatic HTTP/2 prior knowledge (`h2c`) injection for plain HTTP upstream gRPC services.
- WebUI toggles for gRPC-Web on proxy creation/editing, route settings, and platform application forms.

## [2.1.7] - 2026-10-21

### Added
- Automated AI scraper and crawler bot protection switch (`bot_blocker`) for HTTP/HTTPS proxies, individual routes, and platform applications.
- Zero-allocation streaming User-Agent matcher blocking commercial AI training crawlers and aggressive data scrapers: GPTBot, ChatGPT-User, OAI-SearchBot, ClaudeBot, Claude-Web, anthropic-ai, Bytespider, CCBot, PerplexityBot, cohere-ai, Diffbot, FacebookBot, Meta-ExternalAgent, Google-Extended, Amazonbot, Scrapy, and SemrushBot.
- Granular per-route overrides: selectively enable or disable bot blocking on public API endpoints or sensitive web routes.
- WebUI toggles for bot protection on proxy creation/edit, route options, and application forms.

## [2.1.6] - 2026-10-21

### Added
- Disaster recovery encrypted platform snapshot bundles (`.r3bundle`): complete zero-knowledge encrypted backups capturing SQLite databases (`platform.db`), sealing keys (`platform.key`, `keys/*.key`), TLS certificates (`certs/`), accounts (`accounts.toml`), and server configuration (`*.toml`).
- Passphrase-based AES-256-GCM encryption with 10,000-round PBKDF2/SHA-256 key derivation.
- Automatic and manual S3 disaster recovery replication for off-site disaster recovery storage.
- Single-command disaster recovery CLI commands: `r3v3rs3 disaster-recovery backup` and `r3v3rs3 disaster-recovery restore` for instant bare-metal restoration on fresh servers.
- WebUI disaster recovery manager: interactive snapshot browser, encrypted creation dialog, download links, one-click state restore, and direct bundle upload & restore.

## [2.1.5] - 2026-10-21

### Added
- Persistent volume backups and snapshots: create point-in-time `.tar.gz` snapshots of container persistent volume directories via Docker Archive streaming API with zero external dependencies.
- Snapshot restore and deletion: restore historical volume snapshots or delete old snapshots directly from WebUI and REST API.
- Direct archive export and import: download live volume contents as `.tar.gz` archives or import/extract uploaded `.tar.gz` archives into existing container volumes.
- Automatic S3 cloud replication: snapshot backups automatically upload to S3 backup buckets when S3 storage is configured.
- WebUI volume snapshot manager: interactive snapshot browser, archive export/import buttons, and live restore confirmations on the app file manager page.

## [2.1.4] - 2026-10-21

### Added
- BuildKit remote builder daemon support: configure remote BuildKit endpoints (`buildkit.remote_addr`) to offload heavy compilation from the host server.
- Automatic cache mount injection for Git builds: inject `# syntax=docker/dockerfile:1` and `--mount=type=cache` presets for Cargo, NPM, PNPM, Yarn, Go, Pip, Maven, and Gradle into Dockerfile `RUN` steps without manual Dockerfile modifications.
- Custom cache mounts and external cache backends (`cache_from` and `cache_to`) for multi-node and CI layer sharing.

## [2.1.3] - 2026-10-21

### Added
- Webhook-based container registry automatic deployments: automated payload parsing and authentication for Docker Hub (`push_data`), GHCR / GitHub Packages (`package` and `registry_package`), Harbor (`PUSH_ARTIFACT`), Quay (`updated_tags`), and generic image webhooks with HMAC-SHA256, Bearer token, or `?secret=` query parameter.
- Tag matching and tag pattern triggers for image apps: automatic zero-downtime blue-green redeployments when the pushed tag matches the app's image or configured `tag_trigger` pattern.

## [2.1.2] - 2026-10-21

### Added
- Ephemeral preview deployments for pull requests: automatically provision isolated environments on dedicated subdomains upon PR open/reopen, update deployments on synchronize events, and destroy resources when PRs are closed.

## [2.1.1] - 2026-10-21

### Added
- One-click database web management UIs: pre-configured catalog templates in App Store for pgAdmin 4, phpMyAdmin, Redis Commander, Adminer, and Mongo Express, with direct Web UI launch shortcuts on the Databases page.

## [2.1.0] - 2026-10-21

### Added
- Point-in-time recovery (PITR) and continuous WAL archiving: Write-Ahead Log rotation and archiving for PostgreSQL, MySQL, MongoDB, and Redis with precise second-level replay and non-destructive PITR branch cloning.
- Database read replicas and read traffic load balancing: one-click read replica creation with PostgreSQL streaming replication, MySQL binary replication, and automated upstream load balancing across read instances.
- Container resource limits and vertical scaling: configurable CPU quota, RAM limits, swap limits, and OOM killer protection for deployed apps and managed databases.
- Database cloning and branching: one-click staging branch creation from running database containers or verified backup dumps with isolated volume and port provisioning.
- DNS-based upstream load balancing: resolve upstream hostnames to multiple IP addresses with round-robin traffic distribution and TTL-aware caching.
- Compose service health and per-service log viewer: real-time container health monitoring and filtered log streaming for individual services within Docker Compose applications.
- Shared environment variable groups: define reusable environment variable collections and link them across multiple applications.
- Configurable datagram size and session timeout settings for UDP reverse proxies.
- Git-based configuration file loading: automatically read environment variables from repository `.env` or custom config files during app deployment.
- Disk hygiene prune policy: automated cleanup of dangling Docker images, stopped containers, build caches, and temporary archives.
- Self-monitoring system metrics: Prometheus-compatible instrumentation for file descriptor usage, process memory consumption, and Tokio runtime event loop lag.
- Latency histograms and upstream error counters: p50, p95, and p99 request latency distribution metrics and upstream HTTP error counters.
- GeoIP country filtering: MaxMind MMDB-backed country and continent access rule enforcement at route and proxy levels.
- Persistent scoped API tokens: generate long-lived bearer tokens with granular role-based permissions and proxy scopes.
- Per-IP and per-API-key concurrency limits: rate-limiting and connection caps to protect upstream services against abuse.
- User list file support: load basic authentication credentials from external htpasswd-style user list files.
- Route-level client certificate requirement for mTLS: enforce mutual TLS verification conditionally per route.
- Custom error pages for HTTP proxies: render custom branded HTML templates for 4xx and 5xx reverse proxy errors.

### Fixed
- System metrics test runtime and asynchronous test execution stability under Clippy.

## [2.0.5] - 2026-10-19

### Added
- Built-in OIDC and OAuth2 authentication: authenticate users via external OpenID Connect and OAuth2 identity providers (Google, GitHub, Authentik, Keycloak, etc.) alongside local credentials.
- Least connections (`least_conn`) load balancing and replica sticky sessions: route requests to the upstream with the fewest active connections, and support cookie-based sticky sessions across app replicas and agent targets.
- Stale-while-revalidate caching and instant purge endpoint: serve stale cache entries while asynchronously revalidating in the background, with an API endpoint and CLI command to purge cached routes on demand.
- Maintenance mode for proxies: one-click maintenance mode toggle returning custom 503 pages while preserving underlying proxy configuration.
- App store catalog enhancement: categories, real-time search, filtering, and screenshot previews in the one-click application catalog.
- Configuration backup and restore: export and import full r3v3rs3 configuration archives (ports, proxies, access lists, accounts) via API, CLI, and WebUI.
- App resource alert rules: automated CPU and memory threshold monitoring with notifications when app containers exceed configured resource limits.

### Fixed
- WebAuthn RP ID resolution: resolve WebAuthn `rp.id` taking `X-Forwarded-Host` into account when the admin panel is accessed through reverse proxy domains.
- Unified catalog refresh buttons: align styling, behavior, and status indicators across catalog refresh actions in the WebUI.

## [2.0.4] - 2026-10-18

### Added
- Remote agent self-upgrade: master pushes the latest `r3v3rs3` executable binary to target agent hosts upon version difference or on demand, with automatic disk replacement and graceful systemd restart.
- Multi-target replica deployments: run app replicas across multiple agent targets and local target with unified master reverse proxy load balancing (`round_robin`, `client_ip_hash`, `random`, `first`).
- System dashboard: single-screen overview page at `/dashboard` summarizing ports, proxies, apps, databases, targets, disk storage space, and available updates.
- Private container registry credentials: encrypted and sealed (`platform.key`) credentials storage with authenticated pull (`X-Registry-Auth`) support for Docker Hub, GHCR, GitLab, ECR, and private registries.

## [2.0.3] - 2026-10-18

### Added
- WebAuthn and passkey admin login: hardware security keys, Touch ID, Face ID, and biometric device passkey authentication (FIDO2 / ES256 P-256) alongside passwords and TOTP, with resident key support, replay protection counters, and management in the WebUI.
- platform.key rotation workflow: POST /api/platform/key/rotate, CLI r3v3rs3-cli platform rotate-key, and panel settings button to generate a new 256-bit AES encryption key, re-encrypt all stored SQLite secrets, and archive the previous key to platform.key.old.
- Backup SFTP and rsync targets: upload database backup dumps directly to remote SFTP servers with private key or password auth, or sync via rsync over SSH.
- Backup restore verification testing: automated periodic decompression and engine magic byte signature verification (Redis RDB, SQLite 3, PostgreSQL, MySQL/MariaDB) to ensure backups remain uncorrupted and restorable, with on-demand verification API.
- ACME multi-CA and EAB support: ZeroSSL, BuyPass, and custom ACME directory providers with optional External Account Binding (EAB Key ID and HMAC Key) credentials.

### Fixed
- Calibrated fail2ban brute-force protection threshold beyond single-username max attempts to prevent lockouts on targeted username sprays.

## [2.0.2] - 2026-10-18

### Added
- Login brute-force protection: fail2ban-style security layer that temporarily or permanently bans IP addresses after repeated failed login attempts, with an admin management API and WebUI security panel.
- App store catalog update detection: installed apps display a badge when a newer version exists in the catalog, with one-click upgrade.
- Audit log search and management: text search and action type filtering in the panel, CSV export, and configurable log retention days.
- Proxy rule dry run: simulation tool in the proxy form that tests sample requests against unsaved rules to preview route matches, upstream URLs, and redirects.
- Full CLI management: direct subcommands for ports, proxies, apps, and backups in `r3v3rs3-cli`, with OAuth Bearer token authentication on the admin API.
- Declarative CLI apply: `r3v3rs3-cli apply -f` command supporting YAML and JSON manifests for scripted and idempotent infrastructure management.

## [2.0.1] - 2026-10-18

### Added
- Bulk operations: multi-select on apps, ports, and proxies with batch delete and batch deploy.
- Image update checker: periodic check for newer tags of image-based apps with auto-deploy option.
- Scheduled container commands: cron-scheduled commands inside app containers with execution logs.
- Live log tailing: WebSocket log streaming with live search, level filtering, and raw log download.
- Deployment approval and maintenance window: optional manual approval gate before applying deployments and maintenance window time restrictions.
- Git tag triggers: automatic deployments triggered on Git tag push matching glob patterns.
- OAuth 2.1 login redirect: unauthenticated authorization requests to `/oauth/authorize` automatically redirect to the sign-in page with return URL preserved.
- Redesigned OAuth consent screen: modern, responsive authorization consent page with dark and light mode support.

### Fixed
- Fixed return path routing for app store and database views.
- Fixed database and backup action feedback toast notifications.

## [2.0.0] - 2026-10-17

### Added
- Managed databases: the Databases page creates PostgreSQL, MariaDB, MySQL, MongoDB or Redis on the local target with one data volume, a generated password and a fixed `127.0.0.1` port, and deletes the app while the data volume stays.
- App to database connections: the app form picks a database, and the deployment injects `DATABASE_URL` plus the engine variables into the container environment. A database that an app uses cannot be deleted.
- Database backups: one-off or daily scheduled dumps per database, gzip with a SHA-256 manifest, a per-database retention count, and an optional S3 upload with a sealed secret key. A backup restores back into the running container.
- App metrics: the panel shows the CPU, memory, network and request history of an app of the last 24 hours, served by `GET /api/apps/{id}/metrics`.
- Web terminal: the app page opens a shell into the running container over a WebSocket bridge to the Docker exec API.
- App files: `GET`, `PUT` and `DELETE /api/apps/{id}/files` read, write and delete files inside a writable volume, and the panel edits them.
- App Store Compose entries: catalog entries that install a multi-container app from a Compose file, with an environment variable template that the panel asks for before the first deployment.
- The app store catalog and the database engine catalog now live in the [r3v3rs3-apps](https://github.com/KilimcininKorOglu/r3v3rs3-apps) repository. The binary ships embedded copies, refreshes both once a day, and the Databases page has a refresh button plus `POST /api/databases/engines/refresh`.

### Changed
- The engine catalog is data driven: the engine of a database is a catalog key instead of a compiled-in enum, so a new engine in `engines.json` needs no release.
- An app can publish a port on a fixed host port, and a database keeps its port across deployments; the old container stops before the new one binds it, so such apps deploy without a blue-green switch.

### Fixed
- The request counter of a discovered proxy now counts under the resource of the proxy instead of the port id.
- A container file operation no longer fails with `the runtime does not carry the files of a container` on the local Docker runtime.

## [1.8.9] - 2026-10-16

### Added
- An app store that lists the image apps of a catalog and installs one in a click. The catalog lives in the repository, ships embedded in the binary, refreshes from the repository once a day, and a panel button refreshes it at once. Creating an app from the store picks the deployment target and starts the first deployment.

### Changed
- The platform documentation describes the app store and its `GET /api/store` and `POST /api/store/refresh` endpoints.

## [1.8.8] - 2026-10-16

### Added
- MCP clients can register themselves through the RFC 7591 dynamic client registration endpoint.

### Changed
- The public URL is one top-level setting that the settings page manages for every module.
- The public address is its own section at the top of the settings page.
- The project ships a binary and the `install.sh` installer only. It builds and publishes no Docker images anymore.

### Removed
- Cluster mode: the multi-node state sharing, the leader election, the `KvStorage`, the `[cluster]` configuration section and the cluster end-to-end tests. The service discovery from etcd stays.
- The Kubernetes discovery provider, its configuration and its k3s tests.
- The Consul discovery provider, its configuration and its tests. r3v3rs3 discovers services from Docker labels and etcd.

## [1.8.7] - 2026-10-15

### Changed
- The Docker image build uses cargo-chef, so a code-only change rebuilds the workspace crates without recompiling the dependencies.
- The Docker workflow builds the `runtime` and `platform` targets of a platform sequentially on the same runner, so the second build reads the local BuildKit cache instead of the Actions cache.
- The musl archives of the command line client build in the same matrix as the macOS and Windows archives, with cargo zigbuild, instead of serializing the Linux release build.
- The crates publishing skips the local verification build, because the tests already ran in the same release.
- The `web-release` profile drops full LTO and `codegen-units = 1`, which shortens the WebUI build at a small bundle size cost.
- Every branch writes the cargo caches, and clippy keeps its own cache entry, so the lint job starts warm.

## [1.8.6] - 2026-10-15

### Changed
- The app list opens the app domains in a new tab, and the certificate list opens the server certificate domains in the same way. A wildcard, an IPv4 address or a name that carries a port stays plain text.
- The CI runs the normal tests and the Docker runtime tests in two parallel jobs, and the three build jobs share one cargo cache entry, so every run rebuilds less.

## [1.8.5] - 2026-10-15

### Changed
- The proxy list opens the domains of an HTTP proxy in a new tab. A wildcard name stays plain text.
- The Service Discovery card shows the manual proxy count on the same line as the provider state.
- The release workflow closes the release even when the crates of the version went out in an earlier publish attempt.

## [1.8.4] - 2026-10-15

### Changed
- The release ships one glibc binary per architecture again: cargo zigbuild now targets glibc 2.17, so the same archive runs on the older distributions, for example Rocky 8 or Debian 10, that the glibc 2.35 build could not serve. The separate musl archives of the server are gone.
- The command line client is `r3v3rs3-cli` now, with the `install-cli.sh` and `install-cli.ps1` installers. An installed `r3v3rs3-mcp` keeps running; install the new name over it.
- `install.sh` installs `git`, `nano`, Docker and the compose plugin when they are missing, because the deployment platform needs them.
- `install.sh` picks the static build on a system whose glibc is older than 2.35.

### Fixed
- The docs code fences use the languages that Zola knows, so the site build shows no highlight warnings.

## [1.8.3] - 2026-10-15

### Changed
- The proxy list shows the host names of a proxy instead of its port addresses, and the service discovery card names its number as the discovered proxies and adds the count of the manual proxies next to it.

## [1.8.2] - 2026-10-15

### Fixed
- The CDN IP ranges card and the log view rendered their times in UTC instead of the display time zone of the settings page. Every time that the panel shows now follows the setting.

## [1.8.1] - 2026-10-14

### Added
- An app selects the ports that serve it in the app form: a checkbox list of the ports plus a free text field for extra ones. An app without a selection takes the ports of the platform setting, or every HTTP and HTTPS port when that setting is also empty, so a deployment no longer answers 502 because of an empty setting.

### Changed
- The webhooks that could not install because the platform had no public address install themselves when the address is set, and a changed address reinstalls every webhook, because the providers send the push events to the old one.

## [1.8.0] - 2026-10-14

### Added
- The times that the WebUI shows follow a configurable display time zone. The zone is stored in the server config under `admin.display_timezone` (the default stays UTC), the settings page offers a fixed list of IANA zones, and the session carries the zone so every rendered time follows it without a reload.

### Changed
- The CI workflows spend fewer runner minutes: the `Rust` workflow no longer runs a redundant full build step and only saves its caches from `main`, the release workflow creates the draft release in its own job so the binary jobs start without waiting for the WebUI build, and the Docker workflow builds the platform image after the runtime image so it can reuse its cache.

### Fixed
- The session endpoint answer of the accounts API test now expects the `display_timezone` field that the session carries since 1.8.0.

## [1.7.9] - 2026-10-14

### Fixed
- The panel bounced to its own sign-in page in the middle of the Create GitHub App flow: the session checks of the single page application treated a request that died without an answer as a sign-out, and the browser aborts the in-flight requests of a page that starts navigating to GitHub. Only a real 401 answer now ends the session, a failed check leaves the state alone, and the per-page check runs once per mount instead of once per render, which also removes the bursts of session requests that the access log showed while a page was open.

## [1.7.8] - 2026-10-14

### Changed
- The language menu shows the flag with the name of the language, has a fixed width instead of collapsing to the width of a flag, and the flags fit their box without cropping. Verified in Firefox.

## [1.7.7] - 2026-10-14

### Fixed
- The order of an ACME entry that references a DNS credential failed with `an ACME entry names a dns provider and a dns credential, not both`, because the resolved entry kept the reference next to the provider. The resolution now clears the reference, and a regression test runs the resolution and the validation of the entry.

### Changed
- Every certificate list is its own page: `/certs`, `/certs/client`, `/certs/root` and `/certs/acme`. The sidebar opens those routes directly, and the upload, self-sign and new ACME pages return to the route of the list they belong to.

## [1.7.6] - 2026-10-13

### Fixed
- The first visit of the DNS credentials page on a server without `dns-credentials.toml` failed with `failed_to_load_dns_credentials`. The reader now treats the missing file as an empty store.

## [1.7.5] - 2026-10-13

### Added
- Named DNS credentials: the panel stores the credentials of every DNS provider under a name, several per provider, and an ACME entry references one for the DNS-01 challenge instead of carrying its own provider credentials. The admin API never returns the stored credentials, r3v3rs3 seals them with the key in `platform.key`, and a credential that an ACME entry still references cannot be deleted. Entries with an inline provider keep working.
- The DNS Credentials page below Certificates lists the stored credentials and adds or edits them. The ACME forms pick a stored credential or keep the manual entry, and the ACME tab names the credential of an entry in its provider cell.
- The install scripts of the command line MCP client add the install directory to `PATH` themselves.

### Fixed
- The language flags of the WebUI rendered as narrow strips in Firefox, because Firefox sizes an SVG inside an img from the attributes of the SVG file. The flags now carry fixed width and height attributes and crop themselves with `preserveAspectRatio`.

### Changed
- The docs describe the DNS credentials store in the configuration reference.

## [1.7.4] - 2026-10-13

### Added
- `r3v3rs3-mcp update` installs a newer release of the command line MCP client over the running binary and verifies the sha256 digest of the release.

### Changed
- The command line MCP client is distributed from the GitHub releases only. The release workflow builds it for macOS and Windows besides Linux, and `install-mcp.sh` plus `install-mcp.ps1` install the newest release with its sha256 verification. The npm package and the crates.io publish of the client are gone, so a release needs no registry secret.
- The expiry and renewal dates on the certificates page name the exact days left instead of the largest whole unit, so 59 days no longer read as 1 month.

## [1.7.3] - 2026-10-12

### Fixed
- The certificates page showed its four tabs twice: once in the sidebar submenu and once inside the page. The in-page tabs are gone, and the tab follows the link of the sidebar directly, so a sidebar click switches the tab while the page is already open.

## [1.7.2] - 2026-10-12

### Added
- `GET /api/upgrade/status` reports the step of the server upgrade that runs at the moment.
- While the in-panel upgrade runs, a blocking dialog covers the panel and shows the current step. The panel reloads onto the new version when the upgrade finishes.
- The link of the update banner opens the settings page on the Server Upgrade card instead of at the top of the page.

## [1.7.1] - 2026-10-11

### Fixed
- The release 1.7.0 build failed on the dialog text of the ACME "Order again" action, so no binaries, images or crates were published for it. This release carries the fix and everything of 1.7.0.
- The settings page named the platform toggle "Deploy apps on this server", although the platform also deploys to agent targets. It now reads "Enable the deployment feature" and its hint names the agent targets.

### Changed
- The IP filter hints of the access list and proxy forms show an example of a mixed comma separated list of single addresses and CIDR blocks.

## [1.7.0] - 2026-10-11

### Added
- "Order again" in the ACME tab of the certificates page, and `POST /api/acme/{id}/order`, order the certificates of an ACME entry at once without waiting for the renewal time or the one hour retry. The action is recorded as `request_acme` in the audit log.
- The four tabs of the certificates page (server, client, root and ACME) appear as a submenu below Certificates in the sidebar.
- The docs site publishes `llms-full.txt`, adds a `WebSite` JSON-LD document to the home pages, and submits itself to IndexNow after every deploy.

### Changed
- The README overview, the docs feature list and every product tagline now list the complete feature set, including the HTTP/2 and HTTP/3 flood limits, the Prometheus metrics endpoint, the MCP server, the worker process mode and the zero-downtime upgrade.

## [1.6.26] - 2026-10-11

### Added
- The server checks for a newer release every hour and shows a banner on every page of the panel while one exists. The `upgrade.check_interval` setting, managed live from the settings page, sets the cadence.

## [1.6.25] - 2026-10-11

### Fixed
- Toggling the deployment platform on the settings page hides or shows the platform group of the sidebar without a reload: the config update event now refreshes the flag that the sidebar reads.

### Changed
- The release builds the musl binaries with cargo zigbuild on one runner instead of two Alpine containers, caches the WebUI build, and links with thin LTO, so a release finishes sooner.

## [1.6.24] - 2026-10-11

### Fixed
- A reload of a panel page keeps the session: the sign-in cookie carried no path and the browser scoped it to /api, so the cookie was missing from the page request that the reload made.

## [1.6.23] - 2026-10-11

### Changed
- The config test of the settings follows the exec allowlist that the settings page manages.

## [1.6.22] - 2026-10-11

### Added
- The settings page manages the MCP endpoint and its clients, and the allowlist of the exec DNS provider; both apply while the server runs.
- A fresh start writes `config.toml` with every setting and its default value when the file is missing.

### Fixed
- The in-panel upgrade installs again: the verification compared the installed binary with the digest of the compressed archive, and it compares the binary with the bytes that the extraction produced now.

## [1.6.21] - 2026-10-11

### Added
- The settings page turns the deployment platform on and off while the server runs: the platform section travels with the settings of the panel, the sidebar hides the platform pages while it is off, and the app information carries the state.
- The settings page of the panel and this document describe the platform card.

### Fixed
- A signed-out navigation to a panel page renders nothing: the server answers the shell of a WebUI route with a redirect to the sign-in page, and the SPA draws no page outside it.

## [1.6.20] - 2026-10-11

### Fixed
- The in-panel upgrade writes the new binary to the disk again: the archive is extracted into a staging directory, because GNU tar refuses to replace an existing file when the member is named on the command line, and the installed binary is verified against the release digest before the upgrade reports success.

## [1.6.19] - 2026-10-11

### Added
- A regression test proves that the archive download of the in-panel upgrade follows the redirect that GitHub answers.

## [1.6.18] - 2026-10-11

### Fixed
- The in-panel upgrade installs the new release again: the archive download of GitHub answers with a redirect, and the upgrade follows it now. Every failed upgrade step also logs its cause.

## [1.6.17] - 2026-10-11

### Changed
- The auth gate of the panel stays within the complexity limit: its redirect lives in a hook of its own.

## [1.6.16] - 2026-10-11

### Added
- The docs describe the in-panel upgrade, which the previous release added with the "Server Upgrade" card of the settings page.

## [1.6.15] - 2026-10-11

### Fixed
- A fresh sign-in reaches the panel again: the signed-out redirect rechecks the session before it sends a route back to the sign-in page.

## [1.6.14] - 2026-10-11

### Changed
- The WebUI reads the router hooks of every page without `unwrap`, so a component that renders outside the router context fails gracefully instead of panicking on the first render.

## [1.6.13] - 2026-10-11

### Added
- The settings page of the panel carries a Server Upgrade card: an admin clicks the button, the server reads the latest GitHub release, verifies its sha256 digest, replaces its binary and hands the listeners to the new process without dropping the connections.
- The audit log records the server upgrades.

## [1.6.12] - 2026-10-11

### Fixed
- A process that started from an upgrade hands its inherited listeners to the next one again, so every `systemctl reload` upgrades and none needs a restart in between.
- The router of the WebUI is read without `unwrap` in the auth gate.

## [1.6.11] - 2026-10-11

### Fixed
- The WebUI panics no more on the first render: the auth gate reads the router inside the router context that wraps it.

## [1.6.10] - 2026-10-11

### Fixed
- A zero-downtime upgrade starts the new process again after the binary was replaced on disk: the kernel reports the old inode as `<path> (deleted)` and the old path made the spawn fail.

## [1.6.9] - 2026-10-11

### Fixed
- The zero-downtime upgrade also hands the listener of the WebUI to the new process, so the panel keeps serving an upgrade of a server whose only listener is the panel, and the new process never races the old one for the WebUI port.

## [1.6.8] - 2026-10-11

### Fixed
- The zero-downtime upgrade works again: the handover passes the descriptors before the handshake frame, so a read no longer drops them, a failed handover keeps the old process serving, and the server tells systemd `READY=1` and hands the main process over with `MAINPID`, which `install.sh` now writes as `Type=notify` with a reload that upgrades in place.
- The sign-in page opens without flashing the target page first.

### Added
- A fresh `install.sh` run creates the `admin` account as its last step, with a `--password` option or a generated random password that the summary prints once, and opens the WebUI port in the local firewall when the address binds beyond loopback, through firewalld, ufw or an iptables accept rule with persistence.

## [1.6.7] - 2026-10-11

### Added
- A fresh `install.sh` run creates the `admin` account as its last step, with a `--password` option or a generated random password that the summary prints once, and opens the WebUI port in the local firewall when the address binds beyond loopback, through firewalld, ufw or an iptables accept rule with persistence.

### Fixed
- The WebUI shows the sign-in page without flashing the target page first: the app checks the session before it renders any page.

## [1.6.6] - 2026-10-11

### Changed
- The Docker image builds again: the Dockerfile copies the `r3v3rs3-mcp` workspace member, and the builder stage installs `cmake`, `perl` and `libclang-dev` for the aws-lc-sys build.
- The static binary check of the musl release jobs accepts a static PIE binary, whose `file` output names `static-pie linked` instead of `statically linked`.
- The version examples of the tutorials name 1.6.6.

## [1.6.5] - 2026-10-10

### Added
- Prometheus metrics at `GET /api/metrics`: proxied HTTP and HTTP/3 requests, upstream errors, cache hits and misses, and open connections, authorized by an admin session or an MCP Bearer token.
- The client IP resolution reads the RFC 7239 `Forwarded` header, including quoted IPv6 addresses with a port.
- Statically linked musl binaries for x86_64 and aarch64, and `install.sh` picks the musl build on a musl system and falls back to the glibc build.

### Changed
- The HTTP/2 and HTTP/3 flood limits are configurable: the `[http2]` and `[http3]` config sections hold the defaults, a port can override them with its own options, and the WebUI forms expose both.
- The TLS crypto provider is aws-lc-rs, so a TLS handshake negotiates the post-quantum X25519MLKEM768 hybrid.
- The version examples of the tutorials name 1.6.5.
- The docs document the limits, the metrics endpoint and the Docker upgrade behavior.

## [1.6.0] - 2026-10-10

### Added
- Worker processes: the listeners of a master can run in a supervised worker process, with IPC descriptor passing, a worker event bridge into the master event stream, and a worker status report through the admin API and the WebUI.
- Zero-downtime upgrade: SIGUSR2 hands the listeners to a new process without dropping connections.
- Graceful shutdown: the open connections drain before the server stops, and the timeout is configurable in the settings.
- MCP server: the admin API tools are served over MCP at `/api/mcp`.
- OAuth 2.1 authorization server for MCP clients, with PKCE, a WebUI consent form and refresh token rotation.
- A command line MCP client (`r3v3rs3-mcp`) with login, tools and call commands.
- The CDN fetch downloads the Google Cloud load balancer IP ranges.

### Changed
- The version examples of the tutorials name 1.6.0.
- The release checksums are signed with minisign.
- The Docker test images are pulled from the GHCR mirror and the mirror images from mirror.gcr.io.
- The docs document the worker processes, the zero-downtime upgrade, the MCP server and the MCP client.
- Functions that exceed the complexity limit are split into smaller functions.

## [1.5.6] - 2026-10-09

### Changed
- The version examples of the tutorials name 1.5.6.
- The docs site serves the Merriweather Sans font itself instead of Google Fonts.

### Fixed
- The Docker image builds with the Rust compiler that `rust-toolchain.toml` pins instead of the latest stable.
- An uploaded certificate chain names its last certificate as the root certificate again.
- The unmaintained `rustls-pemfile` crate is replaced by the PEM parser of `rustls-pki-types`.

## [1.5.5] - 2026-09-25

### Changed
- The version examples of the tutorials name 1.5.5.

### Fixed
- An HTTP route answers 502 when the request URI for the upstream server cannot be built, for example when the percent-encoded path is longer than the URI limit. Before, the proxy sent the request with the URI of the client.

## [1.5.4] - 2026-09-25

### Changed
- The version examples of the tutorials name 1.5.4.

### Fixed
- The WebUI treats every zero gzip quality, such as `gzip;q=0.0`, as a refusal and sends the plain body.
- The proxy error page carries `Cache-Control: no-store`, so no cache in front of r3v3rs3 keeps it.
- The immutable WebUI bundle files carry no ETag, because a browser never revalidates them.

## [1.5.3] - 2026-09-25

### Changed
- The README, the docs home page, `llms.txt` and the site description name the deployment platform.
- The configuration, deployment platform, service discovery, cluster, accounts and development references are corrected and complete.
- Every tutorial uses the current WebUI labels, the route path rules, the client IP headers and the account rules of the server.
- The Docker install guide keeps the WebUI of the platform container on loopback and names the `proxy_ports` setting.

### Fixed
- `--log` and `--access-log` write the file into the directory of the given path. A bare file name still goes into the log directory.
- The platform image starts the WebUI on its loopback default. Before, it listened on `0.0.0.0:46492`, which host networking opened on every interface of the host.

## [1.5.2] - 2026-09-24

### Changed
- A deployment test on a real Docker Engine waits up to two minutes for the deployment to end, because a Compose deployment on a slow CI runner can take longer than five seconds.

## [1.5.1] - 2026-09-24

### Fixed
- An agent reports online only when its new session replaces no earlier session, also when the link of the earlier session has closed already.

## [1.5.0] - 2026-09-24

### Added
- A deployment platform that runs apps next to the reverse proxy. It has a container runtime on the Docker Engine, an app store with an admin API, and a platform discovery provider that routes the domains of the running apps.
- Image apps deploy blue-green and roll back to an earlier deployment.
- Git apps build their image from a branch before the deployment, and clone a private repository with a sealed Git token.
- Compose apps deploy from a Git branch through the `docker` binary, and a rollback checks out the recorded commit.
- A `-platform` Docker image carries `git` and the `docker` CLI with its Compose and buildx plugins.
- The admin API returns the container log of an app.
- Agent targets run apps on other hosts. An agent connects to the master over mTLS after a one-time enrollment, and the master reaches the agent apps through a tunnel. `install.sh --agent` installs an agent.
- A signed push of GitHub, GitLab, Gitea, Forgejo or a generic HMAC sender deploys an app.
- The notification webhook of the settings receives the deployment and agent events.
- Git provider connections. A GitHub connection is a GitHub App that r3v3rs3 creates from a manifest, also on GitHub Enterprise Server. GitLab and Gitea connections authorize an OAuth application. The app form lists the repositories and branches of a connection, a deployment clones with its token, and r3v3rs3 installs the push webhook of the app at the repository.
- WebUI pages for the apps, their deployments and container log, the targets and the Git providers.
- The WebUI menus moved into a left sidebar.
- The WebUI dialogs use an offline copy of SweetAlert2.

### Changed
- The Turkish WebUI texts and docs keep only real technical terms in English.
- The docs describe the deployment platform, its design decisions and its WebUI pages.
- The shared PKI test helpers moved into the common test module, and client certificate verification on HTTP/3 ports has a test.
- The app row mapper and the GitHub App tests stay within the complexity limit.

### Fixed
- The WebUI event stream, the cluster status and the version load only with a session.
- A signed-out client of a WebUI page with parameters goes to the sign-in page.
- The WebUI login fields name their autocomplete purpose.
- The empty port and proxy lists no longer name an Add button that the reader cannot see.
- The agent writes its log without color codes when the output is not a terminal.
- The master names the reason when it refuses the certificate of an agent.
- A failed webhook installation keeps its API error, so the WebUI shows it in the selected language.

## [1.0.4] - 2026-09-18

### Changed
- Every outdated dependency moved to its next major release. The upgrade covers the RustCrypto crates, rand, base64, toml, toml_edit, rcgen, x509-parser, sqlx, h3, h3-quinn, argon2, built, axum-extra, axum-server, governor and the yew family.
- `rust-toolchain.toml` pins the compiler, so CI, the Docker build and a local build all use the same rustc.
- The Rust workflow gained a lint job that runs `cargo fmt --all --check` and clippy with `-D warnings`.
- The advisory ignore of a crate that is no longer a dependency was removed from `deny.toml`.

### Fixed
- Three clippy lints that the current stable compiler reports.

## [1.0.3] - 2026-09-17

### Changed
- Every crate moved to the 2024 edition.
- Every function of the server, the API and the WebUI now stays at or below a cyclomatic complexity of 10.
- The Rust workflow fails the build when a function goes over that limit.
- The unmaintained backoff and net2 crates replaced with backon and socket2.
- A cargo-deny policy records the supply-chain decisions.
- The dependencies updated to clear the security advisories.
- The test module of the settings page moved to the end of the file.

### Fixed
- The DNS resolver moved to hickory 0.26, which drops the vulnerable release.
- Every admin API response sends no-store.
- The cache headers of the WebUI files corrected.
- The TCP form of the proxy page starts with a TCP value instead of an HTTP one.

## [1.0.2] - 2026-09-17

### Added
- Tutorials for load balancing and health checks, the admin API, Consul and etcd service discovery, team accounts, forward auth, and migration from nginx or Traefik.
- A WebSocket and gRPC section in the load balancing tutorial, and an HTTP/3 section in the HTTPS tutorial.
- Install tutorials for a Linux server and for Docker, and tutorials for Docker service discovery, wildcard certificates, application protection, caching and compression, TCP and UDP proxies, Kubernetes and high availability.
- The upstream DNS resolver setting, DNS SRV server URLs for HTTP routes, and the SRV resolution state in the proxy status.
- A Linux server installer script and a docker compose file.

### Changed
- The step by step guides live under a Tutorials section of the documentation site.
- The author email address of the crates is `k@keremgok.tr`.

### Fixed
- `add-user` replaces an account that the cluster store already has.

## [1.0.1] - 2026-09-15

### Changed
- The Docker image uses a distroless runtime and ships for `linux/amd64` and `linux/arm64`. The image is 73 MB on amd64 and 80 MB on arm64, down from 136 MB.
- GitHub releases ship Linux binaries for x86_64 and aarch64 only. armv7 binaries are no longer built.
- Release binaries and Docker images build on native amd64 and arm64 runners. A manual workflow run builds them without publishing.
- GitHub Actions use current action versions, and the docs site deploys from `main` when the docs change.
- The code and docs no longer contain macOS and Windows specific branches.

### Fixed
- The demo image builds and starts again. It uses the same Debian release as the builder and a baked admin password of at least 8 characters.

## [1.0.0] - 2026-09-15

### Added
- Account roles (`admin`, `editor`, `viewer`) with per-account proxy lists, enforced on every admin API call and in the WebUI.
- Account management through the admin API and the WebUI.
- Proxy session sign-in only for accounts that can see the proxy.
- An audit log of admin changes and sign-ins, with an audit log page.
- Fixed route responses: redirect hosts and status hosts such as a 404 host.
- Shared access lists that attach to proxies and routes.
- Certificate expiry warnings, certificate webhook notifications and bulk certificate delete.
- Cluster mode: configuration in etcd or Consul, leader election for ACME, certificate cleanup and CDN refresh, shared ACME challenges, sessions, rate limit counts and cached responses, cluster status in the WebUI, and cluster end-to-end tests.
- Encrypted cluster values, cluster key files, and KV writes, transactions, leases, locks and watches.
- DNS providers: RFC 2136 with TSIG, exec with an allowlist, webhook, Google Cloud DNS, Azure DNS, Gandi, deSEC, OVH, Linode, Vultr and Porkbun.
- ACME TLS-ALPN-01 challenge with a Pebble end-to-end test.
- ACME DNS-01 challenge with DNS provider APIs, wildcard certificates, and multiple domain names in the WebUI.
- ACME certificates for discovered hosts.
- PROXY protocol headers on ports and to TCP upstream servers.
- Traffic mirroring to shadow servers, redirect rules, route path rewrites and a request body size limit.
- Sticky sessions, configurable upstream retries, a per-server circuit breaker, server weights and weighted load balancing.
- Service discovery from Docker labels, the Consul catalog and KV store, etcd, Kubernetes Ingress resources and the `R3v3rs3Proxy` custom resource, with end-to-end tests. Discovered proxies are read-only.
- Upstream timeouts, load balancing, passive and active health checks, and upstream status.
- Client certificates: verification on TLS ports and client certificates for upstream servers.
- An OpenAPI document and a Swagger UI for the admin API.
- The server version in the WebUI footer.
- English and Turkish translations for the WebUI, the error pages, the sign-in page and the docs site, with flag language menus.
- A responsive WebUI layout and a System, Light and Dark theme selector.
- HTTP/2 to upstream servers with ALPN and h2c.
- In-memory HTTP response cache, response compression with brotli, zstd and gzip, and request and response header rules.
- Route authentication with Basic Auth, bearer tokens, forward auth and panel account sessions.
- Per-client rate limits, IP allow and deny lists, and real client IP resolution behind known CDNs and trusted proxies.
- `max_login_attempts` enforcement per client IP address and username.
- A Settings page for the app config.
- A Makefile for build, test, lint and WebUI targets.

### Changed
- GitHub releases ship only Linux binaries (x86_64, aarch64 and armv7) and the Docker image. CI tests run only on Linux.
- Turkish translations use "erişim listesi" for access lists, and the Turkish WebUI and docs texts read as native Turkish.
- Rate limiters, caches and health groups belong to each server instance.
- The etcd and Consul clients are shared between discovery and cluster mode.
- Storage write errors reach the caller.
- DNS providers are grouped without a change to their serialized form.
- Route defaults and server constructors are added.
- The changelog workflow is removed.
- The Taxy logo is replaced with the r3v3rs3 reverse-arrow mark.
- The project is imported as r3v3rs3, based on Taxy.

### Fixed
- The default database log retention shows as `3months`.
- ACME orders start only on the cluster leader.
- Proxy auth hashes are hidden in the admin API.
- A certificate download returns an error instead of a panic, and only the owner can read the private key in the archive.
- A file path without a directory returns an error instead of a panic.
- A cluster node requires a node name, and a cluster store that stops answering is detected.
- The challenge address is bound only for the reserved port.
- Accepted connections do not block the server loop.
- The server base path is joined once and keeps percent-encoding.
- Certificates are ordered without unwrapping comparisons, and PKCS#1 and SEC1 private keys are read.
- ACME requests send a User-Agent header, instant-acme 0.8.5 accepts challenges without a token, `acme.toml` is restricted to the owner, one ACME order runs per entry at a time, and unvalidatable domain names are rejected.
- Requests route to the most specific host and the longest path.
- UDP upstream replies reach the client, and the UDP and TCP server lists are rebuilt on every port setup.
- The WebUI port form keeps and edits the TLS server names.
- The CDN range snapshot and the license are packaged with the server crate, and the admin static file tests run without the WebUI bundle.
- Proxy error pages show the correct status text.
- The admin `index.html` is revalidated, so upgrades load the new WebUI.
- wasm-opt enables bulk memory and nontrapping float operations.
- An invalid upstream TLS server name returns an error instead of a panic.
- The WebUI no longer stores sessions in `localStorage`.
- The admin server loads the initial app config, and `SetConfig` persists the app config.
- The discovered proxy count shows without a plural noun, and the locale keys are sorted.

### Security
- The private key of a downloaded certificate is readable only by its owner.
- `acme.toml` is restricted to the owner.
