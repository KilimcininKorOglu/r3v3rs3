# Deployment platform design

Status: draft. No code exists for this design yet.

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
  app.rs            App, AppSource, BuildSpec, RuntimeSpec, Domain binding
  deployment.rs     Deployment, DeploymentStatus, DeploymentTrigger
  server_target.rs  Target (local | agent), AgentInfo, AgentStatus
  agent.rs          AgentRequest, AgentResponse, AgentEvent (the wire protocol)

r3v3rs3/src/
  platform/
    mod.rs          PlatformState: owns the store, the runtimes and the queue
    store.rs        SQLite schema and queries
    queue.rs        one worker per app, a global concurrency limit
    pipeline.rs     the deploy steps of section 8
    routes.rs       generates the r3v3rs3 proxy for an app
    secrets.rs      AES-256-GCM encryption of environment values
  runtime/
    mod.rs          ContainerRuntime trait
    docker.rs       the local runtime over the Docker Engine API
    remote.rs       the agent runtime: sends AgentRequest, awaits AgentResponse
  build/
    mod.rs          Builder: clone, checkout, build image or run Compose
    git.rs          git clone and fetch through the `git` binary, with validated operands
  agent/
    server.rs       the agent listener on the master
    client.rs       the `r3v3rs3 agent` command
    session.rs      one connected agent: request ids, pending replies, heartbeat
  server/rpc/
    apps.rs         RPC methods for apps and deployments
    targets.rs      RPC methods for targets and agent tokens
```

The runtime split is the central abstraction. The pipeline talks only to `ContainerRuntime`. The local target uses `runtime::docker`; an agent target uses `runtime::remote`, which forwards the same calls to the agent, and the agent runs them against its own `runtime::docker`. One implementation of the Docker calls serves both targets.

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
7. **Switch traffic.** Replace the upstream server of the app proxy with the new container address. The existing health group drains the old server.
8. **Stop the old container** after a grace period, then remove it.
9. **Finish.** Mark the deployment `running`, the previous one `superseded`, write the audit entry, send the notification.

This is blue-green: the old container serves until the new one passes its health check. The switch in step 7 is a proxy config change, which r3v3rs3 applies without a restart.

**Rollback** creates a new deployment with `trigger = rollback` and the `image_digest`, `spec` and `env` of the chosen earlier deployment. It skips steps 2 and 3 and runs steps 4 to 9.

**Compose** apps use `docker compose -p r3v3rs3-<app> up -d`. The service named in `build.compose.service` receives the traffic. Compose apps deploy by recreate, not blue-green, because Compose manages its own container names. The design states this limit in the WebUI.

## 9. Ingress integration

Each app with at least one domain owns one generated r3v3rs3 HTTP proxy. `platform::routes` writes that proxy from the app spec:

- `vhosts`: the app domains.
- `routes[0].servers`: the address of the running container. On the local target it is the container IP on the app network. On an agent target it is an `agent://<target_id>/<container>:<port>` address, which section 10.4 resolves through the agent tunnel.
- `health_check`: the app health check path.
- `ports`: the HTTP and HTTPS ports the operator selects for apps in the platform settings.

The generated proxy carries a marker, so the proxy list shows it as managed by an app and the proxy edit form refuses direct edits. A certificate for the domains comes from an ACME entry that the platform creates with `http-01`.

An agent target publishes no app port on the agent host. Every request to an agent app travels through the mTLS link of that agent (section 10.4), so the agent host needs no inbound port and works behind NAT.

## 10. Agent

### 10.1 Enrollment

1. The operator creates an agent target in the WebUI. The master generates a random secret, stores its SHA-256 and shows the enrollment token once. The token has the form `<secret>.<ca_hash>`, where `ca_hash` is the SHA-256 of the agent CA certificate.
2. The operator runs on the remote server:
   ```
   r3v3rs3 agent --master master.example:9443 --token <token>
   ```
3. The agent connects to the agent port without a client certificate. It accepts the server certificate only when its chain ends in a CA whose SHA-256 equals `ca_hash`, so the first connection needs no pre-shared CA file and still cannot be intercepted. The agent generates a key pair and sends an enrollment frame with the secret and a certificate signing request. A connection without a client certificate may send only this one frame; the master closes it after the answer.
4. The master verifies the token, signs the certificate with its agent CA, stores the certificate fingerprint, clears the token hash and returns the certificate and the agent CA.
5. The agent stores its key, its certificate and the master CA in its data directory. Every later connection uses mTLS with that certificate.

The token is single use. A lost agent key is replaced by a new enrollment. Revoking an agent deletes its fingerprint, so its certificate stops matching any target.

### 10.2 Transport

- The master listens for agents on a dedicated port, set by `agent_port` in the `[platform]` section of `config.toml` (for example `agent_port = 9443`). The listener is not an entry of the port list, so the port list cannot edit or delete it. Without `agent_port` the master accepts no agents.
- The agent opens a TCP connection to that port and completes a TLS handshake with its client certificate. The listener verifies client certificates against the agent CA. A connection without one is limited to the enrollment frame of section 10.1.
- The master creates the agent CA and its own agent server certificate at the first start with `agent_port`, and stores them under `certs/agent/` in the config directory.
- After the handshake the master maps the certificate fingerprint to the target.
- A stream multiplexer (the `yamux` crate) runs inside the TLS connection, so both sides open independent streams over one connection. TCP is used instead of QUIC, because networks block outbound UDP more often than TCP.
- Stream 1 is the control stream. It carries newline-delimited JSON frames with a size limit of 8 MiB per frame.
- Log streams and tunnel connections each use their own stream, so a large transfer does not delay a control frame.
- The agent reconnects with exponential backoff. The master marks a target offline when no heartbeat arrives for 30 seconds.

