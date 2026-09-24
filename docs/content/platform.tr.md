+++
title = "Deploy platformu"
description = "Container image'larını deploy edin ve alan adlarını r3v3rs3 üzerinden yönlendirin"
weight = 0
+++

# Deploy platformu

Deploy platformu, uygulamaları r3v3rs3 sunucusunun Docker Engine'inde container olarak çalıştırır ve alan adlarını r3v3rs3 proxy'leri üzerinden yönlendirir. Yeni bir deployment, çalışan container'ın yanında başlar. Proxy yeni container'a ancak container sağlık kontrolünden geçince geçer.

Platform geliştirme aşamasındadır. Bu sürüm, WebUI veya yönetim API'si üzerinden yerel Docker Engine'e registry'deki hazır bir image'ı deploy eder, bir Git reposundaki Dockerfile ile image build eder veya bir Git reposundaki Docker Compose dosyasını başlatır. Uzak sunucular sonraki sürümlerde gelir.

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
| `enabled` | `false` | Platformu başlatır. Cluster düğümü platformu çalıştırmaz. |
| `docker` | `unix:///var/run/docker.sock` | Docker Engine API'si: bir Unix socket'i veya düz TCP. TLS desteklenmez. |
| `proxy_ports` | boş | Uygulamaları sunan portların adları veya id'leri, örneğin HTTP portu ve HTTPS portu. |
| `acme` | yok | Uygulamaların alan adlarına sertifika sipariş eden ACME kaydının id'si. Bu değer yoksa uygulamalar sertifika almaz. |

Yönetim API'si `[platform]` bölümünü değiştirmez. Bir değişiklik yeniden başlatmadan sonra geçerli olur.

Platform ilk başlangıçta config dizininde iki dosya oluşturur:

- `platform.db`: uygulamalar, ortam değişkenleri ve deployment'lar.
- `platform.key`: ortam değişkenlerinin değerlerini şifreleyen key. Dosya modu `0600` olur. Bu dosyanın yedeğini `platform.db` yedeğinin yanında saklayın, çünkü değerler bu key olmadan açılmaz.

Docker socket'ine erişim, host üzerinde root erişimine eşittir. Platformu yalnız r3v3rs3'ün her container'ı yönetebileceği bir sunucuda açın. r3v3rs3 bir container içinde çalışıyorsa [Docker ile kurulum](@/tutorials/install-docker.tr.md#deploy-platformu) rehberindeki gibi `-platform` tag son ekli image'ı, host ağ modunu ve Docker socket'ini kullanın.

## Uygulamalar

Bir uygulama bir image'ı, container'ın dinlediği portu ve uygulamaya yönlenen alan adlarını belirtir:

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
- `domains` DNS adlarıdır. Wildcard veya IP adresi kabul edilmez, çünkü her alan adı ACME'den bir sertifika alır.
- `health_check_path`, uygulama hazır olduğunda `2xx` veya `3xx` döndüren bir HTTP yoludur. Bu değer yoksa deployment, port bir bağlantıyı kabul edip açık tutana kadar bekler.
- `volumes` adlandırılmış Docker volume'larını bağlar. Host üzerindeki bir yol bağlanamaz.
- `restart` değeri `no`, `on-failure`, `unless-stopped` veya `always` olur.

Uygulamanın, alan adlarının veya ortam değişkenlerinin değişikliği sonraki deployment'ta geçerli olur.

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
| `repository` | | Kullanıcı adı, parola, sorgu veya fragment içermeyen bir `https://` URL'si. |
| `branch` | `main` | Bir branch veya tag. |
| `context` | `.` | Docker'ın build context olarak aldığı repo dizini. |
| `dockerfile` | `Dockerfile` | `context` dizinine göre Dockerfile yolu. |

