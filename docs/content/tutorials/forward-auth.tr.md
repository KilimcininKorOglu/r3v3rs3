+++
title = "Forward auth ile single sign-on"
description = "Uygulamanızın önüne oauth2-proxy koyun ve kullanıcıyı upstream sunucuya iletin"
weight = 15
+++

# Forward auth ile single sign-on

Forward auth, her request için harici bir servise "bu request geçsin mi?" diye sorar. Identity provider session'ını o servis tutar; r3v3rs3 yalnız sorar ve cevabı taşır. nginx'teki `auth_request`, Traefik'teki `forwardAuth` budur.

Zaten oauth2-proxy, Authelia veya kendi yazdığınız bir servisi çalıştırıyorsanız bunu kullanın. r3v3rs3'ün kendi tuttuğu hesaplar için [Ekip için hesaplar](@/tutorials/team-accounts.tr.md) daha basittir.

## Adım 1: Auth servisini çalıştırın

Örnek olarak Google client'ı ile oauth2-proxy:

```yaml
services:
  oauth2-proxy:
    image: quay.io/oauth2-proxy/oauth2-proxy:v7.6.0
    command:
      - --http-address=0.0.0.0:4180
      - --provider=google
      - --email-domain=example.com
      - --upstream=static://202
      - --reverse-proxy=true
      - --cookie-secure=true
      - --set-xauthrequest=true
    environment:
      OAUTH2_PROXY_CLIENT_ID: <client id>
      OAUTH2_PROXY_CLIENT_SECRET: <client secret>
      OAUTH2_PROXY_COOKIE_SECRET: <32 byte secret>
    ports:
      - 127.0.0.1:4180:4180
```

r3v3rs3 için iki option önemlidir:

- `--reverse-proxy=true`, oauth2-proxy'nin r3v3rs3'ün gönderdiği `X-Forwarded-*` header'larını okumasını sağlar.
- `--set-xauthrequest=true`, kullanıcıyı `X-Auth-Request-User` ve `X-Auth-Request-Email` response header'larına yazar. r3v3rs3 bunları kopyalayabilir.

`/oauth2/auth`, giriş yapmış client için 202, diğerleri için 401 dönen endpoint'tir. `/oauth2/start` ve `/oauth2/callback` tarayıcı girişini yürütür.

## Adım 2: Auth endpoint'lerini yayınlayın

Tarayıcı `/oauth2/` adresine uygulamayla aynı host üzerinden ulaşmalıdır, çünkü session cookie'si o host'a aittir. Proxy'nize bir route ekleyin:

| Route | Sunucular | Kimlik doğrulama |
|---|---|---|
| `/oauth2` | `http://127.0.0.1:4180/` | **Yok** |
| `/` | uygulamanız | **Forward Auth** |

Path'i en uzun eşleşen route kazanır, yani `/oauth2/start` oauth2-proxy'ye, diğer her şey uygulamaya gider. `/oauth2` route'unun kimlik doğrulamasını **Yok** yapın; aksi halde client, giriş sayfasına ulaşmak için session'a ihtiyaç duyar.

## Adım 3: Forward auth'u açın

Proxy'nin kimlik doğrulamasını **Forward Auth** yapın:

| Alan | Değer |
|---|---|
| **Auth URL** | `http://127.0.0.1:4180/oauth2/auth` |
| **Kopyalanacak Response Header'ları** | `X-Auth-Request-User`, `X-Auth-Request-Email` |
| **Timeout (Saniye)** | `10` |

`config.toml` içinde:

```toml
[my-app]
protocol = "http"
vhosts = ["app.example.com"]
auth = { type = "forward", url = "http://127.0.0.1:4180/oauth2/auth", response_headers = ["X-Auth-Request-User"], timeout = "10s" }
routes = [
  { path = "/oauth2", servers = [{ url = "http://127.0.0.1:4180/" }], auth = { type = "none" } },
  { path = "/", servers = [{ url = "http://127.0.0.1:9000/" }] },
]
```

## Adım 4: r3v3rs3 ne gönderir

r3v3rs3 her client request'i için auth URL'ine bir `GET` gönderir. Auth request'i, client request'inin header'larını (connection header'ları ve `Host` hariç) ve beş header'ı taşır:

```
x-forwarded-method: GET
x-forwarded-proto: http
x-forwarded-host: app.example.com
x-forwarded-uri: /
x-forwarded-for: 203.0.113.7
```

