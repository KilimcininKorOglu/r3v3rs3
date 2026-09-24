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

Proxy listesi her sağlayıcının durumunu, eklediği proxy sayısını ve proxy olamayan tanımları gösterir. Yönetim API'si aynı veriyi `GET /api/discovery` adresinde döner.

| Durum | Anlamı |
|---|---|
| `connecting` | Sağlayıcı kaynaklarını henüz okumadı. |
| `running` | Sağlayıcı kaynaklarını okudu ve değişiklikleri izliyor. |
| `error` | Sağlayıcı kaynaklarını okuyamıyor. Son okumasındaki proxy'ler aktif kalır. |

Her sorun, kaynağı ve sebebi gösterir, örneğin `web-1: http.app: port not found: https`.

# Etiketler

Docker etiketleri, Consul etiketleri ve key-value kayıtları aynı key'leri kullanır. Her key şu biçimdedir:

```text
r3v3rs3.<protocol>.<name>.<field>=<value>
```

- `<protocol>` değeri `http`, `tcp` veya `udp` olur.
- `<name>` kaynaktaki proxy'yi belirtir. Bir kaynak birden fazla proxy tanımlayabilir.
- `<field>` yönetim API'sinin proxy modelindeki bir alandır. Nokta iç içe alanları ayırır.
- r3v3rs3, `r3v3rs3.` öneki olmayan etiketleri atlar. Bilinmeyen bir `r3v3rs3.` etiketi veya bilinmeyen bir alan sorun sayılır ve proxy eklenmez.

## Proxy key'leri

| Key | Değer |
|---|---|
| `r3v3rs3.enable` | Sağlayıcı her kaynağı okumuyorsa `true` kaynağı seçer. |
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

Sunucuya açık bir URL ve ağırlık verilebilir:

```text
r3v3rs3.http.app.routes.0.servers.0.url=http://10.0.0.5:8080
r3v3rs3.http.app.routes.0.servers.1.url=http://10.0.0.6:8080
r3v3rs3.http.app.routes.0.servers.1.weight=3
r3v3rs3.http.app.load_balancing=round_robin
```

Diğer alanlar yönetim API'sinin adlarını kullanır:

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

r3v3rs3, proxy'yi kullanmadan önce düz metin password ve token değerlerini hash'e çevirir. Kaynak düz metin değeri tutmaya devam eder. Bu yüzden etiketlerde `password_hash` ve `token_hash` tercih edin.

## TCP ve UDP proxy'ler

`port`, kaynağın adresinde tek bir upstream sunucu tanımlar. `upstream_servers` ile birlikte kullanılamaz.

```text
r3v3rs3.tcp.db.ports=postgres
r3v3rs3.tcp.db.port=5432

r3v3rs3.udp.dns.ports=dns
r3v3rs3.udp.dns.upstream_servers.0.addr=/ip4/10.0.0.53/udp/53
r3v3rs3.udp.dns.session_idle_timeout=30s
```

## ACME sertifikaları

`acme`, sertifika listesindeki mevcut bir ACME kaydını verir. r3v3rs3, kaydın hesabı, challenge'ı ve DNS sağlayıcısı ile proxy'nin virtual host'ları için sertifika alır.

