+++
title = "Development"
description = "Development"
weight = 0
+++

Our project's source code is available on [GitHub](https://github.com/KilimcininKorOglu/r3v3rs3).

# Prerequisites

Before getting started, make sure to install the following prerequisites:

- Rust toolchain: You can install it using [rustup.rs](https://rustup.rs/)
- WASM toolchain: After installing the Rust toolchain, add the WASM target by executing `rustup target add wasm32-unknown-unknown` in your terminal
- [Trunk](https://trunkrs.dev/): Visit the website for installation instructions

# Development Setup

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

# Building for Release

```bash
# Build the WebUI
cd r3v3rs3/r3v3rs3-webui
trunk build --cargo-profile web-release --release

# Build the Server
cd ..
cargo build --release

# Start the server
target/release/r3v3rs3 start
```

# Gitpod

You can instantly start developing r3v3rs3 in your browser using Gitpod.

[![Open in Gitpod](https://gitpod.io/button/open-in-gitpod.svg)](https://gitpod.io/#https://github.com/KilimcininKorOglu/r3v3rs3)
