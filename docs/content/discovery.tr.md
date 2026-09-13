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

# Docker

Docker provider, çalışan container'ların label'larını Docker Engine API ile okur. Container event'lerini izler. Başlayan, duran veya değişen bir container proxy'leri yaklaşık bir saniye içinde günceller.

## Ayarlar

"Ayarlar" sayfasında "Docker Servis Keşfi" bölümünü doldurun veya `config.toml` dosyasını düzenleyin:

```toml
[discovery.docker]
enabled = true
endpoint = "unix:///var/run/docker.sock"
network = "proxy"
exposed_by_default = false
```

| Ayar | Anlamı |
|---|---|
| `enabled` | Provider'ı başlatır. |
| `endpoint` | `unix://<path>`, `tcp://<host>:<port>`, `http://<host>:<port>` veya `https://<host>:<port>`. Varsayılan değer `unix:///var/run/docker.sock` olur. Windows `unix://` desteklemez. |
| `client_cert` | r3v3rs3'ün `https` endpoint'ine gönderdiği client sertifikasının id'si. Server sertifikasını bir sistem root sertifikası veya r3v3rs3'teki bir root sertifika imzalamalıdır. |
| `network` | Upstream adreslerinin alındığı Docker network'ü. Her container tek network'e bağlıysa boş bırakın. |
| `exposed_by_default` | `true`, `r3v3rs3.` label'ı olan her container'ı okur. `false` yalnız `r3v3rs3.enable=true` olan container'ları okur. |

Ayar değişikliği provider'ı server restart olmadan yeniden başlatır. Provider'ın kullandığı sertifika silinemez.

## Container'lar

- Container'ın upstream adresi, seçilen network'teki IP adresidir. `host` network modundaki container `127.0.0.1` kullanır.
- Health check'i başarısız olan container atlanır ve issue olarak gösterilir. Health check'i henüz geçmemiş container atlanır.
- Bir Compose servisinin replica'ları proxy'lerini paylaşır. Her replica kendi server'larını proxy'nin route'larına ekler, böylece proxy yükü replica'lara dağıtır. Replica'lar aynı sayıda route tanımlamalıdır.
- Proxy'nin key'i Compose projesi, Compose servisi ve proxy adından oluşur. Compose dışındaki container kendi adını kullanır.
- r3v3rs3, event gelmezse container listesini beş dakikada bir yeniden okur.

## Compose örneği

```yaml
services:
  r3v3rs3:
    image: ghcr.io/kilimcininkoroglu/r3v3rs3:latest
    volumes:
      - r3v3rs3-config:/root/.config/r3v3rs3
      - /var/run/docker.sock:/var/run/docker.sock:ro
    networks: [proxy]
    ports:
      - 80:80
      - 127.0.0.1:46492:46492

  whoami:
    image: traefik/whoami
    networks: [proxy]
    labels:
      r3v3rs3.enable: "true"
      r3v3rs3.http.whoami.ports: http
      r3v3rs3.http.whoami.vhosts: whoami.example.com
      r3v3rs3.http.whoami.port: "80"

networks:
  proxy:
    name: proxy

volumes:
  r3v3rs3-config:
```

"Portlar" sayfasında `http` adında bir port ekleyin, provider'ı `proxy` network'ü ile açın ve stack'i başlatın. Network `name` alanını vermezse Compose network adının başına proje adını ekler.

> Docker socket erişimi Docker host'unun tam kontrolünü verir. Bu yetki root erişimine eşittir. `:ro` seçeneği yalnız socket dosyasına uygulanır ve API'yi salt okunur yapmaz. Yönetim paneline veya host'a güvenilmeyen network'lerden erişilebiliyorsa r3v3rs3'ü yalnız `GET /containers/json` ve `GET /events` isteklerine izin veren bir Docker socket proxy'sine bağlayın.

# Consul

