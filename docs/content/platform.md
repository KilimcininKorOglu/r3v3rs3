+++
title = "Deployment Platform"
description = "Deploy container images and route their domains through r3v3rs3"
weight = 0
+++

# Deployment Platform

The deployment platform runs apps as containers on the Docker Engine of the r3v3rs3 server and routes their domains through r3v3rs3 proxies. A new deployment starts next to the running container, and the proxy switches to it only after it passes its health check.

The platform is under development. This version deploys a ready image from a registry, builds an image from a Git repository with its Dockerfile, or starts the Docker Compose file of a Git repository, on the local Docker Engine through the admin API. The WebUI pages and remote servers follow in later versions.

## Enable the Platform

Add a `[platform]` section to `config.toml` and restart the server:

```toml
[platform]
enabled = true
docker = "unix:///var/run/docker.sock"
proxy_ports = ["http", "https"]
acme = "bcd-fgh"
```

| Key | Default | Description |
|---|---|---|
| `enabled` | `false` | Starts the platform. A cluster node does not run the platform. |
| `docker` | `unix:///var/run/docker.sock` | The Docker Engine API: a Unix socket or plain TCP. TLS is not supported. |
| `proxy_ports` | empty | The names or the ids of the ports that serve the apps, for example the HTTP port and the HTTPS port. |
| `acme` | none | The id of the ACME entry that orders the certificates of the app domains. Without it the apps get no certificate. |

The admin API does not change the `[platform]` section. A change takes effect after a restart.

At its first start the platform creates two files in the config directory:

- `platform.db`: the apps, their environment variables and their deployments.
- `platform.key`: the key that encrypts the environment values. Its mode is `0600`. Keep a backup of it next to the backup of `platform.db`, because the values do not open without it.

Access to the Docker socket is equal to root access on the host. Enable the platform only on a server where r3v3rs3 may control every container. When r3v3rs3 runs in a container, mount `/var/run/docker.sock` into it.

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

- `target` is `local`, the Docker Engine of the r3v3rs3 server.
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

An app runs one deployment at a time. A second deploy, a rollback or a deletion during a deployment gets `409 app_busy`. A restart of the server marks an unfinished deployment as `failed`.

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

## Routing

The platform adds one proxy for each running app that has at least one domain. The proxies come from the `Platform` provider, so they are read-only like the proxies of [service discovery](@/discovery.md), and the proxy list shows the provider status. Each proxy:

- listens on the ports of `proxy_ports`,
- answers the domains of the app,
- sends the requests to the published port of the container on `127.0.0.1`,
- gets its certificate from the ACME entry of `acme`, with the account, the challenge and the DNS provider of the entry. The rules of [ACME Certificates](@/discovery.md#acme-certificates) apply.

An app without a domain gets no proxy. A running deployment whose container is missing or stopped appears as an issue in the provider status. r3v3rs3 reads the containers every 15 seconds, so a container that Docker restarts on a new port gets its route back.

## Deleting an App

`DELETE /api/apps/{id}` stops and removes the containers, the network and the built images of the app, removes its proxy, and deletes its environment variables and its deployments. The named volumes stay. For a Compose app it runs `docker compose down`, removes the images that the project built and deletes its checkout. The volumes of the Compose project stay.

## Admin API

| Route | Permission | Description |
|---|---|---|
| `GET /api/targets` | Read | The Docker hosts that run apps. |
| `GET /api/apps` | Read | The apps. |
| `POST /api/apps` | Edit | Adds an app. |
| `GET /api/apps/{id}` | Read | Returns an app. |
| `PUT /api/apps/{id}` | Edit | Replaces an app. |
| `DELETE /api/apps/{id}` | Edit | Deletes an app with its containers. |
| `GET /api/apps/{id}/env` | Edit | The environment variables, without secret values. |
| `PUT /api/apps/{id}/env` | Edit | Replaces the environment variables. |
| `PUT /api/apps/{id}/git_token` | Edit | Sets the token of a private repository. |
| `DELETE /api/apps/{id}/git_token` | Edit | Deletes the token. |
| `GET /api/apps/{id}/deployments` | Read | The latest 100 deployments of an app. |
| `POST /api/apps/{id}/deploy` | Edit | Starts a deployment. |
| `GET /api/deployments/{id}` | Read | Returns a deployment. |
| `POST /api/deployments/{id}/rollback` | Edit | Repeats an earlier deployment. |

An account with a proxy list gets `403 forbidden` for every platform route. See [Accounts](@/accounts.md). The audit log records every change of an app, every deployment and every rollback. The summary of an environment change names the keys, never a value, and a token change names only the app.
