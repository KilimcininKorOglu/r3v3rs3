+++
title = "r3v3rs3"
sort_by = "weight"
+++

[![Crates.io](https://img.shields.io/crates/v/r3v3rs3.svg)](https://crates.io/crates/r3v3rs3)
[![GitHub license](https://img.shields.io/github/license/KilimcininKorOglu/r3v3rs3.svg)](https://github.com/KilimcininKorOglu/r3v3rs3/blob/main/LICENSE)
[![Rust](https://github.com/KilimcininKorOglu/r3v3rs3/actions/workflows/rust.yml/badge.svg)](https://github.com/KilimcininKorOglu/r3v3rs3/actions/workflows/rust.yml)
[![dependency status](https://deps.rs/crate/r3v3rs3/latest/status.svg)](https://deps.rs/crate/r3v3rs3)

r3v3rs3 is a reverse proxy server written in Rust. It proxies TCP, UDP, TLS, HTTP and WebSocket traffic, and it accepts incoming HTTP/3 connections. It also deploys apps as containers and routes their domains through its own proxies. It is a fork of [Taxy](https://github.com/picoHz/taxy).

You configure it in a browser. One binary carries the proxy and a WebUI in English and Turkish, and every port, proxy, certificate and account is a form in that WebUI. A change applies without a restart. The same operations are available over an admin API, and r3v3rs3 can also build its proxies from Docker labels or etcd keys.

# Key Features

## Proxying

- TCP, UDP, TLS, HTTP/1.1 and HTTP/2 proxies, including HTTP upgrades and WebSocket
- Partial HTTP/3 support: incoming QUIC connections only, with an `alt-svc` header so browsers learn the QUIC port. Upstream connections use HTTP/2 or HTTP/1.1, and WebTransport is not supported
- Routing by host name (exact, wildcard or regex) and path. The longest matching path wins across every proxy of a port, so the order of the proxies and routes does not change the result
- Path rewrite (strip prefix, regex replacement, add prefix), redirect rules and fixed responses: a redirection host, a status host and 404 host templates
- A route can replace the settings of its proxy: timeouts, retries, rate limit, IP filter, authentication, header rules and body size
- Load balancing with round robin (smooth weighted), random, first healthy server and client IP hash, and weights from 0 to 65535 for draining a server
- Active and passive health checks, a circuit breaker with a half-open trial, sticky sessions with an HMAC-signed cookie, retries with idempotent-method rules and a body replay limit, and traffic mirroring with a percentage
- Upstream connect and request timeouts per proxy and per route, and upstream servers from DNS SRV records (`http+srv://` URLs), refreshed when the TTL expires
- UDP client sessions with an idle timeout and at most 10,000 sessions per port
- HTTP/2 toward plain HTTP servers with h2c for gRPC, and shared upstream connection pools that carry many clients over one HTTP/2 connection
- Configurable HTTP/2 and HTTP/3 flood limits, globally and per port
- The PROXY protocol v1 and v2, received from trusted load balancers and sent to upstream servers

## Security and traffic control

- IP allow and deny lists per proxy and per route, and shared access lists that carry an IP filter and an authentication under one name
- Per-client rate limits with second, minute or hour periods, a burst and a `Retry-After` header, and a request body size limit with `413`
- Basic auth with argon2 hashes, Bearer tokens stored as SHA-256 digests and compared in constant time, forward auth (oauth2-proxy, Authelia) and admin session authentication with TOTP, plus sign-in attempt limits
- Real client IP resolution behind eight known CDNs (Cloudflare, Fastly, Amazon CloudFront, Bunny, Gcore, KeyCDN, Imperva, Google Cloud Load Balancing) and trusted proxies. The CDN ranges refresh daily, and the forged client IP headers are removed from untrusted peers
- Request and response header rules with variables: `{client_ip}`, `{host}`, `{request_id}`, `{route}`, `{client_cert_subject}` and `{client_cert_fingerprint}`
- Response compression (brotli, zstd and gzip) and an in-memory HTTP cache with ETag and Last-Modified revalidation, `304` answers, `X-Cache` and `Age` headers and a purge API
- Mutual TLS: client certificate verification on TLS ports (off, optional or required) and client certificates toward upstream servers

## Certificates

- Server, client and root certificates, uploaded or self-signed
- ACME v2 (for example Let's Encrypt) with the HTTP-01, TLS-ALPN-01 and DNS-01 challenges. DNS-01 issues wildcard certificates through 12 DNS provider APIs, a webhook, an exec command with a program allowlist, or RFC 2136 with TSIG
- Certificate expiry warnings, and webhook notifications for certificate, deployment and agent events with a bearer token and retries

## Operations

- A single binary with a built-in WebUI in English and Turkish, with light, dark and system themes. Configuration changes apply without a restart
- An admin API with an OpenAPI document and a Swagger UI ([Admin API](@/configuration.md#admin-api))
- A Prometheus metrics endpoint at `GET /api/metrics` with request, cache and connection counters ([Metrics](@/configuration.md#metrics))
- An MCP server at `/api/mcp` over OAuth 2.1 with PKCE and `read` and `edit` scopes, for AI hosts such as Claude Code that connect to the address directly, plus the `r3v3rs3-cli` command line client for scripts and terminals ([MCP Server](@/configuration.md#mcp-server))
- Accounts with the `admin`, `editor` and `viewer` roles, per-account proxy lists, TOTP and an audit log with retention ([Accounts](@/accounts.md))
- Access and server logs with hourly file rotation, a text or JSON format, and the `log.db` database behind the log and audit pages ([Logging](@/configuration.md#logging))
- An optional worker process that serves the ports while the master keeps the admin API, so a worker crash does not take the panel down ([Worker Processes](@/configuration.md#worker-processes))
- A zero-downtime upgrade over SIGUSR2 (`systemctl reload r3v3rs3`), an in-panel upgrade button that verifies the sha256 digest, and an hourly check for a newer release with a banner on every page of the panel ([Zero-Downtime Upgrade](@/configuration.md#zero-downtime-upgrade), [In-Panel Upgrade](@/configuration.md#in-panel-upgrade))
- Service discovery from Docker labels and etcd. Discovered proxies are read-only, and a definition that fails to become a proxy is reported as an issue in the proxy list ([Service Discovery](@/discovery.md))

## Deployment platform

- Apps from a registry image, from a Git repository with its Dockerfile, or from the Docker Compose file of a Git repository
- An App Store that installs a catalog image app in one click, refreshed daily from the repository
- Blue-green deployments that switch the proxy only after the new container passes its health check, and rollback to an earlier deployment
- r3v3rs3 routes the domains of the apps itself and orders their certificates through ACME
- Encrypted environment variables, the deployments and the container log of every app in the WebUI and the admin API
- Git provider connections: a GitHub App that r3v3rs3 creates from a manifest, also on GitHub Enterprise Server, and OAuth apps for GitLab and Gitea. A connection lists the repositories and branches, clones private repositories and installs the push webhook
- Push webhooks from GitHub, GitLab, Gitea, Forgejo or a generic HMAC sender start a deployment
- Agent targets run apps on other servers. The agent connects to the master over mTLS after a one-time enrollment ([Deployment Platform](@/platform.md))

# Installation

```bash
curl -fsSL https://raw.githubusercontent.com/KilimcininKorOglu/r3v3rs3/main/install.sh | sudo bash
```

[Installing on a Linux Server](@/tutorials/install-linux.md) covers the options, the paths, the upgrade, the removal and the other install methods: cargo-binstall, `cargo install` and the release archives.

# Development

See [Development](@/development.md).

# First Setup

Create an admin account, start the server and build your first proxy with the [Getting Started](@/tutorials/getting-started.md) guide.
