+++
title = "MCP and CLI"
description = "The MCP server, its tools and the r3 command line client"
weight = 10
+++

## MCP Server

The admin API exposes its complete tool suite over the Model Context Protocol at `/api/mcp` (Streamable HTTP, spec 2025-03-26). With **100% MCP Management Parity**, every single administrative feature, proxy route, port listener, ACME certificate, PaaS application, Docker Compose stack, one-click App Store catalog app, managed database, automated backup, agent target node, and disaster recovery snapshot is fully manageable autonomously via MCP without needing a human operator to open the WebUI.

The guide [MCP and the Command Line Client](@/tutorials/mcp-and-cli.md) sets up MCP, an AI client and `r3` step by step.

### Autonomous MCP Tool Suite

| Category | Tools & Operations | Description |
|---|---|---|
| **System & Monitoring** | `server_status`, `get_config`, `list_audit_log`, `get_system_metrics`, `get_system_anomalies`, `list_banned_ips`, `unban_ip`, `list_quarantined_ips`, `quarantine_ip`, `unquarantine_ip`, `get_quarantine_config`, `update_quarantine_config`, `get_geoip_status`, `update_geoip_config`, `update_geoip_database`, `get_upgrade_status`, `upgrade_server`, `get_workers`, `update_config`, `get_traffic`, `get_traffic_entry`, `get_traffic_geo`, `replay_traffic_request`, `get_anomaly_rules`, `update_anomaly_rules`, `dismiss_anomalies`, `analyze_security_headers`, `preview_proxy`, `get_cdn_status`, `refresh_cdn_ranges`, `list_discovery_status`, `get_logs`, `get_dashboard`, `check_grpc_health`, `invoke_grpc`, `reflect_grpc`, `invoke_proxy_grpc`, `reflect_proxy_grpc`, `rotate_platform_key`, `list_mcp_sessions`, `revoke_mcp_session`, `get_app_info`, `get_metrics`, `get_grafana_dashboard` | Runtime version, worker processes, masked system configuration, tamper-evident audit trail, process open FDs/memory/lag, real-time anomaly alerts, fail2ban brute-force bans, threat auto-quarantine, automated GeoIP MMDB database management, and seamless live server self-upgrade execution. |
| **Ports** | `list_ports`, `get_port`, `get_port_status`, `create_port`, `update_port`, `delete_port`, `reset_port`, `list_network_interfaces` | Complete port listener lifecycle: bind addresses, TLS termination options, socket reset/reload, and host interface enumeration. |
| **Proxies** | `list_proxies`, `get_proxy`, `get_proxy_status`, `create_proxy`, `update_proxy`, `delete_proxy`, `purge_proxy_cache` | Reverse proxy configuration for HTTP, WebSocket, TCP, UDP, TLS, and HTTP/3, upstream health status, and path-selective cache purging. |
| **Certificates & ACME** | `list_certs`, `get_cert`, `check_cert_coverage`, `delete_cert`, `list_acme`, `get_acme`, `create_acme`, `delete_acme`, `request_acme`, `get_acme_cname_instructions`, `list_dns_credentials`, `get_dns_credential`, `create_dns_credential`, `update_dns_credential`, `delete_dns_credential`, `get_ct_config`, `update_ct_config`, `get_ct_status`, `list_ct_entries`, `check_ct_now`, `dismiss_ct_alert`, `update_acme`, `create_self_signed_cert`, `upload_cert`, `download_cert` (chain only, never the private key), `delete_many_certs`, `verify_cname` | TLS certificate inspection, domain coverage verification, ACME automated issuance (HTTP-01, TLS-ALPN-01, DNS-01), RFC 8555 CNAME alias delegation, instant certificate orders/renewals, DNS credential management, and Certificate Transparency rogue detection. |
| **Access Lists** | `list_access_lists`, `get_access_list`, `create_access_list`, `update_access_list`, `delete_access_list` | Access control lists with CIDR IP filtering, HTTP basic authentication, and custom bearer tokens. |
| **PaaS Applications** | `list_apps`, `get_app`, `create_app`, `update_app`, `delete_app`, `deploy_app`, `stop_app`, `start_app`, `restart_app`, `get_app_logs`, `get_app_env`, `update_app_env`, `list_app_deployments`, `rollback_deployment`, `check_app_update`, `list_app_scheduled_commands`, `create_app_scheduled_command`, `update_app_scheduled_command`, `delete_app_scheduled_command`, `run_app_scheduled_command`, `get_deployment`, `approve_deployment`, `reject_deployment`, `get_app_metrics`, `get_app_services`, `get_app_topology`, `upgrade_app_catalog`, `update_app_git_token`, `delete_app_git_token`, `create_app_webhook_secret`, `delete_app_webhook_secret`, `install_app_hook`, `list_app_files`, `download_app_file` (UTF-8 text up to 1 MiB), `upload_app_file` (UTF-8 text up to 1 MiB), `delete_app_file`, `list_volume_backups`, `create_volume_backup`, `restore_volume_backup`, `delete_volume_backup` | Autonomous PaaS container lifecycle: image/git/compose app deployment, container log streaming, environment variable injection, deployment rollbacks, scheduled cron commands management and immediate in-container execution, and Docker registry update detection. |
| **App Store Catalog** | `get_store`, `refresh_store`, `install_store_app` | Instant deployment of curated one-click templates (databases, dashboards, CMS, wikis, analytics) directly from the store into fully configured and routed applications. |
| **Managed Databases & Backups** | `list_databases`, `list_database_engines`, `create_database`, `get_database_credentials`, `list_database_tenants`, `reset_database_password`, `delete_database_tenant`, `list_database_backups`, `create_database_backup`, `delete_database_backup`, `get_backup_config`, `update_backup_config`, `test_database_backup`, `get_database_pitr_range`, `restore_database_pitr`, `archive_database_wal`, `list_database_wal_segments`, `delete_database`, `clone_database`, `list_database_replicas`, `create_database_replica`, `restore_database_backup`, `run_database_backup_now`, `list_all_backups`, `get_backups_overview`, `list_s3_database_backups`, `restore_s3_database_backup`, `refresh_database_engines` | Provisioning PostgreSQL, MySQL, Redis, MariaDB, and MongoDB with automated credentials, immediate backup creation, snapshot deletion, backup integrity verification, PITR point-in-time recovery with branching, WAL archive operations, and remote storage configuration. |
| **Agent Target Nodes** | `list_targets`, `get_target`, `create_target`, `delete_target`, `update_target_region`, `drain_target`, `undrain_target`, `create_target_token`, `upgrade_target`, `upgrade_all_targets` | Multi-node Docker agent infrastructure, one-time enrollment token issuance, target decommissioning, geo-region assignment, traffic draining/undraining, and remote target binary upgrades. |
| **Projects & Env Groups** | `list_projects`, `create_project`, `delete_project`, `list_env_groups`, `create_env_group`, `delete_env_group`, `list_project_environments`, `create_environment`, `update_environment_env`, `sync_environment_env`, `get_project`, `update_project`, `get_environment`, `update_environment`, `delete_environment`, `get_environment_env`, `diff_environments`, `get_env_group`, `update_env_group` | Project organization, multi-environment isolation, shared environment variable groups, and cross-environment configuration synchronization with selective key cloning. |
| **Outbound Webhooks** | `list_outbound_webhooks`, `get_outbound_webhook`, `create_outbound_webhook`, `update_outbound_webhook`, `delete_outbound_webhook`, `test_outbound_webhook` | Outbound event notification webhook dispatchers: endpoint management, HMAC-SHA256 signature secret validation, fine-grained event subscriptions, and end-to-end webhook delivery testing. |
| **Container Registries** | `list_registries`, `create_registry`, `update_registry`, `delete_registry` | Private Docker and OCI container registry credentials: credential storage, secure pull authentication, and server target endpoints. |
| **Git Connections** | `list_git_connections`, `get_git_connection`, `create_git_connection`, `delete_git_connection`, `list_git_repositories`, `list_git_branches`, `update_git_connection`, `authorize_git_connection`, `create_github_app`, `install_github_app` | Git provider connections (GitHub App, GitLab, Gitea): OAuth link management, accessible repository browsing, and remote branch enumeration. |
| **Accounts & Auth Tokens** | `list_accounts`, `create_account`, `update_account`, `delete_account`, `list_tokens`, `create_token`, `delete_token` | User account administration: roles (admin, editor, viewer), proxy isolation scopes, password management, and persistent scoped API token issuance. |
| **Disaster Recovery** | `list_disaster_recovery_backups`, `create_disaster_recovery_backup`, `restore_disaster_recovery_backup`, `delete_disaster_recovery_backup` | Full system snapshot archives with optional AES-256-GCM encryption and automatic S3 replication. |

