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

## Server Name'ler

HTTPS, QUIC üzerinden HTTP ve TLS üzerinden TCP portlarında "Server Name'ler" alanı bulunur. r3v3rs3, server sertifikasını client'ın SNI (Server Name Indication) değerine göre seçer. Client SNI göndermediğinde r3v3rs3, bütün server name'leri taşıyan geçerli bir server sertifikası seçer. Liste boşsa ilk geçerli server sertifikasını seçer.

```toml
[my-port]
listen = "/ip4/0.0.0.0/tcp/443/https"
tls_termination = { server_names = ["example.com", "*.example.com"] }
```

## TLS client authentication

HTTPS, QUIC üzerinden HTTP ve TLS üzerinden TCP portları client'ın sertifikasını doğrulayabilir (mutual TLS). Modu "Client Authentication" alanından seçin:

| Mod | Davranış |
|---|---|
| Kapalı | Port client sertifikası istemez. Varsayılan mod budur. |
| İsteğe bağlı | Port sertifika ister, fakat sertifikasız client'ları da kabul eder. Client'ın gönderdiği sertifika geçerli olmalıdır. |
| Zorunlu | Client geçerli bir sertifika göndermezse TLS handshake başarısız olur. |

İsteğe bağlı ve Zorunlu modlarda "Client CA Sertifikaları" listesinden bir veya daha fazla root sertifika seçin. Client sertifikası bunlardan biriyle imzalanmış olmalıdır. Sistemin root sertifikaları kullanılmaz. r3v3rs3, root sertifika seçilmemiş veya root olmayan bir sertifika seçilmiş port config'ini reddeder. Bir portun kullandığı root sertifika silinemez.

Client authentication config'i geçersiz hale gelirse (örneğin root sertifika config dizininden silinirse) port bütün bağlantıları kapatır ve port listesi "TLS Hatası" gösterir.

HTTPS ve QUIC üzerinden HTTP portlarında header kuralları, doğrulanan sertifikayı `{client_cert_subject}` ve `{client_cert_fingerprint}` değişkenleriyle upstream sunucuya gönderebilir. "Header kuralları" bölümüne bakın.

```toml
[my-port]
listen = "/ip4/0.0.0.0/tcp/443/https"
tls_termination = { server_names = ["example.com"], client_auth = "required", client_ca_certs = ["a1b2c3d"] }
```

## PROXY protocol

r3v3rs3 önündeki bir load balancer, client adresini PROXY protocol header'ı (versiyon 1 veya 2) ile gönderebilir. TCP, TLS üzerinden TCP, HTTP ve HTTPS portları bu header'ı okuyabilir. UDP ve QUIC üzerinden HTTP portları okuyamaz.

"PROXY Protocol Kabul Et" seçeneğini açın. Load balancer'ların IP adreslerini veya CIDR bloklarını "Güvenilen Load Balancer'lar" alanına yazın:

- Güvenilen bir adresten gelen bağlantı geçerli bir header ile başlamalıdır. Header geçersizse, versiyonu "Kabul Edilen Versiyonlar" içinde yoksa veya header "Header Timeout" süresinde gelmezse r3v3rs3 bağlantıyı kapatır. Varsayılan timeout 5 saniyedir.
- r3v3rs3 diğer adreslerden header okumaz. Bu bağlantıların client adresi peer adresidir.
- Versiyon 2 `LOCAL` header'ı ve versiyon 1 `UNKNOWN` header'ı peer adresini korur. Load balancer'lar bunları health check'lerde gönderir.

r3v3rs3 header'ı TLS handshake'ten önce okur. Header'daki adres bağlantının client adresi olur. IP filtreleri, rate limit'ler, client IP hash, `Forwarded` ve `X-Forwarded-For` header'ları ve access log bu adresi kullanır. Access log peer adresini de `peer` alanına yazar.

PROXY protocol header'ı ile başlayan bir ACME HTTP-01 challenge request'i challenge yanıtını almaz. Böyle bir load balancer'ın sunduğu sertifika için DNS-01 challenge kullanın.

```toml
[my-port]
listen = "/ip4/0.0.0.0/tcp/443/https"
tls_termination = { server_names = ["example.com"] }
proxy_protocol = { trusted = ["10.0.0.0/8"], accept = "v2", timeout = "5s" }
```

`accept` değeri `any` (varsayılan), `v1` veya `v2` olur.

## Portu sıfırlama

Port config'ini değiştirdiğinizde açık bağlantılar etkilenmez; bu bağlantılar eski config ile çalışmaya devam eder. Açık bağlantıları kapatmak için portu sıfırlayın.

# Proxy'ler

r3v3rs3 üç proxy türünü destekler:

- HTTP / HTTPS
- TCP / TLS üzerinden TCP
- UDP

Bir proxy'ye birden fazla port bağlayabilirsiniz. Ancak HTTP / HTTPS proxy'sine TCP veya TLS üzerinden TCP portu bağlanamaz. TCP / TLS üzerinden TCP proxy'sine de HTTP veya HTTPS portu bağlanamaz.

## Routing

Bir portu birden fazla HTTP / HTTPS proxy'si kullanıyorsa r3v3rs3, porttaki bütün proxy'lerin bütün route'larını karşılaştırır. Request'i en özel route'a gönderir. Proxy'lerin ve route'ların sırası sonucu değiştirmez.

1. Önce request'in host'u proxy'leri seçer. Host ile aynı olan virtual host (`app.example.com` veya bir IP adresi), wildcard'dan (`*.example.com`) önce gelir. Wildcard, regex pattern'den önce gelir. Regex pattern, virtual host'u olmayan proxy'den önce gelir. Virtual host'u olmayan proxy her host'u kabul eder.
2. Host eşleşmesi aynı olan route'lar arasında path'i en uzun olan route kazanır. Path tam segment'lerle eşleşir. Bu yüzden `/api`, `/api` ve `/api/users` ile eşleşir, `/apiv2` ile eşleşmez.
3. İki route'un host eşleşmesi ve path'i aynıysa önce gelen route kazanır.

Örneğin aşağıdaki route'larda `GET /api/users` request'i `http://api:8080/` sunucusuna, `GET /about` request'i `http://web:3000/` sunucusuna gider. `app.example.com` için gelen request, path'i `/api` olsa da `my-app` proxy'sine gider.

```toml
[my-default]
protocol = "http"
routes = [
  { path = "/", servers = [{ url = "http://web:3000/" }] },
  { path = "/api", servers = [{ url = "http://api:8080/" }] },
]

[my-app]
protocol = "http"
vhosts = ["app.example.com"]
routes = [{ path = "/", servers = [{ url = "http://app:9000/" }] }]
```

## Path Rewrite

r3v3rs3 varsayılan olarak route path'ini request path'inden kaldırır ve kalan kısmı sunucu URL'sinin path'ine ekler. Örneğin `path = "/api"` değerli bir route ve `http://api:8080/v1/` sunucusu için `GET /api/users` request'i `http://api:8080/v1/users` adresine gider. Route'un `rewrite` tablosu path'i şu sırayla değiştirir:

1. `strip_prefix = false` route path'ini korur. Bu durumda aynı request `http://api:8080/v1/api/users` adresine gider.
2. `regex` path'teki ilk eşleşmeyi `replacement` değeriyle değiştirir. Path `/` ile başlar. `${1}` veya `${name}` bir capture group ekler. Eşleşme yoksa path değişmez.
3. `add_prefix` path'in başına `/v2` gibi bir path ekler.

r3v3rs3 query string'i korur. Kimlik doğrulama ve cache, client request'inin path'ini kullanır. r3v3rs3 geçersiz regex'i reddeder. `/` ile başlamayan veya `?` ya da `#` içeren `add_prefix` değerini de reddeder.

Aşağıdaki route'larda `GET /api/users` request'i `http://api:8080/v2/users` adresine, `GET /items/42` request'i `http://shop:9000/item/42` adresine gider.

```toml
[my-shop]
protocol = "http"
routes = [
  { path = "/api", servers = [{ url = "http://api:8080/" }], rewrite = { add_prefix = "/v2" } },
  { path = "/items", servers = [{ url = "http://shop:9000/" }], rewrite = { strip_prefix = false, regex = "^/items/([0-9]+)$", replacement = "/item/${1}" } },
]
```

## Redirect Kuralları

HTTP / HTTPS proxy'sinin `redirects` değeri request'e bir redirect ile yanıt verir. Bu durumda request upstream sunucuya gitmez. Her kuralın `regex`, `target` ve `status` değerleri vardır:

- `regex`; request'in port'suz host'u, path'i ve query'sinden oluşan değerle eşleşir, örneğin `example.com/old/page?id=1`.
- `target` response'un `Location` header'ıdır. `${1}` veya `${name}` bir capture group ekler.
- `status` değeri `301`, `302` (varsayılan), `307` veya `308` olabilir.

Eşleşen ilk kural yanıt verir. r3v3rs3; client IP filtresini, rate limit'i ve `upgrade_insecure` HTTPS redirect'ini kurallardan önce, kimlik doğrulamayı kurallardan sonra uygular. Geçerli bir header değeri oluşturmayan target eşleşme sayılmaz ve r3v3rs3 bir uyarı log'u yazar. r3v3rs3 başka bir status değerini reddeder. Boş olan veya kontrol karakteri içeren target değerini de reddeder.

WebUI'da her satıra bir kuralı `status regex target` biçiminde yazın. Orada regex ve target boşluk içeremez. Regex içinde `\s`, target içinde `%20` kullanın.

```toml
[my-site]
protocol = "http"
vhosts = ["example.com", "www.example.com"]
redirects = [
  { regex = "^www\\.example\\.com/(.*)$", target = "https://example.com/${1}", status = 301 },
  { regex = "^example\\.com/blog/([0-9]+)$", target = "/posts/${1}" },
]
routes = [{ path = "/", servers = [{ url = "http://127.0.0.1:3000/" }] }]
```

## Sabit Response'lar

HTTP / HTTPS proxy'sindeki bir route, request'i `servers` değerine göndermek yerine `response` ile her request'e kendisi yanıt verebilir. Bir route'ta `servers` veya `response` değerlerinden yalnız biri bulunur. Redirect host veya 404 host için sabit response kullanın.

- `type = "redirect"` değeri `target` adresine bir redirect ile yanıt verir. `status` değeri `301`, `302` (varsayılan), `307` veya `308` olabilir. `preserve_path` (varsayılan `true`) request'in path'ini ve query'sini `target` sonuna ekler. Bu durumda `GET /a?b=1` request'i `https://example.com/a?b=1` adresine gider.
- `type = "status"` değeri `status` ile ve isteğe bağlı düz metin `body` ile yanıt verir. `body` en fazla 4096 byte olabilir. `status` değeri `200`, `400`, `403`, `404`, `410`, `429`, `451`, `500`, `502` veya `503` olabilir.

r3v3rs3; client IP filtresini, rate limit'i, `upgrade_insecure` HTTPS redirect'ini, redirect kurallarını ve kimlik doğrulamayı sabit response'tan önce uygular. r3v3rs3 hem `servers` hem `response` içeren route'u reddeder. Geçersiz target, status veya body değerini de reddeder.

WebUI'da her route için route tipini seçin. Yeni proxy sayfası "Redirect host" ve "404 host" şablonlarını sunar. Servis keşfi label'ları aynı alanları ayarlar, örneğin `r3v3rs3.http.old.routes.0.response.type=redirect` ve `r3v3rs3.http.old.routes.0.response.target=https://example.com`. `response` içeren route, container port'undan varsayılan server almaz.

```toml
[old-domain]
protocol = "http"
vhosts = ["old.example.com"]
routes = [{ path = "/", response = { type = "redirect", target = "https://example.com", status = 301 } }]

[catch-all]
protocol = "http"
routes = [{ path = "/", response = { type = "status", status = 404, body = "Not found" } }]
```

## UDP Session'ları

UDP proxy her client adresi için ayrı bir session açar. Her session'ın upstream sunucuya giden kendi socket'i vardır. Bu yüzden upstream sunucu her client'ı farklı bir kaynak porttan görür. r3v3rs3 upstream sunucunun yanıtlarını dinlediği porttan client'a geri gönderir.

İki yönde de `session_idle_timeout` süresince (varsayılan `60s`) paket geçmezse session kapanır. Upstream socket hata verdiğinde veya portun upstream sunucuları ya da idle timeout değeri değiştiğinde de session kapanır. Client'ın sonraki paketi yeni bir session açar. Bir port en fazla 10.000 session tutar. Bu sınıra ulaşılınca r3v3rs3 yeni client'ların paketlerini düşürür.

## Upstream Timeout'ları

r3v3rs3 upstream sunucuyu sınırlı bir süre bekler. Timeout değerlerini `500ms`, `10s` veya `1m` gibi yazın.

- HTTP / HTTPS proxy'sinde `timeouts.connect`, TCP / TLS üzerinden TCP proxy'sinde `connect_timeout` değeri yeni bir upstream bağlantısının DNS sorgusunu, TCP bağlantısını ve TLS handshake'ini sınırlar. Varsayılan değer `10s`'dir.
- HTTP / HTTPS proxy'sinde `timeouts.request` değeri, request başladıktan response header'ları gelene kadar geçen süreyi sınırlar. Yeni bağlantının kurulma süresi de buna dahildir. Varsayılan değer `60s`'dir. `0s` limiti kapatır. Response body'si, WebSocket ve diğer upgrade edilmiş bağlantılar için limit yoktur.
- Bir route, proxy timeout'ları yerine kendi `timeouts` değerini kullanabilir. Route'ta yazılmayan değer proxy değerini değil, varsayılan değeri alır.
- UDP proxy'sinde `session_idle_timeout` değeri boşta kalan client session'ını kapatır. "UDP Session'ları" bölümüne bakın.

Timeout dolduğunda HTTP client'ı 504 Gateway Timeout alır, TCP client bağlantısı kapanır. r3v3rs3 sıfır olan connect timeout ve session idle timeout değerlerini reddeder.

```toml
[my-app]
protocol = "http"
vhosts = ["app.example.com"]
timeouts = { connect = "5s", request = "30s" }
routes = [
  { path = "/", servers = [{ url = "http://127.0.0.1:9000/" }] },
  { path = "/reports", servers = [{ url = "http://127.0.0.1:9001/" }], timeouts = { connect = "5s", request = "5m" } },
]

[my-database]
protocol = "tcp"
upstream_servers = [{ addr = "/ip4/127.0.0.1/tcp/5432" }]
connect_timeout = "3s"
```

## Request Body Boyutu

HTTP / HTTPS proxy'sinde `max_body_size` değeri request body'sini byte cinsinden sınırlar. Varsayılan değer `0`'dır. `0` limiti kapatır. Bir route, proxy değeri yerine kendi `max_body_size` değerini kullanabilir. Route'taki `0` değeri o route için limiti kapatır.

r3v3rs3 `Content-Length` header'ını authentication'dan önce kontrol eder. Bu yüzden limitten büyük bir request 413 Payload Too Large alır ve upstream sunucuya ulaşmaz. `Content-Length` taşımayan body, örneğin chunked body, r3v3rs3 onu upstream sunucuya gönderirken sayılır. Body upstream sunucu yanıt vermeden limiti geçerse r3v3rs3 upstream request'ini durdurur ve client 413 alır. Upstream sunucu böyle bir body'nin başını alabilir. Limit HTTP/1.1, HTTP/2 ve HTTP/3 request'lerine uygulanır.

```toml
[uploads]
protocol = "http"
vhosts = ["files.example.com"]
max_body_size = 1048576
routes = [
  { path = "/", servers = [{ url = "http://127.0.0.1:9000/" }] },
  { path = "/upload", servers = [{ url = "http://127.0.0.1:9000/" }], max_body_size = 104857600 },
]
```

## Trafik Mirroring

Route'un `mirror` değeri route request'lerinin bir kopyasını başka sunuculara gönderir. Örneğin yeni bir sürümü gerçek trafikle denemek için kullanılır. r3v3rs3 mirror sunucuların response'larını atar. Kopyayı retry etmez, health check'e ve circuit breaker'a saymaz ve kopyayı beklemez. Bu yüzden client, route sunucusunun response'unu önceki gibi alır.

- `servers` mirror sunucuların listesidir. Her sunucu her kopyayı alır. Weight değerinin etkisi yoktur. Kopyanın path'i, `rewrite` dahil, route sunucularının path'iyle aynı kurallara uyar.
- `percent` r3v3rs3'ün kopyaladığı request oranıdır. Değer `1` ile `100` (varsayılan) arasındadır.
- `max_body_size` r3v3rs3'ün kopyaladığı en büyük request body'sidir, byte cinsinden. Varsayılan değer `65536`'dır. Body'si daha uzun olan request kopyalanmaz. r3v3rs3 her kopya için memory'de en fazla bu kadar byte tutar.

