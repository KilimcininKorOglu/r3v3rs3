# Deployment platform design

Status: phases 1 to 3 are implemented: the Docker runtime, the store and the admin API, and the blue-green pipeline of image apps on the local target. Phase 4 is implemented too: the Git source with its Dockerfile build, the Compose source and the `-platform` image. Phase 5 is implemented: the container log route and the WebUI pages (the left sidebar, the app list, the app form with its environment variables and Git token, the deployment history with rollback, and the container log). Phases 6 and 7 are implemented as one: agent targets with the agent CA, the one-time enrollment, the mTLS link, the remote runtime, Compose and Git apps on an agent, the loopback forwarders of section 10.4, the target admin API, the Targets page and the agent mode of `install.sh`. The user reference is `docs/content/platform.md`.

This document describes how r3v3rs3 grows from a reverse proxy into a self-hosted deployment platform, in the space of Coolify. It lives outside `docs/content/`, so the Zola site does not publish it.

## 1. Goals

1. Deploy an application to the server that runs r3v3rs3 (the **local target**).
2. Deploy an application to a remote server that runs `r3v3rs3 agent` and connects to r3v3rs3 (the **agent target**).
3. Build an application from a Git repository that holds a `Dockerfile` or a Compose file, or run a ready image from a registry.
4. Route traffic to a deployed application through the existing r3v3rs3 proxy, with TLS from the existing ACME support.
5. Keep every deployment reproducible: each one pins the image digest and a snapshot of its config, so a rollback starts exactly that image again.

## 2. Non-goals

These stay out of scope for the first version. Each one can follow later without a change to the core model.

- Language detection and generated Dockerfiles (Nixpacks, buildpacks). The repository must bring its own `Dockerfile` or Compose file.
- PM2 or other non-container runtimes.
- Kubernetes or Docker Swarm as a runtime.
- VPS provisioning through a cloud provider API.
- Billing, multi-tenant plans, white-label.
- Managed databases, backups, a template registry, PR preview environments. Section 13 lists them as later phases.

## 3. Reference projects

Two projects were studied. Both are cloned into `helper-projects/` for reference only.

| Topic | NineDeploy (TypeScript) | deploy-monster (Go) | Decision for r3v3rs3 |
|---|---|---|---|
| Packaging | pnpm monorepo, Node runtime | One binary, embedded UI | One binary, as today |
| State | SQLite + Drizzle, 40 tables | SQLite + SQLite KV, optional PostgreSQL | SQLite through the existing `sqlx` dependency |
| Ingress | Traefik v3 file provider | Own proxy with autocert | The existing r3v3rs3 proxy |
| Deploy | Blue-green, digest-pinned rollback, cancel checkpoints | Recreate or rolling | Blue-green through the existing health groups, digest-pinned rollback |
| Agent direction | Core dials the agent | Agent dials the master | Agent dials the master |
| Agent transport | Plain HTTP with AES-256-GCM sealed envelopes, no TLS | Hijacked HTTP, token, optional mTLS | TLS with required client certificates (mTLS) |
| Agent commands | Fixed table of about 24 typed operations, operands validated on both ends | Typed message kinds | A closed Rust `enum`, operands validated on both ends |

The two decisions that matter most:

- **The agent dials the master.** A remote server behind NAT or a firewall needs only an outbound connection. The master needs no route to the agent.
- **The agent link is mTLS.** NineDeploy documents its lack of TLS as an open weakness. r3v3rs3 already verifies client certificates on a TLS port (`TlsTermination.client_auth`, `client_ca_certs`), so the agent listener reuses that code instead of a custom envelope.

## 4. What r3v3rs3 already provides

