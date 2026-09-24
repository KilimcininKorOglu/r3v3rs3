+++
title = "r3v3rs3"
sort_by = "weight"
+++

[![Crates.io](https://img.shields.io/crates/v/r3v3rs3.svg)](https://crates.io/crates/r3v3rs3)
[![GitHub license](https://img.shields.io/github/license/KilimcininKorOglu/r3v3rs3.svg)](https://github.com/KilimcininKorOglu/r3v3rs3/blob/main/LICENSE)
[![Rust](https://github.com/KilimcininKorOglu/r3v3rs3/actions/workflows/rust.yml/badge.svg)](https://github.com/KilimcininKorOglu/r3v3rs3/actions/workflows/rust.yml)
[![dependency status](https://deps.rs/crate/r3v3rs3/latest/status.svg)](https://deps.rs/crate/r3v3rs3)

r3v3rs3, Rust ile yazılmış bir reverse proxy sunucusudur. TCP, UDP, TLS, HTTP ve WebSocket bağlantılarını proxy'ler, gelen HTTP/3 bağlantılarını da kabul eder. [Taxy](https://github.com/picoHz/taxy) projesinin bir fork'udur.

Ayarları tarayıcıdan yaparsınız. Tek bir binary hem proxy'yi hem de İngilizce ve Türkçe WebUI'yi taşır; her port, proxy, sertifika ve hesap bu WebUI'de bir formdur. Değişiklik yeniden başlatmadan uygulanır. Aynı işlemler yönetim API'si üzerinden de yapılır. r3v3rs3 proxy'lerini Docker etiketlerinden, Kubernetes Ingress'ten, Consul catalog'undan veya etcd key'lerinden de oluşturabilir.

# Temel özellikler

## Proxy

- TCP, UDP, TLS, HTTP/1.1 ve HTTP/2 proxy'leri. HTTP upgrade ve WebSocket bağlantıları da buna dahildir. Rust ile [tokio](https://tokio.rs/) ve [hyper](https://hyper.rs/) üzerine yazıldı.
- HTTP/3 desteği kısmidir: yalnız gelen QUIC bağlantıları kabul edilir. Upstream bağlantıları HTTP/2 veya HTTP/1.1 kullanır. WebTransport desteği yoktur.
- Host adına (tam, wildcard veya regex) ve yola göre routing. Yol yeniden yazma, yönlendirme kuralları ve yönlendirme host'u ya da 404 host'u gibi sabit yanıtlar.
- Yük dengeleme, aktif ve pasif sağlık kontrolü, circuit breaker, sticky session, yeniden deneme, upstream timeout ve istek yansıtma.
- DNS SRV kayıtlarından gelen upstream sunucuları (`http+srv://` URL'leri). TTL bitince yenilenir.
- Gelen bağlantılarda ve upstream sunuculara giden bağlantılarda PROXY protocol.

## Güvenlik ve istek kontrolü

- IP izin ve engelleme listeleri, istemci başına rate limit, istek gövdesi boyut limiti. Bilinen CDN'lerin ve güvenilen proxy'lerin arkasında gerçek istemci IP'si bulunur.
- Basic, Bearer, forward ve yönetim paneli oturumu ile kimlik doğrulama. Proxy'ler arasında paylaşılan erişim listeleri.
- İstek ve yanıt header kuralları, yanıt sıkıştırma (brotli, zstd ve gzip) ve bellekte tutulan HTTP cache.

## Sertifikalar

- Yüklenen veya kendinden imzalı sunucu, istemci ve kök sertifikaları.
- Mutual TLS: TLS portlarında istemci sertifikası doğrulanır, upstream sunuculara istemci sertifikası gönderilir.
- HTTP-01, TLS-ALPN-01 ve DNS-01 challenge'larıyla ACME v2 (örneğin Let's Encrypt). DNS-01, wildcard sertifikaları 12 DNS sağlayıcısının API'si, webhook, exec komutu veya RFC 2136 ile alır.
- Sertifika süresi uyarıları ve webhook bildirimleri.

## Yönetim

- WebUI'ı içinde taşıyan tek bir binary. WebUI İngilizce ve Türkçedir. Ayar değişiklikleri yeniden başlatmadan uygulanır.
- OpenAPI dokümanı ve Swagger UI sunan yönetim API'si ([Yönetim API'si](@/configuration.tr.md#yonetim-api-si)).
- `admin`, `editor` ve `viewer` rolleri olan hesaplar, hesap başına proxy listesi ve denetim kaydı ([Hesaplar](@/accounts.tr.md)).
- Docker etiketleri, Kubernetes Ingress ve `R3v3rs3Proxy` kaynakları, Consul ve etcd ile servis keşfi ([Servis keşfi](@/discovery.tr.md)).
- Yüksek erişilebilirlik: birden fazla düğüm, etcd veya Consul'da şifreli tek bir durum bilgisini paylaşır ([Kurulum rehberi](@/tutorials/high-availability.tr.md), [Cluster](@/cluster.tr.md)).

# Kurulum

r3v3rs3'ü birkaç yolla kurabilirsiniz.

## Linux sunucu

`install.sh`, son release binary'sini (x86_64 veya aarch64) sha256 kontrolüyle kurar, admin hesabını oluşturur ve r3v3rs3'ü systemd servisi olarak çalıştırır:

```bash
curl -fsSL https://raw.githubusercontent.com/KilimcininKorOglu/r3v3rs3/main/install.sh | sudo bash
```

[Linux sunucuya kurulum](@/tutorials/install-linux.tr.md) rehberi seçenekleri, yolları, yükseltmeyi ve kaldırmayı anlatır.

## Docker

İki volume ile tek container:

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

[Docker ile kurulum](@/tutorials/install-docker.tr.md) rehberi her seçeneği, admin hesabını, Docker Compose'u ve yükseltmeyi anlatır.

## Cargo binstall

[cargo-binstall](https://github.com/cargo-bins/), platformunuza uygun hazır binary'yi indirip kurar. Hazır binary yoksa `cargo install` kullanır.

Önce [cargo-binstall](https://github.com/cargo-bins/cargo-binstall#installation) kurulu olmalıdır.

Ardından r3v3rs3'ü şu komutla kurabilirsiniz:

```bash
$ cargo binstall r3v3rs3
```

## Cargo install

Rust toolchain kurulu olmalıdır. Kurulu değilse [rustup.rs](https://rustup.rs/) adresindeki talimatları izleyin.

crates.io paketinde WebUI statik dosyalar olarak hazır gelir. Bu yüzden WebUI'ı kendiniz build etmeniz gerekmez; bunun için [trunk](https://trunkrs.dev/) ve wasm toolchain gerekirdi.

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
