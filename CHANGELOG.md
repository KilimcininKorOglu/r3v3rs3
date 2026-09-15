# Changelog

All notable changes to this project are documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.1.0/), and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

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