| Platform need | Existing code |
|---|---|
| HTTP, HTTPS, HTTP/3, TCP, UDP ingress | `r3v3rs3/src/proxy/` |
| Route selection by host and path | `Router::get_route` in `r3v3rs3/src/proxy/http/filter.rs` |
| Certificates and ACME | `r3v3rs3/src/certs/`, `r3v3rs3-api/src/acme.rs` |
| mTLS in both directions | `r3v3rs3/src/proxy/tls.rs` |
| Health checks, load balancing, circuit breaker | `r3v3rs3/src/proxy/health.rs` |
| Docker Engine API reads (containers, events) | `r3v3rs3/src/discovery/docker.rs` over `r3v3rs3/src/kv/http.rs` |
| Accounts, permissions, per-proxy scope | `r3v3rs3/src/accounts.rs` |
| Admin API with OpenAPI | `r3v3rs3/src/admin/` |
| SQLite through `sqlx` with an idempotent schema | `r3v3rs3/src/audit.rs`, `r3v3rs3/src/log.rs` |
| Audit log | `r3v3rs3/src/audit.rs` |
| Notifications | `r3v3rs3/src/notify.rs` |
| Cluster mode | `r3v3rs3/src/cluster/` |
| WebUI with i18n | `r3v3rs3-webui/` |

The new work is the container lifecycle: the domain model, the runtime, the build step, the deploy pipeline and the agent link.

## 5. Module layout

All new code lives in the existing crates.

```
r3v3rs3-api/src/
  container.rs      the validated container types: ContainerSpec, ContainerName, ImageRef, EnvKey,
                    ProjectName, ServiceName
  git.rs            the validated Git source types: RepoUrl, GitRef, RelPath, CommitSha
  platform.rs       PlatformConfig, AppSpec, AppEntry, EnvEntry, DeploymentEntry, TargetEntry,
                    TargetRequest, TargetToken

r3v3rs3/src/
  platform/
    mod.rs          PlatformHandle and Platform: the apps, the sealed environment, the app lock
    store.rs        SQLite schema and queries
    deploy.rs       the pipeline of section 8, the health probe and the rollback
    build.rs        the build step of a Git app, the build limit and the removal of old images
    compose.rs      the checkout, the override file and the port check of a Compose app
    proxy.rs        the proxy of a running app, and the snapshots of the Platform provider
    publish.rs      reads the running containers and sends the snapshot to the server
    agents.rs       the agent port, the targets, the enrollment and the audit of an enrollment
    fake.rs         a runtime and a source fetcher in memory for the unit tests
  runtime/
    mod.rs          ContainerRuntime trait
    docker.rs       the local runtime over the Docker Engine API, with the image build
  build/
    mod.rs          SourceFetcher trait and the build context archive
    git.rs          the git checkout of one branch or one commit, without host configuration or hooks
    compose.rs      ComposeRunner and DockerCompose: `docker compose config`, `up` and `down`
    process.rs      runs git and docker with a timeout and the end of stderr in the error
  agent/
    pki.rs          the agent CA, the master certificate, the CSR signing and the pinned verifier
    token.rs        the enrollment token <secret>.<ca_hash>
    frame.rs        the length-prefixed JSON frames and the raw payloads
    link.rs         yamux over TLS: one stream per request, the tunnel streams
    listener.rs     the agent port of the master: enrollment, sessions, pings
    registry.rs     the connected agents with their version and last contact
    protocol.rs     AgentRequest, AgentReply, AgentOutput (the wire protocol)
    remote.rs       RemoteRuntime: ContainerRuntime over the link of a target
    compose.rs      RemoteCompose on the master, AgentCompose on the agent
    forward.rs      the loopback forwarders of the published ports of agent apps
    executor.rs     the agent side: checks and runs each request
    client.rs       `r3v3rs3 agent`: the enrollment, the identity, the reconnect loop
  admin/
    platform.rs     the admin API routes of section 12
```

The runtime split is the central abstraction. The pipeline talks only to `ContainerRuntime`. The local target uses `runtime::docker`; an agent target uses `agent::remote`, which forwards the same calls to the agent, and the agent runs them against its own `runtime::docker`. `Platform::backend` selects the runtime and the Compose runner of a target at the start of each job. One implementation of the Docker calls serves both targets.

