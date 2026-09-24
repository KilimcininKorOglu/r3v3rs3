+++
title = "Geliştirme"
description = "Geliştirme"
weight = 0
+++

Projenin kaynak kodu [GitHub](https://github.com/KilimcininKorOglu/r3v3rs3) üzerindedir.

# Gereksinimler

Başlamadan önce şunları kurun:

- Rust toolchain: [rustup](https://rustup.rs/) kurun. `rust-toolchain.toml` derleyiciyi, clippy'yi, rustfmt'yi ve `wasm32-unknown-unknown` hedefini sabitler. rustup bunları ilk build'de kurar.
- [Trunk](https://trunkrs.dev/): Kurulum talimatları web sitesinde bulunur.

# Geliştirme ortamı

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

`trunk serve`, `/api/` altındaki istekleri `localhost:46492` üzerindeki sunucuya gönderir.

# Testler ve kontroller

- `make test` sunucunun ve API tiplerinin testlerini çalıştırır. CI `cargo nextest run --all-features --no-fail-fast` komutunu çalıştırır.
- `make lint` clippy'yi `-D warnings` ile çalıştırır. `make fmt-check` biçimlendirmeyi kontrol eder.
- CI ayrıca her fonksiyonun cyclomatic complexity değerinin 10 veya altında kaldığını `lizard -l rust -C 10 -w r3v3rs3/src r3v3rs3-api/src r3v3rs3-webui/src` ile kontrol eder.
- `make test-runtime-docker`, `make test-acme-pebble`, `make test-discovery-e2e` ve `make test-cluster-e2e` yok sayılan testleri Docker Engine, Pebble, Consul, etcd ve k3s üzerinde çalıştırır.
- `make check` biçim kontrolünü, clippy'yi, testleri ve WebUI build'ini çalıştırır.

# Release build

```bash
make release

# Start the server
target/release/r3v3rs3 start
```

`make release` önce WebUI'ı `r3v3rs3/dist/webui` dizinine, sonra sunucuyu build eder. Sunucu bu dizini derleme sırasında içine gömer. Bu yüzden WebUI önce build edilir.

# Gitpod

Gitpod ile r3v3rs3'ü doğrudan tarayıcıda geliştirebilirsiniz.

[![Open in Gitpod](https://gitpod.io/button/open-in-gitpod.svg)](https://gitpod.io/#https://github.com/KilimcininKorOglu/r3v3rs3)
