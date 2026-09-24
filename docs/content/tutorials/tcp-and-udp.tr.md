+++
title = "TCP ve UDP proxy'ler"
description = "Bir veritabanını TCP, bir DNS sunucusunu UDP üzerinden yayınlayın, mutual TLS ile"
weight = 9
+++

# TCP ve UDP proxy'ler

r3v3rs3 yalnız HTTP'yi taşımaz. Bu rehber bir PostgreSQL sunucusunu TCP, bir DNS sunucusunu UDP üzerinden yayınlar, sonra veritabanının önüne mutual TLS koyar. Böylece yalnız sertifikası olan bir istemci bağlanır.

TCP veya UDP proxy byte taşır. İstek okumaz, bu yüzden virtual host'u, route'u, yolu ve header kuralları yoktur.

Önce [Başlangıç](@/tutorials/getting-started.tr.md) rehberini izleyin.

## Adım 1: PostgreSQL'i TCP üzerinden yayınlayın

1. Menüde **Portlar** linkine, sonra **Ekle** butonuna tıklayın.
2. Ad alanına `postgres` yazın, ağ arayüzünü ve `5432` portunu seçin, protokol olarak **TCP** seçin.
3. **Oluştur** butonuna tıklayın ve **Dinliyor** durumunu kontrol edin.
4. **Proxy'ler** linkine, sonra **Ekle** butonuna tıklayın ve protokol olarak **TCP / TLS üzerinden TCP** seçin.
5. `postgres` portunu seçin.
6. **Upstream Sunucu** altında **Host** alanına veritabanının adresini, örneğin `10.0.0.5`, **Port** alanına `5432` yazın.
7. **Oluştur** butonuna tıklayın.

```bash
$ psql "postgresql://postgres:<parola>@proxy.example.com:5432/postgres" -c 'select version();'
 PostgreSQL 17.11 on aarch64-unknown-linux-musl, ...
```

r3v3rs3 adresi multiaddr olarak saklar: `/ip4/<adres>/tcp/<port>`, `/ip6/<adres>/tcp/<port>` veya `/dns/<ad>/tcp/<port>`. `/dns` ile r3v3rs3 adı her bağlantıda çözer.

Bir TCP portunu aynı anda tek bir TCP proxy kullanır. Aynı porttaki ikinci bir TCP proxy hatadır.

## Adım 2: Bir DNS sunucusunu UDP üzerinden yayınlayın

1. Protokolü **UDP** olan bir port ekleyin, örneğin `53` portunda.
2. Protokolü **UDP** olan bir proxy ekleyin ve portu seçin. **Upstream Sunucu** altında **Host** alanına `10.0.0.53`, **Port** alanına `53` yazın.

```bash
$ dig +short @proxy.example.com example.com A
172.66.147.243
```

UDP proxy her istemci adresi için bir oturum açar, her oturumun upstream sunucuya kendi socket'i vardır. İki yönde de **Oturum Boşta Kalma Timeout'u (Saniye)** boyunca paket geçmezse oturum kapanır. Varsayılan 60 saniyedir. Bir port en çok 10.000 oturum tutar, dolduğunda yeni istemcilerin paketlerini atar.

## Adım 3: Mutual TLS ekleyin

TLS üzerinden TCP portu TLS'i sonlandırır ve upstream sunucuya düz byte gönderir. İstemci kimlik doğrulaması ile yalnız sizin CA'nızın sertifikasını taşıyan istemci bağlanır.

### Sertifikaları oluşturun

1. **Sertifikalar** linkine, sonra **Sunucu Sertifikaları** sekmesine, sonra **Kendinden İmzalı** butonuna tıklayın.
2. Subject name olarak proxy'nin host adını yazın, örneğin `db.example.com`, ve oluşturun. r3v3rs3 **Kök Sertifikalar** sekmesinde bir CA sertifikası da oluşturur.
3. **İstemci Sertifikaları** sekmesini açın, **Kendinden İmzalı** butonuna tıklayın, **Sertifika Türü** olarak **İstemci Sertifikası** ve bir önceki adımdaki CA'yı seçin, sonra oluşturun.
4. İstemci sertifikasını **İndir** ile indirin. Arşiv `chain.pem` ve `key.pem` dosyalarını taşır. İkisini de istemciye verin.
5. **Kök Sertifikalar** sekmesindeki CA sertifikasını indirin ve `ca.pem` olarak kaydedin. İstemci sunucu sertifikasını onunla kontrol eder.

### Portu bağlayın