## 6. Domain model

### 6.1 Target

A target is a Docker host that can run containers.

- `local`: the Docker Engine of the server that runs r3v3rs3. It exists once, implicitly, when a Docker socket is configured.
- `agent`: a remote server that runs `r3v3rs3 agent`. It is created in the WebUI, which returns a one-time enrollment token.

### 6.2 App

An app is one deployable unit on one target.

- `source`: `git` (repository URL, branch, optional credentials reference) or `image` (image reference).
- `build`: `dockerfile` (path, context directory, build arguments) or `compose` (file path, the service that receives traffic).
- `runtime`: container port, environment variables, volumes, CPU and memory limits, restart policy, health check path.
- `domains`: host names and paths that route to the app. Each becomes a route of an r3v3rs3 HTTP proxy.

### 6.3 Deployment

A deployment is one attempt to bring an app to a new version.

- `status`: `queued`, `building`, `deploying`, `running`, `superseded`, `failed`, `cancelled`.
- `trigger`: `manual`, `webhook`, `rollback`, `api`.
- `commit_sha`, `image_digest`, `spec` (the app spec at start, as JSON) and `env` (the environment at start, sealed with `platform.key`).
- `log`: the build and deploy output, stored as a file under the data directory.

At most one deployment per app is `running`. A newer `running` deployment marks the previous one `superseded`.

## 7. Storage

The platform stores its data in one SQLite file, `platform.db`, in the config directory. The file follows the pattern of `SqliteAuditStore`: `CREATE TABLE IF NOT EXISTS` statements run at the first connection, and every query is parameterized.

```sql
CREATE TABLE IF NOT EXISTS targets (
    id            TEXT PRIMARY KEY,          -- ShortId
    name          TEXT NOT NULL UNIQUE,
    kind          TEXT NOT NULL,             -- 'local' | 'agent'
    token_hash    TEXT,                      -- SHA-256 of the enrollment token, NULL after enrollment
    cert_fingerprint TEXT,                   -- SHA-256 of the agent client certificate
    last_seen_at  INTEGER,
    created_at    INTEGER NOT NULL
);

CREATE TABLE IF NOT EXISTS apps (
    id            TEXT PRIMARY KEY,          -- ShortId
    name          TEXT NOT NULL UNIQUE,
    target_id     TEXT NOT NULL REFERENCES targets(id),
    spec          TEXT NOT NULL,             -- JSON of r3v3rs3_api::platform::AppSpec
    proxy_id      TEXT,                      -- the generated r3v3rs3 proxy
    created_at    INTEGER NOT NULL,
    updated_at    INTEGER NOT NULL
);

CREATE TABLE IF NOT EXISTS app_env (
    app_id        TEXT NOT NULL REFERENCES apps(id) ON DELETE CASCADE,
    key           TEXT NOT NULL,
    value         BLOB NOT NULL,             -- AES-256-GCM nonce + ciphertext
    secret        INTEGER NOT NULL,          -- 1 hides the value in the API
    PRIMARY KEY (app_id, key)
);

CREATE TABLE IF NOT EXISTS deployments (
    id            TEXT PRIMARY KEY,          -- ShortId
    app_id        TEXT NOT NULL REFERENCES apps(id) ON DELETE CASCADE,
    status        TEXT NOT NULL,
    trigger       TEXT NOT NULL,
    commit_sha    TEXT,
    image_digest  TEXT,
    spec          TEXT NOT NULL,             -- app spec snapshot JSON
    env           BLOB NOT NULL,             -- environment snapshot, sealed
    username      TEXT NOT NULL,
    message       TEXT,                      -- the reason of a failure
    started_at    INTEGER NOT NULL,
    finished_at   INTEGER
);
CREATE INDEX IF NOT EXISTS deployments_app ON deployments (app_id, started_at);

CREATE TABLE IF NOT EXISTS app_secrets (
    app_id        TEXT NOT NULL REFERENCES apps(id) ON DELETE CASCADE,
    name          TEXT NOT NULL,             -- 'git_token'
    value         BLOB NOT NULL,             -- sealed, associated data app/<app_id>/secret/<name>
    PRIMARY KEY (app_id, name)
);
```

