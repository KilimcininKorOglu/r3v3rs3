+++
title = "Geliştirme"
description = "Geliştirme"
weight = 0
+++

Projenin kaynak kodu [GitHub](https://github.com/KilimcininKorOglu/r3v3rs3) üzerindedir.

# Gereksinimler

Başlamadan önce şunları kurun:

- Rust toolchain: [rustup.rs](https://rustup.rs/) ile kurabilirsiniz.
- WASM toolchain: Rust toolchain kurulduktan sonra terminalde `rustup target add wasm32-unknown-unknown` komutunu çalıştırarak WASM target'ını ekleyin.
- [Trunk](https://trunkrs.dev/): Kurulum talimatları için web sitesine bakın.

# Geliştirme Ortamı

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

# Release Build

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

r3v3rs3'ü Gitpod ile doğrudan tarayıcınızda geliştirmeye başlayabilirsiniz.

[![Open in Gitpod](https://gitpod.io/button/open-in-gitpod.svg)](https://gitpod.io/#https://github.com/KilimcininKorOglu/r3v3rs3)
