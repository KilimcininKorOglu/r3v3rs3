+++
title = "r3v3rs3"
sort_by = "weight"
+++

[![Crates.io](https://img.shields.io/crates/v/r3v3rs3.svg)](https://crates.io/crates/r3v3rs3)
[![GitHub license](https://img.shields.io/github/license/KilimcininKorOglu/r3v3rs3.svg)](https://github.com/KilimcininKorOglu/r3v3rs3/blob/main/LICENSE)
[![Rust](https://github.com/KilimcininKorOglu/r3v3rs3/actions/workflows/rust.yml/badge.svg)](https://github.com/KilimcininKorOglu/r3v3rs3/actions/workflows/rust.yml)
[![dependency status](https://deps.rs/crate/r3v3rs3/latest/status.svg)](https://deps.rs/crate/r3v3rs3)

# Temel özellikler

- Performans ve güvenlik için Rust ile yazıldı; [tokio](https://tokio.rs/) ve [hyper](https://hyper.rs/) üzerine kurulu.
- TCP, UDP, TLS, HTTP1 ve HTTP2 destekler. HTTP upgrade ve WebSocket bağlantıları da buna dahildir.
- HTTP/3 desteği kısmidir: yalnız gelen QUIC bağlantıları kabul edilir, WebTransport desteği yoktur.
- WebUI ile birlikte tek bir binary olarak gelir, kurulumu kolaydır.
- REST API ile yapılan config değişiklikleri servisi yeniden başlatmadan uygulanır.
- TLS sertifikaları arayüzden içe aktarılabilir veya self-signed sertifika oluşturulabilir.
- Let's Encrypt ile sertifikalar otomatik alınır (ACME v2, HTTP-01 ve DNS-01 challenge'ları). Wildcard sertifikalar için Cloudflare, Route 53, DigitalOcean ve Hetzner Cloud DNS API'leri kullanılır.
- Proxy'leri Docker container label'larından, Kubernetes Ingress kaynaklarından, Consul servis tag'lerinden, Consul ve etcd key-value store'larından oluşturur. Kaynak değişince proxy'leri günceller ([Servis keşfi](@/discovery.tr.md)).
- Birden fazla node'u etcd veya Consul'da paylaşılan tek bir state ile çalıştırır. Değerler şifrelenir, ACME işlerini leader node yapar, session'lar, rate limit ve cache node'lar arasında paylaşılır ([Cluster](@/cluster.tr.md)).

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
  -p 80:80 \
  -p 443:443 \
  -p 127.0.0.1:46492:46492 \
  --restart unless-stopped \
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

## Github Releases

Linux için hazır binary'lerin (x86_64 ve aarch64) son sürümünü doğrudan [releases sayfasından](https://github.com/KilimcininKorOglu/r3v3rs3/releases) da indirebilirsiniz.

Arşivden çıkan binary'yi `$PATH` içindeki bir dizine koymanız yeterlidir.

# Geliştirme

Ayrıntılar için [Geliştirme](@/development.tr.md) bölümüne bakın.

# İlk kurulum

Yönetim paneline erişmek için önce bir kullanıcı oluşturmanız gerekir. Komut sizden bir parola ister. Parola en az 8 karakter olmalıdır.

```bash
# Create a user
$ r3v3rs3 add-user admin
$ password?: ******
```

İki faktörlü doğrulama için `--totp` flag'i ile TOTP'yi açabilirsiniz.

```bash
# Create a user with TOTP enabled
$ r3v3rs3 add-user admin --totp
$ password?: ******

Use this code to setup your TOTP client:
EXAMPLECODEEXAMPLECODE
```

Ardından sunucuyu başlatın.

```bash
$ r3v3rs3 start
```

Sunucu çalışınca yönetim paneline [http://localhost:46492/](http://localhost:46492/) adresinden erişebilirsiniz.

> Sunucu uzak bir makinede çalışıyorsa yönetim paneline SSH port forwarding ile bağlanmanızı öneririz. Panel bağlantısı şifrelenmemiş düz HTTP kullanır. SSH port forwarding ile panelin portunu internete açmanız gerekmez. İsterseniz paneli daha sonra r3v3rs3 üzerinden HTTPS ile de sunabilirsiniz.

# Başlangıç rehberi

Bu rehberde yönetim panelinin kendisi için bir proxy oluşturacağız.

## 1. Giriş yapın

Daha önce oluşturduğunuz kullanıcıyla yönetim paneline giriş yapın.

## 2. Port bağlayın

Proxy oluşturmadan önce dinlenecek bir port bağlamanız gerekir. Bunu "Portlar" bölümünden yapabilirsiniz.

1. Menüde "Portlar" linkine tıklayın.
2. "Ekle" butonuna tıklayın.
3. Porta bir ad verin (örneğin "Web Sitem"). Bu alanı boş bırakabilirsiniz.
4. Dinlenecek network interface'i seçin.
5. Dinlenecek portu seçin. Portu başka bir programın kullanmadığından ve bu portu bağlama izniniz olduğundan emin olun.
6. Protokolü seçin. Bu örnekte "HTTP" kullanacağız.
7. "Oluştur" butonuna tıklayın.
8. Yeni portun listede göründüğünü ve durumunun "Dinliyor" olduğunu kontrol edin.

## 3. Proxy oluşturun

Artık "Proxy'ler" bölümünden bir proxy oluşturabilirsiniz. Bunun için daha önce bağladığınız portu ve hedef URL'i belirtmeniz gerekir.

1. Menüde "Proxy'ler" linkine tıklayın.
2. "Ekle" butonuna tıklayın.
3. Proxy'ye bir ad verin (örneğin "Web Sitem"). Bu alanı boş bırakabilirsiniz.
4. Protokolü seçin. Bu örnekte "HTTP / HTTPS" kullanacağız.
5. Daha önce bağladığınız portu seçin. Birden fazla port da seçebilirsiniz.
6. Bütün host'larla eşleşmesi için "Virtual Host'lar" alanını boş bırakın.
7. "Hedef" alanına hedef URL'i yazın. Bu örnekte yönetim panelinin adresini, "http://localhost:46492" değerini kullanıyoruz.
8. "Oluştur" butonuna tıklayın.
9. Yeni proxy'nin listede göründüğünü kontrol edin. Proxy artık çalışır; yönetim paneline bu proxy üzerinden erişebilirsiniz.

> Proxy'yi internete açmak istiyorsanız firewall ayarlarını da yapmanız gerekebilir.
