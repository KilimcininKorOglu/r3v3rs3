+++
title = "Servis keşfi"
description = "Servis keşfi"
weight = 0
+++

# Servis keşfi

Bir servis keşfi sağlayıcısı, dış sistemdeki proxy tanımlarını okur ve proxy listesine ekler. Bir tanım değişince r3v3rs3 route'ları yeniden başlatmadan günceller.

Bu sayfa referanstır. Docker etiketleriyle çalışan bir kurulum için [Docker'dan proxy'ler](@/tutorials/docker-discovery.tr.md) rehberini izleyin.

## Keşfedilen proxy'ler

- Keşfedilen proxy salt okunurdur. Yönetim API'si, güncelleme ve silme isteğine `proxy_read_only` döner. WebUI proxy'yi kaynağıyla gösterir ve düzenleme işlemlerini göstermez. Proxy'yi değiştirmek için kaynaktaki tanımı değiştirin.
- r3v3rs3 keşfedilen proxy'leri `proxies.toml` dosyasına yazmaz. r3v3rs3 yeniden başlayınca sağlayıcı onları tekrar gönderir.
- Keşfedilen proxy'nin id'si sağlayıcıdan ve tanımın key'inden gelir. Proxy yeniden başlatmadan sonra aynı id'yi alır, bu yüzden log ve durum kayıtları aynı id'yi kullanır.
- Sağlayıcı port açmaz. Tanım, `ports` alanında mevcut portları port adı veya port id'si ile verir. Port adı tam olarak bir portu seçmelidir. Port, proxy'nin protokolünü kabul etmelidir. Bir portun adını değiştirdiğinizde veya portu sildiğinizde r3v3rs3 port adlarını yeniden çözer.
- Keşfedilen TCP proxy, başka bir TCP proxy'nin kullandığı TCP portunu kullanamaz.
- Sağlayıcı bağlantısını kaybedince son okumasındaki proxy'ler aktif kalır.

## Sağlayıcı durumu

Proxy listesi her sağlayıcının durumunu, eklediği proxy sayısını ve proxy olamayan tanımları gösterir. Yönetim API'si aynı veriyi `GET /api/discovery` adresinde döndürür.

| Durum | Anlamı |
|---|---|
| `connecting` | Sağlayıcı kaynaklarını henüz okumadı. |
| `running` | Sağlayıcı kaynaklarını okudu ve değişiklikleri izliyor. |
| `error` | Sağlayıcı kaynaklarını okuyamıyor. Son okumasındaki proxy'ler aktif kalır. |

[Deploy platformu](@/platform.tr.md) çalışırken liste, deploy edilen uygulamaların proxy'lerini yayınlayan `platform` sağlayıcısını da içerir.

Her sorun, kaynağı ve sebebi gösterir, örneğin `web-1: http.app: port not found: https`.

# Etiketler

Docker etiketleri, Consul etiketleri ve key-value kayıtları aynı key'leri kullanır. Her key şu biçimdedir:

```
r3v3rs3.<protocol>.<name>.<field>=<value>
```

- `<protocol>` değeri `http`, `tcp` veya `udp` olur.
- `<name>` kaynaktaki proxy'yi belirtir. Bir kaynak birden fazla proxy tanımlayabilir.
- `<field>` yönetim API'sinin proxy modelindeki bir alandır. Nokta iç içe alanları ayırır.
- r3v3rs3, `r3v3rs3.` öneki olmayan etiketleri atlar. Bilinmeyen bir `r3v3rs3.` etiketi veya bilinmeyen bir alan sorun sayılır ve proxy eklenmez.

## Proxy key'leri

