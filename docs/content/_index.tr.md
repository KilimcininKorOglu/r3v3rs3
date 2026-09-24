+++
title = "r3v3rs3"
sort_by = "weight"
+++

[![Crates.io](https://img.shields.io/crates/v/r3v3rs3.svg)](https://crates.io/crates/r3v3rs3)
[![GitHub license](https://img.shields.io/github/license/KilimcininKorOglu/r3v3rs3.svg)](https://github.com/KilimcininKorOglu/r3v3rs3/blob/main/LICENSE)
[![Rust](https://github.com/KilimcininKorOglu/r3v3rs3/actions/workflows/rust.yml/badge.svg)](https://github.com/KilimcininKorOglu/r3v3rs3/actions/workflows/rust.yml)
[![dependency status](https://deps.rs/crate/r3v3rs3/latest/status.svg)](https://deps.rs/crate/r3v3rs3)

r3v3rs3, Rust ile yazılmış bir reverse proxy sunucusudur. TCP, UDP, TLS, HTTP ve WebSocket bağlantılarını proxy'ler, gelen HTTP/3 bağlantılarını da kabul eder. Uygulamaları container olarak deploy eder ve alan adlarını kendi proxy'leri üzerinden yönlendirir. [Taxy](https://github.com/picoHz/taxy) projesinin bir fork'udur.

Ayarları tarayıcıdan yaparsınız. Tek bir binary hem proxy'yi hem de İngilizce ve Türkçe WebUI'yi taşır; her port, proxy, sertifika ve hesap bu WebUI'de bir formdur. Değişiklik yeniden başlatmadan uygulanır. Aynı işlemler yönetim API'si üzerinden de yapılır. r3v3rs3 proxy'lerini Docker etiketlerinden, Kubernetes Ingress'ten, Consul catalog'undan veya etcd key'lerinden de oluşturabilir.

# Temel özellikler

## Proxy

- TCP, UDP, TLS, HTTP/1.1 ve HTTP/2 proxy'leri. HTTP upgrade ve WebSocket bağlantıları da buna dahildir.
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
- Cluster modu ile yüksek erişilebilirlik: birden fazla düğüm, etcd veya Consul'da şifreli tek bir durum bilgisini paylaşır ([Kurulum rehberi](@/tutorials/high-availability.tr.md), [Cluster](@/cluster.tr.md)).

## Deploy platformu

- Registry image'ından, Dockerfile'ı olan bir Git repository'sinden veya bir Git repository'sindeki Docker Compose dosyasından uygulamalar.
- Proxy'yi ancak yeni container sağlık kontrolünden geçince değiştiren blue-green deployment'lar ve önceki bir deployment'a geri alma.
- r3v3rs3 uygulamaların alan adlarını kendisi yönlendirir ve sertifikalarını ACME ile alır.
- Şifreli ortam değişkenleri. Her uygulamanın deployment'ları ve container log'u WebUI'da ve yönetim API'sinde görünür.
- Git sağlayıcı bağlantıları: r3v3rs3'ün bir manifest'ten oluşturduğu GitHub App (GitHub Enterprise Server'da da) ve GitLab ile Gitea için OAuth uygulamaları. Bağlantı repository'leri ve branch'leri listeler, private repository'leri clone eder ve push webhook'unu kurar.
- GitHub, GitLab, Gitea, Forgejo veya genel bir HMAC göndericisinden gelen push webhook'ları deployment başlatır.
- Agent hedefleri uygulamaları başka sunucularda çalıştırır. Agent, tek seferlik kayıttan sonra master'a mTLS ile bağlanır ([Deploy platformu](@/platform.tr.md)).

# Kurulum

```bash
curl -fsSL https://raw.githubusercontent.com/KilimcininKorOglu/r3v3rs3/main/install.sh | sudo bash
```

[Linux sunucuya kurulum](@/tutorials/install-linux.tr.md) rehberi seçenekleri, yolları, yükseltmeyi, kaldırmayı ve diğer kurulum yollarını anlatır: cargo-binstall, `cargo install` ve release arşivleri. [Docker ile kurulum](@/tutorials/install-docker.tr.md) rehberi container'ı, Docker Compose'u ve deploy platformunun image'ını anlatır.

# Geliştirme

Ayrıntılar için [Geliştirme](@/development.tr.md) bölümüne bakın.

# İlk kurulum

Admin hesabı oluşturmak, sunucuyu başlatmak ve ilk proxy'nizi kurmak için [Başlangıç](@/tutorials/getting-started.tr.md) rehberini izleyin.
