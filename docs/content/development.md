+++
title = "Development"
description = "Development"
weight = 0
+++

Our project's source code is available on [GitHub](https://github.com/KilimcininKorOglu/r3v3rs3).

# Prerequisites

Before getting started, make sure to install the following prerequisites:

- Rust toolchain: install [rustup](https://rustup.rs/). `rust-toolchain.toml` pins the compiler, clippy, rustfmt and the `wasm32-unknown-unknown` target, and rustup installs them at the first build.
- [Trunk](https://trunkrs.dev/): Visit the website for installation instructions

# Development Setup

```bash
# Clone the repository
git clone https://github.com/KilimcininKorOglu/r3v3rs3
cd r3v3rs3

# Create an admin account
cargo run --bin r3v3rs3 -- add-user admin

# Start the server
make run

# In a separate terminal, start `trunk serve` for the WebUI
cd r3v3rs3-webui
trunk serve
```

`trunk serve` sends the requests under `/api/` to the server on `localhost:46492`.

# Tests and Checks

- `make test` runs the tests of the server and the API types. CI runs `cargo nextest run --all-features --no-fail-fast`.
- `make lint` runs clippy with `-D warnings`, and `make fmt-check` checks the formatting.
- CI also checks that every function stays at or below a cyclomatic complexity of 10 with `lizard -l rust -C 10 -w r3v3rs3/src r3v3rs3-api/src r3v3rs3-webui/src`.
- `make test-runtime-docker`, `make test-acme-pebble`, `make test-discovery-e2e` and `make test-cluster-e2e` run the ignored tests against a Docker Engine, Pebble, Consul, etcd and k3s.
- `make check` runs the format check, clippy, the tests and the WebUI build.

# Building for Release

```bash
make release

# Start the server
target/release/r3v3rs3 start
```

`make release` builds the WebUI into `r3v3rs3/dist/webui` and then the server. The server embeds that directory at compile time, so the WebUI is built first.

# Gitpod

You can instantly start developing r3v3rs3 in your browser using Gitpod.

[![Open in Gitpod](https://gitpod.io/button/open-in-gitpod.svg)](https://gitpod.io/#https://github.com/KilimcininKorOglu/r3v3rs3)
