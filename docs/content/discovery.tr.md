+++
title = "Servis keşfi"
description = "Servis keşfi"
weight = 0
+++

# Servis keşfi

Bir discovery provider, dış sistemdeki proxy tanımlarını okur ve proxy listesine ekler. Bir tanım değişince r3v3rs3 route'ları restart olmadan günceller.

## Keşfedilen proxy'ler

- Keşfedilen proxy salt okunurdur. Admin API, güncelleme ve silme isteğine `proxy_read_only` döner. WebUI proxy'yi kaynağıyla gösterir ve düzenleme aksiyonlarını göstermez. Proxy'yi değiştirmek için kaynaktaki tanımı değiştirin.
- r3v3rs3 keşfedilen proxy'leri `proxies.toml` dosyasına yazmaz. Provider onları restart sonrası yeniden gönderir.
- Keşfedilen proxy'nin id'si provider'dan ve tanımın key'inden gelir. Proxy restart sonrası aynı id'yi alır, bu yüzden log ve durum kayıtları aynı id'yi kullanır.
- Provider port açmaz. Tanım, `ports` alanında mevcut portları port adı veya port id'si ile verir. Port adı tam olarak bir portu seçmelidir. Port, proxy'nin protokolünü kabul etmelidir. Bir portun adını değiştirdiğinizde veya portu sildiğinizde r3v3rs3 port adlarını yeniden çözer.
- Keşfedilen TCP proxy, başka bir TCP proxy'nin kullandığı TCP portunu kullanamaz.
- Provider bağlantısını kaybedince son okumasındaki proxy'ler aktif kalır.

## Provider durumu

Proxy listesi her provider'ın durumunu, eklediği proxy sayısını ve proxy olamayan tanımları gösterir. Admin API aynı veriyi `GET /api/discovery` adresinde döner.

| Durum | Anlamı |
|---|---|
| `connecting` | Provider kaynaklarını henüz okumadı. |
| `running` | Provider kaynaklarını okudu ve değişiklikleri izliyor. |
| `error` | Provider kaynaklarını okuyamıyor. Son okumasındaki proxy'ler aktif kalır. |

Bir issue kaynağı ve sebebi verir, örneğin `web-1: http.app: port not found: https`.

# Label'lar

Docker label'ları, Consul tag'leri ve key-value kayıtları aynı key'leri kullanır. Her key şu biçimdedir:

```text
r3v3rs3.<protocol>.<name>.<field>=<value>
```

- `<protocol>` değeri `http`, `tcp` veya `udp` olur.
- `<name>` kaynaktaki proxy'yi belirtir. Bir kaynak birden fazla proxy tanımlayabilir.
- `<field>` admin API proxy modelinin bir alanıdır. Nokta iç içe alanları ayırır.
- r3v3rs3, `r3v3rs3.` prefix'i olmayan label'ları atlar. Bilinmeyen bir `r3v3rs3.` label'ı veya bilinmeyen bir alan issue olur ve proxy eklenmez.

## Proxy key'leri

| Key | Değer |
|---|---|
| `r3v3rs3.enable` | Provider her kaynağı okumuyorsa `true` kaynağı seçer. |
| `ports` | Zorunlu. Virgülle ayrılmış port adları veya port id'leri. |
| `name` | Proxy listesindeki ad. Varsayılan değer `<name>` olur. |
| `active` | `false` proxy'yi pasif olarak ekler. Varsayılan değer `true` olur. |
| `port` | Kaynağın adresindeki upstream server'ın portu. |
| `scheme` | HTTP proxy'nin `port` alanı için `http` veya `https`. Varsayılan değer `http` olur. |

## Değerler

- Boolean değer `true` veya `false` olur. Sayı ondalık yazılır.
- Süre birimle yazılır, örneğin `500ms`, `30s` veya `5m`.
- Liste virgülle ayrılmış bir değerdir: `vhosts=app.example.com,www.example.com`.
- Grup listesi `0`, `1`, `2` gibi key'ler kullanır: `routes.0.path=/`, `routes.1.path=/api`. Sayılar sırayı belirler.
- Boş değer, değer yok demektir.
- `auth` ve header kurallarının alanları metin olduğu için orada virgüllü liste kullanılamaz. Orada numaralı key kullanın: `auth.users.0.username=alice`.

## HTTP proxy'ler

`port` ve `scheme`, `/` path'ine tek server'lı bir route tanımlar. `routes` ile birlikte kullanılamaz.

```text
r3v3rs3.http.app.ports=https
r3v3rs3.http.app.vhosts=app.example.com
r3v3rs3.http.app.port=8080
```

Bir route da `port` ve `scheme` kullanabilir. Aynı route'un `servers` alanıyla birlikte kullanılamaz.

```text
r3v3rs3.http.app.ports=http,https
r3v3rs3.http.app.vhosts=app.example.com
r3v3rs3.http.app.routes.0.path=/
r3v3rs3.http.app.routes.0.port=8080
r3v3rs3.http.app.routes.1.path=/api
r3v3rs3.http.app.routes.1.port=9443
r3v3rs3.http.app.routes.1.scheme=https
```

Server'a açık bir URL verilebilir:

```text
r3v3rs3.http.app.routes.0.servers.0.url=http://10.0.0.5:8080
r3v3rs3.http.app.routes.0.servers.1.url=http://10.0.0.6:8080
r3v3rs3.http.app.load_balancing=round_robin
```

Diğer alanlar admin API adlarını kullanır:

```text
r3v3rs3.http.app.upgrade_insecure=true
r3v3rs3.http.app.ip_filter.allow=10.0.0.0/8,192.168.0.0/16
r3v3rs3.http.app.rate_limit.requests=100
r3v3rs3.http.app.rate_limit.per=minute
r3v3rs3.http.app.rate_limit.burst=20
r3v3rs3.http.app.timeouts.connect=3s
r3v3rs3.http.app.timeouts.request=30s
r3v3rs3.http.app.health_check.interval=10s
r3v3rs3.http.app.health_check.path=/healthz
r3v3rs3.http.app.compression.algorithms=br,gzip
r3v3rs3.http.app.cache.enabled=true
r3v3rs3.http.app.cache.default_ttl=1m
r3v3rs3.http.app.headers.response.0.action=set
r3v3rs3.http.app.headers.response.0.name=X-Frame-Options
r3v3rs3.http.app.headers.response.0.value=DENY
r3v3rs3.http.app.auth.type=basic
r3v3rs3.http.app.auth.realm=Staff
r3v3rs3.http.app.auth.users.0.username=alice
r3v3rs3.http.app.auth.users.0.password=change-me
```

r3v3rs3, proxy'yi kullanmadan önce düz metin password ve token değerlerini hash'e çevirir. Kaynak düz metin değeri tutmaya devam eder. Bu yüzden label'larda `password_hash` ve `token_hash` tercih edin.

## TCP ve UDP proxy'ler

`port`, kaynağın adresinde tek bir upstream server tanımlar. `upstream_servers` ile birlikte kullanılamaz.

```text
r3v3rs3.tcp.db.ports=postgres
r3v3rs3.tcp.db.port=5432

r3v3rs3.udp.dns.ports=dns
r3v3rs3.udp.dns.upstream_servers.0.addr=/ip4/10.0.0.53/udp/53
r3v3rs3.udp.dns.session_idle_timeout=30s
```