### 10.3 Protocol

The protocol is a closed set of typed requests. The agent never receives a program name, a shell string or raw arguments.

```rust
pub enum AgentRequest {
    Ping,
    ImagePull { reference: ImageRef, auth: Option<RegistryAuth> },
    ImageBuild { context: BuildContext, dockerfile: RelPath, tag: ImageTag, args: Vec<(EnvKey, String)> },
    ImageInspect { reference: ImageRef },
    ContainerCreate { spec: ContainerSpec },
    ContainerStart { name: ContainerName },
    ContainerStop { name: ContainerName, timeout_secs: u32 },
    ContainerRemove { name: ContainerName },
    ContainerInspect { name: ContainerName },
    ContainerLogs { name: ContainerName, tail: u32, follow: bool },
    ContainerList { app: AppName },
    NetworkEnsure { name: NetworkName },
    ComposeUp { project: AppName, files: Vec<ComposeFile> },
    ComposeDown { project: AppName },
    GitFetch { url: RepoUrl, branch: GitRef, auth: Option<GitAuth> },
    Stats,
}

pub enum AgentResponse {
    Ok(AgentOutput),
    Error { kind: AgentErrorKind, message: String },
}

pub enum AgentEvent {
    Heartbeat { version: String, stats: HostStats },
    ContainerDied { name: ContainerName, exit_code: i64 },
    LogChunk { stream: StreamId, data: String },
}
```

Every operand is a newtype that validates in `FromStr` and `Deserialize`: `ContainerName`, `NetworkName` and `AppName` accept `[a-z0-9][a-z0-9_.-]{0,62}`; `RelPath` rejects `..` and absolute paths; `RepoUrl` accepts only `https://` and `ssh://`; `GitRef` rejects a leading `-`. The agent validates again after deserialization, so a compromised master cannot pass an operand that the types refuse.

Each request carries an id. The agent answers with the same id. Several requests run at the same time, bounded by a limit on the agent.

### 10.4 Tunnel

The proxy reaches an app on an agent target through the agent link, not through a published port.

1. The proxy selects an upstream server with an `agent://<target_id>/<container>:<port>` address.
2. The connection pool of the proxy opens a new stream on the link of that target and sends one header frame: `{"container": "<name>", "port": <port>}`.
3. The agent verifies that the container carries the `r3v3rs3.app` label, so the tunnel reaches only containers that the platform created. It then opens a TCP connection to the container address on its local Docker network.
4. Both sides copy bytes in both directions until either side closes.

From that point the stream is an ordinary byte stream. HTTP/1.1, HTTP/2 and WebSocket run over it unchanged. The health check of the app uses the same tunnel. An offline agent makes the upstream server unhealthy, and the proxy answers `502`.

## 11. Access control

The platform uses the existing permissions of `r3v3rs3/src/accounts.rs`:

| Action | Permission |
|---|---|
| List apps, deployments, targets, read logs | `Read` |
| Create, change or delete an app, read or change its environment, deploy, restart, roll back | `Edit` |
| Create, enroll or revoke a target | `Admin` |

The first version has no app lists. An account with a proxy list gets `403 forbidden` for every platform request (`Caller::authorize_platform`). App lists and a separate `EditApps` permission can follow later. A secret environment value is never returned by the API; the WebUI shows it as set or not set.

The admin handlers call the platform directly and do not run inside the server loop, because a database query or a Docker call must not delay the ports and the other RPC methods. The admin API reads the platform handle once at its start through the `GetPlatform` RPC method, checks the permission itself and writes the audit entry itself, as `refresh` in `admin/cdn.rs` does.

## 12. API and WebUI

Every new route follows the existing rules: a `#[utoipa::path]` attribute, a unique `operation_id`, registration with `routes!`, and an entry in `OPERATIONS` of `r3v3rs3/tests/admin_openapi_test.rs`.

```
GET    /api/targets
POST   /api/targets                     create an agent target, returns the one-time token
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
6. **Agent.** Agent CA, `agent_port` listener, enrollment, control protocol, `runtime::remote`, `r3v3rs3 agent` command.
7. **Tunnel.** `agent://` upstream addresses, tunnel streams, health checks through the tunnel.
8. **Webhooks and notifications.** Git push deploys, deploy events through `notify.rs`.
9. **Later.** Cluster mode, managed databases, volume backups to S3, a template registry, PR preview environments, metrics history.

## 14. Decisions

1. **Cluster mode.** The first version runs on a single server. `platform.db` is local to that server, so the platform refuses to start in cluster mode with a clear error. Cluster support is a later phase.
2. **Docker image.** A second image with the `-platform` tag suffix carries `git` and `docker`. The default image stays distroless and small. An installation through `install.sh` uses the `git` and `docker` binaries of the host.
3. **Agent port.** A dedicated `agent_port` setting in `config.toml`, outside the port list (section 10.2).
4. **Traffic to agent apps.** Through a tunnel inside the agent link (section 10.4). The agent host opens no app port.
