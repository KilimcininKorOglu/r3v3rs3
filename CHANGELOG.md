# Changelog

All notable changes to this project are documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.1.0/), and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

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