The MCP server is off by default. The settings page of the panel turns it on while the server runs and holds the clients that may sign in. `config.toml` carries the same section:

```toml
[mcp]
enabled = true

[[mcp.clients]]
id = "r3v3rs3-cli"
name = "r3v3rs3 MCP client"
redirect_uris = ["http://127.0.0.1:8765/callback"]
```

A client signs in through the OAuth 2.1 authorization code flow with PKCE: the client opens `/oauth/authorize` in the browser, the signed-in account approves the client on a consent form, and the client trades the one-time code for an access token at `/oauth/token`. The token carries the scopes of the approval: `read` sees the state, `edit` also changes proxies. The consent form shows a checkbox for each requested scope, all checked, and the account unchecks the scopes that it does not grant. A request without a `scope` parameter asks for both `read` and `edit`, and the `WWW-Authenticate` header of a `401` response names both scopes. The role of the account still limits the token: a viewer account cannot change anything with an `edit` token. The metadata documents live at `/.well-known/oauth-authorization-server` and `/.well-known/oauth-protected-resource`.

A client that cannot run the OAuth flow and can only send a fixed `Authorization` header uses an API token of the account (`r3 <server> token create`). An `edit` token grants the `read` and `edit` scopes, a `read` token only `read`, and the role of the account still limits both. A deleted token gets `401` at once.