1. Protokolü **TLS üzerinden TCP** olan bir port ekleyin, örneğin `5433` portunda. Adım 1'deki `5432` portunu zaten bir TCP proxy kullanır.
2. **Sunucu Adları** alanına host adını yazın. r3v3rs3 sunucu sertifikasını istemcinin SNI değerinden seçer.
3. **İstemci Kimlik Doğrulaması** alanında **Zorunlu** seçin.
4. **İstemci CA Sertifikaları** alanında CA sertifikasını seçin. Sistemin kök sertifikaları kullanılmaz.
5. Proxy'yi bu portta, Adım 1'deki upstream sunucuyla oluşturun.

Portu `openssl s_client` ile deneyin:

```bash
$ openssl s_client -connect db.example.com:5433 -servername db.example.com \
    -CAfile ca.pem -cert chain.pem -key key.pem
```

`-cert` ve `-key` olmadan bağlanan istemci bağlantı kuramaz. Alert'in adını istemcinin TLS kütüphanesi yazar.

**İsteğe bağlı** mod sertifikası olmayan istemciyi kabul eder, sertifika gönderen istemcinin sertifikasını kontrol eder. **Kapalı** mod sertifika istemez.

İstemci kimlik doğrulaması ayarı geçersiz olursa, örneğin biri kök sertifikayı sildiğinde, port bütün bağlantıları kapatır ve port listesi **TLS Hatası** gösterir.

## Adım 4: Upstream bağlantısını şifreleyin

Adım 1 ve Adım 3 veritabanına düz byte gönderir. Orada da TLS kullanmak için:

1. Proxy'yi açın ve upstream sunucu için **TLS ile Bağlan** seçeneğini açın. Adresi böylece `/tls` ile biter.
2. Veritabanı istemci sertifikası istiyorsa **İstemci Sertifikası** alanında bir sertifika seçin. r3v3rs3 bu sertifikayı, isteyen her TLS upstream sunucusuna gönderir.

```toml
[my-database]
protocol = "tcp"
client_cert = "a1b2c3d"
upstream_servers = [{ addr = "/dns/db.internal/tcp/5433/tls" }]
```

Bir proxy'nin kullandığı istemci sertifikası silinemez. Sertifika geçersiz olursa TCP proxy sertifikasız bağlanmaz, bağlantıyı kapatır.

## Adım 5: Daha çok upstream sunucu ekleyin

Birkaç upstream sunucusu olan bir TCP proxy bağlantıları dağıtır. UDP proxy bir oturumun paketlerini tek bir sunucuya gönderir.

- **Yük Dengeleme** algoritmayı seçer, örneğin round robin veya istemci IP hash'i.
- Sağlık kontrolü cevap vermeyen sunucuyu çıkarır, tekrar cevap verince geri alır. **Maksimum Hata Sayısı** ve **Kontrol Aralığı (Saniye)** alanları onu ayarlar.
- **Circuit Breaker** sürekli hata veren bir sunucuya yeni bağlantı göndermeyi durdurur. Bütün sunucuların circuit'i açıksa istemcinin bağlantısı kapanır.

Alanları [Yük dengeleme ve sağlık kontrolü](@/configuration.tr.md#yuk-dengeleme-ve-saglik-kontrolu) bölümü anlatır.

## İstemci adresi

TCP proxy istemci adresini upstream sunucudan gizler, çünkü bağlantı r3v3rs3'ten gelir.

- Her upstream bağlantısının istemci adresiyle başlaması için **PROXY Protocol Gönder** seçeneğini açın. Bunu yalnız bütün upstream sunucular o header'ı okuyorsa açın, örneğin PROXY protocol'ü destekleyen bir pooler'ın arkasındaki PostgreSQL için.
- Diğer yönde, porttaki **PROXY Protocol Kabul Et** seçeneği adresi r3v3rs3 önündeki yük dengeleyiciden okur. UDP portu bunu yapamaz.

## TCP veya UDP proxy'de olmayanlar

- Virtual host ve route yoktur. Bir portu tek bir TCP proxy taşır, yani servisi port numarası seçer.
- Cache, sıkıştırma ve header kuralları yoktur. Bunlar HTTP ister.
- Kimlik doğrulama ve erişim listesi yoktur. Mutual TLS'i, firewall'unuzun IP filtresini veya servisin kendi kimlik doğrulamasını kullanın.

## Sonraki adımlar

- [Portlar](@/configuration.tr.md#portlar): bütün port türleri, sunucu adları ve PROXY protocol.
- [UDP oturumları](@/configuration.tr.md#udp-oturumlari): oturum modeli ve sınırları.
- [Upstream istemci sertifikaları](@/configuration.tr.md#upstream-istemci-sertifikalari): upstream sunucuya doğru mutual TLS.