The app spec is one JSON column, as the audit entry is, so a new spec field needs no schema change. Only fields that a query filters on get their own column.

Environment values are encrypted with AES-256-GCM. The key is a 32-byte file `platform.key` in the config directory, created at the first start with mode `0600`. The file has the format of a cluster key file, and the associated data of a value is `app/<app_id>/env/<key>`, so a value copied to another row does not decrypt. A value never leaves the server in plain text except inside the mTLS link to the agent that runs the container.

## 8. Deploy pipeline

One worker per app runs the steps in order. A global limit bounds the builds that run at the same time. Each step checks a cancel flag before it starts.

1. **Snapshot.** Copy the app spec and the sealed environment into the deployment row. The environment stays sealed at rest and is decrypted only when the container starts. The rest of the pipeline reads only the snapshot, so an edit during a deploy does not change it.
2. **Fetch source.** For `git`: clone or fetch into `builds/<app_id>/src`, check out the branch, record `commit_sha`. For `image`: skip.
3. **Build.** For `dockerfile`: build the image with the tag `r3v3rs3/<app>:<deployment_id>`. For `compose`: validate the file. For `image`: pull it.
4. **Resolve digest.** Read the image digest and store it. A rollback starts this digest, never a tag.
5. **Start the new container.** Name `r3v3rs3-<app>-<deployment_id>`, labels `r3v3rs3.app` and `r3v3rs3.deployment`, the app network, the limits, the environment.
6. **Wait for health.** Poll the health check path of the new container until it passes or a timeout expires. A failure stops the new container and marks the deployment `failed`. The old container keeps serving.
7. **Switch traffic.** Mark the deployment `running` and send a new snapshot of the `Platform` provider, whose proxy routes to the published port of the new container (section 9).
8. **Stop the old container** after a drain period of 10 seconds, then remove it.
9. **Finish.** Mark the deployment `running`, the previous one `superseded`, write the audit entry, send the notification.

This is blue-green: the old container serves until the new one passes its health check. The switch in step 7 is a proxy config change, which r3v3rs3 applies without a restart.

Phase 3 implements steps 1, 4 to 9 for image apps. One pipeline runs per app at a time (`AppLock`); a second deploy, a rollback or a deletion gets `409 app_busy`. The cancel flag and the finish notification follow with phase 8. A server restart marks an unfinished deployment `failed`.

Phase 4 implements steps 2 and 3 for Git apps. The deployment status is `building` during both steps. The build runs in `builds/<deployment_id>` under the config directory, which is removed after every build and at the start of the server. `git clone --depth 1 --single-branch` runs without the host configuration, without hooks, with only the HTTPS protocol, and with the token of the app in an `http.extraHeader` taken from the environment. The build context is a tar archive of the `context` directory without `.git`; symbolic links stay links. The Docker Engine builds it through `POST /build` with the classic builder, because BuildKit needs a gRPC session besides the request. At most two builds run at the same time. The image id of the build is the `image_digest` of the deployment. After every pipeline of a Git app, the images of the deployments after the newest five are removed, except the image of the running deployment.

**Rollback** creates a new deployment with `trigger = rollback` and the `image_digest`, `spec` and `env` of the chosen earlier deployment. It skips steps 2 and 3 and runs steps 4 to 9. A built image has no registry, so the rollback of a removed build fails.

