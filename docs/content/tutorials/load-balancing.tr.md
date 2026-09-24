+++
title = "Yük dengeleme ve sağlık kontrolü"
description = "İstekleri birkaç uygulama sunucusuna dağıtın ve bozulan sunucuyu devre dışı bırakın"
weight = 11
+++

# Yük dengeleme ve sağlık kontrolü

Bu rehber bir uygulamanın üç kopyasını tek bir proxy'nin arkasına koyar. Her kopyaya isteklerin bir payını verirsiniz, bozulan kopyayı r3v3rs3 bulur, her istemci tek sunucuda kalır ve bir sunucuyu yeniden başlatma olmadan devre dışı bırakırsınız.

[Başlangıç](@/tutorials/getting-started.tr.md) rehberindeki bir port ve bir proxy gerekir.

## Adım 1: Sunucuları ekleyin

HTTP proxy'nizi açın ve route'a sunucuları ekleyin:

```
http://10.0.0.1:9000/
http://10.0.0.2:9000/
http://10.0.0.3:9000/
```

**Yük Dengeleme** alanı proxy'nin yük dengeleme yöntemini seçer:

| Yöntem | Ne yapar |
|---|---|
| Round robin | Sunucuları sırayla kullanır. Varsayılan. |
| Rastgele | Rastgele bir sunucu seçer. |
| İlk sağlıklı sunucu | İlk sunucuyu kullanır, diğerleri yedektir. |
| İstemci IP hash'i | Her istemci IP adresini aynı sunucuya gönderir. |

HTTP proxy her istek için, TCP proxy her bağlantı için, UDP proxy her istemci oturumu için sunucu seçer. Her HTTP route'unun sunucuları ayrı bir gruptur, yani bir proxy'nin iki route'u farklı sunucu listeleri kullanabilir.

## Adım 2: Bir sunucuya daha çok istek verin

Bir sunucunun **Ağırlık** değeri 0 ile 65535 arasında bir tam sayıdır, varsayılanı 1'dir. Round robin her sunucuyu ağırlığı kadar kullanır ve sıraları döngüye yayar, arka arkaya göndermez.

Ağırlıkları 1 ve 3 olan iki sunucu şu sırayla yanıt verir:

```
server-2 server-1 server-2 server-2 server-2 server-1 server-2 server-2
```

Her dört isteğin üçü ağırlığı 3 olan sunucuya gider. Bir makine diğerinden büyükse bunu kullanın.

Rastgele seçeneği sunucuyu ağırlığıyla orantılı bir olasılıkla seçer. İstemci IP hash'i her sunucuya istemci adreslerinin ağırlığıyla orantılı bir payını verir. İlk sağlıklı sunucu seçeneği ağırlığı yok sayar.

## Adım 3: Bozulan sunucuyu bulun

Aktif sağlık kontrolü kapalıyken r3v3rs3 bir hatayı yalnız gelen isteklerden öğrenir. Art arda **Maksimum Hata Sayısı** kadar hata (varsayılan 1) alan sunucu, **Sağlıksız Kalma Süresi** boyunca (varsayılan 30 saniye) sağlıksız sayılır. Hata, kurulamayan bir bağlantı veya yanıtı gelmeyen bir istektir. 500 yanıtı başarı sayılır, çünkü sunucu yanıt vermiştir.

Ayakta olan ama bozuk çalışan bir sunucuyu bulmak için aktif kontrolü açın:

1. **Kontrol Aralığı (Saniye)** değerini `2` yapın.
2. **Sağlık Kontrolü Yolu** alanına `/health` yazın.
3. **Kontrol Timeout'u (Saniye)** değerini `5` bırakın.

r3v3rs3 her aralıkta her sunucuya `GET /health` gönderir. 2xx veya 3xx durum kodu kontrolü geçer. Yol boşsa HTTP proxy ve TCP proxy bir TCP bağlantısı açar, UDP proxy sunucunun host adını çözümler.

Sonucu durum API'sinden sorun:

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

O andan sonra her istek iki sağlıklı sunucuya gider. Sunucu ancak bir kontrolü geçince geri döner. Başarılı bir istek bu durumu bitirmez, yalnız geçen bir kontrol bitirir.

WebUI'daki proxy listesi `2/3 sağlıklı` yazar ve sağlıksız sunucuyu sayının başlığında gösterir. Liste 10 saniyede bir yenilenir.

Bütün sunucular sağlıksızsa r3v3rs3 istekleri yine onlara gönderir. Kendiliğinden 503 dönmez.