`X-Forwarded-For`, [Client IP](@/configuration.tr.md#client-ip) ayarlarının çözdüğü client IP adresini taşır. Yani öndeki bir CDN, gerçek client'ı policy'nizden gizlemez.

Auth request'i, upstream request'leriyle aynı root sertifikalara güvenir ve proxy'nin client certificate'ını gönderir. Mutual TLS ile HTTPS üzerinden çalışan bir auth servisi için ikinci bir sertifika ayarı gerekmez.

## Adım 5: r3v3rs3 cevabı ne yapar

| Auth response | Sonuç |
|---|---|
| 2xx | Request, kopyalanan header'larla upstream sunucuya gider. |
| Diğer status'ler | Client auth response'unu alır: status, header'lar ve en çok 64 KiB body. |
| Timeout içinde cevap yok veya bağlantı hatası | Client 502 Bad Gateway alır. |

Tarayıcı girişini mümkün kılan satır ikincisidir. `Location` ile `302 Found` dönen bir auth servisi tarayıcıyı giriş sayfasına gönderir ve r3v3rs3 bu redirect'i aynen aktarır:

```bash
$ curl -i https://app.example.com/
HTTP/1.1 302 Found
location: https://sso.example.com/login
```

Session ile aynı request uygulamaya ulaşır:

```bash
$ curl -s https://app.example.com/ -H 'Cookie: <session>'
200
```

Auth servisi kapalıyken her request 502 döner. Servisi r3v3rs3'ün yanında çalıştırın veya load balancer'ınızın health check'i için auth'u atlayan bir route verin.

## Adım 6: Kullanıcıyı upstream request'inde görün

**Kopyalanacak Response Header'ları** listesindeki header'lar auth response'undan upstream request'ine kopyalanır:

```json
{
  "host": "127.0.0.1:9000",
  "cookie": "sso=ok",
  "x-auth-request-user": "alice@example.com",
  "x-auth-request-email": "alice@example.com",
  "x-forwarded-for": "203.0.113.7",
  "x-forwarded-proto": "http",
  "x-forwarded-host": "app.example.com",
  "via": "r3v3rs3"
}
```

Uygulamanız `X-Auth-Request-User` header'ını okur ve kimin çağırdığını bilir. Identity kütüphanesi gerekmez.

**Client bu header'ları taklit edemez.** r3v3rs3, auth response'unu kopyalamadan önce listedeki her header'ı client request'inden siler. `X-Auth-Request-User: attacker@evil` taşıyan bir request, upstream sunucuya auth servisinin verdiği değerle ulaşır:

```
x-auth-request-user: alice@example.com
```

Auth request'i ise client header'larını taşımaya devam eder, yani auth servisiniz sahte değeri görür. Orada listedeki header'ları dikkate almayın.

## Adım 7: Bir route'u açık bırakın

Health check veya webhook girişten geçmemelidir. Ona kimlik doğrulaması **Yok** olan kendi route'unu verin:

```toml
{ path = "/healthz", servers = [{ url = "http://127.0.0.1:9000/" }], auth = { type = "none" } }
```

```bash
$ curl -s -o /dev/null -w '%{http_code}\n' https://app.example.com/healthz
200
```

Route, proxy'nin kimlik doğrulamasının yerine geçer. Bu kural tersine de çalışır: başka türlü açık olan bir proxy'de tek bir route forward auth kullanabilir.

## Sorun giderme

| Belirti | Sebebi |
|---|---|
| Her request 502 dönüyor | Auth servisi cevap vermiyor veya URL yanlış. |
| Tarayıcı uygulama ile giriş sayfası arasında dönüp duruyor | `/oauth2` route'u yok veya kimlik doğrulaması **Yok** değil. |
| Upstream sunucu kullanıcı header'ını almıyor | Header, **Kopyalanacak Response Header'ları** listesinde yok veya oauth2-proxy `--set-xauthrequest` olmadan çalışıyor. |
| Auth servisi client olarak `127.0.0.1` görüyor | [Client IP](@/configuration.tr.md#client-ip) ayarlarında CDN'iniz veya güvenilen proxy'niz tanımlı değil. |

## Referans

- [Forward Auth](@/configuration.tr.md#forward-auth): header'lar, response kuralları ve timeout.
- [Client IP](@/configuration.tr.md#client-ip): `X-Forwarded-For` nasıl çözülür.
- [Routing](@/configuration.tr.md#routing): en uzun path neden kazanır.

## Sonraki adımlar

- [Ekip için hesaplar](@/tutorials/team-accounts.tr.md): aynı koruma r3v3rs3 hesaplarıyla.
- [Bir uygulamayı korumaya alma](@/tutorials/protect-an-app.tr.md): IP filtresi, rate limit ve audit log.
