+++
title = "Deployment Platform"
description = "Deploy container images and route their domains through r3v3rs3"
weight = 0
+++

# Deployment Platform

The deployment platform runs apps as containers on the Docker Engine of the r3v3rs3 server, or of a remote server that runs the r3v3rs3 agent, and routes their domains through r3v3rs3 proxies. A new deployment starts next to the running container, and the proxy switches to it only after it passes its health check.

The platform is under development. This version deploys a ready image from a registry, builds an image from a Git repository with its Dockerfile, or starts the Docker Compose file of a Git repository, on the local Docker Engine or on an [agent target](#agent-targets), through the WebUI or the admin API.

## Enable the Platform

The settings page of the panel carries a **Deployment Platform** card. It turns the platform on and off while the server runs, and holds the Docker Engine address and the ports that serve the apps. The sidebar shows the platform pages only while the platform is on.

`config.toml` still carries the section for an unattended install, and the server reads it at its start:

```toml
[platform]
enabled = true
docker = "unix:///var/run/docker.sock"
proxy_ports = ["http", "https"]
acme = "bcd-fgh"
agent_port = 9443
agent_host = "master.example.com"
```

| Key           | Default                       | Description                                                                                                                                                                                                    |
|---------------|-------------------------------|----------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------|
| `enabled`     | `false`                       | Starts the platform.                                                                                                                                                                                           |
| `docker`      | `unix:///var/run/docker.sock` | The Docker Engine API: a Unix socket or plain TCP. TLS is not supported.                                                                                                                                       |
| `proxy_ports` | empty                         | The names or the ids of the ports that serve an app that selects no ports of its own, for example the HTTP port and the HTTPS port. When this list is also empty, such an app takes every HTTP and HTTPS port. |
| `acme`        | none                          | The id of the ACME entry that orders the certificates of the app domains. Without it the apps get no certificate. The settings card keeps this value; the ACME page manages the entries.                       |
| `agent_port`  | none                          | The TCP port on every IPv4 address that the agents of remote servers connect to. Without it the server accepts no agent. See [Agent targets](#agent-targets). The settings card keeps this value.              |
| `agent_host`  | none                          | The host name or the address of this server in the agent commands of the **Targets** page. Without it the page uses the host name of its own address. The settings card keeps this value.                      |

The settings card applies `enabled`, `docker` and `proxy_ports` while the server runs. `acme`, `agent_port` and `agent_host` stay as they are in the card; a manual change of `config.toml` applies after a restart.

At its first start the platform creates two essential files in the config directory:

- `r3v3rs3.db`: the unified SQLite database storing apps, environment variables, deployments, databases, targets, and Git provider connections alongside all server configuration.
- `r3v3rs3.key`: the master key that encrypts environment values, Git tokens and webhook secrets of the apps, and client secrets, private keys and tokens of Git provider connections. Its mode is `0600`. Keep a backup of it next to the backup of `r3v3rs3.db`, because encrypted values cannot be opened without it.

#### Key Rotation

To rotate the encryption key, click **Rotate Key** in the **Settings** page under **Deployment Platform**, run `r3 platform rotate-key` in the CLI, or send `POST /api/platform/key/rotate`. This generates a fresh 256-bit AES key, decrypts every stored secret in `r3v3rs3.db` using the existing key, re-encrypts them with the new key in a single atomic transaction, writes the new `r3v3rs3.key`, and safely archives the previous key to `r3v3rs3.key.old` (with mode `0600`).

Access to the Docker socket is equal to root access on the host. Enable the platform only on a server where r3v3rs3 may control every container.

## Projects and Environments

Applications and databases can be organized into **Projects** (e.g. *E-Commerce*, *Billing*, *Analytics*) and sub-tier **Environments** (e.g. *Production*, *Staging*, *Development*). This hierarchy provides structured organization, scoping, and environment-level configuration sharing:

- **Projects**: High-level organizational containers. When creating a project, a default `production` environment is provisioned automatically, or custom default environments can be supplied as a comma-separated list.
- **Environments**: Isolated deployment tiers within a project. Each environment holds its own apps, databases, and shared environment variables.
- **Environment Variables**: An environment can define shared key-value configuration and secrets. All applications deployed within that environment automatically inherit these variables.
- **Inheritance & Precedence**: During deployment, environment variables are merged in strict order (see [Environment Variables](#environment-variables)).
- **Standalone Apps**: Apps and databases can remain standalone (`project_id: null`) or be linked to any project and environment at creation or update.
- **WebUI Filtering**: The Apps and Databases dashboards allow filtering resources by project and environment, displaying clear badges for their current tier.

### Cross-Environment Config & Secret Sync

r3v3rs3 includes built-in cross-environment diffing and cryptographic secret synchronization to easily manage configuration across deployment tiers (e.g. *Staging* -> *Production* or *Development* -> *Staging*):

- **Visual Diff Comparison**: Side-by-side inspection comparing environment variables and secrets between any two environments within a project.
- **Diff Analysis & Metrics**:
  - `Identical`: Key and value (or secret state) match exactly across both environments.
  - `Different`: Key exists in both environments, but values or secret flags differ.
  - `Missing in Target`: Key is present in the source environment but missing in the target environment.
  - `Missing in Source`: Key is present in the target environment but missing in the source environment.
- **One-Click Synchronization**:
  - **Sync Missing Keys** (`sync_missing`): Instantly copies all variables and secrets present in the source environment that do not yet exist in the target environment, leaving existing target keys untouched.
  - **Clone All (Overwrite)** (`clone_all`): Replaces all target environment variables with a full clone from the source environment.
  - **Single Key Sync** (`selected_keys`): Synchronizes an individual variable from source to target with one click.
- **Cryptographic Isolation & Zero-Leak Secrets**: Secrets are never returned to the browser in plaintext. When syncing secrets across environments, the server decrypts the value in backend memory and immediately re-seals it using the target environment's cryptographic Associated Authenticated Data (`env_aad(target_env_id, key)`), ensuring that the target environment can decrypt it while preventing unauthorized cross-row decryption attacks.
- **Admin API**:
  - `GET /api/projects/{id}/environments/{source_id}/diff/{target_id}`: Returns the `EnvironmentDiffReport` comparing the two environments.
  - `POST /api/projects/{id}/environments/sync`: Accepts `EnvironmentSyncRequest` (`source_env_id`, `target_env_id`, `mode`, `keys`) and returns `EnvironmentSyncReport`.

## Apps

An app names an image, the port that the container listens on and the domains that route to it:

```json
{
  "name": "shop",
  "target": "local",
  "spec": {
    "source": {"type": "image", "image": "nginx:1.27"},
    "port": 80,
    "domains": ["shop.example.com"],
    "health_check_path": "/healthz",
    "volumes": [{"volume": "shop-data", "target": "/data"}],
    "restart": "unless-stopped",
    "limits": {"memory_bytes": 268435456, "nano_cpus": 500000000}
  }
}
```

- `target` is `local`, the Docker Engine of the r3v3rs3 server, or the id of an [agent target](#agent-targets). The target of an app cannot change after its first deployment: a change answers `409 app_target_fixed`.
- `domains` are DNS names. A wildcard or an IP address is not accepted, because each domain gets a certificate from ACME.
- `ports` are the names or the ids of the ports that serve the app, as the **Ports** and **Extra Ports** fields of the app form select them. An app without `ports` takes the ports of `proxy_ports`, or every HTTP and HTTPS port when that setting is also empty.
- `health_check_path` is an HTTP path that answers `2xx` or `3xx` when the app is ready. Without it a deployment waits until the port accepts a connection and keeps it open.
- `volumes` mounts named Docker volumes, and `"read_only": true` mounts one read-only. A host path cannot be mounted.
- `restart` is `no`, `on-failure`, `unless-stopped` (the default) or `always`.
- `auto_deploy` (optional, default `false`) enables automatic deployment when a newer image tag or updated digest is detected in the container registry.
- `bot_blocker` (optional, default `false`) enables automated AI scraper and bot protection for the app's proxy routes.
- `grpc_web` (optional, default `false`) enables gRPC-Web proxy translation for browser gRPC clients.

A change of an app, of its domains or of its environment variables takes effect at the next deployment.

### Image Updates

For apps with an image source, r3v3rs3 periodically inspects the container registry (Docker Hub, GHCR, etc.) in the background:
- **Registry Tag & Digest Checking**: Checks for newer semver tags matching the image suffix or new manifest digests for the current tag.
- **Update Notifications**: If an update is found, an **Update available** badge appears on the app in the apps list. The app view page displays an update alert banner with current and latest versions and a **Deploy update** button.
- **Auto-Deploy**: When **Auto-deploy on update** is enabled, r3v3rs3 updates the app specification to the newer tag and triggers a deployment automatically.
- **Manual Checks**: An immediate update check can be triggered via the **Check for updates** button in the WebUI or `POST /api/apps/{id}/check_update`.

### Bulk Operations

The app, port, and proxy list views provide multi-selection controls:
- Select multiple items using row checkboxes, or select all visible items using the header checkbox.
- Perform batch deletions with confirmation.
- Deploy multiple selected apps at once.

## App Store

The App Store lists the apps that r3v3rs3 can install in one click. The catalog is the `app-store.json` file of the [r3v3rs3-apps](https://github.com/KilimcininKorOglu/r3v3rs3-apps) repository; the binary also ships an embedded copy, so that a fresh install starts with a full list, and refreshes from the repository once a day. The **Refresh catalog** button of the page downloads the current catalog at once. The API reads it at `GET /api/store` and refreshes it with `POST /api/store/refresh`.

An **image entry** carries the image and the port of the app. **Install** opens the app form with those fields filled in; choose the target, the domains and the ports, then create the app. The first deployment starts as soon as the app is created.

A **Compose entry** points at a Compose file under `app-store-compose/` of the r3v3rs3-apps repository, so an app that needs its own database installs in one click too. The form opens in the Compose mode, and the entry lists the **Variables** that the app needs, such as a database password: fill them in the form, and they go into the environment of the app before its first deployment. A Compose app deploys with a plain `docker compose` run, so it has no blue-green switch and rolls back by deploying an earlier commit.

The catalog holds no other Compose source: an entry that installs your own Compose file belongs on the [Apps](#apps) page, not in the catalog.

### Categories, Search, Logos, and Screenshots

The App Store provides instant search and category filtering:
- **Search**: Filters catalog applications dynamically in real time by name, description, container image, or category.
- **Categories**: Applications are organized into categories such as *Monitoring*, *Security*, *Development*, *Productivity*, *Automation*, *Database*, *Files*, *Media*, *Analytics*, *Social*, and *Management*. Click any category pill to filter the catalog, or click **All** to display all applications.
- **Logos & Screenshots**: Every catalog entry features its official vector/raster logo icon and high-resolution screenshot preview self-hosted directly in the `r3v3rs3-apps` repository, ensuring fast and reliable display without external link rot or third-party rate limits.

### Catalog Updates and One-Click Upgrade

When an app installed from the App Store has a newer version available in the catalog (for example, an updated image tag or an updated Compose configuration), r3v3rs3 displays an **Update available in catalog** badge next to the app in the app list and on the app details page. Clicking **Upgrade to Catalog Version** (or calling `POST /api/apps/{id}/upgrade-catalog`) updates the app specification to match the latest catalog entry and immediately triggers a deployment with the new version.

### Git Source

An app with a `git` source builds its image from a branch of a repository with the Dockerfile of the repository:

```json
"source": {
  "type": "git",
  "repository": "https://github.com/owner/shop.git",
  "branch": "main",
  "context": ".",
  "dockerfile": "Dockerfile"
}
```

| Field         | Default      | Description                                                                                                           |
|---------------|--------------|-----------------------------------------------------------------------------------------------------------------------|
| `repository`  |              | An `https://` URL without a user name, a password, a query or a fragment.                                             |
| `branch`      | `main`       | A branch or a tag.                                                                                                    |
| `context`     | `.`          | The directory of the repository that Docker receives as the build context.                                            |
| `dockerfile`  | `Dockerfile` | The Dockerfile, relative to `context`.                                                                                |
| `connection`  | none         | The id of a [Git provider](#git-providers) connection. The connection clones the repository and installs its webhook. |
| `tag_trigger` | none         | An optional tag pattern (e.g. `v*`, `release-*`) that automatically deploys matching tags when pushed.                |

A private repository needs a [Git provider](#git-providers) connection or an access token of the app. An access token is, for example, a GitHub fine-grained token with read access to the contents of the repository, a GitLab token with the `read_repository` scope, or a Gitea token with the `read:repository` scope. `PUT /api/apps/{id}/git_token` with `{"token": "..."}` sets the token, and `DELETE /api/apps/{id}/git_token` deletes it. r3v3rs3 encrypts the token with `r3v3rs3.key`, and the admin API never returns it: an app shows only `"git_token_set": true`. git receives the token as HTTP basic authentication with the user name `x-access-token`, through an environment variable instead of the URL or the command line.

The server that runs r3v3rs3 needs the `git` binary. r3v3rs3 runs `git` without the configuration of the host, without hooks and only over HTTPS. The build context holds no `.git` directory, and a symbolic link in it stays a link, so a build does not read a file from outside the repository. The context can hold up to 512 MiB.

Docker builds the image with its builder. Two builds run at the same time, and every other build waits. The builder keeps the stages of a multi-stage Dockerfile as its build cache, so the next build of the same app is faster. `docker image prune` removes that cache.

#### BuildKit and Cache Mounts

When building images for Git applications, r3v3rs3 supports Docker Buildx / BuildKit with automatic cache mount injection:

- **Enable BuildKit & Build Cache**: Automatically invokes `docker buildx build --load` instead of the legacy Docker builder.
- **Remote BuildKit Builder**: Optionally point to an external BuildKit daemon endpoint (`buildkit.remote_addr`, e.g. `tcp://10.0.0.5:1234` or `unix:///run/buildkit/buildkitd.sock`) to offload heavy compilation from the host server.
- **Automatic Cache Mount Presets**: r3v3rs3 can automatically inject `# syntax=docker/dockerfile:1` and `--mount=type=cache` directives into Dockerfile `RUN` steps for common language ecosystems without requiring manual Dockerfile modifications:
  - `Cargo`: Caches `/usr/local/cargo/registry` and `/root/.cargo/registry`.
  - `Npm` / `Pnpm` / `Yarn`: Caches `/root/.npm`, `/root/.pnpm-store`, and `/usr/local/share/.cache/yarn`.
  - `Go`: Caches `/go/pkg/mod` and `/root/.cache/go-build`.
  - `Pip`: Caches `/root/.cache/pip`.
  - `Maven` / `Gradle`: Caches `/root/.m2` and `/root/.gradle`.
- **Custom Cache Mounts**: Add custom target paths inside the build container.
- **External Cache Backends**: Configure `cache_from` and `cache_to` (e.g. `type=registry,ref=myregistry/app:cache,mode=max` or `type=local,src=...`) to reuse build layers across multiple worker nodes or CI runs.

### Compose Source

An app with a `compose` source starts the Docker Compose file of a branch of a repository:

```json
"source": {
  "type": "compose",
  "repository": "https://github.com/owner/shop.git",
  "branch": "main",
  "file": "deploy/compose.yaml",
  "service": "web"
}
```

| Field         | Default                                                                                    | Description                                                                                                              |
|---------------|--------------------------------------------------------------------------------------------|--------------------------------------------------------------------------------------------------------------------------|
| `repository`  |                                                                                            | An `https://` URL, as for a Git source. The connection or the Git token of the app clones a private repository.          |
| `branch`      | `main`                                                                                     | A branch or a tag.                                                                                                       |
| `file`        | the first of `compose.yaml`, `compose.yml`, `docker-compose.yaml` and `docker-compose.yml` | The Compose file, relative to the root of the repository. Its relative paths resolve against its own directory.          |
| `service`     |                                                                                            | The service that receives the requests of the domains on `port`. A service name has lowercase letters, digits and `_.-`. |
| `connection`  | none                                                                                       | The id of a [Git provider](#git-providers) connection, as for a Git source.                                              |
| `tag_trigger` | none                                                                                       | An optional tag pattern (e.g. `v*`, `release-*`) that automatically deploys matching tags when pushed.                   |

The server needs the `git` binary and the `docker` binary with the Compose plugin. r3v3rs3 runs `docker compose` with the Compose project `r3v3rs3-<app id>` and adds a file that sets for the service of `service`:

- the container name `r3v3rs3-<app id>-<deployment id>` and the labels of the platform,
- one published port: `port` on a free port of `127.0.0.1`. This port replaces the `ports` of the service in the Compose file.
- the environment variables of the app. The file names only their keys, and `docker compose` reads the values from its own environment, so no value reaches the disk. The same values fill the `${VARIABLE}` references of the Compose file. The names PATH, HOME, DOCKER_CONFIG and DOCKER_HOST belong to the `docker` binary, and an app that sets one fails its deployment.

Another service of the Compose file must not publish a port, because a published port bypasses r3v3rs3. A Compose file with such a port fails the deployment before any service starts, and the message names the service. The services reach each other on the network of the project.

A Compose app sets `volumes`, `restart` and `limits` in the Compose file, so its app spec does not accept them. The service of `service` runs as one container, because it has a container name.

A Compose deployment recreates the service of `service`: Docker stops the old container before the new one starts, so the app does not answer until the new container passes its health check. A deployment that fails its health check keeps the new container for its log, and the app has no healthy container until the next deployment or rollback. The checkout of the running deployment stays in `compose/<app id>/` of the config directory, because a bind mount of the Compose file can read it.

r3v3rs3 does not check the other settings of a Compose file. A Compose file can mount a host path, run a privileged container or use the network of the host, so every account that can edit an app can take over the host through it. Give the Edit permission only to accounts that may do that.

#### Compose Service-Level Health and Logs

For Compose applications consisting of multiple services:
- **Services Card**: The app details page displays a **Services** card listing each service container in the Compose project, including container name, state (`running` or `exited`), container healthcheck status (`Healthy`, `Starting`, `Unhealthy`, or `None`), and exit code if terminated.
- **Per-Service Logs**: On the app's **Log** page, a **Service** selector dropdown allows operators to switch between all services (the primary traffic container) and any individual container in the Compose project. Both static log retrieval (`GET /api/apps/{id}/logs?service=<name>`) and live WebSocket log streaming (`GET /api/apps/{id}/logs/live?service=<name>`) support the service filter.
- **API**: The list of service containers and their health status can be queried directly via `GET /api/apps/{id}/services`.

#### Compose Architecture Topology and Service Dependency Graph

For Docker Compose applications, r3v3rs3 provides an interactive SVG architecture visualizer and service dependency graph:

- **Interactive Architecture Graph**: Visually displays all declared services, networks, and persistent volume mounts as modular cards arranged in dependency order.
- **Dependency & Relationship Edges**:
  - `depends_on`: Solid blue directed bezier arrows indicating service boot dependencies, annotated with health conditions (`healthy`, `started`, `completed`).
  - `networks`: Dashed cyan connections representing container network attachments.
  - `volumes`: Dotted purple connections showing persistent volume attachments and bind mounts, with access mode labels (`ro`/`rw`) and mount paths.
- **Real-Time Health & Runtime Status**: Nodes display live runtime indicators: green halos for healthy/running containers, amber for starting/restarting, and red for exited or unhealthy states. The primary external traffic entrypoint is highlighted with a blue route badge.
- **Node Inspector Flyout**: Clicking any node opens an inspection panel detailing image tags, port mappings, environment variable counts, restart policies, container names, and complete upstream/downstream dependency lists.
- **Filter & Zoom Controls**: Filter by node type (All, Services, Networks, Volumes) and zoom in/out with responsive viewBox scaling.
- **Live Preview in Compose Editor**: The WebUI Compose YAML editor includes a dedicated **Architecture Graph** tab that renders the dependency topology in real time as you edit or select templates.
- **API**: Query the complete parsed topology and dependency graph via `GET /api/apps/{id}/topology`.

#### Visual Compose Editor and Environment Variable Completion

The WebUI includes a visual Docker Compose YAML editor that makes creating and managing multi-service stacks intuitive and error-free:

- **Inline Compose YAML Support**: Applications can be deployed directly from inline YAML (`compose_yaml` in `AppSource::Compose`) without creating an external Git repository, or by pointing to a remote Git repository file.
- **Real-Time YAML Syntax Validation**: Powered by `serde_yaml`, the editor continuously validates YAML structure as you type. If an indentation, mapping, or formatting error occurs, the editor immediately flags the exact line in the gutter and displays a clear error explanation.
- **Service Detection & Click-to-Route**: The editor automatically detects and displays all services defined under `services:`. Clicking any detected service pill sets it as the primary routing service for external domain traffic.
- **Environment Variable & Secret Completion**: Quickly insert environment variables and encrypted secrets (`${VAR_NAME}`) directly into the YAML with dedicated chips and syntax completion.
- **Starter Architecture Templates**: One-click boilerplate templates for popular deployment patterns:
  - *Nginx Web Server* (standalone web container with port mappings).
  - *Node.js + PostgreSQL* (full-stack API connected to a persistent PostgreSQL database).
  - *Python + Redis* (background task/caching queue architecture).
  - *WordPress + MySQL* (complete CMS with database backend).
- **Syntax Preview**: Switch between interactive edit mode and token-highlighted preview mode with colorized keys, strings, and variable references.

### Environment Variables

`PUT /api/apps/{id}/env` replaces the environment variables of an app. A variable with `"secret": true` never leaves the server again: the admin API returns it without its value, and an update without a value keeps the current value. r3v3rs3 encrypts every value with `r3v3rs3.key` and decrypts it only when a container starts.

#### Git-Based Configuration

An app can also load environment variables directly from a `.env` file in a Git repository during deployment:

- In the app form, specify the **Config File Path** (e.g. `.env` or `config/production.env`), or configure `git_config` in `AppSpec`.
- For Git and Compose apps, the file is read from the app's repository checkout by default. You can optionally specify a dedicated **Config Repository** and **Config Branch**.
- For Image apps, specify both the repository URL and the file path.
- The file supports standard `.env` format, including comments (`#`), quotes, and `export` prefixes.
- Explicit panel environment variables override variables loaded from the Git repository, allowing panel-managed secrets to take precedence over repository defaults.

#### Environment Variable Groups

Environment variable groups allow managing reusable collections of configuration variables and secrets shared across multiple apps:

- Manage groups in the WebUI under **Platform -> Environment Variable Groups** or via `/api/env-groups`.
- Each group has a unique name, an optional description, and a list of key-value variables with optional `"secret": true` masking.
- In the app form, check the desired environment variable groups to link them to the application (`env_groups` in `AppSpec`).
- **Precedence Hierarchy**: During deployment, environment variables are merged in the following order:
  1. Base `.env` file loaded from the Git repository (when `git_config` is specified).
  2. Linked environment variable groups (in the order listed in `env_groups`; later groups override earlier ones).
  3. Environment shared variables defined at the Project Environment level (`/api/projects/{project_id}/environments/{environment_id}/env`).
  4. App-specific variables defined directly on the app (override group and environment-level definitions).
  5. Linked database connection variables (e.g. `DATABASE_URL`).
- Secret variables stored in environment variable groups are sealed with cluster/platform keys and are resealed during key rotation (`POST /api/platform/key/rotate`).
- A group cannot be deleted while it is currently linked to any active application.

#### Compose .env Template Resolver & Auto-Secret Generator

Deploying Compose applications from the App Store catalog or external Git repositories frequently requires setting initial secrets and database credentials (such as `POSTGRES_PASSWORD`, `APP_KEY`, `JWT_SECRET`, `ADMIN_PASSWORD`). r3v3rs3 provides an automated template resolver and cryptographically secure secret generator:

- **Heuristic Pattern Matching**: Variable names containing or ending with `_PASSWORD`, `_SECRET`, `_KEY`, `_TOKEN`, `_SALT`, `_HASH`, `_PASSPHRASE`, or known credentials (`JWT_SECRET`, `ROOT_PASSWORD`, `ADMIN_PASSWORD`) are automatically recognized as sensitive secrets.
- **Smart Placeholder Detection**: Values that are empty or set to common template placeholders (`changeme`, `replace_me`, `your_secret_here`, `<secret>`, `TODO`, `""`) are detected for generation while preserving explicit production values.
- **Cryptographic High Entropy**: Passwords are generated as 32-character alphanumeric strings (~190 bits entropy) that are 100% safe from shell expansion and YAML syntax issues. Keys, tokens, and hashes are generated as 64-character lowercase hex strings (256 bits entropy).
- **WebUI One-Click Auto-Fill**:
  - In the App Store pre-deployment form, click **Auto-generate Missing Secrets** (`apps.store_env_generate_secrets`) to immediately populate empty secret inputs with secure random tokens before starting the first deployment.
  - In the **Environment Variables** tab of an application (`/apps/{id}/env`), click **Auto-generate Missing Secrets** (`apps.env_generate_secrets`) to fill missing secret values into the form for inspection before saving.
- **CLI & MCP Management**:
  - CLI: `r3 app generate-env <APP_ID> [--save] [--overwrite]` and `r3 app resolve-template [--template <STR>] [--file <PATH>] [--overwrite]`.
  - MCP: `generate_app_env_secrets` and `resolve_compose_env_template`.
  - REST API: `POST /api/apps/{id}/env/generate` and `POST /api/compose/env/generate`.

## Databases

The **Databases** page installs a database of one engine in one click. The engines are PostgreSQL 17 and 18, MariaDB 11 and 12, MySQL 9, MongoDB 8 and Redis 8, each with its image tag pinned to a version. The engine catalog is the `engines.json` file of the [r3v3rs3-apps](https://github.com/KilimcininKorOglu/r3v3rs3-apps) repository; the binary also ships an embedded copy and refreshes from the repository once a day. The **Refresh catalog** button of the page downloads the current catalog at once; the API refreshes it with `POST /api/databases/engines/refresh`.

**Add a database** takes an engine, a version, a name and, optionally, a port of `127.0.0.1`. Optional **Memory Limit (MB)**, **Swap Limit (MB)**, and **CPU Limit** fields allow setting container resource throttling directly at installation. Without a port the server takes a free one at the creation, and the database keeps that port afterwards: every deployment of a database publishes its port on the same address, unlike an app, whose deployment takes a new port. The creation builds an app on the local target with the image of the version, one named volume for the data of the engine, and a generated password of 32 hex characters as a secret environment variable. The first deployment starts at once.

The response shows the password and the connection string once, like a webhook secret. The panel lists every database with its engine, its port and its connection without the password. The database answers on `127.0.0.1` of the r3v3rs3 server, so only the apps of this server and the programs on the server itself reach it: a database gets no domain and no proxy, and **Delete** on its row removes the app while the volume with the data stays. Redis takes no password, so its port answers to every local client.

A deployment of a database is not blue-green, because the fixed port frees only when the old container stops: r3v3rs3 stops the old container, starts the new one and waits for its health check, so the database answers a few seconds later.

### Connect an App

The **Database** field of the app form connects an app to a database of this server. The next deployment of the app injects the connection into the environment of its container: `DATABASE_URL` with the user, the generated password and the port, plus the variables of the engine, for example `POSTGRES_HOST`, `POSTGRES_PORT`, `POSTGRES_USER`, `POSTGRES_PASSWORD` and `POSTGRES_DATABASE` for PostgreSQL, and `MYSQL_*` for MariaDB and MySQL, `MONGO_*` for MongoDB and `REDIS_URL` for Redis. The container resolves `host.docker.internal` to the host of r3v3rs3, so the address works from every app network.

The injected user is the root user of the database, because r3v3rs3 creates no per-app users. While the database does not run, the deployment of a connected app fails with the reason, and the old container keeps serving. A database that an app uses cannot be deleted: the deletion answers `409 database_in_use` until the apps disconnect from it, and the databases page shows the apps that use each database.

A database needs the local target, and only an account with the Edit permission sees the page. Every creation and deletion goes into the audit log.

### Database Cloning & Branching

The **Clone** button on a database row creates a new, independent copy (branch) of an existing database:

- **Prompt & Creation**: Asking for a new database name (defaults to `{name}-clone`) and an optional host port, r3v3rs3 creates a new database app with its own dedicated volume (`{name}-data`), a unique generated password, and an allocated port on `127.0.0.1`.
- **Live Stream or Backup Fallback**: If the source database is currently running, r3v3rs3 takes a live dump directly from the running container via `exec`. If the source database is stopped, it automatically uses the latest verified backup dump.
- **Automated Restore**: Once the new database container is initialized and healthy, the dump data is automatically restored into the cloned database.
- **Isolated Testing**: The cloned database has its own isolated volume, port, and credentials, allowing developers to test migrations, run staging environments, or debug against production data without affecting the primary database.

### Database Read Replicas & Read Load Balancing

For high read traffic and read/write splitting, a database row features a **Create Replica** action:

- **One-Click Provisioning**: Asking for a replica name (defaults to `{name}-replica`), r3v3rs3 provisions a new replica database pointing to the primary database (`replica_of`). It automatically synchronizes the root authentication credentials with the primary and starts the replica container.
- **Dedicated Read Endpoint**: Each replica operates on its own dedicated host port and volume, isolating read-heavy analytical or reporting queries from write traffic.
- **Automatic Multi-Replica Environment Injection**: When an application binds to a primary database that has running read replicas, r3v3rs3 automatically enriches its container environment:
  - `DATABASE_URL`: The primary database connection string for writes.
  - `DATABASE_READ_URL` & `DATABASE_REPLICA_URL`: Connection strings targeting the primary's active read replicas.
  - `DATABASE_READ_URLS`: A comma-separated list of all active read replica connection strings, directly consumable by multi-host connection pools (such as JDBC, Prisma, pgpool, or typeorm) to balance read traffic across all replicas.
  - Engine-specific read variables (e.g., `POSTGRES_READ_HOST`, `POSTGRES_READ_PORT`, `MYSQL_READ_HOST`, etc.).

### Database Web Administration UIs

For visual inspection, querying, and schema management, r3v3rs3 provides official one-click database management web interfaces in the **App Store**:
- **pgAdmin 4**: Feature-rich administration and SQL development platform for PostgreSQL.
- **phpMyAdmin**: Web interface for MySQL and MariaDB management with arbitrary server support.
- **Redis Commander**: Fast web-based key-value management tool for Redis instances.
- **Adminer**: Lightweight, single-container multi-engine database manager (PostgreSQL, MySQL, SQLite, MongoDB).
- **Mongo Express**: Interactive document and collection manager for MongoDB.

On the **Databases** page, each database row includes a direct **Web UI** action that opens the corresponding App Store template pre-configured for the database engine.

### Backups

The **Backups** link on a database row opens its backups. **Back up now** runs the dump of the engine inside the container (`pg_dump`, `mariadb-dump`, `mysqldump` or `mongodump`; Redis runs `SAVE` and takes its dump file), gzips it and stores it in the `backups` directory of the config directory, one folder per database, with a JSON manifest that carries the SHA-256 of the file. A backup holds no password. A database on an agent target takes no backup, because the dump runs in the container itself.

The backup settings configure the daily time in UTC that every database is backed up at, the number of backups per database that survive (the oldest go first), remote upload destinations, and heartbeat health checks:

- **S3**: An S3-compatible bucket endpoint with region, path prefix, access key, and secret key.
- **SFTP**: Uploads dumps via SSH/SFTP specifying `host`, `port` (default 22), `user`, target `path`, and an optional private key or password.
- **rsync**: Transfers dumps using `rsync` over SSH to a remote target directory.
- **Dead Man's Snitch / Heartbeat Monitoring**: An optional heartbeat ping URL (e.g., Healthchecks.io, Dead Man's Snitch, or Better Stack). Upon successful execution of daily scheduled backups, r3v3rs3 sends an HTTP GET ping with the `r3v3rs3-heartbeat` User-Agent to this endpoint. If any scheduled backup fails, the ping is skipped and a `backup.failure` outbound webhook is dispatched instead, alerting external monitoring services to silent backup failures.

Every new backup uploads to the configured remote targets; a failed upload only goes into the server log, because the local copy exists. Remote credentials are encrypted with `r3v3rs3.key`.

#### Restore & Periodic Verification Test

**Restore** writes the stored dump back into the running container and runs the restore command of the engine (`psql`, `mariadb`, `mysql`, `mongorestore --drop`). Redis takes the dump file into its data volume and restarts, so the restore answers after the server is back. A restore replaces the data of the database.

To ensure that backups remain uncorrupted and recoverable, r3v3rs3 includes an automated periodic restore testing and dry-run simulation mechanism. The system verifies that gzipped dumps decompress cleanly, matches expected database format signatures, and conducts in-memory restore drill simulations:

- **Schema & Data Drill Simulation**: Without starting auxiliary containers or polluting production databases, the engine decompresses the dump in memory and parses schema objects:
  - **SQL Dumps (PostgreSQL / MySQL / MariaDB)**: Parses `CREATE TABLE` definitions, extracts declared table names, detects data payloads (`COPY ... FROM stdin` or `INSERT INTO`), and verifies clean EOF completion markers (e.g. `-- PostgreSQL database dump complete`).
  - **Redis RDB**: Validates binary magic signatures (`REDIS`), version numbers, and file EOF checksums.
  - **SQLite 3**: Checks standard SQLite header strings, valid page sizes, and format versions.
- **Verification Diagnostics & Table Samples**: Each verified backup records detailed drill metrics: execution duration (`drill_duration_ms`), discovered table count, a sample of extracted table names, total schema object count, and whether actual row data was found.
- **Interactive Drill Report Modal**: In the WebUI, clicking the verified badge (`✓ Verified (N tables)`) opens a detailed diagnostics modal showing status banners, format verification stats, duration, payload presence, and extracted table chips. Operators can re-trigger an immediate verification dry-run via **Run Restore Drill** or `POST /api/databases/{id}/backups/{name}/verify`.
- **Emergency Verification Failure Alerts**: If a backup fails checksum verification, gzip decompression, or schema drill integrity checks, r3v3rs3 dispatches an outbound `backup.verification_failed` webhook event (`BackupVerificationFailed`) to immediately alert monitoring systems before data recovery is needed during an outage.

#### Point-in-Time Recovery (PITR) & Continuous Archiving

Beyond nightly or periodic full dumps, r3v3rs3 provides Point-in-Time Recovery (PITR) via continuous Write-Ahead Logging (WAL) and binary log archiving:

- **Recovery Window**: The backups page displays the exact active recovery timeline (from the earliest base backup to the latest recorded change log segment) along with total base backups and archived WAL count.
- **WAL & Log Archiving**: Rotating transaction logs are archived in `<config-dir>/backups/<db_id>/wal/`. Users can trigger manual segment archiving with **Archive WAL Now** or use automated background checkpoints (`POST /api/databases/{id}/wal/archive`):
  - **PostgreSQL**: Archives WAL segments using transaction log rotation (`pg_switch_wal()`) and continuous WAL capture.
  - **MySQL / MariaDB**: Archives rotated binary logs (`FLUSH BINARY LOGS;`).
  - **MongoDB**: Archives oplog mutation archives (`mongodump --oplog`).
  - **Redis**: Captures point-in-time state and Append-Only File (AOF) journals.
- **Precision Recovery**: The **PITR Restore** action prompts for an exact target timestamp (in RFC3339 format, e.g. `2026-10-21T04:30:00Z` or unix milliseconds). r3v3rs3 automatically:
  1. Identifies the latest base backup created prior to the target timestamp and restores it.
  2. Selects all subsequent archived WAL/log segments between the base backup and the recovery moment.
  3. Replays transactions sequentially up to the requested second or millisecond.
- **Safe Branching (PITR Clone)**: If a new branch name is specified, r3v3rs3 provisions an entirely new, isolated database with its own host port, data volume, and root credentials, letting developers inspect past state or extract lost rows without overwriting production. Leaving the branch name blank performs an in-place restore after confirmation.

#### Remote S3 Backup Browser & Disaster Recovery

When local storage is destroyed, disks fail, or a host is rebuilt from scratch, r3v3rs3 connects directly to external AWS S3, Cloudflare R2, or MinIO buckets to browse, download, and restore archived snapshots:

- **S3 Snapshot Browser**: View all database dumps stored in the configured S3 bucket via the WebUI or `GET /api/databases/s3/backups`. The browser displays the originating database name, backup ID, uncompressed size, and timestamp, with live text filtering.
- **Direct Snapshot Download**: Download any remote dump archive directly to your workstation via `GET /api/databases/s3/download?key={key}`.
- **In-Place S3 Restore**: Revert an existing managed database to any past S3 snapshot with one click (`POST /api/databases/s3/restore` with `target_db_id`).
- **Disaster Recovery Provisioning**: When rebuilding after catastrophic hardware loss or server relocation, provision a brand new, fully configured database directly from an S3 snapshot (`POST /api/databases/s3/restore` with `new_database_name` and `engine`). r3v3rs3 launches the container runtime, creates isolated persistent volumes, pulls and uncompresses the remote snapshot, and automatically runs the database engine's restore pipeline in the background.

## Deployments

`POST /api/apps/{id}/deploy` starts a deployment and returns it with the status `queued`. `GET /api/deployments/{id}` shows its progress.

1. r3v3rs3 pulls the image and records its digest. For a Git source it clones the branch, records the commit, builds the image as `r3v3rs3/<app id>:<deployment id>` and records the image id. For a Compose source it clones the branch, records the commit and runs `docker compose up --build`, then continues with step 3.
2. It creates the container `r3v3rs3-<app id>-<deployment id>` on the network `r3v3rs3-<app id>`, so the containers of different apps do not reach each other. The container publishes its port on a free port of `127.0.0.1`, so only r3v3rs3 on the host reaches it.
3. It waits up to 120 seconds for the health check. A container that stops or fails the check ends the deployment as `failed`, and the failure message holds the last lines of the container log. The old container keeps serving.
4. It marks the deployment `running`, marks the previous one `superseded`, and routes the domains to the new container.
5. After 10 seconds it stops and removes the old container, so that its open requests end.

An app runs one deployment at a time. A second deploy, a rollback or a deletion during a deployment gets `409 app_busy`. A [webhook](#webhooks) push during a deployment queues one more deployment instead. A restart of the server marks an unfinished deployment as `failed` with the message `the server stopped during the deployment`, and sends its `deployment_failed` notification.

| Status             | Meaning                                                                                         |
|--------------------|-------------------------------------------------------------------------------------------------|
| `pending_approval` | The deployment waits for manual approval or for the maintenance deploy window.                  |
| `queued`           | The deployment waits to start.                                                                  |
| `building`         | r3v3rs3 clones the branch and builds the image, or reads the Compose file.                      |
| `deploying`        | The image is pulled, `docker compose up` runs, or the new container waits for its health check. |
| `running`          | The container of the deployment serves the app.                                                 |
| `superseded`       | A newer deployment replaced this one.                                                           |
| `failed`           | The deployment stopped. `message` holds the reason.                                             |
| `cancelled`        | The deployment was rejected or cancelled. `message` holds the reason.                           |

### Rollback

`POST /api/deployments/{id}/rollback` starts a new deployment with the image digest, the app settings and the environment variables of an earlier deployment. The digest, not the tag, selects the image, so a rollback runs the same image even after the tag moved. A deployment that failed before it pulled its image has no digest and answers `400 rollback_unavailable`.

A rollback of a Git app starts the built image again and does not build. r3v3rs3 keeps the images of the 5 newest deployments of an app and of its running deployment, and removes older ones. A rollback to a deployment whose image is removed fails with the message `the image ... is no longer present`.

A rollback of a Compose app clones the recorded commit of the earlier deployment and runs `docker compose up --build` again, with the app settings and the environment variables of that deployment. A Compose deployment that failed before it recorded its commit answers `400 rollback_unavailable`.

### Deployment Approval and Deploy Windows

An app can require manual approval or restrict deployments to a scheduled maintenance window via its spec:
- `require_approval` (`bool`): when `true`, every new deployment (manual, webhook, auto-deploy, or rollback) starts with status `pending_approval` and waits until an account with `Edit` permission approves it.
- `deploy_window` (`Option<String>`): an allowed UTC time window in `HH:MM-HH:MM` format (e.g. `02:00-05:00`). When set, any deployment triggered outside the window pauses with status `pending_approval`. If `require_approval` is `false`, the platform's background scheduler automatically approves and starts the deployment once the UTC time enters the window.

`POST /api/deployments/{id}/approve` approves a `pending_approval` deployment and starts its pipeline.
`POST /api/deployments/{id}/reject` rejects the deployment, setting its status to `cancelled`.

## Webhooks

A webhook deploys an app when its Git provider sends a push event. `POST /api/apps/{id}/webhook_secret` creates the secret of the app and returns it once. The secret is 64 hex characters, and r3v3rs3 encrypts it with `r3v3rs3.key`. A second call creates a new secret, and the old one stops working at once. `DELETE /api/apps/{id}/webhook_secret` turns the webhook off.

The provider sends its events to `POST /hooks/apps/{id}` on the address of the admin API. This route needs no session, because the signature of each request proves it. The admin API listens on `127.0.0.1` by default, so the provider needs a way in: add a proxy that sends the path prefix `/hooks/` of a public domain to the admin port, and keep every other path of the admin API private.

| Provider       | Setting                                                                                                                                                                                                                                                                                       |
|----------------|-----------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------|
| GitHub / GHCR  | **Payload URL** is the hook address, **Content type** is `application/json`, and **Secret** is the secret. GitHub signs the body in the `X-Hub-Signature-256` header. Supports Git push, Pull Request events, and GitHub Container Registry (`package` / `registry_package`) push events.     |
| Gitea, Forgejo | **Target URL** is the hook address, **POST Content Type** is `application/json`, and **Secret** is the secret. The provider signs the body in the `X-Gitea-Signature` or `X-Forgejo-Signature` header.                                                                                        |
| GitLab         | **URL** is the hook address, and **Secret token** is the secret. GitLab sends the secret itself in the `X-Gitlab-Token` header. Enable **Push events**, and **Tag push events** when the branch field of the app names a tag. Supports Merge Request events.                                  |
| Docker Hub     | Webhook URL is `<hook address>?secret=<secret>`. Docker Hub sends a JSON payload with `push_data` and `repository`.                                                                                                                                                                           |
| Harbor         | Webhook URL is the hook address with an `Authorization: Bearer <secret>` auth header. Harbor sends `PUSH_ARTIFACT` events with resource tags and digests.                                                                                                                                     |
| Quay           | Webhook URL is `<hook address>?secret=<secret>` or send an `X-Webhook-Secret: <secret>` header. Quay sends notification events with `updated_tags`.                                                                                                                                           |
| Another sender | Sign the body with HMAC-SHA256 and the secret as `X-Signature-256: sha256=<hex>`, or pass the secret token via `Authorization: Bearer <secret>`, `X-Webhook-Token: <secret>`, or `?secret=<secret>` query parameter. A CI job or custom registry can trigger blue-green deployments this way. |

Which request deploys:

- A Git or a Compose app deploys for a push whose `ref` is `refs/heads/<branch>` or `refs/tags/<branch>`, where `<branch>` is the branch field of the app. A push that deletes the branch does not deploy.
- When `tag_trigger` is configured (for example `v*` or `release-*`), pushing a tag that matches the pattern automatically deploys that tag.
- When ephemeral preview deployments are enabled (`preview`), Pull Request / Merge Request events automatically manage an isolated preview app (`<app>-pr-<number>`): `opened` or `reopened` provisions and deploys the preview app on a dedicated domain (`preview_deployed`), `synchronize` deploys new commits, and `closed` destroys the preview app and frees all resources (`preview_destroyed`).
- An image app automatically updates and deploys blue-green when a container registry (Docker Hub, GHCR, Harbor, Quay) or generic webhook delivers a push event for the app's repository. If the pushed tag matches the currently configured tag (such as `:latest`), the new digest is pulled and deployed with zero downtime. If `tag_trigger` is configured (for example `v*` or `release-*`) and the pushed tag matches the pattern, the app's image reference is updated to the new tag and deployed. A push for a mismatched repository or an unrelated tag is safely ignored.
- A request of another sender always deploys, with any body.
- A ping, a push to another branch, or another event answers `200` with `{"outcome": "ignored"}`.

A deploying request answers `200` with `{"outcome": "deployed", "deployment": {...}}` (or `{"outcome": "preview_deployed", "deployment": {...}}` for pull requests). A closed pull request answers `200` with `{"outcome": "preview_destroyed"}`. A push during a running deployment answers `202` with `{"outcome": "queued"}`. When the running deployment ends, one more deployment starts with the latest commit of the branch, so one queued deployment covers every push that arrived in between. r3v3rs3 keeps the queue in memory, so a restart drops it.

| Status                        | Reason                                                                                                                                                                                                 |
|-------------------------------|--------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------|
| `400 invalid_webhook_payload` | The body of a GitHub, Gitea or GitLab push is not JSON, for example with the content type `application/x-www-form-urlencoded`.                                                                         |
| `401 unauthorized`            | The signature is missing or wrong.                                                                                                                                                                     |
| `404 id_not_found`            | No app has this id, or the app has no webhook secret.                                                                                                                                                  |
| `429`                         | The client sent more than 10 requests in a burst, or more than one request per second after the burst. `POST /hooks/apps/{id}` and `GET /oauth/git/callback` share this limit for each client address. |
| `503 platform_disabled`       | The deployment platform is off.                                                                                                                                                                        |

The body can be up to 5 MiB. The audit log records every request that deployed or queued a deployment, with the address of the sender, and without an account.

### Ephemeral Preview Deployments (Pull Requests)

For Git and Compose apps, r3v3rs3 can automatically deploy temporary preview environments for every pull request or merge request.

- **Enable in WebUI**: In the app settings form, toggle **Ephemeral Pull Request Previews**.
- **Domain Template**: Configure an optional domain pattern using `{pr}` and `{app}` placeholders, such as `pr-{pr}.app.example.com`. When left blank, r3v3rs3 automatically prefixes the app's primary domain (`pr-{pr}.<primary_domain>`), or falls back to `pr-{pr}-{app}.localhost`.
- **Environment & Secrets**: Preview environments inherit all environment variables, sealed secrets, and Git access tokens from the parent app. Host ports are automatically unpinned so preview containers never conflict with the primary application or other preview environments.
- **Automated Lifecycle**: When a pull request is opened or reopened, r3v3rs3 provisions the preview app and routes traffic to it. New commits pushed to the pull request trigger updates (`synchronize`). When the pull request is closed or merged, r3v3rs3 automatically removes the preview app, stops its containers, and cleans up all associated proxy routes and volumes.

## Git Providers

A Git provider connection lets r3v3rs3 act for an account of GitHub, GitLab or Gitea. After an admin connects it once, the app form lists the repositories and the branches of that account, a deployment clones a private repository with the token of the connection, and r3v3rs3 installs the push webhook of the app at the repository. The connections belong to the server: an admin adds them, and every account with the Edit permission can use them in its apps. An app without a connection keeps working with its repository address and its own Git token.

A GitHub connection is a GitHub App that r3v3rs3 creates for you. A GitLab or Gitea connection is an OAuth application that you register at the provider.

### Create a GitHub App

On the **Git Providers** page, select GitHub as the provider and give the connection a name. Leave **Address** empty for github.com, or enter the address of a GitHub Enterprise Server. **Organization** takes the GitHub organization that owns the app. Without it the app belongs to your own account.

**Create GitHub App** posts a manifest to GitHub. GitHub opens the page that creates a GitHub App, with these settings filled in:

- The name is `r3v3rs3-<connection name>`, cut to 34 characters. Each app name on GitHub must be unique, so change it on GitHub when it is taken.
- The app is private, so only the account that owns it can install it.
- The permissions are read access to Contents and Metadata, and write access to the repository webhooks.
- The app has no webhook of its own. r3v3rs3 installs the webhook of each app on its repository.

When you create the app on GitHub, GitHub sends the browser to the callback address (`/oauth/git/callback`) with a code. r3v3rs3 exchanges the code for the client id, the client secret and the private key of the app, and stores the connection. The browser then opens the installation page of the app. The state works once and for 10 minutes. The callback needs no session, because the session cookie does not follow a redirect from another site.

On the installation page, select the account or the organization and the repositories. GitHub returns the browser to the **Git Providers** page, which reads the installation of the app from GitHub, and the connection shows **Connected**. **Account** shows the account of the installation. On a connection that is not installed, **Install on GitHub** opens the installation page. The link in the **Address** column opens the page of the app on GitHub, where you can change the selected repositories.

GitHub keeps the callback and return addresses with the WebUI address of the moment the app was created. When the WebUI address changes, change these addresses in the settings of the app on GitHub.

r3v3rs3 encrypts the client secret and the private key with `r3v3rs3.key`, and the admin API never returns them. For each GitHub request r3v3rs3 gets an installation token with a JWT signed by the private key. The token lives one hour, and r3v3rs3 renews it before it expires. When the app is uninstalled on GitHub, the connection shows **Connect again**, and **Connect again** opens the installation page. A GitHub App connection changes only its name.

### GitLab and Gitea

The callback address is the address of the WebUI followed by `/oauth/git/callback`, for example `https://r3v3rs3.example.com/oauth/git/callback`. The **Git Providers** page shows this address for the page that you use. The provider sends the browser of the admin back to it, so it must be the address that the admin opens, not the public address.

| Provider | Where                                                                                    | Settings                                                                                                                                                      |
|----------|------------------------------------------------------------------------------------------|---------------------------------------------------------------------------------------------------------------------------------------------------------------|
| GitLab   | **Preferences**, **Applications**, or the **Applications** of a group or of the instance | **Redirect URI** is the callback address. Select the scope `api`, and keep **Confidential** on.                                                               |
| Gitea    | **Settings**, **Applications**, **Manage OAuth2 Applications**                           | **Redirect URI** is the callback address. Keep **Confidential Client** on. r3v3rs3 asks for the scopes `read:repository`, `write:repository` and `read:user`. |

The provider shows a client id and a client secret. Add both on the **Git Providers** page. A GitLab connection uses `https://gitlab.com` unless you give the address of your own GitLab. A Gitea connection needs the address of its server. The address of a provider uses `https://`; `http://` is allowed only on a loopback address.

### Connect

**Connect** on the row of a GitLab or Gitea connection opens the authorization page of the provider. After the account allows the access, the provider sends the browser back to the callback address, and r3v3rs3 exchanges the code for the tokens of the account. The authorization uses PKCE, and its state works once and for 10 minutes. The page shows **Connected** and the name of the account.

r3v3rs3 encrypts the client secret and the tokens with `r3v3rs3.key`, and the admin API never returns them. A GitLab or Gitea token expires, so r3v3rs3 renews it with its refresh token before it uses it. When the provider refuses the renewal, the connection shows **Connect again**, and **Connect again** authorizes it anew. A change of the provider, the address or the client id of a connection also disconnects it.

### Apps with a Connection

The **Git provider connection** select of the app form offers the connections. With a connected connection the form lists repositories: the recently changed repositories of the account on GitLab and Gitea, and the repositories that the GitHub App is installed on. **Search the repositories** finds one by its name, and a chosen repository fills the **Repository** address and its default branch. **Branch** then lists the branches of the repository. The repository address must belong to the provider of the connection.

A deployment and a rollback clone the repository with the current token of the connection. GitHub takes the installation token as the password of the user `x-access-token`, GitLab takes the token as the password of the user `oauth2`, and Gitea takes it as the user name.

### Webhooks of a Connection

Set the **Public address** on the **Settings** page (`public_url` in `config.toml`), for example `https://deploy.example.com`. The Git provider sends the webhook requests to `<public address>/hooks/apps/{id}`, so the provider must reach that address. See [Webhooks](#webhooks) for the proxy of the `/hooks/` prefix.

When an app with a connection is saved, r3v3rs3 creates the webhook secret of the app if it has none, and installs a push webhook at the repository. When the repository of the app changes, when its connection is removed, or when the app is deleted, r3v3rs3 deletes the old webhook at the provider. A new webhook secret installs the webhook again, and **Turn off webhook** deletes it at the provider.

A failed installation does not stop the change of the app. The **Webhook** section of the app page shows the reason, and **Install the webhook again**, or `POST /api/apps/{id}/hook`, deletes the old webhook of the app and installs it again. Without a public address no webhook is installed, and the app shows that reason. `POST /api/apps/{id}/hook` answers `409 public_url_missing` without a public address, and `400 invalid_git_connection` for an app without a connection. A webhook that r3v3rs3 cannot delete at the provider stays there and gets `404` from r3v3rs3.

A connection that an app uses cannot be deleted and answers `409 git_connection_in_use`. Deleting a connection does not remove its access at the provider: delete the GitHub App in the settings of GitHub, or the OAuth application at GitLab or Gitea.

## Notifications

The notification webhook of the "Settings" page also gets the events of the platform: `deployment_started`, `deployment_running` and `deployment_failed` for each deployment, `agent_online` and `agent_offline` for each agent target, and `app_alert` when an app exceeds its configured CPU or memory alert threshold. See [Notifications](@/configuration.md#notifications) for the JSON body.

### Resource Alert Rules

An app can define alert rules in the **CPU Alert Threshold (%)** and **Memory Alert Threshold (MB)** fields of its form (or `alert_rules` in `AppSpec`). During periodic background metrics collection (every 30 seconds), r3v3rs3 compares the container stats with the configured thresholds. When usage reaches or exceeds a threshold, an `app_alert` notification is delivered to the webhook with the current value and threshold. Repeated alerts for the same app and metric are rate-limited with a 5-minute cooldown to avoid notification flood.

### Container Resource Limits & Vertical Scaling

Applications and databases can enforce fine-grained hardware resource quotas to prevent memory leaks or runaway workloads from starving host system resources:

- **Memory Limit (MB)** (`memory_mb`): Maximum memory allocation for the container. Empty sets no limit.
- **Swap Limit (MB)** (`swap_mb`): Additional swap memory allowance beyond the memory limit. Setting `0` disables swap completely for the container. Empty leaves swap behavior at the Docker engine default.
- **CPU Limit** (`cpus`): Fractional or whole CPU quota, for example `1.5` for 1.5 CPU cores.
- **Dynamic Vertical Scaling**: Updating resource limits in the WebUI or via `PUT /api/apps/{id}` instantly adjusts the running container's cgroups in-place on the Docker runtime without downtime or container restarts.
- **OOM Protection & Threshold Alerts**: When a container has a memory limit configured, r3v3rs3 monitors memory saturation during periodic 30-second sweeps and sends an `app_alert` webhook if usage reaches 90% (OOM warning). If a container is terminated by the kernel OOM killer, an instant `oom_killed` alert is delivered.

### Container Resource Throttling & OOM Diagnostics

To prevent containers from silently crashing due to Out Of Memory (OOM) kills or suffering from severe CPU quota starvation, r3v3rs3 extracts Linux kernel cgroup metrics via the Docker engine stats API:

- **CPU CFS Quota Throttling**: Computes the percentage of elapsed CPU periods throttled (`cpu_throttled_percent`), the throttled period counts, and the cumulative throttled time in milliseconds (`cpu_throttled_time_ms`).
- **Memory Saturation & Fail Counter**: Tracks actual container memory saturation against configured limits and records memory allocation failure attempts (`failcnt` / `oom_kills`).
- **Automated Health Status**: Categorizes container resource state into `healthy`, `throttled_warning` (>= 20% throttling), `throttled_critical` (>= 50% throttling), `oom_risk` (>= 90% memory saturation or allocation failures), and `oom_killed` (process exit code 137 or OOM killer flag).
- **WebUI Diagnostic Card**: Displayed in the application view under the metrics panel, showing live diagnostic badges, metric counters, and actionable remediation recommendations.
- **REST API Endpoint**: `GET /api/apps/{id}/diagnostics` returns the full `AppResourceDiagnostics` payload.
- **100% MCP & CLI Parity**: Queryable autonomously with the `get_app_resource_diagnostics` MCP tool or the CLI command:
  ```bash
  r3 app diagnostics <APP_ID>
  ```

### Deployment Lifecycle Hooks & Migration Runner

r3v3rs3 supports automated lifecycle hooks to execute database migrations, schema verifications, or cache warming routines during deployment transitions:

- **Pre-Deploy Hook** (`lifecycle_hooks.pre_deploy`): Runs inside an ephemeral container created from the newly pulled or built image before existing containers are stopped or traffic is rerouted.
  - Receives the same environment variables, database connections (`uses_database`), networks, and volume mounts as the application container.
  - If the hook command fails (non-zero exit code), the deployment immediately aborts, the error output is recorded in the deployment failure logs, and the existing running container continues serving traffic without interruption (zero-downtime rollback protection).
  - Example commands: `php artisan migrate --force`, `npx prisma migrate deploy`, `alembic upgrade head`.
- **Post-Deploy Hook** (`lifecycle_hooks.post_deploy`): Executes inside the running container after it passes its readiness and health checks and begins serving traffic.
  - Ideal for clearing or warming application caches, generating search indexes, or pinging external observability endpoints.
  - Example commands: `curl -s http://localhost/warmup`, `php artisan config:cache`.
- **100% MCP & CLI Parity**: Lifecycle hooks can be configured via WebUI ("Deployment Lifecycle Hooks" card in the app form), updated via `PUT /api/apps/{id}`, managed via MCP, or managed via the CLI:
  ```bash
  # View current lifecycle hooks
  r3 app hooks <APP_ID>

  # Configure pre-deploy migration and post-deploy warmup
  r3 app hooks <APP_ID> --pre-deploy "npx prisma migrate deploy" --post-deploy "curl -s http://localhost/warmup"

  # Clear lifecycle hooks
  r3 app hooks <APP_ID> --clear
  ```

## Outbound Webhooks & Event Notifications

The platform includes a centralized outbound webhook hub to stream real-time events to external monitoring, notification, and automation services (such as Slack, Discord, PagerDuty, n8n, Datadog, or custom web services). Unlike the single notification webhook in Settings, the outbound webhook manager supports multiple endpoints, granular event subscriptions, custom HTTP headers, sealed signing secrets, and instant test delivery.

### Event Categories & Types

Administrators can selectively subscribe each webhook target to specific events across four categories:

| Category                    | Event Code                | Description                                                            |
|-----------------------------|---------------------------|------------------------------------------------------------------------|
| **Deployments**             | `deploy.success`          | App deployment completed successfully and container is healthy.        |
|                             | `deploy.failure`          | App deployment failed during build, pull, or startup.                  |
|                             | `deploy.pending_approval` | Deployment triggered but paused waiting for manual approval.           |
|                             | `app.crashed`             | App container exited unexpectedly with a non-zero status code.         |
|                             | `app.cpu_throttled`       | Container CPU CFS quota throttling exceeded warning threshold.         |
| **Databases & Backups**     | `backup.success`          | Database backup completed and saved locally.                           |
|                             | `backup.failure`          | Database dump or backup operation failed.                              |
|                             | `backup.upload_failed`    | Uploading a database backup to remote storage (S3/R2/SFTP) failed.     |
|                             | `backup.restored`         | Database successfully restored from a local or remote backup snapshot. |
| **Security & Certificates** | `cert.expiring`           | TLS certificate is within 30 days of expiration.                       |
|                             | `cert.renewed`            | TLS certificate renewed successfully via ACME.                         |
|                             | `cert.renewal_failed`     | TLS certificate renewal attempt failed.                                |
|                             | `security.ip_banned`      | Client IP address banned due to rate limiting or security policies.    |
| **System & Nodes**          | `target.offline`          | Agent node stopped sending heartbeats and was marked offline.          |
|                             | `system.disk_pressure`    | Target node disk usage exceeded the alert threshold.                   |
|                             | `system.oom_kill`         | Container terminated by kernel OOM killer or memory failure.           |
| **System**                  | `test.ping`               | Test ping sent manually from the WebUI or API.                         |

### Payload Format & Headers

Webhooks deliver an HTTP `POST` request with `Content-Type: application/json` and a standard envelope:

```json
{
  "id": "del_01j...",
  "event": "deploy.success",
  "created_at": "2026-10-21T21:40:00Z",
  "timestamp": 1792618800,
  "summary": "Deployment running: my-app (prod)",
  "data": {
    "app_id": "app_01j...",
    "app_name": "my-app",
    "deployment_id": "dep_01j...",
    "environment": "production"
  }
}
```

The delivery includes the following HTTP headers:
- `Content-Type: application/json`
- `User-Agent: r3v3rs3-Webhook/2.0`
- `X-R3v3rs3-Event`: The event code (e.g. `deploy.success`).
- `X-R3v3rs3-Delivery`: A unique UUID delivery identifier.
- `X-R3v3rs3-Signature`: When a webhook signing secret is configured, an HMAC-SHA256 signature in the format `sha256=<hex_digest>`.

Receivers can verify authenticity by computing the HMAC-SHA256 of the raw request body with the configured secret and performing a constant-time comparison against the hex digest in `X-R3v3rs3-Signature`.

### Secret Sealing & Storage

Webhook secrets are encrypted at rest with `r3v3rs3.key` (AES-256-GCM) using the platform's envelope encryption. The plaintext secret is never returned by the API or stored in audit logs. Rotating the platform master key automatically re-encrypts all configured webhook secrets.

## Routing

The platform adds one proxy for each running app that has at least one domain. The proxies come from the `Platform` provider, so they are read-only like the proxies of [service discovery](@/discovery.md), and the proxy list shows the provider status. Each proxy:

- listens on the ports that the app selects in its **Ports** fields, or the ports of `proxy_ports` when it selects none, or every HTTP and HTTPS port when that setting is also empty,
- answers the domains of the app,
- sends the requests to the published port of the container on `127.0.0.1`,
- gets its certificate from the ACME entry of `acme`, with the account, the challenge and the DNS provider of the entry. The rules of [ACME Certificates](@/discovery.md#acme-certificates) apply.

An app without a domain gets no proxy. A running deployment whose container is missing or stopped appears as an issue in the provider status. r3v3rs3 reads the containers every 15 seconds, so a container that Docker restarts on a new port gets its route back.

## Deleting an App

`DELETE /api/apps/{id}` stops and removes the containers, the network and the built images of the app, removes its proxy, and deletes its environment variables and its deployments. The named volumes stay. For a Compose app it runs `docker compose down`, removes the images that the project built and deletes its checkout. The volumes of the Compose project stay.

## Agent Targets

An agent target runs apps on a remote server. That server runs `r3v3rs3 agent`, which connects to the agent port of this server (the master) and runs the requests of the master on its own Docker Engine. The agent opens no port: it dials the master, and every request to an app comes back through that connection. The remote server can sit behind NAT.

### Enable the Agent Port

Set `agent_port` in the `[platform]` section and restart the server. At the first start with `agent_port` the master creates the agent CA and its own agent server certificate under `certs/agent/` in the config directory. The agent port uses this CA, not a certificate of the certificate list. Keep a backup of `certs/agent/`, because a new agent CA makes every agent enroll again. Allow the port in the firewall for the addresses of the agents.

### Add a Target

**Add an agent target** on the **Targets** page, or `POST /api/targets` with `{"name": "edge-1"}`, adds a target and shows its enrollment token once. The token has the form `<secret>.<ca hash>`. The CA hash lets the agent recognize the master at its first connection, so the enrollment needs no CA file and cannot be intercepted. The token enrolls one agent: the agent sends a certificate signing request, the master signs it with the agent CA and deletes the token. From then on the agent connects with its own client certificate.

### Install the Agent

The remote server needs Docker Engine, and the `docker` binary with the Compose plugin for Compose apps. It needs no `git`, because the master clones the repository and sends the files to the agent.

The **Targets** page shows both commands with the address of the master and the token filled in. With `install.sh` on a Linux server with systemd:

```sh
curl -fsSL https://raw.githubusercontent.com/KilimcininKorOglu/r3v3rs3/main/install.sh | sudo bash -s -- --agent --master master.example.com:9443 --token <token>
```

The script installs the binary, creates the systemd service `r3v3rs3-agent` with the data directory `/var/lib/r3v3rs3-agent`, waits until the agent enrolls and then deletes the token. The token never enters the unit file. Run the script again with `--agent` to upgrade; an enrolled agent needs no token. With a new token the script enrolls the agent again, and when that enrollment fails it restores the earlier identity.

| Option        | Environment variable      | Default                                   | Description                                                                        |
|---------------|---------------------------|-------------------------------------------|------------------------------------------------------------------------------------|
| `--master`    | `R3V3RS3_AGENT_MASTER`    |                                           | The agent port of the master as `HOST:PORT`.                                       |
| `--token`     | `R3V3RS3_AGENT_TOKEN`     |                                           | The enrollment token. Only the first start needs it; an enrolled agent ignores it. |
| `--data-dir`  | `R3V3RS3_AGENT_DATA_DIR`  | `agent` in the data directory of the user | The key, the certificate and the Compose files of the agent.                       |
| `--docker`    | `R3V3RS3_AGENT_DOCKER`    | `unix:///var/run/docker.sock`             | The Docker Engine API of the agent host.                                           |
| `--log-level` | `R3V3RS3_AGENT_LOG_LEVEL` | `info`                                    | The log level.                                                                     |

The data directory holds `agent.key` (mode `0600`), `agent.pem`, `ca.pem`, `target` and the `compose/` directory. The agent stops gracefully on SIGINT.

### Apps on an Agent

An app with the id of an agent target in `target` runs there with every source:

- An image app pulls its image on the agent host.
- A Git app is cloned on the master. The master sends the build context to the agent, and the Docker Engine of the agent host builds the image.
- A Compose app is cloned on the master, which also writes the override file. The master sends the deployment directory to the agent, which unpacks it into `compose/<app id>/<deployment id>/` of its data directory and runs `docker compose` there.

The containers publish their port on `127.0.0.1` of the agent host. For each such port the master opens a forwarder on `127.0.0.1` of the master, and the route and the health check of the app use it. Every connection to a forwarder opens a tunnel through the agent connection to the port on the agent host, so HTTP/1.1, HTTP/2 and WebSocket work unchanged.

The agent checks every request again. It changes only containers with the label of the platform, networks and Compose projects whose name starts with `r3v3rs3-`, and images whose name starts with `r3v3rs3/`, and it validates every container spec. A Compose file is not restricted on an agent host either, so an account with the Edit permission can take over the agent host through a Compose app.

### Offline Agents

The master pings every agent every 10 seconds. An agent that closes its connection, or does not answer a ping within 20 seconds, is offline. For an app on an offline target:

- the route stays and answers `502`,
- a deployment fails with the message `the agent of the target is not connected`,
- the log answers `503 agent_offline`.

The agent connects again after a pause that grows from 1 to 60 seconds. When it is back, the routes of its apps work again within 15 seconds.

### Replace or Delete an Agent

**New token** on the **Targets** page, or `POST /api/targets/{id}/token`, gives a target a new enrollment token. The enrolled agent keeps working until another agent enrolls with that token. Then the master disconnects the old agent and refuses its certificate. Use it to move a target to a new server or to replace a lost agent key.

`DELETE /api/targets/{id}` deletes a target without apps and disconnects its agent. A target with an app answers `409 target_in_use`, and the local target answers `403 target_read_only`. An agent whose certificate belongs to no target logs `the master closed the connection before its first request` and keeps trying to connect.

### Agent Remote Self-Upgrade

When the master server is updated to a newer version, it can push the updated `r3v3rs3` executable binary to connected agent hosts:

- **Automatic Upgrade**: When an agent connects whose version is older than the master's version, the master automatically pushes the new binary over the multiplexed TLS link. The agent receives the binary, replaces its executable on disk, and cleanly restarts (via systemd).
- **Manual Upgrade**: On the **Targets** page, an admin can click **Upgrade** on an individual connected agent or **Upgrade All Agents** to push the latest binary to all online targets.
- **Admin API**: `POST /api/targets/{id}/upgrade` pushes the upgrade to an individual agent, and `POST /api/targets/upgrade` upgrades all outdated online agents.

### Link Quality, Latency Matrix & Dynamic Load Balancer De-weighting

Between the master and each connected agent, r3v3rs3 continually monitors the health of the multiplexed mTLS tunnel:

- **Periodic RTT & Latency Tracking**: Every heartbeat ping (default interval 10 seconds) measures the precise round-trip time (RTT). The last 15 latency samples are maintained in a sliding window to calculate jitter (mean absolute deviation) and packet loss percentage.
- **Link Quality Classification**:
  - `Excellent` (Green): RTT <= 50ms, jitter <= 15ms, loss <= 2%. Full 100% load balancing capacity.
  - `Good` (Blue): RTT <= 150ms, jitter <= 40ms, loss <= 10%. Full 100% capacity.
  - `Degraded` (Amber): RTT <= 350ms, jitter <= 80ms, loss <= 25%. Traffic share automatically de-weighted by 50% (`weight = 5`).
  - `Poor` (Red): RTT > 350ms, jitter > 80ms, or loss > 25%. Traffic share de-weighted to critical 10% (`weight = 1`).
  - `Offline` (Gray): The agent is disconnected or failed to answer pings within the timeout window. Weight drops to `0`.
- **Dynamic Load Balancer De-weighting**: Upstream servers running on degraded or poor agent nodes receive proportionally less incoming traffic through r3v3rs3's smooth weighted round robin and least connections balancers, protecting clients from sluggish or dropping nodes.
- **WebUI Sparkline & Metrics**: The **Targets** page renders a live link quality badge, current RTT latency, jitter variance, packet loss, dynamic capacity indicator, and a mini SVG latency sparkline for each connected agent node.

### Multi-Target Replica Deployment

An app can deploy replicas across multiple agent targets and the local target simultaneously:

- In `AppSpec`, set `targets` to a list of target IDs (e.g. `["local", "edge-1", "edge-2"]`).
- Optionally specify `load_balancing` (`round_robin`, `least_conn`, `random`, `first`, or `client_ip_hash`) and `sticky` cookie session settings.
- When a deployment runs, the master prepares the image, runs the container, and waits for health checks across all specified targets.
- Remote agent ports forward over loopback to the master, and the master's reverse proxy automatically balances traffic across all active replicas using the chosen load balancing algorithm.
- If an individual agent becomes temporarily offline, the master continues routing traffic among the remaining healthy replicas.
- **Active Health Checks & Outlier Detection**: When an app configures `health_check_path`, r3v3rs3 proactively polls each replica over HTTP every 10 seconds. Multi-replica apps without an explicit HTTP health check path automatically perform active TCP connection probes every 10s. A circuit breaker with outlier detection (50% failure ratio over a 10s window) automatically isolates failing replicas for 30 seconds, routing client requests strictly to healthy nodes.
- **Target Maintenance & Node Drain**: An admin can put any node into maintenance mode ("Drain") from the **Targets** page or via `POST /api/targets/{id}/drain`. When drained, r3v3rs3 excludes the node from the active upstream proxy pools without dropping existing connections. Clicking **Undrain** or calling `POST /api/targets/{id}/undrain` immediately restores the target to active load balancing.

## Container Registries

When pulling private container images (such as from private Docker Hub repositories, GitHub Packages `ghcr.io`, GitLab Container Registry, AWS ECR, or a self-hosted private registry), configure credentials in the **Registries** page of the WebUI or via the admin API.

```json
{
  "server": "ghcr.io",
  "username": "octocat",
  "password": "ghp_xxxxxxxxxxxxxxxxxxxx"
}
```

- Registry passwords and personal access tokens are sealed with `r3v3rs3.key` and unsealed only when authenticating image pulls via standard Docker `X-Registry-Auth`.
- Key rotation (`POST /api/platform/key/rotate`) automatically unseals and re-encrypts all stored registry secrets in a single transaction.
- When deploying an image app, r3v3rs3 resolves the registry server (e.g. `ghcr.io` or `docker.io` / `registry-1.docker.io`), attaches authentication credentials, and sends the request to the local engine or remote agents.

## WebUI

The **Dashboard** (`/`) provides a single-screen overview of the system state, ports, proxies, apps, databases, targets, disk storage space, and available updates. The **Platform** group of the sidebar holds the **Projects**, **Apps**, **App Store**, **Databases**, **Backups**, **Targets**, **Git Providers**, **Registries** and **Environment Variable Groups** pages, and appears only while the platform is on. The **Git Providers** and **Registries** pages appear for an account with the Edit/Admin permission. The group appears only for an account without a proxy list. An account with the Read permission sees the apps, their deployments and their logs. The Edit permission adds, changes, deploys and deletes apps.

### Dashboard

The **Dashboard** summarizes system operations on a single unified screen:
- **Stat Cards**: Live counts and active ratios for ports, proxies, apps (with update badges), databases, and agent targets, linking directly to each management section.
- **Disk Space**: Real-time storage consumption gauge showing used and available disk capacity in gigabytes and percentage.
- **Recent Deployments**: The latest deployment jobs across all apps, displaying execution status, trigger source, and start time.
- **Quick Links**: Direct shortcuts to create proxies, ports, and apps, or explore the catalog in the App Store.
- **Admin API**: `GET /api/dashboard` returns the `DashboardSummary` object.

### Projects

The **Projects** page organizes applications and databases under structured projects and deployment environments:

- Create projects (e.g. *E-Commerce*) and optionally customize their initial environment names (defaults to `production`).
- Manage multiple isolated environments within each project (e.g. `production`, `staging`, `preview`).
- Configure environment-level shared variables and encrypted secrets that all apps in the environment automatically inherit.
- Direct quick links to filter and view apps and databases belonging to any specific project or environment.
- **Cross-Environment Compare & Sync**: Click **Environments** on any project row to open the multi-environment manager with tabs for each tier and a **Compare & Sync** tab featuring interactive side-by-side variable diffing, one-click synchronization, and individual key sync buttons.

### Apps

The **Apps** page lists every app with its **Source**, its **Domains** and the status of its **Last Deployment**. Clicking the app name opens its dedicated [App Dashboard](#dedicated-app-dashboard). The row actions of an app are:

- **View** opens the dedicated app dashboard console.
- **Edit** opens the application settings form (`/apps/{id}/settings`).
- **Terminal** opens the web terminal.
- **Files** opens the file browser.
- **Deployments** opens the deployment history.
- **Log** opens the container log.
- **Deploy** starts a deployment after a confirmation.

The page reads the apps again every 2 seconds while a deployment is unfinished, and every 10 seconds otherwise.

### Dedicated App Dashboard

Clicking an application in the list opens its dedicated dashboard console at `/apps/{id}`. The dashboard consolidates:

- **Top Navigation Tabs**: Quick switching between **Overview**, **Deployments**, **Logs**, **Terminal**, **Files**, and **Settings**.
- **Live Status & Telemetry**: Container uptime, CPU usage percentage, memory usage against configured limits, and network throughput (RX/TX).
- **Resource Diagnostics Alerts**: Instant warnings for CPU CFS quota throttling and memory OOMKill events.
- **Active Deployment Card**: Current commit hash, branch name, commit message, author, deployment duration, and target node.
- **Quick Actions Toolbar**: One-click **Deploy**, **Restart** container, **Rollback** to previous deployment, **Terminal**, and **Logs**.
- **Services & Containers**: Container state, ID, and healthcheck status for Compose services.
- **Recent Logs Tail**: Direct terminal preview of the latest 15 container log lines.
- **API, MCP & CLI Management**: Complete parity via `GET /api/apps/{id}/dashboard`, MCP tool `get_app_dashboard`, and CLI command `r3 app dashboard <APP_ID>`.

### App Store

The **App Store** page lists the catalog of the apps that install in one click, see [App Store](#app-store). **Install** opens the app form with the fields of the entry filled in; a Compose entry also lists its **Variables**, and creating the app saves them and starts the first deployment. **Refresh catalog** downloads the current catalog from the repository at once.

### Add and Change an App

**Add** opens an empty app form. **Source** selects **Image**, **Git** or **Compose**, and the form shows the fields of that source. A Git or a Compose app can pick its repository through a **Git provider connection**, see [Apps with a Connection](#apps-with-a-connection). A Compose app hides **Volumes**, **Restart Policy**, **Memory Limit (MB)** and **CPU Limit**, because its Compose file sets them.

- **Domains** takes one domain on each line. r3v3rs3 inspects the configured domains in real time and automatically applies **Wildcard SSL Auto-Sharing**: if an active wildcard certificate (such as `*.example.com`) exists on the server, subdomains (e.g. `api.example.com`, `shop.example.com`) are instantly covered with zero manual TLS configuration or duplicate certificate orders. When custom external domains are added, the UI displays CNAME setup instructions pointing to the public host along with a one-click **Verify CNAME** button that verifies public DNS records.
- **Volumes** takes one mount on each line: `volume:/path`, or `volume:/path:ro` for a read-only mount.
- **Memory Limit (MB)** takes a whole number of megabytes, and **CPU Limit** a number of CPUs such as `1.5`. An empty field sets no limit.

**Create** saves the app and opens its page. The page of an existing app adds these parts below the form:

- **Git Token**, for a Git or a Compose app. **Set Token** saves a token, and **Remove token** deletes it after a confirmation. The page shows only whether a token is set.
- **Webhook**. An app with a connection shows here whether its webhook is installed at the provider, or why it is not, with **Install the webhook again**. **Create Secret** creates the [webhook](#webhooks) secret and shows it once with the **Payload URL**, which is the hook address on the address of the page. Copy both before you leave the page. **Create New Secret** replaces the secret after a confirmation, and **Turn off webhook** deletes it after a confirmation.
- **Environment Variables**. **Add variable** adds a row with a **Key**, a **Value** and a **Secret** checkbox. The value of a saved secret variable shows as **Unchanged**. Leave it empty to keep the value. **Save Variables** replaces all variables, and the next deployment uses them.
- **Delete app** deletes the app with its containers after a confirmation.

### Metrics

The page of an app carries a **Metrics** card. While an app runs, the server records a sample every 30 seconds: the CPU of the container, its memory, the bytes of its network interfaces, and the requests that the proxy of the app served, each as the change against the previous sample. The card shows the latest values, two small charts of the CPU and the requests of the last hours, and the time of the last sample. It reads the samples again every 10 seconds. The history stays in the platform database for a day, and an app keeps no samples while it does not run. An app on an agent target carries no container stats, so only its requests are recorded.

### Deployments

The deployment history shows the latest 100 deployments with their status, **Trigger** (**Manual**, **Webhook** or **Rollback**), **Commit**, **Account**, **Started**, **Duration** and **Message**. **Roll back** starts an earlier deployment again after a confirmation. The link appears only for a finished deployment that no longer runs and has a recorded image digest, or a recorded commit for a Compose app. It hides while a deployment of the app is unfinished. The page follows the same refresh intervals as the **Apps** page.

### Log

The log page shows the last 200 lines of stdout and stderr of the running container and streams new lines in real time over a WebSocket connection. The toolbar offers a live tail toggle (streaming or paused), an auto-scroll switch, real-time search filtering with match counts, and a **Download** button to export the log buffer as a `.txt` file. For Docker Compose applications, the toolbar includes a **Service** selector dropdown to switch between the primary traffic container and any specific service container in the project. An app without a running container shows "The app has no running container."

### Terminal

An admin account sees a **Terminal** link on the page of an app, as well as a dedicated **CLI** button next to each running database on the Databases page. It opens an interactive terminal in the running container over a WebSocket connection, and a resize of the window resizes the terminal of the container. The terminal interface offers multi-tab support, persistent command history, and quick maintenance snippet controls:

- **Multi-Tab Sessions**: Open multiple concurrent terminal sessions (tabs) inside the running container. Each tab maintains its own independent WebSocket connection and process execution. Switch seamlessly between tabs without dropping active commands, background jobs, or scrollback history. Open new tabs with one click using the quick shell chips (`+ sh`, `+ bash`, `+ zsh`, or `+ psql` / `+ mysql` / `+ redis-cli` / `+ mongosh` for databases).
- **Session Command History & Navigation**: Executed commands are stored in session memory. Navigate previously run commands directly in the command box using the Up (`↑`) and Down (`↓`) arrow keys. Click the **History** button to open a searchable command history modal with real-time filtering, allowing you to rerun, insert, or copy past commands, or clear session history.
- **Quick Maintenance Snippets**: Toggle the **Snippets** panel to access ready-to-run maintenance commands categorized by **System** (`ps aux`, `top -b -n 1`, `df -h`, `free -m`, `uptime`, `uname -a`), **Files** (`ls -lah`, `pwd`, `du -sh * | sort -h`, `find . -maxdepth 2 -type f`), **Network** (`ip addr`, `netstat -tuln`, `curl -I localhost`, `cat /etc/hosts`, `cat /etc/resolv.conf`), **Environment** (`env | sort`, `whoami && id`), and **Database** (PostgreSQL, MySQL, MariaDB, Redis, and MongoDB maintenance commands). Click any snippet to execute it immediately in the active terminal tab or insert it into the custom input for parameter customization.
- **Shell Selection & Fallback**: Switch between `/bin/sh`, `/bin/bash`, `/bin/zsh`, or enter custom commands. If an advanced shell like `bash` or `zsh` is not installed in the target container (such as minimal Alpine-based images), the system automatically prints an informative notice and falls back to `/bin/sh` without interrupting the session.
- **Database CLI Shortcuts**: When opening the terminal of a managed database (PostgreSQL, MySQL, MariaDB, Redis, MongoDB), the page automatically starts with the appropriate CLI tool (`psql`, `mysql`, `mariadb`, `redis-cli`, `mongosh`). Authentication credentials and environment variables are automatically applied. Operators can also open additional shell tabs side-by-side to inspect files while keeping the database console active.
- **Session Actions**: The **Copy Output** button extracts the active terminal buffer directly to the clipboard, and **Clear** clears the current screen output.

The terminal works while the app runs on the local Docker Engine of the r3v3rs3 server; an app on an agent target has no terminal yet. Every terminal that an account opens goes into the audit log as `exec_app`, including the chosen shell or CLI tool.

### Files

An admin account also sees a **Files** link on the page of an app. It opens the files of the volumes of the running container. The page selects a volume, lists its directories, and opens a directory on **View**. A file row offers **Download**, **Edit** and **Delete**; **Edit** opens a file up to 1 MB as text, and **Save** writes it back. The toolbar of the page uploads a file into the current directory and steps one directory up. A write stays inside a volume that is not read-only, and the page hides the upload, the edit and the delete of a read-only volume. Like the terminal, the files work only while the app runs on the local Docker Engine of the r3v3rs3 server. Every write and delete goes into the audit log as `app_file`.

#### Volume Backups, Snapshots and Direct Archive Export/Import

The Files page provides complete volume backup and snapshot management for persistent volumes:

- **Create Snapshot**: Captures a full compressed `.tar.gz` snapshot of the volume from the running container, saves it locally, and automatically replicates it to remote S3 storage if configured in the backup settings.
- **Restore Snapshot**: Restores a previously captured volume snapshot with a single click, unpacking the snapshot archive directly back into the container's volume.
- **Download Snapshot**: Downloads any stored volume snapshot directly as a compressed `.tar.gz` archive.
- **Export Volume (.tar.gz)**: Streams and downloads the live contents of the volume directly as a `.tar.gz` archive without requiring an explicit saved snapshot.
- **Import Volume (.tar.gz)**: Unpacks and extracts an uploaded `.tar.gz` or `.tar` archive directly into the active volume.

### Scheduled Commands

The page of an app includes a **Scheduled Commands** section to run scheduled or periodic one-off commands inside the container of the running deployment (e.g. database maintenance, cache clear, warmup tasks).

Each command configuration specifies:
- **Name**: A descriptive name for the task.
- **Schedule**: A timing expression. Supports daily time `HH:MM` (UTC), standard 5-field cron syntax (e.g. `*/15 * * * *`, `0 2 * * *`), shorthand macros (`@hourly`, `@daily`), or interval expressions (`@every 15m`, `@every 1h`, `@every 30s`).
- **Command**: The command to execute inside the container, passed to `sh -c`.
- **Enabled**: Whether the background scheduler runs the task when due.

The server checks and triggers due commands in a background loop every 30 seconds. A command can also be run immediately on demand with **Run now**. The last execution status, exit code, and run time are recorded on each row, and **Output** opens a modal displaying stdout and stderr of the latest run.

### Targets

The **Targets** page lists the local target and the agent targets with their **Kind**, their **Status** (**Online**, **Offline** or **Waiting for enrollment**), **Last seen** and the **Version** of the agent. It reads the list again every 10 seconds. The **Target** select of the app form shows the status of each target next to its name.

An admin account also sees:

- **Add an agent target** below the list. **Add** creates the target and shows its **Token** once at the top of the page, with the `install.sh` command and the `docker run` command of the agent. Copy them before you leave the page.
- **New token** on the row of an agent target, which creates a new enrollment token after a confirmation.
- **Delete** on the row of an agent target, which deletes it after a confirmation.
- **Upgrade** on the row of an online agent target, which pushes the current server binary to the agent and restarts it.
- **Upgrade All Agents** above the list when online agents exist, which pushes the update to all connected agents.

Without `agent_port` the add and the new token actions answer `400 agent_port_missing`.

### Disk Hygiene & Prune Policy

The **Targets** page also includes a **Disk Hygiene & Prune Policy** card to automate cleanup of unused Docker resources and prevent disk exhaustion:

- **Automatic Scheduled Pruning**: When enabled, a background worker evaluates and cleans resources on the configured interval (e.g. `24h` or `7days`).
- **Disk-Pressure Reactive Pruning**: In addition to scheduled intervals, an emergency prune triggers automatically whenever node disk usage reaches or exceeds the configured percentage threshold (default: 85%). A 15-minute cooldown prevents rapid thrashing loops, and each triggered emergency cleanup dispatches an outbound `system.disk_pressure` webhook notification and records an audit log entry with reclaimed space details. Setting the threshold to empty disables reactive pruning.
- **Resource Selection**: Configurable toggles for dangling and unused images, stopped containers, BuildKit build cache, and unused anonymous volumes.
- **Build Cache Retention Limit**: In the prune policy settings, **Build cache retention limit** (`build_cache_keep_storage_mb`) specifies the maximum amount of BuildKit cache in megabytes to retain on disk across builds. When specified, BuildKit's `keep-storage` limit is supplied to Docker Engine during prune runs, keeping hot cache layers for subsequent fast compilations while discarding older intermediate layers. Leaving the field empty prunes all unreferenced build cache.
- **Application Rollback Image Retention**: Configured via **Retained image builds per application** (`kept_builds_count`, default: 5). During image pruning and post-deployment cleanup, r3v3rs3 inspects existing image tags across all platform applications and keeps the specified number of newest builds for each app, preventing accidental removal of historical release tags required for zero-downtime rollbacks.
- **Dangling Layers Only Filter**: Enabling **Prune dangling image layers only** (`dangling_layers_only`) instructs Docker Engine to remove only untagged, dangling intermediate layer artifacts (`<none>:<none>`), preserving all tagged repository images.
- **Container Log Rotation & Auto-Vacuum**: All newly deployed Docker containers are automatically configured with `json-file` log limits (`max-size: 50m`, `max-file: 3`) to prevent unbounded log growth. In the prune policy settings, **Truncate Container Logs Exceeding Threshold** (`prune_logs`) can be enabled alongside an optional threshold in megabytes (default: 50 MB). When scheduled or reactive prune runs, active JSON container logs exceeding the threshold are truncated in place without restarting containers, and stale rotated log files (`.1`, `.2`) are deleted.
- **On-Demand Log Vacuum**: The **Logs** page of any application includes a **Vacuum Logs** button (`POST /api/apps/{id}/logs/vacuum`) to immediately reclaim disk space by truncating that specific application's container logs. An administrator can also run `POST /api/platform/logs/vacuum` to vacuum all container logs system-wide.
- **Run Immediately**: Click **Prune Now** (or send `POST /api/platform/prune/run`) to trigger an immediate prune and view the freed disk space and deleted item counts in real time.
- **Run Statistics**: Displays the timestamp of the last scheduled execution, the last emergency disk pressure execution, and the total reclaimed disk space.
- **CLI & MCP Management**: Disk hygiene and prune policies are fully manageable via both CLI and MCP. Use `r3 platform prune-policy` to view the active policy, `r3 platform set-prune-policy --enable true --keep-cache-mb 5120 --kept-builds 5 --dangling-layers-only true` to update retention and prune parameters, and `r3 platform prune` to execute pruning immediately. In MCP, use `get_prune_policy`, `update_prune_policy`, and `run_platform_prune`.

### Node Threshold Alerts

The **Targets** page includes a **Node Threshold Alert Rules** configuration card to continuously monitor host node resources and trigger outbound webhook notifications:

- **Low Free Disk Space Warning (`system.disk_low`)**: Fires an alert whenever available node disk space drops to or below the configured percentage threshold (default: 10%). The notification payload contains total, available, and used disk bytes, current free percentage, and the threshold.
- **Bandwidth Spike Anomaly Detection (`system.bandwidth_spike`)**: Detects sudden traffic surges by evaluating throughput rates in bytes per second against the configured threshold (configured in MB/s in the WebUI). Measured via host interface counters in `/proc/net/dev` with fallback to container network rates.
- **File Descriptor Exhaustion Warning (`system.fd_exhaustion`)**: Evaluates active open file descriptors from `/proc/self/fd` against the process maximum limit (`rlimit` NOFILE). Triggers an emergency webhook notification when the usage percentage reaches or exceeds the threshold (default: 80%).
- **Socket / TIME_WAIT Exhaustion Warning (`system.socket_exhaustion`)**: Parses socket allocations from `/proc/net/sockstat` and alerts when the count of TCP sockets in `TIME_WAIT` state exceeds the configured maximum threshold (e.g. 10,000 sockets), preventing local ephemeral port exhaustion under heavy concurrent traffic.
- **Anti-Spam Cooldown**: Built-in 15-minute cooldown prevents alert flooding during persistent high traffic or prolonged low disk situations.
- **Outbound Webhooks & Auditing**: Subscribed outbound webhooks receive JSON alerts under the `System` category, and rules configuration changes are tracked in the audit trail.
- **CLI & MCP Parity**: Fully manageable via `r3 platform node-alerts` and `r3 platform set-node-alerts` CLI commands, as well as `get_node_alerts` and `update_node_alerts` MCP tools.

### Webhooks

The **Webhooks** page under Administration provides a centralized dashboard for managing outbound event notification targets:

- **Webhook List**: Displays all configured webhooks with their target URL, assigned name, active/inactive badge, sealed secret status badge, and subscribed event tags.
- **Add / Edit Webhook Modal**:
  - **Name & URL**: Target webhook endpoint (e.g. `https://hooks.slack.com/...`).
  - **Custom Headers**: Optional JSON object of custom headers (e.g. `{"Authorization": "Bearer ...", "X-Custom-Token": "..."}`).
  - **Signing Secret**: Optional shared secret for HMAC-SHA256 payload signing.
  - **Active Toggle**: Enable or disable deliveries to the target without deleting it.
  - **Categorized Event Selection**: Checkboxes grouped by category (Deployments, Databases & Backups, Security & Certificates, System & Nodes) with one-click **Select All** and **Deselect All** buttons.
- **Test Webhook**: Send an immediate `test.ping` payload to the target endpoint to verify connectivity and signature validation before saving.

### Backups

The **Backups** page (`/backups`) provides a centralized management console for database and volume backups, automated schedules, disaster recovery, and health monitoring across the entire platform:

- **KPI Cards**: Real-time overview metrics displaying total backup archives, cumulative disk space consumed, timestamp of the most recent backup, configured cron schedule, Dead Man's Snitch / Heartbeat monitor status, and remote S3 backup sync status.
- **Unified Backups Table**: Displays all backups across all database engines (PostgreSQL, MySQL, MariaDB, MongoDB, Redis) and volumes with engine tags, database name, backup identifier, size, creation date, and automated restore verification status badge.
- **Search & Engine Filters**: Instant search filter by database name or backup ID, plus engine dropdown filtering.
- **Row Actions**:
  - **Download**: Direct `.dump.gz` download link.
  - **Verify**: On-demand integrity test decompressing the dump and validating magic bytes/SQL headers without modifying the database.
  - **Restore**: In-place container restore replacing active database data with the selected dump after confirmation.
  - **Delete**: Deletes the local backup file and metadata.
- **Take Backup Now Modal**: Quick one-click execution modal to trigger an immediate backup for any selected database on the system.
- **Disaster Recovery Drawer**: Inspect and restore snapshots stored in remote S3-compatible cloud storage.
- **Settings Modal**: Configure automated UTC daily backup schedules, retention counts, remote storage targets (S3, SFTP, rsync), and Dead Man's Snitch / Heartbeat ping URL.

## Admin API

| Route                                                          | Permission    | Description                                                                                                                                                                                                                               |
|----------------------------------------------------------------|---------------|-------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------|
| `GET /api/backups`                                             | Read          | Lists all database backups across all databases with engine and metadata.                                                                                                                                                                 |
| `GET /api/backups/overview`                                    | Read          | Aggregated statistics, schedule, retention, and heartbeat monitoring status across all databases.                                                                                                                                         |
| `POST /api/backups/run`                                        | Edit          | Triggers an immediate backup of a specified database.                                                                                                                                                                                     |
| `GET /api/store`                                               | Read          | The app store catalog with its state.                                                                                                                                                                                                     |
| `POST /api/store/refresh`                                      | Edit          | Downloads the catalog from the repository and applies it.                                                                                                                                                                                 |
| `GET /api/targets`                                             | Read          | The Docker hosts that run apps, with the state of their agents.                                                                                                                                                                           |
| `POST /api/targets`                                            | Admin         | Adds an agent target and returns its enrollment token once.                                                                                                                                                                               |
| `POST /api/targets/upgrade`                                    | Admin         | Pushes the latest binary to all connected outdated agent targets.                                                                                                                                                                         |
| `POST /api/targets/{id}/token`                                 | Admin         | Returns a new enrollment token of an agent target.                                                                                                                                                                                        |
| `POST /api/targets/{id}/upgrade`                               | Admin         | Pushes the latest server binary to an agent target to upgrade it.                                                                                                                                                                         |
| `POST /api/targets/{id}/drain`                                 | Admin         | Puts a target into maintenance mode, removing its containers from active routing.                                                                                                                                                         |
| `POST /api/targets/{id}/undrain`                               | Admin         | Restores a drained target to active proxy routing.                                                                                                                                                                                        |
| `DELETE /api/targets/{id}`                                     | Admin         | Deletes an agent target without apps and disconnects its agent.                                                                                                                                                                           |
| `GET /api/registries`                                          | Read          | The configured container registries for image pulls.                                                                                                                                                                                      |
| `POST /api/registries`                                         | Admin         | Adds container registry credentials.                                                                                                                                                                                                      |
| `PUT /api/registries/{id}`                                     | Admin         | Updates container registry credentials.                                                                                                                                                                                                   |
| `DELETE /api/registries/{id}`                                  | Admin         | Deletes container registry credentials.                                                                                                                                                                                                   |
| `GET /api/projects`                                            | Read          | Lists all projects with their environments.                                                                                                                                                                                               |
| `POST /api/projects`                                           | Edit          | Creates a new project with optional default environments.                                                                                                                                                                                 |
| `GET /api/projects/{id}`                                       | Read          | Returns project details with environments and app count.                                                                                                                                                                                  |
| `PUT /api/projects/{id}`                                       | Edit          | Replaces project name and description.                                                                                                                                                                                                    |
| `DELETE /api/projects/{id}`                                    | Edit          | Deletes a project.                                                                                                                                                                                                                        |
| `GET /api/projects/{id}/environments`                          | Read          | Lists environments of a project.                                                                                                                                                                                                          |
| `POST /api/projects/{id}/environments`                         | Edit          | Adds an environment to a project.                                                                                                                                                                                                         |
| `GET /api/projects/{id}/environments/{env_id}`                 | Read          | Returns environment details.                                                                                                                                                                                                              |
| `PUT /api/projects/{id}/environments/{env_id}`                 | Edit          | Updates environment name and description.                                                                                                                                                                                                 |
| `DELETE /api/projects/{id}/environments/{env_id}`              | Edit          | Deletes an environment.                                                                                                                                                                                                                   |
| `GET /api/projects/{id}/environments/{env_id}/env`             | Edit          | Returns shared environment variables without secret values.                                                                                                                                                                               |
| `PUT /api/projects/{id}/environments/{env_id}/env`             | Edit          | Replaces shared environment variables for an environment.                                                                                                                                                                                 |
| `GET /api/apps`                                                | Read          | The apps.                                                                                                                                                                                                                                 |
| `POST /api/apps`                                               | Edit          | Adds an app.                                                                                                                                                                                                                              |
| `GET /api/apps/{id}`                                           | Read          | Returns an app.                                                                                                                                                                                                                           |
| `PUT /api/apps/{id}`                                           | Edit          | Replaces an app.                                                                                                                                                                                                                          |
| `DELETE /api/apps/{id}`                                        | Edit          | Deletes an app with its containers.                                                                                                                                                                                                       |
| `GET /api/apps/{id}/env`                                       | Edit          | The environment variables, without secret values.                                                                                                                                                                                         |
| `PUT /api/apps/{id}/env`                                       | Edit          | Replaces the environment variables.                                                                                                                                                                                                       |
| `POST /api/apps/{id}/env/generate`                             | Edit          | Generates cryptographically secure secrets for missing or placeholder environment variables.                                                                                                                                              |
| `POST /api/compose/env/generate`                               | Edit          | Resolves an environment variable template (.env text or entries) and generates secrets.                                                                                                                                                   |
| `PUT /api/apps/{id}/git_token`                                 | Edit          | Sets the token of a private repository.                                                                                                                                                                                                   |
| `DELETE /api/apps/{id}/git_token`                              | Edit          | Deletes the token.                                                                                                                                                                                                                        |
| `POST /api/apps/{id}/webhook_secret`                           | Edit          | Creates a new webhook secret and returns it once.                                                                                                                                                                                         |
| `DELETE /api/apps/{id}/webhook_secret`                         | Edit          | Turns the webhook off.                                                                                                                                                                                                                    |
| `POST /api/apps/{id}/hook`                                     | Edit          | Installs the webhook of an app with a connection again.                                                                                                                                                                                   |
| `POST /hooks/apps/{id}`                                        | The signature | Deploys the app for a signed push. See [Webhooks](#webhooks).                                                                                                                                                                             |
| `GET /api/apps/{id}/deployments`                               | Read          | The latest 100 deployments of an app.                                                                                                                                                                                                     |
| `GET /api/apps/{id}/logs?tail=200`                             | Read          | The last lines of the container log of the running deployment, from 1 to 1000 lines. `running` is `false` when the app has no running deployment.                                                                                         |
| `GET /api/apps/{id}/logs/live?tail=200`                        | Read          | Upgrades the connection to a WebSocket live stream of the running container log.                                                                                                                                                          |
| `GET /api/apps/{id}/logs/size`                                  | Read          | Returns the active and rotated container log sizes for an application.                                                                                                                                                                    |
| `POST /api/apps/{id}/logs/vacuum`                               | Edit          | Truncates active container logs and deletes rotated log files for an application.                                                                                                                                                          |
| `POST /api/apps/{id}/deploy`                                   | Edit          | Starts a deployment.                                                                                                                                                                                                                      |
| `GET /api/apps/{id}/terminal?shell=`                           | Admin         | Switches the connection to a WebSocket terminal of the running container, with optional shell or database CLI selection.                                                                                                                  |
| `GET /api/apps/{id}/files?path=`                               | Admin         | The entries of a directory inside a volume of the running container.                                                                                                                                                                      |
| `GET /api/apps/{id}/files/download?path=`                      | Admin         | The content of a file inside a volume.                                                                                                                                                                                                    |
| `PUT /api/apps/{id}/files?path=`                               | Admin         | Writes the request body into a file inside a writable volume.                                                                                                                                                                             |
| `DELETE /api/apps/{id}/files?path=`                            | Admin         | Deletes a file or a directory inside a writable volume.                                                                                                                                                                                   |
| `GET /api/apps/{id}/commands`                                  | Read          | The scheduled commands configured for an app.                                                                                                                                                                                             |
| `POST /api/apps/{id}/commands`                                 | Edit          | Adds a scheduled command to an app.                                                                                                                                                                                                       |
| `PUT /api/apps/{id}/commands/{cmd_id}`                         | Edit          | Replaces a scheduled command.                                                                                                                                                                                                             |
| `DELETE /api/apps/{id}/commands/{cmd_id}`                      | Edit          | Deletes a scheduled command.                                                                                                                                                                                                              |
| `POST /api/apps/{id}/commands/{cmd_id}/run`                    | Edit          | Runs a scheduled command inside the container immediately and returns the output.                                                                                                                                                         |
| `GET /api/databases`                                           | Read          | The database apps, with their engine, their port and their connection.                                                                                                                                                                    |
| `GET /api/databases/engines`                                   | Read          | The engines and the versions that the panel offers.                                                                                                                                                                                       |
| `POST /api/databases/engines/refresh`                          | Edit          | Downloads the engine catalog from the r3v3rs3-apps repository.                                                                                                                                                                            |
| `POST /api/databases`                                          | Edit          | Creates a database, deploys it and returns its password once.                                                                                                                                                                             |
| `POST /api/databases/{id}/clone`                               | Edit          | Clones an existing database, deploys it and restores the data.                                                                                                                                                                            |
| `DELETE /api/databases/{id}`                                   | Edit          | Deletes the database app. Its volume stays.                                                                                                                                                                                               |
| `GET /api/deployments/{id}`                                    | Read          | Returns a deployment.                                                                                                                                                                                                                     |
| `POST /api/deployments/{id}/rollback`                          | Edit          | Repeats an earlier deployment.                                                                                                                                                                                                            |
| `POST /api/deployments/{id}/approve`                           | Edit          | Approves a pending deployment and starts it.                                                                                                                                                                                              |
| `POST /api/deployments/{id}/reject`                            | Edit          | Rejects a pending deployment.                                                                                                                                                                                                             |
| `GET /api/git/connections`                                     | Edit          | The Git provider connections, without secrets or tokens.                                                                                                                                                                                  |
| `POST /api/git/connections`                                    | Admin         | Adds a GitLab or Gitea connection.                                                                                                                                                                                                        |
| `GET /api/git/connections/{id}`                                | Edit          | Returns a connection.                                                                                                                                                                                                                     |
| `PUT /api/git/connections/{id}`                                | Admin         | Replaces a connection. Without `client_secret` the current secret stays. A GitHub App connection changes only its name.                                                                                                                   |
| `DELETE /api/git/connections/{id}`                             | Admin         | Deletes a connection that no app uses.                                                                                                                                                                                                    |
| `POST /api/git/connections/{id}/authorize`                     | Admin         | Starts an authorization with `{"redirect_uri": "<callback address>"}` and returns the page of the provider, or the installation page of a GitHub App.                                                                                     |
| `POST /api/git/connections/{id}/installation`                  | Admin         | Reads the installation of the GitHub App from GitHub and connects the connection.                                                                                                                                                         |
| `POST /api/git/github_apps`                                    | Admin         | Starts a GitHub App with `{"name": "...", "url": "...", "organization": "...", "redirect_uri": "<callback address>"}`, `url` and `organization` optional, and returns the form address and the manifest that the browser posts to GitHub. |
| `GET /api/git/connections/{id}/repositories?search=&page=`     | Edit          | Up to 50 repositories of the connected account or of the installation.                                                                                                                                                                    |
| `GET /api/git/connections/{id}/branches?repository=owner/name` | Edit          | Up to 50 branches of a repository.                                                                                                                                                                                                        |
| `GET /api/platform/prune`                                      | Read          | The prune policy and disk hygiene settings.                                                                                                                                                                                               |
| `PUT /api/platform/prune`                                      | Admin         | Replaces the prune policy.                                                                                                                                                                                                                |
| `POST /api/platform/prune/run`                                 | Admin         | Runs disk hygiene prune immediately and returns reclaimed space.                                                                                                                                                                          |
| `POST /api/platform/logs/vacuum`                                | Admin         | Truncates all container logs across all applications exceeding the given threshold in megabytes.                                                                                                                                           |
| `GET /api/webhooks`                                            | Read          | Lists all configured outbound webhooks.                                                                                                                                                                                                   |
| `POST /api/webhooks`                                           | Admin         | Creates a new outbound webhook.                                                                                                                                                                                                           |
| `GET /api/webhooks/{id}`                                       | Read          | Returns details of an outbound webhook.                                                                                                                                                                                                   |
| `PUT /api/webhooks/{id}`                                       | Admin         | Updates an outbound webhook configuration or secret.                                                                                                                                                                                      |
| `DELETE /api/webhooks/{id}`                                    | Admin         | Deletes an outbound webhook.                                                                                                                                                                                                              |
| `POST /api/webhooks/test`                                      | Admin         | Sends an immediate test ping payload to a target URL.                                                                                                                                                                                     |
| `GET /oauth/git/callback`                                      | The state     | The callback of the provider and of a new GitHub App. See [Connect](#connect) and [Create a GitHub App](#create-a-github-app).                                                                                                            |

An account with a proxy list gets `403 forbidden` for every platform route. See [Accounts](@/accounts.md). Every platform route answers `503 platform_disabled` when `[platform] enabled` is off, and `503 platform_failed` when the platform failed to open its database or its key. A name that another app, target or connection uses answers `409 app_name_exists`, `409 target_name_exists` or `409 git_connection_name_exists`. A connection that is not connected answers `409 git_connection_not_connected`. `POST /api/git/connections/{id}/installation` before the GitHub App is installed answers `409 github_app_not_installed`, and a failed request to the provider answers `502 git_provider_failed`. The audit log records every change of an app, every deployment, every rollback, every webhook request that deploys, every change of a target, every agent enrollment, every change and authorization of a Git provider connection, and every webhook installation that an account starts. The summary of an environment change names the keys, never a value, a token or secret change names only the app or the target, a webhook request names the app, the provider and the deployment, an enrollment names the target and the version of the agent, a connection names itself and its provider, never its client secret or a token, an authorization also names the account, and a webhook installation names the app.
