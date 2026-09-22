+++
title = "Installing with Docker"
description = "Run r3v3rs3 in one container with two volumes"
weight = 2
+++

# Installing with Docker

One container runs r3v3rs3. Two volumes keep the configuration and the data, so an upgrade replaces only the image.

The image is `ghcr.io/kilimcininkoroglu/r3v3rs3`. It is a distroless image: it holds the r3v3rs3 binary and no shell, so `docker exec` runs `r3v3rs3` and no other program.

## Step 1: Start the Container

```bash
$ docker run -d \
  -v r3v3rs3-config:/root/.config/r3v3rs3 \
  -v r3v3rs3-data:/root/.local/share/r3v3rs3 \
  -p 80:80 \
  -p 443:443 \
  -p 127.0.0.1:46492:46492 \
  --restart unless-stopped \
  --stop-signal SIGINT \
  --name r3v3rs3 \
  ghcr.io/kilimcininkoroglu/r3v3rs3:latest
```

Each option does one thing:

| Option | Reason |
|---|---|
| `-v r3v3rs3-config:/root/.config/r3v3rs3` | The accounts, the proxies, the certificates and the ACME entries. Without it, a new container starts empty. |
| `-v r3v3rs3-data:/root/.local/share/r3v3rs3` | `log.db`, which holds the audit log. |
| `-p 80:80 -p 443:443` | The ports that your proxies use. Publish every port that you add in the WebUI. |
| `-p 127.0.0.1:46492:46492` | The admin panel, on the host only. The image starts with `--webui 0.0.0.0:46492`, so the container listens on every interface and this binding limits it. |
| `--stop-signal SIGINT` | r3v3rs3 shuts down gracefully on SIGINT. Without it, `docker stop` waits ten seconds and then kills the process. |
| `--restart unless-stopped` | The container starts again after a reboot. |

## Step 2: Create the Admin Account

```bash
$ docker exec -t -i r3v3rs3 r3v3rs3 add-user admin
password?: ******
```

The password needs at least 8 characters. Open [http://localhost:46492/](http://localhost:46492/) and sign in.

On a remote host, reach the panel through SSH instead of publishing its port:

```bash
$ ssh -L 46492:127.0.0.1:46492 user@your-server
```

## Step 3: Add a Port

A proxy needs a port, and the container needs to publish it.

1. Add a port in the WebUI, for example `0.0.0.0:80` with the protocol **HTTP**.
2. Check that `-p 80:80` of Step 1 publishes it.

A port that the container does not publish listens inside the container only. Docker cannot add a published port to a running container, so `docker rm` and `docker run` it again with the new `-p` option. The volumes keep everything.

[Getting Started](@/tutorials/getting-started.md) continues with the first proxy.

## Docker Compose

The repository holds a `docker-compose.yml` with host networking:

```bash
$ curl -fsSLO https://raw.githubusercontent.com/KilimcininKorOglu/r3v3rs3/main/docker-compose.yml
$ docker compose up -d
$ docker compose exec r3v3rs3 r3v3rs3 add-user admin
```

Host networking publishes no port, so every port that you add in the WebUI listens on the host at once. It works on a Linux Docker host only. Two variables in a `.env` file next to the file change the setup:

- `R3V3RS3_WEBUI`: the admin panel address. The default is `127.0.0.1:46492`.
- `R3V3RS3_VERSION`: the image tag. The default is `latest`.

## Deployment Platform

The [deployment platform](@/platform.md) needs the `git` binary for Git sources and the `docker` binary with the Compose plugin for Compose sources. The default image has neither. Use the image with the `-platform` tag suffix, for example `latest-platform`. It is a `debian:trixie-slim` image with `git`, the `docker` CLI and its Compose and buildx plugins.

```bash
$ sudo mkdir -p /var/lib/r3v3rs3
$ docker run -d \
  --network host \
  -v /var/run/docker.sock:/var/run/docker.sock \
  -v /var/lib/r3v3rs3:/var/lib/r3v3rs3 \
  -e R3V3RS3_CONFIG_DIR=/var/lib/r3v3rs3 \
  -v r3v3rs3-data:/root/.local/share/r3v3rs3 \
  --restart unless-stopped \
  --stop-signal SIGINT \
  --name r3v3rs3 \
  ghcr.io/kilimcininkoroglu/r3v3rs3:latest-platform
```

The command differs from Step 1 in three places:

| Option | Reason |
|---|---|
| `--network host` | Docker publishes the port of an app on `127.0.0.1` of the host, and r3v3rs3 reaches it there. In a bridge network, `127.0.0.1` is the container itself. |
| `-v /var/run/docker.sock:/var/run/docker.sock` | The platform starts the app containers through the Docker Engine of the host. Access to the socket is equal to root access on the host. |
| `-v /var/lib/r3v3rs3:/var/lib/r3v3rs3` and `R3V3RS3_CONFIG_DIR` | The config directory holds the checkouts of the Compose apps. `docker compose` sends the paths of a bind mount to the Docker Engine of the host, so a bind mount of a file from the repository works only when the directory has the same path on the host and in the container. |

Then enable the platform in `/var/lib/r3v3rs3/config.toml` and restart the container.

## Upgrade

```bash
$ docker pull ghcr.io/kilimcininkoroglu/r3v3rs3:latest
$ docker rm -f r3v3rs3
$ docker run -d ... # the same command as Step 1
```

With Compose:

```bash
$ docker compose pull
$ docker compose up -d
```

The volumes keep the configuration and the accounts, so no account is created again. Pin a version with the tag, for example `:1.0.1`, when you upgrade several hosts in steps.

## What a Restart Does

- The accounts, the ports, the proxies and the certificates come back from the config volume.
- The sessions do not. r3v3rs3 keeps them in memory, so every client signs in again.
- The cached responses and the rate limit counters are also in memory and start empty.

## Next Steps

- [Getting Started](@/tutorials/getting-started.md): the first port and the first proxy.
- [Proxies from Docker](@/tutorials/docker-discovery.md): let your other containers define their proxies with labels.
- [Installing on a Linux Server](@/tutorials/install-linux.md): the systemd install without Docker.
