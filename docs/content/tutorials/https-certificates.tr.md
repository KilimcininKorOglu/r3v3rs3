+++
title = "Wildcard sertifika ile HTTPS"
description = "Bir alan adını ve alt alan adlarını DNS-01 ile HTTPS üzerinden sunun"
weight = 6
+++

# Wildcard sertifika ile HTTPS

Bu rehber `example.com` alan adını ve bütün alt alan adlarını HTTPS üzerinden sunar. r3v3rs3 Let's Encrypt'ten DNS-01 challenge ile tek bir wildcard sertifika alır, HTTP'yi HTTPS'e yönlendirir ve HSTS gönderir.

Wildcard sertifika veren tek challenge DNS-01'dir. DNS-01 doğrulama için açık port da istemez, bu yüzden güvenlik duvarı arkasındaki bir sunucuda çalışır.

## Başlamadan önce

- DNS zone'unu [DNS-01 tablosundaki](@/configuration.tr.md#dns-01) sağlayıcılardan birinde tuttuğunuz bir alan adı, örneğin Cloudflare veya Route 53.
- O sağlayıcının zone'u okuyabilen ve kayıt yazabilen bir API kimlik bilgisi.
- Admin hesabı olan, çalışan bir r3v3rs3. Kurulumu [Başlangıç](@/tutorials/getting-started.tr.md) anlatır.
- `example.com` ve her alt alan adının `A` kaydı sunucunun IP adresini gösterir. DNS-01 bunu kontrol etmez, ama istemcilerinizin buna ihtiyacı vardır.

## Adım 1: API kimlik bilgisini oluşturun

Zone'daki `_acme-challenge` TXT kayıtlarını yazabilen bir kimlik bilgisi oluşturun. r3v3rs3 her kaydı doğrulamadan önce oluşturur, doğrulamadan sonra siler.

| Sağlayıcı | Kimlik bilgisi | İzinler |
|---|---|---|
| Cloudflare | API Token | Zone için `Zone:Read` ve `DNS:Edit` |
| Route 53 | Access Key ID, Secret Access Key | `route53:ListHostedZones` ve `route53:ChangeResourceRecordSets` |

