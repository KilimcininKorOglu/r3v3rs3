+++
title = "r3v3rs3"
sort_by = "weight"
+++

[![Crates.io](https://img.shields.io/crates/v/r3v3rs3.svg)](https://crates.io/crates/r3v3rs3)
[![GitHub license](https://img.shields.io/github/license/KilimcininKorOglu/r3v3rs3.svg)](https://github.com/KilimcininKorOglu/r3v3rs3/blob/main/LICENSE)
[![Rust](https://github.com/KilimcininKorOglu/r3v3rs3/actions/workflows/rust.yml/badge.svg)](https://github.com/KilimcininKorOglu/r3v3rs3/actions/workflows/rust.yml)
[![dependency status](https://deps.rs/crate/r3v3rs3/latest/status.svg)](https://deps.rs/crate/r3v3rs3)

# Temel özellikler

## Proxy

- TCP, UDP, TLS, HTTP/1.1 ve HTTP/2 proxy'leri. HTTP upgrade ve WebSocket bağlantıları da buna dahildir. Rust ile [tokio](https://tokio.rs/) ve [hyper](https://hyper.rs/) üzerine yazıldı.
- HTTP/3 desteği kısmidir: yalnız gelen QUIC bağlantıları kabul edilir. Upstream bağlantıları HTTP/2 veya HTTP/1.1 kullanır. WebTransport desteği yoktur.
- Host adına (tam, wildcard veya regex) ve path'e göre routing. Path rewrite, redirect kuralları ve redirect host ya da 404 host gibi fixed response'lar.
- Load balancing, aktif ve pasif health check, circuit breaker, sticky session, retry, upstream timeout ve traffic mirroring.
- DNS SRV kayıtlarından gelen upstream sunucuları (`http+srv://` URL'leri). TTL bitince yenilenir.
- Gelen bağlantılarda ve upstream sunuculara giden bağlantılarda PROXY protocol.

## Güvenlik ve trafik kontrolü

- IP allow ve deny listeleri, istemci başına rate limit, request body boyut limiti. Bilinen CDN'lerin ve güvenilen proxy'lerin arkasında gerçek istemci IP'si bulunur.
- Basic, Bearer, forward ve yönetim paneli session authentication. Proxy'ler arasında paylaşılan erişim listeleri.
- Request ve response header kuralları, response sıkıştırma (brotli, zstd ve gzip) ve bellekte tutulan HTTP cache.

## Sertifikalar

- Yüklenen veya self-signed server, client ve root sertifikaları.
- Mutual TLS: TLS portlarında client sertifikası doğrulanır, upstream sunuculara client sertifikası gönderilir.
- HTTP-01, TLS-ALPN-01 ve DNS-01 challenge'larıyla ACME v2 (örneğin Let's Encrypt). DNS-01, wildcard sertifikaları 12 DNS provider API'si, webhook, exec komutu veya RFC 2136 ile alır.
- Sertifika süresi uyarıları ve webhook bildirimleri.

## Yönetim

- WebUI'ı içinde taşıyan tek bir binary. WebUI İngilizce ve Türkçedir. Config değişiklikleri yeniden başlatmadan uygulanır.
- OpenAPI dokümanı ve Swagger UI sunan yönetim API'si ([Yönetim API'si](@/configuration.tr.md#yonetim-api-si)).
- `admin`, `editor` ve `viewer` rolleri olan hesaplar, hesap başına proxy listesi ve audit log ([Hesaplar](@/accounts.tr.md)).
- Docker label'ları, Kubernetes Ingress ve `R3v3rs3Proxy` kaynakları, Consul ve etcd ile servis keşfi ([Servis keşfi](@/discovery.tr.md)).
- Yüksek erişilebilirlik: birden fazla node, etcd veya Consul'da şifreli tek bir state paylaşır ([Kurulum rehberi](@/tutorials/high-availability.tr.md), [Cluster](@/cluster.tr.md)).

# Kurulum

r3v3rs3'ü birkaç yolla kurabilirsiniz.

## Linux sunucu

`install.sh`, son release binary'sini (x86_64 veya aarch64) sha256 kontrolüyle kurar, admin hesabını oluşturur ve r3v3rs3'ü systemd servisi olarak çalıştırır:

```bash
curl -fsSL https://raw.githubusercontent.com/KilimcininKorOglu/r3v3rs3/main/install.sh | sudo bash
```

Script, yönetim paneli WebUI adresini sorar. Varsayılan adres `127.0.0.1:46492`. Belirli bir release kurmak veya soruyu atlamak için seçenekleri verin:

```bash
curl -fsSL https://raw.githubusercontent.com/KilimcininKorOglu/r3v3rs3/main/install.sh | sudo bash -s -- --version 1.0.1 --webui 0.0.0.0:46492
```

Config `/etc/r3v3rs3`, log dosyaları `/var/log/r3v3rs3` dizinindedir. Güncellemek için script'i yeniden çalıştırın.

## Docker

r3v3rs3'ü Docker ile başlatmak için şu komutu çalıştırın:

```bash
docker run -d \
  -v r3v3rs3-config:/root/.config/r3v3rs3 \
  -v r3v3rs3-data:/root/.local/share/r3v3rs3 \
  -p 80:80 \
  -p 443:443 \
  -p 127.0.0.1:46492:46492 \
  --restart unless-stopped \
  --stop-signal SIGINT \
  --name r3v3rs3 \
  ghcr.io/kilimcininkoroglu/r3v3rs3:latest
```

Yönetim paneline giriş yapmak için önce bir kullanıcı oluşturmanız gerekir. Admin kullanıcısını şu komutla oluşturun:

```bash
docker exec -t -i r3v3rs3 r3v3rs3 add-user admin
password?: ******
```

## Docker Compose

[`docker-compose.yml`](https://github.com/KilimcininKorOglu/r3v3rs3/blob/main/docker-compose.yml) dosyasını indirip r3v3rs3'ü başlatın:

```bash
$ curl -fsSLO https://raw.githubusercontent.com/KilimcininKorOglu/r3v3rs3/main/docker-compose.yml
$ docker compose up -d
```

Dosya host networking kullanır. Bu yüzden WebUI'da eklediğiniz her port, dosyayı değiştirmeden dinlenir. Host networking yalnız Linux Docker host'unda çalışır. Şu değişkenleri dosyanın yanındaki `.env` dosyasında ayarlayın:

- `R3V3RS3_WEBUI`: yönetim paneli WebUI adresi. Varsayılan `127.0.0.1:46492`.
- `R3V3RS3_VERSION`: image tag'i. Varsayılan `latest`.

Yönetim paneline giriş yapmak için önce bir kullanıcı oluşturmanız gerekir. Admin kullanıcısını şu komutla oluşturun:

```bash
$ docker compose exec r3v3rs3 r3v3rs3 add-user admin
password?: ******
```

Ardından yönetim paneline [http://localhost:46492/](http://localhost:46492/) adresinden erişebilirsiniz.

## Cargo binstall

[cargo-binstall](https://github.com/cargo-bins/), platformunuza uygun hazır binary'yi indirip kurar. Hazır binary yoksa `cargo install` kullanır.

Önce [cargo-binstall](https://github.com/cargo-bins/cargo-binstall#installation) kurulu olmalıdır.

Ardından r3v3rs3'ü şu komutla kurabilirsiniz:

```bash
$ cargo binstall r3v3rs3
```

## Cargo install

Rust toolchain kurulu olmalıdır. Kurulu değilse [rustup.rs](https://rustup.rs/) adresindeki talimatları izleyin.

crates.io paketinde WebUI statik asset olarak hazır gelir. Bu yüzden WebUI'ı kendiniz build etmeniz gerekmez; bunun için [trunk](https://trunkrs.dev/) ve wasm toolchain gerekirdi.

```bash
$ cargo install r3v3rs3
```

## GitHub Releases

Linux için hazır binary'lerin (x86_64 ve aarch64) son sürümünü doğrudan [releases sayfasından](https://github.com/KilimcininKorOglu/r3v3rs3/releases) da indirebilirsiniz.

Arşivden çıkan binary'yi `$PATH` içindeki bir dizine koymanız yeterlidir.

# Geliştirme

Ayrıntılar için [Geliştirme](@/development.tr.md) bölümüne bakın.

# İlk kurulum

Admin hesabı oluşturmak, sunucuyu başlatmak ve ilk proxy'nizi kurmak için [Başlangıç](@/tutorials/getting-started.tr.md) rehberini izleyin.
