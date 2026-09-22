+++
title = "Deploy platformu"
description = "Container image'larını deploy edin ve domain'lerini r3v3rs3 üzerinden yönlendirin"
weight = 0
+++

# Deploy platformu

Deploy platformu, uygulamaları r3v3rs3 sunucusunun Docker Engine'inde container olarak çalıştırır ve domain'lerini r3v3rs3 proxy'leri üzerinden yönlendirir. Yeni bir deployment, çalışan container'ın yanında başlar. Proxy yeni container'a ancak container health check'ten geçince geçer.

Platform geliştirme aşamasındadır. Bu sürüm, yönetim API'si üzerinden yerel Docker Engine'e ya registry'deki hazır bir image'ı deploy eder ya da bir Git reposundaki Dockerfile ile image build eder. WebUI sayfaları, Docker Compose uygulamaları ve uzak sunucular sonraki sürümlerde gelir.

## Platformu açma

`config.toml` dosyasına bir `[platform]` bölümü ekleyin ve sunucuyu yeniden başlatın:

```toml
[platform]
enabled = true
docker = "unix:///var/run/docker.sock"
proxy_ports = ["http", "https"]
acme = "bcd-fgh"
```

| Key | Varsayılan | Açıklama |
|---|---|---|
| `enabled` | `false` | Platformu başlatır. Cluster node'u platformu çalıştırmaz. |
| `docker` | `unix:///var/run/docker.sock` | Docker Engine API'si: bir Unix socket'i veya düz TCP. TLS desteklenmez. |
| `proxy_ports` | boş | Uygulamaları sunan portların adları veya id'leri, örneğin HTTP portu ve HTTPS portu. |
| `acme` | yok | Uygulama domain'lerinin sertifikalarını sipariş eden ACME kaydının id'si. Bu değer yoksa uygulamalar sertifika almaz. |

Yönetim API'si `[platform]` bölümünü değiştirmez. Bir değişiklik yeniden başlatmadan sonra geçerli olur.

Platform ilk başlangıçta config dizininde iki dosya oluşturur:

- `platform.db`: uygulamalar, environment değişkenleri ve deployment'lar.
- `platform.key`: environment değerlerini şifreleyen key. Dosya modu `0600` olur. Bu dosyanın yedeğini `platform.db` yedeğinin yanında saklayın, çünkü değerler bu key olmadan açılmaz.

Docker socket'ine erişim, host üzerinde root erişimine eşittir. Platformu yalnız r3v3rs3'ün her container'ı yönetebileceği bir sunucuda açın. r3v3rs3 bir container içinde çalışıyorsa `/var/run/docker.sock` dosyasını container'a bağlayın.

## Uygulamalar

Bir uygulama bir image'ı, container'ın dinlediği portu ve uygulamaya yönlenen domain'leri belirtir:

```json
{
  "name": "shop",
  "target": "local",
  "spec": {
    "source": {"type": "image", "image": "nginx:1.27"},
    "port": 80,
    "domains": ["shop.example.com"],
    "health_check_path": "/healthz",
    "volumes": [{"volume": "shop-data", "target": "/data"}],
    "restart": "unless-stopped",
    "limits": {"memory_bytes": 268435456, "nano_cpus": 500000000}
  }
}
```

- `target` değeri `local` olur. Bu, r3v3rs3 sunucusunun Docker Engine'idir.
- `domains` DNS adlarıdır. Wildcard veya IP adresi kabul edilmez, çünkü her domain ACME'den bir sertifika alır.
- `health_check_path`, uygulama hazır olduğunda `2xx` veya `3xx` döndüren bir HTTP path'idir. Bu değer yoksa deployment, port bir bağlantıyı kabul edip açık tutana kadar bekler.
- `volumes` adlandırılmış Docker volume'larını bağlar. Host path'i bağlanamaz.
- `restart` değeri `no`, `on-failure`, `unless-stopped` veya `always` olur.

