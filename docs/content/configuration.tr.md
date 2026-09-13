+++
title = "Config"
description = "Config"
weight = 0
+++

# Portlar

Proxy eklemeden önce dinlenecek bir port bağlamanız gerekir. Bunu "Portlar" bölümünden yapabilirsiniz.

r3v3rs3 altı port türünü destekler:

- HTTP
- HTTPS (TLS üzerinden HTTP)
- QUIC üzerinden HTTP (HTTP/3)
- TCP
- TLS üzerinden TCP
- UDP

## Portu sıfırlama

Port config'ini değiştirdiğinizde açık bağlantılar etkilenmez; bu bağlantılar eski config ile çalışmaya devam eder. Açık bağlantıları kapatmak için portu sıfırlayın.

# Proxy'ler

r3v3rs3 üç proxy türünü destekler:

- HTTP / HTTPS
- TCP / TLS üzerinden TCP
- UDP

Bir proxy'ye birden fazla port bağlayabilirsiniz. Ancak HTTP / HTTPS proxy'sine TCP veya TLS üzerinden TCP portu bağlanamaz. TCP / TLS üzerinden TCP proxy'sine de HTTP veya HTTPS portu bağlanamaz.

## Client IP

CDN veya load balancer arkasında r3v3rs3'ün TCP peer'ı ziyaretçi değil, edge sunucusudur. r3v3rs3 gerçek client IP adresini yalnız peer güvenilirse belirler:

- **Bilinen CDN'ler**: Cloudflare, Fastly, Amazon CloudFront, Bunny CDN, Gcore, KeyCDN, Imperva ve Google Cloud Load Balancing. Bu özellik varsayılan olarak açıktır. Proxy ayarlarındaki "Bilinen CDN'lerin Client IP Header'larına Güven" seçeneğiyle kapatabilirsiniz.
- **Güvenilen Proxy'ler**: Proxy'ye eklediğiniz IP adresleri veya CIDR blokları, örneğin yerel bir load balancer.

Peer güvenilirse r3v3rs3 client IP adresini şu sırayla okur:

1. Sağlayıcı header'ı: Cloudflare için `CF-Connecting-IP`, Bunny CDN için `X-Real-IP`, Amazon CloudFront için `CloudFront-Viewer-Address`.
2. `X-Forwarded-For` içinde güvenilen bir proxy'ye veya bilinen bir CDN edge'ine ait olmayan en sağdaki adres.

r3v3rs3 bulduğu adresi upstream sunucuya `X-Real-IP` header'ında gönderir. Gelen `X-Forwarded-For` ve `Forwarded` zincirlerine dokunmaz. Peer güvenilir değilse r3v3rs3 `Forwarded`, `X-Forwarded-For`, `X-Real-IP`, `CF-Connecting-IP`, `True-Client-IP`, `CloudFront-Viewer-Address`, `Fastly-Client-IP` ve `Incap-Client-IP` header'larını siler, çünkü client bu header'lara sahte değer yazabilir.

CDN IP aralıkları binary'ye gömülüdür ve r3v3rs3 bu listeyi her gün yeniden indirir. Son indirilen liste config dizinindeki `cdn-ranges.json` dosyasına yazılır. İndirme başarısız olursa r3v3rs3 son başarılı listeyi kullanmaya devam eder. Listenin durumunu "Ayarlar" bölümünde görebilir, "Şimdi Yenile" butonuyla listeyi hemen yenileyebilirsiniz.

Akamai edge IP aralıklarını yayınlamaz. Akamai kullanıyorsanız Site Shield aralıklarınızı "Güvenilen Proxy'ler" listesine ekleyin.

## IP filtresi

Her HTTP / HTTPS proxy'sinde client'ları IP adresine göre engelleyebilir veya yalnız belirli adreslere izin verebilirsiniz. Filtre, "Client IP" bölümünde belirlenen adrese bakar. Bu yüzden CDN veya güvenilen bir proxy arkasında da doğru çalışır.

- **Engellenen IP Adresleri**: Bu IP adreslerinden veya CIDR bloklarından gelen client'lar `403 Forbidden` alır.
- **İzin Verilen IP Adresleri**: Liste boş değilse proxy'ye yalnız bu IP adreslerinden veya CIDR bloklarından gelen client'lar erişebilir. Diğer client'lar `403 Forbidden` alır.

Bir adres iki listeye de uyuyorsa engellenir.

