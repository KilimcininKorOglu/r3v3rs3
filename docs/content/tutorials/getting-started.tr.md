+++
title = "Başlangıç"
description = "İlk hesaptan çalışan ilk proxy'nize kadar"
weight = 3
+++

# Başlangıç

Bu rehber kurulu bir r3v3rs3 ile çalışan bir proxy kurar. Birkaç dakika sürer.

Önce r3v3rs3'ü kurun. [Ana sayfa](@/_index.tr.md#kurulum) bütün yolları listeler: Linux kurulum script'i, Docker, Docker Compose, Cargo ve release arşivleri.

## Adım 1: Admin hesabı oluşturun

r3v3rs3 hiç hesap olmadan başlar. Komut satırından bir hesap oluşturun. Komut parolayı sorar, parola en az 8 karakter olmalıdır:

```bash
$ r3v3rs3 add-user admin
$ password?: ******
```

İki faktörlü doğrulama için `--totp` ekleyin. Komut secret'ı bir kez yazar. O anda doğrulama uygulamanıza ekleyin:

```bash
$ r3v3rs3 add-user admin --totp
$ password?: ******

Use this code to setup your TOTP client:
EXAMPLECODEEXAMPLECODE
```

`add-user` bir admin hesabı oluşturur. [Hesaplar](@/accounts.tr.md) sayfası `editor` ve `viewer` rollerini ve hesap başına proxy listelerini anlatır.

## Adım 2: Sunucuyu başlatın

```bash
$ r3v3rs3 start
```

Yönetim paneli [http://localhost:46492/](http://localhost:46492/) adresinde dinler. Adım 1'deki hesapla giriş yapın.

> Panel düz HTTP sunar. Uzak bir makinede panelin portunu internete açmak yerine SSH port yönlendirmesi ile bağlanın:
>
> ```bash
> $ ssh -L 46492:127.0.0.1:46492 kullanici@sunucunuz
> ```
>
> Paneli HTTPS ile sunmak için Adım 5'ten sonra bir HTTPS portunda **Hedef** değeri `http://127.0.0.1:46492` olan bir proxy ekleyin.

## Adım 3: Port bağlayın

Bir proxy'nin, istemcileri dinleyen bir porta ihtiyacı vardır.

1. Menüde **Portlar** linkine tıklayın.
2. **Ekle** butonuna tıklayın.
3. Porta bir ad verin, örneğin `Web Sitem`. Ad boş kalabilir.
4. **Ağ Arayüzü** alanında bir ağ arayüzü seçin. `0.0.0.0` bütün ağ arayüzlerinde dinler.
5. Portu seçin, örneğin `80`. Portu başka bir programın kullanmadığından ve bu portu bağlama izniniz olduğundan emin olun. Linux'ta 1024 altındaki bir port root veya `CAP_NET_BIND_SERVICE` yetkisi ister.
6. Protokolü seçin. Bu örnek **HTTP** kullanır.
7. **Oluştur** butonuna tıklayın.
8. Portun listede **Dinliyor** durumuyla göründüğünü kontrol edin. Başka bir durum nedeni söyler, örneğin **Adres Kullanımda** veya **İzin Reddedildi**.

## Adım 4: Proxy oluşturun

1. Menüde **Proxy'ler** linkine tıklayın.
2. **Ekle** butonuna tıklayın.
3. Proxy'ye bir ad verin, örneğin `Web Sitem`. Ad boş kalabilir.
4. Protokol olarak **HTTP / HTTPS** seçin.
5. Adım 3'teki portu seçin. Bir proxy birkaç port kullanabilir.
6. Bütün host'larla eşleşmesi için **Virtual Host'lar** alanını boş bırakın. `example.com` gibi bir değer yalnız o host ile eşleşir.
7. **Hedef** alanına uygulamanızın adresini yazın, örneğin `http://127.0.0.1:3000`.
8. **Oluştur** butonuna tıklayın.
9. Proxy'nin listede göründüğünü kontrol edin. Proxy hemen çalışır.

Adım 3'teki porta bir istek gönderin. Cevap uygulamanızdan gelir:

```bash
$ curl -i http://127.0.0.1/
```

`502 Bad Gateway` cevabı, r3v3rs3'ün hiçbir upstream sunucuya ulaşamadığı anlamına gelir. **Hedef** adresini ve uygulamanızı kontrol edin.

## Adım 5: Sertifika ekleyin

Herkese açık bir site HTTPS ister. r3v3rs3 sertifikaları bir ACME sunucusundan, örneğin Let's Encrypt'ten alır.

1. **HTTPS** protokolü ile ikinci bir port bağlayın, örneğin `443`.
2. Menüde **Sertifikalar** linkine, sonra **ACME** sekmesine, sonra **Ekle** butonuna tıklayın.
3. Sağlayıcıyı seçin, e-posta adresinizi ve alan adlarını yazın.
4. Challenge'ı seçin. **HTTP-01**, internetin eriştiği bir `80` portu ister. **DNS-01** hiçbir açık port istemez ve wildcard sertifika verir.
5. **Sertifika Al** butonuna tıklayın. Order hemen çalışır, sonra her sertifikadan 60 gün sonra tekrar çalışır.
6. HTTPS portunu proxy'nize ekleyin ve alan adını **Virtual Host'lar** alanına yazın.

[Sertifikalar](@/configuration.tr.md#sertifikalar) ve [ACME](@/configuration.tr.md#acme) bölümleri her alanı anlatır.

## Sonraki adımlar

- [Yapılandırma](@/configuration.tr.md): route seçimi, yük dengeleme, cache, kimlik doğrulama, rate limit ve diğer bütün proxy ayarları.
- [Hesaplar](@/accounts.tr.md): roller, proxy listeleri ve denetim kaydı.
- [Servis keşfi](@/discovery.tr.md): Docker etiketleri, Kubernetes, Consul ve etcd'den gelen proxy'ler.
- [Yüksek erişilebilirlik](@/tutorials/high-availability.tr.md): tek bir durum bilgisini paylaşan birkaç düğüm.