Tokens and dynamically registered clients are persisted in SQLite (`r3v3rs3.db`). A server restart or upgrade preserves the active grants, and clients seamlessly refresh their access tokens using their 30-day refresh tokens without prompting for re-authentication. Refresh tokens use single-use rotation with token reuse detection per RFC 6819 and OAuth 2.1: presenting an already-rotated refresh token immediately revokes the compromised token family. Active sessions can be viewed and revoked at any time from the WebUI Settings page or via `GET /api/mcp/sessions` and `DELETE /api/mcp/sessions/{id}`.

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
r3 http://127.0.0.1:46492 platform get-app-info
```

`login` opens the consent page in the browser and stores the tokens in the config directory. A script can pass an access token directly with `--token` or the `R3V3RS3_TOKEN` environment variable. The `update` command installs a newer release over the running binary and verifies its sha256 digest against the checksums of the release.

### Full resource management

`r3` provides direct first-class commands to manage all aspects of the reverse proxy and PaaS platform:

```sh
# Ports
r3 <server> port list
r3 <server> port get <id>
r3 <server> port create <spec.json|yaml|->
r3 <server> port update <id> <spec.json|yaml|->
r3 <server> port delete <id>
r3 <server> port reset <id>

# Proxies
r3 <server> proxy list
r3 <server> proxy get <id>
r3 <server> proxy create <proxy.json|yaml|->
r3 <server> proxy update <id> <proxy.json|yaml|->
r3 <server> proxy get-status <id>
r3 <server> proxy purge-cache <id> [--path /static]
r3 <server> proxy delete <id>

