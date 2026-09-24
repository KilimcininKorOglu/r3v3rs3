+++
title = "Cache ve sıkıştırma"
description = "Tekrarlanan istekleri bellekten karşılayın ve daha küçük yanıtlar gönderin"
weight = 8
+++

# Cache ve sıkıştırma

Bu rehber bir siteyi iki proxy ayarıyla hızlandırır. Cache, tekrarlanan bir isteği bellekten karşılar ve upstream sunucuya hiç gitmez. Sıkıştırma her istemciye daha küçük bir gövde gönderir.

İkisi birlikte çalışır: r3v3rs3 tek bir sıkıştırılmamış yanıt saklar ve onu her istemci için ayrı sıkıştırır. Böylece tek bir kayıt hem Brotli isteyen istemciye, hem gzip isteyen istemciye, hem de sıkıştırma istemeyen istemciye hizmet eder.

Önce [Başlangıç](@/tutorials/getting-started.tr.md) rehberini izleyin. Bu rehber çalışan bir proxy ile devam eder.

## Adım 1: Sıkıştırmayı açın

1. Proxy'yi açın ve **Sıkıştırma** bölümünü bulun.
2. **Brotli**, **Zstandard** ve **Gzip** seçin. Seçim sıranız, istemci birkaç encoding'i aynı `q` değeriyle kabul ettiğinde r3v3rs3'ün tercih ettiği sıradır.
3. **Minimum Boyut (Byte)** alanını `1024` bırakın. Daha küçük bir gövde sıkıştırmayla küçülmez.
4. **Güncelle** butonuna tıklayın.

```bash
$ curl -s http://app.example.com/data.json -o /dev/null -w '%{size_download}\n'
12591

$ curl -s -H 'Accept-Encoding: br' http://app.example.com/data.json -o /dev/null -w '%{size_download}\n'
711
```

r3v3rs3 bir yanıtı yalnız medya türü **Medya Türleri** listesindeyse sıkıştırır. Varsayılan liste `text/*`, `application/json`, `application/javascript`, `application/manifest+json`, `application/wasm`, XML türleri, `image/svg+xml` ve iki font türünü tutar. Bir görsel veya video zaten sıkıştırılmıştır, bu yüzden listede yoktur.

Upstream sunucu yanıtı zaten encode ettiyse, `Cache-Control` içinde `no-transform` varsa ve `text/event-stream` için r3v3rs3 sıkıştırma yapmaz. Sıkıştırma server-sent event'leri geciktirir.

## Adım 2: Cache'i açın

1. Proxy'yi açın ve **Cache** bölümünü bulun.
2. **Cache'i Aç** seçeneğini açın.
3. **Bellek Limiti (Byte)** alanını `67108864` (64 MiB), **Maksimum Yanıt Boyutu (Byte)** alanını `1048576` (1 MiB) bırakın.
4. **Varsayılan TTL (Saniye)** alanına `300` yazın.
5. **Güncelle** butonuna tıklayın.

Proxy'nin bütün route'ları tek bir cache'i paylaşır. Her proxy'nin kendi cache'i vardır.

```bash
$ curl -sI http://app.example.com/data.json | grep -i x-cache
x-cache: MISS

$ curl -sI http://app.example.com/data.json | grep -iE 'x-cache|age'
x-cache: HIT
age: 0
```

`X-Cache: MISS` yanıtın upstream sunucudan geldiği, `HIT` ise bellekten geldiği anlamına gelir. `Age`, upstream sunucunun yanıtı gönderdiği andan beri geçen saniyeyi sayar.

## Adım 3: TTL'i anlayın

Saklanan bir yanıtın ömrü şunlardan var olan ilkinden gelir: `s-maxage`, `max-age`, `Expires`, sonra **Varsayılan TTL (Saniye)**.

Yani kararı upstream sunucu verir, **Varsayılan TTL (Saniye)** de hiçbir şey söylemeyen yanıtları kapsar. Yapabildiğiniz yerde header'ları uygulamanızda ayarlayın:

```text
Cache-Control: public, max-age=300
```

**Varsayılan TTL (Saniye)** alanı `0` ise, r3v3rs3 böyle bir yanıtı yalnız `ETag` veya `Last-Modified` header'ı varsa saklar ve her istekte yeniden doğrular. Bu her istemci isteği için bir koşullu istek demektir, ama hiçbir zaman bayat içerik sunmaz.

Saklanan bir yanıt bayatladıysa ve validator'ı varsa, r3v3rs3 `If-None-Match` veya `If-Modified-Since` ile koşullu istek gönderir. `304 Not Modified` cevabı saklanan header'ları tazeler, istemci de saklanan gövdeyi yeniden indirmeden alır.

