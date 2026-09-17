<div align="center">
<img alt="r3v3rs3 logo" src="https://github.com/KilimcininKorOglu/r3v3rs3/blob/main/logo.svg?raw=true" width="150" />

# r3v3rs3

**Reverse everything.**

A reverse proxy server with a built-in WebUI for TCP, UDP, TLS, HTTP, WebSocket and HTTP/3, written in Rust.

[![Crates.io](https://img.shields.io/crates/v/r3v3rs3.svg)](https://crates.io/crates/r3v3rs3)
[![GitHub license](https://img.shields.io/github/license/KilimcininKorOglu/r3v3rs3.svg)](https://github.com/KilimcininKorOglu/r3v3rs3/blob/main/LICENSE)
[![Rust](https://github.com/KilimcininKorOglu/r3v3rs3/actions/workflows/rust.yml/badge.svg)](https://github.com/KilimcininKorOglu/r3v3rs3/actions/workflows/rust.yml)
[![dependency status](https://deps.rs/crate/r3v3rs3/latest/status.svg)](https://deps.rs/crate/r3v3rs3)

</div>

## Overview

### Proxying

- TCP, UDP, TLS, HTTP/1.1 and HTTP/2 proxies, including HTTP upgrades and WebSocket
- Partial HTTP/3 support: incoming QUIC connections only. Upstream connections use HTTP/2 or HTTP/1.1, and WebTransport is not supported
- Routing by host name (exact, wildcard or regex) and path, with path rewrite, redirect rules and fixed responses such as a redirect host or a 404 host
- Load balancing, active and passive health checks, a circuit breaker, sticky sessions, retries, upstream timeouts and traffic mirroring
- Upstream servers from DNS SRV records (`http+srv://` URLs), refreshed when the TTL expires
- The PROXY protocol on incoming connections and toward upstream servers

### Security and traffic control

- IP allow and deny lists, per-client rate limits, a request body size limit and real client IP resolution behind known CDNs and trusted proxies
- Basic, Bearer, forward and admin session authentication, and shared access lists
- Request and response header rules, response compression (brotli, zstd and gzip) and an in-memory HTTP cache

### Certificates

- Server, client and root certificates, uploaded or self-signed
- Mutual TLS: client certificate verification on TLS ports and client certificates toward upstream servers
- ACME v2 (for example Let's Encrypt) with the HTTP-01, TLS-ALPN-01 and DNS-01 challenges. DNS-01 issues wildcard certificates through 12 DNS provider APIs, a webhook, an exec command or RFC 2136
- Certificate expiry warnings and webhook notifications

### Operations

- A single binary with a built-in WebUI in English and Turkish. Configuration changes apply without a restart
- An admin API with an OpenAPI document and a Swagger UI
- Accounts with the `admin`, `editor` and `viewer` roles, per-account proxy lists and an audit log
- Service discovery from Docker labels, Kubernetes Ingress and `R3v3rs3Proxy` resources, Consul and etcd
- High availability with cluster mode: several nodes share one encrypted state in etcd or Consul

## Documentation

The documentation is available in [English](https://kilimcininkoroglu.github.io/r3v3rs3/) and [Turkish](https://kilimcininkoroglu.github.io/r3v3rs3/tr/):

- [Configuration](https://kilimcininkoroglu.github.io/r3v3rs3/configuration/): ports, proxies, certificates, ACME, settings, the admin API and logging
- [Accounts](https://kilimcininkoroglu.github.io/r3v3rs3/accounts/): roles and permissions
- [Service Discovery](https://kilimcininkoroglu.github.io/r3v3rs3/discovery/): Docker, Kubernetes, Consul and etcd
- [Tutorials](https://kilimcininkoroglu.github.io/r3v3rs3/tutorials/): step by step guides, starting with [High Availability](https://kilimcininkoroglu.github.io/r3v3rs3/tutorials/high-availability/)
- [Cluster](https://kilimcininkoroglu.github.io/r3v3rs3/cluster/): the cluster reference, from the settings to the failure modes
- [Development](https://kilimcininkoroglu.github.io/r3v3rs3/development/)

## Screenshot

![r3v3rs3 WebUI Screenshot](https://raw.githubusercontent.com/KilimcininKorOglu/r3v3rs3/main/screenshot.png)

## Installation

r3v3rs3 runs on Linux. Release binaries and Docker images are available for x86_64 (amd64) and aarch64 (arm64).

### Linux server

`install.sh` installs the latest release binary with its sha256 check, creates the admin account and runs r3v3rs3 as a systemd service:

```bash
curl -fsSL https://raw.githubusercontent.com/KilimcininKorOglu/r3v3rs3/main/install.sh | sudo bash
```

The script asks for the admin WebUI address. The default is `127.0.0.1:46492`. To install a specific release or to skip the question, pass the options:

```bash
curl -fsSL https://raw.githubusercontent.com/KilimcininKorOglu/r3v3rs3/main/install.sh | sudo bash -s -- --version 1.0.1 --webui 0.0.0.0:46492
```

The config is in `/etc/r3v3rs3` and the logs are in `/var/log/r3v3rs3`. Run the script again to upgrade.

### Docker

```bash
docker run -d \
  -v r3v3rs3-config:/root/.config/r3v3rs3 \
  -v r3v3rs3-data:/root/.local/share/r3v3rs3 \
  -p 80:80 \
  -p 443:443 \
  -p 127.0.0.1:46492:46492 \
  --restart unless-stopped \
  --stop-signal SIGINT \
  --name r3v3rs3 \
  ghcr.io/kilimcininkoroglu/r3v3rs3:latest
```

Publish each additional port that you add in the WebUI with another `-p` option. Create the admin account:

```bash
docker exec -it r3v3rs3 r3v3rs3 add-user admin
```

### Docker Compose

Download [`docker-compose.yml`](https://github.com/KilimcininKorOglu/r3v3rs3/blob/main/docker-compose.yml) and start r3v3rs3:

```bash
curl -fsSLO https://raw.githubusercontent.com/KilimcininKorOglu/r3v3rs3/main/docker-compose.yml
docker compose up -d
docker compose exec r3v3rs3 r3v3rs3 add-user admin
```

The file uses host networking, so every port that you add in the WebUI listens without a change to the file. Host networking works only on a Linux Docker host. Set these variables in a `.env` file next to it:

- `R3V3RS3_WEBUI`: the admin WebUI address. The default is `127.0.0.1:46492`.
- `R3V3RS3_VERSION`: the image tag. The default is `latest`.

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

The admin API has an OpenAPI document at `/api/openapi.json` and a Swagger UI at `/api/docs/`. Both require a signed-in session. See [Admin API](https://kilimcininkoroglu.github.io/r3v3rs3/configuration/#admin-api) in the documentation.

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

`make check` runs the format check, clippy, the tests and the WebUI build. `make release` builds the release WebUI and the release binary. The repository also has a Gitpod configuration and a dev container.

[![Open in Gitpod](https://gitpod.io/button/open-in-gitpod.svg)](https://gitpod.io/#https://github.com/KilimcininKorOglu/r3v3rs3)

## Changelog

r3v3rs3 follows [Semantic Versioning](https://semver.org/spec/v2.0.0.html). [CHANGELOG.md](CHANGELOG.md) lists the changes of each release.

## Similar projects

HTTP reverse proxies written in Rust:

- [Sōzu](https://github.com/sozu-proxy/sozu)
- [rpxy](https://github.com/junkurihara/rust-rpxy)

## Credit

r3v3rs3 is based on [Taxy](https://github.com/picoHz/taxy) by picoHz.

The social preview image uses the photo by [cal gao](https://unsplash.com/@ginnta?utm_source=unsplash&utm_medium=referral&utm_content=creditCopyText) on [Unsplash](https://unsplash.com/photos/MASpFp0X2VU?utm_source=unsplash&utm_medium=referral&utm_content=creditCopyText).
