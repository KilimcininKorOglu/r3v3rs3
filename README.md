<div align="center">
<img alt="r3v3rs3 logo" src="https://github.com/KilimcininKorOglu/r3v3rs3/blob/main/logo.svg?raw=true" width="150" />

# r3v3rs3

**Reverse everything.**

A reverse proxy server with a built-in WebUI for TCP, UDP, TLS, HTTP, WebSocket and HTTP/3, and a deployment platform that runs your apps behind it, written in Rust.

[![Crates.io](https://img.shields.io/crates/v/r3v3rs3.svg)](https://crates.io/crates/r3v3rs3)
[![GitHub license](https://img.shields.io/github/license/KilimcininKorOglu/r3v3rs3.svg)](https://github.com/KilimcininKorOglu/r3v3rs3/blob/main/LICENSE)
[![Rust](https://github.com/KilimcininKorOglu/r3v3rs3/actions/workflows/rust.yml/badge.svg)](https://github.com/KilimcininKorOglu/r3v3rs3/actions/workflows/rust.yml)
[![dependency status](https://deps.rs/crate/r3v3rs3/latest/status.svg)](https://deps.rs/crate/r3v3rs3)

</div>

## Overview

### Proxying

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

### Security and traffic control

- IP allow and deny lists per proxy and per route, and shared access lists that carry an IP filter and an authentication under one name
- Per-client rate limits with second, minute or hour periods, a burst and a `Retry-After` header, and a request body size limit with `413`
- Basic auth with argon2 hashes, Bearer tokens stored as SHA-256 digests and compared in constant time, forward auth (oauth2-proxy, Authelia) and admin session authentication with TOTP, plus sign-in attempt limits
- Real client IP resolution behind eight known CDNs (Cloudflare, Fastly, Amazon CloudFront, Bunny, Gcore, KeyCDN, Imperva, Google Cloud Load Balancing) and trusted proxies. The CDN ranges refresh daily, and the forged client IP headers are removed from untrusted peers
- Request and response header rules with variables: `{client_ip}`, `{host}`, `{request_id}`, `{route}`, `{client_cert_subject}` and `{client_cert_fingerprint}`
- Response compression (brotli, zstd and gzip) and an in-memory HTTP cache with ETag and Last-Modified revalidation, `304` answers, `X-Cache` and `Age` headers and a purge API
- Mutual TLS: client certificate verification on TLS ports (off, optional or required) and client certificates toward upstream servers

### Certificates

- Server, client and root certificates, uploaded or self-signed
- ACME v2 (for example Let's Encrypt) with the HTTP-01, TLS-ALPN-01 and DNS-01 challenges. DNS-01 issues wildcard certificates through 12 DNS provider APIs, a webhook, an exec command with a program allowlist, or RFC 2136 with TSIG
- Certificate expiry warnings, and webhook notifications for certificate, deployment and agent events with a bearer token and retries

### Operations

- A single binary with a built-in WebUI in English and Turkish, with light, dark and system themes. Configuration changes apply without a restart
- An admin API with an OpenAPI document and a Swagger UI
- A Prometheus metrics endpoint at `GET /api/metrics` with request, cache and connection counters
- An MCP server at `/api/mcp` over OAuth 2.1 with PKCE and `read` and `edit` scopes, for AI hosts such as Claude Code that connect to the address directly, plus the `r3v3rs3-cli` command line client for scripts and terminals
- Accounts with the `admin`, `editor` and `viewer` roles, per-account proxy lists, TOTP and an audit log with retention
- Access and server logs with hourly file rotation, a text or JSON format, and the `log.db` database behind the log and audit pages
- An optional worker process that serves the ports while the master keeps the admin API, so a worker crash does not take the panel down
- A zero-downtime upgrade over SIGUSR2 (`systemctl reload r3v3rs3`), an in-panel upgrade button that verifies the sha256 digest, and an hourly check for a newer release with a banner on every page of the panel
- Service discovery from Docker labels and etcd. Discovered proxies are read-only, and a definition that fails to become a proxy is reported as an issue in the proxy list

### Deployment platform

- Apps from a registry image, from a Git repository with its Dockerfile, or from the Docker Compose file of a Git repository
- An App Store that installs a catalog app, image or Compose, in one click, refreshed daily from the repository
- A Databases section that installs PostgreSQL, MariaDB, MySQL, MongoDB or Redis in one click, on a fixed port of `127.0.0.1` with a generated password, and injects the connection into the apps that use it
- Blue-green deployments that switch the proxy only after the new container passes its health check, and rollback to an earlier deployment
- A web terminal into the running container of an app, for admins, over a WebSocket from the panel
- A file manager for the volumes of an app in the panel, for admins: list, download, upload, edit and delete
- r3v3rs3 routes the domains of the apps itself and orders their certificates through ACME
- Encrypted environment variables, the deployments and the container log of every app in the WebUI and the admin API
- Git provider connections: a GitHub App that r3v3rs3 creates from a manifest, also on GitHub Enterprise Server, and OAuth apps for GitLab and Gitea. A connection lists the repositories and branches, clones private repositories and installs the push webhook
- Push webhooks from GitHub, GitLab, Gitea, Forgejo or a generic HMAC sender start a deployment
- Agent targets run apps on other servers. The agent connects to the master over mTLS after a one-time enrollment

## Documentation

The documentation is available in [English](https://r3v3rs3.keremgok.tr/) and [Turkish](https://r3v3rs3.keremgok.tr/tr/):

- [Configuration](https://r3v3rs3.keremgok.tr/configuration/): ports, proxies, certificates, ACME, settings, the admin API and logging
- [Accounts](https://r3v3rs3.keremgok.tr/accounts/): roles and permissions
- [Service Discovery](https://r3v3rs3.keremgok.tr/discovery/): Docker and etcd
- [Deployment Platform](https://r3v3rs3.keremgok.tr/platform/): apps, deployments, webhooks, Git providers and agent targets
- [Tutorials](https://r3v3rs3.keremgok.tr/tutorials/): step by step guides, starting with [Getting Started](https://r3v3rs3.keremgok.tr/tutorials/getting-started/)
- [Development](https://r3v3rs3.keremgok.tr/development/)

## Screenshot

![r3v3rs3 WebUI Screenshot](https://raw.githubusercontent.com/KilimcininKorOglu/r3v3rs3/main/screenshot.png)

## Installation

r3v3rs3 runs on Linux. Release binaries are available for x86_64 (amd64) and aarch64 (arm64).

### Linux server

`install.sh` installs the latest release binary with its sha256 check, creates the admin account and runs r3v3rs3 as a systemd service:

```bash
curl -fsSL https://raw.githubusercontent.com/KilimcininKorOglu/r3v3rs3/main/install.sh | sudo bash
```

The script asks for the admin WebUI address. The default is `127.0.0.1:46492`. To install a specific release or to skip the question, pass the options:

```bash
curl -fsSL https://raw.githubusercontent.com/KilimcininKorOglu/r3v3rs3/main/install.sh | sudo bash -s -- --version 1.8.9 --webui 0.0.0.0:46492
```

The config is in `/etc/r3v3rs3` and the logs are in `/var/log/r3v3rs3`. Run the script again to upgrade.

### Cargo

The crates.io package contains the built WebUI, so you do not need trunk or the wasm toolchain.

With [cargo-binstall](https://github.com/cargo-bins/cargo-binstall#installation), which downloads the release binary:

```bash
cargo binstall r3v3rs3
```

With the Rust toolchain from [rustup.rs](https://rustup.rs/), which builds from source:

```bash
cargo install r3v3rs3
```

### GitHub Releases

Download the archive for your architecture from the [releases page](https://github.com/KilimcininKorOglu/r3v3rs3/releases), extract it and put the `r3v3rs3` binary in a directory of your `$PATH`.

## Starting the server

Create an account for the admin panel. The command asks for a password of at least 8 characters:

```bash
r3v3rs3 add-user admin
```

Start the server:

```bash
r3v3rs3 start
```

Open the admin panel at [http://localhost:46492/](http://localhost:46492/). `r3v3rs3 start --help` lists the options. Each option also reads an environment variable, for example `R3V3RS3_WEBUI`, `R3V3RS3_CONFIG_DIR` and `R3V3RS3_LOG_DIR`. The default config directory is `~/.config/r3v3rs3`, and the default log directory is `~/.local/share/r3v3rs3/logs`.

The admin API has an OpenAPI document at `/api/openapi.json` and a Swagger UI at `/api/docs/`. Both require a signed-in session. See [Admin API](https://r3v3rs3.keremgok.tr/configuration/#admin-api) in the documentation.

## Development

Install the Rust toolchain, the wasm target (`rustup target add wasm32-unknown-unknown`) and [trunk](https://trunkrs.dev/).

```bash
git clone https://github.com/KilimcininKorOglu/r3v3rs3
cd r3v3rs3

# Start the server
make run

# In a separate terminal, serve the WebUI with live reload
cd r3v3rs3-webui
trunk serve
```

`make check` runs the format check, clippy, the tests and the WebUI build, and `make check-fast` type-checks the whole workspace without codegen for the loop between edits. `make test-runtime-docker` runs the Docker runtime and deployment platform tests against the local Docker Engine. `make release` builds the release WebUI and the release binary. The repository also has a dev container.

## Changelog

r3v3rs3 follows [Semantic Versioning](https://semver.org/spec/v2.0.0.html). [CHANGELOG.md](CHANGELOG.md) lists the changes of each release.

## Similar projects

HTTP reverse proxies written in Rust:

- [Sōzu](https://github.com/sozu-proxy/sozu)
- [rpxy](https://github.com/junkurihara/rust-rpxy)

## Credit

r3v3rs3 is based on [Taxy](https://github.com/picoHz/taxy) by picoHz.

The social preview image uses the photo by [cal gao](https://unsplash.com/@ginnta?utm_source=unsplash&utm_medium=referral&utm_content=creditCopyText) on [Unsplash](https://unsplash.com/photos/MASpFp0X2VU?utm_source=unsplash&utm_medium=referral&utm_content=creditCopyText).
