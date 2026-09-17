+++
title = "TCP ve UDP proxy'ler"
description = "Bir veritabanını TCP, bir DNS sunucusunu UDP üzerinden yayınlayın, mutual TLS ile"
weight = 9
+++

# TCP ve UDP proxy'ler

r3v3rs3 yalnız HTTP'yi taşımaz. Bu rehber bir PostgreSQL sunucusunu TCP, bir DNS sunucusunu UDP üzerinden yayınlar, sonra veritabanının önüne mutual TLS koyar. Böylece yalnız sertifikası olan bir client bağlanır.

TCP veya UDP proxy byte taşır. Request okumaz, bu yüzden virtual host'u, route'u, path'i ve header kuralları yoktur.

Önce [Başlangıç](@/tutorials/getting-started.tr.md) rehberini izleyin.

## Adım 1: PostgreSQL'i TCP üzerinden yayınlayın

1. Menüde **Portlar** linkine, sonra **Ekle** butonuna tıklayın.
2. Ad alanına `postgres` yazın, interface'i ve `5432` portunu seçin, protokol olarak **TCP** seçin.
3. **Oluştur** butonuna tıklayın ve **Dinliyor** durumunu kontrol edin.
4. **Proxy'ler** linkine, sonra **Ekle** butonuna tıklayın ve protokol olarak **TCP / TLS üzerinden TCP** seçin.
5. `postgres` portunu seçin.
6. **Upstream Sunucu** alanına veritabanının adresini yazın, örneğin `/ip4/10.0.0.5/tcp/5432`.
7. **Oluştur** butonuna tıklayın.

```bash
$ psql "postgresql://postgres:<parola>@proxy.example.com:5432/postgres" -c 'select version();'
 PostgreSQL 17.11 on aarch64-unknown-linux-musl, ...
```

Adres bir multiaddr'dır: `/ip4/<adres>/tcp/<port>`, `/ip6/<adres>/tcp/<port>` veya `/dns/<ad>/tcp/<port>`. `/dns` ile r3v3rs3 adı her bağlantıda çözer.

Bir TCP portunu aynı anda tek bir TCP proxy kullanır. Aynı porttaki ikinci bir TCP proxy hatadır.

## Adım 2: Bir DNS sunucusunu UDP üzerinden yayınlayın

1. Protokolü **UDP** olan bir port ekleyin, örneğin `53` portunda.
2. Protokolü **UDP** olan bir proxy ekleyin, portu seçin ve upstream adresi olarak `/ip4/10.0.0.53/udp/53` yazın.

```bash
$ dig +short @proxy.example.com example.com A
172.66.147.243
```

UDP proxy her client adresi için bir session açar, her session'ın upstream sunucuya kendi socket'i vardır. İki yönde de **Session Boşta Kalma Timeout'u (Saniye)** boyunca paket geçmezse session kapanır. Varsayılan 60 saniyedir. Bir port en çok 10.000 session tutar, dolduğunda yeni client'ların paketlerini atar.

## Adım 3: Mutual TLS ekleyin

TLS üzerinden TCP portu TLS'i sonlandırır ve upstream sunucuya düz byte gönderir. Client authentication ile yalnız sizin CA'nızın sertifikasını taşıyan client bağlanır.

### Sertifikaları oluşturun

1. **Sertifikalar** linkine, sonra **Sunucu Sertifikaları** sekmesine, sonra **Self-sign** butonuna tıklayın.
2. Subject name olarak proxy'nin host adını yazın, örneğin `db.example.com`, ve oluşturun. r3v3rs3 **Root Sertifikaları** sekmesinde bir CA sertifikası da oluşturur.
3. **Client Sertifikaları** sekmesini açın, **Self-sign** butonuna tıklayın, **Sertifika Türü** olarak **Client Sertifikası** ve bir önceki adımdaki CA'yı seçin, sonra oluşturun.
4. Client sertifikasını **İndir** ile indirin. Arşiv `chain.pem` ve `key.pem` dosyalarını taşır. İkisini de client'a verin.

### Portu bağlayın