## Adım 4: Başarısız isteği yeniden deneyin

**Deneme Sayısı** (varsayılan 2), ilk deneme dahil bir isteğin kaç kez denendiğidir. **Yeniden Deneme Koşulları** hangi hataların yeniden denemeyi başlattığını belirler:

- Kurulamayan bağlantı: bağlantı kurulamaz veya bağlantı timeout'u dolar. Sunucu hiçbir şey almadığı için her method yeniden denenir.
- İstek timeout'u, 502, 503, 504: yalnız idempotent method'lar (`GET`, `HEAD`, `OPTIONS`, `TRACE`, `PUT`, `DELETE`) yeniden denenir.

Yeniden deneme sıradaki sunucuya gider ve circuit'i açık olan sunucuyu atlar. Üç deneme ve listede ölü bir sunucu varken her istemci yine 200 alır, durum API'si o sunucunun hatalarını sayar:

```json
{ "url": "http://10.0.0.9:9000/", "healthy": false, "failures": 1, "last_error": "client error (Connect)" }
```

Gövdesi olan bir istek yalnız gövde uzunluğu biliniyorsa ve **Yeniden Gönderim Gövde Limiti (Byte)** değerini aşmıyorsa (varsayılan 0) yeniden denenir. r3v3rs3 böyle bir gövdeyi bellekte tutar. WebSocket ve diğer upgrade istekleri hiç yeniden denenmez. Bir route **Bu Route için Ayrı Yeniden Deneme Ayarları Kullan** seçeneğiyle kendi ayarlarını kullanabilir.

## Adım 5: İstemciyi tek sunucuda tutun

Oturumu tek bir sunucunun belleğinde tutan uygulama sticky session ister.

1. Route'ta **Sticky Cookie'yi Aç** seçeneğini işaretleyin.
2. **Cookie Adı** alanına `app_server` yazın.
3. **Cookie Max-Age (Saniye)** değerini `3600` yapın.

İlk yanıt cookie'yi yazar:

```
set-cookie: app_server=1f8a...; Path=/; HttpOnly; SameSite=Lax; Max-Age=3600
```

Bu cookie'yi taşıyan sonraki her istek aynı sunucuya gider. Değer; proxy, route ve sunucu URL'inin HMAC imzasıdır, bu yüzden istemci sahte bir değerle sunucu seçemez. r3v3rs3 cookie'yi istekten çıkarır, upstream sunucu onu hiç görmez.

Sunucusu sağlıksız olan veya circuit'i açılan istemci başka sunucuya geçer. r3v3rs3 her başlangıçta yeni bir imza key'i üretir, yani yeniden başlatma bütün cookie'leri geçersiz kılar. Tarayıcı kapanınca silinen bir cookie için Max-Age değerini 0 yapın.

TCP ve UDP proxy'de cookie yoktur. Orada **İstemci IP hash'i** kullanın.

## Adım 6: Bir sunucuyu devre dışı bırakın

Sunucunun **Ağırlık** değerini `0` yapın ve kaydedin. Sunucu yeni istek almaz, diğer bütün sunucular sağlıksız olsa da almaz. Açık TCP bağlantıları ve UDP oturumları kalır, sticky istemcileri de kalır, yani üzerindeki oturumlar kendiliğinden biter.

En az bir sunucunun ağırlığı 0'dan büyük olmalıdır. r3v3rs3, bütün sunucuları devre dışı bırakılmış bir route'u kabul etmez.

Deployment böyle yapılır: bir sunucuyu devre dışı bırakın, oturumlarının bitmesini bekleyin, sunucuyu yükseltin, ağırlığını geri verin.

## Adım 7: Bozuk sunucuya giden istekleri kesin

Circuit breaker varsayılan olarak kapalıdır. Proxy'de **Circuit Breaker'ı Aç** seçeneğini işaretleyin:

| Alan | Varsayılan | Anlamı |
|---|---|---|
| **Hata Oranı (%)** | 50 | Circuit'i açan başarısız istek oranı. |
| **Minimum İstek Sayısı** | 20 | Oranın sayılması için pencerede gereken istek sayısı. |
| **Zaman Penceresi (Saniye)** | 10 | Bir sayma penceresinin uzunluğu. |
| **Açık Kalma Süresi (Saniye)** | 30 | Circuit açıldıktan sonra sunucunun istek almadığı süre. |