| Key | Değer |
|---|---|
| `r3v3rs3.enable` | Sağlayıcı her kaynağı okumuyorsa `true` kaynağı seçer. Başka her değer, örneğin `false`, kaynağı `exposed_by_default = true` iken de atlar. |
| `ports` | Zorunlu. Virgülle ayrılmış port adları veya port id'leri. |
| `name` | Proxy listesindeki ad. Varsayılan değer `<name>` olur. |
| `active` | `false` proxy'yi pasif olarak ekler. Varsayılan değer `true` olur. |
| `port` | Kaynağın adresindeki upstream sunucunun portu. |
| `scheme` | HTTP proxy'nin `port` alanı için `http` veya `https`. Varsayılan değer `http` olur. |
| `acme` | HTTP proxy'nin `vhosts` alanı için sertifika alan ACME kaydının id'si. [ACME sertifikaları](@/discovery.tr.md#acme-sertifikalari) bölümüne bakın. |

## Değerler

- Boolean değer `true` veya `false` olur. Sayı ondalık yazılır.
- Süre birimle yazılır, örneğin `500ms`, `30s` veya `5m`.
- Liste virgülle ayrılmış bir değerdir: `vhosts=app.example.com,www.example.com`.
- Grup listesi `0`, `1`, `2` gibi key'ler kullanır: `routes.0.path=/`, `routes.1.path=/api`. Sayılar sırayı belirler.
- Boş değer, değer yok demektir.
- `auth` ve header kurallarının alanları metin olduğu için orada virgüllü liste kullanılamaz. Orada numaralı key kullanın: `auth.users.0.username=alice`.

## HTTP proxy'ler

`port` ve `scheme`, `/` yoluna tek sunuculu bir route tanımlar. `routes` ile birlikte kullanılamaz.

```
r3v3rs3.http.app.ports=https
r3v3rs3.http.app.vhosts=app.example.com
r3v3rs3.http.app.port=8080
```

Bir route da `port` ve `scheme` kullanabilir. Aynı route'un `servers` veya `response` alanıyla birlikte kullanılamaz.

```
r3v3rs3.http.app.ports=http,https
r3v3rs3.http.app.vhosts=app.example.com
r3v3rs3.http.app.routes.0.path=/
r3v3rs3.http.app.routes.0.port=8080
r3v3rs3.http.app.routes.1.path=/api
r3v3rs3.http.app.routes.1.port=9443
r3v3rs3.http.app.routes.1.scheme=https
```

Sunucuya açık bir URL ve ağırlık verilebilir:

```
r3v3rs3.http.app.routes.0.servers.0.url=http://10.0.0.5:8080
r3v3rs3.http.app.routes.0.servers.1.url=http://10.0.0.6:8080
r3v3rs3.http.app.routes.0.servers.1.weight=3
r3v3rs3.http.app.load_balancing=round_robin
```

Diğer alanlar yönetim API'sinin adlarını kullanır:

```
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

r3v3rs3, proxy'yi kullanmadan önce düz metin password ve token değerlerini hash'e çevirir. Kaynak düz metin değeri tutmaya devam eder. Bu yüzden etiketlerde `password_hash` ve `token_hash` tercih edin.

## TCP ve UDP proxy'ler

`port`, kaynağın adresinde tek bir upstream sunucu tanımlar. `upstream_servers` ile birlikte kullanılamaz.

```
r3v3rs3.tcp.db.ports=postgres
r3v3rs3.tcp.db.port=5432

r3v3rs3.udp.dns.ports=dns
r3v3rs3.udp.dns.upstream_servers.0.addr=/ip4/10.0.0.53/udp/53
r3v3rs3.udp.dns.session_idle_timeout=30s
```

## ACME sertifikaları

`acme`, sertifika listesindeki mevcut bir ACME kaydını verir. r3v3rs3, kaydın hesabı, challenge'ı ve DNS sağlayıcısı ile proxy'nin virtual host'ları için sertifika alır.

```
r3v3rs3.http.app.ports=https
r3v3rs3.http.app.vhosts=app.example.com,www.example.com
r3v3rs3.http.app.port=8080
r3v3rs3.http.app.acme=abc-def
```

- Sertifikanın alan adları proxy'nin virtual host'larıdır. Sertifika, kaydın ayrı bir order'ıdır. Bu yüzden kaydın `renewal_days` süresinden sonra kendi zamanında yenilenir. Kaydın kendi sertifikalarının yenileme zamanı değişmez.
- Private key'i olan geçerli bir sunucu sertifikası bütün virtual host'ları kapsıyorsa r3v3rs3 sertifika almaz. Örneğin kaydın wildcard sertifikası bütün host'ları kapsayabilir. Proxy'nin ilk sertifikasından sonra yenileme, proxy sertifikasına göre yapılır.
- Aynı kaydı ve aynı virtual host'ları kullanan proxy'ler tek sertifikayı paylaşır.
- Bulunmayan veya pasif kayıt, virtual host'u olmayan proxy, regex virtual host'u ve kaydın challenge'ının doğrulayamadığı ad sorun sayılır. Örneğin `http-01` ile wildcard virtual host sorun sayılır. Proxy sertifikasız eklenir.
- Pasif proxy sertifika almaz. `acme` alanı olan TCP veya UDP proxy sorun sayılır.
- Başarısız bir order'dan sonra r3v3rs3 bir sonraki order için bir saat bekler.
- Etiket kaldırılınca sertifika yenilenmez. r3v3rs3 süresi dolan sertifikayı sertifika listesinden çıkarır.
- Ingress `r3v3rs3.io/acme` annotation'ını, R3v3rs3Proxy ise `spec.acme` alanını kullanır.

# Docker

Docker sağlayıcısı, çalışan container'ların etiketlerini Docker Engine API ile okur. Container olaylarını izler. Başlayan, duran veya değişen bir container proxy'leri yaklaşık bir saniye içinde günceller.

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
| `enabled` | Sağlayıcıyı başlatır. |
| `endpoint` | `unix://<path>`, `tcp://<host>:<port>`, `http://<host>:<port>` veya `https://<host>:<port>`. Varsayılan değer `unix:///var/run/docker.sock` olur. |
| `client_cert` | r3v3rs3'ün `https` endpoint'ine gönderdiği istemci sertifikasının id'si. Sunucu sertifikasını bir sistem kök sertifikası veya r3v3rs3'teki bir kök sertifika imzalamalıdır. |
| `network` | Upstream adreslerinin alındığı Docker ağı. Her container tek ağa bağlıysa boş bırakın. Bu ağda olmayan veya ayar boşken birden fazla ağa bağlı container sorun sayılır. |
| `exposed_by_default` | `true`, `r3v3rs3.` etiketi olan her container'ı okur. `false` yalnız `r3v3rs3.enable=true` olan container'ları okur. |

Ayar değişikliği yalnız sağlayıcıyı yeniden başlatır, sunucu yeniden başlamaz. Sağlayıcının proxy'leri ve sertifikaları hemen kaldırılır ve ilk okumasıyla geri gelir. Sağlayıcıyı kapatmak onları kaldırır. Sağlayıcının kullandığı sertifika silinemez.

## Container'lar

- Container'ın upstream adresi, seçilen ağdaki IP adresidir. `host` ağ modundaki container `127.0.0.1` kullanır.
- Sağlık kontrolü başarısız olan container atlanır ve sorun olarak gösterilir. Sağlık kontrolünü henüz geçmemiş container atlanır.
- Bir Compose servisinin replikaları proxy'lerini paylaşır. Her replika kendi sunucularını proxy'nin route'larına ekler, böylece proxy yükünü replikalar paylaşır. Replikalar aynı sayıda route tanımlamalıdır.
- Proxy'nin key'i Compose projesi, Compose servisi ve proxy adından oluşur. Compose dışındaki container kendi adını kullanır.
- r3v3rs3, olay gelmezse container listesini beş dakikada bir yeniden okur.

## Compose örneği

r3v3rs3 host'ta çalışır ve `/var/run/docker.sock` dosyasını okur. Compose dosyası etiketli container'ları taşır:

```yaml
services:
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
```

"Portlar" sayfasında `http` adında bir port ekleyin, sağlayıcıyı `proxy` ağı ile açın ve servisi başlatın. Ağ `name` alanını vermezse Compose ağ adının başına proje adını ekler.

> Docker socket erişimi Docker host'unun tam kontrolünü verir. Bu yetki root erişimine eşittir. `:ro` seçeneği yalnız socket dosyasına uygulanır ve API'yi salt okunur yapmaz. Yönetim paneline veya host'a güvenilmeyen ağlardan erişilebiliyorsa r3v3rs3'ü yalnız `GET /containers/json` ve `GET /events` isteklerine izin veren bir Docker socket proxy'sine bağlayın.

# Consul

Consul sağlayıcısı, catalog'daki servislerin `r3v3rs3.*` etiketlerini ve key-value store'da bir önekin altındaki key'leri okur. Değişiklikleri bekleyen sorgularla izler. Kaydedilen, silinen veya sağlık kontrolü başarısız olan bir servis örneği ve değişen bir key proxy'leri yaklaşık bir saniye içinde günceller.

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
| `enabled` | Sağlayıcıyı başlatır. |
| `address` | Bir Consul agent'ının HTTP API'si: `http://<host>:<port>`, `https://<host>:<port>` veya `unix://<path>`. Varsayılan değer `http://127.0.0.1:8500` olur. |
| `client_cert` | r3v3rs3'ün `https` adresine gönderdiği istemci sertifikasının id'si. |
| `token` | ACL token'ı. Catalog için `service:read` ve `node:read`, key-value store için önek üzerinde `key:read` izni gerekir. |
| `datacenter` | Okunacak veri merkezi. Agent'ın kendi veri merkezini okumak için boş bırakın. |
| `catalog` | Catalog'daki servislerin etiketlerini okur. Varsayılan değer `true` olur. |
| `kv` | `prefix` altındaki key'leri okur. Varsayılan değer `true` olur. |
| `prefix` | Key öneki. Varsayılan değer `r3v3rs3` olur. |
| `exposed_by_default` | `true`, `r3v3rs3.` etiketi olan her servisi okur. `false` yalnız `r3v3rs3.enable=true` etiketi olan servisleri okur. |

Sağlayıcı `catalog` veya `kv` ayarını gerektirir. `kv` açıkken `prefix` boş olamaz.

Yönetim API'si token'ı döndürmez. Kayıtlı bir token varsa `GET /api/config` yanıtında `token_set: true` döner. `token` alanı olmayan bir `PUT /api/config` isteği kayıtlı token'ı korur. `"token": ""` token'ı siler. `config.toml` token'ı düz metin olarak saklar. Bu yüzden yapılandırma dizinini yalnız r3v3rs3 kullanıcısı okuyabilmelidir.

## Catalog servisleri

- Etiket `<key>=<value>` biçimindedir, örneğin `r3v3rs3.http.app.ports=https`. `=` içermeyen bir `r3v3rs3.` etiketi sorun sayılır.
- r3v3rs3 yalnız sağlık kontrollerini geçen servis örneklerini okur. Sağlık kontrolünü geçen örneği olmayan seçili bir servis sorun sayılır ve onun proxy'leri kaldırılır.
- Servis örneğinin upstream adresi servis adresidir. Servis adresi olmayan örnek düğüm adresini kullanır.
- `port`, `routes` ve `upstream_servers` alanları olmayan bir proxy servisin portunu kullanır. `port`, `servers` ve `response` alanları olmayan bir route da servisin portunu kullanır.
- Bir servisin örnekleri proxy'lerini paylaşır. Her örnek kendi sunucularını proxy'nin route'larına ekler, böylece proxy yükünü örnekler paylaşır.
- r3v3rs3 her örneği kendi etiketleriyle seçer. `exposed_by_default = false` iken `r3v3rs3.enable=true` etiketi olmayan örnek sunucu eklemez.

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

- Önekin altındaki her key bir etikettir. Önek `r3v3rs3` ise `r3v3rs3/http/app/ports` key'i `r3v3rs3.http.app.ports` etiketi olur.
- r3v3rs3 önekin altındaki her proxy'yi okur. Bu yüzden key'ler `r3v3rs3.enable` gerektirmez.
- Key'in adresi yoktur, bu yüzden `port` kullanılamaz. `routes.<n>.servers.<n>.url` veya `upstream_servers.<n>.addr` kullanın.
- Key'in bir parçası `.` içeremez. Böyle bir key sorun sayılır.

```bash
consul kv put r3v3rs3/http/app/ports https
consul kv put r3v3rs3/http/app/vhosts app.example.com
consul kv put r3v3rs3/http/app/routes/0/servers/0/url http://10.0.0.5:8080
```

# etcd

etcd sağlayıcısı, bir önekin altındaki key'leri etcd v3 HTTP API'si ile okur. Değişiklikleri bir watch stream'i ile izler. Değişen bir key proxy'leri yaklaşık bir saniye içinde günceller.

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
| `enabled` | Sağlayıcıyı başlatır. |
| `endpoints` | Cluster üyelerinin HTTP API adresleri: `http://<host>:<port>`, `https://<host>:<port>` veya `unix://<path>`. Bağlantı başarısız olunca r3v3rs3 sıradaki adrese bağlanır. Varsayılan değer `http://127.0.0.1:2379` olur. |
| `client_cert` | r3v3rs3'ün `https` endpoint'ine gönderdiği istemci sertifikasının id'si. |
| `username` | etcd kimlik doğrulamasının kullanıcısı. Kimlik doğrulama kapalıysa boş bırakın. |
| `password` | Kullanıcının parolası. `username` ile birlikte verin. |
| `prefix` | Key öneki. Varsayılan değer `r3v3rs3` olur. |

`endpoints` en az bir adres içermelidir ve `prefix` boş olamaz.

Yönetim API'si parolayı döndürmez. Kayıtlı bir parola varsa `GET /api/config` yanıtında `password_set: true` döner. `password` alanı olmayan bir `PUT /api/config` isteği kayıtlı parolayı korur. `"password": ""` parolayı siler. `config.toml` parolayı düz metin olarak saklar. Bu yüzden yapılandırma dizinini yalnız r3v3rs3 kullanıcısı okuyabilmelidir.

Kimlik doğrulama açıksa r3v3rs3 kullanıcı adı ve parola ile bir token alır. etcd süresi dolmuş bir token'ı reddedince r3v3rs3 yeni bir token alır ve isteği yeniden gönderir. Kullanıcının önek üzerinde okuma izni olan bir rolü olmalıdır:

```bash
etcdctl role add r3v3rs3-reader
etcdctl role grant-permission r3v3rs3-reader --prefix=true read r3v3rs3/
etcdctl user add r3v3rs3
etcdctl user grant-role r3v3rs3 r3v3rs3-reader
```

## Key'ler

- Key'ler Consul key-value store'unun düzenini kullanır. Önek `r3v3rs3` ise `r3v3rs3/http/app/ports` key'i `r3v3rs3.http.app.ports` etiketi olur.
- r3v3rs3 önekin altındaki her proxy'yi okur. Bu yüzden key'ler `r3v3rs3.enable` gerektirmez.
- Key'in adresi yoktur, bu yüzden `port` kullanılamaz. `routes.<n>.servers.<n>.url` veya `upstream_servers.<n>.addr` kullanın.
- Key'in bir parçası `.` içeremez. Böyle bir key sorun sayılır.
- etcd, watch stream'inin ihtiyaç duyduğu revizyonu compaction ile silerse r3v3rs3 bütün key'leri yeniden okur.

```bash
etcdctl put r3v3rs3/http/app/ports https
etcdctl put r3v3rs3/http/app/vhosts app.example.com
etcdctl put r3v3rs3/http/app/routes/0/servers/0/url http://10.0.0.5:8080
```