Diğer 13 sağlayıcıyı kimlik bilgileriyle [DNS-01 tablosu](@/configuration.tr.md#dns-01) listeler. İkisi bir DNS barındırma sağlayıcısı istemez: webhook sağlayıcısı kendi servisinizi çağırır, exec sağlayıcısı r3v3rs3 host'unda bir program çalıştırır.

## Adım 2: HTTP ve HTTPS portlarını bağlayın

Wildcard sertifika açık port istemez, ama siteniz ister.

1. Menüde **Portlar** linkine, sonra **Ekle** butonuna tıklayın.
2. Ağ arayüzü olarak `0.0.0.0`, port olarak `443`, protokol olarak **HTTPS** seçin. **Oluştur** butonuna tıklayın.
3. İkinci bir port ekleyin: port `80`, protokol **HTTP**. Adım 5 bu portu yönlendirir.
4. İki portun da **Dinliyor** durumunda olduğunu kontrol edin.

Linux'ta 1024 altındaki bir port root veya `CAP_NET_BIND_SERVICE` capability'si ister.

## Adım 3: ACME kaydını oluşturun

1. Menüde **Sertifikalar** linkine, sonra **ACME** sekmesine, sonra **Ekle** butonuna tıklayın.
2. Sağlayıcı olarak **Let's Encrypt** seçin. Sayfa onun directory URL'ini kullanır. **Sağlayıcı** listesinde Google Trust Services, ZeroSSL ve özel bir sunucu da vardır.
3. **E-posta Adresi** alanına adresinizi yazın. Sertifika otoritesi bitiş uyarılarını oraya gönderir.
4. **Alan Adları** alanına `example.com, *.example.com` yazın. Sertifika her adı bir Subject Alternative Name olarak taşır.
5. **Challenge** alanında **DNS-01** seçin.
6. **DNS Sağlayıcısı** alanında sağlayıcınızı seçin ve Adım 1'deki kimlik bilgisi alanlarını doldurun.
7. **Sertifika Al** butonuna tıklayın. Hazır sağlayıcılar her siparişten 60 gün sonra yeni sipariş verir. Özel bir sunucuda **Yenileme Aralığı (Gün)** alanı vardır.

r3v3rs3 sertifikayı hemen, sonra her yenileme kontrolünde ister. `example.com` ve `*.example.com` aynı TXT kayıt adını, `_acme-challenge.example.com` adını paylaşır, bu yüzden doğrulama sırasında kayıt iki değer taşır.

r3v3rs3 kaydı `acme.toml` dosyasında saklar:

```toml
[abc-def]
provider = "Let's Encrypt"
renewal_days = 60
identifiers = ["example.com", "*.example.com"]
challenge_type = "dns-01"

[abc-def.dns_provider]
provider = "cloudflare"
api_token = "<token>"
```

Kaydı bu dosyada değil, panelde oluşturun. r3v3rs3 ACME hesabını kaydı eklediğinizde oluşturur. Dosya hesap key'ini ve kimlik bilgisini düz metin olarak, `0600` izinleriyle tutar. Yönetim API'si onları döndürmez.

## Adım 4: Sertifikayı kontrol edin

Siparişten sonra **Sunucu Sertifikaları** sekmesi sertifikayı gösterir. Sekme sertifikayı vereni, subject adlarını ve **Bitiş Tarihi** değerini yazar. **ACME** sekmesi kaydın **Yenileme Tarihi** değerini gösterir.

Başarısız bir sipariş nedenini sunucu log'una yazar, r3v3rs3 bir saat sonra yeniden sipariş verir. İki neden sık görülür:

- Kimlik bilgisi zone'a yazamıyordur. Log, sağlayıcının API hatasını yazar.
- TXT kaydı 5 dakika sonra hâlâ görünmüyordur. Bunun nedeni sistem çözümleyicisinin cache'te tuttuğu yanıttır. **Ayarlar** sayfasındaki **DNS Challenge Çözümleyicisi** alanını `1.1.1.1:53` yapın veya zone'un authoritative name server'ını yazın.

## Adım 5: Proxy'yi oluşturun

1. Menüde **Proxy'ler** linkine, sonra **Ekle** butonuna tıklayın.
2. Protokol olarak **HTTP / HTTPS** seçin.
3. Adım 2'deki **iki** portu da seçin. Proxy yönlendirme için HTTP portuna, istekleri karşılamak için HTTPS portuna ihtiyaç duyar.
4. **Virtual Host'lar** alanına `app.example.com` yazın. Wildcard sertifika bütün alt alan adlarını kapsar, bu yüzden her alt alan adı kendi proxy'sini alabilir.
5. **Hedef** alanına uygulamanızın adresini yazın, örneğin `http://127.0.0.1:3000`.
6. **HTTP'yi Otomatik Olarak HTTPS'e Yönlendir** seçeneğini açın.
7. **Oluştur** butonuna tıklayın.

r3v3rs3 sertifikayı TLS handshake'indeki SNI adından seçer, bu yüzden HTTPS portunda sertifika ayarı gerekmez.

İki protokolü de kontrol edin:

```bash
$ curl -i http://app.example.com/
HTTP/1.1 301 Moved Permanently
location: https://app.example.com/

$ curl -i https://app.example.com/
HTTP/2 200
```

Yönlendirme yolu ve sorguyu korur. Yönlendirme kurallarından ve kimlik doğrulamadan önce çalışır.

## Adım 6: HSTS gönderin

HSTS, tarayıcıya bir sonraki istekte yönlendirme beklemeden HTTPS kullanmasını söyler. Bunu bir yanıt header'ı kuralı olarak ekleyin:

1. Proxy'yi açın ve **Header Kuralları** bölümünü bulun.
2. **Yanıt Header'ları** alanına şu satırı yazın:

```text
set Strict-Transport-Security: max-age=31536000; includeSubDomains
```

3. **Güncelle** butonuna tıklayın.

```bash
$ curl -sI https://app.example.com/ | grep -i strict
strict-transport-security: max-age=31536000; includeSubDomains
```

- Kural upstream sunucunun yanıtlarını değiştirir. r3v3rs3'ün kendi ürettiği yanıtları, örneğin HTTP yönlendirmesini ve hata sayfalarını değiştirmez. Tarayıcının politikayı saklaması için tek bir başarılı HTTPS yanıtı yeter.
- `max-age=31536000` bir yıldır. Tarayıcı o süre boyunca alan adı için düz HTTP'yi reddeder. Her alt alan adının HTTPS sunduğundan emin olana kadar kısa bir `max-age` ile başlayın, örneğin `300`.
- `includeSubDomains` bütün alt alan adlarını kapsar. Bir alt alan adı hâlâ düz HTTP sunuyorsa bunu göndermeyin.

## Adım 7: HTTP/3 ekleyin

HTTP/3, QUIC üzerinde çalışır ve QUIC UDP'dir. HTTPS portunun yanına ikinci bir port ister; onun yerini almaz.

1. **Portlar** sayfasını açın ve bir port ekleyin.
2. Protokolü **QUIC üzerinden HTTP (HTTP/3)** yapın.
3. `0.0.0.0:443` adresini dinleyin. Bu UDP 443'tür, HTTPS portunun TCP 443'ü ile çakışmaz.
4. **Sunucu Adları** alanına HTTPS portundaki adları yazın, böylece iki port aynı sertifikayı kullanır.
5. Proxy'yi açın ve yeni portu HTTPS portunun yanına ekleyin.

Portun dinleme adresi şöyle olur:

```text
/ip4/0.0.0.0/udp/443/quic/http
```

Bundan sonra proxy'nin her yanıtı bir `alt-svc` header'ı taşır:

```bash
$ curl -sI https://app.example.com/
HTTP/2 200
alt-svc: h2=":443", h3=":443", h3-25=":443"
```

Bu header'ı r3v3rs3 kendisi yazar; değerini proxy'nin HTTPS portundan ve QUIC portundan üretir. Upstream sunucunun gönderdiği `alt-svc` header'ını siler. Tarayıcı header'ı okur, QUIC portunu hatırlar ve sonraki istekte HTTP/3 kullanır. İlk istek her zaman TCP kullanır, bu yüzden UDP kapalıysa hiçbir şey bozulmaz.

Kontrol edilecek üç şey:

- UDP 443'ü güvenlik duvarında ve security group'ta açın. UDP'si kapalı olan istemci HTTP/2 kullanmaya devam eder ve hiçbir hata görmezsiniz.
- Docker UDP'yi ayrı yayınlar: `-p 443:443` yanında `-p 443:443/udp`.
- HTTPS portu olmayan bir proxy yalnız QUIC üzerinden yanıt verir ve `alt-svc` header'ı yalnız `h3` ve `h3-25` taşır. İki portu da proxy'de tutun.

HTTP/3 yalnız gelen bağlantılar için vardır. Upstream bağlantısı HTTP/2 veya HTTP/1.1 kullanır, WebTransport desteklenmez.

## Yenileme

r3v3rs3 son siparişten `renewal_days` sonra yeni bir sertifika ister, hazır sağlayıcılarda 60 gün. Let's Encrypt sertifikası 90 gün geçerlidir, yani yenilemenin 30 gün payı olur. Sipariş DNS-01 challenge'ını tekrarlar, bu yüzden kimlik bilgisi geçerli kalmalıdır. Cluster'da sertifikayı yalnız lider ister.

**Ayarlar** sayfasında bir bildirim webhook'u vardır. Bitişten önce `certificate_expiring`, başarısız bir siparişten sonra `acme_order_failed` olayını gönderir, böylece bozulan bir kimlik bilgisi fark edilmeden kalmaz. Olayları [Bildirimler](@/configuration.tr.md#bildirimler) bölümü anlatır.

## Sonraki adımlar

- [Sertifikalar ve ACME](@/configuration.tr.md#acme): bütün challenge'lar, bütün sağlayıcılar ve saklanan veriler.
- [Header kuralları](@/configuration.tr.md#header-kurallari): diğer kurallar ve değişkenleri.
- [Docker'dan proxy'ler](@/tutorials/docker-discovery.tr.md): bir container etiketi bu ACME kaydını gösterebilir, böylece keşfedilen proxy kendi sertifikasını alır.
