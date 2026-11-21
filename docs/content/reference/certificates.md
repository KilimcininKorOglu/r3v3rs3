+++
title = "Certificates and ACME"
description = "Server, client and root certificates, ACME, DNS credentials and CT monitoring"
weight = 4
+++

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
| `deployment_started`   | A deployment of the [deployment platform](@/reference/platform.md) started.                                     |
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
- r3v3rs3 sends notifications in the background concurrently across up to 16 workers, preventing slow endpoints from causing head-of-line blocking. The queue holds at most 1024 notifications. If the queue ever fills up, critical alerts (such as certificate expirations, ACME failures, and deployment failures) fall back to direct background dispatch so they are never silently dropped.
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

## ACME

r3v3rs3 supports automatic certificate provisioning using [ACME](https://letsencrypt.org/docs/client-options/) (Automatic Certificate Management Environment). ACME is supported by many certificate authorities, such as Let's Encrypt, ZeroSSL, BuyPass, and Google Trust Services.

An ACME entry holds one or more domain names. Enter them separated by commas in the "Domain Names" field, for example `example.com, *.example.com`. The certificate contains every domain name as a Subject Alternative Name. r3v3rs3 renews the certificate automatically before it expires. If an order fails, r3v3rs3 orders again after one hour. The "Order again" action of an entry in the ACME tab, and `POST /api/acme/{id}/order`, order the certificates of the entry at once, without waiting for the renewal time or for the retry.

Select the certificate authority (Let's Encrypt, Google Trust Services, ZeroSSL, BuyPass, BuyPass Test, or "Custom" with its "Server URL") and enter an "Email Address". Certificate authorities that use External Account Binding (EAB), such as Google Trust Services or ZeroSSL, require the "EAB Key ID" and "EAB HMAC Key" of your account at the certificate authority. r3v3rs3 orders a new certificate "Renewal Interval (days)" (`renewal_days`, default `60`) days after the last one.

### Challenges

The certificate authority checks that you control each domain name with a challenge. Select the challenge in the "Challenge" field.

| Challenge   | How it works                                                                                                                                                      | Requirements                                                                                                                              |
|-------------|-------------------------------------------------------------------------------------------------------------------------------------------------------------------|-------------------------------------------------------------------------------------------------------------------------------------------|
| HTTP-01     | The certificate authority requests `http://<domain>/.well-known/acme-challenge/<token>`, and r3v3rs3 answers.                                                     | Every domain name resolves to r3v3rs3, and TCP port 80 is open and accessible from the internet. Wildcard domain names are not possible.  |
| TLS-ALPN-01 | The certificate authority opens a TLS connection to port 443 of the domain with the ALPN protocol `acme-tls/1`, and r3v3rs3 answers with a challenge certificate. | Every domain name resolves to r3v3rs3, and TCP port 443 is open and accessible from the internet. Wildcard domain names are not possible. |
| DNS-01      | r3v3rs3 creates a TXT record `_acme-challenge.<domain>` through the API of your DNS provider.                                                                     | A DNS provider from the table below and an API credential that can edit the zone.                                                         |

A wildcard domain name such as `*.example.com` needs DNS-01. r3v3rs3 rejects a wildcard domain name with HTTP-01 and TLS-ALPN-01.

During a TLS-ALPN-01 challenge, every TLS port and every HTTPS port answers a client that offers only `acme-tls/1` with the challenge certificate. Other clients get the certificate of the port. A TLS port does not connect to the upstream server for a challenge connection. If no TCP or HTTP port uses the port of the "TLS-ALPN Challenge Address" setting, r3v3rs3 listens on that address until the challenges end. An HTTP port without TLS on port 443 cannot answer the challenge.

### DNS-01

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
| Route 53         | Access Key ID, Secret Access Key                                | `route53:ListHostedZones` and `route53:ChangeResourceRecordSets`. Private hosted zones are skipped, and XML entities in zone responses are unescaped automatically.                                                                                                   |
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

r3v3rs3 is tested against mock servers of these APIs and against the [Pebble](https://github.com/letsencrypt/pebble) test certificate authority. It is not tested against real provider accounts. When external DNS and Git provider APIs return an HTTP 429 Too Many Requests response, r3v3rs3 automatically handles rate limits: it parses the `Retry-After` header (up to 30 seconds) or applies exponential backoff, retrying requests up to 3 times before failing.

#### DNS-01 CNAME Delegation / Alias Mode (RFC 8555)

When issuing certificates for external clients or third-party domains, clients often cannot or should not provide their primary DNS provider API tokens to r3v3rs3. Under RFC 8555 DNS alias mode, external domains delegate their ACME DNS-01 validation to a DNS zone managed by r3v3rs3 through a static one-time CNAME record.

##### How it works

1. **Configure `dns_alias`:** When creating an ACME order with DNS-01, set `dns_alias` to your delegation base zone (e.g. `auth.example.com`) or explicit template (e.g. `_acme-challenge.{domain}.auth.example.com`).
2. **Client CNAME Setup:** The domain owner adds a single CNAME record pointing their challenge subdomain to the alias target:
   - **Source:** `_acme-challenge.client.com`
   - **Type:** `CNAME`
   - **Target:** `_acme-challenge.client.com.auth.example.com`
3. **Automated Issuance & Renewal:** During challenge execution, r3v3rs3 publishes the TXT validation record to `_acme-challenge.client.com.auth.example.com` using its own configured DNS provider (e.g. Cloudflare or Route 53). The ACME validation authority follows the CNAME from `client.com` to `auth.example.com` and issues the certificate.

Inspect CNAME instructions and verification status via CLI or MCP:
```bash
r3 acme get-cname-instructions <ACME_ID>
```

### DNS webhook

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

### DNS exec

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

### RFC 2136

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

## Certificate Transparency (CT) Monitor

r3v3rs3 includes a background Certificate Transparency log monitor that inspects public CT logs for your domains to detect rogue, unauthorized, or compromised SSL certificates in real time.

### Background Monitoring & Detection

The monitor runs on a configurable schedule (default: every 6 hours) and automatically discovers domains from active server certificates and ACME configurations, or monitors custom domains configured by the administrator:

- **Auto-Discovery**: Identifies root domains and SANs from all loaded certificates and ACME orders.
- **Allowed Certificate Authorities**: Predefined trusted public CAs (Let's Encrypt, Google Trust Services, DigiCert, Sectigo, ZeroSSL, Cloudflare) or customized trusted issuer lists.
- **Rogue Detection**: Any certificate discovered in public CT logs issued by an unrecognized or unauthorized CA is immediately flagged as rogue (`is_rogue: true`) with status `investigating`.
- **Alerting & Webhooks**: Triggers the `cert.rogue_detected` webhook event with complete certificate metadata (issuer, serial number, common name, SANs, logged timestamp).
- **Query Limits**: A check queries the CT log for four domains at the same time. One query may take 120 seconds, because crt.sh can need more than 20 seconds for an answer; the connection to the log server alone has 10 seconds. A domain whose query fails sets the status to `partial_error` and names the domain in `last_error`.

### CLI Management

Manage CT logs directly using the `r3 cert ct` command line tool:

```bash
# View monitor status and active alerts count
r3 cert ct get-status

# View current configuration
r3 cert ct get-config

# Update configuration
r3 cert ct update-config --enable true --domains "example.com,api.example.com" --interval 7200

# Trigger an immediate on-demand check
r3 cert ct check-now --domain example.com

# List discovered CT log records and rogue certificates
r3 cert ct list-entries --rogue-only

# Dismiss an alert
r3 cert ct dismiss-alert <ENTRY_ID>
```

### Model Context Protocol (MCP)

AI agents can inspect and manage CT logs autonomously via MCP tools:
- `get_ct_config`: Retrieve current monitoring configuration.
- `update_ct_config`: Update check intervals, domains, and allowed CAs.
- `get_ct_status`: Get real-time status, active alerts, and last check time.
- `list_ct_entries`: Query discovered records with filtering.
- `check_ct_now`: Trigger an immediate log query.
- `dismiss_ct_alert`: Acknowledge or resolve an alert.


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
