+++
title = "r3v3rs3"
sort_by = "weight"
+++

[![Crates.io](https://img.shields.io/crates/v/r3v3rs3.svg)](https://crates.io/crates/r3v3rs3)
[![GitHub license](https://img.shields.io/github/license/KilimcininKorOglu/r3v3rs3.svg)](https://github.com/KilimcininKorOglu/r3v3rs3/blob/main/LICENSE)
[![Rust](https://github.com/KilimcininKorOglu/r3v3rs3/actions/workflows/rust.yml/badge.svg)](https://github.com/KilimcininKorOglu/r3v3rs3/actions/workflows/rust.yml)
[![dependency status](https://deps.rs/crate/r3v3rs3/latest/status.svg)](https://deps.rs/crate/r3v3rs3)

# Temel Özellikler

- Performans ve güvenlik için Rust ile yazılmıştır. [tokio](https://tokio.rs/) ve [hyper](https://hyper.rs/) üzerine kuruludur.
- TCP, UDP, TLS, HTTP1 ve HTTP2 destekler. HTTP upgrade ve WebSocket de desteklenir.
- HTTP/3 desteği kısmidir (yalnız gelen QUIC bağlantıları; WebTransport desteklenmez).
- Dahili WebUI'a sahip tek bir binary olarak kolayca kurulur.
- REST API ile yapılan config değişikliklerini servisi yeniden başlatmadan uygular.
- TLS sertifikalarını arayüzden içe aktarır veya self-signed sertifika oluşturur.
- Sertifikaları otomatik almak için Let's Encrypt desteği sunar (ACME v2, yalnız HTTP challenge).

# Kurulum

r3v3rs3 birkaç yolla kurulabilir.

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

Yönetim paneline giriş yapmak için önce bir kullanıcı oluşturmanız gerekir. Admin kullanıcısı oluşturmak için şu komutu çalıştırın:

```bash
docker exec -t -i r3v3rs3 r3v3rs3 add-user admin
password?: ******
```

## Docker Compose

Şu içerikle `docker-compose.yml` adında bir dosya oluşturun:

```yaml
version: "3"
services:
  r3v3rs3:
    image: ghcr.io/kilimcininkoroglu/r3v3rs3:latest
    container_name: r3v3rs3
    volumes:
      - r3v3rs3-config:/root/.config/r3v3rs3
    ports:
      # Add ports here if you want to expose them to the host
      - 80:80
      - 443:443
      - 127.0.0.1:46492:46492 # Admin panel
    restart: unless-stopped

volumes:
  r3v3rs3-config:
```

r3v3rs3'ü başlatmak için şu komutu çalıştırın:

```bash
$ docker-compose up -d
```

Yönetim paneline giriş yapmak için önce bir kullanıcı oluşturmanız gerekir. Admin kullanıcısı oluşturmak için şu komutu çalıştırın:

```bash
$ docker-compose exec r3v3rs3 r3v3rs3 add-user admin
password?: ******
```

Ardından yönetim paneline [http://localhost:46492/](http://localhost:46492/) adresinden erişebilirsiniz.

## Cargo binstall

[cargo-binstall](https://github.com/cargo-bins/) platformunuz için hazır binary'leri otomatik olarak indirir ve kurar. Hazır binary yoksa `cargo install` kullanır.

Önce [cargo-binstall](https://github.com/cargo-bins/cargo-binstall#installation) kurmanız gerekir.

Ardından r3v3rs3'ü şu komutla kurabilirsiniz:

```bash
$ cargo binstall r3v3rs3
```

## Cargo install

Rust toolchain kurulu olmalıdır. Kurulu değilse [rustup.rs](https://rustup.rs/) adresindeki talimatları izleyin.

crates.io paketi WebUI'ı statik bir asset olarak içerir. Bu yüzden WebUI'ı kendiniz build etmeniz gerekmez (bunun için [trunk](https://trunkrs.dev/) ve wasm toolchain gerekir).

```bash
$ cargo install r3v3rs3
```

## Github Releases

En son hazır binary'leri doğrudan [releases sayfasından](https://github.com/KilimcininKorOglu/r3v3rs3/releases) da indirebilirsiniz.

Arşivden çıkan binary'yi `$PATH` içindeki bir dizine koymanız yeterlidir.

# Geliştirme

Ayrıntılar için [Geliştirme](@/development.tr.md) bölümüne bakın.

# İlk Kurulum

Yönetim paneline erişmek için önce bir kullanıcı oluşturmanız gerekir. Komut sizden bir parola ister.

```bash
# Create a user
$ r3v3rs3 add-user admin
$ password?: ******
```

İki faktörlü doğrulama için TOTP kullanmak istiyorsanız `--totp` flag'i ile TOTP'yi açabilirsiniz.

```bash
# Create a user with TOTP enabled
$ r3v3rs3 add-user admin --totp
$ password?: ******

Use this code to setup your TOTP client:
EXAMPLECODEEXAMPLECODE
```

Ardından sunucuyu başlatabilirsiniz.

```bash
$ r3v3rs3 start
```

Sunucu çalışınca yönetim paneline [http://localhost:46492/](http://localhost:46492/) adresinden erişebilirsiniz.

> Sunucu uzak bir makinede çalışıyorsa yönetim panelinin güvenliği için SSH port forwarding kullanmanızı önemle öneririz. Panel bağlantısı şifrelenmemiş düz HTTP kullanır ve bu yöntem panelin portunu internete açmaz. Paneli daha sonra r3v3rs3 ile HTTPS üzerinden de sunabilirsiniz.

# Başlangıç Rehberi

Bu rehberde yönetim panelinin kendisi için bir proxy oluşturacağız.

## 1. Giriş yapın

Daha önce oluşturduğunuz kullanıcı ile yönetim paneline giriş yapın.

## 2. Port bağlayın

Proxy oluşturmadan önce dinlenecek bir port bağlamanız gerekir. Bunu "Portlar" bölümünde yapabilirsiniz.

1. Menüde "Portlar" linkine tıklayın.
2. "Ekle" butonuna tıklayın.
3. Porta bir ad verin (örneğin "Web Sitem"). Bu alanı boş bırakabilirsiniz.
4. Dinlenecek network interface'i seçin.
5. Dinlenecek portu seçin. Portun kullanımda olmadığından ve portu bağlama izniniz olduğundan emin olun.
6. Protokolü seçin. Bu örnekte "HTTP" kullanacağız.
7. "Oluştur" butonuna tıklayın.
8. Oluşturulan portun listede göründüğünü ve durumunun "Dinliyor" olduğunu kontrol edin.

## 3. Proxy oluşturun

Artık "Proxy'ler" bölümünde bir proxy oluşturabilirsiniz. Daha önce bağladığınız portu ve hedef URL'i belirtmeniz gerekir.

1. Menüde "Proxy'ler" linkine tıklayın.
2. "Ekle" butonuna tıklayın.
3. Proxy'ye bir ad verin (örneğin "Web Sitem"). Bu alanı boş bırakabilirsiniz.
4. Protokolü seçin. Bu örnekte "HTTP / HTTPS" kullanacağız.
5. Daha önce bağladığınız portu seçin. İsterseniz birden fazla port seçebilirsiniz.
6. Bütün host'larla eşleşmesi için "Virtual Host'lar" alanını boş bırakın.
7. Hedef URL'i girin. Bu örnekte yönetim panelinin adresi olan "http://localhost:46492" kullanılır. URL'i "Hedef" alanına yazın.
8. "Oluştur" butonuna tıklayın.
9. Oluşturulan proxy'nin listede göründüğünü kontrol edin. Proxy artık aktiftir ve yönetim paneline proxy üzerinden erişebilirsiniz.

> Proxy'yi internete açmak istiyorsanız firewall ayarlarını yapmanız gerekebilir.