**Compose** apps run the `docker` CLI with its Compose plugin, because the Engine API has no Compose endpoint. The project is `r3v3rs3-<app>`. The checkout is `compose/<app>/<deployment>/src` under the config directory, and the checkout of the running deployment stays, because a bind mount of the project reads it; every other checkout of the app is removed after `up`. `docker compose` runs with a cleared environment: `PATH`, `DOCKER_HOST` from the `docker` setting, `HOME` and `DOCKER_CONFIG` of the host, and the decrypted app environment, which fills the interpolation of the Compose file. An app variable with one of those four names fails the deployment. An override file `r3v3rs3.override.yaml` next to the checkout gives the service named in `source.service` the container name `r3v3rs3-<app>-<deployment>`, the platform labels, the app environment by name only (so no value reaches the disk), and `ports: !override ["127.0.0.1::<port>"]`, which replaces the ports of that service. `docker compose config --format json` then validates the project, and a published port on any other service fails the deployment before `up`. The status is `building` for the checkout and the validation and `deploying` from `up --detach --build --remove-orphans`. The container name changes with every deployment, so Compose recreates the service: the old container stops before the new one starts, and steps 6 to 9 run without the drain. A failed health check leaves the new container for its log. After every Compose job the dangling images with the label `com.docker.compose.project=<project>` are pruned through the Engine API. A rollback clones the `commit_sha` of the chosen deployment with a detached `git fetch --depth 1` of that commit and runs `up --build` again; a deployment without a commit cannot be rolled back. Deleting the app, or deploying it with another source kind, runs `docker compose down --remove-orphans`, prunes every image of the project and removes the checkouts. The volumes of the project stay.

## 9. Ingress integration

Each running app with at least one domain gets one r3v3rs3 HTTP proxy. The platform does not write the proxy into `proxies.toml`: it sends the proxies of all apps as the snapshot of the `Platform` discovery provider, so the existing discovery code resolves the ports, orders the certificates and shows the provider status, and the proxies are read-only like every discovered proxy.

- `vhosts`: the app domains.
- `routes[0].servers`: `http://127.0.0.1:<port>/`, the port that Docker publishes for the container port on `127.0.0.1`. On an agent target it is the port of a loopback forwarder of the master, which section 10.4 connects to the agent tunnel.
- `ports`: the ports of `proxy_ports` in the `[platform]` section of `config.toml`.
- `acme`: the existing ACME entry of `acme` in the `[platform]` section. The certificate follows the rules of discovered proxies.

`publish.rs` reads the running deployments and inspects their containers every 15 seconds, and sends a snapshot only when the result changed. A missing or stopped container becomes an issue of the provider status. `DiscoveryProvider::ALL` does not list `Platform`, so a change of the discovery settings never clears the app proxies. An app without a domain gets no proxy, because a proxy without virtual hosts would answer every host name of its ports.

An agent target publishes no app port on the agent host. Every request to an agent app travels through the mTLS link of that agent (section 10.4), so the agent host needs no inbound port and works behind NAT.

## 10. Agent

### 10.1 Enrollment

1. The operator creates an agent target in the WebUI or with `POST /api/targets`. The master generates a random secret of 43 alphanumeric characters, stores its SHA-256 and shows the enrollment token once. The token has the form `<secret>.<ca_hash>`, where `ca_hash` is the SHA-256 of the agent CA certificate.
2. The operator runs on the remote server one of the commands that the Targets page shows: `install.sh --agent --master <host:port> --token <token>`, which installs the `r3v3rs3-agent` systemd service, or `docker run` of the `-platform` image with `agent --master <host:port> --token <token>`.
3. The agent connects to the agent port without a client certificate. It accepts the server certificate only when its chain ends in a CA whose SHA-256 equals `ca_hash`, so the first connection needs no pre-shared CA file and still cannot be intercepted. The agent generates a key pair and sends an enrollment frame with the secret, a certificate signing request and its version. A connection without a client certificate may send only this one frame; the master closes it after the answer.
4. The master verifies the token, signs the certificate with its agent CA, stores the certificate fingerprint, clears the token hash, disconnects an agent of an earlier enrollment of the target and returns the certificate and the agent CA.
5. The agent stores its key (`agent.key`, mode `0600`), its certificate, the master CA and its target id in its data directory. Every later connection uses mTLS with that certificate, and the agent ignores a token.