Consul provider, catalog'daki servislerin `r3v3rs3.*` tag'lerini ve key-value store'da bir prefix'in altındaki key'leri okur. Değişiklikleri blocking query'lerle izler. Kaydedilen, silinen veya health check'i başarısız olan bir servis instance'ı ve değişen bir key proxy'leri yaklaşık bir saniye içinde günceller.

## Ayarlar

"Ayarlar" sayfasında "Consul Servis Keşfi" bölümünü doldurun veya `config.toml` dosyasını düzenleyin:

```toml
[discovery.consul]
enabled = true
address = "http://127.0.0.1:8500"
token = "<ACL token>"
catalog = true
kv = true
prefix = "r3v3rs3"
exposed_by_default = false
```

| Ayar | Anlamı |
|---|---|
| `enabled` | Provider'ı başlatır. |
| `address` | Bir Consul agent'ının HTTP API'si: `http://<host>:<port>`, `https://<host>:<port>` veya `unix://<path>`. Varsayılan değer `http://127.0.0.1:8500` olur. |
| `client_cert` | r3v3rs3'ün `https` adresine gönderdiği client sertifikasının id'si. |
| `token` | ACL token'ı. Catalog için `service:read` ve `node:read`, key-value store için prefix üzerinde `key:read` izni gerekir. |
| `datacenter` | Okunacak datacenter. Agent'ın kendi datacenter'ını okumak için boş bırakın. |
| `catalog` | Catalog'daki servislerin tag'lerini okur. Varsayılan değer `true` olur. |
| `kv` | `prefix` altındaki key'leri okur. Varsayılan değer `true` olur. |
| `prefix` | Key prefix'i. Varsayılan değer `r3v3rs3` olur. |
| `exposed_by_default` | `true`, `r3v3rs3.` tag'i olan her servisi okur. `false` yalnız `r3v3rs3.enable=true` tag'i olan servisleri okur. |

Admin API token'ı döndürmez. Kayıtlı bir token varsa `GET /api/config` yanıtında `token_set: true` döner. `token` alanı olmayan bir `PUT /api/config` isteği kayıtlı token'ı korur. `"token": ""` token'ı siler. `config.toml` token'ı düz metin olarak saklar. Bu yüzden config dizinini yalnız r3v3rs3 kullanıcısı okuyabilmelidir.

## Catalog servisleri

- Tag `<key>=<value>` biçimindedir, örneğin `r3v3rs3.http.app.ports=https`. `=` içermeyen bir `r3v3rs3.` tag'i issue olur.
- r3v3rs3 yalnız health check'lerini geçen instance'ları okur. Health check'i geçen instance'ı olmayan seçili bir servis issue olur ve proxy'leri kaldırılır.
- Instance'ın upstream adresi servis adresidir. Servis adresi olmayan instance node adresini kullanır.
- `port`, `routes` ve `upstream_servers` alanları olmayan bir proxy servisin portunu kullanır. `port` ve `servers` alanları olmayan bir route da servisin portunu kullanır.
- Bir servisin instance'ları proxy'lerini paylaşır. Her instance kendi server'larını proxy'nin route'larına ekler, böylece proxy yükü instance'lara dağıtır.

Servisi Consul agent'ına kaydedin, örneğin `consul services register whoami.json` komutuyla:

```json
{
  "Service": {
    "Name": "whoami",
    "Port": 8080,
    "Tags": [
      "r3v3rs3.enable=true",
      "r3v3rs3.http.whoami.ports=http",
      "r3v3rs3.http.whoami.vhosts=whoami.example.com"
    ],
    "Check": { "HTTP": "http://localhost:8080/", "Interval": "10s" }
  }
}
```

## Key-value store

- Prefix'in altındaki her key bir label'dır. Prefix `r3v3rs3` ise `r3v3rs3/http/app/ports` key'i `r3v3rs3.http.app.ports` label'ı olur.
- r3v3rs3 prefix'in altındaki her proxy'yi okur. Bu yüzden key'ler `r3v3rs3.enable` gerektirmez.
- Key'in adresi yoktur, bu yüzden `port` kullanılamaz. `routes.<n>.servers.<n>.url` veya `upstream_servers.<n>.addr` kullanın.
- Key'in bir parçası `.` içeremez. Böyle bir key issue olur.

