+++
title = "Deployment Platform"
description = "Deploy container images and route their domains through r3v3rs3"
weight = 0
+++

# Deployment Platform

The deployment platform runs apps as containers on the Docker Engine of the r3v3rs3 server, or of a remote server that runs the r3v3rs3 agent, and routes their domains through r3v3rs3 proxies. A new deployment starts next to the running container, and the proxy switches to it only after it passes its health check.

The platform is under development. This version deploys a ready image from a registry, builds an image from a Git repository with its Dockerfile, or starts the Docker Compose file of a Git repository, on the local Docker Engine or on an [agent target](#agent-targets), through the WebUI or the admin API.

## Enable the Platform

Add a `[platform]` section to `config.toml` and restart the server:

```toml
[platform]
enabled = true
docker = "unix:///var/run/docker.sock"
proxy_ports = ["http", "https"]
acme = "bcd-fgh"
agent_port = 9443
agent_host = "master.example.com"
```

| Key | Default | Description |
|---|---|---|
| `enabled` | `false` | Starts the platform. A cluster node does not run the platform. |
| `docker` | `unix:///var/run/docker.sock` | The Docker Engine API: a Unix socket or plain TCP. TLS is not supported. |
| `proxy_ports` | empty | The names or the ids of the ports that serve the apps, for example the HTTP port and the HTTPS port. |
| `acme` | none | The id of the ACME entry that orders the certificates of the app domains. Without it the apps get no certificate. |
| `agent_port` | none | The TCP port on every IPv4 address that the agents of remote servers connect to. Without it the server accepts no agent. See [Agent targets](#agent-targets). |
| `agent_host` | none | The host name or the address of this server in the agent commands of the **Targets** page. Without it the page uses the host name of its own address. |

The admin API does not change the `[platform]` section. A change takes effect after a restart.

At its first start the platform creates two files in the config directory:

- `platform.db`: the apps, their environment variables and their deployments.
- `platform.key`: the key that encrypts the environment values. Its mode is `0600`. Keep a backup of it next to the backup of `platform.db`, because the values do not open without it.

Access to the Docker socket is equal to root access on the host. Enable the platform only on a server where r3v3rs3 may control every container. When r3v3rs3 runs in a container, use the image with the `-platform` tag suffix, host networking and the Docker socket, as [Installing with Docker](@/tutorials/install-docker.md#deployment-platform) shows.

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
- `health_check_path` is an HTTP path that answers `2xx` or `3xx` when the app is ready. Without it a deployment waits until the port accepts a connection and keeps it open.
- `volumes` mounts named Docker volumes. A host path cannot be mounted.
- `restart` is `no`, `on-failure`, `unless-stopped` or `always`.

A change of an app, of its domains or of its environment variables takes effect at the next deployment.

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

| Field | Default | Description |
|---|---|---|
| `repository` | | An `https://` URL without a user name, a password, a query or a fragment. |
| `branch` | `main` | A branch or a tag. |
| `context` | `.` | The directory of the repository that Docker receives as the build context. |
| `dockerfile` | `Dockerfile` | The Dockerfile, relative to `context`. |

A private repository needs an access token, for example a GitHub fine-grained token with read access to the contents of the repository, or a GitLab or Gitea token with the `read_repository` scope. `PUT /api/apps/{id}/git_token` with `{"token": "..."}` sets the token, and `DELETE /api/apps/{id}/git_token` deletes it. r3v3rs3 encrypts the token with `platform.key`, and the admin API never returns it: an app shows only `"git_token_set": true`. git receives the token as HTTP basic authentication with the user name `x-access-token`, through an environment variable instead of the URL or the command line.

The server that runs r3v3rs3 needs the `git` binary. r3v3rs3 runs `git` without the configuration of the host, without hooks and only over HTTPS. The build context holds no `.git` directory, and a symbolic link in it stays a link, so a build does not read a file from outside the repository. The context can hold up to 512 MiB.

Docker builds the image with its classic builder. Two builds run at the same time, and every other build waits. The builder keeps the stages of a multi-stage Dockerfile as its build cache, so the next build of the same app is faster. `docker image prune` removes that cache.

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

| Field | Default | Description |
|---|---|---|
| `repository` | | An `https://` URL, as for a Git source. The Git token of the app clones a private repository. |
| `branch` | `main` | A branch or a tag. |
| `file` | the first of `compose.yaml`, `compose.yml`, `docker-compose.yaml` and `docker-compose.yml` | The Compose file, relative to the root of the repository. Its relative paths resolve against its own directory. |
| `service` | | The service that receives the requests of the domains on `port`. A service name has lowercase letters, digits and `_.-`. |

The server needs the `git` binary and the `docker` binary with the Compose plugin. r3v3rs3 runs `docker compose` with the Compose project `r3v3rs3-<app id>` and adds a file that sets for the service of `service`:

- the container name `r3v3rs3-<app id>-<deployment id>` and the labels of the platform,
- one published port: `port` on a free port of `127.0.0.1`. This port replaces the `ports` of the service in the Compose file.
- the environment variables of the app. The file names only their keys, and `docker compose` reads the values from its own environment, so no value reaches the disk. The same values fill the `${VARIABLE}` references of the Compose file. The names `PATH`, `HOME`, `DOCKER_CONFIG` and `DOCKER_HOST` belong to the `docker` binary, and an app that sets one fails its deployment.

Another service of the Compose file must not publish a port, because a published port bypasses r3v3rs3. A Compose file with such a port fails the deployment before any service starts, and the message names the service. The services reach each other on the network of the project.

A Compose app sets `volumes`, `restart` and `limits` in the Compose file, so its app spec does not accept them. The service of `service` runs as one container, because it has a container name.

A Compose deployment recreates the service of `service`: Docker stops the old container before the new one starts, so the app does not answer until the new container passes its health check. A deployment that fails its health check keeps the new container for its log, and the app has no healthy container until the next deployment or rollback. The checkout of the running deployment stays in `compose/<app id>/` of the config directory, because a bind mount of the Compose file can read it.

r3v3rs3 does not check the other settings of a Compose file. A Compose file can mount a host path, run a privileged container or use the network of the host, so every account that can edit an app can take over the host through it. Give the Edit permission only to accounts that may do that.

### Environment Variables

`PUT /api/apps/{id}/env` replaces the environment variables of an app. A variable with `"secret": true` never leaves the server again: the admin API returns it without its value, and an update without a value keeps the current value. r3v3rs3 encrypts every value with `platform.key` and decrypts it only when a container starts.

## Deployments

`POST /api/apps/{id}/deploy` starts a deployment and returns it with the status `queued`. `GET /api/deployments/{id}` shows its progress.

1. r3v3rs3 pulls the image and records its digest. For a Git source it clones the branch, records the commit, builds the image as `r3v3rs3/<app id>:<deployment id>` and records the image id. For a Compose source it clones the branch, records the commit and runs `docker compose up --build`, then continues with step 3.
2. It creates the container `r3v3rs3-<app id>-<deployment id>` on the network `r3v3rs3-<app id>`, so the containers of different apps do not reach each other. The container publishes its port on a free port of `127.0.0.1`, so only r3v3rs3 on the host reaches it.
3. It waits up to 120 seconds for the health check. A container that stops or fails the check ends the deployment as `failed`, and the failure message holds the last lines of the container log. The old container keeps serving.
4. It marks the deployment `running`, marks the previous one `superseded`, and routes the domains to the new container.
5. After 10 seconds it stops and removes the old container, so that its open requests end.

An app runs one deployment at a time. A second deploy, a rollback or a deletion during a deployment gets `409 app_busy`. A [webhook](#webhooks) push during a deployment queues one more deployment instead. A restart of the server marks an unfinished deployment as `failed`.

| Status | Meaning |
|---|---|
| `queued` | The deployment waits to start. |
| `building` | r3v3rs3 clones the branch and builds the image, or reads the Compose file. |
| `deploying` | The image is pulled, `docker compose up` runs, or the new container waits for its health check. |
| `running` | The container of the deployment serves the app. |
| `superseded` | A newer deployment replaced this one. |
| `failed` | The deployment stopped. `message` holds the reason. |

### Rollback

`POST /api/deployments/{id}/rollback` starts a new deployment with the image digest, the app settings and the environment variables of an earlier deployment. The digest, not the tag, selects the image, so a rollback runs the same image even after the tag moved. A deployment that failed before it pulled its image has no digest and answers `400 rollback_unavailable`.

A rollback of a Git app starts the built image again and does not build. r3v3rs3 keeps the images of the 5 newest deployments of an app and of its running deployment, and removes older ones. A rollback to a deployment whose image is removed fails with the message `the image ... is no longer present`.

A rollback of a Compose app clones the recorded commit of the earlier deployment and runs `docker compose up --build` again, with the app settings and the environment variables of that deployment. A Compose deployment that failed before it recorded its commit answers `400 rollback_unavailable`.

## Webhooks

A webhook deploys an app when its Git provider sends a push event. `POST /api/apps/{id}/webhook_secret` creates the secret of the app and returns it once. The secret is 64 hex characters, and r3v3rs3 encrypts it with `platform.key`. A second call creates a new secret, and the old one stops working at once. `DELETE /api/apps/{id}/webhook_secret` turns the webhook off.

The provider sends its events to `POST /hooks/apps/{id}` on the address of the admin API. This route needs no session, because the signature of each request proves it. The admin API listens on `127.0.0.1` by default, so the provider needs a way in: add a proxy that sends the path prefix `/hooks/` of a public domain to the admin port, and keep every other path of the admin API private.

| Provider | Setting |
|---|---|
| GitHub | **Payload URL** is the hook address, **Content type** is `application/json`, and **Secret** is the secret. GitHub signs the body in the `X-Hub-Signature-256` header. |
| Gitea, Forgejo | **Target URL** is the hook address, **POST Content Type** is `application/json`, and **Secret** is the secret. The provider signs the body in the `X-Gitea-Signature` or `X-Forgejo-Signature` header. |
| GitLab | **URL** is the hook address, and **Secret token** is the secret. GitLab sends the secret itself in the `X-Gitlab-Token` header. Enable **Push events**, and **Tag push events** when the branch field of the app names a tag. |
| Another sender | Sign the body with HMAC-SHA256 and the secret, and send the hex signature as `X-Signature-256: sha256=<hex>`. A CI job can deploy an image app this way after it pushes a new image. |

Which request deploys:

- A Git or a Compose app deploys for a push whose `ref` is `refs/heads/<branch>` or `refs/tags/<branch>`, where `<branch>` is the branch field of the app. A push that deletes the branch does not deploy.
- An image app has no branch, so every signed push deploys it again, and r3v3rs3 pulls the tag of the image again.
- A request of another sender always deploys, with any body.
- A ping, a push to another branch, or another event answers `200` with `{"outcome": "ignored"}`.

A deploying request answers `200` with `{"outcome": "deployed", "deployment": {...}}`. The deployment has the trigger `webhook` and the account `webhook`. A push during a running deployment answers `202` with `{"outcome": "queued"}`. When the running deployment ends, one more deployment starts with the latest commit of the branch, so one queued deployment covers every push that arrived in between. r3v3rs3 keeps the queue in memory, so a restart drops it.

| Status | Reason |
|---|---|
| `400 invalid_webhook_payload` | The body of a GitHub, Gitea or GitLab push is not JSON, for example with the content type `application/x-www-form-urlencoded`. |
| `401 unauthorized` | The signature is missing or wrong. |
| `404 id_not_found` | No app has this id, or the app has no webhook secret. |
| `429` | The client sent more than 10 requests in a burst, or more than one request per second after the burst. |

The body can be up to 5 MiB. The audit log records every request that deployed or queued a deployment, with the address of the sender, and without an account.

## Notifications

The notification webhook of the "Settings" page also gets the events of the platform: `deployment_started`, `deployment_running` and `deployment_failed` for each deployment, and `agent_online` and `agent_offline` for each agent target. See [Notifications](@/configuration.md#notifications) for the JSON body.

## Routing

The platform adds one proxy for each running app that has at least one domain. The proxies come from the `Platform` provider, so they are read-only like the proxies of [service discovery](@/discovery.md), and the proxy list shows the provider status. Each proxy:

- listens on the ports of `proxy_ports`,
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

With Docker, use the image with the `-platform` tag suffix, because it holds the `docker` binary with the Compose plugin:

```sh
docker run -d --name r3v3rs3-agent --restart unless-stopped --network host --stop-signal SIGINT \
  -v /var/run/docker.sock:/var/run/docker.sock \
  -v /var/lib/r3v3rs3-agent:/var/lib/r3v3rs3-agent \
  --entrypoint /usr/bin/r3v3rs3 \
  ghcr.io/kilimcininkoroglu/r3v3rs3:latest-platform \
  agent --master master.example.com:9443 --data-dir /var/lib/r3v3rs3-agent --token <token>
```

The container needs host networking, because the agent reaches the apps on `127.0.0.1` of the host. The data directory needs the same path in the container and on the host, because `docker compose` sends the bind mount paths of a Compose file to the Docker Engine of the host.

| Option | Environment variable | Default | Description |
|---|---|---|---|
| `--master` | `R3V3RS3_AGENT_MASTER` | | The agent port of the master as `HOST:PORT`. |
| `--token` | `R3V3RS3_AGENT_TOKEN` | | The enrollment token. Only the first start needs it; an enrolled agent ignores it. |
| `--data-dir` | `R3V3RS3_AGENT_DATA_DIR` | `agent` in the data directory of the user | The key, the certificate and the Compose files of the agent. |
| `--docker` | `R3V3RS3_AGENT_DOCKER` | `unix:///var/run/docker.sock` | The Docker Engine API of the agent host. |
| `--log-level` | `R3V3RS3_AGENT_LOG_LEVEL` | `info` | The log level. |

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

## WebUI

The **Platform** group of the sidebar holds the **Apps** and **Targets** pages. The group appears only for an account without a proxy list. An account with the Read permission sees the apps, their deployments and their logs. The Edit permission adds, changes, deploys and deletes apps.

### Apps

The **Apps** page lists every app with its **Source**, its **Domains** and the status of its **Last Deployment**. The row actions of an app are:

- **Edit** opens the app page. An account without the Edit permission sees **View** and a read-only form.
- **Deployments** opens the deployment history.
- **Log** opens the container log.
- **Deploy** starts a deployment after a confirmation.

The page reads the apps again every 2 seconds while a deployment is unfinished, and every 10 seconds otherwise.

### Add and Change an App

**Add** opens an empty app form. **Source** selects **Image**, **Git** or **Compose**, and the form shows the fields of that source. A Compose app hides **Volumes**, **Restart Policy**, **Memory Limit (MB)** and **CPU Limit**, because its Compose file sets them.

- **Domains** takes one domain on each line.
- **Volumes** takes one mount on each line: `volume:/path`, or `volume:/path:ro` for a read-only mount.
- **Memory Limit (MB)** takes a whole number of megabytes, and **CPU Limit** a number of CPUs such as `1.5`. An empty field sets no limit.

**Create** saves the app and opens its page. The page of an existing app adds these parts below the form:

- **Git Token**, for a Git or a Compose app. **Set Token** saves a token, and **Remove token** deletes it after a confirmation. The page shows only whether a token is set.
- **Webhook**. **Create Secret** creates the [webhook](#webhooks) secret and shows it once with the **Payload URL**, which is the hook address on the address of the page. Copy both before you leave the page. **Create New Secret** replaces the secret after a confirmation, and **Turn off webhook** deletes it after a confirmation.
- **Environment Variables**. **Add variable** adds a row with a **Key**, a **Value** and a **Secret** checkbox. The value of a saved secret variable shows as **Unchanged**. Leave it empty to keep the value. **Save Variables** replaces all variables, and the next deployment uses them.
- **Delete app** deletes the app with its containers after a confirmation.

### Deployments

The deployment history shows the latest 100 deployments with their status, **Trigger**, **Commit**, **Account**, **Started**, **Duration** and **Message**. **Roll back** starts an earlier deployment again after a confirmation. The link appears only for a finished deployment that no longer runs and has a recorded image digest, or a recorded commit for a Compose app. It hides while a deployment of the app is unfinished. The page follows the same refresh intervals as the **Apps** page.

### Log

The log page shows the last 200 lines of stdout and stderr of the running container and reads them again every 10 seconds. **Refresh** reads them at once. An app without a running container shows "The app has no running container."

### Targets

The **Targets** page lists the local target and the agent targets with their **Kind**, their **Status** (**Online**, **Offline** or **Waiting for enrollment**), **Last seen** and the **Version** of the agent. It reads the list again every 10 seconds. The **Target** select of the app form shows the status of each target next to its name.

An admin account also sees:

- **Add an agent target** below the list. **Add** creates the target and shows its **Token** once at the top of the page, with the `install.sh` command and the `docker run` command of the agent. Copy them before you leave the page.
- **New token** on the row of an agent target, which creates a new enrollment token after a confirmation.
- **Delete** on the row of an agent target, which deletes it after a confirmation.

Without `agent_port` the add and the new token actions answer that the agent port is not set.

## Admin API

| Route | Permission | Description |
|---|---|---|
| `GET /api/targets` | Read | The Docker hosts that run apps, with the state of their agents. |
| `POST /api/targets` | Admin | Adds an agent target and returns its enrollment token once. |
| `POST /api/targets/{id}/token` | Admin | Returns a new enrollment token of an agent target. |
| `DELETE /api/targets/{id}` | Admin | Deletes an agent target without apps and disconnects its agent. |
| `GET /api/apps` | Read | The apps. |
| `POST /api/apps` | Edit | Adds an app. |
| `GET /api/apps/{id}` | Read | Returns an app. |
| `PUT /api/apps/{id}` | Edit | Replaces an app. |
| `DELETE /api/apps/{id}` | Edit | Deletes an app with its containers. |
| `GET /api/apps/{id}/env` | Edit | The environment variables, without secret values. |
| `PUT /api/apps/{id}/env` | Edit | Replaces the environment variables. |
| `PUT /api/apps/{id}/git_token` | Edit | Sets the token of a private repository. |
| `DELETE /api/apps/{id}/git_token` | Edit | Deletes the token. |
| `POST /api/apps/{id}/webhook_secret` | Edit | Creates a new webhook secret and returns it once. |
| `DELETE /api/apps/{id}/webhook_secret` | Edit | Turns the webhook off. |
| `POST /hooks/apps/{id}` | The signature | Deploys the app for a signed push. See [Webhooks](#webhooks). |
| `GET /api/apps/{id}/deployments` | Read | The latest 100 deployments of an app. |
| `GET /api/apps/{id}/logs?tail=200` | Read | The last lines of the container log of the running deployment, from 1 to 1000 lines. `running` is `false` when the app has no running deployment. |
| `POST /api/apps/{id}/deploy` | Edit | Starts a deployment. |
| `GET /api/deployments/{id}` | Read | Returns a deployment. |
| `POST /api/deployments/{id}/rollback` | Edit | Repeats an earlier deployment. |

An account with a proxy list gets `403 forbidden` for every platform route. See [Accounts](@/accounts.md). The audit log records every change of an app, every deployment, every rollback, every webhook request that deploys, every change of a target and every agent enrollment. The summary of an environment change names the keys, never a value, a token or secret change names only the app or the target, a webhook request names the app, the provider and the deployment, and an enrollment names the target and the version of the agent.
