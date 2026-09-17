+++
title = "Wildcard sertifika ile HTTPS"
description = "Bir domain'i ve alt domain'lerini DNS-01 ile HTTPS üzerinden sunun"
weight = 4
+++

# Wildcard sertifika ile HTTPS

Bu rehber `example.com` domain'ini ve bütün alt domain'lerini HTTPS üzerinden sunar. r3v3rs3 Let's Encrypt'ten DNS-01 challenge ile tek bir wildcard sertifika alır, HTTP'yi HTTPS'e yönlendirir ve HSTS gönderir.

Wildcard sertifika veren tek challenge DNS-01'dir. DNS-01 doğrulama için açık port da istemez, bu yüzden firewall arkasındaki bir sunucuda çalışır.

## Başlamadan önce

- DNS zone'unu [DNS-01 tablosundaki](@/configuration.tr.md#dns-01) provider'lardan birinde tuttuğunuz bir domain, örneğin Cloudflare veya Route 53.
- O provider'ın zone'u okuyabilen ve kayıt yazabilen bir API credential'ı.
- Admin hesabı olan, çalışan bir r3v3rs3. Kurulumu [Başlangıç](@/tutorials/getting-started.tr.md) anlatır.
- `example.com` ve her alt domain'in `A` kaydı sunucunun IP adresini gösterir. DNS-01 bunu kontrol etmez, ama client'larınızın buna ihtiyacı vardır.

## Adım 1: API credential'ını oluşturun

Zone'daki `_acme-challenge` TXT kayıtlarını yazabilen bir credential oluşturun. r3v3rs3 her kaydı doğrulamadan önce oluşturur, doğrulamadan sonra siler.

| Provider | Credential | İzinler |
|---|---|---|
| Cloudflare | API Token | Zone için `Zone:Read` ve `DNS:Edit` |
| Route 53 | Access Key ID, Secret Access Key | `route53:ListHostedZones` ve `route53:ChangeResourceRecordSets` |