Private bir repo bir erişim token'ı gerektirir. Örnekler: reponun içeriğini okuma izni olan bir GitHub fine-grained token'ı, veya `read_repository` kapsamı olan bir GitLab ya da Gitea token'ı. `PUT /api/apps/{id}/git_token`, `{"token": "..."}` gövdesiyle token'ı ayarlar. `DELETE /api/apps/{id}/git_token` token'ı siler. r3v3rs3 token'ı `platform.key` ile şifreler. Yönetim API'si token'ı hiçbir zaman döndürmez, uygulamada yalnız `"git_token_set": true` görünür. git token'ı `x-access-token` kullanıcı adıyla HTTP Basic kimlik doğrulaması olarak alır. Token URL'de veya komut satırında yer almaz, git'e bir ortam değişkeniyle ulaşır.

r3v3rs3'ü çalıştıran sunucuda `git` binary'si bulunmalıdır. r3v3rs3, `git`'i host'un yapılandırması ve hook'lar olmadan, yalnız HTTPS üzerinden çalıştırır. Build context'inde `.git` dizini yer almaz. Context'teki bir sembolik link link olarak kalır. Bu yüzden bir build, repo dışındaki bir dosyayı okuyamaz. Context en fazla 512 MiB olabilir.

Docker image'ı klasik builder ile build eder. Aynı anda iki build çalışır, diğerleri sırada bekler. Builder, çok aşamalı bir Dockerfile'ın aşamalarını build cache'i olarak saklar. Bu yüzden aynı uygulamanın sonraki build'i daha hızlı biter. `docker image prune` bu cache'i siler.

### Compose kaynağı

`compose` kaynağı olan bir uygulama, bir reponun bir branch'indeki Docker Compose dosyasını başlatır:

```json
"source": {
  "type": "compose",
  "repository": "https://github.com/owner/shop.git",
  "branch": "main",
  "file": "deploy/compose.yaml",
  "service": "web"
}
```

| Alan | Varsayılan | Açıklama |
|---|---|---|
| `repository` | | Git kaynağındaki gibi bir `https://` URL'si. Uygulamanın Git token'ı private bir repoyu clone eder. |
| `branch` | `main` | Bir branch veya tag. |
| `file` | `compose.yaml`, `compose.yml`, `docker-compose.yaml` ve `docker-compose.yml` dosyalarından ilk bulunan | Reponun köküne göre Compose dosyası. Dosyadaki göreli yollar dosyanın kendi dizinine göre çözülür. |
| `service` | | Alan adına gelen istekleri `port` üzerinde alan servis. Servis adı küçük harf, rakam ve `_.-` içerir. |

Sunucuda `git` binary'si ve Compose eklentisi olan `docker` binary'si bulunmalıdır. r3v3rs3, `docker compose` komutunu `r3v3rs3-<app id>` Compose projesiyle çalıştırır ve `service` servisi için şu ayarları yapan bir dosya ekler:

- `r3v3rs3-<app id>-<deployment id>` container adı ve platformun etiketleri,
- tek bir dışarı açılmış port: `127.0.0.1` üzerindeki boş bir porta `port`. Bu port, Compose dosyasındaki servisin `ports` değerinin yerini alır.
- uygulamanın ortam değişkenleri. Dosya yalnız key'leri içerir. `docker compose` değerleri kendi ortamından okur, bu yüzden hiçbir değer diske yazılmaz. Aynı değerler Compose dosyasındaki `${VARIABLE}` referanslarını da doldurur. `PATH`, `HOME`, `DOCKER_CONFIG` ve `DOCKER_HOST` adları `docker` binary'sine aittir. Bu adlardan birini kullanan uygulamanın deployment'ı başarısız olur.

Compose dosyasındaki başka bir servis port dışarı açamaz, çünkü dışarı açılmış bir port r3v3rs3'ü atlar. Böyle bir port içeren Compose dosyası, hiçbir servis başlamadan deployment'ı başarısız yapar. Hata mesajı servisin adını verir. Servisler birbirine projenin ağı üzerinden ulaşır.

Compose uygulaması `volumes`, `restart` ve `limits` ayarlarını Compose dosyasında yapar. Bu yüzden uygulama tanımı bu alanları kabul etmez. `service` servisi tek bir container olarak çalışır, çünkü bir container adı vardır.