Bir route, "Bu Route için IP Filtresini Değiştir" seçeneğiyle proxy listeleri yerine yalnız kendi listelerini kullanır. Route'un iki listesi de boşsa bu route'a her client erişebilir.

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

## Rate limit

Her HTTP / HTTPS proxy'sinde bir client IP adresinin gönderebileceği request sayısını sınırlayabilirsiniz. r3v3rs3 request'leri "Client IP" bölümünde belirlenen adrese göre sayar.

- **Request**: Belirlenen süre içinde izin verilen request sayısı. `0` limiti kapatır.
- **Süre**: Sayacın süresi: saniye, dakika veya saat.
- **Burst**: Bir client'ın limit devreye girmeden art arda gönderebileceği request sayısı. `0` girilirse "Request" değeri kullanılır.

Limiti aşan client, `Retry-After` header'ıyla birlikte `429 Too Many Requests` alır.

Bir route, "Bu Route için Rate Limit'i Değiştir" seçeneğiyle proxy limiti yerine kendi limitini kullanabilir. Bu seçeneği açmayan route'lar her client için ortak bir sayaç kullanır. Route ayarında request değeri `0` ise o route'ta limit uygulanmaz.

r3v3rs3 sayaçları memory'de tutar. Config değiştiğinde sayaçlar korunur; yalnız limitin kendisi değişirse sıfırlanır. Sunucuyu yeniden başlatmak da sayaçları sıfırlar.

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

## Kimlik doğrulama

Her HTTP / HTTPS proxy'sinde "Kimlik Doğrulama" bölümünden kimlik doğrulamayı zorunlu hale getirebilirsiniz. Bir route, "Bu Route için Kimlik Doğrulamayı Değiştir" seçeneğiyle proxy ayarı yerine kendi ayarını kullanabilir. O route'u bütün client'lara açmak için "Yok" seçin.

r3v3rs3 kimlik doğrulamayı IP filtresinden, rate limit'ten ve HTTPS redirect'inden sonra yapar. Bu sayede "HTTP'yi Otomatik Olarak HTTPS'e Yönlendir" seçeneği açıksa tarayıcı kimlik bilgilerini şifreli bağlantı üzerinden gönderir.

### Basic Auth

Geçerli kullanıcı adı ve parola göndermeyen client'lar `WWW-Authenticate: Basic realm="..."` header'ıyla birlikte `401 Unauthorized` alır. Tarayıcı bunun üzerine giriş penceresini açar.

- **Realm**: Tarayıcının giriş penceresinde gösterdiği ad. Boş bırakılırsa `r3v3rs3` kullanılır.
- **Kullanıcılar**: Kullanıcı adları ve parolalar. Kullanıcı adında iki nokta üst üste bulunamaz.

r3v3rs3 parolaları argon2 hash olarak saklar; düz metin parolayı hiçbir zaman kaydetmez. Parolayı değiştirmek istemiyorsanız parola alanını boş bırakın. r3v3rs3, request'i upstream sunucuya göndermeden önce `Authorization` header'ını siler.

Argon2 kasıtlı olarak CPU harcar. r3v3rs3 her kimlik bilgisini bir kez doğrular ve sonucu config değişene kadar memory'de tutar. Parola denemelerini sınırlamak için rate limit kullanın.

`proxies.toml` dosyasında `password_hash` yerine `password` yazabilirsiniz. r3v3rs3 başlarken bu değeri hash'e çevirir.

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

Client, proxy'de tanımlı token'lardan birini `Authorization: Bearer <token>` header'ıyla göndermelidir. Token göndermeyen client `WWW-Authenticate: Bearer realm="r3v3rs3"` ile birlikte `401 Unauthorized` alır. Yanlış token gönderen client da aynı response'u alır; bu response'ta ayrıca `error="invalid_token"` bulunur.

- **Ad**: Token'ı tanımak için verdiğiniz ad.
- **Token**: En az 16 karakterlik rastgele bir değer. Örneğin `openssl rand -hex 32` komutuyla üretebilirsiniz.

r3v3rs3 her token'ın SHA-256 digest'ini saklar; düz metin token'ı hiçbir zaman kaydetmez. Digest'leri sabit sürede karşılaştırır. Token'ı değiştirmek istemiyorsanız token alanını boş bırakın. r3v3rs3, request'i upstream sunucuya göndermeden önce `Authorization` header'ını siler. Bu yüzden bearer kimlik doğrulaması kullanan bir route'ta upstream sunucuya kendi bearer token'ı ulaşmaz.

