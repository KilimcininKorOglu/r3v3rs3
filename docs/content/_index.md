+++
title = "r3v3rs3"
sort_by = "weight"
+++

[![Crates.io](https://img.shields.io/crates/v/r3v3rs3.svg)](https://crates.io/crates/r3v3rs3)
[![GitHub license](https://img.shields.io/github/license/KilimcininKorOglu/r3v3rs3.svg)](https://github.com/KilimcininKorOglu/r3v3rs3/blob/main/LICENSE)
[![Rust](https://github.com/KilimcininKorOglu/r3v3rs3/actions/workflows/rust.yml/badge.svg)](https://github.com/KilimcininKorOglu/r3v3rs3/actions/workflows/rust.yml)
[![dependency status](https://deps.rs/crate/r3v3rs3/latest/status.svg)](https://deps.rs/crate/r3v3rs3)

r3v3rs3 is a reverse proxy server written in Rust. It proxies TCP, UDP, TLS, HTTP and WebSocket traffic, and it accepts incoming HTTP/3 connections. It also deploys apps as containers and routes their domains through its own proxies. It is a fork of [Taxy](https://github.com/picoHz/taxy).

You configure it in a browser. One binary carries the proxy and a WebUI in English and Turkish, and every port, proxy, certificate and account is a form in that WebUI. A change applies without a restart. The same operations are available over an admin API, and r3v3rs3 can also build its proxies from Docker labels, a Kubernetes Ingress, the Consul catalog or etcd keys.

# Key Features

## Proxying

- TCP, UDP, TLS, HTTP/1.1 and HTTP/2 proxies, including HTTP upgrades and WebSocket
- Partial HTTP/3 support: incoming QUIC connections only. Upstream connections use HTTP/2 or HTTP/1.1, and WebTransport is not supported
- Routing by host name (exact, wildcard or regex) and path, with path rewrite, redirect rules and fixed responses such as a redirect host or a 404 host
- Load balancing, active and passive health checks, a circuit breaker, sticky sessions, retries, upstream timeouts and traffic mirroring
- Upstream servers from DNS SRV records (`http+srv://` URLs), refreshed when the TTL expires
- The PROXY protocol on incoming connections and toward upstream servers

## Security and traffic control

- IP allow and deny lists, per-client rate limits, a request body size limit and real client IP resolution behind known CDNs and trusted proxies
- Basic, Bearer, forward and admin session authentication, and shared access lists
- Request and response header rules, response compression (brotli, zstd and gzip) and an in-memory HTTP cache

## Certificates

- Server, client and root certificates, uploaded or self-signed
- Mutual TLS: client certificate verification on TLS ports and client certificates toward upstream servers
- ACME v2 (for example Let's Encrypt) with the HTTP-01, TLS-ALPN-01 and DNS-01 challenges. DNS-01 issues wildcard certificates through 12 DNS provider APIs, a webhook, an exec command or RFC 2136
- Certificate expiry warnings and webhook notifications

## Operations

- A single binary with a built-in WebUI in English and Turkish. Configuration changes apply without a restart
- An admin API with an OpenAPI document and a Swagger UI ([Admin API](@/configuration.md#admin-api))
- Accounts with the `admin`, `editor` and `viewer` roles, per-account proxy lists and an audit log ([Accounts](@/accounts.md))
- Service discovery from Docker labels, Kubernetes Ingress and `R3v3rs3Proxy` resources, Consul and etcd ([Service Discovery](@/discovery.md))
- High availability with cluster mode: several nodes share one encrypted state in etcd or Consul ([Setup guide](@/tutorials/high-availability.md), [Cluster](@/cluster.md))

## Deployment platform

- Apps from a registry image, from a Git repository with its Dockerfile, or from the Docker Compose file of a Git repository
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

[Installing on a Linux Server](@/tutorials/install-linux.md) covers the options, the paths, the upgrade, the removal and the other install methods: cargo-binstall, `cargo install` and the release archives. [Installing with Docker](@/tutorials/install-docker.md) covers the container, Docker Compose and the image of the deployment platform.

# Development

Please refer to the [Development](@/development.md) section for details.

# First Setup

Create an admin account, start the server and build your first proxy with the [Getting Started](@/tutorials/getting-started.md) guide.