The token is single use. `POST /api/targets/{id}/token` creates a new token; the enrolled agent keeps working until another agent enrolls with it, which replaces a lost agent key or moves a target to a new server. `DELETE /api/targets/{id}` deletes a target without apps. After either change the old certificate matches no target.

### 10.2 Transport

- The master listens for agents on a dedicated port on every IPv4 address, set by `agent_port` in the `[platform]` section of `config.toml` (for example `agent_port = 9443`). The listener is not an entry of the port list, so the port list cannot edit or delete it. Without `agent_port` the master accepts no agents. `agent_host` names the address that the enrollment commands of the WebUI show.
- The agent opens a TCP connection to that port and completes a TLS handshake with its client certificate. The listener verifies client certificates against the agent CA. A connection without one is limited to the enrollment frame of section 10.1.
- The master creates the agent CA and its own agent server certificate (subject name `r3v3rs3-master`) at the first start with `agent_port`, and stores them under `certs/agent/` in the config directory.
- After the handshake the master maps the certificate fingerprint to the target. A certificate that belongs to no target gets its connection closed before the first request. The agent reports such a close as a refused certificate.
- A stream multiplexer (the `yamux` crate) runs inside the TLS connection. The agent is the yamux server: the master opens one stream per request, and the agent answers it. TCP is used instead of QUIC, because networks block outbound UDP more often than TCP.
- A frame is a big-endian `u32` length and that many bytes of JSON, at most 8 MiB. A stream carries one request frame, an optional raw payload (a `u64` length and the bytes, for a build context or a deployment directory), and one answer frame. A request id is not needed, because the stream pairs the request with its answer, and concurrent requests use concurrent streams.
- A tunnel stream carries the bytes of one TCP connection after its answer frame, so a large transfer does not delay another request.
- The master pings the agent every 10 seconds, and a ping without an answer within 20 seconds marks the target offline. The agent reconnects when the master sends no request for 45 seconds or when the connection closes, with a pause that doubles from 1 to 60 seconds.

### 10.3 Protocol

The protocol is a closed set of typed requests. The agent never receives a program name, a shell string or raw arguments.

The requests follow the methods of `ContainerRuntime` and `ComposeRunner` (`r3v3rs3/src/agent/protocol.rs`). The enums are tagged externally, because an internally tagged enum cannot read the integer keys of the published port map.

```rust
pub enum AgentRequest {
    Ping,
    Version,
    PullImage { image: ImageRef },
    InspectImage { image: ImageRef },
    BuildImage { dockerfile: RelPath, tag: ImageRef },          // payload: the build context
    RemoveImage { image: ImageRef },
    PruneProjectImages { project: ProjectName, all: bool },
    EnsureNetwork { network: NetworkName, app: AppName },
    RemoveNetwork { network: NetworkName },
    CreateContainer { spec: ContainerSpec },
    StartContainer { name: ContainerName },
    StopContainer { name: ContainerName, timeout_secs: u64 },
    RemoveContainer { name: ContainerName },
    InspectContainer { name: ContainerName },
    ListContainers { app: AppName },
    Logs { name: ContainerName, tail: u32 },
    Tunnel { container: ContainerName, port: u16 },
    ComposeConfig(ComposeRequest),                              // payload: the deployment directory
    ComposeUp(ComposeRequest),
    ComposeDown { project: ProjectName },
    ComposeRetain { app: ShortId, keep: ShortId },
    ComposeRemove { app: ShortId },
}

pub enum AgentReply {
    Ok(AgentOutput),
    Error { message: String },
}
```

