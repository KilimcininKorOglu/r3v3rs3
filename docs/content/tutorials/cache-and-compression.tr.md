+++
title = "Cache ve compression"
description = "Tekrarlanan request'leri memory'den karşılayın ve daha küçük response gönderin"
weight = 8
+++

# Cache ve compression

Bu rehber bir siteyi iki proxy ayarıyla hızlandırır. Cache, tekrarlanan bir request'i memory'den karşılar ve upstream sunucuya hiç gitmez. Compression her client'a daha küçük bir body gönderir.

İkisi birlikte çalışır: r3v3rs3 tek bir sıkıştırılmamış response saklar ve onu her client için ayrı sıkıştırır. Böylece tek bir kayıt hem Brotli isteyen client'a, hem gzip isteyen client'a, hem de compression istemeyen client'a hizmet eder.

Önce [Başlangıç](@/tutorials/getting-started.tr.md) rehberini izleyin. Bu rehber çalışan bir proxy ile devam eder.

## Adım 1: Compression'ı açın

1. Proxy'yi açın ve **Compression** bölümünü bulun.
2. **Brotli**, **Zstandard** ve **Gzip** seçin. Seçim sıranız, client birkaç encoding'i aynı `q` değeriyle kabul ettiğinde r3v3rs3'ün tercih ettiği sıradır.
3. **Minimum Boyut (Byte)** alanını `1024` bırakın. Daha küçük bir body sıkıştırmayla küçülmez.
4. Kaydedin.

```bash
$ curl -s http://app.example.com/data.json -o /dev/null -w '%{size_download}\n'
12591

$ curl -s -H 'Accept-Encoding: br' http://app.example.com/data.json -o /dev/null -w '%{size_download}\n'
711
```

r3v3rs3 bir response'u yalnız media type'ı **Media Type'lar** listesindeyse sıkıştırır. Varsayılan liste `text/*`, `application/json`, `application/javascript`, `application/wasm`, XML type'ları, `image/svg+xml` ve iki font type'ını tutar. Bir görsel veya video zaten sıkıştırılmıştır, bu yüzden listede yoktur.

Upstream sunucu response'u zaten encode ettiyse, `Cache-Control` içinde `no-transform` varsa ve `text/event-stream` için r3v3rs3 sıkıştırma yapmaz. Sıkıştırma server-sent event'leri geciktirir.

## Adım 2: Cache'i açın

1. Proxy'yi açın ve **Cache** bölümünü bulun.
2. **Cache'i Aç** seçeneğini açın.
3. **Memory Limiti (Byte)** alanını `67108864` (64 MiB), **Maksimum Response Boyutu (Byte)** alanını `1048576` (1 MiB) bırakın.
4. **Varsayılan TTL (Saniye)** alanına `300` yazın.
5. Kaydedin.

Proxy'nin bütün route'ları tek bir cache'i paylaşır. Her proxy'nin kendi cache'i vardır.

```bash
$ curl -sI http://app.example.com/data.json | grep -i x-cache
x-cache: MISS

$ curl -sI http://app.example.com/data.json | grep -iE 'x-cache|age'
x-cache: HIT
age: 0
```

`X-Cache: MISS` response'un upstream sunucudan geldiği, `HIT` ise memory'den geldiği anlamına gelir. `Age`, upstream sunucunun response'u gönderdiği andan beri geçen saniyeyi sayar.

## Adım 3: TTL'i anlayın

Saklanan bir response'un ömrü şunlardan var olan ilkinden gelir: `s-maxage`, `max-age`, `Expires`, sonra **Varsayılan TTL (Saniye)**.

Yani kararı upstream sunucu verir, **Varsayılan TTL (Saniye)** de hiçbir şey söylemeyen response'ları kapsar. Yapabildiğiniz yerde header'ları uygulamanızda ayarlayın:

```text
Cache-Control: public, max-age=300
```

**Varsayılan TTL (Saniye)** alanı `0` ise, r3v3rs3 böyle bir response'u yalnız `ETag` veya `Last-Modified` header'ı varsa saklar ve her request'te yeniden doğrular. Bu her client request'i için bir conditional request demektir, ama hiçbir zaman bayat içerik sunmaz.