# Certificates & ACME
r3 <server> cert list
r3 <server> cert get <id>
r3 <server> cert upload --cert server.crt --key server.key
r3 <server> cert check-coverage
r3 <server> cert delete <id>
r3 <server> cert create-self-signed --san app.example.com [--ca <ca-id>] [--kind client]
r3 <server> cert download <id> [--output cert.tar.gz]
r3 <server> cert delete-many <id> <id>
r3 <server> cert verify-cname --domain app.example.com [--expected-target panel.example.com]
r3 <server> acme list
r3 <server> acme get <id>
r3 <server> acme create <acme.json|yaml|->
r3 <server> acme update <id> <acme.json|yaml|->
r3 <server> acme request <id>
r3 <server> acme delete <id>

# Access Lists & DNS Providers
r3 <server> acl list
r3 <server> acl get <id>
r3 <server> acl create <acl.json|yaml|->
r3 <server> acl update <id> <acl.json|yaml|->
r3 <server> acl delete <id>
r3 <server> dns list
r3 <server> dns get <id>
r3 <server> dns create <dns.json|yaml|->
r3 <server> dns update <id> <dns.json|yaml|->
r3 <server> dns delete <id>

# Deployed Applications
r3 <server> app list
r3 <server> app get <id>
r3 <server> app create <app.json|yaml|->
r3 <server> app update <id> <app.json|yaml|->
r3 <server> app deploy <id>
r3 <server> app rollback-deployment <id>
r3 <server> app start <id>
r3 <server> app stop <id>
r3 <server> app restart <id>
r3 <server> app get-logs <id> [--tail 100]
r3 <server> app get-log-size <id>
r3 <server> app vacuum-logs <id>
r3 <server> app get-env <id>
r3 <server> app update-env <id> KEY=VAL [KEY2=VAL2...]
r3 <server> app delete <id>
r3 <server> app check-update <id>
r3 <server> app list-deployments <id>
r3 <server> app get-deployment <deployment-id>
r3 <server> app approve-deployment <deployment-id>
r3 <server> app reject-deployment <deployment-id>
r3 <server> app get-metrics <id> [--hours 6]
r3 <server> app get-services <id>
r3 <server> app get-topology <id>
r3 <server> app upgrade-catalog <id>
printf '%s' "$TOKEN" | r3 <server> app update-git-token <id>
r3 <server> app delete-git-token <id>
r3 <server> app create-webhook-secret <id>
r3 <server> app delete-webhook-secret <id>
r3 <server> app install-hook <id>
r3 <server> app files list <id> --path /data
r3 <server> app files download <id> --path /data/app.db [--output app.db]
r3 <server> app files upload <id> --path /data/config.yml --file ./config.yml
r3 <server> app files delete <id> --path /data/old.log
r3 <server> app volumes list-backups <id> <volume>
r3 <server> app volumes create-backup <id> <volume>
r3 <server> app volumes restore-backup <id> <volume> <backup-id>
r3 <server> app volumes delete-backup <id> <volume> <backup-id>
r3 <server> app volumes download-backup <id> <volume> <backup-id> [--output data.tar.gz]
r3 <server> app volumes export-archive <id> <volume> [--output data-export.tar.gz]
r3 <server> app volumes import-archive <id> <volume> --file ./data.tar.gz

# Managed Databases
r3 <server> db list
r3 <server> db get-app <id>
r3 <server> db create --name my-db --engine postgres [--version postgres:18-alpine]
r3 <server> db clone <id> --name my-db-clone
r3 <server> db create-replica <id> --name my-db-replica
r3 <server> db get-app-logs <id> [--tail 100]
r3 <server> db get-credentials <id>
r3 <server> db list-tenants <id>
r3 <server> db reset-password <id> [--tenant <name>]
r3 <server> db delete-tenant <id> <tenant>
r3 <server> db delete <id>
r3 <server> db list-replicas <id>
r3 <server> db refresh-engines
r3 <server> db list-engines

