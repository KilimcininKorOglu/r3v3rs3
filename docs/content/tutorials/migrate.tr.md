+++
title = "nginx veya Traefik'ten geçiş"
description = "Yapılandırmanızı çevirin, iki proxy'yi yan yana çalıştırın ve geçişi yapın"
weight = 16
+++

# nginx veya Traefik'ten geçiş

Bu rehber, bir nginx veya Traefik yapılandırmasında bulunan parçaları çevirir ve geçişi kesintisiz yapar.

Plan şudur: r3v3rs3'ü başka bir portta çalıştırın, yapılandırmayı orada kurun, `Host` header'ı ile test edin, sonra portu taşıyın.

## Adım 1: Elinizdekini listeleyin

Mevcut yapılandırmanızdaki her maddeyi yazın. Çoğu kurulumda beş tür madde vardır:

1. Server block'ları veya router'lar: hangi host nereye gidiyor.
2. Location'lar veya kurallar: hangi yol nereye gidiyor ve yol nasıl değişiyor.
3. Sertifikalar: dosya veya ACME.
4. Ek özellikler: yönlendirme, header, basic auth, rate limit, IP filtresi, cache.
5. Upstream'ler: sunucular, ağırlıkları ve sağlık kontrolleri.

Aşağıdaki tablolardaki her maddenin r3v3rs3 karşılığı vardır. Tabloda olmayan bir şeyin, örneğin bir Lua script'inin veya bir nginx modülünün, karşılığı yoktur; o parça için başka bir çözüm gerekir.

## Adım 2: nginx direktiflerini çevirin

| nginx | r3v3rs3 |
|---|---|
| `listen 443 ssl;` | **HTTPS** protokollü bir port ve onun **Sunucu Adları** alanı |
| `listen 443 quic;` | **QUIC üzerinden HTTP (HTTP/3)** protokollü bir port |
| `server_name app.example.com;` | Proxy'nin **Virtual Host'lar** alanı |
| `location /api { }` | Yolu `/api` olan bir route |
| `proxy_pass http://api:8080/v1/;` | Route'ta `http://api:8080/v1/` sunucusu |
| `upstream app { server a; server b; }` | Bir route'ta iki sunucu |
| `server a weight=3;` | **Hedef** alanında `http://a/ 3`: ağırlık URL'den sonra gelir |
| `return 301 https://$host$request_uri;` | **HTTP'yi Otomatik Olarak HTTPS'e Yönlendir** |
| `rewrite ^/items/([0-9]+)$ /item/$1 break;` | Route'ta **Yol Regex'i** ve **Yerine Yazılacak Değer** |
| `add_header X-Frame-Options DENY;` | Yanıt header'ı kuralı: `set X-Frame-Options: DENY` |
| `auth_basic` ve `auth_basic_user_file` | Proxy'de kullanıcılarıyla **Basic Auth** |
| `auth_request /auth;` | **Doğrulama URL'i** ile **Forward Auth** |
| `allow` ve `deny` | **İzin Verilen IP Adresleri** ve **Engellenen IP Adresleri** |
| `limit_req_zone` ve `limit_req` | İstek sayısı, süre ve burst ile **Rate Limit** |
| `proxy_cache_path` ve `proxy_cache` | Bellek limiti ve varsayılan TTL ile **Cache** |
| `gzip on;` | Algoritmalar ve minimum boyut ile **Sıkıştırma** |
| `return 404;` | **Sabit durum kodu** route türü |
| `proxy_set_header X-Real-IP $remote_addr;` | Yalnız güvenilen bir proxy veya CDN için yazılır. Diğer durumda `set X-Real-IP: {client_ip}` istek header'ı kuralı. [İstemci IP adresi](@/configuration.tr.md#istemci-ip-adresi) bölümüne bakın |
| `client_max_body_size 10m;` | **İstek Gövdesi Limiti** |
| `proxy_read_timeout 60s;` | Proxy'nin veya route'un **İstek Timeout'u (Saniye)** alanı |
| `stream { server { proxy_pass db:5432; } }` | TCP proxy |

Zaman kaybettiren iki fark:

- **Location'ların sırası sonucu değiştirmez.** nginx, kendi öncelik kurallarına ve dosyadaki sıraya göre seçer. r3v3rs3 isteği her zaman, portun bütün proxy'leri içinde yolu en uzun eşleşen route'a gönderir. `/api`, `/api` ve `/api/users` ile eşleşir, `/apiv2` ile eşleşmez.
- **Sondaki eğik çizgi kuralı aynı değildir.** nginx'te `proxy_pass http://api:8080/` location önekini kaldırır, `proxy_pass http://api:8080` ise korur. r3v3rs3'te route yolu varsayılan olarak kaldırılır (**Route Yolunu Kaldır**) ve kalan kısım sunucu URL'inin yoluna eklenir:

```
route /api, sunucu http://api:8080/v1/    ->  GET /api/users  =  /v1/users
route /keep, Route Yolunu Kaldır kapalı    ->  GET /keep/x    =  /keep/x
```

Sorgu iki durumda da korunur.

## Adım 3: Traefik etiketlerini çevirin

| Traefik | r3v3rs3 |
|---|---|
| `entrypoints` | Portlar |
| `Host(\`app.example.com\`)` | Proxy'nin **Virtual Host'lar** alanı |
| `PathPrefix(\`/api\`)` | Yolu `/api` olan bir route |
| `PathPrefix` olmayan router | Yolu `/` olan bir route |
| `loadBalancer.servers` içeren `service` | Route'un sunucuları |
| `loadBalancer.healthCheck` | **Sağlık Kontrolü Yolu** ve **Kontrol Aralığı (Saniye)** ile sağlık kontrolü |
| `loadBalancer.sticky.cookie` | Cookie adıyla **Sticky Cookie'yi Aç** |
| `middlewares.stripPrefix` | Route'un varsayılan davranışı |
| `middlewares.addPrefix` | **Eklenecek Önek** |
| `middlewares.redirectScheme` | **HTTP'yi Otomatik Olarak HTTPS'e Yönlendir** |
| `middlewares.redirectRegex` | `regex`, `target` ve `status` içeren yönlendirme kuralı |
| `middlewares.basicAuth` | **Basic Auth** |
| `middlewares.forwardAuth` | **Forward Auth**. [Forward auth ile single sign-on](@/tutorials/forward-auth.tr.md) rehberine bakın |
| `middlewares.ipAllowList` | **IP Filtresi** |
| `middlewares.rateLimit` | **Rate Limit** |
| `middlewares.headers` | Header kuralları |
| `middlewares.compress` | **Sıkıştırma** |
| ACME ile `certresolver` | ACME kaydı. [Wildcard sertifika ile HTTPS](@/tutorials/https-certificates.tr.md) rehberine bakın |
| Docker etiketleri | Docker servis keşfi. [Docker'dan proxy'ler](@/tutorials/docker-discovery.tr.md) rehberine bakın |
| `tcp` router | TCP proxy |

Docker etiketleriyle kurulmuş bir Traefik kurulumu en az işi ister: Docker servis keşfini açın ve `traefik.` etiketlerinin yanına `r3v3rs3.` etiketlerini yazın. İki proxy de aynı container'ları okur, hiçbir şeyi silmeden ikisini karşılaştırabilirsiniz.

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

DNS'te hiçbir şey değişmez, kullanıcılarınız eski proxy'de kalır. Adım 1'deki listedeki her madde eskisi gibi yanıt verene kadar çalışın.

HTTPS için `0.0.0.0:8443` portunu ekleyin ve `--resolve` kullanın:

```bash
$ curl -sI --resolve app.example.com:8443:127.0.0.1 https://app.example.com:8443/
```

## Adım 5: Yanıtları karşılaştırın

Durum kodu yetmez. İki proxy'nin header'larını karşılaştırın:

```bash
$ curl -sI -H 'Host: app.example.com' http://127.0.0.1:8080/ > new.txt
$ curl -sI https://app.example.com/ > old.txt
$ diff old.txt new.txt
```

