+++
title = "Load balancing ve health check"
description = "Trafiği birkaç uygulama sunucusuna dağıtın ve bozulan sunucuyu devre dışı bırakın"
weight = 11
+++

# Load balancing ve health check

Bu rehber bir uygulamanın üç kopyasını tek bir proxy'nin arkasına koyar. Her kopyaya trafiğin bir payını verirsiniz, bozulan kopyayı r3v3rs3 bulur, her client tek sunucuda kalır ve bir sunucuyu restart olmadan devre dışı bırakırsınız.

[Başlangıç](@/tutorials/getting-started.tr.md) rehberindeki bir port ve bir proxy gerekir.

## Adım 1: Sunucuları ekleyin

HTTP proxy'nizi açın ve route'a sunucuları ekleyin:

```
http://10.0.0.1:9000/
http://10.0.0.2:9000/
http://10.0.0.3:9000/
```

**Load Balancing** alanı proxy'nin policy'sini seçer:

| Policy | Ne yapar |
|---|---|
| Round robin | Sunucuları sırayla kullanır. Varsayılan. |
| Rastgele | Rastgele bir sunucu seçer. |
| İlk sağlıklı sunucu | İlk sunucuyu kullanır, diğerleri yedektir. |
| Client IP hash | Her client IP adresini aynı sunucuya gönderir. |

HTTP proxy her request için, TCP proxy her bağlantı için, UDP proxy her client session'ı için sunucu seçer. Her HTTP route'unun sunucuları ayrı bir gruptur, yani bir proxy'nin iki route'u farklı sunucu listeleri kullanabilir.

## Adım 2: Bir sunucuya daha çok trafik verin

Bir sunucunun **Weight** değeri 0 ile 65535 arasında bir tam sayıdır, varsayılanı 1'dir. Round robin her sunucuyu weight değeri kadar kullanır ve sıraları döngüye yayar, arka arkaya göndermez.

Weight değerleri 1 ve 3 olan iki sunucu şu sırayla cevap verir:

```
server-2 server-1 server-2 server-2 server-2 server-1 server-2 server-2
```

Her dört request'in üçü weight değeri 3 olan sunucuya gider. Bir makine diğerinden büyükse bunu kullanın.

Rastgele seçeneği sunucuyu weight değeriyle orantılı bir olasılıkla seçer. Client IP hash her sunucuya client adreslerinin weight ile orantılı bir payını verir. İlk sağlıklı sunucu seçeneği weight değerini yok sayar.

## Adım 3: Bozulan sunucuyu bulun

Aktif health check kapalıyken r3v3rs3 bir hatayı yalnız trafikten öğrenir. Art arda **Maksimum Hata Sayısı** kadar hata (varsayılan 1) alan sunucu, **Sağlıksız Kalma Süresi** boyunca (varsayılan 30 saniye) sağlıksız sayılır. Hata, kurulamayan bir bağlantı veya response gelmeyen bir request'tir. 500 response'u başarı sayılır, çünkü sunucu cevap vermiştir.

Ayakta olan ama bozuk çalışan bir sunucuyu bulmak için aktif kontrolü açın:

1. **Kontrol Aralığı (Saniye)** değerini `2` yapın.
2. **Health Check Path** alanına `/health` yazın.
3. **Kontrol Timeout'u (Saniye)** değerini `5` bırakın.

r3v3rs3 her aralıkta her sunucuya `GET /health` gönderir. 2xx veya 3xx status kontrolü geçer. Path boşsa HTTP proxy ve TCP proxy bir TCP bağlantısı açar, UDP proxy sunucunun host adını çözer.

Sonucu status API'sinden sorun:

```bash
$ curl -s -b cookies.txt http://127.0.0.1:46492/api/proxies/<proxy-id>/status
```

`/health` isteğine 500 dönen sunucu şöyle görünür:

```json
{
  "url": "http://10.0.0.3:9000/",
  "weight": 1,
  "healthy": false,
  "failures": 0,
  "last_error": "the active check received status 500 Internal Server Error"
}
```

O andan sonra her request iki sağlıklı sunucuya gider. Sunucu ancak bir kontrolü geçince geri döner. Başarılı bir request bu durumu bitirmez, yalnız geçen bir kontrol bitirir.

WebUI'daki proxy listesi `2/3 sağlıklı` yazar ve sağlıksız sunucuyu sayının başlığında gösterir. Liste 10 saniyede bir yenilenir.

