+++
title = "Config"
description = "Config"
weight = 0
+++

# Portlar

Proxy yapılandırmadan önce dinlenecek bir port bağlamanız gerekir. Bunu "Portlar" bölümünde yapabilirsiniz.

r3v3rs3 altı port türünü destekler:

- HTTP
- HTTPS (TLS üzerinden HTTP)
- QUIC üzerinden HTTP (HTTP/3)
- TCP
- TLS üzerinden TCP
- UDP

## Portu Sıfırlama

Port config'ini değiştirmek mevcut bağlantıları etkilemez. Eski bağlantılar eski config'i kullanmaya devam eder. Mevcut bağlantıları zorla kapatmak için portu sıfırlayabilirsiniz.

# Proxy'ler

r3v3rs3 üç proxy türünü destekler:

- HTTP / HTTPS
- TCP / TLS üzerinden TCP
- UDP

Bir proxy'ye birden fazla port bağlanabilir. Ancak TCP / TLS üzerinden TCP portları bir HTTP / HTTPS proxy'sine bağlanamaz. Bunun tersi de mümkün değildir.

## Client IP

Bir CDN veya load balancer arkasında r3v3rs3'ün TCP peer'ı ziyaretçi değil, edge sunucusudur. r3v3rs3 gerçek client IP adresini yalnız peer güvenilir olduğunda çözümler:

- **Bilinen CDN'ler**: Cloudflare, Fastly, Amazon CloudFront, Bunny CDN, Gcore, KeyCDN, Imperva ve Google Cloud Load Balancing. Bu özellik varsayılan olarak açıktır. Kapatmak için proxy ayarlarında "Bilinen CDN'lerin Client IP Header'larına Güven" seçeneğini kapatın.
- **Güvenilen Proxy'ler**: Proxy'ye eklediğiniz IP adresleri veya CIDR blokları, örneğin yerel bir load balancer.

Güvenilen bir peer için r3v3rs3 client IP adresini şu sırayla okur:

1. Sağlayıcı header'ı: Cloudflare için `CF-Connecting-IP`, Bunny CDN için `X-Real-IP`, Amazon CloudFront için `CloudFront-Viewer-Address`.
2. `X-Forwarded-For` içinde güvenilen bir proxy veya bilinen bir CDN edge'i olmayan en sağdaki adres.

r3v3rs3 ardından çözümlenen adresi upstream sunucuya `X-Real-IP` header'ında gönderir ve gelen `X-Forwarded-For` ve `Forwarded` zincirlerini korur. Güvenilmeyen bir peer için r3v3rs3 `Forwarded`, `X-Forwarded-For`, `X-Real-IP`, `CF-Connecting-IP`, `True-Client-IP`, `CloudFront-Viewer-Address`, `Fastly-Client-IP` ve `Incap-Client-IP` header'larını siler, çünkü client bu header'ları sahte değerlerle gönderebilir.

CDN IP aralıkları binary içine derlenir ve her gün yeniden indirilir. Son indirilen liste config dizinindeki `cdn-ranges.json` dosyasına kaydedilir. İndirme başarısız olursa r3v3rs3 bilinen son listeyi kullanmaya devam eder. "Ayarlar" bölümü listenin durumunu gösterir ve bir "Şimdi Yenile" butonu içerir.

Akamai edge IP aralıklarını yayınlamaz. Akamai Site Shield aralıklarınızı bunun yerine "Güvenilen Proxy'ler" listesine ekleyin.

## IP Filtresi

Her HTTP / HTTPS proxy'si için client'lara IP adresine göre izin verebilir veya client'ları engelleyebilirsiniz. r3v3rs3 "Client IP" bölümünün çözümlediği client IP adresini kontrol eder. Bu yüzden filtre bir CDN veya güvenilen bir proxy arkasında da çalışır.

- **Engellenen IP Adresleri**: Bu IP adreslerindeki veya CIDR bloklarındaki client'lar `403 Forbidden` alır.
- **İzin Verilen IP Adresleri**: Liste boş değilse proxy'ye yalnız bu IP adreslerindeki veya CIDR bloklarındaki client'lar erişebilir. Diğer client'lar `403 Forbidden` alır.