`proxies.toml` dosyasında `token_hash` yerine `token` yazabilirsiniz. r3v3rs3 başlarken bu değeri digest'e çevirir.

```toml
[my-api]
protocol = "http"
vhosts = ["api.example.com"]
auth = { type = "bearer", tokens = [{ name = "ci", token_hash = "<sha-256 hex digest>" }] }
routes = [{ path = "/", servers = [{ url = "http://127.0.0.1:9000/" }] }]
```

### Forward Auth

r3v3rs3 her request'e izin verilip verilmeyeceğini oauth2-proxy veya Authelia gibi harici bir servise sorar. Bu özellik nginx'teki `auth_request` gibi çalışır.

r3v3rs3 her client request'inde auth URL'ine bir `GET` request'i gönderir. Bu request, bağlantı header'ları ve `Host` dışında client'ın bütün header'larını taşır. Bunlara ek olarak şu header'lar da eklenir:

| Header | Değer |
|---|---|
| `X-Forwarded-Method` | Client request'inin method'u. |
| `X-Forwarded-Proto` | `http` veya `https`. |
| `X-Forwarded-Host` | Client request'inin host'u. |
| `X-Forwarded-Uri` | Client request'inin path ve query kısmı. |
| `X-Forwarded-For` | "Client IP" bölümünde belirlenen client IP adresi. |