Every operand is a newtype that validates in `FromStr` and `Deserialize`: `ContainerName`, `NetworkName` and `AppName` accept `[a-z0-9][a-z0-9_.-]{0,62}`; `RelPath` rejects `..` and absolute paths. The agent checks again after deserialization (`r3v3rs3/src/agent/executor.rs`), so a compromised master cannot reach anything outside the platform:

- a container operation needs the `r3v3rs3.app` label; a foreign container is invisible, and stopping or removing it does nothing,
- a network or a Compose project needs the `r3v3rs3-` prefix, and a built image the `r3v3rs3/` prefix,
- `ContainerSpec::validate` runs before a create,
- a Compose deployment unpacks only below `compose/` of the data directory of the agent.

The git checkout stays on the master: the master sends the build context of a Git app and the deployment directory of a Compose app as a tar payload, so the agent host needs no `git`.

### 10.4 Tunnel

The proxy reaches an app on an agent target through the agent link, not through a port that the agent host opens to the network. The implementation differs from the first design, which gave the proxy an `agent://` upstream address: that change needed new code in the connection pool and in the health check. The master uses loopback forwarders instead, and the proxy code stays unchanged.

1. The container publishes its port on `127.0.0.1` of the agent host, as on the local target (decision 7).
2. `RemoteRuntime::inspect_container` replaces each published port with the port of a forwarder: a listener on `127.0.0.1:0` of the master for the target, the container and the port (`r3v3rs3/src/agent/forward.rs`). The deploy pipeline, the publish step and the app proxy see an ordinary `127.0.0.1` address.
3. Every connection that a forwarder accepts opens a stream with `AgentRequest::Tunnel { container, port }`. The agent checks that the container carries the label, runs and publishes `port`, connects to `127.0.0.1:<host port>` and answers `AgentOutput::Tunnel`.
4. Both sides copy bytes in both directions until either side closes.

HTTP/1.1, HTTP/2 and WebSocket run over the tunnel unchanged, and the health check of a deployment uses the same forwarder. The publish step keeps the route of an app on an offline target with its old forwarder port, and the forwarder closes every connection, so the proxy answers `502`. Deleting an app or a target closes its forwarders.

## 11. Access control

The platform uses the existing permissions of `r3v3rs3/src/accounts.rs`:

| Action | Permission |
|---|---|
| List apps, deployments, targets, read logs | `Read` |
| Create, change or delete an app, read or change its environment, deploy, restart, roll back | `Edit` |
| Create, enroll or revoke a target | `Admin` |

The first version has no app lists. An account with a proxy list gets `403 forbidden` for every platform request (`Caller::authorize_platform`). App lists and a separate permission for app changes can follow later. A secret environment value is never returned by the API; the WebUI shows it as set or not set.

The admin handlers call the platform directly and do not run inside the server loop, because a database query or a Docker call must not delay the ports and the other RPC methods. The admin API reads the platform handle once at its start through the `GetPlatform` RPC method, checks the permission itself and writes the audit entry itself, as `refresh` in `admin/cdn.rs` does.

## 12. API and WebUI

Every new route follows the existing rules: a `#[utoipa::path]` attribute, a unique `operation_id`, registration with `routes!`, and an entry in `OPERATIONS` of `r3v3rs3/tests/admin_openapi_test.rs`.

```
GET    /api/targets
POST   /api/targets                     create an agent target, returns the one-time token
POST   /api/targets/{id}/token          a new enrollment token for an agent target
DELETE /api/targets/{id}
GET    /api/apps
POST   /api/apps
GET    /api/apps/{id}
PUT    /api/apps/{id}
DELETE /api/apps/{id}
GET    /api/apps/{id}/env               a secret variable has no value
PUT    /api/apps/{id}/env               a secret variable without a value keeps its value
POST   /api/apps/{id}/deploy
POST   /api/apps/{id}/restart
GET    /api/apps/{id}/deployments
GET    /api/deployments/{id}
POST   /api/deployments/{id}/cancel
POST   /api/deployments/{id}/rollback
GET    /api/deployments/{id}/log        server-sent events while the deployment runs
POST   /hooks/apps/{id}                 Git webhook, HMAC-SHA256 signature
```

