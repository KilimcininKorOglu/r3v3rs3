# syntax=docker/dockerfile:1

# The WebUI is WebAssembly, so it is built once on the build platform for every target platform.
FROM --platform=$BUILDPLATFORM rust:1-trixie AS webui
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
WORKDIR /usr/src/app
COPY Cargo.toml Cargo.lock ./
COPY r3v3rs3 r3v3rs3
COPY r3v3rs3-api r3v3rs3-api
COPY r3v3rs3-webui r3v3rs3-webui
WORKDIR /usr/src/app/r3v3rs3-webui
RUN trunk build --cargo-profile web-release --release

FROM rust:1-trixie AS builder
WORKDIR /usr/src/app
COPY Cargo.toml Cargo.lock ./
COPY r3v3rs3 r3v3rs3
COPY r3v3rs3-api r3v3rs3-api
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
