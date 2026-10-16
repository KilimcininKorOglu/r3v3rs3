+++
title = "Proxies from Docker"
description = "Let your containers define their own proxies with labels"
weight = 5
+++

# Proxies from Docker

r3v3rs3 can read the labels of your running containers and build the proxies from them. A container that starts gets its proxy within about one second, and a container that stops loses it. You add no proxy by hand.

This guide builds that setup with Docker Compose and checks each step. It takes a few minutes.

## What You Build

```
   client ---> :80 r3v3rs3 (on the host) ---> whoami container(s)
                    |
                    +-- reads the Docker socket for labels and events
```

r3v3rs3 runs on the host and reads the Docker socket. It reads the label of each container and sends the traffic to the address of the container on the network.

> Access to the Docker socket equals root access on the host. r3v3rs3 reads the socket as the user that runs it, and the install script runs the service as root.

## Step 1: Install r3v3rs3 on the Host

[Installing on a Linux Server](@/tutorials/install-linux.md) installs the binary and creates the admin account. Open [http://localhost:46492/](http://localhost:46492/) and sign in.

## Step 2: Write the Compose File

```yaml
services:
  whoami:
    image: traefik/whoami
    networks: [proxy]
    labels:
      r3v3rs3.enable: "true"
      r3v3rs3.http.whoami.ports: http
      r3v3rs3.http.whoami.vhosts: whoami.example.com
      r3v3rs3.http.whoami.port: "80"

networks:
  proxy:
    name: proxy
```

The labels say: this container has one HTTP proxy named `whoami`, it uses the r3v3rs3 port named `http`, it answers for the host `whoami.example.com`, and its upstream server listens on port `80` of the container.

`name: proxy` under the network keeps the network name as `proxy`. Without it Compose adds the project name.

Start the service:

```bash
$ docker compose up -d
```

## Step 3: Bind the Port

A discovery provider opens no port. The label `ports: http` names a port that must already exist.

1. Click **Ports** in the menu, then **Add**.
2. Write `http` in the name field. The label uses this name.
3. Select the interface `0.0.0.0` and the port `80`, with the protocol **HTTP**.
4. Click **Create** and check the state **Listening**.

## Step 4: Turn On the Docker Provider

1. Click **Settings** in the menu.
2. Find **Docker Service Discovery** and turn it on.
3. Write `unix:///var/run/docker.sock` in the endpoint field.
4. Write `proxy` in the network field. This is the name of the Docker network of Step 2.
5. Leave **Read every container** off, so r3v3rs3 reads only the containers with `r3v3rs3.enable=true`.
6. Save.

The same settings in `config.toml`:

```toml
[discovery.docker]
enabled = true
endpoint = "unix:///var/run/docker.sock"
network = "proxy"
exposed_by_default = false
```

## Step 5: Check the Result

The **Proxies** page now lists a proxy named `whoami` with the source **Docker**. It has no edit button, because the container owns it. The page also shows the provider state:

```bash
$ curl -b session.txt http://127.0.0.1:46492/api/discovery
[{"provider":"docker","state":"running","proxies":1,"updated_at":1789642012}]
```

Send a request with the host of the label:

```bash
$ curl -H 'Host: whoami.example.com' http://127.0.0.1/
Hostname: 307d1b9d215d
IP: 192.168.164.2
...
```

Another host gets `502`, because no route matches it.

`state` is `error` when the provider cannot read the socket. An issue names the resource and the reason, for example `whoami: http.whoami: port not found: http`. That message means Step 3 is missing.

## Step 6: Scale the Service

```bash
$ docker compose up -d --scale whoami=3
```

Within a second the proxy has three servers, one per replica:

```bash
$ curl -H 'Host: whoami.example.com' http://127.0.0.1/ | grep Hostname
```

Repeat the request. The answers rotate over the three containers, because the replicas of one Compose service share their proxy and each replica adds its own server.

Stop a replica and the proxy drops its server. Stop every replica and the proxy disappears.

## Add More Containers

Each new container needs its own labels. The proxy name after the protocol keeps two containers apart:

```yaml
  api:
    image: my/api
    networks: [proxy]
    labels:
      r3v3rs3.enable: "true"
      r3v3rs3.http.api.ports: http
      r3v3rs3.http.api.vhosts: api.example.com
      r3v3rs3.http.api.port: "3000"
      r3v3rs3.http.api.rate_limit.requests: "100"
      r3v3rs3.http.api.rate_limit.per: minute
```

Every field of the admin API proxy model works as a label. [Labels](@/discovery.md#labels) lists the keys, the value formats and the TCP and UDP proxies. r3v3rs3 replaces a plain text password or token with a hash before it uses the proxy, so prefer `password_hash` and `token_hash` in a label.

## Certificates

An HTTP proxy from a label can get its certificate automatically. Create an ACME entry first, then name its id in the label:

```yaml
      r3v3rs3.http.api.acme: e7k-2np
```

r3v3rs3 orders a certificate for the `vhosts` of the proxy. [ACME Certificates](@/discovery.md#acme-certificates) describes the rules.

## Next Steps

- [Service Discovery](@/discovery.md): every provider and every label key.
- [Configuration](@/configuration.md): what each proxy field does.
- [Proxies from etcd](@/tutorials/etcd-discovery.md): proxies built from the keys of an etcd cluster.