Uygulamanın, domain'lerinin veya environment değişkenlerinin değişikliği sonraki deployment'ta geçerli olur.

### Git kaynağı

`git` kaynağı olan bir uygulama, image'ını bir reponun bir branch'inden, reponun Dockerfile'ı ile build eder:

```json
"source": {
  "type": "git",
  "repository": "https://github.com/owner/shop.git",
  "branch": "main",
  "context": ".",
  "dockerfile": "Dockerfile"
}
```

| Alan | Varsayılan | Açıklama |
|---|---|---|
| `repository` | | Kullanıcı adı, parola, query veya fragment içermeyen bir `https://` URL'si. |
| `branch` | `main` | Bir branch veya tag. |
| `context` | `.` | Docker'ın build context olarak aldığı repo dizini. |
| `dockerfile` | `Dockerfile` | `context` dizinine göre Dockerfile yolu. |

r3v3rs3'ü çalıştıran sunucuda `git` binary'si bulunmalıdır. r3v3rs3, `git`'i host'un yapılandırması ve hook'lar olmadan, yalnız HTTPS üzerinden çalıştırır. Build context'inde `.git` dizini yer almaz. Context'teki bir sembolik link link olarak kalır. Bu yüzden bir build, repo dışındaki bir dosyayı okuyamaz. Context en fazla 512 MiB olabilir.

Docker image'ı klasik builder ile build eder. Aynı anda iki build çalışır, diğerleri sırada bekler. Builder, multi-stage bir Dockerfile'ın stage'lerini build cache'i olarak saklar. Bu yüzden aynı uygulamanın sonraki build'i daha hızlı biter. `docker image prune` bu cache'i siler.

### Environment değişkenleri

`PUT /api/apps/{id}/env` bir uygulamanın environment değişkenlerini değiştirir. `"secret": true` olan bir değişkenin değeri yönetim API'sinden bir daha çıkmaz: API değişkeni değeri olmadan döndürür, değersiz bir güncelleme de mevcut değeri korur. r3v3rs3 her değeri `platform.key` ile şifreler ve yalnız bir container başlarken açar.

## Deployment'lar

`POST /api/apps/{id}/deploy` bir deployment başlatır ve onu `queued` durumuyla döndürür. `GET /api/deployments/{id}` ilerlemeyi gösterir.

1. r3v3rs3 image'ı çeker ve digest'ini kaydeder. Git kaynağında branch'i clone eder, commit'i kaydeder, image'ı `r3v3rs3/<app id>:<deployment id>` adıyla build eder ve image id'sini kaydeder.
2. `r3v3rs3-<app id>-<deployment id>` container'ını `r3v3rs3-<app id>` network'ünde oluşturur. Böylece farklı uygulamaların container'ları birbirine ulaşamaz. Container portunu `127.0.0.1` üzerindeki boş bir porta publish eder. Böylece container'a yalnız host üzerindeki r3v3rs3 ulaşır.
3. Health check için en fazla 120 saniye bekler. Duran veya check'ten geçemeyen bir container deployment'ı `failed` ile bitirir. Hata mesajı container log'unun son satırlarını içerir. Eski container istekleri karşılamaya devam eder.
4. Deployment'ı `running`, öncekini `superseded` olarak işaretler ve domain'leri yeni container'a yönlendirir.
5. 10 saniye sonra eski container'ı durdurur ve siler. Bu süre, eski container'daki açık isteklerin bitmesi içindir.

Bir uygulama aynı anda tek bir deployment çalıştırır. Bir deployment sürerken gelen ikinci deploy, rollback veya silme isteği `409 app_busy` alır. Sunucu yeniden başlarsa bitmemiş deployment `failed` olur.

| Durum | Anlamı |
|---|---|
| `queued` | Deployment başlamayı bekliyor. |
| `building` | r3v3rs3 branch'i clone ediyor ve image'ı build ediyor. |
| `deploying` | Image çekiliyor veya yeni container health check'i bekliyor. |
| `running` | Deployment'ın container'ı uygulamayı sunuyor. |
| `superseded` | Daha yeni bir deployment bunun yerini aldı. |
| `failed` | Deployment durdu. Nedeni `message` alanındadır. |