Engellenen bir adres, izin verilen bir adresten önce uygulanır.

Bir route "Bu Route için IP Filtresini Değiştir" seçeneği ile proxy listelerinin yerine kendi listelerini kullanabilir. Route bu durumda yalnız kendi listelerini kullanır. İki route listesi de boşsa route her client'a izin verir.

```toml
[my-proxy]
protocol = "http"
vhosts = ["example.com"]
ip_filter = { allow = ["192.168.0.0/16"], deny = ["192.168.10.0/24"] }
routes = [
  { path = "/", servers = [{ url = "http://127.0.0.1:8080/" }] },
  { path = "/public", servers = [{ url = "http://127.0.0.1:8080/public" }], ip_filter = {} },
]
```

## Rate Limit

Her HTTP / HTTPS proxy'si için her client IP adresinin request hızını sınırlayabilirsiniz. r3v3rs3 request'leri "Client IP" bölümünün çözümlediği client IP adresine göre sayar.

- **Request**: Her periyotta izin verilen request sayısı. `0` limiti kapatır.
- **Süre**: Periyot: saniye, dakika veya saat.
- **Burst**: Limit uygulanmadan önce bir client'ın bir anda gönderebileceği request sayısı. `0` "Request" değerini kullanır.

Limiti aşan bir client `Retry-After` header'ı ile birlikte `429 Too Many Requests` alır.

Bir route "Bu Route için Rate Limit'i Değiştir" seçeneği ile proxy limitinin yerine kendi limitini kullanabilir. Bu seçeneği kullanmayan route'lar her client için tek bir sayacı paylaşır. `0` request değerine sahip bir route ayarı limiti o route için kapatır.

r3v3rs3 sayaçları memory'de tutar. Limitin kendisi değişmedikçe bir config değişikliği sayaçları korur. Yeniden başlatma sayaçları sıfırlar.

```toml
[my-proxy]
protocol = "http"
vhosts = ["example.com"]
rate_limit = { requests = 10, per = "second", burst = 20 }
routes = [
  { path = "/", servers = [{ url = "http://127.0.0.1:8080/" }] },
  { path = "/login", servers = [{ url = "http://127.0.0.1:8080/login" }], rate_limit = { requests = 5, per = "minute" } },
]
```

## Kimlik Doğrulama

Her HTTP / HTTPS proxy'si için "Kimlik Doğrulama" bölümünde kimlik doğrulamayı zorunlu tutabilirsiniz. Bir route "Bu Route için Kimlik Doğrulamayı Değiştir" seçeneği ile proxy kimlik doğrulamasının yerine kendi ayarını kullanabilir. O route'ta her client'a izin vermek için bu ayarda "Yok" seçeneğini seçin.

r3v3rs3 kimlik doğrulamayı IP filtresi, rate limit ve HTTPS redirect'inden sonra kontrol eder. Bu yüzden "HTTP'yi Otomatik Olarak HTTPS'e Yönlendir" açıkken tarayıcı kimlik bilgilerini güvenli bağlantı üzerinden gönderir.

### Basic Auth

Geçerli bir kullanıcı adı ve parola göndermeyen client'lar `WWW-Authenticate: Basic realm="..."` header'ı ile birlikte `401 Unauthorized` alır. Tarayıcı ardından bir giriş penceresi gösterir.

- **Realm**: Tarayıcının giriş penceresinde gösterdiği ad. Boş bırakılırsa `r3v3rs3` kullanılır.
- **Kullanıcılar**: Kullanıcı adları ve parolalar. Kullanıcı adı iki nokta üst üste içermemelidir.

r3v3rs3 her parolayı argon2 hash olarak saklar ve düz metin parolayı hiçbir zaman kaydetmez. Mevcut parolayı korumak için parola alanını boş bırakın. r3v3rs3 request'i upstream sunucuya göndermeden önce `Authorization` header'ını siler.

