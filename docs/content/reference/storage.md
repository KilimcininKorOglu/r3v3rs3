+++
title = "Configuration and Storage"
description = "Where r3v3rs3 keeps its state, backups and disaster recovery"
weight = 1
+++

r3v3rs3 stores all server configuration, reverse proxy rules, ports, access lists, ACME settings, user accounts, and deployment platform data in a unified SQLite database, `r3v3rs3.db`, located in the configuration directory (default `/etc/r3v3rs3`).

- **Unified SQLite Database (`r3v3rs3.db`)**: Atomic transactions and WAL mode ensure high concurrency, immediate durability, and rock-solid reliability across restarts. Configuration mutations for ports, reverse proxies, and access lists perform granular upserts and targeted deletions, eliminating full-table deletion churn and preserving modification timestamps on unchanged items.
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

# Show the settings, then save an edited copy (JSON or YAML file, or '-' for standard input)
$ r3 config get > settings.json
$ r3 config update settings.json
```

### MCP

The tools `backup_config` and `restore_config` do the same; pass the answer of `backup_config` as the `backup` argument of `restore_config`. `test_notification` (and `r3 config test-notification`) sends a test message to the saved notification webhook.

### REST API

- `GET /api/config/backup`: Returns the complete `ConfigBackup` object. Requires `admin` permission.
- `POST /api/config/restore`: Restores the configuration from a `ConfigBackup` JSON payload. Hot-reloads all ports, proxies, access lists, and settings immediately. Requires `admin` permission and records an entry in the audit log.

## Disaster Recovery & Encrypted Platform Snapshots (S3)

Unlike standard JSON configuration backups, **Disaster Recovery Snapshots** (`.r3bundle`) capture the full operational state of r3v3rs3 and its Coolify-like deployment platform in a single, AES-256-GCM encrypted bundle:

- **Complete State Included**: Contains `r3v3rs3.db` (unified SQLite database with all settings, ports, proxies, access lists, ACME certificates, user accounts, apps, deployments, databases, and environment variables), `r3v3rs3.key`, cluster sealing keys (`keys/`), TLS certificates and private keys (`certs/`), and legacy backup files if present.
- **AES-256-GCM Encryption**: Encrypted with a required user-defined passphrase, from which Argon2id derives the key with a random salt. Bundles made by older releases with SHA-256 key derivation still restore. A backup or restore request without a passphrase is refused with `400 disaster_recovery_passphrase_required`. Unencrypted data never leaves the server memory.
- **Automatic S3 Replication**: When remote S3 storage is configured in database backup settings, disaster recovery backups are automatically uploaded to `{prefix}/disaster-recovery/` in the remote bucket.
- **Single-Command Disaster Recovery**: Allows restoring the entire system onto a completely fresh server with a single CLI command.

### WebUI

Open the **Backups** page (`/backups`) and select the **Platform Disaster Recovery** tab:
- Enter the encryption **Passphrase** and check **Upload to remote S3 storage**.
- Click **Create Encrypted Snapshot** to create a point-in-time `.r3bundle`.
- Download, restore, or delete existing snapshots from the table using action icon buttons.
- Click **Upload & Restore .r3bundle** to upload an existing bundle file and restore the entire server state with confirmation.

### Remote Administration CLI

The `r3` CLI provides full remote management parity for disaster recovery:

```bash
# List all full disaster recovery snapshots
$ r3 dr list

# Create an encrypted snapshot and replicate to remote S3
$ r3 dr create --passphrase "my-secret-passphrase"

# Restore platform state from a snapshot ID
$ r3 dr restore <id> --passphrase "my-secret-passphrase"

# Download a snapshot (.r3bundle)
$ r3 dr download <id> -o backup.r3bundle

# Restore directly from a local bundle file
$ r3 dr restore-upload -f backup.r3bundle --passphrase "my-secret-passphrase"
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
- `POST /api/disaster-recovery/backups`: Creates a new encrypted snapshot with the required passphrase and optional S3 replication.
- `GET /api/disaster-recovery/backups/{id}/download`: Downloads the `.r3bundle` file.
- `DELETE /api/disaster-recovery/backups/{id}`: Deletes a disaster recovery backup.
- `POST /api/disaster-recovery/backups/{id}/restore`: Restores from a stored snapshot ID.
- `POST /api/disaster-recovery/restore-upload`: Restores from an uploaded `.r3bundle` payload with `X-Passphrase` header.


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


## Configuration Files

r3v3rs3 stores its configuration in TOML files in `$XDG_CONFIG_HOME/r3v3rs3` or `$HOME/.config/r3v3rs3`.

You can override the default location by setting the `R3V3RS3_CONFIG_DIR` environment variable or the `--config-dir` command-line option.

These files can be edited manually, but r3v3rs3 does not detect the change. Restart the server after editing a configuration file.