Diğer 13 provider'ı credential'larıyla [DNS-01 tablosu](@/configuration.tr.md#dns-01) listeler. İkisi hosting provider'ı istemez: webhook provider'ı kendi servisinizi çağırır, exec provider'ı r3v3rs3 host'unda bir program çalıştırır.

## Adım 2: HTTP ve HTTPS portlarını bağlayın

Wildcard sertifika açık port istemez, ama siteniz ister.

1. Menüde **Portlar** linkine, sonra **Ekle** butonuna tıklayın.
2. Interface olarak `0.0.0.0`, port olarak `443`, protokol olarak **HTTPS** seçin. **Oluştur** butonuna tıklayın.
3. İkinci bir port ekleyin: port `80`, protokol **HTTP**. Adım 5 bu portu yönlendirir.
4. İki portun da **Dinliyor** durumunda olduğunu kontrol edin.

Linux'ta 1024 altındaki bir port root veya `CAP_NET_BIND_SERVICE` capability'si ister.

## Adım 3: ACME kaydını oluşturun

1. Menüde **Sertifikalar** linkine, sonra **ACME** sekmesine, sonra **Ekle** butonuna tıklayın.
2. Sağlayıcı olarak **Let's Encrypt** seçin. Sayfa onun directory URL'ini kullanır. **ACME Sağlayıcısı** listesinde Google Trust Services, ZeroSSL ve custom bir sunucu da vardır.
3. **E-posta Adresi** alanına adresinizi yazın. Sertifika otoritesi bitiş uyarılarını oraya gönderir.
4. **Domain Adları** alanına `example.com, *.example.com` yazın. Sertifika her adı bir Subject Alternative Name olarak taşır.
5. **Challenge** alanında **DNS-01** seçin.
6. **DNS Provider** alanında provider'ınızı seçin ve Adım 1'deki credential alanlarını doldurun.
7. **Oluştur** butonuna tıklayın. Hazır sağlayıcılar her order'dan 60 gün sonra yeni order verir. Custom bir sunucuda **Yenileme Aralığı (Gün)** alanı vardır.

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

Kaydı bu dosyada değil, panelde oluşturun. r3v3rs3 ACME hesabını kaydı eklediğinizde oluşturur. Dosya hesap key'ini ve credential'ı düz metin olarak, `0600` moduyla tutar. Admin API onları döndürmez.

## Adım 4: Sertifikayı kontrol edin

Order'dan sonra **Sunucu Sertifikaları** sekmesi sertifikayı gösterir. Sekme issuer'ı, subject adlarını ve **Yenileme Tarihi** alanını yazar.

Başarısız bir order nedenini sunucu log'una yazar, r3v3rs3 bir saat sonra tekrar order verir. İki neden sık görülür:

- Credential zone'a yazamıyordur. Log, provider'ın API hatasını yazar.
- TXT kaydı 5 dakika sonra hâlâ görünmüyordur. Bunun nedeni sistem resolver'ının cache'lediği cevaptır. **Ayarlar** sayfasındaki **DNS Challenge Resolver** alanını `1.1.1.1:53` yapın veya zone'un authoritative name server'ını yazın.

## Adım 5: Proxy'yi oluşturun

1. Menüde **Proxy'ler** linkine, sonra **Ekle** butonuna tıklayın.
2. Protokol olarak **HTTP / HTTPS** seçin.
3. Adım 2'deki **iki** portu da seçin. Proxy yönlendirme için HTTP portuna, trafik için HTTPS portuna ihtiyaç duyar.
4. **Virtual Host'lar** alanına `app.example.com` yazın. Wildcard sertifika bütün alt domain'leri kapsar, bu yüzden her alt domain kendi proxy'sini alabilir.
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

Yönlendirme path'i ve query'yi korur. Redirect kurallarından ve kimlik doğrulamasından önce çalışır.

## Adım 6: HSTS gönderin

HSTS, tarayıcıya bir sonraki request'te yönlendirme beklemeden HTTPS kullanmasını söyler. Bunu bir response header kuralı olarak ekleyin:

1. Proxy'yi açın ve **Header Kuralları** bölümünü bulun.
2. **Response Header'ları** alanına şu satırı yazın:

```text
set Strict-Transport-Security: max-age=31536000; includeSubDomains
```

3. Kaydedin.

```bash
$ curl -sI https://app.example.com/ | grep -i strict
strict-transport-security: max-age=31536000; includeSubDomains
```

- Kural upstream sunucunun response'larını değiştirir. r3v3rs3'ün kendi ürettiği response'ları, örneğin HTTP yönlendirmesini ve hata sayfalarını değiştirmez. Tarayıcının politikayı saklaması için tek bir başarılı HTTPS response'u yeter.
- `max-age=31536000` bir yıldır. Tarayıcı o süre boyunca domain için düz HTTP'yi reddeder. Her alt domain'in HTTPS sunduğundan emin olana kadar kısa bir `max-age` ile başlayın, örneğin `300`.
- `includeSubDomains` bütün alt domain'leri kapsar. Bir alt domain hâlâ düz HTTP sunuyorsa bunu göndermeyin.

## Yenileme

r3v3rs3 son order'dan `renewal_days` sonra yeni bir sertifika ister, hazır sağlayıcılarda 60 gün. Let's Encrypt sertifikası 90 gün geçerlidir, yani yenilemenin 30 gün payı olur. Order DNS-01 challenge'ını tekrarlar, bu yüzden credential geçerli kalmalıdır. Cluster'da sertifikayı yalnız leader ister.

**Ayarlar** sayfasında bir bildirim webhook'u vardır. Bitişten önce `certificate_expiring`, başarısız bir order'dan sonra `acme_order_failed` event'i gönderir, böylece bozulan bir credential sessiz kalmaz. Event'leri [Bildirimler](@/configuration.tr.md#bildirimler) bölümü anlatır.

## Sonraki adımlar

- [Sertifikalar ve ACME](@/configuration.tr.md#acme): bütün challenge'lar, bütün provider'lar ve saklanan veriler.
- [Header kuralları](@/configuration.tr.md#header-kurallari): diğer kurallar ve değişkenleri.
- [Docker'dan proxy'ler](@/tutorials/docker-discovery.tr.md): bir container label'ı bu ACME kaydını gösterebilir, böylece keşfedilen proxy kendi sertifikasını alır.