# Backups, Automated Testing, Point-in-Time Recovery (PITR) & WAL
r3 <server> backup get-config
r3 <server> backup list <database-id>
r3 <server> backup create <database-id>
r3 <server> backup test <database-id> <backup-id>
r3 <server> backup restore <database-id> <backup-id>
r3 <server> backup get-pitr-range <database-id>
r3 <server> backup restore-pitr <database-id> <timestamp_ms> [--new-name <name>] [--host-port <port>]
r3 <server> backup archive-wal <database-id>
r3 <server> backup list-wal-segments <database-id>
r3 <server> backup delete <database-id> <backup-id>
r3 <server> backup list-all
r3 <server> backup get-overview
r3 <server> backup run-now <database-id>
r3 <server> backup list-s3 [--database <name>]
r3 <server> backup download-s3 --key <key> [--output db.sql.gz]
r3 <server> backup download <database-id> <backup-id> [--output db.sql.gz]
r3 <server> backup restore-s3 <key> [--target-db <id> | --new-name <name> [--engine <engine>]]

# Multi-Node Agent Targets
r3 <server> target list
r3 <server> target get <id>
r3 <server> target create --name worker-1 --host 10.0.0.5 --port 46493
r3 <server> target drain <id>
r3 <server> target undrain <id>
r3 <server> target upgrade <id>
r3 <server> target upgrade-all
r3 <server> target create-token <id>
r3 <server> target delete <id>

# Projects & Environments
r3 <server> project list
r3 <server> project get <id>
r3 <server> project create --name my-project [--description "Core apps"]
r3 <server> project sync-environment-env <project-id> --from staging --to production
r3 <server> project delete <id>
r3 <server> project update <id> --name my-project [--description "Core apps"]
r3 <server> project create-environment <project-id> --name staging
r3 <server> project get-environment <project-id> <env-id>
r3 <server> project update-environment <project-id> <env-id> --name stage
r3 <server> project delete-environment <project-id> <env-id>
r3 <server> project get-environment-env <project-id> <env-id>
r3 <server> project update-environment-env <project-id> <env-id> <vars.json|yaml|->
r3 <server> project diff-environments <project-id> <source-env-id> <target-env-id>
r3 <server> env-group list
r3 <server> env-group get <id>
r3 <server> env-group create <group.json|yaml|->
r3 <server> env-group update <id> <group.json|yaml|->
r3 <server> env-group delete <id>

# App Store & One-Click Templates
r3 <server> store get
r3 <server> store refresh
r3 <server> store install-app <store-app-id> --name my-service [--domain service.example.com]

# Traffic Telemetry & Request Inspection
r3 <server> traffic connections list [--proxy <id>]
r3 <server> traffic connections terminate <connection-id>
r3 <server> traffic get-priority-stats
r3 <server> traffic get-geo
r3 <server> traffic replay-request --entry <log-entry-id>
r3 <server> traffic get [--resource-id <proxy-id>] [--status 502] [--search /api] [--country DE] [--limit 50]
r3 <server> traffic get-entry <log-entry-id>
r3 <server> traffic stream [--resource-id <proxy-id>] [--status 502] [--search /api] [--country DE]
r3 <server> app stream-logs <id> [--tail 100] [--service web]
r3 <server> app open-terminal <id> [--shell /bin/bash]
r3 <server> traffic replay-request <request.json|yaml|->
r3 <server> security get-system-anomalies
r3 <server> security get-anomaly-rules
r3 <server> security update-anomaly-rules <rules.json|yaml|->
r3 <server> security dismiss-anomalies
r3 <server> security analyze-headers <rules.json|yaml|->
r3 <server> proxy preview <request.json|yaml|->
r3 <server> port get-status <port-id>
r3 <server> port list-network-interfaces
r3 <server> cdn get-status
r3 <server> cdn refresh-ranges
r3 <server> discovery list-status
r3 <server> get-logs server [--since 1700000000] [--until 1700003600] [--limit 100]
r3 <server> get-dashboard
r3 <server> audit list-log [--username admin] [--action create_proxy] [--resource-id <id>] [--search text] [--limit 50]
r3 <server> grpc check-health <request.json|yaml|->
r3 <server> grpc invoke <request.json|yaml|->
r3 <server> grpc reflect <request.json|yaml|->
r3 <server> proxy invoke-grpc <proxy-id> <request.json|yaml|->
r3 <server> proxy reflect-grpc <proxy-id> [<request.json|yaml|->]
r3 <server> backup update-config <config.json|yaml|->
r3 <server> platform list-mcp-sessions
r3 <server> platform revoke-mcp-session <session-id>
r3 <server> platform get-grafana-dashboard [--output r3v3rs3-grafana.json]

