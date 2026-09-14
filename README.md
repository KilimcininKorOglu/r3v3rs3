<div align="center">
<img alt="edition logo" src="https://github.com/KilimcininKorOglu/r3v3rs3/blob/main/logo.svg?raw=true" width="150" />

# r3v3rs3

A reverse proxy server with built-in WebUI, supporting TCP/UDP/HTTP/TLS/WebSocket, written in Rust.

[![Crates.io](https://img.shields.io/crates/v/r3v3rs3.svg)](https://crates.io/crates/r3v3rs3)
[![GitHub license](https://img.shields.io/github/license/KilimcininKorOglu/r3v3rs3.svg)](https://github.com/KilimcininKorOglu/r3v3rs3/blob/main/LICENSE)
[![Rust](https://github.com/KilimcininKorOglu/r3v3rs3/actions/workflows/rust.yml/badge.svg)](https://github.com/KilimcininKorOglu/r3v3rs3/actions/workflows/rust.yml)
[![dependency status](https://deps.rs/crate/r3v3rs3/latest/status.svg)](https://deps.rs/crate/r3v3rs3)

</div>

## Notice

r3v3rs3 is currently in early development. Please be aware that breaking changes may occur frequently, particularly when upgrading between minor versions (e.g., from 0.3.x to 0.4.x).

## Overview

- Built with Rust for optimal performance and safety, powered by tokio and hyper
- Supports TCP, UDP, TLS, HTTP1, and HTTP2, including HTTP upgrading and WebSocket functionality
- Partial HTTP/3 support (incoming QUIC connections only; WebTransport not supported)
- Easily deployable single binary with a built-in WebUI
- Allows live configuration updates via a REST API without restarting the service
- Imports TLS certificates from the GUI or can generate a self-signed certificate
- Supports mutual TLS: verifies client certificates on TLS ports and sends a client certificate to upstream servers
- Provides Let's Encrypt support (ACME v2 with the HTTP-01 and DNS-01 challenges) for seamless certificate provisioning, including wildcard certificates through the Cloudflare, Route 53, DigitalOcean and Hetzner Cloud DNS APIs
- Discovers proxies from Docker container labels, Kubernetes Ingress resources, Consul service tags and the Consul and etcd key-value stores, and updates them when the source changes

## Documentation

- [English](https://kilimcininkoroglu.github.io/r3v3rs3/)
- [Türkçe](https://kilimcininkoroglu.github.io/r3v3rs3/tr/)

## Screenshot

![r3v3rs3 WebUI Screenshot](https://raw.githubusercontent.com/KilimcininKorOglu/r3v3rs3/main/screenshot.png)

## Installation

There are multiple ways to install r3v3rs3.

## Docker

Run the following command to start r3v3rs3 using Docker:

```bash
docker run -d \
  -v r3v3rs3-config:/root/.config/r3v3rs3 \
  -p 80:80 \
  -p 443:443 \
  -p 127.0.0.1:46492:46492 \
  --restart unless-stopped \
  --name r3v3rs3 \
  ghcr.io/kilimcininkoroglu/r3v3rs3:latest
```

To log in to the admin panel, you'll first need to create a user. Follow the steps below to create an admin user:

```bash
docker exec -t -i r3v3rs3 r3v3rs3 add-user admin
password?: ******
```

### Docker Compose

Create a file named `docker-compose.yml` with the following content:

```yaml
version: "3"
services:
  r3v3rs3:
    image: ghcr.io/kilimcininkoroglu/r3v3rs3:latest
    container_name: r3v3rs3
    volumes:
      - r3v3rs3-config:/root/.config/r3v3rs3
      # Uncomment to discover proxies from Docker labels
      # - /var/run/docker.sock:/var/run/docker.sock:ro
    ports:
      # Add ports here if you want to expose them to the host
      - 80:80
      - 443:443
      - 127.0.0.1:46492:46492 # Admin panel
    restart: unless-stopped

volumes:
  r3v3rs3-config:
```

Run the following command to start r3v3rs3:

```bash
$ docker-compose up -d
```

To log in to the admin panel, you'll first need to create a user. Follow the steps below to create an admin user:

```bash
$ docker-compose exec r3v3rs3 r3v3rs3 add-user admin
password?: ******
```

Then, you can access the admin panel at [http://localhost:46492/](http://localhost:46492/).

### Cargo binstall

[cargo-binstall](https://github.com/cargo-bins/) automatically downloads and installs pre-built binaries for your platform. If there is no pre-built binary available, it will fall back to `cargo install`.

You need to install [cargo-binstall](https://github.com/cargo-bins/cargo-binstall#installation) first.

Then you can install r3v3rs3 with:

```bash
$ cargo binstall r3v3rs3
```

### Cargo install

You need to have the Rust toolchain installed. If you don't, please follow the instructions on [rustup.rs](https://rustup.rs/).

The package on crates.io comes bundled with the WebUI as a static asset. Thus, you don't need to build it yourself (which would require [trunk](https://trunkrs.dev/) and wasm toolchain).

```bash
$ cargo install r3v3rs3
```

### Github Releases

Alternatively, you can directly download the latest pre-built binaries from the [releases page](https://github.com/KilimcininKorOglu/r3v3rs3/releases).

You simply put the extracted binary somewhere in your `$PATH` and you're good to go.

## Starting the server

First, you need to create a user to access the admin panel. You will be prompted for a password.

```bash
# Create a user
$ r3v3rs3 add-user admin
$ password?: ******
```

Then, you can start the server.

```bash
$ r3v3rs3 start
```

Once the server is running, you can access the admin panel at [http://localhost:46492/](http://localhost:46492/).

The admin API has an OpenAPI document at `/api/openapi.json` and a Swagger UI at `/api/docs/`. Both require a signed-in session. See [Admin API](https://kilimcininkoroglu.github.io/r3v3rs3/configuration/#admin-api) in the documentation.

## Development

To contribute or develop r3v3rs3, follow these steps:

```bash
# Clone the repository
git clone https://github.com/KilimcininKorOglu/r3v3rs3

# Start the server
cd r3v3rs3
cargo run

# In a separate terminal, start `trunk serve` for the WebUI
cd r3v3rs3-webui
trunk serve
```

### Gitpod

You can instantly start developing r3v3rs3 in your browser using Gitpod.

[![Open in Gitpod](https://gitpod.io/button/open-in-gitpod.svg)](https://gitpod.io/#https://github.com/KilimcininKorOglu/r3v3rs3)

## Similar projects

HTTP reverse proxies written in Rust:

- [Sōzu](https://github.com/sozu-proxy/sozu)
- [rpxy](https://github.com/junkurihara/rust-rpxy)

## Credit

r3v3rs3 is based on [Taxy](https://github.com/picoHz/taxy) by picoHz.

The social preview image uses the photo by [cal gao](https://unsplash.com/@ginnta?utm_source=unsplash&utm_medium=referral&utm_content=creditCopyText) on [Unsplash](https://unsplash.com/photos/MASpFp0X2VU?utm_source=unsplash&utm_medium=referral&utm_content=creditCopyText).