Cache header'larına, `Set-Cookie` özniteliklerine, CORS header'larına ve yönlendirmelere bakın. r3v3rs3 upstream isteğine `via: r3v3rs3` ile `forwarded` ve `x-forwarded-*` header'larını ekler:

```json
{
  "forwarded": "for=203.0.113.7, host=app.example.com, proto=https",
  "x-forwarded-for": "203.0.113.7",
  "x-forwarded-proto": "https",
  "x-forwarded-host": "app.example.com",
  "via": "r3v3rs3"
}
```

r3v3rs3 `X-Real-IP` header'ını yalnız karşı uç güvenilen bir proxy veya CDN olduğunda yazar. Doğrudan gelen istemcilerde `X-Real-IP` okuyan bir uygulama için `set X-Real-IP: {client_ip}` istek header'ı kuralını ekleyin.

CDN arkasındaysanız karşılaştırmadan önce CDN'i [İstemci IP adresi](@/configuration.tr.md#istemci-ip-adresi) ayarlarında tanımlayın. Aksi halde her log satırı CDN'in adresini taşır.

## Adım 6: Geçişi yapın

Şu sırayla yapın:

1. Başka bir makineye geçiyorsanız bir gün önce DNS kayıtlarının TTL değerini beş dakikaya düşürün.
2. Eski proxy'yi durdurun veya başka bir porta taşıyın. İki süreç aynı portu dinleyemez.
3. r3v3rs3 portlarını `8080` ve `8443` yerine `80` ve `443` yapın. Değişiklik yeniden başlatma olmadan uygulanır.
4. Sertifikaları kontrol edin. ACME'de HTTP-01 challenge'ı port 80'i, TLS-ALPN-01 challenge'ı port 443'ü ister, bu yüzden geçişten önce verilen bir sertifika siparişi başarısız olabilir.
5. Gerçek adresi test edin:

```bash
$ curl -sI https://app.example.com/
```

6. Birkaç dakika proxy listesini izleyin. Sağlıklı sunucu sayısı ve servis keşfi sağlayıcılarının sorunları orada görünür.

Geri dönmek için portları eski haline getirin ve eski proxy'yi başlatın. Emin olana kadar eski yapılandırma dosyasını saklayın, çünkü geri dönüş yolu tam olarak budur.

## Adım 7: Eski kurulumu kaldırın

Birkaç gün sorunsuz geçtikten sonra:

- Eski proxy'yi ve yapılandırmasını silin.
- r3v3rs3 kullanmıyorsa eski sertifika dosyalarını silin.
- Docker servis keşfine geçtiyseniz container'larınızdan `traefik.` etiketlerini kaldırın.
- r3v3rs3'te sakladığınız test portlarını silin.

## Karşılığı olmayanlar

| Özellik | Durum |
|---|---|
| Lua, njs ve diğer nginx modülleri | Yok. |
| Tam yol eşleşmesi | Yok. Yol, bütün segmentleriyle önek olarak eşleşir. |
| Yanıt gövdesini değiştirme (`sub_filter`) | Yok. |
| Disk cache | Yok. Cache bellektedir. |
| Upstream sunucuya HTTP/3 | Yok. Upstream bağlantısı HTTP/2 veya HTTP/1.1 kullanır. |
| WebTransport | Yok. |

## Referans

- [Route seçimi](@/configuration.tr.md#route-secimi) ve [Yol yeniden yazma](@/configuration.tr.md#yol-yeniden-yazma).
- [Yönlendirme kuralları](@/configuration.tr.md#yonlendirme-kurallari) ve [Sabit yanıtlar](@/configuration.tr.md#sabit-yanitlar).
- [İstemci IP adresi](@/configuration.tr.md#istemci-ip-adresi): header'lar ve güvenilen proxy'ler.

## Sonraki adımlar

- [Yük dengeleme ve sağlık kontrolü](@/tutorials/load-balancing.tr.md): geçişin upstream tarafı.
- [Wildcard sertifika ile HTTPS](@/tutorials/https-certificates.tr.md): sertifikalar.
- [Docker'dan proxy'ler](@/tutorials/docker-discovery.tr.md): yapılandırma dosyası yerine etiketler.