Compose deployment'ı `service` servisini yeniden oluşturur: Docker yeni container başlamadan önce eskisini durdurur. Bu yüzden uygulama, yeni container sağlık kontrolünden geçene kadar yanıt vermez. Sağlık kontrolünden geçemeyen bir deployment yeni container'ı log'u için saklar. Uygulamanın sonraki deployment'a veya geri almaya kadar sağlıklı bir container'ı olmaz. Çalışan deployment'ın checkout'u config dizinindeki `compose/<app id>/` altında kalır, çünkü Compose dosyasındaki bir bind mount onu okuyabilir.

r3v3rs3, Compose dosyasının diğer ayarlarını denetlemez. Bir Compose dosyası host üzerindeki bir yolu bağlayabilir, privileged container çalıştırabilir veya host'un ağını kullanabilir. Bu yüzden uygulama düzenleyebilen her hesap, bu yolla host'u ele geçirebilir. Edit iznini yalnız buna yetkisi olan hesaplara verin.

### Ortam değişkenleri

`PUT /api/apps/{id}/env` bir uygulamanın ortam değişkenlerini değiştirir. `"secret": true` olan bir değişkenin değeri yönetim API'sinden bir daha çıkmaz: API değişkeni değeri olmadan döndürür, değersiz bir güncelleme de mevcut değeri korur. r3v3rs3 her değeri `platform.key` ile şifreler ve yalnız bir container başlarken açar.

## Deployment'lar

`POST /api/apps/{id}/deploy` bir deployment başlatır ve onu `queued` durumuyla döndürür. `GET /api/deployments/{id}` ilerlemeyi gösterir.

1. r3v3rs3 image'ı çeker ve özetini kaydeder. Git kaynağında branch'i clone eder, commit'i kaydeder, image'ı `r3v3rs3/<app id>:<deployment id>` adıyla build eder ve image id'sini kaydeder. Compose kaynağında branch'i clone eder, commit'i kaydeder, `docker compose up --build` çalıştırır ve 3. adımla devam eder.
2. `r3v3rs3-<app id>-<deployment id>` container'ını `r3v3rs3-<app id>` ağında oluşturur. Böylece farklı uygulamaların container'ları birbirine ulaşamaz. Container portunu `127.0.0.1` üzerindeki boş bir porttan dışarı açar. Böylece container'a yalnız host üzerindeki r3v3rs3 ulaşır.
3. Sağlık kontrolü için en fazla 120 saniye bekler. Duran veya kontrolden geçemeyen bir container deployment'ı `failed` ile bitirir. Hata mesajı container log'unun son satırlarını içerir. Eski container istekleri karşılamaya devam eder.
4. Deployment'ı `running`, öncekini `superseded` olarak işaretler ve alan adlarını yeni container'a yönlendirir.
5. 10 saniye sonra eski container'ı durdurur ve siler. Bu süre, eski container'daki açık isteklerin bitmesi içindir.

Bir uygulama aynı anda tek bir deployment çalıştırır. Bir deployment sürerken gelen ikinci deploy, geri alma veya silme isteği `409 app_busy` alır. Sunucu yeniden başlarsa bitmemiş deployment `failed` olur.

| Durum | Anlamı |
|---|---|
| `queued` | Deployment başlamayı bekliyor. |
| `building` | r3v3rs3 branch'i clone ediyor ve image'ı build ediyor veya Compose dosyasını okuyor. |
| `deploying` | Image çekiliyor, `docker compose up` çalışıyor veya yeni container sağlık kontrolünü bekliyor. |
| `running` | Deployment'ın container'ı uygulamayı sunuyor. |
| `superseded` | Daha yeni bir deployment bunun yerini aldı. |
| `failed` | Deployment durdu. Nedeni `message` alanındadır. |

### Geri alma

`POST /api/deployments/{id}/rollback`, önceki bir deployment'ın image özeti, uygulama ayarları ve ortam değişkenleriyle yeni bir deployment başlatır. Image'ı tag değil özet seçer. Bu yüzden tag değişse bile geri alma aynı image'ı çalıştırır. Image'ını çekmeden önce başarısız olan bir deployment'ın özeti yoktur. Bu deployment için geri alma isteği `400 rollback_unavailable` alır.

