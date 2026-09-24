+++
title = "Docker ile kurulum"
description = "r3v3rs3'ü iki volume ile tek container'da çalıştırın"
weight = 2
+++

# Docker ile kurulum

r3v3rs3'ü tek bir container çalıştırır. İki volume yapılandırmayı ve veriyi saklar, böylece yükseltme yalnız image'ı değiştirir.

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

Her seçeneğin tek bir işi vardır:

| Seçenek | Nedeni |
|---|---|
| `-v r3v3rs3-config:/root/.config/r3v3rs3` | Hesaplar, proxy'ler, sertifikalar ve ACME kayıtları. Bu olmadan yeni bir container boş başlar. |
| `-v r3v3rs3-data:/root/.local/share/r3v3rs3` | Denetim kaydını tutan `log.db`. |
| `-p 80:80 -p 443:443` | Proxy'lerinizin kullandığı portlar. Panelde eklediğiniz her portu yayınlayın. |
| `-p 127.0.0.1:46492:46492` | Yalnız host'ta açılan yönetim paneli. Image `--webui 0.0.0.0:46492` ile başlar, yani container bütün ağ arayüzlerinde dinler; bu port eşlemesi onu sınırlar. |
| `--stop-signal SIGINT` | r3v3rs3 SIGINT ile düzgün kapanır. Bu olmadan `docker stop` on saniye bekler, sonra süreci öldürür. |
| `--restart unless-stopped` | Makine yeniden başladıktan sonra container da yeniden başlar. |

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
2. Adım 1'deki `-p 80:80` seçeneğinin o portu yayınladığını kontrol edin.

Container'ın yayınlamadığı bir port yalnız container içinde dinler. Docker çalışan bir container'a yayınlanan port ekleyemez, bu yüzden `docker rm` yapın ve yeni `-p` seçeneği ile `docker run` komutunu tekrar çalıştırın. Volume'lar her şeyi korur.

[Başlangıç](@/tutorials/getting-started.tr.md) rehberi ilk proxy ile devam eder.

## Docker Compose

Repository'de host ağ modunu kullanan bir `docker-compose.yml` vardır:

```bash
$ curl -fsSLO https://raw.githubusercontent.com/KilimcininKorOglu/r3v3rs3/main/docker-compose.yml
$ docker compose up -d
$ docker compose exec r3v3rs3 r3v3rs3 add-user admin
```

Host ağ modu hiçbir port yayınlamaz, panelde eklediğiniz her port host'ta hemen dinler. Yalnız Linux Docker host'unda çalışır. Dosyanın yanındaki bir `.env` dosyasındaki iki değişken kurulumu değiştirir:

- `R3V3RS3_WEBUI`: yönetim panelinin adresi. Varsayılan `127.0.0.1:46492` değeridir.
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
  --entrypoint /usr/bin/r3v3rs3 \
  ghcr.io/kilimcininkoroglu/r3v3rs3:latest-platform \
  start --webui 127.0.0.1:46492
```

Komut Adım 1'den dört yerde ayrılır:

| Seçenek | Neden |
|---|---|
| `--network host` | Docker bir uygulamanın portunu host'un `127.0.0.1` adresinde yayınlar ve r3v3rs3 porta orada ulaşır. Bridge ağında `127.0.0.1` container'ın kendisidir. |
| `-v /var/run/docker.sock:/var/run/docker.sock` | Platform, uygulama container'larını host'un Docker Engine'i üzerinden başlatır. Socket'e erişim, host üzerinde root erişimine eşittir. |
| `-v /var/lib/r3v3rs3:/var/lib/r3v3rs3` ve `R3V3RS3_CONFIG_DIR` | Config dizini Compose uygulamalarının checkout'larını tutar. `docker compose` bir bind mount'un yollarını host'un Docker Engine'ine gönderir. Bu yüzden repository'deki bir dosyanın bind mount'u yalnız dizinin host'ta ve container'da aynı yolda olduğu durumda çalışır. |
| `--entrypoint /usr/bin/r3v3rs3` ve `start --webui 127.0.0.1:46492` | Yönetim panelini host'un `127.0.0.1` adresinde tutar. 1.5.2 ve önceki platform image'ları `--webui 0.0.0.0:46492` ile başlar. Host ağ modunda bu adres paneli host'un bütün ağ arayüzlerinde açar. |

Sonra `/var/lib/r3v3rs3/config.toml` dosyasına `enabled = true` ve `proxy_ports = ["<http portu>", "<https portu>"]` değerleriyle bir `[platform]` bölümü yazın ve container'ı yeniden başlatın. Uygulamalar route'larını yalnız `proxy_ports` içindeki portlarda alır. Uygulamaları kenar çubuğundaki **Platform** grubunun **Uygulamalar** sayfasında yönetin.

### Agent

Bir [agent hedefi](@/platform.tr.md#agent-hedefleri), uygulamaları başka bir sunucuda çalıştırır. Master'da `[platform]` bölümüne `agent_port` ekleyin. Host ağ modunda bu port host üzerinde dinler. Hedefi **Hedefler** sayfasında ekleyin, sonra diğer sunucuda agent'ı sayfanın gösterdiği `docker run` komutuyla başlatın:

```bash
$ docker run -d --name r3v3rs3-agent --restart unless-stopped --network host --stop-signal SIGINT \
  -v /var/run/docker.sock:/var/run/docker.sock \
  -v /var/lib/r3v3rs3-agent:/var/lib/r3v3rs3-agent \
  --entrypoint /usr/bin/r3v3rs3 \
  ghcr.io/kilimcininkoroglu/r3v3rs3:latest-platform \
  agent --master master.example.com:9443 --data-dir /var/lib/r3v3rs3-agent --token <token>
```

Agent, yukarıdaki tablodaki nedenlerle host ağ modunu, Docker socket'ini ve aynı yoldaki veri dizinini ister. Token yalnız ilk başlangıçta gerekir: agent'ın key'i ve sertifikası `/var/lib/r3v3rs3-agent` dizininde kalır. Bu yüzden aynı dizini kullanan yeni bir container token olmadan bağlanır.

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

Volume'lar yapılandırmayı ve hesapları korur, yani hiçbir hesap yeniden oluşturulmaz. Birkaç host'u adım adım yükseltirken sürümü tag ile sabitleyin, örneğin `:v1.5.2`.

## Yeniden başlatma ne yapar

- Hesaplar, portlar, proxy'ler ve sertifikalar yapılandırma volume'ünden geri gelir.
- Oturumlar geri gelmez. r3v3rs3 onları bellekte tutar, bu yüzden her istemci yeniden giriş yapar.
- Cache'teki yanıtlar ve rate limit sayaçları da bellektedir ve boş başlar.

## Sonraki adımlar

- [Başlangıç](@/tutorials/getting-started.tr.md): ilk port ve ilk proxy.
- [Docker'dan proxy'ler](@/tutorials/docker-discovery.tr.md): diğer container'larınız proxy'lerini etiketlerle tanımlasın.
- [Linux sunucuya kurulum](@/tutorials/install-linux.tr.md): Docker'sız systemd kurulumu.
