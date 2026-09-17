+++
title = "nginx veya Traefik'ten geçiş"
description = "Config'inizi çevirin, iki proxy'yi yan yana çalıştırın ve trafiği devredin"
weight = 16
+++

# nginx veya Traefik'ten geçiş

Bu rehber, bir nginx veya Traefik config'inde bulunan parçaları çevirir ve trafiği kesintisiz devreder.

Plan şudur: r3v3rs3'ü başka bir portta çalıştırın, config'i orada kurun, `Host` header'ı ile test edin, sonra portu taşıyın.

## Adım 1: Elinizdekini listeleyin

Mevcut config'inizdeki her maddeyi yazın. Çoğu kurulumda beş tür madde vardır:

1. Server block'ları veya router'lar: hangi host nereye gidiyor.
2. Location'lar veya rule'lar: hangi path nereye gidiyor ve path nasıl değişiyor.
3. Sertifikalar: dosya veya ACME.
4. Ek özellikler: redirect, header, basic auth, rate limit, IP filtresi, cache.
5. Upstream'ler: sunucular, weight değerleri ve health check'ler.

Aşağıdaki tablolardaki her maddenin r3v3rs3 karşılığı vardır. Tabloda olmayan bir şeyin, örneğin bir Lua script'inin veya bir nginx modülünün, karşılığı yoktur; o parça için başka bir çözüm gerekir.

## Adım 2: nginx direktiflerini çevirin