Bütün sunucular sağlıksızsa r3v3rs3 trafiği yine onlara gönderir. Kendiliğinden 503 dönmez.

## Adım 4: Başarısız request'i tekrar gönderin

**Deneme Sayısı** (varsayılan 2), ilk deneme dahil bir request'in kaç kez denendiğidir. **Retry Koşulları** hangi hataların retry başlattığını belirler:

- Kurulamayan bağlantı: bağlantı kurulamaz veya connect timeout dolar. Sunucu hiçbir şey almadığı için her method tekrar denenir.
- Request timeout, 502, 503, 504: yalnız idempotent method'lar (`GET`, `HEAD`, `OPTIONS`, `TRACE`, `PUT`, `DELETE`) tekrar denenir.

Retry sıradaki sunucuya gider ve circuit'i açık olan sunucuyu atlar. Üç deneme ve listede ölü bir sunucu varken her client yine 200 alır, status API'si o sunucunun hatalarını sayar:

```json
{ "url": "http://10.0.0.9:9000/", "healthy": false, "failures": 1, "last_error": "client error (Connect)" }
```

Body'si olan bir request yalnız body uzunluğu biliniyorsa ve **Replay Body Limiti (Byte)** değerinden küçükse (varsayılan 0) tekrar denenir. r3v3rs3 böyle bir body'yi memory'de tutar. WebSocket ve diğer upgrade request'leri hiç tekrar denenmez. Bir route **Bu Route için Ayrı Retry Ayarları Kullan** seçeneğiyle kendi ayarlarını kullanabilir.

## Adım 5: Client'ı tek sunucuda tutun

Session'ı tek bir sunucunun memory'sinde tutan uygulama sticky session ister.

1. Route'ta **Sticky Cookie'yi Aç** seçeneğini işaretleyin.
2. **Cookie Adı** alanına `app_server` yazın.
3. **Cookie Max-Age (Saniye)** değerini `3600` yapın.

İlk response cookie'yi yazar:

```
set-cookie: app_server=1f8a...; Path=/; HttpOnly; SameSite=Lax; Max-Age=3600
```

Bu cookie'yi taşıyan sonraki her request aynı sunucuya gider. Değer; proxy, route ve sunucu URL'inin HMAC imzasıdır, bu yüzden client sahte bir değerle sunucu seçemez. r3v3rs3 cookie'yi request'ten çıkarır, upstream sunucu onu hiç görmez.

Sunucusu sağlıksız olan veya circuit'i açılan client başka sunucuya geçer. r3v3rs3 her başlangıçta yeni bir imza key'i üretir, yani restart bütün cookie'leri geçersiz kılar. Tarayıcı kapanınca silinen bir cookie için Max-Age değerini 0 yapın.

TCP ve UDP proxy'de cookie yoktur. Orada **Client IP hash** kullanın.

## Adım 6: Bir sunucuyu devre dışı bırakın

Sunucunun **Weight** değerini `0` yapın ve kaydedin. Sunucu yeni trafik almaz, diğer bütün sunucular sağlıksız olsa da almaz. Açık TCP bağlantıları ve UDP session'ları kalır, sticky client'ları da kalır, yani üzerindeki session'lar kendiliğinden biter.

En az bir sunucunun weight değeri 0'dan büyük olmalıdır. r3v3rs3, bütün sunucuları devre dışı bırakılmış bir route'u kabul etmez.

Deployment böyle yapılır: bir sunucuyu devre dışı bırakın, session'larının bitmesini bekleyin, sunucuyu yükseltin, weight değerini geri verin.

## Adım 7: Bozuk sunucuya trafiği kesin

Circuit breaker varsayılan olarak kapalıdır. Proxy'de **Circuit Breaker'ı Aç** seçeneğini işaretleyin:

| Alan | Varsayılan | Anlamı |
|---|---|---|
| **Hata Oranı (%)** | 50 | Circuit'i açan başarısız request oranı. |
| **Minimum Request Sayısı** | 20 | Oranın sayılması için pencerede gereken request sayısı. |
| **Zaman Penceresi (Saniye)** | 10 | Bir sayma penceresinin uzunluğu. |
| **Açık Kalma Süresi (Saniye)** | 30 | Circuit açıldıktan sonra trafiksiz geçen süre. |