```text
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
| `network` | Upstream adreslerinin alındığı Docker ağı. Her container tek ağa bağlıysa boş bırakın. |
| `exposed_by_default` | `true`, `r3v3rs3.` etiketi olan her container'ı okur. `false` yalnız `r3v3rs3.enable=true` olan container'ları okur. |

Ayar değişikliği yalnız sağlayıcıyı yeniden başlatır, sunucu yeniden başlamaz. Sağlayıcının kullandığı sertifika silinemez.

## Container'lar

- Container'ın upstream adresi, seçilen ağdaki IP adresidir. `host` ağ modundaki container `127.0.0.1` kullanır.
- Sağlık kontrolü başarısız olan container atlanır ve sorun olarak gösterilir. Sağlık kontrolünü henüz geçmemiş container atlanır.
- Bir Compose servisinin replikaları proxy'lerini paylaşır. Her replika kendi sunucularını proxy'nin route'larına ekler, böylece proxy yükü replikalara dağıtır. Replikalar aynı sayıda route tanımlamalıdır.
- Proxy'nin key'i Compose projesi, Compose servisi ve proxy adından oluşur. Compose dışındaki container kendi adını kullanır.
- r3v3rs3, olay gelmezse container listesini beş dakikada bir yeniden okur.

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

"Portlar" sayfasında `http` adında bir port ekleyin, sağlayıcıyı `proxy` ağı ile açın ve stack'i başlatın. Ağ `name` alanını vermezse Compose ağ adının başına proje adını ekler.

> Docker socket erişimi Docker host'unun tam kontrolünü verir. Bu yetki root erişimine eşittir. `:ro` seçeneği yalnız socket dosyasına uygulanır ve API'yi salt okunur yapmaz. Yönetim paneline veya host'a güvenilmeyen ağlardan erişilebiliyorsa r3v3rs3'ü yalnız `GET /containers/json` ve `GET /events` isteklerine izin veren bir Docker socket proxy'sine bağlayın.

# Kubernetes

Kubernetes sağlayıcısı, `networking.k8s.io/v1` Ingress kaynaklarını, `r3v3rs3.io/v1` R3v3rs3Proxy kaynaklarını, backend'lerinin Service ve EndpointSlice'larını ve Ingress kaynaklarının TLS secret'larını okur. Değişiklikleri watch stream'leriyle izler. Değişen bir kaynak, hazır hale gelen yeni bir pod veya yenilenen bir secret proxy'leri yaklaşık bir saniye içinde günceller.

`deploy/kubernetes` dizininde custom resource definition (`crd.yaml`), service account ve ClusterRole'ü (`rbac.yaml`) ve örnek bir Deployment (`deployment.yaml`) bulunur.

## Ayarlar

"Ayarlar" sayfasında "Kubernetes Servis Keşfi" bölümünü doldurun veya `config.toml` dosyasını düzenleyin:

```toml
[discovery.kubernetes]
enabled = true
kubeconfig = ""
namespaces = []
ingress = true
crd = true
ingress_class = "r3v3rs3"
ports = ["http"]
```

| Ayar | Anlamı |
|---|---|
| `enabled` | Sağlayıcıyı başlatır. |
| `kubeconfig` | Kubeconfig dosyasının yolu. `KUBECONFIG` değişkenini veya `~/.kube/config` dosyasını kullanmak için boş bırakın. Cluster içinde boş değer pod'un service account'unu kullanır. |
| `namespaces` | Okunacak namespace'ler. Her namespace'i okumak için boş bırakın. |
| `ingress` | Ingress kaynaklarını okur. Varsayılan değer `true` olur. |
| `crd` | R3v3rs3Proxy kaynaklarını okur. Varsayılan değer `false` olur. Önce custom resource definition'ı kurun, çünkü sağlayıcı cluster'ın tanımadığı bir kaynağı izleyemez. Sağlayıcı `ingress` veya `crd` ayarını gerektirir. |
| `ingress_class` | r3v3rs3 yalnız bu class'ın Ingress kaynaklarını okur. Class, `spec.ingressClassName` alanından veya `kubernetes.io/ingress.class` annotation'ından gelir. Her Ingress'i okumak için boş bırakın. |
| `ports` | `r3v3rs3.io/ports` annotation'ı olmayan bir Ingress'in kullandığı port adları veya id'leri. |

Ayar değişikliği yalnız sağlayıcıyı yeniden başlatır, sunucu yeniden başlamaz.

## Ingress kaynakları

- Ingress'in her host'u bir HTTP proxy olur. Host, proxy'nin virtual host'udur. Host'un her yolu bir route olur. Proxy'nin adı `<namespace>/<name> <host>` olur.
- Host'u olmayan kurallar ve `spec.defaultBackend` virtual host'u olmayan tek bir proxy olur. Porttaki başka hiçbir proxy'nin eşleşmediği istekler bu proxy'ye gelir.
- `Prefix` ve `ImplementationSpecific` yol türleri yolu önek olarak eşler. r3v3rs3'te tam yol eşleşmesi yoktur. Bu yüzden `Exact` türündeki yol sorun sayılır ve route olmaz.
- Route'un sunucuları, Service'in EndpointSlice'larında Service portunun hazır endpoint'leridir. Backend, Service portunu `port.number` veya `port.name` ile seçer. `ready` koşulu olmayan endpoint hazır sayılır.
- Adı `https` olan veya `appProtocol` değeri `https` olan Service portu endpoint'lere HTTPS ile bağlanır.
- Bulunmayan Service, bulunmayan port veya hazır endpoint'i olmayan Service sorun sayılır. Route sunucusuz kalır. Bu yüzden route'un istekleri hata alır ve başka bir route'a gitmez.
- Yalnız Service backend'i kullanılabilir. `resource` backend'i sorun sayılır.

## Annotation'lar

`r3v3rs3.io/<field>` annotation'ı Ingress'in proxy'lerinde bir alanı ayarlar. Alanlar ve değerler, [Etiketler](@/discovery.tr.md#etiketler) bölümündeki HTTP proxy alanlarıdır. `r3v3rs3.http.<name>.` öneki yazılmaz.

| Annotation | Anlamı |
|---|---|
| `r3v3rs3.io/ports` | Proxy'lerin port adları veya id'leri. `ports` ayarının yerine geçer. Bu annotation'ı ve ayarı olmayan Ingress sorun sayılır. |
| `r3v3rs3.io/name` | Proxy'lerin adı. |
| `r3v3rs3.io/acme` | Her host için sertifika alan ACME kaydı. [ACME sertifikaları](@/discovery.tr.md#acme-sertifikalari) bölümüne bakın. |
| `r3v3rs3.io/<field>` | Diğer her alan, örneğin `r3v3rs3.io/rate_limit.requests` veya `r3v3rs3.io/headers.response.0.name`. |

`routes` ve `vhosts` alanlarını Ingress kuralları ayarlar. Bu yüzden `routes`, `vhosts`, `port` veya `scheme` annotation'ı sorun sayılır.

## TLS secret'ları

- r3v3rs3, seçili bir Ingress'in `spec.tls` alanında adı geçen `kubernetes.io/tls` secret'larını okur. Her secret'ı sertifika listesine sunucu sertifikası olarak ekler. TLS portu sertifikayı, yüklenen bir sertifikada olduğu gibi sunucu adıyla seçer.
- r3v3rs3 bu sertifikaları kaydetmez. Sertifika listesi sağlayıcıyı gösterir. Secret'tan gelen sertifika silinemez. Ingress veya secret silinince sertifika listeden çıkar.
- Bulunmayan secret veya sertifikası ya da private key'i geçersiz olan secret sorun sayılır.

## R3v3rs3Proxy kaynakları

Bir R3v3rs3Proxy kaynağı, proxy modelinin alanlarıyla bir proxy tanımlar. Bu yüzden TCP veya UDP proxy de tanımlayabilir ve her alanı ayarlayabilir. Önce tanımı kurun, sonra `crd` ayarını açın:

```bash
kubectl apply -f deploy/kubernetes/crd.yaml
```

- `spec.protocol` değeri `http`, `tcp` veya `udp` olur. Varsayılan değer `http` olur.
- `spec` içindeki diğer alanlar, [Etiketler](@/discovery.tr.md#etiketler) bölümündeki proxy alanlarıdır. Örneğin `ports`, `name`, `active`, `acme`, `vhosts` ve `routes`. Liste bir YAML listesidir. Sayı ve boolean birer YAML değeridir.
- `ports` ayarı yoksa `ports` alanı gereklidir. Varsayılan ad `<namespace>/<name>` olur.
- `name` ve `port` alanlarıyla verilen `service`, kaynağın namespace'indeki bir Service'i seçer. `port`, Service portunun numarası veya adıdır. HTTP proxy'nin bir route'unda Service'in hazır endpoint'leri route'un sunucuları olur. TCP veya UDP proxy'de upstream sunucular olur. `service`, `servers` veya `upstream_servers` ile birlikte kullanılamaz.
- Service'inin hazır endpoint'i olmayan route, Ingress'te olduğu gibi sunucusuz kalır ve sorun sayılır.
- Kaynağın adresi yoktur, bu yüzden `port` ve `scheme` kullanılamaz.
- `spec` içindeki bir key `.` içeremez.
- `kubectl get rproxy` kaynakları listeler.

```yaml
apiVersion: r3v3rs3.io/v1
kind: R3v3rs3Proxy
metadata:
  name: whoami
  namespace: default