# Security, Banned & Quarantined IPs
r3 <server> security list-banned-ips
r3 <server> security unban-ip <ip>
r3 <server> security list-quarantined-ips
r3 <server> security quarantine-ip <ip> [--reason "Scanner probe"] [--duration-seconds 3600]
r3 <server> security unquarantine-ip <ip>
r3 <server> security get-quarantine-config
r3 <server> security update-quarantine-config [--enabled true] [--max-violations 5] [--window-seconds 60] [--ban-duration-seconds 3600] [--block-status-codes 404,403] [--scanner-patterns "/.env,/.git"] [--allowed-ips "198.51.100.0/24"]

# Webhooks & Scheduled Cron Jobs
r3 <server> webhook list
r3 <server> webhook get <id>
r3 <server> webhook create <webhook.json|yaml|->
r3 <server> webhook update <id> <webhook.json|yaml|->
r3 <server> webhook test <id>
r3 <server> webhook delete <id>
r3 <server> cron list <app-id>
r3 <server> cron create <app-id> --cron "0 * * * *" --command "php artisan schedule:run" [--name hourly]
r3 <server> cron update <app-id> <job-id> [--cron "*/15 * * * *"] [--command "php artisan queue:work"] [--name queue]
r3 <server> cron run <app-id> <job-id>
r3 <server> cron delete <app-id> <job-id>

# Container Registries
r3 <server> registry list
r3 <server> registry create --server ghcr.io --username user --password token
r3 <server> registry update <id> [--server ghcr.io] [--username user] [--password token]
r3 <server> registry delete <id>

# Git Provider Connections & Repositories
r3 <server> git list
r3 <server> git get <id>
r3 <server> git create --name my-gitlab --provider gitlab --client-id <id> [--client-secret <secret>]
r3 <server> git list-repositories <id> [--search query] [--page 1]
r3 <server> git list-branches <id> --repo owner/repo
r3 <server> git delete <id>
r3 <server> git update <id> <connection.json|yaml|->
r3 <server> git authorize <id> --redirect-uri https://panel.example.com/oauth/git/callback
r3 <server> git create-github-app --name team-github --redirect-uri https://panel.example.com/oauth/git/callback [--organization my-team] [--url https://github.example.com]
r3 <server> git install-github-app <id>

# User Accounts & Auth Tokens
r3 <server> account list
r3 <server> account create --username operator --password <pass> [--role editor]
r3 <server> account update <username> [--password <pass>] [--role admin]
r3 <server> account delete <username>
r3 <server> token list
r3 <server> token create --name ci-deploy [--scope edit]
r3 <server> token delete <id>

# Platform Status & Maintenance
r3 <server> platform get-app-info
r3 <server> platform get-workers
r3 <server> platform get-metrics
r3 <server> platform run-prune
r3 <server> platform vacuum-logs [--threshold-mb 50]
r3 <server> platform upgrade-server
r3 <server> platform get-upgrade-status
r3 <server> platform rotate-key
```

`r3 app open-terminal` also takes commands on its standard input, for example `echo 'df -h' | r3 <server> app open-terminal <id>`. When the input ends, the client sends Ctrl-D to the shell and prints the output until the shell exits.

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