Saklanan bir response bayatladıysa ve validator'ı varsa, r3v3rs3 `If-None-Match` veya `If-Modified-Since` ile conditional request gönderir. `304 Not Modified` cevabı saklanan header'ları tazeler, client da body'yi yeniden indirmeden alır.

## Adım 4: Nelerin saklanmadığını bilin

r3v3rs3 bir response'u yalnız bütün koşullar sağlanınca saklar. Sürekli `MISS` görmenin sık nedenleri:

- Request `GET` veya `HEAD` değildir, ya da `Range`, `Upgrade` veya `Cache-Control: no-store` taşır.
- Response `Set-Cookie` header'ı taşır. Session response'u paylaşılmamalıdır.
- `Cache-Control` içinde `no-store` veya `private` vardır.
- Response `Vary: *` taşır.
- Request `Authorization` header'ı taşır ve `Cache-Control` içinde `public`, `s-maxage`, `must-revalidate` değerlerinden hiçbiri yoktur.
- Body, **Maksimum Response Boyutu (Byte)** değerinden büyüktür.
- Status şunlardan biri değildir: `200`, `203`, `204`, `300`, `301`, `308`, `404`, `405`, `410`, `414`, `501`.

Cache key'i, istenen host ile request'in path'i ve query'sidir. Load balancing key'i değiştirmez, yani bütün upstream sunucular tek bir kaydı paylaşır. Response, request header adlarını taşıyan bir `Vary` header'ı içeriyorsa r3v3rs3 saklanan response'u yalnız o header'ları aynı değerlerle taşıyan request'e verir.

## Adım 5: Deploy'dan sonra temizleyin

Deploy dosyaları değiştirir, ama saklanan response'lar TTL'lerini korur.

- Proxy listesinde **Temizle** linkine tıklayın.
- Veya `DELETE /api/proxies/{id}/cache` gönderin.

```bash
$ curl -b session.txt -X DELETE http://127.0.0.1:46492/api/proxies/hsy-cns/cache
$ curl -sI http://app.example.com/data.json | grep -i x-cache
x-cache: MISS
```

r3v3rs3'ün restart'ı ve cache ayarlarının değişmesi de saklanan bütün response'ları siler. Saklanan response'lar yalnız memory'de durur.

## Adım 6: CDN arkasında gerçek client IP'sini görün

r3v3rs3 önündeki bir CDN de cache tutar ve ziyaretçi adresini proxy'den gizler. Doğru ayar olmadan rate limit ve IP filtresi edge sunucuyu görür.

r3v3rs3 sekiz bilinen CDN'in IP range'lerine varsayılan olarak güvenir: Cloudflare, Fastly, Amazon CloudFront, Bunny CDN, Gcore, KeyCDN, Imperva ve Google Cloud Load Balancing. Güvenilen bir peer için provider header'ını okur, örneğin `CF-Connecting-IP`, ve çözümlenen adresi upstream sunucuya `X-Real-IP` header'ında gönderir.

- Range'ler binary'nin içine derlenir ve her gün yeniden indirilir. **Ayarlar** sayfası listenin durumunu gösterir ve bir **Şimdi Yenile** butonu taşır.
- Kendi load balancer'ınızı proxy'nin **Güvenilen Proxy'ler** alanına ekleyin.
- Akamai edge range'lerini yayınlamaz. Site Shield range'lerinizi **Güvenilen Proxy'ler** alanına yazın.

## Cluster'da

Her node'un kendi cache'i vardır. `share_cache = true` ile node'lar response'ları cluster store'a da yazar, böylece bir node'un aldığı response diğerlerine de hizmet eder.

- Bir node, en az 60 saniye taze kalacak bir response'u cluster store'a yazar.
- Temizleme paylaşılan response'ları da siler ve her node kendi yerel cache'ini temizler.
- Saklanan her response store'a bir write demektir, yani yoğun bir cache store'u compaction'lar arasında büyütür.

Sınırları [Paylaşılan cache](@/cluster.tr.md#paylasilan-cache) bölümü anlatır.

## Sonraki adımlar

- [Cache](@/configuration.tr.md#cache) ve [Compression](@/configuration.tr.md#compression): bütün alanlar ve bütün koşullar.
- [Client IP](@/configuration.tr.md#client-ip): CDN arkasındaki header sırası.
- [Load balancing ve health check](@/configuration.tr.md#load-balancing-ve-health-check): tek bir cache arkasında daha çok upstream sunucu.