WebUI pages: Targets (list, create, enrollment command), Apps (list with status), App detail (settings, environment, domains, deployments, live log), and a deploy button with a confirmation. Every string is a key in `en.json` and `tr.json`.

## 13. Phases

Each phase ends with integration tests and docs, and is usable on its own.

1. **Runtime.** `ContainerRuntime` trait and `runtime::docker` with create, start, stop, remove, inspect, logs, pull, network. Integration test against a local Docker Engine, skipped when no socket exists.
2. **Store and model.** `platform.db` schema, `r3v3rs3-api` types, RPC methods and admin routes for apps and the local target.
3. **Pipeline for images.** Deploy an `image` source to the local target with blue-green switching and rollback. Generated proxy and ACME entry.
4. **Build.** Git fetch, Dockerfile build, Compose up. A second Docker image, `ghcr.io/kilimcininkoroglu/r3v3rs3:<version>-platform`, adds the `git` and `docker` binaries (with the Compose plugin). The existing distroless image stays unchanged.
5. **WebUI.** Targets, Apps, App detail, live log.
6. **Agent.** Agent CA, `agent_port` listener, enrollment, the request protocol, `agent::remote`, `r3v3rs3 agent` command.
7. **Tunnel.** Loopback forwarders and tunnel streams, health checks through the tunnel. Implemented together with phase 6.
8. **Webhooks and notifications.** Git push deploys, deploy events through `notify.rs`.
9. **Later.** Cluster mode, managed databases, volume backups to S3, a template registry, PR preview environments, metrics history.

## 14. Decisions

1. **Cluster mode.** The first version runs on a single server. `platform.db` is local to that server, so the platform refuses to start in cluster mode with a clear error. Cluster support is a later phase.
2. **Docker image.** A second image with the `-platform` tag suffix carries `git` and `docker`. The default image stays distroless and small. The `platform` target of the `Dockerfile` is `debian:trixie-slim` with `git` from apt and the static `docker` CLI, Compose and buildx plugins of the pinned `docker:<version>-cli` image. buildx is included, because without it Compose builds with the classic builder, which rejects `RUN --mount`. `latest` becomes `latest-platform`. In a container the platform needs host networking and the config directory on the same path as on the host, because the bind mounts of a Compose file resolve on the host. An installation through `install.sh` uses the `git` and `docker` binaries of the host.
3. **Agent port.** A dedicated `agent_port` setting in `config.toml`, outside the port list (section 10.2).
4. **Traffic to agent apps.** Through a tunnel inside the agent link, reached through a loopback forwarder on the master (section 10.4). The agent host opens no port to the network.
5. **App proxy ports.** The operator lists them in `proxy_ports` of the `[platform]` section. The platform opens no port of its own.
6. **App certificates.** From an existing ACME entry named by `acme` in the `[platform]` section. The platform creates no ACME entry.
7. **Container address.** Docker publishes the app port on a free port of `127.0.0.1`. r3v3rs3 reaches the container there, and other hosts do not. This works the same whether r3v3rs3 runs on the host or in a container with host networking.
8. **Proxy delivery.** The app proxies travel as the snapshot of the `Platform` discovery provider (section 9), not as stored proxies.
9. **Private repositories.** An HTTPS access token per app, sealed in `app_secrets`. SSH keys are not supported.
10. **Dockerfile builds.** Through the Docker Engine API, not the `docker` CLI, so a Dockerfile app needs only `git` on the host.
11. **Compose settings.** r3v3rs3 does not restrict a Compose file: privileged containers, host binds and the host network are allowed, so an account with the Edit permission can take over the host. The user reference states this risk. Only published ports are refused, except the one port of the served service that the override file sets.
12. **Compose rollback.** A rollback checks out the recorded commit and builds again, instead of keeping the built images of every deployment.