Bağlantı kurulamazsa, bir timeout dolarsa veya sunucu 502, 503 ya da 504 dönerse istek başarısız sayılır. Açık kalma süresi dolunca tek bir deneme isteği sunucuyu test eder: başarı circuit'i kapatır, hata yeniden açar. Bir route'un bütün sunucularının circuit'i açıksa istemci, hiçbir sunucuya istek gitmeden 503 Service Unavailable alır.

Durum API'si `"circuit": "open"` veya `"circuit": "half_open"` gösterir. UDP proxy'lerde circuit breaker yoktur.

## Adım 8: İsteklerin kopyasını yeni sürüme gönderin

**Bu Route'un İsteklerini Yansıtma Sunucularına Kopyala** seçeneği route'un isteklerini başka sunuculara kopyalar, böylece yeni sürümü gerçek isteklerle test edersiniz:

1. Yeni sürümü **Yansıtma Sunucuları** listesine ekleyin.
2. **Kopyalanan İstekler (%)** değerini `10` yapın.

Her yansıtma sunucusu, kopyalanan isteğin aynı method, header, yol ve gövde ile bir kopyasını alır. r3v3rs3 onların yanıtlarını atar, yeniden denemez, beklemez ve sağlık kontrollerinde saymaz. İstemci, route sunucusunun yanıtını önceki gibi alır.

Kopya, r3v3rs3 istek gövdesinin tamamını okuduktan sonra gider. Cache'ten gelen yanıtlar, upgrade istekleri, maksimum gövde boyutundan (varsayılan 65536 byte) büyük gövdeler ve 64 kopya gönderilirken gelen istekler kopyalanmaz. Kopya, kimlik doğrulamanın kaldırmadığı cookie gibi kimlik bilgilerini taşır. Bu yüzden istekleri yalnız bu veriyle güvendiğiniz bir sunucuya yansıtın.

## WebSocket ve gRPC

İkisi de aynı HTTP proxy üzerinden çalışır, ama farklı ayarlar ister.

### WebSocket

WebSocket proxy'si için ayar gerekmez. r3v3rs3, HTTP ve HTTPS proxy'lerinde upgrade'i ve frame'leri iki yönde de aktarır:

```
ws://app.example.com/socket  -> http://10.0.0.1:9000/ sunuculu bir route
wss://app.example.com/socket -> aynı route, HTTPS portunda
```

WebSocket bağlantısında iki şey değişir:

- Bağlantı hiç yeniden denenmez ve kopyalanmaz, çünkü gövde bitmez.
- h2c açık olsa da upstream sunucuya her zaman HTTP/1.1 ile gidilir.

Bağlantılarınız timeout'tan uzun süre açık kalıyorsa route'un istek timeout'unu `0` yapın.

### gRPC

gRPC uçtan uca HTTP/2 ister.

HTTPS portunda istemciler HTTP/2'yi ALPN ile alır. Upstream tarafında:

- HTTPS sunucu ALPN ile `h2` seçer, ek ayar gerekmez.
- Düz HTTP sunucu HTTP/1.1 alır ve gRPC bunu kullanamaz. Proxy'de **Düz HTTP Sunucuları için HTTP/2 Kullan (h2c)** seçeneğini açın.

h2c seçeneğini yalnız proxy'nin bütün düz HTTP sunucuları prior knowledge ile HTTP/2 kabul ediyorsa açın. Kabul etmeyen sunucu ilk istekte hata döner.

Bir portun aynı istemci sertifikasını ve aynı bağlantı timeout'unu kullanan proxy'leri upstream bağlantılarını paylaşır, yani tek bir HTTP/2 bağlantısı çok sayıda istemcinin stream'ini taşır.

## Referans

- [Yük dengeleme ve sağlık kontrolü](@/configuration.tr.md#yuk-dengeleme-ve-saglik-kontrolu): her alan ve varsayılanı.
- [Circuit breaker](@/configuration.tr.md#circuit-breaker) ve [Sticky session'lar](@/configuration.tr.md#sticky-session-lar).
- [HTTP/2](@/configuration.tr.md#http-2), [WebSocket](@/configuration.tr.md#websocket) ve [İstek yansıtma](@/configuration.tr.md#istek-yansitma).

## Sonraki adımlar

- [Bir uygulamayı korumaya alma](@/tutorials/protect-an-app.tr.md): erişim listesi, kimlik doğrulama ve rate limit.
- [Cache ve sıkıştırma](@/tutorials/cache-and-compression.tr.md): sunuculara daha az istek.
- [Yüksek erişilebilirlik](@/tutorials/high-availability.tr.md): tek durum bilgisini paylaşan birkaç r3v3rs3 düğümü.