| nginx | r3v3rs3 |
|---|---|
| `listen 443 ssl;` | **HTTPS** protokollü bir port ve **TLS Termination** |
| `listen 443 quic;` | **QUIC üzerinden HTTP (HTTP/3)** protokollü bir port |
| `server_name app.example.com;` | Proxy'nin **Virtual Host'lar** alanı |
| `location /api { }` | Path'i `/api` olan bir route |
| `proxy_pass http://api:8080/v1/;` | Route'ta `http://api:8080/v1/` sunucusu |
| `upstream app { server a; server b; }` | Bir route'ta iki sunucu |
| `server a weight=3;` | Sunucunun **Weight** değeri `3` |
| `return 301 https://$host$request_uri;` | **HTTP'yi Otomatik Olarak HTTPS'e Yönlendir** |
| `rewrite ^/items/([0-9]+)$ /item/$1 break;` | Route'ta **Path Regex'i** ve **Yerine Yazılacak Değer** |
| `add_header X-Frame-Options DENY;` | Response header kuralı: `set X-Frame-Options: DENY` |
| `auth_basic` ve `auth_basic_user_file` | Proxy'de kullanıcılarıyla **Basic Auth** |
| `auth_request /auth;` | Auth URL'i ile **Forward Auth** |
| `allow` ve `deny` | **IP Filtresi** allow ve deny listeleri |
| `limit_req_zone` ve `limit_req` | Request sayısı, süre ve burst ile **Rate Limit** |
| `proxy_cache_path` ve `proxy_cache` | Memory limiti ve TTL ile **Cache** |
| `gzip on;` | Algoritmalar ve minimum boyut ile **Compression** |
| `return 404;` | **Sabit status kodu** route tipi |
| `proxy_set_header X-Real-IP $remote_addr;` | Otomatiktir. [Client IP](@/configuration.tr.md#client-ip) bölümüne bakın |
| `client_max_body_size 10m;` | **Request Body Limiti** |
| `proxy_read_timeout 60s;` | Proxy'nin veya route'un **Request Timeout'u** |
| `stream { server { proxy_pass db:5432; } }` | TCP proxy |

Zaman kaybettiren iki fark:

- **Location'ların sırası sonucu değiştirmez.** nginx, kendi öncelik kurallarına ve dosyadaki sıraya göre seçer. r3v3rs3 request'i her zaman, portun bütün proxy'leri içinde path'i en uzun eşleşen route'a gönderir. `/api`, `/api` ve `/api/users` ile eşleşir, `/apiv2` ile eşleşmez.
- **Sondaki slash kuralı aynı değildir.** nginx'te `proxy_pass http://api:8080/` location prefix'ini kaldırır, `proxy_pass http://api:8080` ise korur. r3v3rs3'te route path'i her zaman kaldırılır ve kalan kısım sunucu URL'inin path'ine eklenir:

```
route /api, sunucu http://api:8080/v1/   ->  GET /api/users  =  /v1/users
route /keep, Route Path'ini Kaldır kapalı ->  GET /keep/x    =  /keep/x
```

Query string iki durumda da korunur.

## Adım 3: Traefik label'larını çevirin

| Traefik | r3v3rs3 |
|---|---|
| `entrypoints` | Portlar |
| `Host(\`app.example.com\`)` | Proxy'nin **Virtual Host'lar** alanı |
| `PathPrefix(\`/api\`)` | Path'i `/api` olan bir route |
| `PathPrefix` olmayan router | Path'i `/` olan bir route |
| `loadBalancer.servers` içeren `service` | Route'un sunucuları |
| `loadBalancer.healthCheck` | Path ve aralık ile **Health Check** |
| `loadBalancer.sticky.cookie` | Cookie adıyla **Sticky Cookie'yi Aç** |
| `middlewares.stripPrefix` | Route'un varsayılan davranışı |
| `middlewares.addPrefix` | **Eklenecek Prefix** |
| `middlewares.redirectScheme` | **HTTP'yi Otomatik Olarak HTTPS'e Yönlendir** |
| `middlewares.redirectRegex` | `regex`, `target` ve `status` içeren redirect kuralı |
| `middlewares.basicAuth` | **Basic Auth** |
| `middlewares.forwardAuth` | **Forward Auth**. [Forward auth ile single sign-on](@/tutorials/forward-auth.tr.md) rehberine bakın |
| `middlewares.ipAllowList` | **IP Filtresi** |
| `middlewares.rateLimit` | **Rate Limit** |
| `middlewares.headers` | Header kuralları |
| `middlewares.compress` | **Compression** |
| ACME ile `certresolver` | ACME kaydı. [Wildcard sertifika ile HTTPS](@/tutorials/https-certificates.tr.md) rehberine bakın |
| Docker label'ları | Docker service discovery. [Docker'dan proxy'ler](@/tutorials/docker-discovery.tr.md) rehberine bakın |
| `tcp` router | TCP proxy |

Docker label'larıyla kurulmuş bir Traefik kurulumu en az işi ister: Docker provider'ını açın ve `traefik.` label'larının yanına `r3v3rs3.` label'larını yazın. İki proxy de aynı container'ları okur, hiçbir şeyi silmeden ikisini karşılaştırabilirsiniz.

## Adım 4: Başka bir portta kurun

Port 80 ve 443'e henüz dokunmayın. r3v3rs3'e boş bir port verin:

1. **HTTP** protokolü ile `0.0.0.0:8080` portunu ekleyin.
2. Bütün proxy'leri gerçek virtual host'larıyla kurun.
3. Her birini `Host` header'ı ile test edin:

```bash
$ curl -s -o /dev/null -w '%{http_code}\n' -H 'Host: app.example.com' http://127.0.0.1:8080/
200
$ curl -s -H 'Host: app.example.com' http://127.0.0.1:8080/api/users
```

DNS'te hiçbir şey değişmez, kullanıcılarınız eski proxy'de kalır. Adım 1'deki listedeki her madde eskisi gibi cevap verene kadar çalışın.

HTTPS için `0.0.0.0:8443` portunu ekleyin ve `--resolve` kullanın:

```bash
$ curl -sI --resolve app.example.com:8443:127.0.0.1 https://app.example.com:8443/
```

## Adım 5: Response'ları karşılaştırın

Status kodu yetmez. İki proxy'nin header'larını karşılaştırın:

```bash
$ curl -sI -H 'Host: app.example.com' http://127.0.0.1:8080/ > new.txt
$ curl -sI https://app.example.com/ > old.txt
$ diff old.txt new.txt
```

Cache header'larına, `Set-Cookie` attribute'larına, CORS header'larına ve redirect'lere bakın. r3v3rs3 upstream request'ine `via: r3v3rs3` ile `forwarded` ve `x-forwarded-*` header'larını ekler:

```json
{
  "forwarded": "for=203.0.113.7, host=app.example.com, proto=https",
  "x-forwarded-for": "203.0.113.7",
  "x-forwarded-proto": "https",
  "x-forwarded-host": "app.example.com",
  "via": "r3v3rs3"
}
```

`X-Real-IP` okuyan bir uygulama için bu header'ı yazan bir request header kuralı ekleyin, çünkü r3v3rs3 bu header'ı göndermez.

CDN arkasındaysanız karşılaştırmadan önce CDN'i [Client IP](@/configuration.tr.md#client-ip) ayarlarında tanımlayın. Aksi halde her log satırı CDN'in adresini taşır.

## Adım 6: Trafiği devredin

Şu sırayla yapın:

1. Başka bir makineye geçiyorsanız bir gün önce DNS kayıtlarının TTL değerini beş dakikaya düşürün.
2. Eski proxy'yi durdurun veya başka bir porta taşıyın. İki process aynı portu dinleyemez.
3. r3v3rs3 portlarını `8080` ve `8443` yerine `80` ve `443` yapın. Değişiklik restart olmadan uygulanır.
4. Sertifikaları kontrol edin. ACME'de HTTP-01 challenge'ı port 80'i, TLS-ALPN-01 challenge'ı port 443'ü ister, bu yüzden devirden önceki bir order başarısız olabilir.
5. Gerçek adresi test edin:

```bash
$ curl -sI https://app.example.com/
```

6. Birkaç dakika proxy listesini izleyin. Sağlıklı sunucu sayısı ve discovery provider'larının issue'ları orada görünür.

Geri dönmek için portları eski haline getirin ve eski proxy'yi başlatın. Emin olana kadar eski config dosyasını saklayın, çünkü geri dönüş yolu tam olarak budur.

## Adım 7: Eski kurulumu kaldırın

Birkaç gün sorunsuz geçtikten sonra:

- Eski proxy'yi ve config'ini silin.
- r3v3rs3 kullanmıyorsa eski sertifika dosyalarını silin.
- Docker service discovery'ye geçtiyseniz container'larınızdan `traefik.` label'larını kaldırın.
- r3v3rs3'ten `8080` ve `8443` portlarını silin.

## Karşılığı olmayanlar

| Özellik | Durum |
|---|---|
| Lua, njs ve diğer nginx modülleri | Yok. |
| Tam path eşleşmesi | Yok. Path, segment bazında prefix olarak eşleşir. |
| Response body değiştirme (`sub_filter`) | Yok. |
| Disk cache | Yok. Cache memory'dedir. |
| Upstream'e HTTP/3 | Yok. Upstream bağlantısı HTTP/2 veya HTTP/1.1 kullanır. |
| WebTransport | Yok. |

## Referans

- [Routing](@/configuration.tr.md#routing) ve [Path rewrite](@/configuration.tr.md#path-rewrite).
- [Redirect kuralları](@/configuration.tr.md#redirect-kurallari) ve [Sabit response'lar](@/configuration.tr.md#sabit-response-lar).
- [Client IP](@/configuration.tr.md#client-ip): header'lar ve güvenilen proxy'ler.

## Sonraki adımlar

- [Load balancing ve health check](@/tutorials/load-balancing.tr.md): geçişin upstream tarafı.
- [Wildcard sertifika ile HTTPS](@/tutorials/https-certificates.tr.md): sertifikalar.
- [Docker'dan proxy'ler](@/tutorials/docker-discovery.tr.md): config dosyası yerine label.
