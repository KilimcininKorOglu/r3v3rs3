# syntax=docker/dockerfile:1

# Both Rust stages use the compiler of `channel` in rust-toolchain.toml. Raise the tags with it.
FROM --platform=$BUILDPLATFORM rust:1.98.1-trixie AS chef
ARG BUILDARCH
ARG CARGO_CHEF_VERSION=0.1.71
RUN set -eu; \
    case "$BUILDARCH" in \
      amd64) triple=x86_64-unknown-linux-gnu ;; \
      arm64) triple=aarch64-unknown-linux-gnu ;; \
      *) echo "unsupported build platform: $BUILDARCH" >&2; exit 1 ;; \
    esac; \
    url="https://github.com/LukeMathWalker/cargo-chef/releases/download/v${CARGO_CHEF_VERSION}/cargo-chef-${triple}.tar.gz"; \
    curl -fsSL "$url" | tar -xz -C /usr/local/bin cargo-chef
WORKDIR /usr/src/app

FROM --platform=$BUILDPLATFORM chef AS planner
COPY Cargo.toml Cargo.lock ./
COPY r3v3rs3 r3v3rs3
COPY r3v3rs3-api r3v3rs3-api
COPY r3v3rs3-cli r3v3rs3-cli
COPY r3v3rs3-webui r3v3rs3-webui
RUN cargo chef prepare --recipe-path recipe.json

# The WebUI is WebAssembly, so it is built once on the build platform for every target platform.
FROM --platform=$BUILDPLATFORM chef AS webui
ARG BUILDARCH
ARG TRUNK_VERSION=0.21.14
RUN set -eu; \
    case "$BUILDARCH" in \
      amd64) triple=x86_64-unknown-linux-gnu ;; \
      arm64) triple=aarch64-unknown-linux-gnu ;; \
      *) echo "unsupported build platform: $BUILDARCH" >&2; exit 1 ;; \
    esac; \
    base="https://github.com/trunk-rs/trunk/releases/download/v${TRUNK_VERSION}/trunk-${triple}.tar.gz"; \
    curl -fsSLo /tmp/trunk.tar.gz "$base"; \
    echo "$(curl -fsSL "$base.sha256" | awk '{print $1}')  /tmp/trunk.tar.gz" | sha256sum -c -; \
    tar -xzf /tmp/trunk.tar.gz -C /usr/local/bin trunk; \
    rm /tmp/trunk.tar.gz
RUN rustup target add wasm32-unknown-unknown
COPY --from=planner /usr/src/app/recipe.json recipe.json
RUN cargo chef cook --profile web-release --target wasm32-unknown-unknown --package r3v3rs3-webui --recipe-path recipe.json
COPY Cargo.toml Cargo.lock ./
COPY r3v3rs3 r3v3rs3
COPY r3v3rs3-api r3v3rs3-api
COPY r3v3rs3-cli r3v3rs3-cli
COPY r3v3rs3-webui r3v3rs3-webui
WORKDIR /usr/src/app/r3v3rs3-webui
RUN trunk build --cargo-profile web-release --release

FROM rust:1.98.1-trixie AS builder
ARG TARGETARCH
ARG CARGO_CHEF_VERSION=0.1.71
# aws-lc-sys builds its C sources with cmake and perl, and bindgen needs libclang.
RUN apt-get update \
    && apt-get install -y --no-install-recommends cmake perl libclang-dev \
    && rm -rf /var/lib/apt/lists/*
RUN set -eu; \
    case "$TARGETARCH" in \
      amd64) triple=x86_64-unknown-linux-gnu ;; \
      arm64) triple=aarch64-unknown-linux-gnu ;; \
      *) echo "unsupported target platform: $TARGETARCH" >&2; exit 1 ;; \
    esac; \
    url="https://github.com/LukeMathWalker/cargo-chef/releases/download/v${CARGO_CHEF_VERSION}/cargo-chef-${triple}.tar.gz"; \
    curl -fsSL "$url" | tar -xz -C /usr/local/bin cargo-chef
WORKDIR /usr/src/app
COPY --from=planner /usr/src/app/recipe.json recipe.json
RUN cargo chef cook --release --package r3v3rs3 --recipe-path recipe.json
COPY Cargo.toml Cargo.lock ./
COPY r3v3rs3 r3v3rs3
COPY r3v3rs3-api r3v3rs3-api
COPY r3v3rs3-cli r3v3rs3-cli
COPY r3v3rs3-webui r3v3rs3-webui
COPY --from=webui /usr/src/app/r3v3rs3/dist/webui r3v3rs3/dist/webui
RUN cargo build --release --locked -p r3v3rs3

FROM docker:29.8.1-cli AS docker-cli

# The platform image adds git for the Git sources of the deployment platform, and the static docker
# CLI with the Compose plugin for its Compose sources. The buildx plugin lets Compose build with
# BuildKit, which a Dockerfile with `RUN --mount` needs.
FROM debian:trixie-slim AS platform
RUN apt-get update \
    && apt-get install -y --no-install-recommends ca-certificates git \
    && rm -rf /var/lib/apt/lists/*
COPY --from=docker-cli /usr/local/bin/docker /usr/local/bin/docker
COPY --from=docker-cli /usr/local/libexec/docker/cli-plugins/docker-compose /usr/local/libexec/docker/cli-plugins/docker-buildx /usr/local/libexec/docker/cli-plugins/
COPY --from=builder /usr/src/app/target/release/r3v3rs3 /usr/bin/r3v3rs3
# The platform image runs with host networking, so the WebUI keeps its loopback default.
ENTRYPOINT ["/usr/bin/r3v3rs3", "start"]

# distroless/cc holds glibc, libgcc and the CA certificates the binary needs, and nothing else.
FROM gcr.io/distroless/cc-debian13 AS runtime
COPY --from=builder /usr/src/app/target/release/r3v3rs3 /usr/bin/r3v3rs3
ENTRYPOINT ["/usr/bin/r3v3rs3", "start", "--webui", "0.0.0.0:46492"]
