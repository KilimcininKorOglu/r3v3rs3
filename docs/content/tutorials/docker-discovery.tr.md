+++
title = "Docker'dan proxy'ler"
description = "Container'larınız kendi proxy'lerini label ile tanımlasın"
weight = 5
+++

# Docker'dan proxy'ler

r3v3rs3 çalışan container'larınızın label'larını okur ve proxy'leri onlardan kurar. Başlayan bir container yaklaşık bir saniye içinde proxy'sini alır, duran bir container proxy'sini kaybeder. Elle hiç proxy eklemezsiniz.

Bu rehber o kurulumu Docker Compose ile yapar ve her adımı doğrular. Birkaç dakika sürer.

## Ne kuruyorsunuz

```
   client ---> :80 r3v3rs3 ---> whoami container(ları)
                    |
                    +-- label ve event için Docker socket'ini okur
```

r3v3rs3 ile container'lar aynı Docker network'ünü paylaşır. r3v3rs3 her container'ın label'ını okur ve trafiği container'ın o network'teki adresine gönderir.

> Docker socket'ine erişim, host üzerinde root erişimine eşittir. `:ro` seçeneği yalnız socket dosyası içindir, API'yi read-only yapmaz. Host güvenilmeyen bir network'ten erişilebiliyorsa önüne yalnız `GET /containers/json` ve `GET /events` isteklerine izin veren bir Docker socket proxy'si koyun.

## Adım 1: Compose dosyasını yazın

```yaml
services:
  r3v3rs3:
    image: ghcr.io/kilimcininkoroglu/r3v3rs3:latest
    environment:
      R3V3RS3_WEBUI: 0.0.0.0:46492
    volumes:
      - r3v3rs3-config:/root/.config/r3v3rs3
      - r3v3rs3-data:/root/.local/share/r3v3rs3
      - /var/run/docker.sock:/var/run/docker.sock:ro
    networks: [proxy]
    ports:
      - "80:80"
      - "127.0.0.1:46492:46492"
    stop_signal: SIGINT

  whoami:
    image: traefik/whoami
    networks: [proxy]
    labels:
      r3v3rs3.enable: "true"
      r3v3rs3.http.whoami.ports: http
      r3v3rs3.http.whoami.vhosts: whoami.example.com
      r3v3rs3.http.whoami.port: "80"

networks:
  proxy:
    name: proxy

volumes:
  r3v3rs3-config:
  r3v3rs3-data:
```

Label'lar şunu söyler: bu container'ın `whoami` adında bir HTTP proxy'si vardır, r3v3rs3'ün `http` adlı portunu kullanır, `whoami.example.com` host'una cevap verir ve upstream sunucusu container'ın `80` portunda dinler.

Network altındaki `name: proxy` satırı network adını `proxy` olarak bırakır. Onsuz Compose başa proje adını ekler.

Stack'i başlatın:

```bash
$ docker compose up -d
```

## Adım 2: Admin hesabını oluşturun

```bash
$ docker compose exec r3v3rs3 r3v3rs3 add-user admin
```