### Rollback

`POST /api/deployments/{id}/rollback`, önceki bir deployment'ın image digest'i, uygulama ayarları ve environment değişkenleriyle yeni bir deployment başlatır. Image'ı tag değil digest seçer. Bu yüzden tag değişse bile rollback aynı image'ı çalıştırır. Image'ını çekmeden önce başarısız olan bir deployment'ın digest'i yoktur. Bu deployment için rollback isteği `400 rollback_unavailable` alır.

Bir Git uygulamasının rollback'i build yapmaz, build edilmiş image'ı yeniden başlatır. r3v3rs3, bir uygulamanın en yeni 5 deployment'ının ve çalışan deployment'ının image'larını saklar, daha eskilerini siler. Image'ı silinmiş bir deployment'a rollback `the image ... is no longer present` mesajıyla başarısız olur.

## Yönlendirme

Platform, en az bir domain'i olan her çalışan uygulama için bir proxy ekler. Bu proxy'ler `Platform` provider'ından gelir. Bu yüzden [servis keşfi](@/discovery.tr.md) proxy'leri gibi salt okunurdur ve proxy listesi provider durumunu gösterir. Her proxy:

- `proxy_ports` portlarında dinler,
- uygulamanın domain'lerine cevap verir,
- istekleri container'ın `127.0.0.1` üzerindeki publish edilmiş portuna gönderir,
- sertifikasını `acme` ACME kaydından, kaydın hesabı, challenge'ı ve DNS provider'ı ile alır. [ACME sertifikaları](@/discovery.tr.md#acme-sertifikalari) kuralları geçerlidir.

Domain'i olmayan bir uygulama proxy almaz. Container'ı olmayan veya durmuş olan çalışan bir deployment, provider durumunda bir sorun olarak görünür. r3v3rs3 container'ları 15 saniyede bir okur. Bu yüzden Docker'ın yeni bir portta yeniden başlattığı container yönlendirmesini geri alır.

## Uygulamayı silme

`DELETE /api/apps/{id}` uygulamanın container'larını, network'ünü ve build edilmiş image'larını durdurup siler, proxy'sini kaldırır, environment değişkenlerini ve deployment'larını siler. Adlandırılmış volume'lar kalır.

## Yönetim API'si

| Route | İzin | Açıklama |
|---|---|---|
| `GET /api/targets` | Read | Uygulamaları çalıştıran Docker host'ları. |
| `GET /api/apps` | Read | Uygulamalar. |
| `POST /api/apps` | Edit | Uygulama ekler. |
| `GET /api/apps/{id}` | Read | Bir uygulamayı döndürür. |
| `PUT /api/apps/{id}` | Edit | Bir uygulamayı değiştirir. |
| `DELETE /api/apps/{id}` | Edit | Bir uygulamayı container'larıyla birlikte siler. |
| `GET /api/apps/{id}/env` | Edit | Secret değerleri olmadan environment değişkenleri. |
| `PUT /api/apps/{id}/env` | Edit | Environment değişkenlerini değiştirir. |
| `GET /api/apps/{id}/deployments` | Read | Bir uygulamanın son 100 deployment'ı. |
| `POST /api/apps/{id}/deploy` | Edit | Bir deployment başlatır. |
| `GET /api/deployments/{id}` | Read | Bir deployment'ı döndürür. |
| `POST /api/deployments/{id}/rollback` | Edit | Önceki bir deployment'ı tekrarlar. |

Proxy listesi olan hesap, her platform route'u için `403 forbidden` alır. Bkz. [Hesaplar](@/accounts.tr.md). Audit log, uygulamanın her değişikliğini, her deployment'ı ve her rollback'i kaydeder. Environment değişikliğinin özeti key'leri adlandırır, hiçbir değeri içermez.