Argon2 bilerek CPU zamanı harcar. r3v3rs3 her kimlik bilgisini bir kez doğrular ve sonucu config değişene kadar memory'de tutar. Parola tahmin denemelerini sınırlamak için rate limit kullanın.

`proxies.toml` dosyasında `password_hash` yerine `password` yazabilirsiniz. r3v3rs3 başlangıçta bu değeri bir hash ile değiştirir.

```toml
[my-proxy]
protocol = "http"
vhosts = ["example.com"]
auth = { type = "basic", realm = "Staff", users = [{ username = "alice", password_hash = "$argon2id$v=19$m=19456,t=2,p=1$..." }] }
routes = [
  { path = "/", servers = [{ url = "http://127.0.0.1:8080/" }] },
  { path = "/health", servers = [{ url = "http://127.0.0.1:8080/health" }], auth = { type = "none" } },
]
```

### Bearer Token

Client'lar proxy token'larından biri ile `Authorization: Bearer <token>` göndermelidir. Token göndermeyen client'lar `WWW-Authenticate: Bearer realm="r3v3rs3"` ile birlikte `401 Unauthorized` alır. Yanlış token gönderen client'lar aynı response'u `error="invalid_token"` ile alır.

- **Ad**: Token'ı tanımlayan bir etiket.
- **Token**: En az 16 karakterlik rastgele bir değer. Örneğin `openssl rand -hex 32` ile bir değer oluşturun.

r3v3rs3 her token'ın SHA-256 digest'ini saklar ve düz metin token'ı hiçbir zaman kaydetmez. Digest'leri sabit sürede karşılaştırır. Mevcut token'ı korumak için token alanını boş bırakın. r3v3rs3 request'i upstream sunucuya göndermeden önce `Authorization` header'ını siler. Bu yüzden bearer kimlik doğrulaması olan bir route'ta upstream sunucu kendi bearer token'ını alamaz.

`proxies.toml` dosyasında `token_hash` yerine `token` yazabilirsiniz. r3v3rs3 başlangıçta bu değeri bir digest ile değiştirir.

```toml
[my-api]
protocol = "http"
vhosts = ["api.example.com"]
auth = { type = "bearer", tokens = [{ name = "ci", token_hash = "<sha-256 hex digest>" }] }
routes = [{ path = "/", servers = [{ url = "http://127.0.0.1:9000/" }] }]
```

### Forward Auth

r3v3rs3 her request'e izin verilip verilmeyeceğini harici bir servise, örneğin oauth2-proxy veya Authelia'ya sorar. Bu özellik nginx'teki `auth_request` gibi çalışır.

Her client request'i için r3v3rs3 auth URL'ine bir `GET` request'i gönderir. Auth request'i, bağlantı header'ları ve `Host` dışındaki client request header'larını taşır. Ayrıca şu header'ları içerir:

| Header | Değer |
|---|---|
| `X-Forwarded-Method` | Client request'inin method'u. |
| `X-Forwarded-Proto` | `http` veya `https`. |
| `X-Forwarded-Host` | Client request'inin host'u. |
| `X-Forwarded-Uri` | Client request'inin path ve query değeri. |
| `X-Forwarded-For` | "Client IP" bölümünün çözümlediği client IP adresi. |