## Adım 4: Nelerin saklanmadığını bilin

r3v3rs3 bir yanıtı yalnız bütün koşullar sağlanınca saklar. Sürekli `MISS` görmenin sık nedenleri:

- İstek `GET` veya `HEAD` değildir, ya da `Range`, `Upgrade` veya `Cache-Control: no-store` taşır.
- Yanıt `Set-Cookie` header'ı taşır. Bir oturuma ait yanıt paylaşılmamalıdır.
- `Cache-Control` içinde `no-store` veya `private` vardır.
- Yanıt `Vary: *` taşır.
- İstek `Authorization` header'ı taşır ve `Cache-Control` içinde `public`, `s-maxage`, `must-revalidate` değerlerinden hiçbiri yoktur.
- Gövde, **Maksimum Yanıt Boyutu (Byte)** değerinden büyüktür.
- Durum kodu şunlardan biri değildir: `200`, `203`, `204`, `300`, `301`, `308`, `404`, `405`, `410`, `414`, `501`.

Cache key'i, istenen host ile isteğin yolu ve sorgusudur. Yük dengeleme key'i değiştirmez, yani bütün upstream sunucular tek bir kaydı paylaşır. Yanıt, istek header adlarını taşıyan bir `Vary` header'ı içeriyorsa r3v3rs3 saklanan yanıtı yalnız o header'ları aynı değerlerle taşıyan isteğe verir.

## Adım 5: Deploy'dan sonra temizleyin

Deploy dosyaları değiştirir, ama saklanan yanıtlar TTL'lerini korur.

- Proxy listesinde **Temizle** linkine tıklayın.
- Veya `DELETE /api/proxies/{id}/cache` gönderin.

```bash
$ curl -b session.txt -X DELETE http://127.0.0.1:46492/api/proxies/hsy-cns/cache
$ curl -sI http://app.example.com/data.json | grep -i x-cache
x-cache: MISS
```

r3v3rs3'ün yeniden başlaması ve cache ayarlarının değişmesi de saklanan bütün yanıtları siler. Saklanan yanıtlar yalnız bellekte durur.

## Adım 6: CDN arkasında gerçek istemci IP'sini görün

r3v3rs3 önündeki bir CDN de cache tutar ve ziyaretçi adresini proxy'den gizler. Doğru ayar olmadan rate limit ve IP filtresi edge sunucuyu görür.

r3v3rs3 sekiz bilinen CDN'in IP aralıklarına varsayılan olarak güvenir: Cloudflare, Fastly, Amazon CloudFront, Bunny CDN, Gcore, KeyCDN, Imperva ve Google Cloud Load Balancing. Güvenilen bir karşı taraf için sağlayıcının header'ını okur, örneğin `CF-Connecting-IP`, ve çözümlenen adresi upstream sunucuya `X-Real-IP` header'ında gönderir.

- Aralıklar binary'nin içine derlenir ve her gün yeniden indirilir. **Ayarlar** sayfası listenin durumunu gösterir ve bir **Şimdi Yenile** butonu taşır.
- Kendi yük dengeleyicinizi proxy'nin **Güvenilen Proxy'ler** alanına ekleyin.
- Akamai edge aralıklarını yayınlamaz. Site Shield aralıklarınızı **Güvenilen Proxy'ler** alanına yazın.

## Cluster'da

Her düğümün kendi cache'i vardır. `share_cache = true` ile düğümler yanıtları cluster veri deposuna da yazar, böylece bir düğümün aldığı yanıt diğerlerine de hizmet eder.

- Bir düğüm, en az 60 saniye taze kalacak bir yanıtı cluster veri deposuna yazar.
- Temizleme paylaşılan yanıtları da siler ve her düğüm kendi yerel cache'ini temizler.
- Saklanan her yanıt veri deposuna bir yazma demektir, yani yoğun bir cache veri deposunu compaction'lar arasında büyütür.

Sınırları [Paylaşılan cache](@/cluster.tr.md#paylasilan-cache) bölümü anlatır.

## Sonraki adımlar

- [Cache](@/configuration.tr.md#cache) ve [Sıkıştırma](@/configuration.tr.md#sikistirma): bütün alanlar ve bütün koşullar.
- [İstemci IP adresi](@/configuration.tr.md#istemci-ip-adresi): CDN arkasındaki header sırası.
- [Yük dengeleme ve sağlık kontrolü](@/configuration.tr.md#yuk-dengeleme-ve-saglik-kontrolu): tek bir cache arkasında daha çok upstream sunucu.
