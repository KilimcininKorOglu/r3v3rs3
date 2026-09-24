+++
title = "Forward auth ile single sign-on"
description = "Uygulamanızın önüne oauth2-proxy koyun ve kullanıcıyı upstream sunucuya iletin"
weight = 15
+++

# Forward auth ile single sign-on

Forward auth, her istek için harici bir servise "bu istek geçsin mi?" diye sorar. Kimlik sağlayıcısındaki oturumu o servis tutar; r3v3rs3 yalnız sorar ve cevabı taşır. nginx'teki `auth_request`, Traefik'teki `forwardAuth` budur.

Zaten oauth2-proxy, Authelia veya kendi yazdığınız bir servisi çalıştırıyorsanız bunu kullanın. r3v3rs3'ün kendi tuttuğu hesaplar için [Ekip için hesaplar](@/tutorials/team-accounts.tr.md) daha basittir.

## Adım 1: Doğrulama servisini çalıştırın

Örnek olarak Google istemcisi ile oauth2-proxy:

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

r3v3rs3 için iki seçenek önemlidir:

- `--reverse-proxy=true`, oauth2-proxy'nin r3v3rs3'ün gönderdiği `X-Forwarded-*` header'larını okumasını sağlar.
- `--set-xauthrequest=true`, kullanıcıyı `X-Auth-Request-User` ve `X-Auth-Request-Email` yanıt header'larına yazar. r3v3rs3 bunları kopyalayabilir.

`/oauth2/auth`, giriş yapmış istemci için 202, diğerleri için 401 dönen endpoint'tir. `/oauth2/start` ve `/oauth2/callback` tarayıcı girişini yürütür.

## Adım 2: Doğrulama endpoint'lerini yayınlayın

Tarayıcı `/oauth2/` adresine uygulamayla aynı host üzerinden ulaşmalıdır, çünkü oturum cookie'si o host'a aittir. Proxy'nize bir route ekleyin:

| Route | Sunucular | Kimlik doğrulama |
|---|---|---|
| `/oauth2` | `http://127.0.0.1:4180/` | **Yok** |
| `/` | uygulamanız | **Forward Auth** |

Yolu en uzun eşleşen route kazanır, yani `/oauth2/start` oauth2-proxy'ye, diğer her şey uygulamaya gider. `/oauth2` route'unun kimlik doğrulamasını **Yok** yapın; aksi halde istemci, giriş sayfasına ulaşmak için oturuma ihtiyaç duyar.

## Adım 3: Forward auth'u açın

Proxy'nin kimlik doğrulamasını **Forward Auth** yapın:

| Alan | Değer |
|---|---|
| **Doğrulama URL'i** | `http://127.0.0.1:4180/oauth2/auth` |
| **Kopyalanacak Yanıt Header'ları** | `X-Auth-Request-User`, `X-Auth-Request-Email` |
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

r3v3rs3 her istemci isteği için doğrulama URL'ine bir `GET` gönderir. Doğrulama isteği, istemci isteğinin header'larını (connection header'ları ve `Host` hariç) ve beş header'ı taşır:

```
x-forwarded-method: GET
x-forwarded-proto: http
x-forwarded-host: app.example.com
x-forwarded-uri: /
x-forwarded-for: 203.0.113.7
```

`X-Forwarded-For`, [İstemci IP adresi](@/configuration.tr.md#istemci-ip-adresi) ayarlarının çözdüğü istemci IP adresini taşır. Yani öndeki bir CDN, gerçek istemciyi erişim kurallarınızdan gizlemez.

Doğrulama isteği, upstream isteklerle aynı kök sertifikalara güvenir ve proxy'nin istemci sertifikasını gönderir. Mutual TLS ile HTTPS üzerinden çalışan bir doğrulama servisi için ikinci bir sertifika ayarı gerekmez.

## Adım 5: r3v3rs3 cevabı ne yapar

| Doğrulama yanıtı | Sonuç |
|---|---|
| 2xx | İstek, kopyalanan header'larla upstream sunucuya gider. |
| Diğer durum kodları | İstemci doğrulama yanıtını alır: durum kodu, header'lar ve en çok 64 KiB gövde. |
| Timeout içinde cevap yok veya bağlantı hatası | İstemci 502 Bad Gateway alır. |

Tarayıcı girişini mümkün kılan satır ikincisidir. `Location` ile `302 Found` dönen bir doğrulama servisi tarayıcıyı giriş sayfasına gönderir ve r3v3rs3 bu yönlendirmeyi aynen aktarır:

```bash
$ curl -i https://app.example.com/
HTTP/1.1 302 Found
location: https://sso.example.com/login
```

Oturumla gönderilen aynı istek uygulamaya ulaşır:

```bash
$ curl -s https://app.example.com/ -H 'Cookie: <session>'
200
```

Doğrulama servisi kapalıyken her istek 502 döner. Servisi r3v3rs3'ün yanında çalıştırın veya yük dengeleyicinizin sağlık kontrolü için doğrulamayı atlayan bir route verin.

## Adım 6: Kullanıcıyı upstream isteğinde görün

**Kopyalanacak Yanıt Header'ları** listesindeki header'lar doğrulama yanıtından upstream isteğine kopyalanır:

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

Uygulamanız `X-Auth-Request-User` header'ını okur ve kimin çağırdığını bilir. Kimlik kütüphanesi gerekmez.

**İstemci bu header'ları taklit edemez.** r3v3rs3, doğrulama yanıtını kopyalamadan önce listedeki her header'ı istemci isteğinden siler. `X-Auth-Request-User: attacker@evil` taşıyan bir istek, upstream sunucuya doğrulama servisinin verdiği değerle ulaşır:

```
x-auth-request-user: alice@example.com
```

Doğrulama isteği ise istemci header'larını taşımaya devam eder, yani doğrulama servisiniz sahte değeri görür. Orada listedeki header'ları dikkate almayın.

## Adım 7: Bir route'u açık bırakın

Sağlık kontrolü veya webhook girişten geçmemelidir. Ona kimlik doğrulaması **Yok** olan kendi route'unu verin:

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
| Her istek 502 dönüyor | Doğrulama servisi cevap vermiyor veya URL yanlış. |
| Tarayıcı uygulama ile giriş sayfası arasında dönüp duruyor | `/oauth2` route'u yok veya kimlik doğrulaması **Yok** değil. |
| Upstream sunucu kullanıcı header'ını almıyor | Header, **Kopyalanacak Yanıt Header'ları** listesinde yok veya oauth2-proxy `--set-xauthrequest` olmadan çalışıyor. |
| Doğrulama servisi istemci olarak `127.0.0.1` görüyor | [İstemci IP adresi](@/configuration.tr.md#istemci-ip-adresi) ayarlarında CDN'iniz veya güvenilen proxy'niz tanımlı değil. |

## Referans

- [Forward Auth](@/configuration.tr.md#forward-auth): header'lar, yanıt kuralları ve timeout.
- [İstemci IP adresi](@/configuration.tr.md#istemci-ip-adresi): `X-Forwarded-For` nasıl çözülür.
- [Route seçimi](@/configuration.tr.md#route-secimi): en uzun yol neden kazanır.

## Sonraki adımlar

- [Ekip için hesaplar](@/tutorials/team-accounts.tr.md): aynı koruma r3v3rs3 hesaplarıyla.
- [Bir uygulamayı korumaya alma](@/tutorials/protect-an-app.tr.md): IP filtresi, rate limit ve denetim kaydı.