- **2xx response**: r3v3rs3 request'i upstream sunucuya gönderir. "Kopyalanacak Response Header'ları" alanında listelenen header'ları auth response'undan upstream request'ine kopyalar. Client bu header'ları kendisi gönderemesin diye önce client request'indeki aynı adlı header'ları siler.
- **Diğer response'lar**: r3v3rs3 auth response'unu (status, header'lar ve en fazla 64 KiB body) client'a gönderir. Bu sayede giriş sayfasına yapılan redirect'ler de çalışır.
- **Timeout süresinde response gelmezse veya bağlantı hatası olursa**: Client `502 Bad Gateway` alır.

Auth request'i, upstream request'leriyle aynı root sertifikalarına güvenir.

```toml
[my-app]
protocol = "http"
vhosts = ["app.example.com"]
auth = { type = "forward", url = "http://127.0.0.1:4180/oauth2/auth", response_headers = ["X-Auth-Request-User"], timeout = "10s" }
routes = [{ path = "/", servers = [{ url = "http://127.0.0.1:9000/" }] }]
```

### Panel Session

Client'lar, yönetim panelinde kullandığınız r3v3rs3 hesaplarıyla giriş yapar. Kendi kimlik doğrulaması olmayan bir web uygulamasını korumak için bu yöntemi kullanabilirsiniz.

- Session'ı olmayan `GET` ve `HEAD` request'leri `302 Found` ile giriş sayfasına yönlendirilir. Giriş yapıldıktan sonra r3v3rs3 tarayıcıyı ilk istenen path'e geri gönderir.
- Session'ı olmayan diğer request'ler `401 Unauthorized` alır.

Bu kimlik doğrulamayı kullanan her route, kendi path'inin altında şu endpoint'leri sunar. `/` route'unun giriş sayfası `/.r3v3rs3/auth/login` adresindedir. `/admin` route'unda bu adres `/admin/.r3v3rs3/auth/login` olur.

| Endpoint | Method | İşlem |
|---|---|---|
| `.r3v3rs3/auth/login` | `GET` | Giriş formunu gösterir. |
| `.r3v3rs3/auth/login` | `POST` | Kullanıcı adını, parolayı ve TOTP kodunu kontrol eder, ardından session cookie'sini ayarlar. |
| `.r3v3rs3/auth/logout` | `POST` | Session'ı sonlandırır ve session cookie'sini siler. |

TOTP kodu yalnız TOTP'si açık hesaplarda istenir. `r3v3rs3_session` cookie'si `HttpOnly` ve `SameSite=Lax` attribute'larını taşır; HTTPS ve HTTP/3 bağlantılarında `Secure` attribute'u da eklenir. Cookie'nin `Domain` attribute'u yoktur ve r3v3rs3 bir session'ı yalnız client'ın giriş yaptığı host'ta kabul eder. r3v3rs3, request'i upstream sunucuya göndermeden önce session cookie'sini siler.

`config.toml` dosyasındaki `[admin]` ayarları bu session'lara da uygulanır:

- `session_expiry`: Session'ın geçerlilik süresi. En az 5 dakika olabilir.
- `max_login_attempts` ve `login_attempts_reset`: Her client IP adresi ve kullanıcı adı için başarısız giriş limiti. Engellenen client, sıfırlama süresi geçene kadar `429 Too Many Requests` alır.

r3v3rs3 session'ları memory'de tutar. Sunucu yeniden başladığında bütün session'lar sona erer. Uygulamanıza çıkış butonu eklemek için şu formu kullanabilirsiniz:

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

## Header kuralları

Proxy'den geçen request ve response'ların header'larını "Header Kuralları" bölümünden değiştirebilirsiniz. Bir route, "Bu Route için Header Kurallarını Değiştir" seçeneğiyle proxy kuralları yerine kendi kurallarını kullanabilir.

Her satıra bir kural yazın:

| Kural | İşlem |
|---|---|
| `set Name: value` | Header'ın mevcut değerlerini bu değerle değiştirir. |
| `append Name: value` | Mevcut değerleri koruyarak bir değer ekler. |
| `remove Name` | Header'ı siler. |

Boş satırlar ve `#` ile başlayan satırlar atlanır.

- **Request Header'ları**: Kurallar, r3v3rs3'ün upstream sunucuya gönderdiği request'e uygulanır. r3v3rs3 önce `Forwarded`, `X-Forwarded-*` ve `Via` header'larını ayarlar, kurallar bundan sonra çalışır. Yani bir kural bu header'ları da değiştirebilir.
- **Response Header'ları**: Kurallar, upstream response client'a gitmeden önce uygulanır. Hata sayfaları, redirect'ler ve giriş sayfaları gibi r3v3rs3'ün kendi ürettiği response'lara uygulanmaz.

Değerlerde şu değişkenleri kullanabilirsiniz. Süslü parantez yazmak için `{{` ve `}}` kullanın.

| Değişken | Değer |
|---|---|
| `{client_ip}` | "Client IP" bölümünde belirlenen client IP adresi. |
| `{host}` | İstenen host adı. |
| `{scheme}` | `http` veya `https`. |
| `{request_id}` | Rastgele 32 karakterlik hex ID. Aynı request'in request ve response kuralları aynı ID'yi kullanır. |
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

Proxy'den geçen response'ları "Compression" bölümünden sıkıştırabilirsiniz. Bir veya daha fazla encoding seçtiğinizde compression açılır. Hiçbir encoding seçili değilse compression kapalıdır.

| Encoding | `Content-Encoding` | Seviye |
|---|---|---|
| Brotli | `br` | Quality 4 |
| Zstandard | `zstd` | Level 3 |
| Gzip | `gzip` | Level 6 |

r3v3rs3 request'in `Accept-Encoding` header'ını okur ve client'ın kabul ettiği encoding'lerden `q` değeri en yüksek olanı seçer. Birden fazla encoding aynı `q` değerine sahipse `algorithms` listesindeki sıra geçerli olur. Panelde bu sıra, encoding'leri seçtiğiniz sıradır.

r3v3rs3 bir response'u yalnız şu koşulların hepsi sağlanırsa sıkıştırır:

- Status `1xx`, `204 No Content`, `206 Partial Content` veya `304 Not Modified` değildir.
- Upstream sunucu response'u encode etmemiştir ve response'ta `Content-Range` header'ı yoktur.
- `Cache-Control` içinde `no-transform` yoktur.
- `Content-Type` header'ındaki media type `mime_types` listesindedir. `text/*` bütün text türleriyle eşleşir. r3v3rs3 `text/event-stream` türünü hiçbir zaman sıkıştırmaz, çünkü compression server-sent event'leri geciktirir.
- `Content-Length` değeri en az `min_size` kadardır. `Content-Length` header'ı olmayan stream response'ları da sıkıştırılır.

Bu koşulları sağlayan response'larda r3v3rs3 `Vary` header'ına `Accept-Encoding` ekler. Response'u sıkıştırdığında `Content-Length` ve `Accept-Ranges` header'larını da siler, strong `ETag` değerini weak `ETag` değerine çevirir. Response header kuralları compression'dan önce çalıştığı için bir kural `Cache-Control: no-transform` ayarlayarak compression'ı engelleyebilir.

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

Proxy'den geçen response'ları "Cache" bölümünden memory'de saklayabilirsiniz. Cache'teki bir response, upstream sunucuya gidilmeden doğrudan client'a gönderilir. Her proxy'nin ayrı bir cache'i vardır ve proxy'nin bütün route'ları bu cache'i ortak kullanır.

| Ayar | Varsayılan | Açıklama |
|---|---|---|
| `enabled` | `false` | Cache'i açar. |
| `max_size` | `67108864` (64 MiB) | Saklanan response'lar için byte cinsinden memory limiti. Cache dolunca r3v3rs3 en az kullanılan response'ları siler. |
| `max_entry_size` | `1048576` (1 MiB) | Body'si bu değerden büyük response'lar saklanmaz. |
| `default_ttl` | `0s` | `Cache-Control: max-age`, `s-maxage` veya `Expires` içermeyen response'un geçerlilik süresi. Değer `0s` ise r3v3rs3 böyle bir response'u yalnız `ETag` veya `Last-Modified` header'ı varsa saklar ve her request'te yeniden doğrular. |

r3v3rs3 cache'i yalnız `Range`, `Upgrade` ve `Cache-Control: no-store` içermeyen `GET` ve `HEAD` request'lerinde kullanır. `HEAD` request'ine saklanan `GET` response'u verilir. Client `Cache-Control: no-cache` veya `Pragma: no-cache` gönderirse r3v3rs3 cache'e bakmadan request'i upstream sunucuya iletir ve gelen yeni response'u saklar. Cache key, istenen host ile upstream URL'den oluşur.

r3v3rs3 bir response'u yalnız şu koşulların hepsi sağlanırsa saklar:

- Status `200`, `203`, `204`, `300`, `301`, `308`, `404`, `405`, `410`, `414` veya `501` değerlerinden biridir.
- `Cache-Control` içinde `no-store` veya `private` yoktur.
- Response'ta `Set-Cookie` header'ı yoktur.
- Response'ta `Vary: *` header'ı yoktur.
- Request'te `Authorization` header'ı varsa `Cache-Control` içinde `public`, `s-maxage` veya `must-revalidate` bulunur.
- Response'un geçerlilik süresi veya validator'ı vardır ve body'si `max_entry_size` değerini aşmaz.

Geçerlilik süresi için sırasıyla `s-maxage`, `max-age`, `Expires` ve `default_ttl` değerlerine bakılır. `Cache-Control: no-cache` bu süreyi sıfır yapar. Saklanan response'un yaşına upstream response'taki `Age` header'ının değeri de eklenir.

Saklanan response'un süresi dolmuşsa ve response'ta `ETag` veya `Last-Modified` header'ı varsa r3v3rs3 `If-None-Match` veya `If-Modified-Since` ile koşullu bir request gönderir. Upstream sunucu `304 Not Modified` dönerse r3v3rs3 saklanan header'ları günceller ve cache'teki response'u gönderir. Validator'ı olan response, süresi dolduktan sonra da bir saat cache'te kalır. Eşleşen `If-None-Match` veya `If-Modified-Since` header'ı gönderen client, cache'ten `304 Not Modified` alır.

Her cache key için tek bir response saklanır. Response'un `Vary` header'ı request header'larını listeliyorsa saklanan response yalnız bu header'larda aynı değerleri gönderen request'lere verilir.

r3v3rs3, cache'i kullanabilecek request'lerdeki `Accept-Encoding` header'ını siler. Böylece upstream sunucu response'ları encode etmeden gönderir ve "Compression" ayarları response'u her client için ayrıca sıkıştırır. Bu request'lere verilen response'larda `X-Cache` header'ı bulunur: cache'ten gelen response için `HIT`, upstream sunucudan gelen response için `MISS`. Cache'ten gelen response'ta ayrıca `Age` header'ı vardır.

Bir proxy'nin cache'ini boşaltmak için proxy listesindeki "Temizle" linkine tıklayın veya `DELETE /api/proxies/{id}/cache` request'i gönderin. Saklanan response'lar, cache ayarları değişene veya sunucu yeniden başlatılana kadar memory'de kalır.

```toml
[my-app]
protocol = "http"
vhosts = ["app.example.com"]
cache = { enabled = true, max_size = 67108864, max_entry_size = 1048576, default_ttl = "5m" }
routes = [{ path = "/", servers = [{ url = "http://127.0.0.1:9000/" }] }]
```

## HTTP/2

r3v3rs3, HTTP ve HTTPS proxy'lerinde hem upstream hem de downstream bağlantılarda HTTP/2 destekler.

Downstream tarafında client destekliyorsa HTTP/2 otomatik olarak seçilir. Çoğu web tarayıcısı HTTP/2'yi yalnız TLS üzerinden kullanır, çünkü sunucunun HTTP/2 desteklediğini ALPN (Application-Layer Protocol Negotiation) ile öğrenir.

Upstream tarafında r3v3rs3, HTTPS sunucularına ALPN ile `h2` ve `http/1.1` önerir ve sunucunun seçtiği protokolü kullanır. Düz HTTP bağlantısında protokol seçimi yapılamadığı için düz HTTP sunucularına HTTP/1.1 ile bağlanılır. Proxy'nin düz HTTP sunucuları prior knowledge ile HTTP/2 (h2c) kabul ediyorsa `h2c = true` ayarlayın. WebSocket ve diğer upgrade request'leri her zaman HTTP/1.1 kullanır. Bir porttaki bütün bağlantılar upstream bağlantılarını ortak kullanır; bu yüzden tek bir HTTP/2 upstream bağlantısı birçok client'ın request'lerini taşır.

```toml
[my-app]
protocol = "http"
vhosts = ["app.example.com"]
h2c = true
routes = [{ path = "/", servers = [{ url = "http://127.0.0.1:9000/" }] }]
```

## WebSocket

r3v3rs3, HTTP ve HTTPS proxy'lerinde WebSocket'i (ve HTTP upgrade'i) destekler. Bunun için ayrıca bir ayar yapmanız gerekmez.

## HTTP/3

HTTP/3 proxy'lemeyi açmak için "Portlar" bölümünde bir QUIC portu bağlayın ve protokol olarak "QUIC üzerinden HTTP (HTTP/3)" seçin. HTTP/3 yalnız gelen bağlantılarda desteklenir. Upstream bağlantılar HTTP/2 veya HTTP/1.1 kullanır.

WebTransport desteklenmez.

# Sertifikalar

## Sunucu sertifikaları

TLS üzerinden TCP ve HTTPS proxy'leri için bir sunucu sertifikası gerekir. Sunucu sertifikasını üç yolla ekleyebilirsiniz:

1. Self-signed bir sertifika oluşturun.
2. Bir dosyadan sertifika içe aktarın (yalnız PEM formatı).
3. Sertifikayı [ACME](https://letsencrypt.org/how-it-works/) ile otomatik alın.

r3v3rs3, TLS client hello mesajındaki SNI (Server Name Indication) değerine göre uygun sertifikayı otomatik olarak seçer.

## Root sertifikaları

Upstream sunucunuz sistemin güvenmediği sertifikalar kullanıyorsa bu sertifikaları root sertifika deposuna eklemeniz gerekir. r3v3rs3, sistemin root sertifikalarına ek olarak bu depodaki root sertifikalarının imzaladığı bütün sertifikalara da otomatik olarak güvenir.

Self-signed bir sertifika oluşturduğunuzda r3v3rs3 bir CA sertifikası da oluşturur ve onu root sertifika deposuna ekler.

# ACME

r3v3rs3, sertifikaları [ACME](https://letsencrypt.org/docs/client-options/) (Automatic Certificate Management Environment) ile otomatik alabilir. Let's Encrypt, ZeroSSL ve Google Trust Services gibi birçok sertifika otoritesi ACME'yi destekler.

r3v3rs3 ACME v2'yi yalnız HTTP challenge ile destekler. TCP 80 portunun açık ve internetten erişilebilir olduğundan emin olun.

# Ayarlar

WebUI'daki "Ayarlar" bölümünden, `config.toml` dosyasında saklanan ve bütün sunucuyu etkileyen seçenekleri değiştirebilirsiniz. Değişiklikler hemen uygulanır ve dosyaya yazılır.

| Ayar | Varsayılan | Açıklama |
|---|---|---|
| Session Süresi | `1h` | Yönetim paneli session'ının geçerlilik süresi. En az 5 dakika olabilir. |
| Maksimum Giriş Denemesi | `10` | Her client IP adresi ve kullanıcı adı için izin verilen başarısız giriş sayısı. |
| Giriş Denemesi Sıfırlama | `15m` | Limite ulaşıldıktan sonraki bekleme süresi. |
| Arka Plan Görevi Aralığı | `1h` | Sertifika yenileme ve log temizleme görevlerinin çalışma aralığı. |
| HTTP Challenge Adresi | `0.0.0.0:80` | ACME HTTP challenge'larının dinlendiği adres. |
| Veritabanı Log Saklama Süresi | `3months` | Log'ların log veritabanında ne kadar tutulacağı. |

Süreleri `30s`, `15m`, `1h` veya `7days` gibi okunabilir bir biçimde yazın.

# Config dosyaları

r3v3rs3 config'ini TOML dosyalarında saklar. Bu dosyaların konumu işletim sistemine göre değişir:

- Linux: `$XDG_CONFIG_HOME/r3v3rs3` veya `$HOME/.config/r3v3rs3`
- macOS: `$HOME/Library/Application Support/r3v3rs3`
- Windows: `%APPDATA%\r3v3rs3\config`

Varsayılan konumu `R3V3RS3_CONFIG_DIR` environment variable'ı veya `--config-dir` komut satırı seçeneğiyle değiştirebilirsiniz.

Bu dosyaları elle de düzenleyebilirsiniz. Ancak r3v3rs3 config dosyalarındaki değişiklikleri kendiliğinden algılamaz. Değişikliklerin geçerli olması için dosyayı düzenledikten sonra sunucuyu yeniden başlatın.

# WebUI

r3v3rs3 bir WebUI ile birlikte gelir. WebUI varsayılan olarak localhost:46492 adresinde çalışır. Portu `R3V3RS3_WEBUI` environment variable'ı veya `--webui` komut satırı seçeneğiyle değiştirebilirsiniz. WebUI'ı kapatmak için `R3V3RS3_NO_WEBUI=1` environment variable'ını ayarlayın veya `--no-webui` komut satırı seçeneğini kullanın.

WebUI dilini navbar'daki bayrak menüsünden seçebilirsiniz: İngilizce veya Türkçe. Tema menüsünde Sistem, Açık ve Koyu seçenekleri bulunur. WebUI bu seçimleri `r3v3rs3_lang` ve `r3v3rs3_theme` cookie'lerinde saklar. Bu cookie'ler yoksa WebUI İngilizce ve sistem temasıyla açılır.

r3v3rs3'ün hata sayfaları ve Panel Session giriş sayfası da bu cookie'lere bakar. Ancak tarayıcı bu cookie'leri yalnız WebUI'ın host'una gönderir. Bu yüzden başka bir host'taki proxy'nin sayfaları İngilizce ve sistem temasıyla açılır.

# Yönetim API'si

WebUI, `/api` altındaki yönetim API'sini kullanır. r3v3rs3 bu API'nin OpenAPI dokümanını sunucu kodundan üretir. Bu yüzden doküman, çalışan sürümün route'larını listeler.

- OpenAPI dokümanı: `http://localhost:46492/api/openapi.json`
- Swagger UI: `http://localhost:46492/api/docs/`

İki adres de session ister. Önce WebUI'a giriş yapın, sonra adresleri aynı tarayıcıda açın. WebUI footer'ındaki API linki de Swagger UI'ı açar.

Bir script, `POST /api/login` ile giriş yapar ve cevaptaki `token` cookie'sini sonraki request'lerle gönderir:

```bash
$ curl -c cookies.txt -H 'Content-Type: application/json' \
    -d '{"username":"admin","method":"password","password":"passw0rd","insecure":true}' \
    http://localhost:46492/api/login
$ curl -b cookies.txt http://localhost:46492/api/ports
```

`"insecure": true` değeri cookie'den `Secure` özelliğini kaldırır. Yönetim paneli düz HTTP kullanıyorsa bu değeri gönderin.

# Log

r3v3rs3 varsayılan olarak log'ları standart çıktıya yazar. Bunu `R3V3RS3_LOG`, `R3V3RS3_ACCESS_LOG` environment variable'larıyla veya `--log`, `--access-log` komut satırı seçenekleriyle değiştirebilirsiniz.

```bash
$ r3v3rs3 start --log /var/log/r3v3rs3.log --access-log /var/log/r3v3rs3-access.log
```

Log seviyesini değiştirmek için `R3V3RS3_LOG_LEVEL`, `R3V3RS3_ACCESS_LOG_LEVEL` environment variable'larını veya `--log-level`, `--access-log-level` komut satırı seçeneklerini kullanın.