Bir Git uygulamasını geri alma build yapmaz, build edilmiş image'ı yeniden başlatır. r3v3rs3, bir uygulamanın en yeni 5 deployment'ının ve çalışan deployment'ının image'larını saklar, daha eskilerini siler. Image'ı silinmiş bir deployment'a geri alma `the image ... is no longer present` mesajıyla başarısız olur.

Bir Compose uygulamasını geri alma, önceki deployment'ın kaydedilmiş commit'ini clone eder ve `docker compose up --build` komutunu o deployment'ın uygulama ayarları ve ortam değişkenleriyle yeniden çalıştırır. Commit'ini kaydetmeden önce başarısız olan bir Compose deployment'ı için geri alma isteği `400 rollback_unavailable` alır.

## Yönlendirme

Platform, en az bir alan adı olan her çalışan uygulama için bir proxy ekler. Bu proxy'ler `Platform` sağlayıcısından gelir. Bu yüzden [servis keşfi](@/discovery.tr.md) proxy'leri gibi salt okunurdur ve proxy listesi sağlayıcının durumunu gösterir. Her proxy:

- `proxy_ports` portlarında dinler,
- uygulamanın alan adlarına yanıt verir,
- istekleri container'ın `127.0.0.1` üzerindeki dışarı açılmış portuna gönderir,
- sertifikasını `acme` ACME kaydından, kaydın hesabı, challenge'ı ve DNS sağlayıcısı ile alır. [ACME sertifikaları](@/discovery.tr.md#acme-sertifikalari) kuralları geçerlidir.

Alan adı olmayan bir uygulama proxy almaz. Container'ı olmayan veya durmuş olan çalışan bir deployment, sağlayıcı durumunda bir sorun olarak görünür. r3v3rs3 container'ları 15 saniyede bir okur. Bu yüzden Docker'ın yeni bir portta yeniden başlattığı container yönlendirmesini geri alır.

## Uygulamayı silme

`DELETE /api/apps/{id}` uygulamanın container'larını, ağını ve build edilmiş image'larını durdurup siler, proxy'sini kaldırır, ortam değişkenlerini ve deployment'larını siler. Adlandırılmış volume'lar kalır. Compose uygulamasında `docker compose down` çalıştırır, projenin build ettiği image'ları ve checkout'u siler. Compose projesinin volume'ları kalır.

## WebUI

Kenar çubuğundaki **Platform** grubu **Uygulamalar** sayfasını içerir. Grup yalnız proxy listesi olmayan hesaplarda görünür. Read izni olan hesap uygulamaları, deployment'larını ve log'larını görür. Edit izni uygulama ekler, değiştirir, deploy eder ve siler.

### Uygulamalar

**Uygulamalar** sayfası her uygulamayı **Kaynak**, **Alan adları** ve **Son deployment** durumuyla listeler. Bir uygulamanın satır işlemleri şunlardır:

- **Düzenle** uygulama sayfasını açar. Edit izni olmayan hesap **Görüntüle** ve salt okunur bir form görür.
- **Deployment'lar** deployment geçmişini açar.
- **Log** container log'unu açar.
- **Deploy et** onaydan sonra bir deployment başlatır.

Sayfa, bitmemiş bir deployment varken uygulamaları 2 saniyede bir, yoksa 10 saniyede bir yeniden okur.

### Uygulama ekleme ve değiştirme

**Ekle** boş bir uygulama formu açar. **Kaynak** alanında **Image**, **Git** veya **Compose** seçilir ve form o kaynağın alanlarını gösterir. Compose uygulamasında **Volume'lar**, **Yeniden başlatma politikası**, **Bellek limiti (MB)** ve **CPU limiti** gizlenir, çünkü bunları Compose dosyası ayarlar.

- **Alan adları** alanına her satıra bir alan adı yazın.
- **Volume'lar** alanına her satıra bir mount yazın: `volume:/yol`, salt okunur mount için `volume:/yol:ro`.
- **Bellek limiti (MB)** tam bir megabayt değeri, **CPU limiti** `1.5` gibi bir CPU sayısı alır. Boş alan limit koymaz.

