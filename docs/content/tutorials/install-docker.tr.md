+++
title = "Docker ile kurulum"
description = "r3v3rs3'ü iki volume ile tek container'da çalıştırın"
weight = 2
+++

# Docker ile kurulum

r3v3rs3'ü tek bir container çalıştırır. İki volume config'i ve veriyi saklar, böylece yükseltme yalnız image'ı değiştirir.

Image `ghcr.io/kilimcininkoroglu/r3v3rs3` adresindedir. Distroless bir image'dır: içinde r3v3rs3 binary'si vardır, shell yoktur. Bu yüzden `docker exec` yalnız `r3v3rs3` komutunu çalıştırır, başka program çalıştırmaz.

## Adım 1: Container'ı başlatın

```bash
$ docker run -d \
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

Her option'ın tek bir işi vardır:

| Option | Nedeni |
|---|---|
| `-v r3v3rs3-config:/root/.config/r3v3rs3` | Hesaplar, proxy'ler, sertifikalar ve ACME kayıtları. Bu olmadan yeni bir container boş başlar. |
| `-v r3v3rs3-data:/root/.local/share/r3v3rs3` | Audit log'u tutan `log.db`. |
| `-p 80:80 -p 443:443` | Proxy'lerinizin kullandığı portlar. Panelde eklediğiniz her portu yayınlayın. |
| `-p 127.0.0.1:46492:46492` | Yalnız host'ta açılan admin paneli. Image `--webui 0.0.0.0:46492` ile başlar, yani container bütün interface'lerde dinler; bu binding onu sınırlar. |
| `--stop-signal SIGINT` | r3v3rs3 SIGINT ile düzgün kapanır. Bu olmadan `docker stop` on saniye bekler, sonra process'i öldürür. |
| `--restart unless-stopped` | Container reboot'tan sonra yeniden başlar. |

## Adım 2: Admin hesabını oluşturun

```bash
$ docker exec -t -i r3v3rs3 r3v3rs3 add-user admin
password?: ******
```

Parola en az 8 karakter olmalıdır. [http://localhost:46492/](http://localhost:46492/) adresini açın ve giriş yapın.

Uzak bir host'ta panelin portunu açmak yerine SSH ile ulaşın:

```bash
$ ssh -L 46492:127.0.0.1:46492 kullanici@sunucunuz
```

## Adım 3: Port ekleyin

Bir proxy porta ihtiyaç duyar, container'ın da o portu yayınlaması gerekir.

1. Panelde bir port ekleyin, örneğin **HTTP** protokolü ile `0.0.0.0:80`.
2. Adım 1'deki `-p 80:80` option'ının o portu yayınladığını kontrol edin.

Container'ın yayınlamadığı bir port yalnız container içinde dinler. Docker çalışan bir container'a yayınlanan port ekleyemez, bu yüzden `docker rm` yapın ve yeni `-p` option'ı ile `docker run` komutunu tekrar çalıştırın. Volume'lar her şeyi korur.

[Başlangıç](@/tutorials/getting-started.tr.md) rehberi ilk proxy ile devam eder.

## Docker Compose

Depoda host networking kullanan bir `docker-compose.yml` vardır:

```bash
$ curl -fsSLO https://raw.githubusercontent.com/KilimcininKorOglu/r3v3rs3/main/docker-compose.yml
$ docker compose up -d
$ docker compose exec r3v3rs3 r3v3rs3 add-user admin
```

Host networking hiçbir port yayınlamaz, panelde eklediğiniz her port host'ta hemen dinler. Yalnız Linux Docker host'unda çalışır. Dosyanın yanındaki bir `.env` dosyasındaki iki değişken kurulumu değiştirir:

- `R3V3RS3_WEBUI`: admin paneli adresi. Varsayılan `127.0.0.1:46492` değeridir.
- `R3V3RS3_VERSION`: image tag'i. Varsayılan `latest` değeridir.

## Deploy platformu

[Deploy platformu](@/platform.tr.md), Git kaynakları için `git` binary'sine, Compose kaynakları için Compose plugin'i olan `docker` binary'sine ihtiyaç duyar. Varsayılan image ikisini de içermez. `-platform` tag son ekli image'ı kullanın, örneğin `latest-platform`. Bu image, `git`, `docker` CLI ve onun Compose ve buildx plugin'lerini içeren bir `debian:trixie-slim` image'ıdır.

```bash
$ sudo mkdir -p /var/lib/r3v3rs3
$ docker run -d \
  --network host \
  -v /var/run/docker.sock:/var/run/docker.sock \
  -v /var/lib/r3v3rs3:/var/lib/r3v3rs3 \
  -e R3V3RS3_CONFIG_DIR=/var/lib/r3v3rs3 \
  -v r3v3rs3-data:/root/.local/share/r3v3rs3 \
  --restart unless-stopped \
  --stop-signal SIGINT \
  --name r3v3rs3 \
  ghcr.io/kilimcininkoroglu/r3v3rs3:latest-platform
```

Komut Adım 1'den üç yerde ayrılır:

| Option | Neden |
|---|---|
| `--network host` | Docker bir uygulamanın portunu host'un `127.0.0.1` adresinde yayınlar ve r3v3rs3 porta orada ulaşır. Bridge network'te `127.0.0.1` container'ın kendisidir. |
| `-v /var/run/docker.sock:/var/run/docker.sock` | Platform, uygulama container'larını host'un Docker Engine'i üzerinden başlatır. Socket'e erişim, host üzerinde root erişimine eşittir. |
| `-v /var/lib/r3v3rs3:/var/lib/r3v3rs3` ve `R3V3RS3_CONFIG_DIR` | Config dizini Compose uygulamalarının checkout'larını tutar. `docker compose` bir bind mount'un yollarını host'un Docker Engine'ine gönderir. Bu yüzden repodaki bir dosyanın bind mount'u yalnız dizinin host'ta ve container'da aynı yolda olduğu durumda çalışır. |

Sonra platformu `/var/lib/r3v3rs3/config.toml` dosyasında açın ve container'ı yeniden başlatın. Uygulamaları sidebar'daki **Platform** grubunun **Uygulamalar** sayfasında yönetin.

## Yükseltme

```bash
$ docker pull ghcr.io/kilimcininkoroglu/r3v3rs3:latest
$ docker rm -f r3v3rs3
$ docker run -d ... # Adım 1'deki aynı komut
```

Compose ile:

```bash
$ docker compose pull
$ docker compose up -d
```

Volume'lar config'i ve hesapları korur, yani hiçbir hesap yeniden oluşturulmaz. Birkaç host'u adım adım yükseltirken sürümü tag ile sabitleyin, örneğin `:1.0.1`.

## Restart ne yapar

- Hesaplar, portlar, proxy'ler ve sertifikalar config volume'ünden geri gelir.
- Session'lar gelmez. r3v3rs3 onları memory'de tutar, bu yüzden her client yeniden giriş yapar.
- Cache'lenen response'lar ve rate limit sayaçları da memory'dedir ve boş başlar.

## Sonraki adımlar

- [Başlangıç](@/tutorials/getting-started.tr.md): ilk port ve ilk proxy.
- [Docker'dan proxy'ler](@/tutorials/docker-discovery.tr.md): diğer container'larınız proxy'lerini label ile tanımlasın.
- [Linux sunucuya kurulum](@/tutorials/install-linux.tr.md): Docker'sız systemd kurulumu.
