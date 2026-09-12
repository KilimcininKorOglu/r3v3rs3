# Use the official Rust image as the base image for the builder stage
FROM rust:latest as builder

# Install trunk
RUN cargo install trunk
RUN rustup target add wasm32-unknown-unknown

# Set the working directory
WORKDIR /usr/src/app

# Copy the actual source code
COPY Cargo.toml Cargo.lock ./
COPY r3v3rs3 r3v3rs3
COPY r3v3rs3-api r3v3rs3-api
COPY r3v3rs3-webui r3v3rs3-webui

# Build the web UI
WORKDIR /usr/src/app/r3v3rs3-webui
RUN trunk build --cargo-profile web-release --release
WORKDIR /usr/src/app

# Build the Rust project
RUN cargo build --release

# Prepare the final image
FROM debian:bookworm-slim as runtime

# Install dependencies for the Rust binary
RUN apt-get update && \
    apt-get install -y --no-install-recommends \
    ca-certificates && \
    rm -rf /var/lib/apt/lists/*

# Set the working directory
WORKDIR /app

# Copy the Rust binary from the builder stage
COPY --from=builder /usr/src/app/target/release/r3v3rs3 /usr/bin

# Set the entrypoint to run the Rust binary
ENTRYPOINT ["r3v3rs3", "start", "--webui", "0.0.0.0:46492"]