**Oluştur** uygulamayı kaydeder ve sayfasını açar. Mevcut bir uygulamanın sayfasında formun altında üç bölüm daha bulunur:

- **Git token**, Git veya Compose uygulamasında. **Token'ı ayarla** bir token kaydeder, **Token'ı kaldır** onaydan sonra token'ı siler. Sayfa yalnız token'ın ayarlı olup olmadığını gösterir.
- **Ortam değişkenleri**. **Değişken ekle**, **Key**, **Değer** ve **Secret** kutusu olan bir satır ekler. Kaydedilmiş secret bir değişkenin değeri **Değişmedi** olarak görünür. Değeri korumak için alanı boş bırakın. **Değişkenleri kaydet** bütün değişkenleri değiştirir ve sonraki deployment bunları kullanır.
- **Uygulamayı sil** onaydan sonra uygulamayı container'larıyla birlikte siler.

### Deployment geçmişi

Deployment geçmişi son 100 deployment'ı durumu, **Tetikleyen**, **Commit**, **Hesap**, **Başlangıç**, **Süre** ve **Mesaj** ile gösterir. **Geri al** onaydan sonra önceki bir deployment'ı yeniden başlatır. Link yalnız bitmiş, artık çalışmayan ve image özeti kaydedilmiş bir deployment'ta görünür. Compose uygulamasında kaydedilmiş commit gerekir. Uygulamanın bitmemiş bir deployment'ı varken link gizlenir. Sayfa **Uygulamalar** sayfasıyla aynı aralıklarla yenilenir.

### Log

Log sayfası çalışan container'ın stdout ve stderr çıktısının son 200 satırını gösterir ve 10 saniyede bir yeniden okur. **Yenile** satırları hemen okur. Çalışan container'ı olmayan uygulamada "Uygulamanın çalışan container'ı yok." mesajı görünür.

## Yönetim API'si

| Route | İzin | Açıklama |
|---|---|---|
| `GET /api/targets` | Read | Uygulamaları çalıştıran Docker host'ları. |
| `GET /api/apps` | Read | Uygulamalar. |
| `POST /api/apps` | Edit | Uygulama ekler. |
| `GET /api/apps/{id}` | Read | Bir uygulamayı döndürür. |
| `PUT /api/apps/{id}` | Edit | Bir uygulamayı değiştirir. |
| `DELETE /api/apps/{id}` | Edit | Bir uygulamayı container'larıyla birlikte siler. |
| `GET /api/apps/{id}/env` | Edit | Secret değerleri olmadan ortam değişkenleri. |
| `PUT /api/apps/{id}/env` | Edit | Ortam değişkenlerini değiştirir. |
| `PUT /api/apps/{id}/git_token` | Edit | Private bir reponun token'ını ayarlar. |
| `DELETE /api/apps/{id}/git_token` | Edit | Token'ı siler. |
| `GET /api/apps/{id}/deployments` | Read | Bir uygulamanın son 100 deployment'ı. |
| `GET /api/apps/{id}/logs?tail=200` | Read | Çalışan deployment'ın container log'unun son satırları, 1 ile 1000 satır arası. Uygulamanın çalışan deployment'ı yoksa `running` değeri `false` olur. |
| `POST /api/apps/{id}/deploy` | Edit | Bir deployment başlatır. |
| `GET /api/deployments/{id}` | Read | Bir deployment'ı döndürür. |
| `POST /api/deployments/{id}/rollback` | Edit | Önceki bir deployment'ı tekrarlar. |

Proxy listesi olan hesap, her platform route'u için `403 forbidden` alır. Bkz. [Hesaplar](@/accounts.tr.md). Denetim kaydı, uygulamanın her değişikliğini, her deployment'ı ve her geri almayı kaydeder. Ortam değişkeni değişikliğinin özeti key'leri adlandırır, hiçbir değeri içermez. Token değişikliğinin özeti yalnız uygulamayı adlandırır.