```bash
consul kv put r3v3rs3/http/app/ports https
consul kv put r3v3rs3/http/app/vhosts app.example.com
consul kv put r3v3rs3/http/app/routes/0/servers/0/url http://10.0.0.5:8080
```

# etcd

etcd provider, bir prefix'in altındaki key'leri etcd v3 HTTP API'si ile okur. Değişiklikleri bir watch stream'i ile izler. Değişen bir key proxy'leri yaklaşık bir saniye içinde günceller.

## Ayarlar

"Ayarlar" sayfasında "etcd Servis Keşfi" bölümünü doldurun veya `config.toml` dosyasını düzenleyin:

```toml
[discovery.etcd]
enabled = true
endpoints = ["http://10.0.0.1:2379", "http://10.0.0.2:2379"]
username = "r3v3rs3"
password = "<password>"
prefix = "r3v3rs3"
```

| Ayar | Anlamı |
|---|---|
| `enabled` | Provider'ı başlatır. |
| `endpoints` | Cluster üyelerinin HTTP API adresleri: `http://<host>:<port>`, `https://<host>:<port>` veya `unix://<path>`. Bağlantı başarısız olunca r3v3rs3 sıradaki adrese bağlanır. Varsayılan değer `http://127.0.0.1:2379` olur. |
| `client_cert` | r3v3rs3'ün `https` endpoint'ine gönderdiği client sertifikasının id'si. |
| `username` | etcd authentication kullanıcısı. Authentication kapalıysa boş bırakın. |
| `password` | Kullanıcının parolası. `username` ile birlikte verin. |
| `prefix` | Key prefix'i. Varsayılan değer `r3v3rs3` olur. |

Admin API parolayı döndürmez. Kayıtlı bir parola varsa `GET /api/config` yanıtında `password_set: true` döner. `password` alanı olmayan bir `PUT /api/config` isteği kayıtlı parolayı korur. `"password": ""` parolayı siler. `config.toml` parolayı düz metin olarak saklar. Bu yüzden config dizinini yalnız r3v3rs3 kullanıcısı okuyabilmelidir.

Authentication açıksa r3v3rs3 kullanıcı adı ve parola ile bir token alır. etcd süresi dolmuş bir token'ı reddedince r3v3rs3 yeni bir token alır ve isteği yeniden gönderir. Kullanıcının prefix üzerinde okuma izni olan bir rolü olmalıdır:

```bash
etcdctl role add r3v3rs3-reader
etcdctl role grant-permission r3v3rs3-reader --prefix=true read r3v3rs3/
etcdctl user add r3v3rs3
etcdctl user grant-role r3v3rs3 r3v3rs3-reader
```

## Key'ler

- Key'ler Consul key-value store'unun düzenini kullanır. Prefix `r3v3rs3` ise `r3v3rs3/http/app/ports` key'i `r3v3rs3.http.app.ports` label'ı olur.
- r3v3rs3 prefix'in altındaki her proxy'yi okur. Bu yüzden key'ler `r3v3rs3.enable` gerektirmez.
- Key'in adresi yoktur, bu yüzden `port` kullanılamaz. `routes.<n>.servers.<n>.url` veya `upstream_servers.<n>.addr` kullanın.
- Key'in bir parçası `.` içeremez. Böyle bir key issue olur.
- etcd, watch stream'inin ihtiyaç duyduğu revision'ı compact ederse r3v3rs3 bütün key'leri yeniden okur.

```bash
etcdctl put r3v3rs3/http/app/ports https
etcdctl put r3v3rs3/http/app/vhosts app.example.com
etcdctl put r3v3rs3/http/app/routes/0/servers/0/url http://10.0.0.5:8080
```