[http://localhost:46492/](http://localhost:46492/) adresini açın ve giriş yapın.

## Adım 3: Portu bağlayın

Bir discovery sağlayıcısı port açmaz. `ports: http` label'ı, önceden var olması gereken bir portun adını yazar.

1. Menüde **Portlar** linkine, sonra **Ekle** butonuna tıklayın.
2. Ad alanına `http` yazın. Label bu adı kullanır.
3. Interface olarak `0.0.0.0`, port olarak `80` ve protokol olarak **HTTP** seçin.
4. **Oluştur** butonuna tıklayın ve **Dinliyor** durumunu kontrol edin.

## Adım 4: Docker sağlayıcısını açın

1. Menüde **Ayarlar** linkine tıklayın.
2. **Docker Service Discovery** bölümünü bulun ve açın.
3. Endpoint alanına `unix:///var/run/docker.sock` yazın.
4. Network alanına `proxy` yazın. Bu, Adım 1'deki Docker network'ünün adıdır.
5. **Expose containers by default** seçeneğini kapalı bırakın. Böylece r3v3rs3 yalnız `r3v3rs3.enable=true` label'ı olan container'ları okur.
6. Kaydedin.

Aynı ayarlar `config.toml` içinde:

```toml
[discovery.docker]
enabled = true
endpoint = "unix:///var/run/docker.sock"
network = "proxy"
exposed_by_default = false
```

## Adım 5: Sonucu kontrol edin

**Proxy'ler** sayfası artık kaynağı `docker` olan `whoami` adlı bir proxy listeler. Proxy'nin düzenleme butonu yoktur, çünkü sahibi container'dır. Sayfa sağlayıcının durumunu da gösterir:

```bash
$ curl -b session.txt http://127.0.0.1:46492/api/discovery
[{"provider":"docker","state":"running","proxies":1,"updated_at":1789642012}]
```

Label'daki host ile bir request gönderin:

```bash
$ curl -H 'Host: whoami.example.com' http://127.0.0.1/
Hostname: 307d1b9d215d
IP: 192.168.164.2
...
```

Başka bir host `502` alır, çünkü hiçbir route eşleşmez.

Sağlayıcı socket'i okuyamazsa `state` alanı `error` olur. Bir issue kaynağı ve nedeni yazar, örneğin `whoami: http.whoami: port not found: http`. Bu mesaj Adım 3'ün eksik olduğu anlamına gelir.

## Adım 6: Servisi ölçekleyin

```bash
$ docker compose up -d --scale whoami=3
```

Bir saniye içinde proxy'nin her replikaya bir tane olmak üzere üç sunucusu olur:

```bash
$ curl -H 'Host: whoami.example.com' http://127.0.0.1/ | grep Hostname
```

Request'i tekrarlayın. Cevaplar üç container arasında dönüşümlü gelir, çünkü bir Compose servisinin replikaları proxy'lerini paylaşır ve her replika kendi sunucusunu ekler.

Bir replikayı durdurun, proxy o sunucuyu bırakır. Bütün replikalar durunca proxy kaybolur.

## Yeni container ekleme

Her yeni container'ın kendi label'ları olur. Protokolden sonraki proxy adı iki container'ı birbirinden ayırır:

```yaml
  api:
    image: my/api
    networks: [proxy]
    labels:
      r3v3rs3.enable: "true"
      r3v3rs3.http.api.ports: http
      r3v3rs3.http.api.vhosts: api.example.com
      r3v3rs3.http.api.port: "3000"
      r3v3rs3.http.api.rate_limit.requests: "100"
      r3v3rs3.http.api.rate_limit.per: minute
```

Admin API proxy modelinin her alanı label olarak çalışır. [Label'lar](@/discovery.tr.md) sayfası key'leri, değer biçimlerini, TCP ve UDP proxy'lerini listeler. r3v3rs3 düz metin parolayı ve token'ı kullanmadan önce hash'e çevirir, bu yüzden label'da `password_hash` ve `token_hash` tercih edin.

## Sertifikalar

Label'dan gelen bir HTTP proxy sertifikasını kendiliğinden alabilir. Önce bir ACME kaydı oluşturun, sonra id'sini label'da yazın:

```yaml
      r3v3rs3.http.api.acme: e7k-2np
```

r3v3rs3 proxy'nin `vhosts` değeri için sertifika ister. Kuralları [Servis keşfi](@/discovery.tr.md) sayfasının ACME bölümü anlatır.

## Sonraki adımlar

- [Servis keşfi](@/discovery.tr.md): bütün sağlayıcılar, bütün label key'leri ve Kubernetes, Consul, etcd kaynakları.
- [Config](@/configuration.tr.md): her proxy alanının ne yaptığı.
- [Yüksek erişilebilirlik](@/tutorials/high-availability.tr.md): tek bir state paylaşan birkaç node.
