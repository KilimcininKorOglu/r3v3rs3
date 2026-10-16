+++
title = "Docker'dan proxy'ler"
description = "Container'larınız kendi proxy'lerini etiketlerle tanımlasın"
weight = 5
+++

# Docker'dan proxy'ler

r3v3rs3 çalışan container'larınızın etiketlerini okur ve proxy'leri onlardan kurar. Başlayan bir container yaklaşık bir saniye içinde proxy'sini alır, duran bir container proxy'sini kaybeder. Elle hiç proxy eklemezsiniz.

Bu rehber o kurulumu Docker Compose ile yapar ve her adımı doğrular. Birkaç dakika sürer.

## Ne kuruyorsunuz

```
   istemci ---> :80 r3v3rs3 (host'ta) ---> whoami container(ları)
                     |
                     +-- etiketler ve olaylar için Docker socket'ini okur
```

r3v3rs3 host'ta çalışır ve Docker socket'ini okur. Her container'ın etiketini okur ve istekleri container'ın o ağdaki adresine gönderir.

> Docker socket'ine erişim, host üzerinde root erişimine eşittir. r3v3rs3 socket'i çalıştığı kullanıcı olarak okur; install.sh servisi root olarak çalıştırır.

## Adım 1: r3v3rs3'ü host'a kurun

[Linux sunucuya kurulum](@/tutorials/install-linux.tr.md) rehberi binary'yi kurar ve admin hesabını oluşturur. [http://localhost:46492/](http://localhost:46492/) adresini açın ve giriş yapın.

## Adım 2: Compose dosyasını yazın

```yaml
services:
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
```

Etiketler şunu söyler: bu container'ın `whoami` adında bir HTTP proxy'si vardır, r3v3rs3'ün `http` adlı portunu kullanır, `whoami.example.com` host'una yanıt verir ve upstream sunucusu container'ın `80` portunda dinler.

Ağ tanımının altındaki `name: proxy` satırı ağ adını `proxy` olarak bırakır. Onsuz Compose başa proje adını ekler.

Servisi başlatın:

```bash
$ docker compose up -d
```

## Adım 3: Portu bağlayın

Bir servis keşfi sağlayıcısı port açmaz. `ports: http` etiketi, önceden var olması gereken bir portun adını yazar.

1. Menüde **Portlar** linkine, sonra **Ekle** butonuna tıklayın.
2. Ad alanına `http` yazın. Etiket bu adı kullanır.
3. Ağ arayüzü olarak `0.0.0.0`, port olarak `80` ve protokol olarak **HTTP** seçin.
4. **Oluştur** butonuna tıklayın ve **Dinliyor** durumunu kontrol edin.

## Adım 4: Docker sağlayıcısını açın

1. Menüde **Ayarlar** linkine tıklayın.
2. **Docker Servis Keşfi** bölümünü bulun ve açın.
3. **Endpoint** alanına `unix:///var/run/docker.sock` yazın.
4. **Ağ** alanına `proxy` yazın. Bu, Adım 2'deki Docker ağının adıdır.
5. **Her container'ı oku** seçeneğini kapalı bırakın. Böylece r3v3rs3 yalnız `r3v3rs3.enable=true` etiketi olan container'ları okur.
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

**Proxy'ler** sayfası artık kaynağı **Docker** olan `whoami` adlı bir proxy listeler. Proxy'nin düzenleme butonu yoktur, çünkü sahibi container'dır. Sayfa sağlayıcının durumunu da gösterir:

```bash
$ curl -b session.txt http://127.0.0.1:46492/api/discovery
[{"provider":"docker","state":"running","proxies":1,"updated_at":1789642012}]
```

Etiketteki host ile bir istek gönderin:

```bash
$ curl -H 'Host: whoami.example.com' http://127.0.0.1/
Hostname: 307d1b9d215d
IP: 192.168.164.2
...
```

Başka bir host `502` alır, çünkü hiçbir route eşleşmez.

Sağlayıcı socket'i okuyamazsa `state` alanı `error` olur. Bir sorun kaydı kaynağı ve nedeni yazar, örneğin `whoami: http.whoami: port not found: http`. Bu mesaj Adım 3'ün eksik olduğu anlamına gelir.

## Adım 6: Servisi ölçekleyin

```bash
$ docker compose up -d --scale whoami=3
```

Bir saniye içinde proxy'nin her replikaya bir tane olmak üzere üç sunucusu olur:

```bash
$ curl -H 'Host: whoami.example.com' http://127.0.0.1/ | grep Hostname
```

İsteği tekrarlayın. Yanıtlar üç container arasında dönüşümlü gelir, çünkü bir Compose servisinin replikaları proxy'lerini paylaşır ve her replika kendi sunucusunu ekler.

Bir replikayı durdurun, proxy o sunucuyu bırakır. Bütün replikalar durunca proxy kaybolur.

## Yeni container ekleme

Her yeni container'ın kendi etiketleri olur. Protokolden sonraki proxy adı iki container'ı birbirinden ayırır:

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

Yönetim API'sinin proxy modelindeki her alan etiket olarak çalışır. [Etiketler](@/discovery.tr.md#etiketler) bölümü key'leri, değer biçimlerini, TCP ve UDP proxy'lerini listeler. r3v3rs3 düz metin parolayı ve token'ı kullanmadan önce hash'e çevirir, bu yüzden etikette `password_hash` ve `token_hash` tercih edin.

## Sertifikalar

Etiketten gelen bir HTTP proxy sertifikasını kendiliğinden alabilir. Önce bir ACME kaydı oluşturun, sonra id'sini etikete yazın:

```yaml
      r3v3rs3.http.api.acme: e7k-2np
```

r3v3rs3 proxy'nin `vhosts` değeri için sertifika ister. Kuralları [ACME sertifikaları](@/discovery.tr.md#acme-sertifikalari) bölümü anlatır.

## Sonraki adımlar

- [Servis keşfi](@/discovery.tr.md): bütün sağlayıcılar, bütün etiket key'leri ve Kubernetes, Consul, etcd kaynakları.
- [Yapılandırma](@/configuration.tr.md): her proxy alanının ne yaptığı.
- [Yüksek erişilebilirlik](@/tutorials/high-availability.tr.md): tek bir durum bilgisini paylaşan birkaç düğüm.
