+++
title = "Getting Started"
description = "From the first account to your first working proxy"
weight = 1
+++

# Getting Started

This guide takes an installed r3v3rs3 and builds a working proxy. It takes a few minutes.

Install r3v3rs3 first. The [home page](@/_index.md#installation) lists every way: the Linux install script, Docker, Docker Compose, Cargo and the release archives.

## Step 1: Create an Admin Account

r3v3rs3 starts with no account. Create one on the command line. The command asks for the password, which needs at least 8 characters:

```bash
$ r3v3rs3 add-user admin
$ password?: ******
```

For two-factor authentication, add `--totp`. The command prints the secret once. Add it to your authenticator app at that time:

```bash
$ r3v3rs3 add-user admin --totp
$ password?: ******

Use this code to setup your TOTP client:
EXAMPLECODEEXAMPLECODE
```

`add-user` creates an admin account. [Accounts](@/accounts.md) describes the `editor` and `viewer` roles and the per-account proxy lists.

## Step 2: Start the Server

```bash
$ r3v3rs3 start
```

The admin WebUI listens on [http://localhost:46492/](http://localhost:46492/). Sign in with the account of Step 1.

> The WebUI serves plain HTTP. On a remote machine, reach it through SSH port forwarding instead of opening its port to the internet:
>
> ```bash
> $ ssh -L 46492:127.0.0.1:46492 user@your-server
> ```
>
> Step 5 shows how to serve the WebUI over HTTPS through r3v3rs3 itself.

## Step 3: Bind a Port

A proxy needs a port that listens for the clients.

1. Click **Ports** in the menu.
2. Click **Add**.
3. Name the port, for example `My Website`. The name can stay empty.
4. Select the network interface. `0.0.0.0` listens on every interface.
5. Select the port, for example `80`. Check that no other program uses it, and that you may bind it. A Linux port below 1024 needs root or the `CAP_NET_BIND_SERVICE` capability.
6. Select the protocol. This example uses **HTTP**.
7. Click **Create**.
8. Check that the port appears in the list with the state **Listening**. Another state names the reason, for example **Address In Use** or **Permission Denied**.

## Step 4: Create a Proxy

1. Click **Proxies** in the menu.
2. Click **Add**.
3. Name the proxy, for example `My Website`. The name can stay empty.
4. Select the protocol **HTTP / HTTPS**.
5. Select the port of Step 3. A proxy can use several ports.
6. Leave **Virtual Hosts** empty, so the proxy matches every host. A value like `example.com` matches that host only.
7. Write the address of your application in **Target**, for example `http://127.0.0.1:3000`.
8. Click **Create**.
9. Check that the proxy appears in the list. It is active at once.

Send a request to the port of Step 3. The answer comes from your application:

```bash
$ curl -i http://127.0.0.1/
```

A `502 Bad Gateway` means r3v3rs3 reached no upstream server. Check the **Target** address and your application.

## Step 5: Add a Certificate

A public site needs HTTPS. r3v3rs3 orders certificates from an ACME server, for example Let's Encrypt.

1. Bind a second port with the protocol **HTTPS**, for example `443`.
2. Click **Certificates** in the menu, then the **ACME** tab, then **Add**.
3. Select the provider, write your email address and the domain names.
4. Select the challenge. **HTTP-01** needs a port `80` that the internet reaches. **DNS-01** needs no open port and issues wildcard certificates.
5. Click **Create**. The order runs at once and then every day.
6. Add the HTTPS port to your proxy and write the domain name in **Virtual Hosts**.

[Certificates](@/configuration.md#certificates) and [ACME](@/configuration.md#acme) describe every field.

## Next Steps

- [Configuration](@/configuration.md): routing, load balancing, caching, authentication, rate limits and every other proxy setting.
- [Accounts](@/accounts.md): roles, proxy lists and the audit log.
- [Service Discovery](@/discovery.md): proxies from Docker labels, Kubernetes, Consul and etcd.
- [High Availability](@/tutorials/high-availability.md): several nodes with one shared state.