spec:
  ports: [https]
  vhosts: [whoami.example.com]
  rate_limit:
    requests: 100
    per: minute
  routes:
    - path: /
      service:
        name: whoami
        port: http
---
apiVersion: r3v3rs3.io/v1
kind: R3v3rs3Proxy
metadata:
  name: postgres
  namespace: default
spec:
  protocol: tcp
  ports: [postgres]
  service:
    name: postgres
    port: 5432
```

## RBAC

Sağlayıcı beş kaynağı listeler ve izler. `deploy/kubernetes/rbac.yaml` dosyası bu nesneleri içerir. r3v3rs3'ün service account'una bir ClusterRole verin veya `namespaces` ayarındaki her namespace'te bir Role verin:

```yaml
apiVersion: v1
kind: ServiceAccount
metadata:
  name: r3v3rs3
  namespace: r3v3rs3
---
apiVersion: rbac.authorization.k8s.io/v1
kind: ClusterRole
metadata:
  name: r3v3rs3
rules:
  - apiGroups: ["networking.k8s.io"]
    resources: ["ingresses"]
    verbs: ["get", "list", "watch"]
  - apiGroups: [""]
    resources: ["services", "secrets"]
    verbs: ["get", "list", "watch"]
  - apiGroups: ["discovery.k8s.io"]
    resources: ["endpointslices"]
    verbs: ["get", "list", "watch"]
  - apiGroups: ["r3v3rs3.io"]
    resources: ["r3v3rs3proxies"]
    verbs: ["get", "list", "watch"]