1. Protokolü **TLS üzerinden TCP** olan bir port ekleyin.
2. **Server Name'ler** alanına host adını yazın. r3v3rs3 sunucu sertifikasını client'ın SNI değerinden seçer.
3. **Client Authentication** alanında **Zorunlu** seçin.
4. **Client CA Sertifikaları** alanında CA sertifikasını seçin. Sistem root sertifikaları kullanılmaz.
5. Proxy'yi bu portta, Adım 1'deki upstream sunucuyla oluşturun.

Sertifikası olmayan bir client bağlantı kuramaz. Mesaj client'ın TLS kütüphanesine göre değişir:

```bash
$ curl --cacert ca.pem https://db.example.com:5432/
curl: (56) LibreSSL SSL_read: LibreSSL/3.3.6: error:1404C45C:SSL routines:ST_OK:reason(1116), errno 0

$ curl --cacert ca.pem --cert chain.pem --key key.pem https://db.example.com:5432/ -o /dev/null -w '%{http_code}\n'
200
```

**İsteğe bağlı** mod sertifikası olmayan client'ı kabul eder, sertifika gönderen client'ın sertifikasını kontrol eder. **Kapalı** mod sertifika istemez.

Client authentication config'i geçersiz olursa, örneğin biri root sertifikayı sildiğinde, port bütün bağlantıları kapatır ve port listesi **TLS Hatası** gösterir.

## Adım 4: Upstream bağlantısını şifreleyin

Adım 1 ve Adım 3 veritabanına düz byte gönderir. Orada da TLS kullanmak için:

1. Proxy'yi açın ve upstream sunucu için **TLS ile Bağlan** seçeneğini açın. Adresi böylece `/tls` ile biter.
2. Veritabanı client sertifikası istiyorsa **Client Sertifikası** alanında bir sertifika seçin. r3v3rs3 bu sertifikayı, isteyen her TLS upstream sunucusuna gönderir.

```toml
[my-database]
protocol = "tcp"
client_cert = "a1b2c3d"
upstream_servers = [{ addr = "/dns/db.internal/tcp/5433/tls" }]
```

Bir proxy'nin kullandığı client sertifikası silinemez. Sertifika geçersiz olursa TCP proxy sertifikasız bağlanmaz, bağlantıyı kapatır.

## Adım 5: Daha çok upstream sunucu ekleyin

Birkaç upstream sunucusu olan bir TCP proxy bağlantıları dağıtır. UDP proxy bir session'ın paketlerini tek bir sunucuya gönderir.

- **Load Balancing** algoritmayı seçer, örneğin round robin veya client IP hash.
- **Health Check** cevap vermeyen sunucuyu çıkarır, tekrar cevap verince geri alır.
- **Circuit Breaker** sürekli hata veren bir sunucuya yeni bağlantı göndermeyi durdurur. Bütün sunucuların circuit'i açıksa client bağlantısı kapanır.

Alanları [Load balancing ve health check](@/configuration.tr.md#load-balancing-ve-health-check) bölümü anlatır.

## Client adresi

TCP proxy client adresini upstream sunucudan gizler, çünkü bağlantı r3v3rs3'ten gelir.

- Her upstream bağlantısının client adresiyle başlaması için **PROXY Protocol Gönder** seçeneğini açın. Bunu yalnız bütün upstream sunucular o header'ı okuyorsa açın.
- Diğer yönde, porttaki **PROXY Protocol Kabul Et** seçeneği adresi r3v3rs3 önündeki load balancer'dan okur. UDP portu bunu yapamaz.

## TCP veya UDP proxy'de olmayanlar

- Virtual host ve route yoktur. Bir portu tek bir TCP proxy taşır, yani servisi port numarası seçer.
- Cache, compression ve header kuralları yoktur. Bunlar HTTP ister.
- Kimlik doğrulama ve erişim listesi yoktur. Mutual TLS'i, firewall'unuzun IP filtresini veya servisin kendi kimlik doğrulamasını kullanın.

## Sonraki adımlar

- [Portlar](@/configuration.tr.md#portlar): bütün port tipleri, server name'ler ve PROXY protocol.
- [UDP session'ları](@/configuration.tr.md#udp-session-lari): session modeli ve sınırları.
- [Upstream client sertifikaları](@/configuration.tr.md#upstream-client-sertifikalari): upstream sunucuya doğru mutual TLS.
