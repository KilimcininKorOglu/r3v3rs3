+++
title = "Deployment Platform"
description = "Deploy container images and route their domains through r3v3rs3"
weight = 0
+++

# Deployment Platform

The deployment platform runs apps as containers on the Docker Engine of the r3v3rs3 server and routes their domains through r3v3rs3 proxies. A new deployment starts next to the running container, and the proxy switches to it only after it passes its health check.

The platform is under development. This version deploys a ready image from a registry, or builds an image from a Git repository with its Dockerfile, on the local Docker Engine through the admin API. The WebUI pages, Docker Compose apps and remote servers follow in later versions.

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

The server that runs r3v3rs3 needs the `git` binary. r3v3rs3 runs `git` without the configuration of the host, without hooks and only over HTTPS. The build context holds no `.git` directory, and a symbolic link in it stays a link, so a build does not read a file from outside the repository. The context can hold up to 512 MiB.

Docker builds the image with its classic builder. Two builds run at the same time, and every other build waits. The builder keeps the stages of a multi-stage Dockerfile as its build cache, so the next build of the same app is faster. `docker image prune` removes that cache.

### Environment Variables

`PUT /api/apps/{id}/env` replaces the environment variables of an app. A variable with `"secret": true` never leaves the server again: the admin API returns it without its value, and an update without a value keeps the current value. r3v3rs3 encrypts every value with `platform.key` and decrypts it only when a container starts.

## Deployments

`POST /api/apps/{id}/deploy` starts a deployment and returns it with the status `queued`. `GET /api/deployments/{id}` shows its progress.

1. r3v3rs3 pulls the image and records its digest. For a Git source it clones the branch, records the commit, builds the image as `r3v3rs3/<app id>:<deployment id>` and records the image id.
2. It creates the container `r3v3rs3-<app id>-<deployment id>` on the network `r3v3rs3-<app id>`, so the containers of different apps do not reach each other. The container publishes its port on a free port of `127.0.0.1`, so only r3v3rs3 on the host reaches it.
3. It waits up to 120 seconds for the health check. A container that stops or fails the check ends the deployment as `failed`, and the failure message holds the last lines of the container log. The old container keeps serving.
4. It marks the deployment `running`, marks the previous one `superseded`, and routes the domains to the new container.
5. After 10 seconds it stops and removes the old container, so that its open requests end.

An app runs one deployment at a time. A second deploy, a rollback or a deletion during a deployment gets `409 app_busy`. A restart of the server marks an unfinished deployment as `failed`.

| Status | Meaning |
|---|---|
| `queued` | The deployment waits to start. |
| `building` | r3v3rs3 clones the branch and builds the image. |
| `deploying` | The image is pulled, or the new container waits for its health check. |
| `running` | The container of the deployment serves the app. |
| `superseded` | A newer deployment replaced this one. |
| `failed` | The deployment stopped. `message` holds the reason. |

### Rollback

`POST /api/deployments/{id}/rollback` starts a new deployment with the image digest, the app settings and the environment variables of an earlier deployment. The digest, not the tag, selects the image, so a rollback runs the same image even after the tag moved. A deployment that failed before it pulled its image has no digest and answers `400 rollback_unavailable`.

A rollback of a Git app starts the built image again and does not build. r3v3rs3 keeps the images of the 5 newest deployments of an app and of its running deployment, and removes older ones. A rollback to a deployment whose image is removed fails with the message `the image ... is no longer present`.

## Routing

The platform adds one proxy for each running app that has at least one domain. The proxies come from the `Platform` provider, so they are read-only like the proxies of [service discovery](@/discovery.md), and the proxy list shows the provider status. Each proxy:

- listens on the ports of `proxy_ports`,
- answers the domains of the app,
- sends the requests to the published port of the container on `127.0.0.1`,
- gets its certificate from the ACME entry of `acme`, with the account, the challenge and the DNS provider of the entry. The rules of [ACME Certificates](@/discovery.md#acme-certificates) apply.

An app without a domain gets no proxy. A running deployment whose container is missing or stopped appears as an issue in the provider status. r3v3rs3 reads the containers every 15 seconds, so a container that Docker restarts on a new port gets its route back.

## Deleting an App

`DELETE /api/apps/{id}` stops and removes the containers, the network and the built images of the app, removes its proxy, and deletes its environment variables and its deployments. The named volumes stay.

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
| `GET /api/apps/{id}/deployments` | Read | The latest 100 deployments of an app. |
| `POST /api/apps/{id}/deploy` | Edit | Starts a deployment. |
| `GET /api/deployments/{id}` | Read | Returns a deployment. |
| `POST /api/deployments/{id}/rollback` | Edit | Repeats an earlier deployment. |

An account with a proxy list gets `403 forbidden` for every platform route. See [Accounts](@/accounts.md). The audit log records every change of an app, every deployment and every rollback. The summary of an environment change names the keys, never a value.