---
apiVersion: rbac.authorization.k8s.io/v1
kind: ClusterRoleBinding
metadata:
  name: r3v3rs3
roleRef:
  apiGroup: rbac.authorization.k8s.io
  kind: ClusterRole
  name: r3v3rs3
subjects:
  - kind: ServiceAccount
    name: r3v3rs3
    namespace: r3v3rs3
```

Sağlayıcı yalnız `kubernetes.io/tls` türündeki secret'ları okur. Kubernetes RBAC ise `list` iznini türe göre sınırlayamaz. Bu yüzden rol, r3v3rs3'ün namespace'lerindeki bütün secret'ları okumasına izin verir. Bu erişimi sınırlamak için `namespaces` ayarını ve her namespace'te bir Role kullanın.

## Ingress örneği

```yaml
apiVersion: networking.k8s.io/v1
kind: Ingress
metadata:
  name: whoami
  namespace: default
  annotations:
    r3v3rs3.io/ports: https
    r3v3rs3.io/headers.response.0.action: set
    r3v3rs3.io/headers.response.0.name: X-Frame-Options
    r3v3rs3.io/headers.response.0.value: DENY
spec:
  ingressClassName: r3v3rs3
  tls:
    - hosts: [whoami.example.com]
      secretName: whoami-tls
  rules:
    - host: whoami.example.com
      http:
        paths:
          - path: /
            pathType: Prefix
            backend:
              service:
                name: whoami
                port:
                  name: http
```

"Portlar" sayfasında `https` adında bir TLS portu ekleyin. r3v3rs3 pod adreslerine erişebilmelidir. Bu yüzden r3v3rs3'ü cluster içinde veya cluster'ın bir düğümünde çalıştırın.

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

Yönetim API'si token'ı döndürmez. Kayıtlı bir token varsa `GET /api/config` yanıtında `token_set: true` döner. `token` alanı olmayan bir `PUT /api/config` isteği kayıtlı token'ı korur. `"token": ""` token'ı siler. `config.toml` token'ı düz metin olarak saklar. Bu yüzden config dizinini yalnız r3v3rs3 kullanıcısı okuyabilmelidir.

## Catalog servisleri

- Etiket `<key>=<value>` biçimindedir, örneğin `r3v3rs3.http.app.ports=https`. `=` içermeyen bir `r3v3rs3.` etiketi sorun sayılır.
- r3v3rs3 yalnız sağlık kontrollerini geçen servis örneklerini okur. Sağlık kontrolünü geçen örneği olmayan seçili bir servis sorun sayılır ve proxy'leri kaldırılır.
- Servis örneğinin upstream adresi servis adresidir. Servis adresi olmayan örnek düğüm adresini kullanır.
- `port`, `routes` ve `upstream_servers` alanları olmayan bir proxy servisin portunu kullanır. `port` ve `servers` alanları olmayan bir route da servisin portunu kullanır.
- Bir servisin örnekleri proxy'lerini paylaşır. Her örnek kendi sunucularını proxy'nin route'larına ekler, böylece proxy yükü örneklere dağıtır.

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

Yönetim API'si parolayı döndürmez. Kayıtlı bir parola varsa `GET /api/config` yanıtında `password_set: true` döner. `password` alanı olmayan bir `PUT /api/config` isteği kayıtlı parolayı korur. `"password": ""` parolayı siler. `config.toml` parolayı düz metin olarak saklar. Bu yüzden config dizinini yalnız r3v3rs3 kullanıcısı okuyabilmelidir.

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