Bağlantı kurulamazsa, bir timeout dolarsa veya sunucu 502, 503 ya da 504 dönerse request başarısız sayılır. Açık kalma süresi dolunca tek bir deneme request'i sunucuyu test eder: başarı circuit'i kapatır, hata yeniden açar. Bir route'un bütün sunucularının circuit'i açıksa client, hiçbir sunucuya request gitmeden 503 Service Unavailable alır.

Status API'si `"circuit": "open"` veya `"circuit": "half_open"` gösterir. UDP proxy'lerde circuit breaker yoktur.

## Adım 8: Trafiğin kopyasını yeni sürüme gönderin

**Bu Route'un Request'lerini Mirror Sunuculara Kopyala** seçeneği route'un request'lerini başka sunuculara kopyalar, böylece yeni sürümü gerçek trafikle test edersiniz:

1. Yeni sürümü **Mirror Sunucular** listesine ekleyin.
2. **Kopyalanan Request'ler (%)** değerini `10` yapın.

Her mirror sunucu, kopyalanan request'in aynı method, header, path ve body ile bir kopyasını alır. r3v3rs3 onların response'larını atar, retry etmez, beklemez ve health check'lerde saymaz. Client, route sunucusunun response'unu önceki gibi alır.

Kopya, r3v3rs3 request body'sinin tamamını okuduktan sonra gider. Cache'ten gelen response'lar, upgrade request'leri, maksimum body boyutundan (varsayılan 65536 byte) büyük body'ler ve 64 kopya gönderilirken gelen request'ler kopyalanmaz. Kopya, kimlik doğrulamanın kaldırmadığı cookie gibi credential'ları taşır. Bu yüzden yalnız bu veriyle güvendiğiniz bir sunucuya mirror yapın.

## WebSocket ve gRPC

İkisi de aynı HTTP proxy üzerinden çalışır, ama farklı ayarlar ister.

### WebSocket

WebSocket proxy'si için ayar gerekmez. r3v3rs3, HTTP ve HTTPS proxy'lerinde upgrade'i ve frame'leri iki yönde de aktarır:

```
ws://app.example.com/socket  -> http://10.0.0.1:9000/ sunuculu bir route
wss://app.example.com/socket -> aynı route, HTTPS portunda
```

WebSocket bağlantısında iki şey değişir:

- Bağlantı hiç retry edilmez ve kopyalanmaz, çünkü body bitmez.
- h2c açık olsa da upstream'e her zaman HTTP/1.1 ile gidilir.

Bağlantılarınız timeout'tan uzun süre açık kalıyorsa route'un request timeout değerini `0` yapın.

### gRPC

gRPC uçtan uca HTTP/2 ister.

HTTPS portunda client'lar HTTP/2'yi ALPN ile alır. Upstream tarafında:

- HTTPS sunucu ALPN ile `h2` seçer, ek ayar gerekmez.
- Düz HTTP sunucu HTTP/1.1 alır ve gRPC bunu kullanamaz. Proxy'de **Düz HTTP Sunucuları için HTTP/2 Kullan (h2c)** seçeneğini açın.

h2c seçeneğini yalnız proxy'nin bütün düz HTTP sunucuları prior knowledge ile HTTP/2 kabul ediyorsa açın. Kabul etmeyen sunucu ilk request'te hata döner.

Aynı client certificate'ı ve aynı connect timeout'u kullanan proxy'ler upstream bağlantılarını paylaşır, yani tek bir HTTP/2 bağlantısı çok sayıda client'ın stream'ini taşır.

## Referans

- [Load Balancing ve Health Check](@/configuration.tr.md#load-balancing-ve-health-check): her alan ve varsayılanı.
- [Circuit Breaker](@/configuration.tr.md#circuit-breaker) ve [Sticky Session'lar](@/configuration.tr.md#sticky-session-lar).
- [HTTP/2](@/configuration.tr.md#http-2), [WebSocket](@/configuration.tr.md#websocket) ve [Trafik Mirroring](@/configuration.tr.md#trafik-mirroring).

## Sonraki adımlar

- [Bir uygulamayı korumaya alma](@/tutorials/protect-an-app.tr.md): erişim listesi, kimlik doğrulama ve rate limit.
- [Cache ve compression](@/tutorials/cache-and-compression.tr.md): sunuculara daha az request.
- [Yüksek erişilebilirlik](@/tutorials/high-availability.tr.md): tek state paylaşan birkaç r3v3rs3 node'u.
