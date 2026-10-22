+++
title = "Deploy platformu"
description = "Container image'larını deploy edin ve alan adlarını r3v3rs3 üzerinden yönlendirin"
weight = 0
+++

# Deploy platformu

Deploy platformu, uygulamaları r3v3rs3 sunucusunun veya r3v3rs3 agent'ı çalıştıran uzak bir sunucunun Docker Engine'inde container olarak çalıştırır ve alan adlarını r3v3rs3 proxy'leri üzerinden yönlendirir. Yeni bir deployment, çalışan container'ın yanında başlar. Proxy yeni container'a ancak container sağlık kontrolünden geçince geçer.

Platform geliştirme aşamasındadır. Bu sürüm, WebUI veya yönetim API'si üzerinden yerel Docker Engine'e ya da bir [agent hedefine](#agent-hedefleri) registry'deki hazır bir image'ı deploy eder, bir Git reposundaki Dockerfile ile image'ı build eder veya bir Git reposundaki Docker Compose dosyasını başlatır.

## Platformu açma

Web panelinin ayarlar sayfasında bir **Deployment Platform** kartı vardır. Bu kart platformu sunucu çalışırken açar ve kapatır, Docker Engine adresini ve uygulamalara hizmet veren portları tutar. Sidebar, platform kapalıyken platform sayfalarını göstermez.

`config.toml` bölümü katılımsız kurulumlar için yerinde kalır ve sunucu başlangıçta onu okur:

```toml
[platform]
enabled = true
docker = "unix:///var/run/docker.sock"
proxy_ports = ["http", "https"]
acme = "bcd-fgh"
agent_port = 9443
agent_host = "master.example.com"
```

| Key | Varsayılan | Açıklama |
|---|---|---|
| `enabled` | `false` | Platformu başlatır. |
| `docker` | `unix:///var/run/docker.sock` | Docker Engine API'si: bir Unix socket'i veya düz TCP. TLS desteklenmez. |
| `proxy_ports` | boş | Kendi portlarını seçmeyen bir uygulamayı sunan portların adları veya id'leri, örneğin HTTP portu ve HTTPS portu. Bu liste de boşsa böyle bir uygulama tüm HTTP ve HTTPS portlarını alır. |
| `acme` | yok | Uygulamaların alan adlarına sertifika sipariş eden ACME kaydının id'si. Bu değer yoksa uygulamalar sertifika almaz. Ayarlar kartı bu değeri korur; kayıtları ACME sayfası yönetir. |
| `agent_port` | yok | Uzak sunuculardaki agent'ların bağlandığı TCP portu. Sunucu bu portu bütün IPv4 adreslerinde dinler. Bu değer yoksa sunucu hiçbir agent'ı kabul etmez. [Agent hedefleri](#agent-hedefleri) bölümüne bakın. Ayarlar kartı bu değeri korur. |
| `agent_host` | yok | **Hedefler** sayfasındaki agent komutlarında bu sunucunun host adı veya adresi. Bu değer yoksa sayfa kendi adresindeki host adını kullanır. Ayarlar kartı bu değeri korur. |

Ayar kartı `enabled`, `docker` ve `proxy_ports` değerlerini değiştirir; değişiklik sunucu yeniden başlatılmadan geçerli olur. `acme`, `agent_port` ve `agent_host` değerleri kartta korunur, ama kart onları düzenlemez.

Platform ilk başlangıçta yapılandırma dizininde iki dosya oluşturur:

- `platform.db`: uygulamalar, ortam değişkenleri, deployment'lar, hedefler ve Git sağlayıcı bağlantıları.
- `platform.key`: ortam değişkenlerinin değerlerini, uygulamaların Git token'larını ve webhook secret'larını, Git sağlayıcı bağlantılarının client secret'larını, private key'lerini ve token'larını şifreleyen key. Dosya modu `0600` olur. Bu dosyanın yedeğini `platform.db` yedeğinin yanında saklayın, çünkü değerler bu key olmadan açılmaz.

#### Anahtar Rotasyonu (Key Rotation)

Platform anahtarını yenilemek için **Ayarlar** sayfasındaki **Deployment Platform** kartında **Anahtarı Yenile** butonuna tıklayın, CLI'da `r3v3rs3-cli platform rotate-key` çalıştırın veya `POST /api/platform/key/rotate` çağrısı yapın. Bu işlem yeni bir 256-bit AES anahtarı üretir, `platform.db` içindeki tüm gizli verileri eski anahtarla çözüp tek bir veritabanı işleminde yeni anahtarla yeniden mühürler, yeni `platform.key` dosyasını yazar ve önceki anahtarı güvenli biçimde `platform.key.old` (`0600` moduyla) dosyasına arşivler.

Docker socket'ine erişim, host üzerinde root erişimine eşittir. Platformu yalnız r3v3rs3'ün her container'ı yönetebileceği bir sunucuda açın.

## Projeler ve Ortamlar

Uygulamalar ve veritabanları, **Projeler** (örn. *E-Ticaret*, *Faturalandırma*, *Analitik*) ve alt kademedeki **Ortamlar** (örn. *Production*, *Staging*, *Development*) altında organize edilebilir. Bu hiyerarşi hem düzenli bir yönetim hem de ortam düzeyinde paylaşılan yapılandırma desteği sunar:

- **Projeler**: Üst düzey organizasyon kapsayıcılarıdır. Bir proje oluşturulduğunda varsayılan olarak bir `production` ortamı oluşturulur; istenirse virgülle ayrılarak özel varsayılan ortamlar da tanımlanabilir.
- **Ortamlar**: Bir proje altındaki yalıtılmış dağıtım katmanlarıdır. Her ortam kendi uygulamalarını, veritabanlarını ve paylaşılan ortak ortam değişkenlerini barındırır.
- **Ortam Değişkenleri**: Bir ortam, ortak anahtar-değer yapılandırmaları ve gizli değişkenler (secret) tanımlayabilir. Bu ortamda çalışan tüm uygulamalar söz konusu değişkenleri otomatik olarak miras alır.
- **Miras Alma & Öncelik Sıralaması**: Dağıtım sırasında ortam değişkenleri katı bir öncelik sırasıyla birleştirilir (bkz. [Ortam değişkenleri](#ortam-degiskenleri)).
- **Bağımsız Uygulamalar**: Uygulamalar ve veritabanları projesiz (bağımsız, `project_id: null`) kalabilir veya oluşturulurken/güncellenirken herhangi bir proje ve ortama atanabilir.
- **WebUI Filtreleme**: Uygulamalar ve Veritabanları kontrol panelleri, kaynakları proje ve ortama göre filtrelemeyi ve mevcut katmanlarını rozetlerle göstermeyi destekler.

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

- `target` değeri, r3v3rs3 sunucusunun Docker Engine'i olan `local` veya bir [agent hedefinin](#agent-hedefleri) id'sidir. Bir uygulamanın hedefi ilk deployment'tan sonra değişmez: bir değişiklik `409 app_target_fixed` yanıtını alır.
- `domains` DNS adlarıdır. Wildcard veya IP adresi kabul edilmez, çünkü her alan adı ACME'den bir sertifika alır.
- `ports`, uygulamayı sunan portların adları veya id'leridir. Uygulama formundaki **Portlar** ve **Ek Portlar** alanları bunları seçer. `ports` değeri boşsa uygulama `proxy_ports` portlarını alır; o ayar da boşsa tüm HTTP ve HTTPS portlarını alır.
- `health_check_path`, uygulama hazır olduğunda `2xx` veya `3xx` döndüren bir HTTP yoludur. Bu değer yoksa deployment, port bir bağlantıyı kabul edip açık tutana kadar bekler.
- `volumes` adlandırılmış Docker volume'larını bağlar. `"read_only": true` volume'u salt okunur bağlar. Host üzerindeki bir yol bağlanamaz.
- `restart` değeri `no`, `on-failure`, `unless-stopped` (varsayılan) veya `always` olur.
- `auto_deploy` (isteğe bağlı, varsayılan `false`), container registry'de yeni bir image tag'i veya güncellenmiş digest tespit edildiğinde otomatik deployment'ı etkinleştirir.
- `bot_blocker` (isteğe bağlı, varsayılan `false`), uygulamanın proxy route'ları için otomatik yapay zeka kazıyıcı ve bot korumasını etkinleştirir.
- `grpc_web` (isteğe bağlı, varsayılan `false`), tarayıcı gRPC istemcileri için gRPC-Web dönüştürme desteğini etkinleştirir.

Uygulamanın, alan adlarının veya ortam değişkenlerinin değişikliği sonraki deployment'ta geçerli olur.

### Image Güncellemeleri

Image kaynağı kullanan uygulamalar için r3v3rs3, arka planda container registry'sini (Docker Hub, GitHub Container Registry vb.) periyodik olarak kontrol eder:
- **Registry Tag ve Digest Denetimi**: Mevcut tag için yeni manifest digest'larını veya semver olarak daha yeni sürüm tag'lerini kontrol eder.
- **Güncelleme Bildirimleri**: Bir güncelleme bulunduğunda, uygulama listesinde **Güncelleme mevcut** etiketi gösterilir. Uygulama detay sayfasında ise mevcut ve en son sürümleri içeren bir uyarı paneli ve **Güncellemeyi deploy et** butonu yer alır.
- **Otomatik Deploy**: **Güncellemede otomatik deploy** seçeneği açık olduğunda, r3v3rs3 yeni tag çıktığında uygulama tanımını günceller ve deployment'ı otomatik olarak başlatır.
- **Manuel Denetim**: WebUI üzerindeki **Güncellemeleri denetle** butonu veya `POST /api/apps/{id}/check_update` API çağrısı ile anında kontrol yapılabilir.

### Toplu İşlemler

Uygulama, port ve proxy listeleri çoklu seçim kontrollerini destekler:
- Satır onay kutularını kullanarak birden fazla öğeyi seçebilir veya başlık kutusuyla tüm görünür öğeleri işaretleyebilirsiniz.
- Seçilen öğeleri onay alarak toplu silebilirsiniz.
- Birden fazla seçili uygulamayı tek seferde deploy edebilirsiniz.

## App Store

App Store, r3v3rs3'ün tek tıkla kurabileceği uygulamaları listeler. Katalog, [r3v3rs3-apps](https://github.com/KilimcininKorOglu/r3v3rs3-apps) deposunun `app-store.json` dosyasıdır; binary ayrıca gömülü bir kopya taşır, böylece taze kurulum listeyle başlar, ve günde bir kez depodan yenilenir. Sayfadaki **Kataloğu yenile** butonu güncel kataloğu hemen indirir. API kataloğu `GET /api/store` ile okur, `POST /api/store/refresh` ile yeniler.

Bir **image girdisi** uygulamanın image'ını ve portunu taşır. **Kur** butonu o alanlar doldurulmuş app formunu açar; hedefi, alan adlarını ve portları seçip uygulamayı oluşturun. İlk deployment, uygulama oluşturulur oluşturulmaz başlar.

Bir **Compose girdisi**, r3v3rs3-apps deposunun `app-store-compose/` altındaki bir Compose dosyasına işaret eder; böylece kendi veritabanına ihtiyaç duyan bir uygulama da tek tıkla kurulur. Form Compose modunda açılır ve girdi, uygulamanın ihtiyaç duyduğu **Değişkenleri** listeler, örneğin bir veritabanı parolası: formda doldurun, değerler ilk deployment'tan önce uygulamanın ortam değişkenlerine yazılır. Compose app'i düz bir `docker compose` çalıştırmasıyla deploy olur; blue-green geçişi yoktur ve geri alma, eski bir commit'i yeniden deploy etmekle olur.

Katalog başka Compose kaynağı tutmaz: kendi Compose dosyanızı kuran girdi App Store'a değil, [Uygulamalar](#uygulamalar) sayfasına aittir.

### Kategoriler, Arama ve Ekran Görüntüleri

App Store, anlık arama ve kategori filtreleme özellikleri sunar:
- **Arama**: Katalog uygulamalarını ada, açıklamaya, konteyner imajına veya kategoriye göre anında filtreler.
- **Kategoriler**: Uygulamalar *İzleme*, *Güvenlik*, *Geliştirme*, *Üretkenlik*, *Otomasyon*, *Veritabanı*, *Dosyalar*, *Medya*, *Analitik*, *Sosyal* ve *Yönetim* gibi kategorilere ayrılmıştır. Kataloğu filtrelemek için herhangi bir kategori etiketine tıklayabilir veya tüm uygulamaları görmek için **Tümü** seçeneğini seçebilirsiniz.
- **Ekran Görüntüleri & Önizleme**: Katalog girdileri ekran görüntüsü önizlemeleri ve kategori rozetleri içerebilir; bu sayede kurulumdan önce uygulamanın arayüzü doğrudan incelenebilir.

### Katalog Güncellemeleri ve Tek Tıkla Yükseltme

App Store'dan kurulan bir uygulamanın katalogda daha yeni bir sürümü bulunduğunda (örneğin güncellenmiş bir imaj etiketi veya yeni bir Compose yapılandırması), r3v3rs3 uygulamalar listesinde ve uygulama detay sayfasında **Katalogda yeni sürüm var** rozeti gösterir. **Katalog Sürümüne Yükselt** butonuna tıklamak (veya `POST /api/apps/{id}/upgrade-catalog` çağırmak) uygulama yapılandırmasını en son katalog sürümüyle günceller ve hemen yeni sürümle deployment başlatır.

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
| `connection` | yok | Bir [Git sağlayıcı](#git-saglayicilari) bağlantısının id'si. Bağlantı repoyu clone eder ve webhook'unu kurar. |
| `tag_trigger` | yok | Push edildiğinde eşleşen etiketleri (tag) otomatik olarak deploy eden etiket kalıbı (örn. `v*`, `release-*`). |

Private bir repo, bir [Git sağlayıcı](#git-saglayicilari) bağlantısı veya uygulamanın erişim token'ını gerektirir. Erişim token'ı örnekleri: reponun içeriğini okuma izni olan bir GitHub fine-grained token'ı, `read_repository` kapsamı olan bir GitLab token'ı veya `read:repository` kapsamı olan bir Gitea token'ı. `PUT /api/apps/{id}/git_token`, `{"token": "..."}` gövdesiyle token'ı ayarlar. `DELETE /api/apps/{id}/git_token` token'ı siler. r3v3rs3 token'ı `platform.key` ile şifreler. Yönetim API'si token'ı hiçbir zaman döndürmez, uygulamada yalnız `"git_token_set": true` görünür. r3v3rs3 token'ı, clone sırasında `x-access-token` kullanıcı adıyla HTTP Basic kimlik doğrulaması olarak kullanır. Token URL'de veya komut satırında yer almaz, git'e bir ortam değişkeniyle ulaşır.

r3v3rs3'ü çalıştıran sunucuda `git` binary'si bulunmalıdır. r3v3rs3, `git`'i host'un yapılandırması ve hook'lar olmadan, yalnız HTTPS üzerinden çalıştırır. Build context'inde `.git` dizini yer almaz. Context'teki bir sembolik link link olarak kalır. Bu yüzden bir build, repo dışındaki bir dosyayı okuyamaz. Context en fazla 512 MiB olabilir.

Docker image'ı builder ile build eder. Aynı anda iki build çalışır, diğerleri sırada bekler. Builder, çok aşamalı bir Dockerfile'ın aşamalarını build cache'i olarak saklar. Bu yüzden aynı uygulamanın sonraki build'i daha hızlı biter. `docker image prune` bu cache'i siler.

#### BuildKit ve Derleme Önbellekleri

Git tabanlı uygulamalar derlenirken r3v3rs3, otomatik önbellek bağlama (cache mount) enjeksiyonu ile Docker Buildx / BuildKit derleyicisini destekler:

- **BuildKit ve Derleme Önbelleğini Etkinleştir**: Eski klasik Docker derleyicisi yerine `docker buildx build --load` komutunu çalıştırır.
- **Uzak BuildKit Derleyicisi**: Ağır derleme işlemlerini ana sunucudan dışarı aktarmak için isteğe bağlı olarak harici bir BuildKit daemon uç noktasına (`buildkit.remote_addr`, ör. `tcp://10.0.0.5:1234` veya `unix:///run/buildkit/buildkitd.sock`) bağlanabilir.
- **Otomatik Önbellek Bağlama Şablonları**: r3v3rs3, popüler programlama dilleri ve paket yöneticileri için Dockerfile'ı elle değiştirmeye gerek kalmadan `# syntax=docker/dockerfile:1` ve `--mount=type=cache` yönergelerini `RUN` adımlarına otomatik olarak enjekte eder:
  - `Cargo`: `/usr/local/cargo/registry` ve `/root/.cargo/registry` dizinlerini önbelleğe alır.
  - `Npm` / `Pnpm` / `Yarn`: `/root/.npm`, `/root/.pnpm-store` ve `/usr/local/share/.cache/yarn` dizinlerini önbelleğe alır.
  - `Go`: `/go/pkg/mod` ve `/root/.cache/go-build` dizinlerini önbelleğe alır.
  - `Pip`: `/root/.cache/pip` dizinini önbelleğe alır.
  - `Maven` / `Gradle`: `/root/.m2` ve `/root/.gradle` dizinlerini önbelleğe alır.
- **Özel Önbellek Bağlamaları**: Konteyner içinde satır başına özel hedef dizin yolları belirleyebilirsiniz.
- **Dış Önbellek Desteği**: Katmanların birden çok sunucu veya CI çalıştırmasında yeniden kullanılabilmesi için `cache_from` ve `cache_to` parametrelerini (ör. `type=registry,ref=myregistry/app:cache,mode=max` veya `type=local,src=...`) yapılandırabilirsiniz.

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
| `repository` | | Git kaynağındaki gibi bir `https://` URL'si. Private bir repoyu bağlantı veya uygulamanın Git token'ı clone eder. |
| `branch` | `main` | Bir branch veya tag. |
| `file` | `compose.yaml`, `compose.yml`, `docker-compose.yaml` ve `docker-compose.yml` dosyalarından ilk bulunan | Reponun köküne göre Compose dosyası. Dosyadaki göreli yollar dosyanın kendi dizinine göre çözülür. |
| `service` | | Alan adına gelen istekleri `port` üzerinde alan servis. Servis adı küçük harf, rakam ve `_.-` içerir. |
| `connection` | yok | Git kaynağındaki gibi bir [Git sağlayıcı](#git-saglayicilari) bağlantısının id'si. |
| `tag_trigger` | yok | Push edildiğinde eşleşen etiketleri (tag) otomatik olarak deploy eden etiket kalıbı (örn. `v*`, `release-*`). |

Sunucuda `git` binary'si ve Compose eklentisi olan `docker` binary'si bulunmalıdır. r3v3rs3, `docker compose` komutunu `r3v3rs3-<app id>` Compose projesiyle çalıştırır ve `service` servisi için şu ayarları yapan bir dosya ekler:

- `r3v3rs3-<app id>-<deployment id>` container adı ve platformun etiketleri,
- tek bir dışarı açılmış port: `127.0.0.1` üzerindeki boş bir porta `port`. Bu port, Compose dosyasındaki servisin `ports` değerinin yerini alır.
- uygulamanın ortam değişkenleri. Dosya yalnız key'leri içerir. `docker compose` değerleri kendi ortamından okur, bu yüzden hiçbir değer diske yazılmaz. Aynı değerler Compose dosyasındaki `${VARIABLE}` referanslarını da doldurur. PATH, HOME, DOCKER_CONFIG ve DOCKER_HOST adları `docker` binary'sine aittir. Bu adlardan birini kullanan uygulamanın deployment'ı başarısız olur.

Compose dosyasındaki başka bir servis port dışarı açamaz, çünkü dışarı açılmış bir port r3v3rs3'ü atlar. Böyle bir port içeren Compose dosyası, hiçbir servis başlamadan deployment'ı başarısız yapar. Hata mesajı servisin adını verir. Servisler birbirine projenin ağı üzerinden ulaşır.

Compose uygulaması `volumes`, `restart` ve `limits` ayarlarını Compose dosyasında yapar. Bu yüzden uygulama tanımı bu alanları kabul etmez. `service` servisi tek bir container olarak çalışır, çünkü bir container adı vardır.

Compose deployment'ı `service` servisini yeniden oluşturur: Docker yeni container başlamadan önce eskisini durdurur. Bu yüzden uygulama, yeni container sağlık kontrolünden geçene kadar yanıt vermez. Sağlık kontrolünden geçemeyen bir deployment yeni container'ı log'u için saklar. Uygulamanın sonraki deployment'a veya geri almaya kadar sağlıklı bir container'ı olmaz. Çalışan deployment'ın checkout'u yapılandırma dizinindeki `compose/<app id>/` altında kalır, çünkü Compose dosyasındaki bir bind mount onu okuyabilir.

r3v3rs3, Compose dosyasının diğer ayarlarını denetlemez. Bir Compose dosyası host üzerindeki bir yolu bağlayabilir, privileged container çalıştırabilir veya host'un ağını kullanabilir. Bu yüzden uygulama düzenleyebilen her hesap, bu yolla host'u ele geçirebilir. Edit iznini yalnız buna yetkisi olan hesaplara verin.

#### Compose Servis Düzeyinde Sağlık ve Loglar

Birden fazla servisten oluşan Compose uygulamalarında:
- **Servisler Kartı**: Uygulama detay sayfası, Compose projesindeki her bir servis konteynerini listeleyen bir **Servisler** kartı gösterir; konteyner adı, çalışma durumu (`running` veya `exited`), sağlık kontrolü durumu (`Sağlıklı`, `Başlatılıyor`, `Sağlıksız` veya `Yok`) ve durmuşsa çıkış kodunu içerir.
- **Servis Bazlı Loglar**: Uygulamanın **Log** sayfasında yer alan **Servis** seçici menüsü, ana trafik servisinin yanı sıra Compose projesindeki herhangi bir servisin loglarını ayrı ayrı izlemeyi sağlar. Hem statik log alımı (`GET /api/apps/{id}/logs?service=<isim>`) hem de canlı WebSocket log akışı (`GET /api/apps/{id}/logs/live?service=<isim>`) servis filtresini destekler.
- **API**: Servis konteynerlerinin listesi ve sağlık durumları doğrudan `GET /api/apps/{id}/services` üzerinden sorgulanabilir.

#### Görsel Compose Editörü ve Çevre Değişkeni Tamamlama

WebUI, çok servisli stack'leri yapılandırmayı sezgisel ve hatasız hale getiren görsel bir Docker Compose YAML editörü sunar:

- **Doğrudan (Inline) Compose YAML Desteği**: Uygulamalar harici bir Git deposuna ihtiyaç duymadan doğrudan panel üzerinden girilen Compose YAML (`AppSource::Compose` içindeki `compose_yaml`) ile veya geleneksel Git deposundaki dosya yolu üzerinden dağıtılabilir.
- **Gerçek Zamanlı YAML Sözdizimi Doğrulama**: `serde_yaml` ile güçlendirilen editör, yazım anında YAML yapısını anlık olarak denetler. Girintileme (indentation), anahtar eşleme veya sözdizimi hatalarında ilgili satır sol kenar numarasında kırmızıyla işaretlenir ve anlaşılır hata bildirimi gösterilir.
- **Servis Tespiti ve Tek Tıkla Yönlendirme**: Editör, `services:` altında tanımlanan tüm servisleri otomatik olarak tespit edip etiket olarak listeler. Herhangi bir servis etiketine tıklamak, o servisi dış alan adı trafiğini alan ana yönlendirme servisi olarak ayarlar.
- **Çevre Değişkeni ve Secret Tamamlama**: Uygulamaya, proje ortamına veya çevre gruplarına ait değişkenler ile şifreli gizli anahtarlar (`${DEGISKEN_ADI}`) hızlı ekleme butonları ve sözdizimi tamamlama ile doğrudan YAML içine yerleştirilebilir.
- **Hazır Mimari Şablonları**: Popüler dağıtım mimarileri için tek tıkla yüklenebilen başlangıç şablonları:
  - *Nginx Web Sunucusu* (port yönlendirmeli bağımsız web konteyneri).
  - *Node.js + PostgreSQL* (kalıcı veritabanına bağlı API servisi).
  - *Python + Redis* (önbellek ve kuyruk mimarisi).
  - *WordPress + MySQL* (veritabanı arka uçlu eksiksiz CMS).
- **Sözdizimi Önizleme**: Etkileşimli düzenleme modu ile anahtarları, değerleri ve değişken referanslarını renklendiren sözdizimi önizleme modu arasında geçiş yapılabilir.

### Ortam değişkenleri

`PUT /api/apps/{id}/env` bir uygulamanın ortam değişkenlerini değiştirir. `"secret": true` olan bir değişkenin değeri yönetim API'sinden bir daha çıkmaz: API değişkeni değeri olmadan döndürür, değersiz bir güncelleme de mevcut değeri korur. r3v3rs3 her değeri `platform.key` ile şifreler ve yalnız bir container başlarken açar.

#### Git Tabanlı Yapılandırma

Bir uygulama ayrıca deployment sırasında ortam değişkenlerini doğrudan bir Git deposundaki `.env` dosyasından yükleyebilir:

- Uygulama formunda **Yapılandırma Dosya Yolu** alanını belirtin (ör. `.env` veya `config/production.env`) veya `AppSpec` içinde `git_config` alanını yapılandırın.
- Git ve Compose uygulamalarında dosya varsayılan olarak uygulamanın kendi depo checkout'undan okunur. İsteğe bağlı olarak özel bir **Yapılandırma Deposu** ve **Yapılandırma Dalı** da belirtilebilir.
- Image uygulamalarında hem depo URL'si hem de dosya yolu belirtilir.
- Dosya; yorum satırları (`#`), tırnak işaretleri ve `export` önekleri dahil standart `.env` biçimini destekler.
- Panelden tanımlanan açık ortam değişkenleri, Git deposundan yüklenen değişkenleri geçersiz kılar; böylece panelde yönetilen gizli anahtarlar depo varsayılanlarına göre öncelik kazanır.

#### Çevre Değişkeni Grupları

Çevre değişkeni grupları, birden fazla uygulama tarafından paylaşılan yeniden kullanılabilir yapılandırma değişkenleri ve secret setlerini tek merkezden yönetmenizi sağlar:

- Grupları WebUI'da **Platform -> Çevre Grupları** altından veya `/api/env-groups` üzerinden yönetin.
- Her grup benzersiz bir ada, isteğe bağlı bir açıklamaya ve isteğe bağlı `"secret": true` gizlemesine sahip anahtar-değer değişkenlerine sahiptir.
- Uygulama formunda, uygulamaya bağlanacak ortam değişkeni gruplarını işaretleyin (`AppSpec` içindeki `env_groups`).
- **Öncelik Sırası**: Deployment sırasında ortam değişkenleri şu sırayla birleştirilir:
  1. Git deposundan yüklenen temel `.env` dosyası (`git_config` belirtildiğinde).
  2. Bağlı çevre değişkeni grupları (`env_groups` sırasına göre; sonraki gruplar öncekileri geçersiz kılar).
  3. Proje Ortamı düzeyinde tanımlanan paylaşılan ortam değişkenleri (`/api/projects/{project_id}/environments/{environment_id}/env`).
  4. Doğrudan uygulama üzerinde tanımlanan uygulamaya özel değişkenler (grup ve ortam tanımlarını geçersiz kılar).
  5. Bağlı veritabanı bağlantı değişkenleri (ör. `DATABASE_URL`).
- Çevre değişkeni gruplarındaki gizli değişkenler platform anahtarlarıyla şifrelenir ve anahtar rotasyonu (`POST /api/platform/key/rotate`) sırasında otomatik olarak yeniden mühürlenir.
- Aktif bir uygulamaya bağlı olan bir çevre değişkeni grubu silinemez.

## Veritabanları

**Veritabanları** sayfası bir motorun veritabanını tek tıkla kurar. Motorlar PostgreSQL 17 ve 18, MariaDB 11 ve 12, MySQL 9, MongoDB 8 ve Redis 8'dir; her birinin image tag'i bir sürüme sabitlenmiştir. Engine kataloğu, [r3v3rs3-apps](https://github.com/KilimcininKorOglu/r3v3rs3-apps) deposunun `engines.json` dosyasıdır; binary ayrıca gömülü bir kopya taşır ve günde bir kez depodan yenilenir. Sayfadaki **Kataloğu yenile** butonu güncel kataloğu hemen indirir; API ise `POST /api/databases/engines/refresh` ile yeniler.

**Veritabanı ekle** bir motor, bir sürüm, bir ad ve isteğe bağlı olarak bir `127.0.0.1` portu alır. İsteğe bağlı **Bellek Sınırı (MB)**, **Swap Sınırı (MB)** ve **CPU Sınırı** alanları kurulum anında container kaynak sınırlandırmalarını (throttling) belirlemeyi sağlar. Port boş bırakılırsa sunucu oluşturma anında boş bir port alır ve veritabanı o portu sonradan da korur: bir veritabanının her deployment'ı portunu aynı adrese yayınlar; bir uygulamadan farklı olarak adres değişmez. Oluşturma, local target'ta sürümün image'ıyla, motorun verileri için bir named volume ile ve 32 hex karakterli üretilmiş bir parolayı secret ortam değişkeni olarak taşıyan bir app yaratır. İlk deployment hemen başlar.

Yanıt, parolayı ve bağlantı dizesini webhook secret gibi bir kez gösterir. Panel her veritabanını motoru, portu ve parolasız bağlantısıyla listeler. Veritabanı r3v3rs3 sunucusunun `127.0.0.1` adresini dinler; yalnız bu sunucudaki uygulamalar ve sunucunun kendi programları ulaşır: bir veritabanının alan adı ve proxy'si yoktur ve satırındaki **Sil** app'i kaldırırken verilerin olduğu volume kalır. Redis parola almaz, bu yüzden portu yerel her istemciye yanıt verir.

Bir veritabanının deployment'ı blue-green değildir, çünkü sabit port eski container durmadan boşalmaz: r3v3rs3 eski container'ı durdurur, yenisini başlatır ve sağlık kontrolünü bekler; veritabanı birkaç saniye sonra yanıtlamaya devam eder.

### Bir uygulamayı bağlamak

App formundaki **Veritabanı** alanı, bir uygulamayı bu sunucudaki bir veritabanına bağlar. Uygulamanın sonraki deployment'ı bağlantıyı container'ının ortamına ekler: kullanıcıyı, üretilmiş parolayı ve portu taşıyan `DATABASE_URL`, artı motorun değişkenleri; örneğin PostgreSQL için `POSTGRES_HOST`, `POSTGRES_PORT`, `POSTGRES_USER`, `POSTGRES_PASSWORD` ve `POSTGRES_DATABASE`, MariaDB ve MySQL için `MYSQL_*`, MongoDB için `MONGO_*`, Redis için `REDIS_URL`. Container, `host.docker.internal` adresini r3v3rs3'ün hostuna çözer; adres her app ağında çalışır.

Enjekte edilen kullanıcı veritabanının root kullanıcısıdır; r3v3rs3 app başına kullanıcı yaratmaz. Veritabanı çalışmadığı sürece bağlı bir uygulamanın deployment'ı nedeniyle başarısız olur ve eski container karşılamaya devam eder. Bir uygulamanın kullandığı veritabanı silinemez: silme, uygulamalar bağlantılarını kesene kadar `409 database_in_use` yanıtını alır ve veritabanları sayfası her veritabanını kullanan uygulamaları gösterir.

Veritabanı local target ister ve sayfayı yalnız Edit izni olan bir hesap görür. Her oluşturma ve silme audit log'una geçer.

### Veritabanı Klonlama ve Branching

Veritabanı satırındaki **Klonla** butonu, mevcut bir veritabanının bağımsız yeni bir kopyasını (branch) oluşturur:

- **Adlandırma ve Kurulum**: Yeni veritabanı adını (varsayılan `{name}-clone`) ve isteğe bağlı host portunu sorarak r3v3rs3, kendi özel volume'ü (`{name}-data`), benzersiz yeni parolası ve `127.0.0.1` üzerinde ayrılan portuyla yeni bir veritabanı uygulaması oluşturur.
- **Canlı Dump veya Yedekten Geri Yükleme**: Kaynak veritabanı çalışıyorsa r3v3rs3 doğrudan çalışan container'dan `exec` ile anlık canlı dump alır. Kaynak veritabanı durdurulmuşsa otomatik olarak en son doğrulanmış yedek dump'ını kullanır.
- **Otomatik Geri Yükleme**: Yeni veritabanı container'ı başlatılıp sağlıklı duruma geldikten sonra, dump verisi otomatik olarak klonlanan yeni veritabanına aktarılır.
- **İzole Test ve Staging**: Klonlanan veritabanı tamamen bağımsız bir volume, port ve kimlik bilgilerine sahip olduğundan geliştiricilerin canlı veriyi etkilemeden migrasyonları test etmesine, staging ortamı kurmasına veya hata ayıklamasına olanak tanır.

### Veritabanı Read Replica & Okuma Yük Dengeleme

Yüksek okuma trafiği ve okuma/yazma ayrımı (read/write splitting) için her veritabanı satırında **Replica Oluştur** işlemi bulunur:

- **Tek Tıkla Kurulum**: Yeni replica adını (varsayılan `{name}-replica`) soran r3v3rs3, birincil (Primary) veritabanına bağlı (`replica_of`) yeni bir kopya veritabanı oluşturur. Birincil veritabanının kimlik doğrulama parolasını otomatik olarak kopyaya eşitler ve replica container'ını ayağa kaldırır.
- **Ayrılmış Okuma Uç Noktası**: Her replica kendi bağımsız host portu ve volume'ü üzerinde çalışarak raporlama ve yoğun okuma sorgularının yazma trafiğini yavaşlatmasını önler.
- **Otomatik Çoklu Replica Ortam Değişkeni Enjeksiyonu**: Bir uygulama, aktif read replica'ları bulunan bir birincil veritabanına bağlandığında, r3v3rs3 container ortamına şu değişkenleri otomatik olarak ekler:
  - `DATABASE_URL`: Yazma işlemleri için birincil veritabanı bağlantı adresi.
  - `DATABASE_READ_URL` ve `DATABASE_REPLICA_URL`: Aktif read replica'yı hedefleyen bağlantı adresi.
  - `DATABASE_READ_URLS`: Aktif tüm replica bağlantı adreslerinin virgülle ayrılmış listesi; çoklu sunucu destekleyen bağlantı havuzlarının (JDBC, Prisma, pgpool, typeorm vb.) okuma trafiğini tüm replikalar arasında dağıtmasını sağlar.
  - Motora özel read değişkenleri (ör. `POSTGRES_READ_HOST`, `POSTGRES_READ_PORT`, `MYSQL_READ_HOST` vb.).

### Veritabanı Web Yönetim Arayüzleri

Görsel veri inceleme, sorgulama ve şema yönetimi için r3v3rs3, **App Store** bünyesinde resmi tek tıkla kurulabilir veritabanı yönetim arayüzleri sunar:
- **pgAdmin 4**: PostgreSQL için zengin özellikli yönetim ve SQL geliştirme platformu.
- **phpMyAdmin**: MySQL ve MariaDB veritabanları için web yönetim arayüzü.
- **Redis Commander**: Redis örnekleri için hızlı web tabanlı anahtar-değer yönetim aracı.
- **Adminer**: Tek container'da çoklu motor destekleyen (PostgreSQL, MySQL, SQLite, MongoDB) hafif veritabanı yöneticisi.
- **Mongo Express**: MongoDB için etkileşimli döküman ve koleksiyon yönetim paneli.

**Veritabanları** sayfasında her veritabanı satırı, ilgili veritabanı motoruna uygun App Store şablonunu doğrudan açan bir **Web Arayüzü** kısayol bağlantısı içerir.

### Yedekler

Veritabanı satırındaki **Yedekler** bağlantısı o veritabanının yedeklerini açar. **Şimdi yedekle**, engine'in dump'ını container'ın içinde çalıştırır (`pg_dump`, `mariadb-dump`, `mysqldump` veya `mongodump`; Redis `SAVE` çalıştırır ve dump dosyasını alır), gzip'ler ve config dizinindeki `backups` dizinine kaydeder; veritabanı başına bir klasör, dosyanın SHA-256'sını taşıyan bir JSON manifest ile birlikte. Bir yedek parola içermez. Agent target üzerindeki bir veritabanı yedek alamaz; çünkü dump container'ın içinde çalışır.

Aynı sayfadaki yedekleme ayarları, her veritabanının yedeklendiği UTC'deki günlük saati, veritabanı başına kalacak yedek sayısını (en eskiler önce gider), uzak depolama hedeflerini ve nabız (heartbeat) izleme kontrolünü yapılandırır:

- **S3**: Bucket'ı yolunda taşıyan bir `http` veya `https` endpoint, bir region, bir key prefix, bir access key ve secret key.
- **SFTP**: `host`, `port` (varsayılan 22), `user`, hedef `path` ve isteğe bağlı SSH private key (veya parola) ile aktarım.
- **rsync**: SSH üzerinden uzak hedef dizine `rsync` ile yedek aktarımı.
- **Dead Man's Snitch / Heartbeat İzleme**: İsteğe bağlı bir sinyal (ping) URL'si (örn. Healthchecks.io, Dead Man's Snitch veya Better Stack). Günlük planlı yedekler başarıyla tamamlandığında r3v3rs3 bu adrese `r3v3rs3-heartbeat` User-Agent başlığıyla bir HTTP GET çağrısı yapar. Herhangi bir planlı yedekleme başarısız olursa bu ping atlanarak yerine `backup.failure` giden webhook bildirimi gönderilir; böylece harici servisler sessiz yedekleme arızalarını anında fark eder.

Her yeni yedek yapılandırılan uzak hedeflere aktarılır; başarısız bir yükleme yalnız sunucu log'una geçer, çünkü yerel kopya vardır. Uzak kimlik bilgileri `platform.key` ile şifrelenir.

#### Geri Yükleme & Periyodik Doğrulama Testi

**Geri yükle**, kayıtlı dump'ı çalışan container'a yazar ve engine'in restore komutunu çalıştırır (`psql`, `mariadb`, `mysql`, `mongorestore --drop`). Redis dump dosyasını data volume'üne alır ve yeniden başlar; bu yüzden restore, sunucu geri geldikten sonra tamamlanır. Geri yükleme veritabanının verisinin yerine geçer.

Yedeklerin bozulmadan saklandığını ve geri yüklenebilir olduğunu garanti etmek için r3v3rs3 otomatik periyodik geri yükleme doğrulama testi içerir. Sistem gzipped dump'ların hatasız açıldığını ve ilgili veritabanı motorunun sihirli baytlarıyla veya SQL başlıklarıyla (Redis RDB, SQLite 3, PostgreSQL, MySQL/MariaDB) eşleştiğini doğrular. Doğrulanan yedeklerde onay zaman damgası (`verified_at`) yer alır; ayrıca WebUI üzerinden veya `POST /api/databases/{id}/backups/{name}/verify` ile anında test başlatılabilir.

#### Zaman Noktasına Geri Dönüş (PITR) ve Sürekli Günlük Arşivleme

Gecelik veya periyodik tam dump'ların ötesinde r3v3rs3, sürekli Write-Ahead Logging (WAL) ve ikili günlük (binary log) arşivleme ile Zaman Noktasına Geri Dönüş (Point-in-Time Recovery - PITR) yeteneği sunar:

- **Kurtarma Penceresi (Recovery Window)**: Yedekler sayfasında en eski temel (base) yedekten en son kaydedilen değişiklik günlüğü segmentine kadar olan etkin zaman aralığı, toplam temel yedek ve arşivlenmiş WAL sayısı net bir özetle gösterilir.
- **WAL ve Günlük Arşivleme**: Dönen işlem günlükleri `<config-dir>/backups/<db_id>/wal/` dizininde arşivlenir. Kullanıcılar **WAL Günlüğünü Arşivle** butonuyla anında manuel arşiv tetikleyebilir veya arka plan kontrol noktalarını kullanabilir (`POST /api/databases/{id}/wal/archive`):
  - **PostgreSQL**: İşlem günlüğü rotasyonu (`pg_switch_wal()`) ve sürekli WAL yakalamasıyla segmentleri arşivler.
  - **MySQL / MariaDB**: Döndürülen ikili günlükleri (`FLUSH BINARY LOGS;`) arşivler.
  - **MongoDB**: Oplog mutasyon arşivlerini (`mongodump --oplog`) depolar.
  - **Redis**: Anlık durum görüntüsünü ve Yalnızca-Ekleme Günlüğü (AOF) kayıtlarını yakalar.
- **Hassas Dakikaya Geri Dönüş**: **Zaman Noktasına Geri Yükle (PITR)** işlemi hedeflenen kesin zaman damgasını sorar (RFC3339 biçimi, örn. `2026-10-21T04:30:00Z` veya unix milisaniyesi). r3v3rs3 otomatik olarak:
  1. Hedef zamandan önceki en güncel temel yedeği belirler ve geri yükler.
  2. Temel yedek ile hedef an arasındaki tüm arşivlenmiş WAL/günlük segmentlerini sıralı biçimde seçer.
  3. İşlemleri istenen saniyeye veya milisaniyeye kadar artımlı olarak yeniden uygular (replay).
- **Güvenli Dal Oluşturma (PITR Clone)**: İsteğe bağlı olarak yeni bir dal (branch) adı girilirse, r3v3rs3 mevcut canlı veritabanını bozmadan tamamen ayrı bir host portu, data volume'ü ve yetkileriyle yeni bir veritabanı kurar. Böylece geliştiriciler canlı ortamı kesintiye uğratmadan geçmiş veriyi inceleyebilir veya silinen tabloları kurtarabilir. Dal adı boş bırakıldığında onay ile mevcut veritabanı üzerinde geri yükleme yapılır.

#### Uzak S3 Yedek Gezgini & Felaket Kurtarma (Disaster Recovery)

Yerel depolama alanı kaybolduğunda, diskler arızalandığında veya sunucu sıfırdan kurulduğunda r3v3rs3; AWS S3, Cloudflare R2 veya MinIO uzak deposundaki snapshot'ları doğrudan keşfetme, indirme ve geri yükleme imkanı sunar:

- **S3 Snapshot Gezgini**: WebUI üzerindeki **S3 / Uzak Yedekler** paneli veya `GET /api/databases/s3/backups` uç noktasıyla uzak bucket'ta arşivlenen tüm veritabanı yedeklerini canlı olarak listeleyin. Gezgin; kaynak veritabanı adı, yedek kimliği, dosya boyutu ve zaman damgası bilgileriyle anlık arama/filtreleme sağlar.
- **Doğrudan Snapshot İndirme**: Uzak depodaki herhangi bir veritabanı dump arşivini `GET /api/databases/s3/download?key={key}` ile doğrudan yerel bilgisayarınıza indirin.
- **Mevcut Veritabanına S3'ten Geri Yükleme**: Çalışan bir veritabanını tek tıkla S3'teki herhangi bir geçmiş snapshot'a döndürün (`POST /api/databases/s3/restore` ve `target_db_id`).
- **Felaket Kurtarma Sihirbazı (Disaster Recovery)**: Sunucu tamamen çöktüğünde veya sıfırdan yeni bir sunucu kurulduğunda, S3 snapshot'ından tek tıkla yepyeni ve çalışan bir veritabanı ayağa kaldırın (`POST /api/databases/s3/restore`, `new_database_name` ve `engine`). r3v3rs3 konteyner ortamını hazırlar, kalıcı volume'ü bağlar, uzak yedeği indirip açarak arkaplanda ilgili veritabanı motoruna otomatik olarak geri yükler.

## Deployment'lar

`POST /api/apps/{id}/deploy` bir deployment başlatır ve onu `queued` durumuyla döndürür. `GET /api/deployments/{id}` ilerlemeyi gösterir.

1. r3v3rs3 image'ı çeker ve özetini kaydeder. Git kaynağında branch'i clone eder, commit'i kaydeder, image'ı `r3v3rs3/<app id>:<deployment id>` adıyla build eder ve image id'sini kaydeder. Compose kaynağında branch'i clone eder, commit'i kaydeder, `docker compose up --build` çalıştırır ve 3. adımla devam eder.
2. `r3v3rs3-<app id>-<deployment id>` container'ını `r3v3rs3-<app id>` ağında oluşturur. Böylece farklı uygulamaların container'ları birbirine ulaşamaz. Container portunu `127.0.0.1` üzerindeki boş bir porttan dışarı açar. Böylece container'a yalnız host üzerindeki r3v3rs3 ulaşır.
3. Sağlık kontrolü için en fazla 120 saniye bekler. Duran veya kontrolden geçemeyen bir container deployment'ı `failed` ile bitirir. Hata mesajı container log'unun son satırlarını içerir. Eski container istekleri karşılamaya devam eder.
4. Deployment'ı `running`, öncekini `superseded` olarak işaretler ve alan adlarını yeni container'a yönlendirir.
5. 10 saniye sonra eski container'ı durdurur ve siler. Bu süre, eski container'daki açık isteklerin bitmesi içindir.

Bir uygulama aynı anda tek bir deployment çalıştırır. Bir deployment sürerken gelen ikinci deploy, geri alma veya silme isteği `409 app_busy` alır. Bir deployment sürerken gelen [webhook](#webhook-lar) push'u ise bir deployment daha kuyruğa alır. Sunucu yeniden başlarsa bitmemiş deployment `the server stopped during the deployment` mesajıyla `failed` olur ve `deployment_failed` bildirimi gönderilir.

| Durum | Anlamı |
|---|---|
| `pending_approval` | Dağıtım manuel onay veya bakım penceresi bekliyor. |
| `queued` | Deployment başlamayı bekliyor. |
| `building` | r3v3rs3 branch'i clone ediyor ve image'ı build ediyor veya Compose dosyasını okuyor. |
| `deploying` | Image çekiliyor, `docker compose up` çalışıyor veya yeni container sağlık kontrolünü bekliyor. |
| `running` | Deployment'ın container'ı uygulamayı sunuyor. |
| `superseded` | Daha yeni bir deployment bunun yerini aldı. |
| `failed` | Deployment durdu. Nedeni `message` alanındadır. |
| `cancelled` | Dağıtım reddedildi veya iptal edildi. Nedeni `message` alanındadır. |

### Geri alma

`POST /api/deployments/{id}/rollback`, önceki bir deployment'ın image özeti, uygulama ayarları ve ortam değişkenleriyle yeni bir deployment başlatır. Image'ı tag değil özet seçer. Bu yüzden tag değişse bile geri alma aynı image'ı çalıştırır. Image'ını çekmeden önce başarısız olan bir deployment'ın özeti yoktur. Bu deployment için geri alma isteği `400 rollback_unavailable` alır.

Bir Git uygulamasını geri alma build yapmaz, build edilmiş image'ı yeniden başlatır. r3v3rs3, bir uygulamanın en yeni 5 deployment'ının ve çalışan deployment'ının image'larını saklar, daha eskilerini siler. Image'ı silinmiş bir deployment'a geri alma `the image ... is no longer present` mesajıyla başarısız olur.

Bir Compose uygulamasını geri alma, önceki deployment'ın kaydedilmiş commit'ini clone eder ve `docker compose up --build` komutunu o deployment'ın uygulama ayarları ve ortam değişkenleriyle yeniden çalıştırır. Commit'ini kaydetmeden önce başarısız olan bir Compose deployment'ı için geri alma isteği `400 rollback_unavailable` alır.

### Dağıtım onayı ve dağıtım pencereleri

Bir uygulama manuel onay gerektirebilir veya dağıtımları planlı bir bakım penceresiyle sınırlandırabilir:
- `require_approval` (`bool`): `true` olduğunda, her yeni dağıtım (manuel, webhook, otomatik dağıtım veya geri alma) `pending_approval` durumuyla başlar ve `Edit` yetkisine sahip bir kullanıcı onaylayana kadar bekler.
- `deploy_window` (`Option<String>`): `HH:MM-HH:MM` formatında izin verilen UTC saat aralığı (ör. `02:00-05:00`). Tanımlandığında, bu aralık dışında tetiklenen dağıtımlar `pending_approval` durumuna geçer. `require_approval` `false` ise, UTC saati pencereye girdiğinde arka plan zamanlayıcısı dağıtımı otomatik olarak onaylar ve başlatır.

`POST /api/deployments/{id}/approve` bekleyen bir dağıtımı onaylar ve sürecini başlatır.
`POST /api/deployments/{id}/reject` bekleyen dağıtımı reddederek durumunu `cancelled` yapar.

## Webhook'lar

Webhook, Git sağlayıcısı bir push olayı gönderdiğinde uygulamayı deploy eder. `POST /api/apps/{id}/webhook_secret` uygulamanın secret'ını oluşturur ve onu bir kez döndürür. Secret 64 hex karakterdir ve r3v3rs3 onu `platform.key` ile şifreler. İkinci bir çağrı yeni bir secret oluşturur ve eski secret hemen çalışmaz olur. `DELETE /api/apps/{id}/webhook_secret` webhook'u kapatır.

Sağlayıcı olaylarını yönetim API'sinin adresindeki `POST /hooks/apps/{id}` route'una gönderir. Bu route oturum istemez, çünkü her isteği kendi imzası doğrular. Yönetim API'si varsayılan olarak `127.0.0.1` üzerinde dinler, bu yüzden sağlayıcının API'ye ulaşacak bir yolu olmalıdır: herkese açık bir alan adının `/hooks/` yol önekini yönetim portuna gönderen bir proxy ekleyin ve yönetim API'sinin diğer yollarını kapalı tutun.

| Sağlayıcı | Ayar |
|---|---|
| GitHub / GHCR | **Payload URL** hook adresidir, **Content type** `application/json` değeridir, **Secret** secret'tır. GitHub gövdeyi `X-Hub-Signature-256` header'ında imzalar. Git push, Pull Request olaylarını ve GitHub Container Registry (`package` / `registry_package`) yayınlama olaylarını destekler. |
| Gitea, Forgejo | **Target URL** hook adresidir, **POST Content Type** `application/json` değeridir, **Secret** secret'tır. Sağlayıcı gövdeyi `X-Gitea-Signature` veya `X-Forgejo-Signature` header'ında imzalar. |
| GitLab | **URL** hook adresidir, **Secret token** secret'tır. GitLab secret'ın kendisini `X-Gitlab-Token` header'ında gönderir. **Push events** seçeneğini açın. Uygulamanın branch alanı bir tag adıysa **Tag push events** seçeneğini de açın. Merge Request olaylarını destekler. |
| Docker Hub | Webhook URL'i `<hook adresi>?secret=<secret>` şeklindedir. Docker Hub `push_data` ve `repository` içeren JSON gövdesi gönderir. |
| Harbor | Webhook URL'i hook adresidir ve `Authorization: Bearer <secret>` yetkilendirme header'ı kullanılır. Harbor kaynak etiketleri ve özetleriyle `PUSH_ARTIFACT` olayları gönderir. |
| Quay | Webhook URL'i `<hook adresi>?secret=<secret>` şeklindedir veya `X-Webhook-Secret: <secret>` header'ı gönderilir. Quay `updated_tags` içeren bildirimler gönderir. |
| Başka bir gönderici | Gövdeyi secret ile HMAC-SHA256 kullanarak imzalayın (`X-Signature-256: sha256=<hex>`) veya secret belirtecini `Authorization: Bearer <secret>`, `X-Webhook-Token: <secret>` ya da `?secret=<secret>` sorgu parametresiyle iletin. Bir CI işi veya özel registry bu yöntemle sıfır kesintili blue-green deployment tetikleyebilir. |

Hangi istek deploy eder:

- Git veya Compose uygulaması, `ref` değeri `refs/heads/<branch>` veya `refs/tags/<branch>` olan bir push ile deploy olur. Burada `<branch>` uygulamanın branch alanıdır. Branch'i silen bir push deploy etmez.
- `tag_trigger` tanımlandığında (örneğin `v*` veya `release-*`), bu kalıba uyan bir etiket (tag) push edildiğinde otomatik olarak o etiket deploy edilir.
- Geçici önizleme ortamları etkinleştirildiğinde (`preview`), Pull Request / Merge Request olayları izole bir önizleme uygulamasını (`<uygulama>-pr-<numara>`) otomatik olarak yönetir: `opened` veya `reopened` önizleme uygulamasını oluşturur ve ona özel bir alan adında deploy eder (`preview_deployed`), `synchronize` yeni commit'leri deploy eder ve `closed` önizleme uygulamasını yok ederek tüm kaynakları serbest bırakır (`preview_destroyed`).
- Image uygulaması, bir container registry (Docker Hub, GHCR, Harbor, Quay) veya genel webhook uygulamanın deposuna yeni bir push bildirdiğinde otomatik olarak sıfır kesintiyle (blue-green) güncellenir ve deploy edilir. Gönderilen etiket mevcut etiketle eşleşirse (örneğin `:latest`), yeni digest çekilerek sıfır kesintiyle devreye alınır. Eğer `tag_trigger` tanımlıysa (örneğin `v*` veya `release-*`) ve gelen etiket bu kalıba uyuyorsa, uygulamanın image referansı yeni etiketle güncellenir ve deploy edilir. Farklı bir depoya veya eşleşmeyen bir etikete ait push'lar güvenle yoksayılır (`ignored`).
- Başka bir göndericinin isteği gövdesi ne olursa olsun her zaman deploy eder.
- Ping, başka bir branch'e push veya başka bir olay `200` ve `{"outcome": "ignored"}` yanıtını alır.

Deploy eden istek `200` ve `{"outcome": "deployed", "deployment": {...}}` (veya pull request'ler için `{"outcome": "preview_deployed", "deployment": {...}}`) yanıtını alır. Kapatılan bir pull request `200` ve `{"outcome": "preview_destroyed"}` yanıtını alır. Çalışan bir deployment sırasında gelen push `202` ve `{"outcome": "queued"}` yanıtını alır. Çalışan deployment bitince branch'in en son commit'iyle bir deployment daha başlar. Böylece kuyruktaki tek deployment aradaki bütün push'ları kapsar. r3v3rs3 kuyruğu bellekte tutar, bu yüzden yeniden başlatma kuyruğu siler.

| Durum kodu | Nedeni |
|---|---|
| `400 invalid_webhook_payload` | GitHub, Gitea veya GitLab push'unun gövdesi JSON değildir, örneğin içerik türü `application/x-www-form-urlencoded` olduğunda. |
| `401 unauthorized` | İmza yoktur veya yanlıştır. |
| `404 id_not_found` | Bu id ile bir uygulama yoktur veya uygulamanın webhook secret'ı yoktur. |
| `429` | İstemci art arda 10'dan fazla istek, bundan sonra da saniyede birden fazla istek gönderdi. `POST /hooks/apps/{id}` ve `GET /oauth/git/callback` bu limiti her istemci adresi için paylaşır. |
| `503 platform_disabled` | Deploy platformu kapalıdır. |

Gövde en fazla 5 MiB olabilir. Denetim kaydı, bir deployment başlatan veya kuyruğa alan her isteği göndericinin adresiyle ve hesapsız olarak kaydeder.

### Geçici Önizleme Ortamları (Pull Request'ler)

Git ve Compose uygulamaları için r3v3rs3, her pull request veya merge request için izole geçici önizleme ortamlarını otomatik olarak dağıtabilir.

- **WebUI'de Açma**: Uygulama ayarları formunda **Geçici Pull Request Önizleme Ortamları** seçeneğini işaretleyin.
- **Alan Adı Şablonu**: `{pr}` ve `{app}` yer tutucularını kullanarak isteğe bağlı bir alan adı kalıbı belirleyin (örn. `pr-{pr}.app.example.com`). Boş bırakıldığında r3v3rs3 uygulamanın birincil alan adını önekler (`pr-{pr}.<alan_adi>`) veya `pr-{pr}-{app}.localhost` kullanır.
- **Ortam Değişkenleri & Secret'lar**: Önizleme ortamları ana uygulamanın tüm ortam değişkenlerini, şifrelenmiş secret'larını ve Git erişim token'larını otomatik olarak devralır. Host portları ana uygulama veya diğer önizlemelerle port çakışmasını önlemek için dinamik olarak serbest bırakılır.
- **Otomatik Yaşam Döngüsü**: Pull Request açıldığında veya yeniden açıldığında r3v3rs3 önizleme uygulamasını oluşturur ve trafiği ona yönlendirir. Pull request'e gönderilen yeni commit'ler güncellemeleri tetikler (`synchronize`). Pull Request kapatıldığında veya merge edildiğinde r3v3rs3 önizleme uygulamasını, konteynerlerini, proxy route'larını ve volume'lerini tamamen temizler.

## Git sağlayıcıları

Git sağlayıcı bağlantısı, r3v3rs3'ün bir GitHub, GitLab veya Gitea hesabı adına çalışmasını sağlar. Bir admin bağlantıyı bir kez bağladıktan sonra uygulama formu o hesabın repository'lerini ve branch'lerini listeler. Deployment private bir repoyu bağlantının token'ıyla clone eder ve r3v3rs3 uygulamanın push webhook'unu repoya kurar. Bağlantılar sunucuya aittir: bir admin onları ekler, Edit izni olan her hesap onları uygulamalarında kullanır. Bağlantısı olmayan uygulama, repo adresi ve kendi Git token'ıyla çalışmaya devam eder.

GitHub bağlantısı, r3v3rs3'ün sizin için oluşturduğu bir GitHub App'tir. GitLab ve Gitea bağlantısı ise sağlayıcıda kaydettiğiniz bir OAuth uygulamasıdır.

### GitHub App oluşturma

**Git Sağlayıcıları** sayfasında sağlayıcı olarak GitHub'ı seçin ve bağlantıya bir ad verin. **Adres** alanını github.com için boş bırakın. GitHub Enterprise Server için sunucunun adresini girin. **Organizasyon** alanı, uygulamanın sahibi olacak GitHub organizasyonunu alır. Alanı boş bırakırsanız uygulama kendi hesabınızda oluşur.

**GitHub App oluştur** bir manifest'i GitHub'a gönderir. GitHub, ayarları doldurulmuş bir GitHub App oluşturma sayfası açar:

- Ad `r3v3rs3-<bağlantı adı>` olur ve 34 karakterde kesilir. GitHub'da her uygulama adı tek olmalıdır. Ad alınmışsa GitHub'da değiştirin.
- Uygulama private'tır, yani yalnız sahibinin hesabına kurulabilir.
- İzinler: Contents okuma, Metadata okuma ve repository webhook'larını yazma.
- Uygulamanın kendi webhook'u yoktur. r3v3rs3 her uygulamanın webhook'unu reposuna ayrıca kurar.

Uygulamayı GitHub'da oluşturduğunuzda GitHub tarayıcıyı callback adresine (`/oauth/git/callback`) bir kodla gönderir. r3v3rs3 kodu uygulamanın client id'si, client secret'ı ve private key'iyle değiştirir ve bağlantıyı kaydeder. Tarayıcı ardından uygulamanın kurulum sayfasına gider. State bir kez ve 10 dakika boyunca geçerlidir. Callback oturum istemez, çünkü oturum cookie'si başka bir siteden gelen yönlendirmeyle gönderilmez.

Kurulum sayfasında hesabı veya organizasyonu ve repository'leri seçin. GitHub tarayıcıyı **Git Sağlayıcıları** sayfasına döndürür. Sayfa uygulamanın kurulumunu GitHub'dan okur ve bağlantı **Bağlı** durumuna geçer. **Hesap** sütunu kurulumun hesabını gösterir. Kurulmamış bir bağlantıda **GitHub'da kur** kurulum sayfasını açar. **Adres** sütunundaki link uygulamanın GitHub sayfasını açar. Repo seçimini orada değiştirebilirsiniz.

GitHub, callback ve dönüş adreslerini uygulama oluştuğu andaki WebUI adresiyle saklar. WebUI adresi değişirse bu adresleri uygulamanın GitHub ayarlarında değiştirin.

r3v3rs3, client secret'ı ve private key'i `platform.key` ile şifreler. Yönetim API'si onları hiçbir zaman döndürmez. Her GitHub isteği için r3v3rs3 private key ile imzalanmış bir JWT'yle bir installation token'ı alır. Token bir saat geçerlidir ve r3v3rs3 onu süresi dolmadan yeniler. Uygulama GitHub'da kaldırılırsa bağlantı **Yeniden bağlanın** durumuna geçer. **Yeniden bağlan** kurulum sayfasını açar. Bir GitHub App bağlantısında yalnız ad değişir.

### GitLab ve Gitea

Callback adresi, WebUI adresinin sonuna `/oauth/git/callback` eklenerek oluşur, örneğin `https://r3v3rs3.example.com/oauth/git/callback`. **Git Sağlayıcıları** sayfası bu adresi kullandığınız sayfaya göre gösterir. Sağlayıcı admin'in tarayıcısını bu adrese geri gönderir. Bu yüzden adres, genel adres değil, admin'in açtığı adres olmalıdır.

| Sağlayıcı | Yer | Ayarlar |
|---|---|---|
| GitLab | **Preferences**, **Applications**, veya bir grubun ya da sunucunun **Applications** sayfası | **Redirect URI** callback adresidir. `api` scope'unu seçin ve **Confidential** seçeneğini açık bırakın. |
| Gitea | **Settings**, **Applications**, **Manage OAuth2 Applications** | **Redirect URI** callback adresidir. **Confidential Client** seçeneğini açık bırakın. r3v3rs3 `read:repository`, `write:repository` ve `read:user` scope'larını ister. |

Sağlayıcı bir client id ve bir client secret gösterir. İkisini de **Git Sağlayıcıları** sayfasında ekleyin. GitLab bağlantısı, kendi GitLab'ınızın adresini girmezseniz `https://gitlab.com` kullanır. Gitea bağlantısı sunucusunun adresini ister. Sağlayıcı adresi `https://` kullanır. `http://` yalnız bir loopback adresinde kabul edilir.

### Bağlanma

Bir GitLab veya Gitea bağlantısının satırındaki **Bağlan** sağlayıcının yetkilendirme sayfasını açar. Hesap erişime izin verince sağlayıcı tarayıcıyı callback adresine geri gönderir ve r3v3rs3 kodu hesabın token'larıyla değiştirir. Yetkilendirme PKCE kullanır. State bir kez ve 10 dakika boyunca geçerlidir. Sayfa **Bağlı** durumunu ve hesabın adını gösterir.

r3v3rs3, client secret'ı ve token'ları `platform.key` ile şifreler. Yönetim API'si onları hiçbir zaman döndürmez. GitLab ve Gitea token'larının süresi dolar. Bu yüzden r3v3rs3 token'ı kullanmadan önce refresh token'ıyla yeniler. Sağlayıcı yenilemeyi reddederse bağlantı **Yeniden bağlanın** durumunu gösterir ve **Yeniden bağlan** bağlantıyı yeniden yetkilendirir. Bir bağlantının sağlayıcısını, adresini veya client id'sini değiştirmek de bağlantıyı koparır.

### Bağlantılı uygulamalar

Uygulama formundaki **Git sağlayıcı bağlantısı** seçimi bağlantıları listeler. Bağlı bir bağlantı seçilince form repository'leri listeler: GitLab ve Gitea hesabın son değişen repository'lerini, GitHub ise uygulamanın kurulduğu repository'leri gösterir. **Repository'lerde ara** bir repoyu adıyla bulur. Seçilen repo **Repository** adresini ve varsayılan branch'ini doldurur. Ardından **Branch** alanı reponun branch'lerini listeler. Repo adresi bağlantının sağlayıcısına ait olmalıdır.

Deployment ve geri alma, repoyu bağlantının güncel token'ıyla clone eder. GitHub installation token'ını `x-access-token` kullanıcısının parolası olarak, GitLab token'ı `oauth2` kullanıcısının parolası olarak, Gitea ise token'ı kullanıcı adı olarak alır.

### Bağlantının webhook'ları

**Ayarlar** sayfasında **Genel adres** alanını ayarlayın (`config.toml` içinde `public_url`), örneğin `https://deploy.example.com`. Git sağlayıcısı webhook isteklerini `<genel adres>/hooks/apps/{id}` adresine gönderir. Bu yüzden sağlayıcı o adrese ulaşabilmelidir. `/hooks/` önekinin proxy'si için [Webhook'lar](#webhook-lar) bölümüne bakın.

Bağlantısı olan bir uygulama kaydedilince r3v3rs3, uygulamanın webhook secret'ı yoksa onu oluşturur ve repoya bir push webhook'u kurar. Uygulamanın reposu değişince, bağlantısı kaldırılınca veya uygulama silinince r3v3rs3 sağlayıcıdaki eski webhook'u siler. Yeni bir webhook secret'ı webhook'u yeniden kurar. **Webhook'u kapat** webhook'u sağlayıcıda siler.

Başarısız bir kurulum uygulamanın değişikliğini durdurmaz. Uygulama sayfasının **Webhook** bölümü nedeni gösterir. **Webhook'u yeniden kur** veya `POST /api/apps/{id}/hook`, uygulamanın eski webhook'unu siler ve onu yeniden kurar. Genel adres yoksa hiçbir webhook kurulmaz ve uygulama bu nedeni gösterir. `POST /api/apps/{id}/hook`, genel adres yoksa `409 public_url_missing`, bağlantısı olmayan uygulamada `400 invalid_git_connection` yanıtını verir. r3v3rs3'ün sağlayıcıda silemediği bir webhook orada kalır ve r3v3rs3'ten `404` alır.

Bir uygulamanın kullandığı bağlantı silinemez ve `409 git_connection_in_use` yanıtını alır. Bağlantıyı silmek sağlayıcıdaki yetkiyi kaldırmaz. GitHub App'i GitHub ayarlarında, GitLab veya Gitea OAuth uygulamasını sağlayıcıda silin.

## Bildirimler

"Ayarlar" sayfasındaki bildirim webhook'u platformun olaylarını da alır: her deployment için `deployment_started`, `deployment_running` ve `deployment_failed`, her agent hedefi için `agent_online` ve `agent_offline`, ve bir uygulama yapılandırılan CPU veya bellek uyarı eşiğini aştığında `app_alert`. JSON gövdesi için [Bildirimler](@/configuration.tr.md#bildirimler) bölümüne bakın.

### Kaynak Uyarı Kuralları

Bir uygulama, formundaki **CPU Uyarı Eşiği (%)** ve **Bellek Uyarı Eşiği (MB)** alanlarında (veya `AppSpec` içindeki `alert_rules`) uyarı kuralları tanımlayabilir. Periyodik arka plan metrik toplaması sırasında (her 30 saniyede bir), r3v3rs3 container istatistiklerini yapılandırılan eşiklerle karşılaştırır. Kullanım bir eşiğe ulaştığında veya onu aştığında, webhook'a anlık değer ve eşik bilgisiyle bir `app_alert` bildirimi gönderilir. Bildirim kirliliğini önlemek amacıyla aynı uygulama ve metrik için tekrarlanan uyarılar 5 dakikalık bekleme süresiyle sınırlandırılır.

### Konteyner Kaynak Sınırları ve Dikey Ölçekleme

Uygulamalar ve veritabanları, bellek sızıntılarının veya aşırı CPU tüketen işlemlerin sunucu kaynaklarını kilitlemesini önlemek için ayrıntılı donanım kotaları tanımlayabilir:

- **Bellek Sınırı (MB)** (`memory_mb`): Container için azami bellek tahsisi. Boş bırakıldığında sınır konmaz.
- **Swap Sınırı (MB)** (`swap_mb`): Bellek sınırına ek olarak izin verilen swap bellek miktarı. `0` girildiğinde container için swap tamamen devre dışı bırakılır. Boş bırakıldığında Docker motoru varsayılanı geçerlidir.
- **CPU Sınırı** (`cpus`): Kesirli veya tam CPU çekirdek kotası, örneğin 1.5 çekirdek için `1.5`.
- **Dinamik Dikey Ölçekleme (Vertical Scaling)**: WebUI veya `PUT /api/apps/{id}` üzerinden kaynak sınırları güncellendiğinde, r3v3rs3 çalışan container'ın cgroup ayarlarını kesinti olmadan veya container'ı yeniden başlatmadan anında günceller.
- **OOM Koruması ve Eşik Uyarıları**: Bir container'a bellek sınırı tanımlandığında, r3v3rs3 30 saniyelik periyodik kontrollerde bellek doluluğunu izler ve kullanım %90'a ulaştığında webhook'a `app_alert` (OOM uyarısı) iletir. Container çekirdek OOM killer tarafından sonlandırılırsa anında `oom_killed` uyarısı gönderilir.

## Giden Webhook'lar & Olay Bildirimleri

Platform, sistemde meydana gelen olayları harici izleme, bildirim ve otomasyon servislerine (Slack, Discord, PagerDuty, n8n, Datadog veya özel web servisleri) gerçek zamanlı olarak iletmek için merkezi bir giden webhook motoru içerir. Ayarlar sayfasındaki tekli bildirim webhook'undan farklı olarak, giden webhook yöneticisi birden fazla uç noktayı, ayrıntılı olay aboneliklerini, özel HTTP başlıklarını, şifreli imzalama anahtarlarını ve anında test gönderimini destekler.

### Olay Kategorileri ve Türleri

Yöneticiler, her webhook hedefi için dört ana kategori altında sunulan 14 olay türünden istediklerini seçerek abone olabilir:

| Kategori | Olay Kodu | Açıklama |
|---|---|---|
| **Deployment'lar** | `deploy.success` | Uygulama deployment'ı başarıyla tamamlandı ve container sağlıklı. |
| | `deploy.failure` | Uygulama deployment'ı derleme, çekme veya başlatma sırasında başarısız oldu. |
| | `deploy.pending_approval` | Deployment oluşturuldu ancak manuel onay bekliyor. |
| | `app.crashed` | Uygulama container'ı sıfır dışı bir çıkış koduyla beklenmedik şekilde çöktü veya kapandı. |
| **Veritabanları & Yedekler** | `backup.success` | Veritabanı yedeği başarıyla alındı ve yerel diske kaydedildi. |
| | `backup.failure` | Veritabanı dump alma veya yedekleme işlemi başarısız oldu. |
| | `backup.upload_failed` | Veritabanı yedeğinin uzak depoya (S3/R2/SFTP) yüklenmesi başarısız oldu. |
| | `backup.restored` | Veritabanı yerel veya uzak yedek snapshot'ından başarıyla geri yüklendi. |
| **Güvenlik & Sertifikalar** | `cert.expiring` | TLS sertifikasının süresinin dolmasına 30 günden az kaldı. |
| | `cert.renewed` | TLS sertifikası ACME üzerinden başarıyla yenilendi. |
| | `cert.renewal_failed` | TLS sertifikası yenileme denemesi başarısız oldu. |
| | `security.ip_banned` | İstemci IP adresi hız sınırı veya güvenlik kuralı nedeniyle yasaklandı. |
| **Sistem & Düğümler** | `target.offline` | Agent düğümü kalp atışı göndermeyi durdurdu ve çevrimdışı olarak işaretlendi. |
| | `system.disk_pressure` | Hedef düğüm disk doluluk oranı uyarı eşiğini aştı. |
| **Sistem** | `test.ping` | WebUI veya API üzerinden manuel olarak gönderilen test ping'i. |

### Yük Formatı ve Başlıklar

Webhook'lar standart bir zarf formatında `Content-Type: application/json` başlığıyla HTTP `POST` istekleri gönderir:

```json
{
  "id": "del_01j...",
  "event": "deploy.success",
  "created_at": "2026-10-21T21:40:00Z",
  "timestamp": 1792618800,
  "summary": "Deployment running: my-app (prod)",
  "data": {
    "app_id": "app_01j...",
    "app_name": "my-app",
    "deployment_id": "dep_01j...",
    "environment": "production"
  }
}
```

İletim aşağıdaki HTTP başlıklarını içerir:
- `Content-Type: application/json`
- `User-Agent: r3v3rs3-Webhook/2.0`
- `X-R3v3rs3-Event`: Olay kodu (örn. `deploy.success`).
- `X-R3v3rs3-Delivery`: Benzersiz UUID iletim tanıtıcısı.
- `X-R3v3rs3-Signature`: Webhook imzalama anahtarı tanımlanmışsa `sha256=<hex_digest>` biçiminde HMAC-SHA256 imzası.

Alıcılar, istek gövdesinin HMAC-SHA256 özetini yapılandırılan gizli anahtar ile hesaplayıp `X-R3v3rs3-Signature` başlığındaki hex özetiyle sabit zamanlı karşılaştırma (constant-time comparison) yaparak isteğin r3v3rs3'ten geldiğini doğrulayabilir.

### Gizli Anahtar Şifreleme ve Güvenlik

Webhook imzalama anahtarları, platformun zarf şifrelemesi kullanılarak `platform.key` ile disk üzerinde AES-256-GCM ile şifrelenir. Düz metin anahtar asla API'den döndürülmez veya denetim günlüklerine yazılmaz. Platform ana anahtarı döndürüldüğünde (key rotation), yapılandırılmış tüm webhook anahtarları otomatik olarak yeni anahtarla yeniden mühürlenir.

## Yönlendirme

Platform, en az bir alan adı olan her çalışan uygulama için bir proxy ekler. Bu proxy'ler `Platform` sağlayıcısından gelir. Bu yüzden [servis keşfi](@/discovery.tr.md) proxy'leri gibi salt okunurdur ve proxy listesi sağlayıcının durumunu gösterir. Her proxy:

- uygulamanın **Portlar** alanlarında seçtiği portlarda dinler; seçim yoksa `proxy_ports` portlarını, o ayar da boşsa tüm HTTP ve HTTPS portlarını kullanır,
- uygulamanın alan adlarına yanıt verir,
- istekleri container'ın `127.0.0.1` üzerindeki dışarı açılmış portuna gönderir,
- sertifikasını `acme` ACME kaydından, kaydın hesabı, challenge'ı ve DNS sağlayıcısı ile alır. [ACME sertifikaları](@/discovery.tr.md#acme-sertifikalari) kuralları geçerlidir.

Alan adı olmayan bir uygulama proxy almaz. Container'ı olmayan veya durmuş olan çalışan bir deployment, sağlayıcı durumunda bir sorun olarak görünür. r3v3rs3 container'ları 15 saniyede bir okur. Bu yüzden Docker'ın yeni bir portta yeniden başlattığı container yönlendirmesini geri alır.

## Uygulamayı silme

`DELETE /api/apps/{id}` uygulamanın container'larını, ağını ve build edilmiş image'larını durdurup siler, proxy'sini kaldırır, ortam değişkenlerini ve deployment'larını siler. Adlandırılmış volume'lar kalır. Compose uygulamasında `docker compose down` çalıştırır, projenin build ettiği image'ları ve checkout'u siler. Compose projesinin volume'ları kalır.

## Agent hedefleri

Agent hedefi, uygulamaları uzak bir sunucuda çalıştırır. Bu sunucu `r3v3rs3 agent` komutunu çalıştırır. Agent, bu sunucunun (master'ın) agent portuna bağlanır ve master'ın isteklerini kendi Docker Engine'inde çalıştırır. Agent hiçbir port açmaz: master'a kendisi bağlanır ve bir uygulamaya giden her istek bu bağlantı üzerinden gelir. Uzak sunucu NAT arkasında olabilir.

### Agent portunu açma

`[platform]` bölümünde `agent_port` değerini ayarlayın ve sunucuyu yeniden başlatın. Master, `agent_port` ile ilk başlangıçta yapılandırma dizinindeki `certs/agent/` altında agent CA'sını ve kendi agent sunucu sertifikasını oluşturur. Agent portu sertifika listesindeki bir sertifikayı değil, bu CA'yı kullanır. `certs/agent/` dizininin yedeğini saklayın, çünkü yeni bir agent CA'sı bütün agent'ların yeniden kaydolmasını gerektirir. Firewall'da portu agent'ların adreslerine açın.

### Hedef ekleme

**Hedefler** sayfasındaki **Agent hedefi ekle** formu veya `{"name": "edge-1"}` gövdeli `POST /api/targets` isteği bir hedef ekler ve kayıt token'ını bir kez gösterir. Token `<secret>.<ca hash>` biçimindedir. CA hash'i, agent'ın ilk bağlantıda master'ı tanımasını sağlar. Bu yüzden kayıt için CA dosyası gerekmez ve bağlantı araya giren biri tarafından dinlenemez. Token tek bir agent'ı kaydeder: agent bir sertifika imzalama isteği (CSR) gönderir, master onu agent CA'sı ile imzalar ve token'ı siler. Agent bundan sonra kendi istemci sertifikasıyla bağlanır.

### Agent'ı kurma

Uzak sunucuda Docker Engine, Compose uygulamaları için de Compose eklentili `docker` binary'si gerekir. `git` gerekmez, çünkü master repoyu clone eder ve dosyaları agent'a gönderir.

**Hedefler** sayfası iki komutu da master adresi ve token dolu olarak gösterir. systemd kullanan bir Linux sunucusunda `install.sh` ile:

```sh
curl -fsSL https://raw.githubusercontent.com/KilimcininKorOglu/r3v3rs3/main/install.sh | sudo bash -s -- --agent --master master.example.com:9443 --token <token>
```

Script binary'yi kurar, `/var/lib/r3v3rs3-agent` veri dizinini kullanan `r3v3rs3-agent` systemd servisini oluşturur, agent kaydolana kadar bekler ve sonra token'ı siler. Token unit dosyasına hiç yazılmaz. Güncellemek için script'i `--agent` ile yeniden çalıştırın. Kayıtlı bir agent token istemez. Yeni bir token ile script agent'ı yeniden kaydeder. Bu kayıt başarısız olursa önceki kimliği geri yükler.

| Seçenek | Ortam değişkeni | Varsayılan | Açıklama |
|---|---|---|---|
| `--master` | `R3V3RS3_AGENT_MASTER` | | Master'ın agent portu, `HOST:PORT` biçiminde. |
| `--token` | `R3V3RS3_AGENT_TOKEN` | | Kayıt token'ı. Yalnız ilk başlangıçta gerekir. Kayıtlı bir agent bu değeri kullanmaz. |
| `--data-dir` | `R3V3RS3_AGENT_DATA_DIR` | kullanıcının veri dizinindeki `agent` | Agent'ın key'i, sertifikası ve Compose dosyaları. |
| `--docker` | `R3V3RS3_AGENT_DOCKER` | `unix:///var/run/docker.sock` | Agent host'unun Docker Engine API'si. |
| `--log-level` | `R3V3RS3_AGENT_LOG_LEVEL` | `info` | Log seviyesi. |

Veri dizini `agent.key` (mod `0600`), `agent.pem`, `ca.pem`, `target` dosyalarını ve `compose/` dizinini içerir. Agent SIGINT ile düzgün kapanır.

### Agent'taki uygulamalar

`target` değeri bir agent hedefinin id'si olan uygulama, her kaynak türüyle o hedefte çalışır:

- Image uygulaması image'ını agent host'unda pull eder.
- Git uygulaması master'da clone edilir. Master build context'i agent'a gönderir ve image'ı agent host'unun Docker Engine'i build eder.
- Compose uygulaması master'da clone edilir ve override dosyasını da master yazar. Master deployment dizinini agent'a gönderir. Agent dizini kendi veri dizinindeki `compose/<app id>/<deployment id>/` altına açar ve `docker compose` komutunu orada çalıştırır.

Container'lar portlarını agent host'unun `127.0.0.1` adresinde publish eder. Master her böyle port için kendi `127.0.0.1` adresinde bir forwarder açar. Uygulamanın route'u ve sağlık kontrolü bu forwarder'ı kullanır. Forwarder'a gelen her bağlantı, agent bağlantısı üzerinden agent host'undaki porta bir tunnel açar. Bu yüzden HTTP/1.1, HTTP/2 ve WebSocket değişmeden çalışır.

Agent her isteği yeniden kontrol eder. Yalnız platform etiketi olan container'ları, adı `r3v3rs3-` ile başlayan ağları ve Compose projelerini ve adı `r3v3rs3/` ile başlayan image'ları değiştirir. Her container ayarını da doğrular. Agent host'unda da Compose dosyası kısıtlanmaz. Bu yüzden Edit izni olan bir hesap, bir Compose uygulaması üzerinden agent host'unu ele geçirebilir.

### Bağlantısı kopan agent'lar

Master her agent'a 10 saniyede bir ping gönderir. Bağlantısını kapatan veya bir ping'e 20 saniye içinde yanıt vermeyen agent bağlı değil sayılır. Hedefi bağlı olmayan bir uygulamada:

- route kalır ve `502` yanıtı verir,
- bir deployment `the agent of the target is not connected` mesajıyla başarısız olur,
- log isteği `503 agent_offline` yanıtını alır.

Agent, 1 saniyeden 60 saniyeye kadar uzayan bir beklemeden sonra yeniden bağlanır. Agent geri gelince uygulamalarının route'ları 15 saniye içinde yeniden çalışır.

### Agent'ı değiştirme veya silme

**Hedefler** sayfasındaki **Yeni token** işlemi veya `POST /api/targets/{id}/token` isteği, hedefe yeni bir kayıt token'ı verir. Kayıtlı agent, başka bir agent bu token ile kaydolana kadar çalışmaya devam eder. Sonra master eski agent'ın bağlantısını keser ve sertifikasını reddeder. Bu işlemi bir hedefi yeni bir sunucuya taşımak veya kaybolan bir agent key'inin yerine yenisini almak için kullanın.

`DELETE /api/targets/{id}`, uygulaması olmayan bir hedefi siler ve agent'ının bağlantısını keser. Uygulaması olan hedef `409 target_in_use`, yerel hedef `403 target_read_only` yanıtını alır. Sertifikası hiçbir hedefe ait olmayan agent log'a `the master closed the connection before its first request` yazar ve bağlanmayı denemeye devam eder.

### Agent'a Uzaktan Kendi Başına Güncelleme

Master sunucu yeni bir sürüme güncellendiğinde, güncel `r3v3rs3` çalıştırılabilir ikili dosyasını bağlı agent host'larına itebilir:

- **Otomatik Güncelleme**: Sürümü master sürümünden eski olan bir agent bağlandığında, master yeni ikili dosyayı çoklu TLS bağlantısı üzerinden otomatik olarak iter. Agent ikili dosyayı alır, diskteki çalıştırılabilir dosyasını yenisiyle değiştirir ve temiz bir şekilde yeniden başlar (systemd aracılığıyla).
- **Elle Güncelleme**: **Hedefler** sayfasında bir admin, bağlı bir agent satırındaki **Güncelle** butonuna veya tüm bağlı hedeflere en son ikili dosyayı itmek için **Tüm Agent'ları Güncelle** butonuna tıklayabilir.
- **Yönetim API'si**: `POST /api/targets/{id}/upgrade` belirli bir agent'a güncellemeyi iter, `POST /api/targets/upgrade` ise tüm eski sürümlü bağlı agent'ları günceller.

### Çoklu Hedef Replika Dağıtımı

Bir uygulama, aynı anda birden fazla agent hedefi ve yerel hedef üzerinde replikalar halinde dağıtılabilir:

- `AppSpec` içinde `targets` alanına hedef ID'leri listesi verilir (örn. `["local", "edge-1", "edge-2"]`).
- İsteğe bağlı olarak `load_balancing` (`round_robin`, `least_conn`, `random`, `first` veya `client_ip_hash`) ve `sticky` çerez oturumu (sticky session) ayarları seçilebilir.
- Bir deployment çalıştığında, master tüm belirtilen hedeflerde imajı hazırlar, konteyneri başlatır ve sağlık kontrollerini bekler.
- Uzak agent portları master üzerinde loopback forwarder ile eşlenir ve master'ın ters proxysi seçilen yük dengeleme algoritmasıyla trafiği tüm etkin replikalar arasında otomatik dengeler.
- Herhangi bir agent geçici olarak çevrimdışı kalırsa, master trafiği diğer sağlıklı replikalar arasında yönlendirmeye devam eder.
- **Aktif Sağlık Kontrolleri & Outlier Tespiti**: Bir uygulama `health_check_path` tanımladığında, r3v3rs3 her 10 saniyede bir HTTP üzerinden tüm replikaları periyodik olarak sorgular. Açık bir HTTP kontrol yolu belirtilmeyen çoklu replikalı uygulamalarda ise her 10 saniyede bir aktif TCP bağlantı yoklaması yapılır. Devre kesici (circuit breaker) ve anomali tespiti (10 saniyelik pencerede %50 hata oranı eşiği), arızalanan replikaları 30 saniye boyunca otomatik olarak devreden çıkararak trafiği yalnızca sağlıklı düğümlere yönlendirir.
- **Düğüm Bakımı & Havuz Drenajı (Node Drain)**: Sistem yöneticisi, **Hedefler** sayfasından veya `POST /api/targets/{id}/drain` çağrısıyla herhangi bir hedef düğümü bakım moduna ("Drenaj") alabilir. Drenaj durumundaki düğüm, mevcut açık bağlantıları kesintiye uğratmadan yeni trafik yönlendirme havuzundan çıkarılır. **Aktif et** butonuna tıklanarak veya `POST /api/targets/{id}/undrain` çağrısıyla düğüm sıfır kesintiyle yeniden yük dengeleme havuzuna alınır.

## Konteyner Registry'leri

Özel konteyner imajlarını çekerken (örneğin özel Docker Hub depoları, GitHub Packages `ghcr.io`, GitLab Registry, AWS ECR veya özel bir registry sunucusu), kimlik bilgilerini WebUI'daki **Registry'ler** sayfasından veya admin API üzerinden yapılandırın.

```json
{
  "server": "ghcr.io",
  "username": "octocat",
  "password": "ghp_xxxxxxxxxxxxxxxxxxxx"
}
```

- Registry parolaları ve kişisel erişim token'ları `platform.key` ile mühürlenir ve yalnızca standart Docker `X-Registry-Auth` ile imaj çekme sırasında hedef konteyner motoruna iletilmek üzere açılır.
- Anahtar döndürme (`POST /api/platform/key/rotate`), depolanan tüm registry sırlarını tek bir atomik işlemde çözüp yeni anahtarla yeniden mühürler.
- Bir imaj uygulaması deploy edilirken, r3v3rs3 ilgili registry sunucusunu (örn. `ghcr.io` veya `docker.io` / `registry-1.docker.io`) tespit eder, eşleşen kimlik bilgilerini ekler ve çekme isteğini yerel motora veya uzak agent'lara iletir.

## WebUI

**Kontrol Paneli** (`/`), sistem durumunun, portların, proxy'lerin, uygulamaların, veritabanlarının, hedeflerin, disk depolama alanının ve mevcut güncellemelerin tek ekranda özetini sunar. Kenar çubuğundaki **Platform** grubu **Projeler**, **Uygulamalar**, **App Store**, **Veritabanları**, **Yedekler**, **Hedefler**, **Git Sağlayıcıları**, **Registry'ler** ve **Çevre Grupları** sayfalarını içerir. Grup yalnız platform açıkken görünür. **Git Sağlayıcıları** ve **Registry'ler** sayfaları Edit/Admin izni olan hesaba görünür. Orada bağlantı veya registry ekleme, değiştirme ve silme yalnız admin'e açıktır. Grup yalnız proxy listesi olmayan hesaplarda görünür. Read izni olan hesap uygulamaları, deployment'larını ve log'larını görür. Edit izni uygulama ekler, değiştirir, deploy eder ve siler.

### Kontrol Paneli

**Kontrol Paneli**, sistem operasyonlarını tek bir birleşik ekranda özetler:
- **Durum Kartları**: Portlar, proxy'ler, uygulamalar (güncelleme rozetleriyle), veritabanları ve agent hedefleri için canlı sayılar ve aktif oranları; doğrudan ilgili yönetim sayfasına bağlanır.
- **Disk Alanı**: Kullanılan ve kullanılabilir disk kapasitesini gigabayt ve yüzde olarak gösteren gerçek zamanlı depolama göstergesi.
- **Son Dağıtımlar**: Tüm uygulamalardaki en son deployment işleri; çalışma durumunu, tetikleyici kaynağını ve başlama zamanını görüntüler.
- **Hızlı Bağlantılar**: Proxy, port ve uygulama oluşturmak veya App Store kataloğunu keşfetmek için doğrudan kısayollar.
- **Admin API**: `GET /api/dashboard` uç noktası `DashboardSummary` nesnesini döndürür.

### Projeler

**Projeler** sayfası uygulamaları ve veritabanlarını yapılandırılmış projeler ve dağıtım ortamları altında organize eder:

- Projeler (örn. *E-Ticaret*) oluşturun ve isteğe bağlı olarak ilk ortam adlarını belirleyin (varsayılan: `production`).
- Her proje altında birden fazla yalıtılmış ortam yönetin (örn. `production`, `staging`, `preview`).
- Ortamdaki tüm uygulamaların otomatik olarak miras alacağı paylaşılan ortak ortam değişkenlerini ve şifrelenmiş gizli anahtarları yapılandırın.
- Belirli bir projeye veya ortama ait uygulamaları ve veritabanlarını filtrelemek ve görüntülemek için doğrudan hızlı bağlantılar.

### Uygulamalar

**Uygulamalar** sayfası her uygulamayı **Kaynak**, **Alan adları** ve **Son deployment** durumuyla listeler. Bir uygulamanın satır işlemleri şunlardır:

- **Düzenle** uygulama sayfasını açar. Edit izni olmayan hesap **Görüntüle** ve salt okunur bir form görür.
- **Deployment'lar** deployment geçmişini açar.
- **Log** container log'unu açar.
- **Deploy et** onaydan sonra bir deployment başlatır.

Sayfa, bitmemiş bir deployment varken uygulamaları 2 saniyede bir, yoksa 10 saniyede bir yeniden okur.

### App Store

**App Store** sayfası tek tıkla kurulan uygulamaların kataloğunu listeler; ayrıntı için [App Store](#app-store) bölümüne bakın. **Kur**, girdinin alanları doldurulmuş app formunu açar; Compose girdisi ayrıca **Değişkenlerini** listeler, uygulamayı oluşturmak onları kaydeder ve ilk deployment'ı başlatır. **Kataloğu yenile** güncel kataloğu depodan hemen indirir.

### Uygulama ekleme ve değiştirme

**Ekle** boş bir uygulama formu açar. **Kaynak** alanında **Image**, **Git** veya **Compose** seçilir ve form o kaynağın alanlarını gösterir. Git veya Compose uygulaması reposunu bir **Git sağlayıcı bağlantısı** ile seçebilir, bkz. [Bağlantılı uygulamalar](#baglantili-uygulamalar). Compose uygulamasında **Volume'lar**, **Yeniden başlatma politikası**, **Bellek limiti (MB)** ve **CPU limiti** gizlenir, çünkü bunları Compose dosyası ayarlar.

- **Alan adları** alanına her satıra bir alan adı yazın. r3v3rs3 yapılandırılan alan adlarını gerçek zamanlı denetler ve **Otomatik Wildcard SSL Paylaşımı** uygular: sunucuda aktif bir wildcard sertifikası (`*.example.com` gibi) varsa, alt alan adları (`api.example.com`, `shop.example.com`) ek bir TLS ayarına veya yinelenen sertifika siparişine gerek kalmadan anında korunur. Harici özel alan adları eklendiğinde WebUI, genel ana bilgisayara yönlenen CNAME kurulum yönergeleri ve tek tıkla genel DNS kayıtlarını sorgulayan **CNAME Doğrula** butonunu sunar.
- **Volume'lar** alanına her satıra bir mount yazın: `volume:/yol`, salt okunur mount için `volume:/yol:ro`.
- **Bellek limiti (MB)** tam bir megabayt değeri, **CPU limiti** `1.5` gibi bir CPU sayısı alır. Boş alan limit koymaz.

**Oluştur** uygulamayı kaydeder ve sayfasını açar. Mevcut bir uygulamanın sayfasında formun altında şu bölümler bulunur:

- **Git token**, Git veya Compose uygulamasında. **Token'ı ayarla** bir token kaydeder, **Token'ı kaldır** onaydan sonra token'ı siler. Sayfa yalnız token'ın ayarlı olup olmadığını gösterir.
- **Webhook**. Bağlantısı olan uygulamada bu bölüm, webhook'un sağlayıcıda kurulu olup olmadığını veya neden kurulmadığını **Webhook'u yeniden kur** ile birlikte gösterir. **Secret oluştur** [webhook](#webhook-lar) secret'ını oluşturur ve onu sayfanın adresindeki hook adresi olan **Payload URL** ile birlikte bir kez gösterir. Sayfadan ayrılmadan önce ikisini de kopyalayın. **Yeni secret oluştur** onaydan sonra secret'ı değiştirir, **Webhook'u kapat** onaydan sonra secret'ı siler.
- **Ortam değişkenleri**. **Değişken ekle**, **Key**, **Değer** ve **Secret** kutusu olan bir satır ekler. Kaydedilmiş secret bir değişkenin değeri **Değişmedi** olarak görünür. Değeri korumak için alanı boş bırakın. **Değişkenleri kaydet** bütün değişkenleri değiştirir ve sonraki deployment bunları kullanır.
- **Uygulamayı sil** onaydan sonra uygulamayı container'larıyla birlikte siler.

### Metrikler

Uygulamanın sayfasında bir **Metrikler** kartı vardır. Uygulama çalıştığı sürece sunucu 30 saniyede bir bir örnek kaydeder: container'ın CPU'su, belleği, ağ arayüzlerinin baytları ve uygulamanın proxy'sinin karşıladığı istekler; her biri önceki örneğe göre değişim olarak. Kart en son değerleri, son saatlerin CPU ve istek grafiklerini ve son örneğin zamanını gösterir. Örnekleri 10 saniyede bir yeniden okur. Geçmiş bir gün boyunca platform veritabanında kalır; çalışmayan bir uygulama örnek kaydetmez. Agent target üzerindeki bir uygulamanın container istatistikleri alınmaz, yalnız istekleri kaydedilir.

### Deployment geçmişi

Deployment geçmişi son 100 deployment'ı durumu, **Tetikleyen** (**Elle**, **Webhook** veya **Geri alma**), **Commit**, **Hesap**, **Başlangıç**, **Süre** ve **Mesaj** ile gösterir. **Geri al** onaydan sonra önceki bir deployment'ı yeniden başlatır. Link yalnız bitmiş, artık çalışmayan ve image özeti kaydedilmiş bir deployment'ta görünür. Compose uygulamasında kaydedilmiş commit gerekir. Uygulamanın bitmemiş bir deployment'ı varken link gizlenir. Sayfa **Uygulamalar** sayfasıyla aynı aralıklarla yenilenir.

### Log

Log sayfası çalışan container'ın stdout ve stderr çıktısının son 200 satırını gösterir ve yeni satırları WebSocket bağlantısı üzerinden canlı aktarır. Üst araç çubuğu canlı akış geçişi (canlı veya duraklatıldı), otomatik kaydırma anahtarı, eşleşen satır sayısını gösteren gerçek zamanlı arama/filtreleme kutusu ve log çıktısını `.txt` dosyası olarak dışa aktaran **İndir** butonu sunar. Docker Compose uygulamalarında araç çubuğundaki **Servis** seçici menüsü sayesinde projedeki belirli bir servis konteynerinin stdout ve stderr çıktıları filtrelenebilir. Çalışan container'ı olmayan uygulamada "Uygulamanın çalışan container'ı yok." mesajı görünür.

### Terminal

Admin hesap, uygulama sayfasında bir **Terminal** linki, ayrıca Veritabanları sayfasında çalışan her veritabanının yanında doğrudan bir **CLI** butonu görür. Link, WebSocket bağlantısı üzerinden çalışan container içinde etkileşimli bir terminal açar; pencerenin boyut değişikliği container terminalinin boyutunu da değiştirir. Terminal üst araç çubuğu hızlı kabuk seçimi ve oturum kontrolleri sunar:

- **Kabuk Seçimi (Shell Selection)**: `/bin/sh`, `/bin/bash`, `/bin/zsh` arasında tek tıkla geçiş yapabilir veya özel komut girebilirsiniz. Hedef konteynerde `bash` veya `zsh` gibi gelişmiş bir kabuk yüklü değilse (minimalist Alpine tabanlı kalıplar gibi), sistem oturumu koparmadan bilgilendirici bir uyarı yazdırır ve otomatik olarak `/bin/sh` kabuğuna geri döner.
- **Veritabanı CLI Kısayolları**: Yönetilen bir veritabanının (PostgreSQL, MySQL, MariaDB, Redis, MongoDB) terminaline girildiğinde veya Veritabanları sayfasından doğrudan tıklandığında, ilgili veritabanı motorunun CLI kısayolu (`psql`, `mysql`, `mariadb`, `redis-cli`, `mongosh`) hazır olarak gelir. Konteyner ortam değişkenlerindeki kimlik ve parola bilgileri otomatik uygulanarak doğrudan ilgili konsola bağlanılır.
- **Oturum Geçmişi & Aksiyonlar**: **Çıktıyı Kopyala** butonu terminal tamponundaki tüm metni doğrudan panoya kopyalar; **Temizle** butonu ise ekran çıktısını anında sıfırlar.

Terminal, uygulama r3v3rs3 sunucusunun kendi Docker Engine'inde çalışırken kullanılabilir; agent hedefindeki bir uygulamanın henüz terminali yoktur. Bir hesabın açtığı her terminal audit log'una, seçilen kabuk veya CLI aracı bilgisiyle birlikte `exec_app` olarak geçer.

### Dosyalar

Admin hesap, uygulama sayfasında bir **Dosyalar** linki de görür. Link, çalışan container'ın volume'lerindeki dosyaları açar. Sayfa bir volume seçer, dizinlerini listeler ve **Görüntüle** ile bir dizini açar. Bir dosya satırı **İndir**, **Düzenle** ve **Sil** sunar; **Düzenle** 1 MB'a kadar bir dosyayı metin olarak açar ve **Kaydet** geri yazar. Sayfanın üst barı geçerli dizine bir dosya yükler ve bir üst dizine çıkar. Yazma işlemleri read-only olmayan bir volume içinde kalır; sayfa read-only bir volume için yükleme, düzenleme ve silme butonlarını göstermez. Terminal gibi dosyalar da yalnız uygulama r3v3rs3 sunucusunun kendi Docker Engine'inde çalışırken kullanılabilir. Her yazma ve silme işlemi audit log'una `app_file` olarak geçer.

#### Volume Yedekleri, Snapshot'lar ve Doğrudan Arşiv Dışa/İçe Aktarma

Dosyalar sayfası kalıcı volume'ler için kapsamlı yedekleme ve snapshot yönetimi sunar:

- **Snapshot Oluştur**: Çalışan container üzerinden volume'ün tam sıkıştırılmış `.tar.gz` snapshot'ını alır, yerel diskte saklar ve yedekleme ayarlarında S3 yapılandırılmışsa otomatik olarak harici S3 depolamasına yükler.
- **Snapshot'tan Geri Yükle**: Alınmış bir snapshot'ı tek tıkla doğrudan container volume'ünün içine açarak geri yükler.
- **Snapshot İndir**: Kayıtlı herhangi bir volume snapshot'ını `.tar.gz` formatında tarayıcıya indirir.
- **Arşivi Dışa Aktar (.tar.gz)**: Volume'ün canlı içeriğini ayrı bir snapshot dosyası oluşturmadan doğrudan `.tar.gz` akışı olarak indirir.
- **Arşiv İçe Aktar**: Yüklenen bir `.tar.gz` veya `.tar` arşivini doğrudan aktif volume içine çıkarır (extract eder).

### Zamanlanmış Komutlar

Uygulama sayfasında çalışan deployment container'ı içinde düzenli aralıklarla tek seferlik veya periyodik komutlar çalıştırmak için bir **Zamanlanmış Komutlar** bölümü yer alır (veritabanı bakımı, cache temizleme, ısınma görevleri vb.).

Her komut tanımı şunları içerir:
- **İsim**: Görevi tanımlayan açıklayıcı bir isim.
- **Zamanlama**: Zaman ifadesi. Günlük UTC saati `HH:MM` (ör. `03:00`), standart 5 alanlı cron sözdizimi (ör. `*/15 * * * *`, `0 2 * * *`), kısaltma makroları (`@hourly`, `@daily`) veya aralık ifadeleri (`@every 15m`, `@every 1h`, `@every 30s`) desteklenir.
- **Komut**: Container içinde `sh -c` ile çalıştırılacak komut.
- **Etkin**: Arka plan zamanlayıcısının süresi gelen görevi otomatik olarak çalıştırıp çalıştırmayacağı.

Sunucu, süresi gelen komutları kontrol edip çalıştırmak için her 30 saniyede bir arka plan döngüsü yürütür. Bir komut ayrıca **Şimdi çalıştır** butonu ile anında elle de çalıştırılabilir. Her satırda son çalıştırma durumu, çıkış kodu ve süresi kaydedilir; **Çıktı** butonu ise en son çalıştırmanın stdout ve stderr çıktısını gösteren bir modal açar.

### Hedefler

**Hedefler** sayfası yerel hedefi ve agent hedeflerini **Tür**, **Durum** (**Bağlı**, **Bağlı değil** veya **Kayıt bekliyor**), **Son görülme** ve agent'ın **Sürüm** bilgisiyle listeler. Sayfa listeyi 10 saniyede bir yeniden okur. Uygulama formundaki **Hedef** seçimi her hedefin adının yanında durumunu gösterir.

Admin hesabı ayrıca şunları görür:

- Listenin altındaki **Agent hedefi ekle** formu. **Ekle** hedefi oluşturur ve **Token** değerini, agent'ın `install.sh` komutu ve `docker run` komutuyla birlikte sayfanın üstünde bir kez gösterir. Sayfadan ayrılmadan önce bunları kopyalayın.
- Bir agent hedefinin satırındaki **Yeni token**. Onaydan sonra yeni bir kayıt token'ı oluşturur.
- Bir agent hedefinin satırındaki **Sil**. Onaydan sonra hedefi siler.
- Bir bağlı agent hedefinin satırındaki **Güncelle**. En son sunucu ikili dosyasını agent'a iter ve agent'ı yeniden başlatır.
- Bağlı agent'lar varken listenin üstündeki **Tüm Agent'ları Güncelle** butonu. Tüm bağlı agent'lara güncellemeyi iter.

`agent_port` ayarlı değilse ekleme ve yeni token işlemleri `400 agent_port_missing` yanıtını alır.

### Disk Hijyeni ve Temizleme Politikası

**Hedefler** sayfası, kullanılmayan Docker kaynaklarını otomatik temizlemek ve disk tükenmesini önlemek için bir **Disk Hijyeni ve Temizleme Politikası** kartı sunar:

- **Otomatik Zamanlanmış Temizlik**: Etkinleştirildiğinde, arka plan işleyicisi kaynakları yapılandırılan zaman aralığına göre (ör. `24h` veya `7days`) temizler.
- **Disk Basıncına Duyarlı Acil Temizlik**: Zamanlanmış aralıklara ek olarak, düğüm disk kullanımı yapılandırılan yüzde eşiğine (varsayılan: %85) ulaştığında veya aştığında otomatik bir acil durum temizliği tetiklenir. 15 dakikalık bekleme süresi (cooldown) peş peşe gereksiz döngüleri engeller; tetiklenen her acil temizlik `system.disk_pressure` giden webhook bildirimini ateşler ve geri kazanılan alan ayrıntılarıyla denetim günlüğü kaydı oluşturur. Eşik boş bırakılarak reaktif temizlik devre dışı bırakılabilir.
- **Kaynak Seçimi**: Kullanılmayan ve artık (dangling) imajlar, durdurulmuş konteynerler, BuildKit derleme önbelleği ve referanssız birimler için ayrı kontrol anahtarları.
- **Şimdi Temizle**: Anında temizlik başlatmak ve geri kazanılan alan ile silinen öğe sayılarını canlı görmek için **Şimdi Temizle** butonuna tıklayın (veya `POST /api/platform/prune/run` gönderin).
- **Çalıştırma İstatistikleri**: Son zamanlanmış temizleme, son acil durum disk basıncı temizleme zamanını ve geri kazanılan disk alanını gösterir.

### Düğüm Eşik Alarmları

**Hedefler** sayfası, ana makine kaynaklarını sürekli izlemek ve kritik eşiklere ulaşıldığında giden webhook bildirimleri göndermek için bir **Düğüm Eşik Alarm Kuralları** kartı sunar:

- **Düşük Boş Disk Alanı Uyarısı (`system.disk_low`)**: Kullanılabilir düğüm disk alanı yapılandırılan yüzde eşiğine (varsayılan: %10) veya altına düştüğünde alarm tetikler. Bildirim yükü toplam, kullanılabilir ve kullanılan disk baytlarını, geçerli boş yüzdeyi ve tetikleme eşiğini içerir.
- **Bant Genişliği Sıçrama Algılama (`system.bandwidth_spike`)**: Toplam düğüm ağ çıkış hızını saniye başına bayt cinsinden yapılandırılan eşikle karşılaştırarak ani trafik artışlarını algılar (WebUI'da MB/s cinsinden yapılandırılır). Linux üzerinde `/proc/net/dev` sayaçları, yedek olarak da konteyner ağ hızları üzerinden ölçülür.
- **Spam Önleme Bekleme Süresi (Cooldown)**: Dahili 15 dakikalık bekleme süresi, uzun süreli yüksek trafik veya sürekli düşük disk durumlarında bildirim bombardımanını engeller.
- **Giden Webhook'lar ve Denetim Kayıtları**: Abone olunan giden webhook'lar bildirimleri `Sistem` kategorisi altında alır ve kural değişiklikleri denetim günlüğünde kayıt altına alınır.

### Webhook'lar

Yönetim altındaki **Webhook'lar** sayfası, giden olay bildirim hedeflerini yönetmek için merkezi bir kontrol paneli sunar:

- **Webhook Listesi**: Yapılandırılan tüm webhook'ları hedef URL, atanan isim, aktif/pasif rozeti, mühürlü anahtar durumu rozeti ve abone olunan olay etiketleriyle birlikte listeler.
- **Webhook Ekleme / Düzenleme Modalı**:
  - **İsim ve URL**: Hedef webhook uç noktası (örn. `https://hooks.slack.com/...`).
  - **Özel Başlıklar**: İsteğe bağlı özel HTTP başlıkları JSON nesnesi (örn. `{"Authorization": "Bearer ...", "X-Custom-Token": "..."}`).
  - **İmzalama Anahtarı**: HMAC-SHA256 yük imzalama için paylaşılan gizli anahtar.
  - **Aktif/Pasif**: Hedefi silmeden bildirim gönderimini geçici olarak durdurma veya açma.
  - **Kategorize Edilmiş Olay Seçimi**: Dört kategori altında gruplanmış onay kutuları (Deployment'lar, Veritabanları & Yedekler, Güvenlik & Sertifikalar, Sistem & Düğümler) ve tek tıkla **Tümünü Seç** / **Seçimi Temizle** düğmeleri.
- **Webhook Testi**: Yapılandırmayı kaydetmeden önce bağlantıyı ve imza doğrulamasını doğrulamak için hedef uç noktaya anında bir `test.ping` yükü gönderme.

### Yedekler

**Yedekler** sayfası (`/backups`), tüm platformdaki veritabanı ve volume yedeklerini, otomatik zamanlamaları, olağanüstü durum kurtarmayı (disaster recovery) ve sistem sağlığı izlemesini tek bir noktadan yönetir:

- **Durum Kartları**: Toplam alınan yedek sayısı, tüketilen birikimli disk alanı, en son alınan yedeğin zaman damgası, yapılandırılan cron takvimi, Dead Man's Snitch / Heartbeat izleme durumu ve uzak S3 eşitleme durumunu gösteren gerçek zamanlı göstergeler.
- **Birleşik Yedekler Tablosu**: Tüm veritabanı motorlarındaki (PostgreSQL, MySQL, MariaDB, MongoDB, Redis) yedekleri motor etiketleri, veritabanı adı, yedek kimliği, boyutu, oluşturulma tarihi ve otomatik geri yükleme doğrulama durum rozeti ile listeler.
- **Arama ve Motor Filtreleri**: Veritabanı adı veya yedek kimliğine göre anında arama, ayrıca motor seçimine göre filtreleme.
- **Satır İşlemleri**:
  - **İndir**: Sıkıştırılmış `.dump.gz` dosyasını doğrudan indirme.
  - **Doğrula**: Canlı veritabanını değiştirmeden yedeğin açılabilirliğini ve sihirli bayt / SQL başlık bütünlüğünü anında test etme.
  - **Geri Yükle**: Onay sonrasında aktif veritabanı verisinin yerine seçilen yedeği canlı container'a yazma.
  - **Sil**: Yerel yedek dosyasını ve manifest verisini silme.
- **Şimdi Yedek Al Modalı**: Sistemde yapılandırılmış herhangi bir veritabanını açılır listeden seçerek anında yedekleme tetikleme.
- **Olağanüstü Durum Kurtarma Çekmecesi**: Uzak S3 uyumlu bulut depolamada saklanan snapshot'ları inceleme ve geri yükleme.
- **Ayarlar Modalı**: Otomatik UTC günlük yedekleme saatini, saklanacak yedek limitini, uzak depolama hedeflerini (S3, SFTP, rsync) ve Dead Man's Snitch / Heartbeat ping URL'sini yapılandırma.

## Yönetim API'si

| Route | İzin | Açıklama |
|---|---|---|
| `GET /api/backups` | Read | Tüm veritabanlarındaki yedekleri motor ve meta verileriyle listeler. |
| `GET /api/backups/overview` | Read | Tüm veritabanları genelinde özet istatistikler, zamanlama, saklama ve heartbeat izleme durumu. |
| `POST /api/backups/run` | Edit | Belirtilen bir veritabanının anında yedeğini alır. |
| `GET /api/targets` | Read | Uygulamaları çalıştıran Docker host'ları ve agent'larının durumu. |
| `POST /api/targets` | Admin | Bir agent hedefi ekler ve kayıt token'ını bir kez döndürür. |
| `POST /api/targets/upgrade` | Admin | Bağlı tüm eski sürümlü agent hedeflerine en güncel ikili dosyayı iter. |
| `POST /api/targets/{id}/token` | Admin | Bir agent hedefinin yeni kayıt token'ını döndürür. |
| `POST /api/targets/{id}/upgrade` | Admin | Belirtilen agent hedefine en güncel ikili dosyayı iterek hedefin agent'ını günceller. |
| `POST /api/targets/{id}/drain` | Admin | Bir hedefi bakım moduna alır, konteynerlerini aktif yönlendirmeden çıkarır. |
| `POST /api/targets/{id}/undrain` | Admin | Drenajdaki bir hedefi tekrar aktif yönlendirme havuzuna alır. |
| `DELETE /api/targets/{id}` | Admin | Uygulaması olmayan bir agent hedefini siler ve agent'ının bağlantısını keser. |
| `GET /api/registries` | Read | İmaj çekme için yapılandırılmış konteyner registry'leri. |
| `POST /api/registries` | Admin | Konteyner registry kimlik bilgisi ekler. |
| `PUT /api/registries/{id}` | Admin | Konteyner registry kimlik bilgilerini günceller. |
| `DELETE /api/registries/{id}` | Admin | Konteyner registry kimlik bilgilerini siler. |
| `GET /api/projects` | Read | Tüm projeleri ortamlarıyla birlikte listeler. |
| `POST /api/projects` | Edit | İsteğe bağlı varsayılan ortamlarla yeni bir proje oluşturur. |
| `GET /api/projects/{id}` | Read | Ortamları ve uygulama sayısıyla proje ayrıntılarını döndürür. |
| `PUT /api/projects/{id}` | Edit | Proje adını ve açıklamasını günceller. |
| `DELETE /api/projects/{id}` | Edit | Bir projeyi siler. |
| `GET /api/projects/{id}/environments` | Read | Bir projenin ortamlarını listeler. |
| `POST /api/projects/{id}/environments` | Edit | Bir projeye yeni bir ortam ekler. |
| `GET /api/projects/{id}/environments/{env_id}` | Read | Ortam ayrıntılarını döndürür. |
| `PUT /api/projects/{id}/environments/{env_id}` | Edit | Ortam adını ve açıklamasını günceller. |
| `DELETE /api/projects/{id}/environments/{env_id}` | Edit | Bir ortamı siler. |
| `GET /api/projects/{id}/environments/{env_id}/env` | Edit | Secret değerleri olmadan paylaşılan ortam değişkenlerini döndürür. |
| `PUT /api/projects/{id}/environments/{env_id}/env` | Edit | Bir ortam için paylaşılan ortak ortam değişkenlerini değiştirir. |
| `GET /api/apps` | Read | Uygulamalar. |
| `POST /api/apps` | Edit | Uygulama ekler. |
| `GET /api/apps/{id}` | Read | Bir uygulamayı döndürür. |
| `PUT /api/apps/{id}` | Edit | Bir uygulamayı değiştirir. |
| `DELETE /api/apps/{id}` | Edit | Bir uygulamayı container'larıyla birlikte siler. |
| `GET /api/apps/{id}/env` | Edit | Secret değerleri olmadan ortam değişkenleri. |
| `PUT /api/apps/{id}/env` | Edit | Ortam değişkenlerini değiştirir. |
| `PUT /api/apps/{id}/git_token` | Edit | Private bir reponun token'ını ayarlar. |
| `DELETE /api/apps/{id}/git_token` | Edit | Token'ı siler. |
| `POST /api/apps/{id}/webhook_secret` | Edit | Yeni bir webhook secret'ı oluşturur ve onu bir kez döndürür. |
| `DELETE /api/apps/{id}/webhook_secret` | Edit | Webhook'u kapatır. |
| `POST /api/apps/{id}/hook` | Edit | Bağlantısı olan bir uygulamanın webhook'unu yeniden kurar. |
| `POST /hooks/apps/{id}` | İmza | İmzalı bir push ile uygulamayı deploy eder. Bkz. [Webhook'lar](#webhook-lar). |
| `GET /api/apps/{id}/deployments` | Read | Bir uygulamanın son 100 deployment'ı. |
| `GET /api/apps/{id}/logs?tail=200` | Read | Çalışan deployment'ın container log'unun son satırları, 1 ile 1000 satır arası. Uygulamanın çalışan deployment'ı yoksa `running` değeri `false` olur. |
| `GET /api/apps/{id}/logs/live?tail=200` | Read | Bağlantıyı çalışan container'ın log'unun canlı WebSocket akışına çevirir. |
| `POST /api/apps/{id}/deploy` | Edit | Bir deployment başlatır. |
| `GET /api/apps/{id}/terminal?shell=` | Admin | Bağlantıyı çalışan container'ın WebSocket terminaline çevirir (isteğe bağlı kabuk veya DB CLI aracı seçimiyle). |
| `GET /api/apps/{id}/files?path=` | Admin | Çalışan container'ın bir volume'ündeki dizinin girdilerini döndürür. |
| `GET /api/apps/{id}/files/download?path=` | Admin | Bir volume içindeki dosyanın içeriğini döndürür. |
| `PUT /api/apps/{id}/files?path=` | Admin | İstek gövdesini yazılabilir bir volume içindeki dosyaya yazar. |
| `DELETE /api/apps/{id}/files?path=` | Admin | Yazılabilir bir volume içindeki dosyayı veya dizini siler. |
| `GET /api/apps/{id}/commands` | Read | Bir uygulamanın zamanlanmış komutlarını listeler. |
| `POST /api/apps/{id}/commands` | Edit | Uygulamaya yeni bir zamanlanmış komut ekler. |
| `PUT /api/apps/{id}/commands/{cmd_id}` | Edit | Mevcut bir zamanlanmış komutu günceller. |
| `DELETE /api/apps/{id}/commands/{cmd_id}` | Edit | Bir zamanlanmış komutu siler. |
| `POST /api/apps/{id}/commands/{cmd_id}/run` | Edit | Zamanlanmış bir komutu container içinde derhal çalıştırır ve çıktısını döner. |
| `GET /api/databases` | Read | Veritabanı uygulamaları; motor, port ve bağlantılarıyla. |
| `GET /api/databases/engines` | Read | Panelin sunduğu motorlar ve sürümler. |
| `POST /api/databases/engines/refresh` | Edit | Engine kataloğunu r3v3rs3-apps deposundan indirir. |
| `POST /api/databases` | Edit | Bir veritabanı oluşturur, deployment'ını başlatır ve parolasını bir kez döndürür. |
| `POST /api/databases/{id}/clone` | Edit | Mevcut bir veritabanını klonlar, deployment'ını başlatır ve verileri aktarır. |
| `DELETE /api/databases/{id}` | Edit | Veritabanı app'ini siler. Volume'ü kalır. |
| `GET /api/deployments/{id}` | Read | Bir deployment'ı döndürür. |
| `POST /api/deployments/{id}/rollback` | Edit | Önceki bir deployment'ı tekrarlar. |
| `POST /api/deployments/{id}/approve` | Edit | Bekleyen bir dağıtımı onaylar ve başlatır. |
| `POST /api/deployments/{id}/reject` | Edit | Bekleyen bir dağıtımı reddeder. |
| `GET /api/git/connections` | Edit | Secret'lar ve token'lar olmadan Git sağlayıcı bağlantıları. |
| `POST /api/git/connections` | Admin | GitLab veya Gitea bağlantısı ekler. |
| `GET /api/git/connections/{id}` | Edit | Bir bağlantıyı döndürür. |
| `PUT /api/git/connections/{id}` | Admin | Bir bağlantıyı değiştirir. `client_secret` yoksa mevcut secret kalır. GitHub App bağlantısında yalnız ad değişir. |
| `DELETE /api/git/connections/{id}` | Admin | Hiçbir uygulamanın kullanmadığı bir bağlantıyı siler. |
| `POST /api/git/connections/{id}/authorize` | Admin | `{"redirect_uri": "<callback adresi>"}` gövdesiyle bir yetkilendirme başlatır ve sağlayıcının sayfasını, GitHub App'te ise kurulum sayfasını döndürür. |
| `POST /api/git/connections/{id}/installation` | Admin | GitHub App'in kurulumunu GitHub'dan okur ve bağlantıyı bağlar. |
| `POST /api/git/github_apps` | Admin | `{"name": "...", "url": "...", "organization": "...", "redirect_uri": "<callback adresi>"}` gövdesiyle bir GitHub App başlatır. `url` ve `organization` isteğe bağlıdır. Yanıt, tarayıcının GitHub'a gönderdiği form adresini ve manifest'i içerir. |
| `GET /api/git/connections/{id}/repositories?search=&page=` | Edit | Bağlı hesabın veya kurulumun en fazla 50 repository'si. |
| `GET /api/git/connections/{id}/branches?repository=owner/name` | Edit | Bir reponun en fazla 50 branch'i. |
| `GET /api/platform/prune` | Read | Disk temizleme politikasını ve hijyen ayarlarını döndürür. |
| `PUT /api/platform/prune` | Admin | Disk temizleme politikasını günceller. |
| `POST /api/platform/prune/run` | Admin | Disk hijyeni temizliğini derhal çalıştırır ve geri kazanılan alanı döndürür. |
| `GET /api/webhooks` | Read | Yapılandırılmış tüm giden webhook'ları listeler. |
| `POST /api/webhooks` | Admin | Yeni bir giden webhook oluşturur. |
| `GET /api/webhooks/{id}` | Read | Bir giden webhook'un ayrıntılarını döndürür. |
| `PUT /api/webhooks/{id}` | Admin | Bir giden webhook yapılandırmasını veya anahtarını günceller. |
| `DELETE /api/webhooks/{id}` | Admin | Bir giden webhook'u siler. |
| `POST /api/webhooks/test` | Admin | Hedef URL'ye anında test ping yükü gönderir. |
| `GET /oauth/git/callback` | State | Sağlayıcının ve yeni bir GitHub App'in callback'i. Bkz. [Bağlanma](#baglanma) ve [GitHub App oluşturma](#github-app-olusturma). |

Proxy listesi olan hesap, her platform route'u için `403 forbidden` alır. Bkz. [Hesaplar](@/accounts.tr.md). `[platform] enabled` kapalıyken her platform route'u `503 platform_disabled`, platform veritabanını veya key'ini açamadığında `503 platform_failed` yanıtını verir. Başka bir uygulamanın, hedefin veya bağlantının kullandığı ad `409 app_name_exists`, `409 target_name_exists` veya `409 git_connection_name_exists` alır. Bağlı olmayan bağlantı `409 git_connection_not_connected` alır. GitHub App kurulmadan `POST /api/git/connections/{id}/installation` isteği `409 github_app_not_installed`, sağlayıcıya giden başarısız istek `502 git_provider_failed` alır. Denetim kaydı, uygulamanın her değişikliğini, her deployment'ı, her geri almayı, deploy eden her webhook isteğini, hedefin her değişikliğini, her agent kaydını, Git sağlayıcı bağlantısının her değişikliğini ve yetkilendirmesini ve bir hesabın başlattığı her webhook kurulumunu kaydeder. Ortam değişkeni değişikliğinin özeti key'leri adlandırır, hiçbir değeri içermez. Token veya secret değişikliğinin özeti yalnız uygulamayı veya hedefi adlandırır. Webhook isteğinin özeti uygulamayı, sağlayıcıyı ve deployment'ı adlandırır. Agent kaydının özeti hedefi ve agent'ın sürümünü adlandırır. Bağlantının özeti bağlantıyı ve sağlayıcısını adlandırır, client secret'ı veya bir token'ı hiçbir zaman içermez. Yetkilendirmenin özeti hesabı da adlandırır. Webhook kurulumunun özeti uygulamayı adlandırır.