- **2xx response**: r3v3rs3 request'i upstream sunucuya gönderir. "Kopyalanacak Response Header'ları" alanındaki header'ları auth response'undan upstream request'ine kopyalar. Client bu header'ları kendisi gönderemesin diye önce onları client request'inden siler.
- **Diğer response'lar**: r3v3rs3 auth response'unu (status, header'lar ve en fazla 64 KiB body) client'a gönderir. Bu yüzden bir giriş sayfasına redirect çalışır.
- **Timeout içinde response gelmemesi veya bağlantı hatası**: Client `502 Bad Gateway` alır.

Auth request'i, upstream request'lerinin güvendiği root sertifikalarına güvenir.

```toml
[my-app]
protocol = "http"
vhosts = ["app.example.com"]
auth = { type = "forward", url = "http://127.0.0.1:4180/oauth2/auth", response_headers = ["X-Auth-Request-User"], timeout = "10s" }
routes = [{ path = "/", servers = [{ url = "http://127.0.0.1:9000/" }] }]
```

### Panel Session

Client'lar bir r3v3rs3 panel hesabı ile giriş yapar. Bu hesap, yönetim panelinde kullandığınız hesabın aynısıdır. Kendi kimlik doğrulaması olmayan bir web uygulamasını korumak için bu yöntemi kullanın.

- Session'ı olmayan bir `GET` veya `HEAD` request'i giriş sayfasına yönlendiren `302 Found` alır. Girişten sonra r3v3rs3 tarayıcıyı istenen path'e geri yönlendirir.
- Session'ı olmayan diğer request'ler `401 Unauthorized` alır.

Bu kimlik doğrulamayı kullanan her route kendi path'inin altında şu endpoint'leri sunar. `/` route'u için giriş sayfası `/.r3v3rs3/auth/login` adresindedir. `/admin` route'u için adres `/admin/.r3v3rs3/auth/login` olur.

| Endpoint | Method | İşlem |
|---|---|---|
| `.r3v3rs3/auth/login` | `GET` | Giriş formunu gösterir. |
| `.r3v3rs3/auth/login` | `POST` | Kullanıcı adını, parolayı ve TOTP kodunu kontrol eder, ardından session cookie'sini ayarlar. |
| `.r3v3rs3/auth/logout` | `POST` | Session'ı sonlandırır ve session cookie'sini siler. |

TOTP kodu yalnız TOTP açık hesaplar için gereklidir. `r3v3rs3_session` session cookie'sinde `HttpOnly` ve `SameSite=Lax` attribute'ları, HTTPS ve HTTP/3 üzerinde ayrıca `Secure` attribute'u bulunur. Cookie'de `Domain` attribute'u yoktur ve r3v3rs3 bir session'ı yalnız client'ın giriş yaptığı host üzerinde kabul eder. r3v3rs3 request'i upstream sunucuya göndermeden önce session cookie'sini siler.

`config.toml` dosyasındaki `[admin]` ayarları bu session'lara da uygulanır:

- `session_expiry`: Bir session'ın ömrü. En düşük değer 5 dakikadır.
- `max_login_attempts` ve `login_attempts_reset`: Her client IP adresi ve kullanıcı adı için başarısız giriş limiti. Engellenen bir client sıfırlama süresi geçene kadar `429 Too Many Requests` alır.

r3v3rs3 session'ları memory'de tutar. Bu yüzden yeniden başlatma her client'ın session'ını sonlandırır. Uygulamanıza bir çıkış butonu eklemek için:

```html
<form method="post" action="/.r3v3rs3/auth/logout"><button>Sign Out</button></form>
```

```toml
[my-app]
protocol = "http"
vhosts = ["app.example.com"]
auth = { type = "session" }
routes = [{ path = "/", servers = [{ url = "http://127.0.0.1:9000/" }] }]
```

## Header Kuralları

Proxy'lenen request ve response'ların header'larını "Header Kuralları" bölümünde değiştirebilirsiniz. Bir route "Bu Route için Header Kurallarını Değiştir" seçeneği ile proxy kurallarının yerine kendi kurallarını kullanabilir.

Her satıra bir kural yazın:

| Kural | İşlem |
|---|---|
| `set Name: value` | Header'ın bütün değerlerini değiştirir. |
| `append Name: value` | Mevcut değerleri koruyarak bir değer ekler. |
| `remove Name` | Header'ı siler. |

r3v3rs3 boş satırları ve `#` ile başlayan satırları atlar.

- **Request Header'ları**: Kurallar r3v3rs3'ün upstream sunucuya gönderdiği request'i değiştirir. Kurallar, r3v3rs3 `Forwarded`, `X-Forwarded-*` ve `Via` header'larını ayarladıktan sonra çalışır. Bu yüzden bir kural bu header'ları değiştirebilir.
- **Response Header'ları**: Kurallar upstream response'u, r3v3rs3 onu client'a göndermeden önce değiştirir. Hata sayfaları, redirect'ler ve giriş sayfaları gibi r3v3rs3'ün kendisinin oluşturduğu response'ları değiştirmez.

Değerlerde şu değişkenler kullanılabilir. Süslü parantezin kendisi için `{{` ve `}}` yazın.

| Değişken | Değer |
|---|---|
| `{client_ip}` | "Client IP" bölümünün çözümlediği client IP adresi. |
| `{host}` | İstenen host adı. |
| `{scheme}` | `http` veya `https`. |
| `{request_id}` | Rastgele 32 karakterlik bir hex ID. Bir request'in request ve response kuralları aynı ID'yi kullanır. |
| `{route}` | Eşleşen route'un path'i, örneğin `/api`. |

Kurallar `Connection`, `Content-Length`, `Host`, `Keep-Alive`, `Proxy-Connection`, `TE`, `Trailer`, `Transfer-Encoding` ve `Upgrade` header'larını değiştiremez, çünkü bu header'lar bağlantıyı ve mesajın framing'ini kontrol eder.

```toml
[my-app]
protocol = "http"
vhosts = ["app.example.com"]
headers = { request = [{ action = "set", name = "X-Request-Id", value = "{request_id}" }, { action = "remove", name = "X-Debug" }], response = [{ action = "set", name = "X-Frame-Options", value = "DENY" }, { action = "remove", name = "Server" }] }
routes = [{ path = "/", servers = [{ url = "http://127.0.0.1:9000/" }] }]
```

## Compression

Proxy'lenen response'ları "Compression" bölümünde sıkıştırabilirsiniz. Compression'ı açmak için bir veya daha fazla encoding seçin. Compression'ı kapatmak için hiçbir encoding seçmeyin.

| Encoding | `Content-Encoding` | Seviye |
|---|---|---|
| Brotli | `br` | Quality 4 |
| Zstandard | `zstd` | Level 3 |
| Gzip | `gzip` | Level 6 |

r3v3rs3 request'in `Accept-Encoding` header'ını okur ve kabul edilen encoding'lerden `q` değeri en yüksek olanı kullanır. Client birden fazla encoding'i aynı `q` değeri ile kabul ederse r3v3rs3 `algorithms` sırasını kullanır. Panelde encoding'leri seçme sıranız bu sırayı belirler.

r3v3rs3 bir response'u yalnız şu koşulların hepsi sağlandığında sıkıştırır:

- Status `1xx`, `204 No Content`, `206 Partial Content` veya `304 Not Modified` değildir.
- Upstream sunucu response'u encode etmemiştir ve response'ta `Content-Range` header'ı yoktur.
- `Cache-Control` içinde `no-transform` yoktur.
- `Content-Type` media type'ı `mime_types` içindedir. `text/*` bütün text türleriyle eşleşir. r3v3rs3 `text/event-stream` türünü hiçbir zaman sıkıştırmaz, çünkü compression server-sent event'leri geciktirir.
- `Content-Length`, `min_size` değerine eşit veya ondan büyüktür. r3v3rs3 `Content-Length` içermeyen stream response'larını sıkıştırır.

Bir response bu koşulları sağladığında r3v3rs3 `Vary` header'ına `Accept-Encoding` ekler. r3v3rs3 response'u sıkıştırdığında ayrıca `Content-Length` ve `Accept-Ranges` header'larını siler ve strong `ETag` değerini weak `ETag` değerine çevirir. Response header kuralları compression'dan önce çalışır. Bu yüzden bir kural compression'ı durdurmak için `Cache-Control: no-transform` ayarlayabilir.

| Ayar | Varsayılan |
|---|---|
| `algorithms` | Boş. Compression kapalıdır. |
| `min_size` | `1024` byte |
| `mime_types` | `text/*`, `application/javascript`, `application/json`, `application/manifest+json`, `application/wasm`, `application/xml`, `application/xhtml+xml`, `application/rss+xml`, `application/atom+xml`, `image/svg+xml`, `font/otf`, `font/ttf` |

```toml
[my-app]
protocol = "http"
vhosts = ["app.example.com"]
compression = { algorithms = ["br", "zstd", "gzip"], min_size = 1024, mime_types = ["text/*", "application/json"] }
routes = [{ path = "/", servers = [{ url = "http://127.0.0.1:9000/" }] }]
```

## Cache

Proxy'lenen response'ları "Cache" bölümünde memory'de saklayabilirsiniz. Saklanan bir response, upstream sunucuya request gönderilmeden client'a gider. Bir proxy'nin bütün route'ları tek bir cache'i paylaşır ve her proxy'nin kendi cache'i vardır.

| Ayar | Varsayılan | Açıklama |
|---|---|---|
| `enabled` | `false` | Cache'i açar. |
| `max_size` | `67108864` (64 MiB) | Saklanan response'ların byte cinsinden memory limiti. Cache dolduğunda r3v3rs3 en az kullanılan response'ları siler. |
| `max_entry_size` | `1048576` (1 MiB) | r3v3rs3 body'si bu değerden büyük bir response'u saklamaz. |
| `default_ttl` | `0s` | `Cache-Control: max-age`, `s-maxage` veya `Expires` içermeyen bir response'un ömrü. `0s` değerinde r3v3rs3 böyle bir response'u yalnız `ETag` veya `Last-Modified` header'ı varsa saklar ve her request'te yeniden doğrular. |

r3v3rs3 cache'i yalnız `Range`, `Upgrade` ve `Cache-Control: no-store` içermeyen `GET` ve `HEAD` request'leri için kullanır. Bir `HEAD` request'i saklanan `GET` response'unu kullanır. Client `Cache-Control: no-cache` veya `Pragma: no-cache` gönderdiğinde r3v3rs3 request'i upstream sunucuya gönderir ve yeni response'u saklar. Cache key, istenen host ve upstream URL'dir.

r3v3rs3 bir response'u yalnız şu koşulların hepsi sağlandığında saklar:

- Status `200`, `203`, `204`, `300`, `301`, `308`, `404`, `405`, `410`, `414` veya `501` değerlerinden biridir.
- `Cache-Control` içinde `no-store` veya `private` yoktur.
- Response'ta `Set-Cookie` header'ı yoktur.
- Response'ta `Vary: *` header'ı yoktur.
- Request'te `Authorization` header'ı varsa `Cache-Control` içinde `public`, `s-maxage` veya `must-revalidate` bulunur.
- Response'un bir ömrü veya bir validator'ı vardır ve body `max_entry_size` değerinden büyük değildir.

Ömür sırasıyla `s-maxage`, `max-age`, `Expires` ve `default_ttl` değerlerinden gelir. `Cache-Control: no-cache` ömrü sıfır yapar. Saklanan bir response'un yaşı, upstream response'un `Age` header'ını da içerir.

Saklanan bir response'un süresi dolmuşsa ve response'ta `ETag` veya `Last-Modified` header'ı varsa r3v3rs3 `If-None-Match` veya `If-Modified-Since` ile koşullu bir request gönderir. Upstream sunucu `304 Not Modified` döndürdüğünde r3v3rs3 saklanan header'ları günceller ve saklanan response'u gönderir. r3v3rs3 validator'ı olan bir response'u süresi dolduktan sonra bir saat daha tutar. Eşleşen bir `If-None-Match` veya `If-Modified-Since` header'ı gönderen client, cache'ten `304 Not Modified` alır.

r3v3rs3 her cache key için bir response saklar. `Vary` request header'larını belirttiğinde r3v3rs3 saklanan response'u yalnız bu header'larda aynı değerleri gönderen bir request'e gönderir.

r3v3rs3 cache kullanabilen request'lerden `Accept-Encoding` header'ını siler. Böylece upstream sunucu encode edilmemiş response'lar gönderir. "Compression" bölümü ardından response'u her client için sıkıştırır. Bu request'lere verilen her response bir `X-Cache` header'ı içerir: saklanan response için `HIT`, upstream sunucudan gelen response için `MISS`. Cache'ten gelen bir response ayrıca bir `Age` header'ı içerir.

Bir proxy'nin saklanan bütün response'larını silmek için proxy listesinde "Temizle" linkine tıklayın veya `DELETE /api/proxies/{id}/cache` request'i gönderin. Cache ayarları değişmediği sürece saklanan response'lar memory'de kalır. Yeniden başlatma veya cache ayarlarındaki bir değişiklik saklanan response'ları siler.

```toml
[my-app]
protocol = "http"
vhosts = ["app.example.com"]
cache = { enabled = true, max_size = 67108864, max_entry_size = 1048576, default_ttl = "5m" }
routes = [{ path = "/", servers = [{ url = "http://127.0.0.1:9000/" }] }]
```

## HTTP/2

r3v3rs3, HTTP ve HTTPS proxy'lerinde hem upstream hem de downstream bağlantılarda HTTP/2 destekler.

Downstream tarafında client destekliyorsa HTTP/2 otomatik olarak seçilir. Çoğu web tarayıcısı HTTP/2'yi yalnız TLS üzerinden kullanır, çünkü sunucunun HTTP/2 desteklediğini öğrenmek için ALPN (Application-Layer Protocol Negotiation) gerekir.

Upstream tarafında r3v3rs3 HTTPS sunucularına ALPN ile `h2` ve `http/1.1` sunar ve sunucunun seçtiği protokolü kullanır. Düz HTTP sunucuları HTTP/1.1 alır, çünkü düz bir bağlantı protokolü seçemez. Düz HTTP sunucuları prior knowledge ile HTTP/2 (h2c) kabul eden bir proxy için `h2c = true` ayarlayın. WebSocket ve diğer upgrade request'leri her zaman HTTP/1.1 kullanır. Bir portun bütün bağlantıları upstream bağlantılarını paylaşır. Bu yüzden tek bir HTTP/2 upstream bağlantısı birçok client'ın request'lerini taşır.

```toml
[my-app]
protocol = "http"
vhosts = ["app.example.com"]
h2c = true
routes = [{ path = "/", servers = [{ url = "http://127.0.0.1:9000/" }] }]
```

## WebSocket

r3v3rs3, HTTP ve HTTPS proxy'lerinde WebSocket (ve HTTP upgrade) destekler. WebSocket desteğini açmak için ayrıca bir şey yapmanız gerekmez.

## HTTP/3

HTTP/3 proxy'lemeyi açmak için "Portlar" bölümünde bir QUIC portu bağlayın ve protokol olarak "QUIC üzerinden HTTP (HTTP/3)" seçin. HTTP/3 yalnız gelen bağlantılar için desteklenir. Upstream bağlantılar HTTP/2 veya HTTP/1.1 kullanır.

WebTransport desteklenmez.

# Sertifikalar

## Sunucu Sertifikaları

TLS üzerinden TCP ve HTTPS proxy'leri için r3v3rs3 bir sunucu sertifikası gerektirir. Sunucu sertifikası üç yolla kurulabilir:

1. Self-signed bir sertifika oluşturun.
2. Bir dosyadan sertifika içe aktarın (yalnız PEM formatı).
3. Sertifikayı otomatik almak için [ACME](https://letsencrypt.org/how-it-works/) kullanın.

r3v3rs3, TLS client hello mesajındaki SNI (Server Name Indication) değerine göre sertifikayı otomatik olarak bulur.

## Root Sertifikaları

Upstream sunucunuz sistemin güvenmediği sertifikalar kullanıyorsa bu sertifikaları root sertifika deposuna eklemeniz gerekir. r3v3rs3, sistemin root sertifikalarına ek olarak root sertifikasının imzaladığı bütün sertifikalara otomatik olarak güvenir.

Self-signed bir sertifika oluşturduğunuzda r3v3rs3 ayrıca otomatik olarak bir CA sertifikası oluşturur ve bu sertifikayı root sertifika deposuna ekler.

# ACME

r3v3rs3, [ACME](https://letsencrypt.org/docs/client-options/) (Automatic Certificate Management Environment) ile otomatik sertifika almayı destekler. Let's Encrypt, ZeroSSL ve Google Trust Services gibi birçok sertifika otoritesi ACME destekler.

r3v3rs3 yalnız HTTP challenge ile ACME v2 destekler. TCP 80 portunun açık olduğundan ve internetten erişilebildiğinden emin olun.

# Ayarlar

WebUI'daki "Ayarlar" bölümü, `config.toml` dosyasında saklanan sunucu genelindeki seçenekleri düzenler. Değişiklikler hemen uygulanır ve dosyaya kaydedilir.

| Ayar | Varsayılan | Açıklama |
|---|---|---|
| Session Süresi | `1h` | Bir yönetim paneli session'ının ömrü. En düşük değer 5 dakikadır. |
| Maksimum Giriş Denemesi | `10` | Her client IP ve kullanıcı adı için izin verilen başarısız giriş sayısı. |
| Giriş Denemesi Sıfırlama | `15m` | Limite ulaşıldıktan sonraki bekleme süresi. |
| Arka Plan Görevi Aralığı | `1h` | Sertifika yenileme ve log temizleme görevlerinin aralığı. |
| HTTP Challenge Adresi | `0.0.0.0:80` | ACME HTTP challenge'ları için dinleme adresi. |
| Veritabanı Log Saklama Süresi | `3months` | Log'ların log veritabanında tutulduğu süre. |

Süreler okunabilir bir biçim kullanır, örneğin `30s`, `15m`, `1h` veya `7days`.

# Config Dosyaları

r3v3rs3 config'ini TOML dosyalarında saklar. Bu dosyaların konumu işletim sistemine göre değişir:

- Linux: `$XDG_CONFIG_HOME/r3v3rs3` veya `$HOME/.config/r3v3rs3`
- macOS: `$HOME/Library/Application Support/r3v3rs3`
- Windows: `%APPDATA%\r3v3rs3\config`

Varsayılan konumu `R3V3RS3_CONFIG_DIR` environment variable'ı veya `--config-dir` komut satırı seçeneği ile değiştirebilirsiniz.

Gerekirse bu dosyaları elle düzenleyebilirsiniz. Ancak r3v3rs3 config dosyalarındaki değişiklikleri otomatik olarak algılamaz. Değişikliklerin uygulanması için bir config dosyasını düzenledikten sonra sunucuyu yeniden başlatmanız gerekir.

# WebUI

r3v3rs3 dahili bir WebUI içerir. WebUI varsayılan olarak localhost:46492 adresinde sunulur. Portu `R3V3RS3_WEBUI` environment variable'ı veya `--webui` komut satırı seçeneği ile değiştirebilirsiniz. WebUI'ı kapatmak için `R3V3RS3_NO_WEBUI=1` environment variable'ını ayarlayın veya `--no-webui` komut satırı seçeneğini kullanın.

Navbar'daki bayrak menüsü WebUI dilini seçer: İngilizce veya Türkçe. Tema menüsü Sistem, Açık veya Koyu temayı seçer. WebUI seçimleri `r3v3rs3_lang` ve `r3v3rs3_theme` cookie'lerinde saklar. Bu cookie'ler yoksa WebUI İngilizce ve sistem temasıyla açılır.

r3v3rs3'ün hata sayfaları ve Panel Session giriş sayfası da bu cookie'leri okur. Tarayıcı bu cookie'leri yalnız WebUI host'una gönderir. Bu yüzden başka bir host'taki proxy'nin sayfaları İngilizce ve sistem temasıyla gösterilir.

# Log

r3v3rs3 varsayılan olarak standart çıktıya log yazar. Bu davranışı `R3V3RS3_LOG`, `R3V3RS3_ACCESS_LOG` environment variable'ları veya `--log`, `--access-log` komut satırı seçenekleri ile değiştirebilirsiniz.

```bash
$ r3v3rs3 start --log /var/log/r3v3rs3.log --access-log /var/log/r3v3rs3-access.log
```

Log seviyesini değiştirmek için `R3V3RS3_LOG_LEVEL`, `R3V3RS3_ACCESS_LOG_LEVEL` environment variable'larını veya `--log-level`, `--access-log-level` komut satırı seçeneklerini kullanın.