r3v3rs3 kopyayı request body'sinin tamamını okuduktan sonra gönderir. Hata veren veya client'ın tamamlamadığı body için kopya gönderilmez. Cache'ten gelen response, WebSocket gibi upgrade request'leri ve route'ta 64 kopya gönderilirken gelen request kopyalanmaz. Kopya, header kuralları uygulandıktan sonraki method ve header'ları taşır. Bu yüzden kimlik doğrulamanın kaldırmadığı cookie gibi credential'ları da taşır. Bu verilere güvendiğiniz bir mirror sunucu kullanın. r3v3rs3 `1` ile `100` dışındaki `percent` değerini reddeder.

```toml
[my-api]
protocol = "http"
vhosts = ["api.example.com"]
routes = [
  { path = "/", servers = [{ url = "http://127.0.0.1:9000/" }], mirror = { servers = [{ url = "http://127.0.0.1:9100/" }], percent = 10 } },
]
```

## Load Balancing ve Health Check

Birden fazla upstream sunucusu olan proxy veya HTTP route, trafiği `load_balancing` değerine göre dağıtır:

- `round_robin` (varsayılan) sunucuları sırayla kullanır.
- `random` rastgele bir sunucu seçer.
- `first` ilk sağlıklı sunucuyu kullanır. Diğer sunucular yedektir.
- `client_ip_hash` her client IP adresini, o sunucu sağlıklı kaldıkça aynı sunucuya gönderir. HTTP proxy, belirlenen client IP adresini kullanır (bkz. [Client IP](#client-ip)). Bir sunucu sağlıksız olursa, drain edilirse veya silinirse yalnız o sunucunun client'ları diğer sunuculara geçer.

Her sunucunun `0` ile `65535` arasında bir `weight` değeri vardır (varsayılan `1`). `round_robin` her sunucuyu weight değeri kadar kullanır. nginx'in smooth weighted round robin yöntemindeki gibi, bir sunucunun sıraları döngüye yayılır. Örneğin `3` ve `1` weight değerlerinde her dört request'in üçü ilk sunucuya gider. `random` sunucuyu weight değeriyle orantılı bir olasılıkla seçer. `client_ip_hash` her sunucuya, weight değeriyle orantılı sayıda client adresi verir. `first` weight değerini dikkate almaz. `weight = 0` olan sunucu, diğer bütün sunucular sağlıksız olsa da yeni trafik almaz. Bu yüzden bir sunucuyu silmeden servis dışına alabilirsiniz. Sunucunun açık TCP bağlantıları ve UDP session'ları devam eder. Her proxy'nin veya HTTP route'unun en az bir sunucusunun weight değeri `0`'dan büyük olmalıdır.

HTTP proxy her request için, TCP proxy her bağlantı için, UDP proxy her client session'ı için bir sunucu seçer. Her HTTP route'unun sunucuları ayrı bir gruptur.

TCP proxy seçilen sunucuya bağlanamazsa veya connect timeout dolarsa r3v3rs3 bir kez sıradaki sunucuyu dener.

HTTP proxy, başarısız request'i retry policy'sine göre sıradaki sunucuya yeniden gönderir. `retry.attempts` (varsayılan `2`), bir request'in ilk deneme dahil en fazla kaç kez gönderileceğini belirler ve `1` ile `10` arasında olmalıdır. `attempts = 1` retry'ı kapatır. `retry.retry_on` (varsayılan `["connect"]`) retry başlatan hataları listeler:

- `connect`: bağlantı kurulamaz veya connect timeout dolar. Sunucuya hiçbir şey ulaşmadığı için r3v3rs3 her method'u retry eder.
- `timeout`: request timeout dolar.
- `http_502`, `http_503`, `http_504`: sunucu bu status ile yanıt verir.

`timeout`, `http_502`, `http_503` ve `http_504` yalnız RFC 9110'daki idempotent method'ları retry eder: `GET`, `HEAD`, `OPTIONS`, `TRACE`, `PUT` ve `DELETE`. Request timeout her denemeye ayrı uygulanır. Retry, policy sırasındaki sıradaki sunucuya gider ve circuit'i açık sunucuyu atlar. Son denemeden sonra client son response'u veya hatayı alır.

Body'si olan request yalnız body uzunluğu biliniyorsa (örneğin `Content-Length` ile) ve `retry.replay_body_limit` (varsayılan `0`) byte değerini aşmıyorsa retry edilir. r3v3rs3 bu body'yi memory'de tutar. Değer `0` ise yalnız body'si olmayan request'ler retry edilir. WebSocket gibi upgrade request'leri retry edilmez. Bir route kendi `retry` ayarıyla proxy'nin retry policy'sini değiştirebilir.

Pasif health check her sunucunun art arda aldığı hataları sayar. Hata, kurulamayan bir bağlantı veya response gelmeyen bir request'tir. `health_check.max_fails` (varsayılan `1`) kadar hatadan sonra sunucu `health_check.fail_timeout` (varsayılan `30s`) süresince sağlıksız sayılır. Sağlıksız sunucu yeni trafiği yalnız sağlıklı sunuculardan sonra alır. Bütün sunucular sağlıksızsa r3v3rs3 trafiği aynı sırayla yine onlara gönderir. Başarılı bir deneme hata sayısını sıfırlar. `max_fails = 0` kontrolü kapatır. 500 gibi hata status'lu bir HTTP response başarılı sayılır, çünkü sunucu yanıt vermiştir.

Aktif health check, `health_check.interval` değeri `0s`'den büyükse çalışır (varsayılan `0s`, kapalı). r3v3rs3 her interval'de her sunucuyu kontrol eder:

- `health_check.path` verilen HTTP proxy, her sunucunun kök adresinden bu path'e `GET` gönderir. 2xx veya 3xx status kontrolü geçer. Path `/` ile başlamalıdır ve yalnız HTTP proxy path kullanır.
- Path'i olmayan HTTP proxy ve TCP proxy her sunucuya TCP bağlantısı açar.
- UDP proxy her sunucunun host adını çözümler.

`health_check.timeout` (varsayılan `5s`) her kontrolü sınırlar. Kontrolü geçemeyen sunucu, bir kontrol başarılı olana kadar sağlıksız kalır. Bu kural `max_fails = 0` olduğunda da geçerlidir. Başarılı bir request bu durumu bitirmez.

Sunucular, weight değerleri, policy ve health check ayarları değişmediği sürece config reload sonrasında sunucuların sağlık durumu korunur.

Status API'si (`GET /api/proxies/{id}/status`), her upstream sunucusunun sağlık durumunu `upstreams` alanında listeler: adres, `weight`, `healthy`, art arda hata sayısı `failures` ve `last_error`. WebUI'daki proxy listesi sağlıklı sunucu sayısını gösterir ve status'leri 10 saniyede bir yeniler. Sayının title'ı sağlıksız sunucuları son hatalarıyla listeler.

```toml
[my-app]
protocol = "http"
vhosts = ["app.example.com"]
load_balancing = "round_robin"
health_check = { max_fails = 3, fail_timeout = "10s", interval = "10s", timeout = "2s", path = "/health" }
retry = { attempts = 3, retry_on = ["connect", "http_503"], replay_body_limit = 65536 }
routes = [
  { path = "/", servers = [{ url = "http://10.0.0.1:9000/" }, { url = "http://10.0.0.2:9000/", weight = 3 }] },
]

[my-database]
protocol = "tcp"
load_balancing = "first"
upstream_servers = [
  { addr = "/ip4/10.0.0.1/tcp/5432" },
  { addr = "/ip4/10.0.0.2/tcp/5432" },
]
```

## Circuit Breaker

Circuit breaker, HTTP veya TCP proxy'nin hata veren bir upstream sunucusuna giden trafiği bir süre durdurur. Varsayılan olarak kapalıdır. Her HTTP route'undaki her sunucunun kendi circuit'i vardır. UDP proxy'lerde circuit breaker yoktur.

- HTTP request'i şu durumlarda başarısız sayılır: bağlantı kurulamaz, connect veya request timeout dolar ya da sunucu 502, 503 veya 504 döner. TCP bağlantısı, kurulamazsa başarısız sayılır.
- r3v3rs3 her sunucunun request'lerini `circuit_breaker.window` (varsayılan `10s`) uzunluğundaki window'larda sayar. Bir window en az `circuit_breaker.min_requests` (varsayılan `20`) request içeriyorsa ve bunların en az `circuit_breaker.failure_ratio` (varsayılan `50`) yüzdesi başarısız olduysa circuit açılır.
- Açık circuit, `circuit_breaker.open_duration` (varsayılan `30s`) dolana kadar trafik almaz. Sonra circuit half-open olur ve sunucuya tek bir deneme request'i gider. Başarılı deneme circuit'i kapatır. Başarısız deneme circuit'i yeniden açar.
- Bir HTTP route'unun bütün sunucularının circuit'i açıksa client, sunucuya request gitmeden 503 Service Unavailable alır. TCP client bağlantısı kapanır.

Pasif health check 5xx response'u yine başarılı sayar, çünkü sunucu yanıt vermiştir. Status API'si circuit'i kapalı olmayan sunucu için `"circuit": "open"` veya `"circuit": "half_open"` gösterir. WebUI'daki proxy listesi böyle bir sunucuyu sağlıklı saymaz ve sayının title'ında circuit durumunu yazar.

```toml
[my-app]
protocol = "http"
vhosts = ["app.example.com"]
circuit_breaker = { enabled = true, failure_ratio = 50, min_requests = 20, window = "10s", open_duration = "30s" }
routes = [
  { path = "/", servers = [{ url = "http://10.0.0.1:9000/" }, { url = "http://10.0.0.2:9000/" }] },
]
```

## Sticky Session'lar

`sticky`, HTTP proxy'nin her client'ını bir cookie ile tek bir upstream sunucusunda tutar. Varsayılan olarak kapalıdır.

- Client'a giden ilk response `sticky.name` (varsayılan `r3v3rs3_affinity`) cookie'sini yazar. Cookie değeri proxy'nin, route'un ve sunucu URL'inin HMAC-SHA256 imzasıdır. Bu yüzden client sahte bir değerle sunucu seçemez. Route'un hiçbir sunucusuna uymayan değer dikkate alınmaz.
- Geçerli cookie taşıyan request, sunucu sağlıklı olduğu ve circuit'i açık olmadığı sürece aynı sunucuya gider. `weight = 0` olan sunucu sticky client'larını korur, böylece drain edilen sunucu mevcut session'larını tamamlar. Aksi halde sunucuyu `load_balancing` seçer ve response yeni bir cookie yazar. Başka bir sunucuya giden retry de yeni bir cookie yazar.
- r3v3rs3 cookie'yi request'ten siler, bu yüzden upstream sunucu cookie'yi almaz.
- Cookie route'un `Path` değerini, `HttpOnly` ve `SameSite=Lax` özelliklerini taşır. HTTPS ve HTTP/3'te `Secure` de eklenir. `sticky.max_age`, `Max-Age` değerini belirler. `max_age` yoksa cookie browser kapanınca silinir.
- Ad, RFC 6265'teki cookie token kuralına uymalıdır: boşluk ve ayırıcı karakter içermeyen görünür ASCII karakterler.
- r3v3rs3 her başlangıçta yeni bir imza key'i üretir. Yeniden başlatmadan sonra eski cookie'ler geçersiz olur ve her client sunucusunu yine `load_balancing` ile alır.
- Cache'ten gelen response cookie yazmaz.

TCP veya UDP proxy ve cookie saklamayan client'lar bunun yerine `load_balancing = "client_ip_hash"` kullanabilir.

```toml
[my-app]
protocol = "http"
vhosts = ["app.example.com"]
sticky = { enabled = true, name = "app_server", max_age = "1h" }
routes = [
  { path = "/", servers = [{ url = "http://10.0.0.1:9000/" }, { url = "http://10.0.0.2:9000/" }] },
]
```

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

Bir route, "Bu Route için Ayrı IP Filtresi Kullan" seçeneğiyle proxy listeleri yerine yalnız kendi listelerini kullanır. Route'un iki listesi de boşsa bu route'a her client erişebilir.

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

Bir route, "Bu Route için Ayrı Rate Limit Kullan" seçeneğiyle proxy limiti yerine kendi limitini kullanabilir. Bu seçeneği açmayan route'lar her client için ortak bir sayaç kullanır. Route ayarında request değeri `0` ise o route'ta limit uygulanmaz.

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

Her HTTP / HTTPS proxy'sinde "Kimlik Doğrulama" bölümünden kimlik doğrulamayı zorunlu hale getirebilirsiniz. Bir route, "Bu Route için Ayrı Kimlik Doğrulama Kullan" seçeneğiyle proxy ayarı yerine kendi ayarını kullanabilir. O route'u bütün client'lara açmak için "Yok" seçin.

r3v3rs3 kimlik doğrulamayı IP filtresinden, rate limit'ten ve HTTPS redirect'inden sonra yapar. Bu sayede "HTTP'yi Otomatik Olarak HTTPS'e Yönlendir" seçeneği açıksa tarayıcı kimlik bilgilerini şifreli bağlantı üzerinden gönderir.

### Basic Auth

Geçerli kullanıcı adı ve parola göndermeyen client'lar `WWW-Authenticate: Basic realm="..."` header'ıyla birlikte `401 Unauthorized` alır. Tarayıcı bunun üzerine giriş penceresini açar.

- **Realm**: Tarayıcının giriş penceresinde gösterdiği ad. Boş bırakılırsa `r3v3rs3` kullanılır.
- **Kullanıcılar**: Kullanıcı adları ve parolalar. Kullanıcı adında iki nokta üst üste bulunamaz.

r3v3rs3 parolaları argon2 hash olarak saklar; düz metin parolayı hiçbir zaman kaydetmez. Admin API hash'i döndürmez. Parolası olan kullanıcı için `password_set: true` döndürür. Parolayı değiştirmek istemiyorsanız parola alanını boş bırakın. r3v3rs3, request'i upstream sunucuya göndermeden önce `Authorization` header'ını siler.

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

r3v3rs3 her token'ın SHA-256 digest'ini saklar; düz metin token'ı hiçbir zaman kaydetmez. Digest'leri sabit sürede karşılaştırır. Admin API digest'i döndürmez. Değeri olan token için `token_set: true` döndürür. Token'ı değiştirmek istemiyorsanız token alanını boş bırakın. r3v3rs3, request'i upstream sunucuya göndermeden önce `Authorization` header'ını siler. Bu yüzden bearer kimlik doğrulaması kullanan bir route'ta upstream sunucuya kendi bearer token'ı ulaşmaz.

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

Auth request'i, upstream request'leriyle aynı root sertifikalarına güvenir. Proxy'nin client sertifikasını da gönderir. Ayrıntılar için "Upstream client sertifikaları" bölümüne bakın.

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

Yalnız proxy'yi görebilen hesaplar giriş yapar. Proxy listesi olan bir hesap yalnız listesindeki proxy'leri görür. r3v3rs3 hesabı her request'te kontrol eder. Hesap silinirse, hesap değişirse veya proxy hesabın listesinden çıkarılırsa session sona erer. r3v3rs3 session'ın hesabını kaydetmeye başlamadan önce açılan session'lar geçersizdir. Bu durumda client yeniden giriş yapar.

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

## Erişim listeleri

Erişim listesi, bir IP filtresini ve bir kimlik doğrulamayı bir adla tutar. Birden fazla proxy ve route aynı erişim listesini kullanabilir. Liste değişince değişiklik yeniden başlatma olmadan hepsine uygulanır.

WebUI'daki "Erişim Listeleri" sayfası listeleri ekler, değiştirir ve siler. HTTP / HTTPS proxy'nin "Erişim Listesi" alanı proxy için bir liste seçer. Route'un "Erişim Listesi" alanı route için bir liste seçer. Yönetim API'sinde ve `proxies.toml` dosyasında `access_list` alanı bir proxy'nin veya route'un listesini ayarlar.

- Proxy'nin listesi, proxy'nin "IP Filtresi" ve "Kimlik Doğrulama" ayarlarının yerini alır.
- Route'un listesi, route'un IP filtresinin ve kimlik doğrulamasının yerini alır.
- Kendi listesi veya override'ı olmayan route, proxy'nin ayarlarını kullanır. Proxy'nin listesi de bu ayarlara dahildir.
- Listesi olan proxy veya route kendi IP filtresini veya kimlik doğrulamasını ayarlayamaz. Yönetim API'si `400 access_list_conflict` döndürür.
- Bilinmeyen bir listeyi kullanan proxy `400 access_list_not_found` alır. Liste runtime'da yoksa, örneğin henüz sync olmamış bir cluster'da, proxy veya route her client'a `403 Forbidden` döndürür.
- Bir proxy'nin veya route'un kullandığı liste silinemez. Yönetim API'si `400 access_list_in_use` döndürür.

Config dizinindeki `access_lists.toml` dosyası listeleri tutar. Dosya parola hash'lerini ve token digest'lerini taşır. Bu yüzden r3v3rs3 dosyayı `0600` moduyla yazar. Yönetim API'si, proxy kimlik doğrulamasında olduğu gibi hash'leri döndürmez.

```toml
# access_lists.toml
[office]
name = "Office"
ip_filter = { allow = ["192.168.0.0/16"] }
auth = { type = "basic", realm = "Office", users = [{ username = "alice", password_hash = "$argon2id$v=19$m=19456,t=2,p=1$..." }] }
```

```toml
# proxies.toml
[my-app]
protocol = "http"
vhosts = ["app.example.com"]
access_list = "office"
routes = [
  { path = "/", servers = [{ url = "http://127.0.0.1:9000/" }] },
  { path = "/health", servers = [{ url = "http://127.0.0.1:9000/health" }], ip_filter = {}, auth = { type = "none" } },
]
```

| Endpoint | İşlem |
|---|---|
| `GET /api/access_lists` | Erişim listelerini listeler. |
| `POST /api/access_lists` | Bir erişim listesi ekler. |
| `PUT /api/access_lists/{id}` | Bir erişim listesini değiştirir. Yeni secret'ı olmayan kullanıcı veya token hash'ini korur. |
| `DELETE /api/access_lists/{id}` | Bir erişim listesini siler. |

Her hesap listeleri okur. Listeleri yalnız admin veya proxy listesi olmayan editör değiştirir. Ayrıntılar için [Hesaplar](@/accounts.tr.md) sayfasına bakın.

## Header kuralları

Proxy'den geçen request ve response'ların header'larını "Header Kuralları" bölümünden değiştirebilirsiniz. Bir route, "Bu Route için Ayrı Header Kuralları Kullan" seçeneğiyle proxy kuralları yerine kendi kurallarını kullanabilir.

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
| `{client_cert_subject}` | Doğrulanan client sertifikasının subject değeri, örneğin `CN=client.example.com`. Client sertifikası yoksa boştur. |
| `{client_cert_fingerprint}` | Doğrulanan client sertifikasının hex SHA-256 fingerprint'i. Client sertifikası yoksa boştur. |

Client aynı adlı bir header'ı kendisi de gönderebilir. Client sertifikası header'ları için `append` yerine `set` kullanın. Böylece kural client'ın gönderdiği değeri değiştirir.

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

r3v3rs3 cache'i yalnız `Range`, `Upgrade` ve `Cache-Control: no-store` içermeyen `GET` ve `HEAD` request'lerinde kullanır. `HEAD` request'ine saklanan `GET` response'u verilir. Client `Cache-Control: no-cache` veya `Pragma: no-cache` gönderirse r3v3rs3 cache'e bakmadan request'i upstream sunucuya iletir ve gelen yeni response'u saklar. Cache key, istenen host ile request'in path ve query değerinden oluşur. Bu yüzden load balancing'in seçtiği upstream sunucu key'i değiştirmez.

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

Upstream tarafında r3v3rs3, HTTPS sunucularına ALPN ile `h2` ve `http/1.1` önerir ve sunucunun seçtiği protokolü kullanır. Düz HTTP bağlantısında protokol seçimi yapılamadığı için düz HTTP sunucularına HTTP/1.1 ile bağlanılır. Proxy'nin düz HTTP sunucuları prior knowledge ile HTTP/2 (h2c) kabul ediyorsa `h2c = true` ayarlayın. WebSocket ve diğer upgrade request'leri her zaman HTTP/1.1 kullanır. Bir portta aynı client sertifikasını ve aynı connect timeout değerini kullanan proxy'ler upstream bağlantılarını ortak kullanır. Bu yüzden tek bir HTTP/2 upstream bağlantısı birçok client'ın request'lerini taşır.

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

## Upstream client sertifikaları

Upstream sunucu client sertifikası isteyebilir (mutual TLS). Sertifikayı HTTP / HTTPS proxy'sinin veya TCP / TLS üzerinden TCP proxy'sinin "Client Sertifikası" alanında seçin. Listede private key'i olan client sertifikaları görünür. Ayrıntılar için "Client sertifikaları" bölümüne bakın.

- Proxy sertifikayı, sertifika isteyen her TLS upstream sunucusuna gönderir. Düz HTTP veya düz TCP upstream sunucuları sertifikayı kullanmaz.
- HTTP / HTTPS proxy'sinde forward auth request'i de aynı sertifikayı gönderir.
- Sertifika yoksa, client sertifikası değilse veya private key'i yoksa r3v3rs3 proxy ayarını reddeder. Bir proxy'nin kullandığı client sertifikası silinemez.
- Sertifika geçersiz hale gelirse, örneğin config dizininden silinirse, proxy sertifikasız bağlanmaz. HTTP / HTTPS proxy'si `502 Bad Gateway` döner, TCP proxy'si bağlantıyı kapatır.

TCP proxy'sinde TLS bekleyen upstream sunucusu için "TLS ile Bağlan" seçeneğini açın. Bu sunucunun adresi `/tls` ile biter.

```toml
[my-app]
protocol = "http"
vhosts = ["app.example.com"]
client_cert = "a1b2c3d"
routes = [{ path = "/", servers = [{ url = "https://10.0.0.5:8443/" }] }]

[my-database]
protocol = "tcp"
client_cert = "a1b2c3d"
upstream_servers = [{ addr = "/dns/db.internal/tcp/5433/tls" }]
```

## PROXY protocol gönderme

TCP / TLS üzerinden TCP proxy'si client adresini upstream sunuculara gönderebilir. Versiyonu "PROXY Protocol Gönder" alanında seçin. Bundan sonra her upstream bağlantısı bir PROXY protocol header'ı ile başlar. TLS upstream sunucusunda header TLS handshake'ten önce gider.

- Kaynak adres bağlantının client adresidir. Port PROXY protocol alıyorsa bu adres o header'daki adrestir. "PROXY protocol" bölümüne bakın.
- Hedef adres client'ın bağlandığı adrestir.
- İki adresin ailesi farklıysa ikisi de IPv6 adresi olarak yazılır. IPv4 adresi IPv4-mapped IPv6 adresine dönüşür.
- Aktif health check versiyon 2 `LOCAL` header'ı gönderir.

Bunu yalnız bütün upstream sunucular header'ı okuyorsa açın, çünkü header'ı okumayan sunucu onu veri olarak alır. HTTP / HTTPS proxy'leri PROXY protocol göndermez. Onların yerine `Forwarded` ve `X-Forwarded-For` header'larını kullanın.

```toml
[my-mail]
protocol = "tcp"
proxy_protocol = "v2"
upstream_servers = [{ addr = "/dns/mail.internal/tcp/25" }]
```

# Sertifikalar

## Sunucu sertifikaları

TLS üzerinden TCP ve HTTPS proxy'leri için bir sunucu sertifikası gerekir. Sunucu sertifikasını üç yolla ekleyebilirsiniz:

1. Self-signed bir sertifika oluşturun.
2. Bir dosyadan sertifika içe aktarın (yalnız PEM formatı).
3. Sertifikayı [ACME](https://letsencrypt.org/how-it-works/) ile otomatik alın.

r3v3rs3, TLS client hello mesajındaki SNI (Server Name Indication) değerine göre uygun sertifikayı otomatik olarak seçer.

## Client sertifikaları

TLS sunucusu, client'ı doğrulamak için client sertifikası isteyebilir. "Client Sertifikaları" sekmesinde client sertifikasını iki yolla ekleyebilirsiniz:

1. Self-signed bir sertifika oluşturun ve sertifika türü olarak "Client Sertifikası" seçin. r3v3rs3 sertifikaya `clientAuth` extended key usage değerini ekler. Seçilen CA sertifikası sertifikayı imzalar.
2. Sertifika zincirini ve private key'i dosyadan içe aktarın (yalnız PEM formatı). Client sertifikası için private key gerekir.

Proxy, client sertifikasını upstream sunucularına gönderir. Ayrıntılar için "Upstream client sertifikaları" bölümüne bakın.

## Root sertifikaları

Upstream sunucunuz sistemin güvenmediği sertifikalar kullanıyorsa bu sertifikaları root sertifika deposuna eklemeniz gerekir. r3v3rs3, sistemin root sertifikalarına ek olarak bu depodaki root sertifikalarının imzaladığı bütün sertifikalara da otomatik olarak güvenir.

Self-signed bir sertifika oluşturduğunuzda r3v3rs3 bir CA sertifikası da oluşturur ve onu root sertifika deposuna ekler.

## Süre uyarıları

Sertifika listesi, "Sertifika Süre Uyarısı" (varsayılan `14days`) süresi içinde sona erecek sertifikayı "Süresi dolmak üzere" ile işaretler. Süresi dolmuş sertifikayı "Süresi doldu" ile işaretler.

## Bildirimler

r3v3rs3 şu olaylar için bildirim webhook'una bir JSON `POST` isteği gönderir:

| Olay | Ne zaman |
|---|---|
| `certificate_expiring` | Bir sertifika "Sertifika Süre Uyarısı" süresi içinde sona erer. |
| `certificate_expired` | Bir sertifikanın süresi dolmuştur. |
| `acme_order_failed` | Bir ACME order başarısız olmuştur. Başarısız her order bir olay gönderir. |
| `test` | Bir admin test bildirimi göndermiştir. |

```json
{"event": "certificate_expiring", "time": 1757894400, "node": "proxy-1", "certificate": {"id": "a1b2c3d", "san": ["example.com"], "not_after": 1759104000}}
```

- `time` saniye cinsinden Unix zamanıdır. `node` cluster node'unun adıdır. Cluster yoksa bu alan bulunmaz. `certificate` alanı sertifika olayının sertifikasını gösterir. `acme` alanı ACME kaydının `id` ve `identifiers` değerlerini taşır. `error` alanı `acme_order_failed` olayının hatasını açıklar.
- Token varsa istek `Authorization: Bearer <token>` header'ını taşır. Admin API token'ı döndürmez.
- 2xx status başarı sayılır. r3v3rs3 başarısız isteği en fazla üç kez gönderir: 1 saniye sonra bir kez daha, 2 saniye sonra bir kez daha. "Webhook Timeout" (varsayılan `10s`) her denemeyi sınırlar.
- Leader sertifikaları her "Arka Plan Görevi Aralığı" süresinde kontrol eder. Bir sertifikanın her olayı bir kez gönderilir. Yenilenen sertifikanın fingerprint'i değişir. Bu yüzden onun olayları yeniden gönderilir. r3v3rs3 gönderilen olayları config dizinindeki `notifications.json` dosyasında veya cluster store'da tutar.
- r3v3rs3 bildirimleri arka planda gönderir. Kuyruk en fazla 64 bildirim tutar. Kuyruk doluysa r3v3rs3 bildirimi göndermez ve bir hata log'u yazar.
- Webhook URL'i HTTPS kullanmalıdır. HTTP yalnız loopback adresinde kullanılabilir.

Webhook'u "Ayarlar" sayfasının "Bildirimler" bölümünde ayarlayın. "Test Bildirimi Gönder" butonu kayıtlı ayarlardaki webhook'a bir `test` olayı gönderir. `POST /api/config/notifications/test` aynı işi yapar. Bu endpoint'i yalnız admin hesabı çağırabilir. Webhook ayarlı değilse yanıt `400 notification_webhook_missing` olur. Webhook isteği başarısız olursa yanıt `502 notification_failed` olur.

```toml
[notifications]
cert_expiry_warning = "14days"
webhook = { url = "https://hooks.example.com/r3v3rs3", token = "<token>", timeout = "10s" }
```

`config.toml` dosyası token'ı düz metin olarak tutar.

## Birden fazla sertifikayı silme

Listedeki checkbox'larla sertifikaları seçin ve "Seçilenleri Sil" butonuna tıklayın. `{"ids": [...]}` body'si ile gönderilen `POST /api/certs/delete` isteği en fazla 200 id için aynı işi yapar. Yanıt, istekteki sırayla her id için bir sonuç taşır:

| Sonuç | Anlamı |
|---|---|
| `deleted` | r3v3rs3 sertifikayı sildi. |
| `in_use` | Bir port, proxy veya discovery provider sertifikayı kullanıyor. Sertifika kalır. |
| `read_only` | Sertifikayı servis keşfi yönetiyor. Sertifika kalır. |
| `not_found` | Bu id'ye sahip sertifika yok. |
| `failed` | Storage sertifikayı silmedi. Sunucu log'u nedeni yazar. |

İstekte tekrar eden bir id tek bir sonuç alır.

# ACME

r3v3rs3, sertifikaları [ACME](https://letsencrypt.org/docs/client-options/) (Automatic Certificate Management Environment) ile otomatik alabilir. Let's Encrypt, ZeroSSL ve Google Trust Services gibi birçok sertifika otoritesi ACME'yi destekler.

Bir ACME kaydı bir veya daha fazla domain adı içerir. Domain adlarını "Domain Adları" alanına virgülle ayırarak yazın, örneğin `example.com, *.example.com`. Sertifika her domain adını Subject Alternative Name olarak içerir. r3v3rs3 sertifikayı süresi dolmadan otomatik yeniler. Bir order başarısız olursa r3v3rs3 bir saat sonra yeniden order oluşturur.

## Challenge'lar

Sertifika otoritesi, her domain adını sizin yönettiğinizi bir challenge ile doğrular. Challenge'ı "Challenge" alanından seçin.

| Challenge | Nasıl çalışır | Gereksinimler |
|---|---|---|
| HTTP-01 | Sertifika otoritesi `http://<domain>/.well-known/acme-challenge/<token>` adresine istek gönderir ve r3v3rs3 yanıt verir. | Her domain adı r3v3rs3'e çözümlenmeli, TCP 80 portu açık ve internetten erişilebilir olmalıdır. Wildcard domain adı kullanılamaz. |
| TLS-ALPN-01 | Sertifika otoritesi, domain adının 443 portuna `acme-tls/1` ALPN protokolüyle bir TLS bağlantısı açar ve r3v3rs3 bir challenge sertifikasıyla yanıt verir. | Her domain adı r3v3rs3'e çözümlenmeli, TCP 443 portu açık ve internetten erişilebilir olmalıdır. Wildcard domain adı kullanılamaz. |
| DNS-01 | r3v3rs3, DNS provider'ınızın API'si ile `_acme-challenge.<domain>` TXT kaydını oluşturur. | Aşağıdaki tablodaki DNS provider'larından biri ve zone'u düzenleyebilen bir API credential'ı. |

`*.example.com` gibi bir wildcard domain adı DNS-01 gerektirir. r3v3rs3, HTTP-01 veya TLS-ALPN-01 ile girilen wildcard domain adını reddeder.

TLS-ALPN-01 challenge'ı sürerken her TLS portu ve her HTTPS portu, yalnız `acme-tls/1` sunan bir client'a challenge sertifikasıyla yanıt verir. Diğer client'lar portun sertifikasını alır. TLS portu, challenge bağlantısı için upstream sunucusuna bağlanmaz. Hiçbir TCP veya HTTP portu "TLS-ALPN Challenge Adresi" ayarındaki portu kullanmıyorsa r3v3rs3, challenge'lar bitene kadar bu adresi dinler. 443 portundaki TLS'siz bir HTTP portu challenge'a yanıt veremez.

## DNS-01

r3v3rs3 her domain adı için şu adımları uygular:

1. Provider API'si ile domain adının zone'unu bulur. Adı içeren en uzun zone kullanılır.
2. `_acme-challenge.<domain>` TXT kaydını 60 saniyelik TTL ile oluşturur. Linode'da TTL, Linode'un kabul ettiği en düşük değer olan 300 saniyedir. Porkbun'a TTL gönderilmez, bu yüzden kayıt hesabın en düşük TTL değerini alır. Gandi'de TTL, Gandi'nin kabul ettiği en düşük değer olan 300 saniyedir. deSEC'te TTL 3600 saniyedir, çünkü deSEC domain'in minimum TTL değerinden düşük bir TTL'i reddeder. Gandi, deSEC, Azure DNS ve Google Cloud DNS bir adın bütün kayıt kümesini yazar. Bu yüzden r3v3rs3 kendi değerlerini mevcut TXT değerlerine ekler ve yalnız kendi değerlerini siler. `*.example.com` için kayıt adı `example.com` ile aynıdır: `_acme-challenge.example.com`. Bu yüzden kayıt iki değer taşır.
3. TXT değerleri görünene kadar DNS'i 5 saniyede bir sorgular, en fazla 5 dakika bekler. Sorgulanan DNS sunucusunu "DNS Challenge Resolver" ayarı belirler. Ayar boşsa r3v3rs3 sistem resolver'ını kullanır.
4. Sertifika otoritesine challenge'ların hazır olduğunu bildirir ve doğrulama için en fazla 3 dakika bekler.
5. TXT kayıtlarını siler. Order başarısız olsa da kayıtları siler.

Sistem resolver'ı cache'teki eski yanıtları döndürebilir. Propagation kontrolü sık başarısız oluyorsa "DNS Challenge Resolver" ayarına `1.1.1.1:53` gibi public bir resolver veya zone'un authoritative name server'ını yazın.

| DNS provider | Credential'lar | Gereken izinler |
|---|---|---|
| Cloudflare | API Token | Zone için `Zone:Read` ve `DNS:Edit`. |
| Route 53 | Access Key ID, Secret Access Key | `route53:ListHostedZones` ve `route53:ChangeResourceRecordSets`. Private hosted zone'lar atlanır. |
| Azure DNS | Tenant ID, Client ID, Client Secret, Subscription ID | Zone'larda DNS Zone Contributor rolü olan bir service principal. r3v3rs3, subscription'daki DNS zone'larını listeler ve resource group'u zone ID'sinden alır. |
| Google Cloud DNS | Service Account Key (JSON), Project ID | Zone'ların projesinde DNS Administrator rolü (`roles/dns.admin`) olan bir service account'un JSON key dosyası. Project ID boşsa r3v3rs3 key'in projesini kullanır. Private zone'lar atlanır. |
| deSEC | API Token | Hesabın bir token'ı. Policy ile sınırlanmış bir token, `_acme-challenge` TXT kayıt kümelerine yazma izni vermelidir. |
| DigitalOcean | API Token | Domain'leri okuyabilen, domain kayıtlarını oluşturup silebilen bir token. |
| Gandi | API Token | Domain'leri okuyabilen ve LiveDNS kayıtlarını değiştirebilen bir personal access token. |
| Hetzner Cloud | API Token | Okuma ve yazma yetkisi olan bir Hetzner Cloud proje token'ı. Zone, Hetzner Cloud DNS'te olmalıdır. |
| Linode | API Token | Domains için okuma ve yazma yetkisi olan bir personal access token. |
| Vultr | API Key | Hesabın API key'i. |
| Porkbun | API Key, Secret API Key | Porkbun domain yönetiminde domain için "API Access" açık olmalıdır. |
| OVHcloud | API Endpoint, Application Key, Application Secret, Consumer Key | `GET /domain/zone`, `POST /domain/zone/*` ve `DELETE /domain/zone/*` yetkileri olan bir consumer key. Endpoint `ovh-eu`, `ovh-ca`, `ovh-us`, `kimsufi-eu`, `kimsufi-ca`, `soyoustart-eu` veya `soyoustart-ca` olabilir. r3v3rs3, kayıtları oluşturduktan sonra ve sildikten sonra zone'u refresh eder. |
| Webhook | Webhook URL, Bearer Token | TXT kayıtlarını oluşturan ve silen kendi servisiniz. Aşağıdaki "DNS webhook" bölümüne bakın. |
| Exec | Program Yolu | r3v3rs3 host'unda TXT kayıtlarını oluşturan ve silen bir program. Aşağıdaki "DNS exec" bölümüne bakın. |
| RFC 2136 | DNS Sunucusu, Zone, TSIG Key Adı, TSIG Algoritması, TSIG Secret | Dynamic update kabul eden bir DNS sunucusu, örneğin BIND, Knot DNS veya PowerDNS. Aşağıdaki "RFC 2136" bölümüne bakın. |

r3v3rs3 bu API'lerin mock sunucularıyla ve [Pebble](https://github.com/letsencrypt/pebble) test sertifika otoritesiyle test edilir. Gerçek provider hesaplarıyla test edilmez.

## DNS webhook

Webhook provider'ı TXT kayıtlarını bir DNS hosting servisinin API'si yerine kendi servisinize gönderir. r3v3rs3 her challenge adı için webhook URL'ine bu JSON body ile bir `POST` isteği gönderir:

```json
{"action": "add", "fqdn": "_acme-challenge.example.com", "values": ["<TXT değeri>"]}
```

- `action` doğrulamadan önce `add`, doğrulamadan sonra `remove` olur. Bir `add` isteği başarısız olursa r3v3rs3 `remove` isteği de gönderir.
- `fqdn`, sondaki nokta olmadan TXT kaydının adıdır. `values`, adın bütün TXT değerlerini taşır.
- Servis, adın diğer TXT değerlerini korumalıdır.
- Bearer token varsa istek `Authorization: Bearer <token>` header'ını taşır.
- 2xx status başarı sayılır. r3v3rs3 yanıt için en fazla 30 saniye bekler.

Webhook URL'i HTTPS kullanmalıdır. HTTP yalnız loopback adresinde kullanılabilir, örneğin `http://127.0.0.1:8080/acme`.

## DNS exec

Exec provider'ı her TXT değeri için r3v3rs3 host'unda bir program çalıştırır:

```
<program> add <fqdn> <değer>
<program> remove <fqdn> <değer>
```

- r3v3rs3 programı shell kullanmadan doğrudan başlatır. Program boş bir environment alır ve standart girdi almaz.
- `fqdn`, sondaki nokta olmadan TXT kaydının adıdır.
- Program, adın diğer TXT değerlerini korumalıdır.
- Exit status 0 başarı sayılır. Diğer her exit status başarısızlıktır ve hata mesajı standart hata çıktısının başını taşır.
- Bir `add` çağrısı başarısız olursa r3v3rs3, adın eklediği değerleri ve başarısız değer için `remove` çalıştırır.
- Timeout dolunca r3v3rs3 programı durdurur ve çağrı başarısız olur.

Program, `config.toml` dosyasının `[acme_exec]` bölümünde olmalıdır:

```toml
[acme_exec]
programs = ["/usr/local/bin/r3v3rs3-dns-hook"]
timeout = "30s"
```

- Bu bölümü yalnız dosya belirler. Admin API ve WebUI bu bölümü değiştiremez. Bölümü düzenledikten sonra r3v3rs3'ü yeniden başlatın.
- Provider'ın programı ve `programs` listesindeki her giriş mutlak yol olmalıdır. r3v3rs3 sembolik link'leri çözer ve çözülen yolları karşılaştırır.
- r3v3rs3 programı ACME girişini eklediğinizde ve her çalıştırmadan önce kontrol eder.
- `timeout`, tek bir çağrının en uzun çalışma süresidir. Varsayılan değer `30s`.

## RFC 2136

RFC 2136 provider'ı, zone'un primary DNS sunucusuna dynamic update gönderir. Her mesaj bir TSIG imzası taşır.

- **DNS Sunucusu**, primary sunucunun `host:port` biçimindeki adresidir, örneğin `ns1.example.com:53`. r3v3rs3 mesajları TCP üstünden gönderir.
- **Zone**, adların zone'udur, örneğin `example.com`. Boş olursa r3v3rs3 her TXT adı için sunucudan SOA kaydını ister ve bu kaydın owner adını kullanır.
- **TSIG Key Adı**, **TSIG Algoritması** ve **TSIG Secret**, sunucudaki key ile aynı olmalıdır. Algoritma `hmac-sha256`, `hmac-sha384` veya `hmac-sha512` olabilir. Secret base64 biçimindedir.
- Tek bir update, bir adın bütün TXT değerlerini ekler. Doğrulamadan sonra ikinci bir update yalnız bu değerleri siler. Adın diğer TXT değerleri kalır.
- r3v3rs3 her yanıtın TSIG imzasını doğrular. r3v3rs3 ile sunucunun saatleri arasındaki fark en fazla 300 saniye olabilir.

Bir BIND örneği. `tsig-keygen -a hmac-sha256 r3v3rs3` komutu yeni bir secret ile key bloğunu yazar:

```
key "r3v3rs3" {
    algorithm hmac-sha256;
    secret "<base64 secret>";
};

zone "example.com" {
    type primary;
    file "example.com.zone";
    update-policy {
        grant r3v3rs3 name _acme-challenge.example.com. TXT;
    };
};
```

Her challenge adı için bir `grant` kuralı ekleyin. `*.example.com` sertifikası da `_acme-challenge.example.com` adını kullanır.

## Saklanan veriler

r3v3rs3, ACME kayıtlarını config dizinindeki `acme.toml` dosyasında saklar. Dosya, her ACME hesabının private key'ini ve DNS provider credential'larını düz metin olarak içerir. Unix'te r3v3rs3 dosyayı `0600` izniyle oluşturur ve yazar. Böylece dosyayı yalnız process'in sahibi okuyabilir. Yönetim API'si ve WebUI credential'ları hiçbir zaman döndürmez. ACME listesi yalnız provider adını gösterir, örneğin `Let's Encrypt (DNS-01, Cloudflare)`.

ACME kayıtlarını WebUI'dan oluşturun. r3v3rs3, ACME hesabını kayıt eklendiğinde oluşturur. `acme.toml` içindeki bir kayıt şöyle görünür:

```toml
version = "0.3.40"

[acme1]
provider = "Let's Encrypt"
renewal_days = 60
identifiers = ["example.com", "*.example.com"]
challenge_type = "dns-01"

[acme1.dns_provider]
provider = "cloudflare"
api_token = "<Cloudflare API token>"

[acme1.account]
id = "https://acme-v02.api.letsencrypt.org/acme/acct/123456789"
key_pkcs8 = "<hesabın private key'i>"
directory = "https://acme-v02.api.letsencrypt.org/directory"
```

`dns_provider` altındaki `provider` değeri `cloudflare`, `route53`, `digitalocean`, `hetzner`, `linode`, `vultr`, `gandi`, `desec`, `porkbun`, `ovh`, `azure`, `google_cloud`, `webhook`, `exec` veya `rfc2136` olabilir. Route 53, `api_token` yerine `access_key_id` ve `secret_access_key` kullanır. Porkbun, `api_key` ve `secret_api_key` kullanır. OVHcloud, `endpoint`, `application_key`, `application_secret` ve `consumer_key` kullanır. Azure DNS, `tenant_id`, `client_id`, `client_secret` ve `subscription_id` kullanır. Google Cloud DNS, `service_account_key` ve isteğe bağlı `project_id` kullanır. Webhook, `url` ve isteğe bağlı `token` kullanır. Exec, `program` kullanır. RFC 2136; `server`, isteğe bağlı `zone`, `key_name`, `key_algorithm` ve `key_secret` kullanır. Vultr API key'i `api_token` alanına yazılır.

# Ayarlar

WebUI'daki "Ayarlar" bölümünden, `config.toml` dosyasında saklanan ve bütün sunucuyu etkileyen seçenekleri değiştirebilirsiniz. Değişiklikler hemen uygulanır ve dosyaya yazılır.

| Ayar | Varsayılan | Açıklama |
|---|---|---|
| Session Süresi | `1h` | Yönetim paneli session'ının geçerlilik süresi. En az 5 dakika olabilir. |
| Maksimum Giriş Denemesi | `10` | Her client IP adresi ve kullanıcı adı için izin verilen başarısız giriş sayısı. |
| Giriş Denemesi Sıfırlama Süresi | `15m` | Limite ulaşıldıktan sonraki bekleme süresi. |
| Arka Plan Görevi Aralığı | `1h` | Sertifika yenileme ve log temizleme görevlerinin çalışma aralığı. |
| HTTP Challenge Adresi | `0.0.0.0:80` | ACME HTTP challenge'larının dinlendiği adres. |
| TLS-ALPN Challenge Adresi | `0.0.0.0:443` | Hiçbir port bu portu kullanmıyorsa ACME TLS-ALPN-01 challenge'larının dinlendiği adres. |
| DNS Challenge Resolver | boş | r3v3rs3'ün DNS-01 challenge'ının TXT kayıtları görünene kadar sorguladığı DNS sunucusu, örneğin `1.1.1.1:53`. Boş bırakılırsa sistem resolver'ı kullanılır. |
| Veritabanı Log Saklama Süresi | `3months` | Log'ların log veritabanında ne kadar tutulacağı. |
| Audit Log Saklama Süresi | `1year` | Audit log'daki bir kaydın ne kadar tutulacağı. Ayrıntılar için [Audit log](#audit-log) bölümüne bakın. |
| Sertifika Süre Uyarısı | `14days` | Sertifika listesi bu süre içinde sona erecek sertifikayı işaretler. Webhook bu sertifika için bildirim alır. Ayrıntılar için [Bildirimler](#bildirimler) bölümüne bakın. |
| Webhook URL | boş | Bildirim webhook'u. Boşsa bildirim gönderilmez. |
| Webhook Token | boş | Webhook isteklerinin bearer token'ı. Admin API bu değeri döndürmez. |
| Webhook Timeout | `10s` | Tek bir webhook isteğinin en uzun süresi. |

Süreleri `30s`, `15m`, `1h` veya `7days` gibi okunabilir bir biçimde yazın.

# Config dosyaları

r3v3rs3 config'ini TOML dosyalarında saklar. Bu dosyaların konumu işletim sistemine göre değişir:

- Linux: `$XDG_CONFIG_HOME/r3v3rs3` veya `$HOME/.config/r3v3rs3`
- macOS: `$HOME/Library/Application Support/r3v3rs3`
- Windows: `%APPDATA%\r3v3rs3\config`

Varsayılan konumu `R3V3RS3_CONFIG_DIR` environment variable'ı veya `--config-dir` komut satırı seçeneğiyle değiştirebilirsiniz.

Bu dosyaları elle de düzenleyebilirsiniz. Ancak r3v3rs3 config dosyalarındaki değişiklikleri kendiliğinden algılamaz. Değişikliklerin geçerli olması için dosyayı düzenledikten sonra sunucuyu yeniden başlatın.

Cluster'daki bir node yalnız `config.toml` dosyasını okur. State'in geri kalanı etcd veya Consul'dadır. Ayrıntılar için [Cluster](@/cluster.tr.md) sayfasına bakın.

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

# Audit log

r3v3rs3, bir hesabın WebUI veya yönetim API'si ile yaptığı değişiklikleri kaydeder: portlar, proxy'ler, erişim listeleri, sertifikalar, ACME kayıtları, ayarlar, CDN IP aralığı yenilemeleri ve hesaplar. Yönetim paneline her giriş, her başarısız giriş denemesi ve her çıkış da kaydedilir. r3v3rs3'ün kendi yaptığı değişiklikler kaydedilmez. Sertifika yenileme ve keşfedilen proxy'ler buna örnektir.

Her kayıtta zaman, hesap, client IP adresi, işlem, değişen kaynağın id'si ve kısa bir özet bulunur. Özet isimleri, adresleri ve rolleri içerir. Parola, token veya key içermez.

Tek sunucu audit log'u log dizinindeki `log.db` dosyasının `audit_log` tablosunda tutar. Cluster audit log'u cluster store'da şifreli tutar. Böylece her node bütün node'ların kayıtlarını okur. Cluster'daki bir kayıt, onu yazan node'un adını da içerir.

Bir kaydın ne kadar tutulacağını "Audit Log Saklama Süresi" ayarı belirler. Varsayılan değer `1year` olur. Cluster bir günün kayıtlarını birlikte siler. Silme, o gün saklama süresini geçtikten sonra yapılır.

Audit log'a yazma başarısız olursa değişiklik geri alınmaz. r3v3rs3 hatayı log'a yazar.

WebUI'daki Audit Log sayfası kayıtları en yeni kayıttan başlayarak listeler. Bu sayfayı yalnız admin hesabı açar. Sayfa kayıtları hesaba, kaynağa ve döneme göre filtreler. Sayfada en fazla 500 kayıt görünür.

`GET /api/audit` aynı kayıtları döner. Bu endpoint'i yalnız admin hesabı çağırabilir. Query parametrelerinin hepsi isteğe bağlıdır:

| Parametre | Açıklama |
|---|---|
| `since` | Unix milisaniye cinsinden en erken zaman. Varsayılan değer, `until` değerinden 31 gün öncesidir. |
| `until` | Unix milisaniye cinsinden en geç zaman. Varsayılan değer şu andır. |
| `username` | Kayıtların ait olduğu hesap. |
| `resource_id` | Değişen kaynağın id'si veya değişen kullanıcı adı. |
| `limit` | Yanıttaki en fazla kayıt sayısı. Varsayılan değer `100`, en yüksek değer `500` olur. |

Cluster, `until` değerinden en fazla 31 gün geriye okur. Bu yüzden daha uzun bir dönem daha eski kayıtları döndürmez.

# Log

r3v3rs3 varsayılan olarak log'ları standart çıktıya yazar. Bunu `R3V3RS3_LOG`, `R3V3RS3_ACCESS_LOG` environment variable'larıyla veya `--log`, `--access-log` komut satırı seçenekleriyle değiştirebilirsiniz.

```bash
$ r3v3rs3 start --log /var/log/r3v3rs3.log --access-log /var/log/r3v3rs3-access.log
```

Log seviyesini değiştirmek için `R3V3RS3_LOG_LEVEL`, `R3V3RS3_ACCESS_LOG_LEVEL` environment variable'larını veya `--log-level`, `--access-log-level` komut satırı seçeneklerini kullanın.
