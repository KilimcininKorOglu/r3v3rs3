# Changelog

All notable changes to this project are documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.1.0/), and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

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
