+++
title = "Yapılandırma"
description = "Yapılandırma"
weight = 0
+++

# Yapılandırma ve Veri Depolama

r3v3rs3; sunucu yapılandırmasını, ters proxy kurallarını, portları, erişim listelerini, ACME ayarlarını, kullanıcı hesaplarını ve deployment platformu verilerini yapılandırma dizininde (varsayılan `/etc/r3v3rs3`) yer alan birleşik bir SQLite veritabanında (`r3v3rs3.db`) saklar.

- **Birleşik SQLite Veritabanı (`r3v3rs3.db`)**: Atomik transaction'lar ve WAL modu sayesinde yüksek eşzamanlılık, kesintisiz veri bütünlüğü ve restart sonrasında sıfır kayıp güvencesi sunar.
- **Ana Şifreleme Anahtarı (`r3v3rs3.key`)**: Hassas kimlik bilgileri (DNS API anahtarları, Git token'ları, ortam değişkeni gizli değerleri), `0600` dosya izinlerine sahip `r3v3rs3.key` anahtarı ile AES-256-GCM kullanılarak mühürlenir.
- **Diskte Saklanan Dosyalar**: TLS PEM sertifika ve private key dosyaları `certs/<tür>/<id>/*.pem` altında diskte dosya olarak tutulur (göreli yolları `r3v3rs3.db` içinde saklanır). Harici JSON veri setleri (`cdn-ranges.json`, `app-store.json`, `engines.json`) katalog senkronizasyonu için diskte dosya olarak kalır.
- **Log Ayrıştırma (`log.db`)**: Yüksek frekanslı tracing ve istek kayıtları, `r3v3rs3.db` üzerinde kilitlenmeyi ve şişmeyi önlemek için ayrı bir `log.db` veritabanında tutulur.
- **Otomatik Legacy Migrasyon**: Yapılandırma dizininde eski düz dosyalar (`config.toml`, `ports.toml`, `proxies.toml`, `access_lists.toml`, `acme.toml`, `dns-credentials.toml`, `accounts.toml`, `notifications.json` veya `platform.db` / `platform.key`) tespit edildiğinde r3v3rs3 açılışta bu verileri otomatik olarak `r3v3rs3.db` veritabanına aktarır ve orijinal dosyaları güvenle `*.migrated` olarak yeniden adlandırır; böylece hiçbir veri kaybı yaşanmaz.

Panelin ayarlar sayfası sunucu çalışırken bütün ayarları yönetir ve değişiklikler restart gerekmeden derhal uygulanır. İstisna `worker_mode`'dur: process mimarisini değiştirir; ayarlardan veya CLI'dan tanımlanabilir ve restart ile geçerlilik kazanır.

## Yapılandırma Yedeği ve Geri Yükleme

r3v3rs3 tüm yapılandırmasını (ayarlar, portlar, proxy'ler, erişim listeleri, ACME yapılandırmaları ve depolama dosyaları) tek bir JSON yedek dosyasına aktarabilir ve başka bir sunucuya geri yükleyebilir:

- **Gizli anahtarlar maskeli**: Webhook belirteçleri, proxy ve erişim listesi parola özetleri ve ACME sağlayıcı API anahtarları gibi hassas bilgiler yedek dosyasında maskelenir; böylece yedekler güvenle saklanabilir ve taşınabilir.
- **Hesaplar hariç**: Kullanıcı hesapları yapılandırma yedekleme ve geri yükleme işlemlerine kesinlikle dahil edilmez. Bir sunucuya yedek geri yüklendiğinde mevcut yönetici hesapları ve parolaları değişmeden korunur.

### WebUI

**Ayarlar** sayfasını açın ve **Yapılandırma Yedeği ve Geri Yükleme** kartına gidin:
- `r3v3rs3-config-backup-<zaman_damgası>.json` dosyasını indirmek için **Yedeği İndir** düğmesine tıklayın.
- **Yedeği Geri Yükle** seçeneğine tıklayarak geçerli bir JSON yedek dosyası seçin. Yapılandırma uygulanmadan önce bir onay iletişim kutusu gösterilir.

### CLI

`r3` CLI yedekleme ve geri yükleme için komutlar sunar:

```bash
# Yapılandırma yedeğini bir dosyaya aktarma
$ r3 config backup -o r3v3rs3-backup.json

# Yedeği standart çıktıya verme (scriptler veya uzak yedekleme için)
$ r3 config backup

# Yapılandırmayı bir dosyadan geri yükleme
$ r3 config restore -f r3v3rs3-backup.json

# Yapılandırmayı standart girdiden geri yükleme
$ cat r3v3rs3-backup.json | r3 config restore -f -
```

### REST API

- `GET /api/config/backup`: Tam `ConfigBackup` nesnesini döner. `admin` yetkisi gerektirir.
- `POST /api/config/restore`: `ConfigBackup` JSON yükünden yapılandırmayı geri yükler. Bütün portları, proxy'leri, erişim listelerini ve ayarları kesinti olmadan anında yeniden yükler. `admin` yetkisi gerektirir ve denetim kaydına (`RestoreConfig`) işlenir.

## Felaket Kurtarma & Şifreli Platform Yedekleri (S3)

Standart JSON yapılandırma yedeklerinden farklı olarak, **Felaket Kurtarma Paketleri** (`.r3bundle`), r3v3rs3'ün ve Coolify benzeri uygulama dağıtım platformunun tüm çalışma durumunu AES-256-GCM ile şifrelenmiş tek bir arşivde saklar:

- **Tüm Durum Dahil**: `r3v3rs3.db` (tüm ayarları, portları, proxy'leri, erişim listelerini, ACME sertifikalarını, kullanıcı hesaplarını, uygulamaları, dağıtımları, veritabanlarını ve ortam değişkenlerini barındıran birleşik SQLite veritabanı), `r3v3rs3.key`, kümeleme mühür anahtarları (`keys/`), TLS sertifikaları ve özel anahtarları (`certs/`) ve varsa eski yedek dosyaları eksiksiz olarak arşivlenir.
- **AES-256-GCM Şifreleme**: İsteğe bağlı bir parola ve tuzlu SHA-256 anahtar türetimi (10.000 tur) ile AES-256-GCM standardında şifrelenir. Şifrelenmemiş veriler asla sunucu belleği dışına çıkmaz.
- **Otomatik S3 Senkronizasyonu**: Veritabanı yedekleme ayarlarında S3 uzak depolama yapılandırıldığında, oluşturulan felaket kurtarma yedekleri otomatik olarak uzak bucket içindeki `{prefix}/disaster-recovery/` altına yüklenir.
- **Tek Komutla Felaket Kurtarma**: Sıfırdan kurulan yeni bir sunucuya tek bir komutla tüm platformu ve verileri eksiksiz aktarabilme imkanı sunar.

### WebUI

**Ayarlar** sayfasını açın ve **Felaket Kurtarma & Şifreli Platform Yedekleri (S3)** kartına gidin:
- İsteğe bağlı bir şifreleme **Parolası** girin ve **Uzak S3 depolamasına yükle** seçeneğini işaretleyin.
- **Şifreli Yedek Oluştur** düğmesine tıklayarak anlık bir `.r3bundle` paketi oluşturun.
- Tablo üzerinden mevcut yedekleri indirin, geri yükleyin veya silin.
- **.r3bundle Yükle ve Geri Yükle** seçeneğiyle harici bir yedek dosyasını yükleyip tüm platform durumunu onay kutusu ile geri yükleyin.

### CLI (Yeni Sunucuda Tek Komutla Geri Yükleme)

Yeni bir sunucuda `r3v3rs3` kurulduktan sonra:

```bash
# Şifreli yedek paketinden tüm platform durumunu geri yükleme
$ r3v3rs3 disaster-recovery restore --file backup.r3bundle --passphrase "gizli-parolam"

# Sunucuyu tüm uygulamalar, sertifikalar ve hesaplarla başlatma
$ r3v3rs3 start
```

CLI üzerinden anlık felaket kurtarma yedeği alma:

```bash
# Parola ile felaket kurtarma yedeği oluşturma
$ r3v3rs3 disaster-recovery backup --output /var/backups/r3v3rs3.r3bundle --passphrase "gizli-parolam"
```

### REST API

- `GET /api/disaster-recovery/backups`: Boyutları, zaman damgaları ve S3 durumuyla felaket kurtarma yedeklerini listeler.
- `POST /api/disaster-recovery/backups`: İsteğe bağlı parola ve S3 yüklemesiyle yeni bir şifreli yedek oluşturur.
- `GET /api/disaster-recovery/backups/{id}/download`: `.r3bundle` dosyasını indirir.
- `DELETE /api/disaster-recovery/backups/{id}`: Bir felaket kurtarma yedeğini siler.
- `POST /api/disaster-recovery/backups/{id}/restore`: Saklanan bir yedek kimliğinden geri yükleme yapar.
- `POST /api/disaster-recovery/restore-upload`: `X-Passphrase` başlığıyla yüklenen bir `.r3bundle` paketinden geri yükleme yapar.

# Portlar

Proxy eklemeden önce dinlenecek bir port bağlamanız gerekir. Bunu "Portlar" bölümünden yapabilirsiniz.

r3v3rs3 altı port türünü destekler:

- HTTP
- HTTPS (TLS üzerinden HTTP)
- QUIC üzerinden HTTP (HTTP/3)
- TCP
- TLS üzerinden TCP
- UDP

Portun `listen` adresi `/ip4/<adres>` veya `/ip6/<adres>` ile başlar. Ardından `/tcp/<port>` (TCP), `/tcp/<port>/tls` (TLS üzerinden TCP), `/tcp/<port>/http` (HTTP), `/tcp/<port>/https` (HTTPS), `/udp/<port>` (UDP) veya `/udp/<port>/quic/http` (QUIC üzerinden HTTP) gelir. `name` portun isteğe bağlı adıdır. Bir portu veya proxy'yi silmeden durdurmak için "Aktif" seçeneğini kapatın (`active = false`).

## Sunucu adları

HTTPS, QUIC üzerinden HTTP ve TLS üzerinden TCP portlarında "Sunucu Adları" alanı bulunur. r3v3rs3, sunucu sertifikasını istemcinin SNI (Server Name Indication) değerine göre seçer. İstemci SNI göndermediğinde r3v3rs3, bütün sunucu adlarını taşıyan geçerli bir sunucu sertifikası seçer. Liste boşsa ilk geçerli sunucu sertifikasını seçer.

```toml
[my-port]
listen = "/ip4/0.0.0.0/tcp/443/https"
tls_termination = { server_names = ["example.com", "*.example.com"] }
```

## TLS istemci kimlik doğrulaması

HTTPS, QUIC üzerinden HTTP ve TLS üzerinden TCP portları istemcinin sertifikasını doğrulayabilir (mutual TLS). Modu "İstemci Kimlik Doğrulaması" alanından seçin:

| Mod          | Davranış                                                                                                               |
|--------------|------------------------------------------------------------------------------------------------------------------------|
| Kapalı       | Port istemci sertifikası istemez. Varsayılan mod budur.                                                                |
| İsteğe bağlı | Port sertifika ister, fakat sertifikasız istemcileri de kabul eder. İstemcinin gönderdiği sertifika geçerli olmalıdır. |
| Zorunlu      | İstemci geçerli bir sertifika göndermezse TLS handshake başarısız olur.                                                |

İsteğe bağlı ve Zorunlu modlarda "İstemci CA Sertifikaları" listesinden bir veya daha fazla kök sertifika seçin. İstemci sertifikası bunlardan biriyle imzalanmış olmalıdır. Sistemin kök sertifikaları kullanılmaz. r3v3rs3, kök sertifika seçilmemiş veya kök olmayan bir sertifika seçilmiş port ayarını reddeder. Bir portun kullandığı kök sertifika silinemez.

İstemci kimlik doğrulaması ayarı geçersiz hale gelirse (örneğin kök sertifika yapılandırma dizininden silinirse) port bütün bağlantıları kapatır ve port listesi "TLS Hatası" gösterir.

HTTPS ve QUIC üzerinden HTTP portlarında header kuralları, doğrulanan sertifikayı `{client_cert_subject}` ve `{client_cert_fingerprint}` değişkenleriyle upstream sunucuya gönderebilir. "Header kuralları" bölümüne bakın.

```toml
[my-port]
listen = "/ip4/0.0.0.0/tcp/443/https"
tls_termination = { server_names = ["example.com"], client_auth = "required", client_ca_certs = ["a1b2c3d"] }
```

### Route Başına İstemci Sertifikası Zorunluluğu

Bir HTTPS veya QUIC üzerinden HTTP portu `Optional` istemci doğrulaması kullandığında, route bazında `require_client_cert = true` ile istemci sertifikası zorunlu kılınabilir. `require_client_cert = true` olarak yapılandırılmış bir route'a doğrulanmış bir istemci sertifikası olmadan gelen istekler doğrudan `403 Forbidden` ile reddedilir. Böylece aynı port üzerinde hem genel erişime açık route'lar (`/`, `/docs`) hem de mTLS ile kilitlenmiş özel route'lar (`/admin`, `/internal`) birlikte sunulabilir:

```toml
[my-app]
protocol = "http"
vhosts = ["app.example.com"]
routes = [
  { path = "/public", servers = [{ url = "http://127.0.0.1:8080/public" }] },
  { path = "/admin", servers = [{ url = "http://127.0.0.1:8080/admin" }], require_client_cert = true },
]
```

## PROXY protocol

r3v3rs3 önündeki bir yük dengeleyici, istemci adresini PROXY protocol header'ı (sürüm 1 veya 2) ile gönderebilir. TCP, TLS üzerinden TCP, HTTP ve HTTPS portları bu header'ı okuyabilir. UDP ve QUIC üzerinden HTTP portları okuyamaz.

"PROXY Protocol Kabul Et" seçeneğini açın. Yük dengeleyicilerin IP adreslerini veya CIDR bloklarını "Güvenilen Yük Dengeleyiciler" alanına yazın:

- Güvenilen bir adresten gelen bağlantı geçerli bir header ile başlamalıdır. Header geçersizse, sürümü "Kabul Edilen Sürümler" içinde yoksa veya header "Header Timeout (Saniye)" alanındaki sürede gelmezse r3v3rs3 bağlantıyı kapatır. Varsayılan timeout 5 saniyedir.
- r3v3rs3 diğer adreslerden header okumaz. Bu bağlantıların istemci adresi peer adresidir.
- Sürüm 2 `LOCAL` header'ı ve sürüm 1 `UNKNOWN` header'ı peer adresini korur. Yük dengeleyiciler bunları sağlık kontrollerinde gönderir.

r3v3rs3 header'ı TLS handshake'ten önce okur. Header'daki adres bağlantının istemci adresi olur. IP filtreleri, rate limit'ler, istemci IP hash'i, `Forwarded` ve `X-Forwarded-For` header'ları ve access log bu adresi kullanır. Access log peer adresini de `peer` alanına yazar.

PROXY protocol header'ı ile başlayan bir ACME HTTP-01 challenge isteği challenge yanıtını almaz. Böyle bir yük dengeleyicinin sunduğu sertifika için DNS-01 challenge kullanın.

```toml
[my-port]
listen = "/ip4/0.0.0.0/tcp/443/https"
tls_termination = { server_names = ["example.com"] }
proxy_protocol = { trusted = ["10.0.0.0/8"], accept = "v2", timeout = "5s" }
```

`accept` değeri `any` (varsayılan), `v1` veya `v2` olur.

## Portu sıfırlama

Port ayarını değiştirdiğinizde açık bağlantılar etkilenmez; bu bağlantılar eski ayarla çalışmaya devam eder. Açık bağlantıları kapatmak için portu sıfırlayın.

# Proxy'ler

r3v3rs3 üç proxy türünü destekler:

- HTTP / HTTPS
- TCP / TLS üzerinden TCP
- UDP

Bir proxy'ye birden fazla port bağlayabilirsiniz. Ancak HTTP / HTTPS proxy'sine TCP veya TLS üzerinden TCP portu bağlanamaz. TCP / TLS üzerinden TCP proxy'sine de HTTP veya HTTPS portu bağlanamaz.

## Route seçimi

Bir portu birden fazla HTTP / HTTPS proxy'si kullanıyorsa r3v3rs3, porttaki bütün proxy'lerin bütün route'larını karşılaştırır. İsteği en özel route'a gönderir. Proxy'lerin ve route'ların sırası sonucu değiştirmez.

1. Önce isteğin host'u proxy'leri seçer. Host ile aynı olan virtual host (`app.example.com` veya bir IP adresi), wildcard'dan (`*.example.com`) önce gelir. Wildcard, regex kalıbından önce gelir. Regex kalıbı, virtual host'u olmayan proxy'den önce gelir. Virtual host'u olmayan proxy her host'u kabul eder.
2. Host eşleşmesi aynı olan route'lar arasında yolu en uzun olan route kazanır. Yol tam bölümlerle eşleşir. Bu yüzden `/api` yolu hem `/api` hem de `/api/users` isteğiyle eşleşir, `/apiv2` ile eşleşmez.
3. İki route'un host eşleşmesi ve yolu aynıysa önce gelen route kazanır.

Örneğin aşağıdaki route'larda `GET /api/users` isteği `http://api:8080/` sunucusuna, `GET /about` isteği `http://web:3000/` sunucusuna gider. `app.example.com` için gelen istek, yolu `/api` olsa da `my-app` proxy'sine gider.

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

### DNS SRV sunucuları

`http+srv` veya `https+srv` şemalı bir sunucu URL'i, sunucularını host adının DNS SRV kayıtlarından alır. r3v3rs3 adı çözer ve route, her SRV hedefi için bir sunucu alır: `+srv` önündeki şema, hedefin `host:port` değeri ve URL'in yolu ile sorgusu. Consul, Kubernetes headless service'leri ve diğer service registry'ler bu kayıtları yayınlar. URL'de port yazılmaz, portları SRV kayıtları verir.

```toml
[my-api]
protocol = "http"
routes = [{ path = "/", servers = [{ url = "http+srv://_http._tcp.api.service.consul/v1" }] }]
```

`_http._tcp.api.service.consul. 30 IN SRV 0 5 8080 api-1.node.consul.` ve `... 0 1 8080 api-2.node.consul.` kayıtlarıyla route, istekleri `http://api-1.node.consul:8080/v1` ve `http://api-2.node.consul:8080/v1` sunucularına `5` ve `1` ağırlıklarıyla gönderir.

- Yalnız en düşük öncelik değerine sahip kayıtlar kullanılır. Daha yüksek değerli kayıtlar DNS'te yedektir ve r3v3rs3 bunları kullanmaz.
- Kaydın SRV ağırlığı, sunucunun `weight` değeri olur. `0` ağırlığı `1` olur, çünkü r3v3rs3'te ağırlığı `0` olan sunucu yeni istek almaz. SRV sunucusunun kendisine `weight` verilemez.
- r3v3rs3, yanıtın TTL süresi bitince adı yeniden çözer. En erken 5 saniye sonra çözer. Hedefler değişince route yeni sunucuları yeniden başlatmadan alır. Kalan sunucuların sağlık durumu korunur. Başarısız bir sorgu son hedefleri korur ve 5 saniye sonra yeniden denenir.
- SRV adının hedefi olmayan route 502 döner. Route'un başka sunucuları varsa istekleri onlar alır.
- "Upstream DNS Çözümleyicisi" ayarı ([Ayarlar](#ayarlar) bölümüne bakın) SRV sorgularının DNS sunucusunu seçer, örneğin Consul DNS için `127.0.0.1:8600`. Ayar boşsa r3v3rs3 sistemin çözümleyicisini kullanır. Hedeflerin host adlarını bağlantı sırasında sistemin çözümleyicisi çözer.

### Kural Testi ve Simülasyon (Dry Run)

Proxy ayarlarını kaydetmeden önce gelen isteklerin nasıl yönlendirileceğini yerleşik kural testi aracıyla deneyebilirsiniz. WebUI proxy formundaki **Kural Testi ve Simülasyon** kartı örnek bir URL, HTTP metodu ve isteğe bağlı istemci IP'si girmenize olanak tanır. **İsteği Simüle Et** butonuna tıklandığında kaydedilmemiş kurallar bellekte test edilir:

- İsteğin hangi route veya yönlendirme kuralıyla eşleştiğini belirler (veya eşleşmeme nedenini açıklar).
- Yol yeniden yazma kurallarından (`strip_prefix`, `add_prefix`, regex değişimi) sonra çözümlenen upstream sunucu URL'sini gösterir.
- İstemci IP filtrelerini (`allow` / `deny` listeleri) kontrol ederek isteğin engellenip engellenmeyeceğini bildirir.
- Kimlik doğrulama gereksinimini (Basic Auth, Bearer, Forward Auth veya Panel Oturumu) belirtir.

Yönetim API'si bu aracı `POST /api/proxies/dry-run` uç noktasında sunar:

```bash
curl -X POST http://localhost:8080/api/proxies/dry-run \
  -H "Cookie: r3v3rs3_session=..." \
  -H "Content-Type: application/json" \
  -d '{"proxy": {"routes": [{"path": "/api", "servers": [{"url": "http://127.0.0.1:8080"}]}]}, "url": "https://example.com/api/users", "method": "GET"}'
```

## HTTPS yönlendirmesi

"HTTP'yi Otomatik Olarak HTTPS'e Yönlendir" (`upgrade_insecure`, varsayılan `true`) düz HTTP isteğine `301` ile yanıt verir ve istemciyi aynı host ile yolun proxy'nin HTTPS portundaki adresine yönlendirir. HTTPS portu olmayan proxy yönlendirme yapmaz.

## Yol yeniden yazma

r3v3rs3 varsayılan olarak route'un yolunu isteğin yolundan kaldırır ve kalan kısmı sunucu URL'sinin yoluna ekler. Örneğin `path = "/api"` değerli bir route ve `http://api:8080/v1/` sunucusu için `GET /api/users` isteği `http://api:8080/v1/users` adresine gider. Route'un `rewrite` tablosu yolu şu sırayla değiştirir:

1. `strip_prefix = false` route'un yolunu korur. Bu durumda aynı istek `http://api:8080/v1/api/users` adresine gider.
2. `regex` yoldaki ilk eşleşmeyi `replacement` değeriyle değiştirir. Yol `/` ile başlar. `${1}` veya `${name}` bir capture group ekler. Eşleşme yoksa yol değişmez.
3. `add_prefix` yolun başına `/v2` gibi bir yol ekler.

r3v3rs3 sorgu dizesini korur. Kimlik doğrulama ve cache, istemci isteğinin yolunu kullanır. r3v3rs3 geçersiz regex'i reddeder. `/` ile başlamayan veya `?` ya da `#` içeren `add_prefix` değerini de reddeder.

Aşağıdaki route'larda `GET /api/users` isteği `http://api:8080/v2/users` adresine, `GET /items/42` isteği `http://shop:9000/item/42` adresine gider.

```toml
[my-shop]
protocol = "http"
routes = [
  { path = "/api", servers = [{ url = "http://api:8080/" }], rewrite = { add_prefix = "/v2" } },
  { path = "/items", servers = [{ url = "http://shop:9000/" }], rewrite = { strip_prefix = false, regex = "^/items/([0-9]+)$", replacement = "/item/${1}" } },
]
```

## Yönlendirme kuralları

HTTP / HTTPS proxy'sinin `redirects` değeri isteğe bir yönlendirme ile yanıt verir. Bu durumda istek upstream sunucuya gitmez. Her kuralın `regex`, `target` ve `status` değerleri vardır:

- `regex`; isteğin portsuz host'u, yolu ve sorgusundan oluşan değerle eşleşir, örneğin `example.com/old/page?id=1`.
- `target` yanıtın `Location` header'ıdır. `${1}` veya `${name}` bir capture group ekler.
- `status` değeri `301`, `302` (varsayılan), `307` veya `308` olabilir.

Eşleşen ilk kural yanıt verir. r3v3rs3 istemci IP filtresini, rate limit'i ve `upgrade_insecure` HTTPS yönlendirmesini kurallardan önce uygular. Kimlik doğrulamayı ise kurallardan sonra uygular. Geçerli bir header değeri oluşturmayan hedef eşleşme sayılmaz ve r3v3rs3 bir uyarı log'u yazar. r3v3rs3 başka bir durum kodunu reddeder. Boş olan veya kontrol karakteri içeren hedef değerini de reddeder.

WebUI'da her satıra bir kuralı `status regex target` biçiminde yazın. Orada regex ve hedef boşluk içeremez. Regex içinde `\s`, hedef içinde `%20` kullanın.

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

## Sabit yanıtlar

HTTP / HTTPS proxy'sindeki bir route, isteği `servers` değerine göndermek yerine `response` ile her isteğe kendisi yanıt verebilir. Bir route'ta `servers` veya `response` değerlerinden yalnız biri bulunur. Yönlendirme host'u veya 404 host'u için sabit yanıt kullanın.

- `type = "redirect"` değeri `target` adresine bir yönlendirme ile yanıt verir. `status` değeri `301`, `302` (varsayılan), `307` veya `308` olabilir. `preserve_path` (varsayılan `true`) isteğin yolunu ve sorgusunu `target` sonuna ekler. Bu durumda `GET /a?b=1` isteği `https://example.com/a?b=1` adresine gider.
- `type = "status"` değeri `status` ile ve isteğe bağlı düz metin `body` ile yanıt verir. `body` en fazla 4096 byte olabilir. `status` değeri `200`, `400`, `403`, `404`, `410`, `429`, `451`, `500`, `502` veya `503` olabilir.

r3v3rs3; istemci IP filtresini, rate limit'i, `upgrade_insecure` HTTPS yönlendirmesini, yönlendirme kurallarını ve kimlik doğrulamayı sabit yanıttan önce uygular. r3v3rs3 hem `servers` hem `response` içeren route'u reddeder. Geçersiz hedef, durum kodu veya gövde değerini de reddeder.

WebUI'da her route için route türünü seçin. Yeni proxy sayfası "Yönlendirme host'u" ve "404 host'u" şablonlarını sunar. Servis keşfi etiketleri aynı alanları ayarlar, örneğin `r3v3rs3.http.old.routes.0.response.type=redirect` ve `r3v3rs3.http.old.routes.0.response.target=https://example.com`. `response` içeren route, container portundan varsayılan sunucu almaz.

```toml
[old-domain]
protocol = "http"
vhosts = ["old.example.com"]
routes = [{ path = "/", response = { type = "redirect", target = "https://example.com", status = 301 } }]

[catch-all]
protocol = "http"
routes = [{ path = "/", response = { type = "status", status = 404, body = "Not found" } }]
```

## Bakım modu

HTTP / HTTPS proxy'si, mevcut route ve upstream sunucu yapılandırmasını kaybetmeden veya değiştirmeden dilediğiniz an bakım moduna alınabilir. Bakım modu etkinken r3v3rs3, gelen tüm istekleri upstream sunuculara yönlendirmeden doğrudan belirlenen durum koduyla (varsayılan `503 Service Unavailable`) ve isteğe bağlı `Retry-After` header'ı ile yanıtlar.

- `enabled` (boolean, varsayılan `false`): Bakım modunu açar veya kapatır.
- `status` (tamsayı, varsayılan `503`): İstemcilere dönülen HTTP durum kodu (100–599).
- `retry_after` (isteğe bağlı tamsayı): İstemcilere ve arama motoru botlarına ne zaman tekrar kontrol edeceklerini bildiren `Retry-After` header'ındaki saniye değeri.
- `body` (isteğe bağlı metin): Özel HTML veya düz metin yanıt gövdesi. Boş bırakıldığında r3v3rs3 dahili, modern ve responsive koyu temalı bir bakım sayfası sunar (`text/html; charset=utf-8`).

WebUI'da:
- **Proxies** listesinde her HTTP proxy satırında tek tıkla durumu değiştiren **Bakıma al** / **Bakımdan çıkar** aksiyon bağlantısı bulunur; durum rozeti anında sarı **Bakımda** olarak güncellenir.
- Proxy düzenleme formunda durum kodunu, yeniden deneme süresini ve özel yanıt metnini ayarlayabileceğiniz bir **Bakım Modu** kartı yer alır.

```toml
[my-app]
protocol = "http"
vhosts = ["app.example.com"]
maintenance = { enabled = true, status = 503, retry_after = 300, body = "Sistem bakımı devam etmektedir. Lütfen kısa süre sonra tekrar deneyin." }
routes = [{ path = "/", servers = [{ url = "http://127.0.0.1:8080/" }] }]
```

## Özel Hata Sayfaları

Bir HTTP / HTTPS proxy'si, standart HTTP hata durumları (400 ile 599 arası, örn. `404 Not Found` veya `502 Bad Gateway`) için özel HTML hata sayfaları tanımlayabilir. Upstream sunucu bu durum kodunu döndürdüğünde veya r3v3rs3 bir ağ geçidi hatası ürettiğinde (örneğin upstream sunucuya ulaşılamadığında ya da hiçbir route eşleşmediğinde), r3v3rs3 hatayı yakalayarak `Content-Type: text/html; charset=utf-8` ve `Cache-Control: no-store` başlıklarıyla özel HTML sayfanızı döndürür.

- `status` (tamsayı, 400–599): HTTP hata durum kodu.
- `body` (metin, en fazla 64 KB): Hata sayfası için özel HTML içeriği.

### Görsel Hata Sayfası Tasarımcısı & Canlı Önizleme

WebUI üzerinde, HTTP proxy yapılandırma sayfasındaki **Özel Hata Sayfaları** bölümü gelişmiş ve etkileşimli bir tasarımcı sunar:

- **Hazır HTML/CSS Şablon Kütüphanesi**: Tek tıkla uygulanabilen, responsive, karanlık mod uyumlu ve harici bağımlılık gerektirmeyen şablonlar:
  - **Minimalist Koyu**: Radyal ışıltı, durum rozeti ve modern butonlarla şık koyu tema.
  - **Modern Cam**: Canlı degradeli tipografi ve zarif buzlu cam (glassmorphism) kart tasarımı.
  - **Geliştirici Terminali**: Simüle edilmiş HTTP başlıkları, teşhis satırları ve yanıp sönen imleç içeren CLI konsol teması.
  - **İllüstrasyonlu Kart**: Temiz dahili vektör SVG grafiği ve samimi kullanıcı mesajları içeren ortalanmış modern kart.
  - **Bakım Bildirimi**: Dişli çark illüstrasyonu ve canlı durum rozeti içeren kehribar bakım uyarısı.
- **Gerçek Zamanlı Canlı Önizleme**: Siz kodu düzenledikçe anında güncellenen yalıtılmış ve korumalı (sandboxed) iframe önizleme penceresi.
- **Üç Görünüm Modu**: **HTML/CSS Kodu** editörü, **Canlı Önizleme** veya **Yan Yana Görünüm** (kod editörü ve canlı önizleme aynı anda).
- **Duyarlı Görüntüleme Alanı (Viewport) Kontrolleri**: Tasarımı **Masaüstü (%100)**, **Tablet (768px)** ve **Mobil (375px)** genişliklerinde test edebilme.
- **Hızlı Kod Kısayolları**: Sık kullanılan durum kodlarını (`404`, `502`, `503`, `500`, `403`, `429`) tek tıkla seçme butonları.
- **Canlı Boyut Sayacı**: Kodun boyutunu 64 KB sınırına göre anlık olarak gösteren bayt sayacı.

```toml
[my-app]
protocol = "http"
vhosts = ["app.example.com"]
error_pages = [
  { status = 404, body = "<h1>Sayfa Bulunamadı</h1><p>Aradığınız sayfa mevcut değil.</p>" },
  { status = 502, body = "<h1>Hizmet Kullanılamıyor</h1><p>Sunucularımız şu anda yanıt vermiyor.</p>" },
]
routes = [{ path = "/", servers = [{ url = "http://127.0.0.1:8080/" }] }]
```

## UDP oturumları

UDP proxy her istemci adresi için ayrı bir oturum açar. Her oturumun upstream sunucuya giden kendi socket'i vardır. Bu yüzden upstream sunucu her istemciyi farklı bir kaynak porttan görür. r3v3rs3 upstream sunucunun yanıtlarını dinlediği porttan istemciye geri gönderir.

İki yönde de `session_idle_timeout` süresince (varsayılan `60s`) paket geçmezse oturum kapanır. Upstream socket hata verdiğinde veya portun upstream sunucuları ya da boşta kalma timeout'u değiştiğinde de oturum kapanır. İstemcinin sonraki paketi yeni bir oturum açar. Bir port en fazla 10.000 oturum tutar. Bu sınıra ulaşılınca r3v3rs3 yeni istemcilerin paketlerini düşürür.

`max_datagram_size`, UDP paketleri için bayt cinsinden maksimum yük boyutunu belirler (512 ile 65527 arasında, varsayılan `65527`). Bu boyuttan büyük paketler kırpılır.

## Upstream timeout'ları

r3v3rs3 upstream sunucuyu sınırlı bir süre bekler. Timeout değerlerini `500ms`, `10s` veya `1m` gibi yazın.

- HTTP / HTTPS proxy'sinde `timeouts.connect`, TCP / TLS üzerinden TCP proxy'sinde `connect_timeout` değeri yeni bir upstream bağlantısının DNS sorgusunu, TCP bağlantısını ve TLS handshake'ini sınırlar. Varsayılan değer `10s`'dir.
- HTTP / HTTPS proxy'sinde `timeouts.request` değeri, istek başladıktan yanıt header'ları gelene kadar geçen süreyi sınırlar. Yeni bağlantının kurulma süresi de buna dahildir. Varsayılan değer `60s`'dir. `0s` limiti kapatır. Yanıt gövdesi, WebSocket ve diğer upgrade edilmiş bağlantılar için limit yoktur.
- Bir route, proxy'nin timeout'ları yerine kendi `timeouts` değerini kullanabilir. Route'ta yazılmayan değer proxy değerini değil, varsayılan değeri alır.
- UDP proxy'sinde `session_idle_timeout` değeri boşta kalan istemci oturumunu kapatır. "UDP oturumları" bölümüne bakın.

Timeout dolduğunda HTTP istemcisi 504 Gateway Timeout alır, TCP istemcisinin bağlantısı kapanır. r3v3rs3 sıfır olan bağlantı timeout'u ve oturum boşta kalma timeout'u değerlerini reddeder.

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

## İstek gövdesi boyutu

HTTP / HTTPS proxy'sinde `max_body_size` değeri istek gövdesini byte cinsinden sınırlar. Varsayılan değer `0`'dır. `0` limiti kapatır. Bir route, proxy değeri yerine kendi `max_body_size` değerini kullanabilir. Route'taki `0` değeri o route için limiti kapatır.

r3v3rs3 `Content-Length` header'ını kimlik doğrulamadan önce kontrol eder. Bu yüzden limitten büyük bir istek 413 Payload Too Large alır ve upstream sunucuya ulaşmaz. r3v3rs3, `Content-Length` taşımayan bir gövdeyi (örneğin chunked gövde) upstream sunucuya gönderirken sayar. Gövde, upstream sunucu yanıt vermeden limiti geçerse r3v3rs3 upstream isteğini durdurur ve istemci 413 alır. Upstream sunucu böyle bir gövdenin başını alabilir. Limit HTTP/1.1, HTTP/2 ve HTTP/3 isteklerine uygulanır.

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

## İstek yansıtma

Route'un `mirror` değeri route isteklerinin bir kopyasını başka sunuculara gönderir. Örneğin yeni bir sürümü gerçek isteklerle denemek için kullanılır. r3v3rs3 yansıtma sunucularının yanıtlarını atar. Kopyayı yeniden denemez, sağlık kontrolüne ve circuit breaker'a saymaz ve kopyayı beklemez. Bu yüzden istemci, route sunucusunun yanıtını önceki gibi alır.

- `servers` yansıtma sunucularının listesidir. Her sunucu her kopyayı alır. Ağırlığın etkisi yoktur. Kopyanın yolu, `rewrite` dahil, route sunucularının yoluyla aynı kurallara uyar.
- `percent` r3v3rs3'ün kopyaladığı isteklerin oranıdır. Değer `1` ile `100` (varsayılan) arasındadır.
- `max_body_size` r3v3rs3'ün kopyaladığı en büyük istek gövdesidir, byte cinsinden. Varsayılan değer `65536`'dır. Gövdesi daha uzun olan istek kopyalanmaz. r3v3rs3 her kopya için bellekte en fazla bu kadar byte tutar.

r3v3rs3 kopyayı istek gövdesinin tamamını okuduktan sonra gönderir. Hata veren veya istemcinin tamamlamadığı gövde için kopya gönderilmez. Cache'ten gelen yanıt, WebSocket gibi upgrade istekleri ve route'ta 64 kopya gönderilirken gelen istek kopyalanmaz. Kopya, header kuralları uygulandıktan sonraki method ve header'ları taşır. Bu yüzden kimlik doğrulamanın kaldırmadığı cookie gibi kimlik bilgilerini de taşır. Bu verilere güvendiğiniz bir yansıtma sunucusu kullanın. r3v3rs3 `1` ile `100` dışındaki `percent` değerini reddeder.

```toml
[my-api]
protocol = "http"
vhosts = ["api.example.com"]
routes = [
  { path = "/", servers = [{ url = "http://127.0.0.1:9000/" }], mirror = { servers = [{ url = "http://127.0.0.1:9100/" }], percent = 10 } },
]
```

## Yük dengeleme ve sağlık kontrolü

Birden fazla upstream sunucusu olan proxy veya HTTP route, istekleri ve bağlantıları `load_balancing` değerine göre dağıtır:

- `round_robin` (varsayılan) sunucuları sırayla kullanır.
- `least_conn` yeni istek veya bağlantıları sunucu ağırlıklarını hesaba katarak en az aktif bağlantıya sahip sunucuya yönlendirir.
- `random` rastgele bir sunucu seçer.
- `first` ilk sağlıklı sunucuyu kullanır. Diğer sunucular yedektir.
- `client_ip_hash` istemciyi IP adresine göre seçer: o sunucu sağlıklı kaldıkça aynı istemci aynı sunucuya gider. HTTP proxy, belirlenen istemci IP adresini kullanır (bkz. [İstemci IP adresi](#istemci-ip-adresi)). Bir sunucu sağlıksız olursa, servis dışına alınırsa veya silinirse yalnız o sunucunun istemcileri diğer sunuculara geçer.

Her sunucunun `0` ile `65535` arasında bir `weight` değeri vardır (varsayılan `1`). `round_robin` her sunucuyu ağırlığı kadar kullanır. nginx'in smooth weighted round robin yöntemindeki gibi, bir sunucunun sıraları döngüye yayılır. `least_conn` sunucuları aktif bağlantı sayısının ağırlığa bölümüne göre (`aktif / ağırlık`) sıralar. Örneğin `3` ve `1` ağırlıklarında her dört isteğin üçü ilk sunucuya gider. `random` sunucuyu ağırlığıyla orantılı bir olasılıkla seçer. `client_ip_hash` her sunucuya, ağırlığıyla orantılı sayıda istemci adresi verir. `first` ağırlığı dikkate almaz. `weight = 0` olan sunucu, diğer bütün sunucular sağlıksız olsa da yeni istek veya bağlantı almaz. Bu yüzden bir sunucuyu silmeden servis dışına alabilirsiniz. Sunucunun açık TCP bağlantıları ve UDP oturumları devam eder. Her proxy'nin veya HTTP route'unun en az bir sunucusunun ağırlığı `0`'dan büyük olmalıdır.

HTTP proxy her istek için, TCP proxy her bağlantı için, UDP proxy her istemci oturumu için bir sunucu seçer. Her HTTP route'unun sunucuları ayrı bir gruptur.

TCP proxy seçilen sunucuya bağlanamazsa veya bağlantı timeout'u dolarsa r3v3rs3 bir kez sıradaki sunucuyu dener.

HTTP proxy, başarısız isteği yeniden deneme ayarlarına göre sıradaki sunucuya yeniden gönderir. `retry.attempts` (varsayılan `2`), bir isteğin ilk deneme dahil en fazla kaç kez gönderileceğini belirler ve `1` ile `10` arasında olmalıdır. `attempts = 1` yeniden denemeyi kapatır. `retry.retry_on` (varsayılan `["connect"]`) yeniden denemeyi başlatan hataları listeler:

- `connect`: bağlantı kurulamaz veya bağlantı timeout'u dolar. Sunucuya hiçbir şey ulaşmadığı için r3v3rs3 her method'u yeniden dener.
- `timeout`: istek timeout'u dolar.
- `http_502`, `http_503`, `http_504`: sunucu bu durum koduyla yanıt verir.

`timeout`, `http_502`, `http_503` ve `http_504` yalnız RFC 9110'daki idempotent method'ları yeniden dener: `GET`, `HEAD`, `OPTIONS`, `TRACE`, `PUT` ve `DELETE`. İstek timeout'u her denemeye ayrı uygulanır. Yeniden deneme, yük dengeleme yönteminin sırasındaki sıradaki sunucuya gider ve circuit'i açık sunucuyu atlar. Son denemeden sonra istemci son yanıtı veya hatayı alır.

Gövdesi olan istek yalnız gövde uzunluğu biliniyorsa (örneğin `Content-Length` ile) ve `retry.replay_body_limit` (varsayılan `0`) byte değerini aşmıyorsa yeniden denenir. r3v3rs3 bu gövdeyi bellekte tutar. Değer `0` ise yalnız gövdesi olmayan istekler yeniden denenir. WebSocket gibi upgrade istekleri yeniden denenmez. Bir route kendi `retry` ayarıyla proxy'nin yeniden deneme ayarlarını değiştirebilir.

Pasif sağlık kontrolü her sunucunun art arda aldığı hataları sayar. Hata, kurulamayan bir bağlantı veya yanıt gelmeyen bir istektir. `health_check.max_fails` (varsayılan `1`) kadar hatadan sonra sunucu `health_check.fail_timeout` (varsayılan `30s`) süresince sağlıksız sayılır. Sağlıksız sunucu yeni istekleri ve bağlantıları yalnız sağlıklı sunuculardan sonra alır. Bütün sunucular sağlıksızsa r3v3rs3 istekleri ve bağlantıları aynı sırayla yine onlara gönderir. Başarılı bir deneme hata sayısını sıfırlar. `max_fails = 0` kontrolü kapatır. 500 gibi hata durum kodlu bir HTTP yanıtı başarılı sayılır, çünkü sunucu yanıt vermiştir.

Aktif sağlık kontrolü, `health_check.interval` değeri `0s`'den büyükse çalışır (varsayılan `0s`, kapalı). r3v3rs3 her aralıkta her sunucuyu kontrol eder:

- `health_check.path` verilen HTTP proxy, her sunucunun kök adresinden bu yola `GET` gönderir. 2xx veya 3xx durum kodu kontrolü geçer. Yol `/` ile başlamalıdır ve yolu yalnız HTTP proxy kullanır.
- Yolu olmayan HTTP proxy ve TCP proxy her sunucuya TCP bağlantısı açar.
- UDP proxy her sunucunun host adını çözümler.

`health_check.timeout` (varsayılan `5s`) her kontrolü sınırlar. Kontrolü geçemeyen sunucu, bir kontrol başarılı olana kadar sağlıksız kalır. Bu kural `max_fails = 0` olduğunda da geçerlidir. Başarılı bir istek bu durumu bitirmez.

Sunucular, ağırlıkları, yük dengeleme yöntemi ve sağlık kontrolü ayarları değişmediği sürece ayarlar yeniden yüklendikten sonra sunucuların sağlık durumu korunur.

Durum API'si (`GET /api/proxies/{id}/status`), her upstream sunucusunun sağlık durumunu `upstreams` alanında listeler: adres, `weight`, `healthy`, art arda hata sayısı `failures` ve `last_error`. WebUI'daki proxy listesi sağlıklı sunucu sayısını gösterir ve durumları 10 saniyede bir yeniler. Sayının üzerine gelince görünen metin, sağlıksız sunucuları son hatalarıyla listeler.

[DNS SRV sunucuları](#dns-srv-sunuculari) olan bir HTTP proxy'si her SRV adını `srv` alanında da listeler: `name`, son başarılı sorgunun hedefleri `targets` (`host:port`), son sorgu başarısızsa `error` ve son başarılı sorgunun Unix epoch'tan bu yana saniye cinsinden zamanı `refreshed_at`. WebUI'daki proxy listesi, sorgusu başarısız olan her SRV adı için bir satır gösterir.

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

### DNS Tabanlı Upstream Yük Dengeleme

Bir upstream sunucusu birden fazla IP adresine çözümlenen bir ana makine adı kullandığında (ör. çoklu A/AAAA kayıtlı round-robin DNS, Docker Swarm servisleri, Kubernetes headless servisleri veya AWS Route53):

- **Otomatik Round Robin**: r3v3rs3 ardışık bağlantılar veya istekler için çözümlenen IP adresleri arasında round-robin sırasıyla rotasyon yapar.
- **DNS TTL Cache'i**: Çözümlenen IP adresleri, DNS kaydının TTL (Time To Live) süresi boyunca bellekte önbelleğe alınır; böylece her istekte gereksiz DNS sorgusu yapılmaz.
- **Dinamik Yeniden Çözümleme**: DNS TTL süresi dolduğunda r3v3rs3 otomatik olarak DNS'i yeniden sorgular. Eklenen, güncellenen veya kaldırılan backend IP adresleri, sunucu yeniden başlatılmadan veya proxy yapılandırması yeniden yüklenmeden dinamik olarak algılanır.
- **Failover**: HTTP ve HTTPS upstream'lerinde seçilen IP'ye bağlanılamazsa r3v3rs3 otomatik olarak rotasyon sırasındaki diğer çözümlenmiş IP'leri dener.
- **Protokol Desteği**: DNS tabanlı round-robin yük dengeleme ve TTL cache'i; HTTP/HTTPS upstream bağlantı havuzlarında, TCP proxy'lerde ve UDP proxy oturumlarında geçerlidir.
- **Negatif Cache**: Bir DNS sorgusu başarısız olursa (ör. NXDOMAIN veya DNS sunucu zaman aşımı), DNS sorgu döngülerini önlemek için hata 5 saniye boyunca önbelleğe alınır.

## Circuit Breaker

Circuit breaker, HTTP veya TCP proxy'nin hata veren bir upstream sunucusuna giden istekleri ve bağlantıları bir süre durdurur. Varsayılan olarak kapalıdır. Her HTTP route'undaki her sunucunun kendi circuit'i vardır. UDP proxy'lerde circuit breaker yoktur.

- HTTP isteği şu durumlarda başarısız sayılır: bağlantı kurulamaz, bağlantı timeout'u veya istek timeout'u dolar ya da sunucu 502, 503 veya 504 döner. TCP bağlantısı, kurulamazsa başarısız sayılır.
- r3v3rs3 her sunucunun isteklerini `circuit_breaker.window` (varsayılan `10s`) uzunluğundaki zaman pencerelerinde sayar. Bir zaman penceresi en az `circuit_breaker.min_requests` (varsayılan `20`) istek içeriyorsa ve bunların en az `circuit_breaker.failure_ratio` (varsayılan `50`) yüzdesi başarısız olduysa circuit açılır.
- Açık circuit, `circuit_breaker.open_duration` (varsayılan `30s`) dolana kadar istek veya bağlantı almaz. Sonra circuit yarı açık olur ve sunucuya tek bir deneme isteği gider. Başarılı deneme circuit'i kapatır. Başarısız deneme circuit'i yeniden açar.
- Bir HTTP route'unun bütün sunucularının circuit'i açıksa istemci, sunucuya istek gitmeden 503 Service Unavailable alır. TCP istemcisinin bağlantısı kapanır.

Pasif sağlık kontrolü 5xx yanıtını yine başarılı sayar, çünkü sunucu yanıt vermiştir. Durum API'si circuit'i kapalı olmayan sunucu için `"circuit": "open"` veya `"circuit": "half_open"` gösterir. WebUI'daki proxy listesi böyle bir sunucuyu sağlıklı saymaz ve sayının üzerine gelince görünen metinde circuit durumunu yazar.

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

`sticky`, HTTP proxy'nin her istemcisini bir cookie ile tek bir upstream sunucusunda tutar. Varsayılan olarak kapalıdır. Bireysel bir HTTP route'u da `route.sticky` ile kendi sticky session ayarını tanımlayabilir veya proxy ayarını geçersiz kılabilir.

- İstemciye giden ilk yanıt `sticky.name` (varsayılan `r3v3rs3_affinity`) cookie'sini yazar. Cookie değeri proxy'nin, route'un ve sunucu URL'inin HMAC-SHA256 imzasıdır. Bu yüzden istemci sahte bir değerle sunucu seçemez. Route'un hiçbir sunucusuna uymayan değer dikkate alınmaz.
- Geçerli cookie taşıyan istek, sunucu sağlıklı olduğu ve circuit'i açık olmadığı sürece aynı sunucuya gider. `weight = 0` olan sunucu sticky istemcilerini korur, böylece servis dışına alınan sunucu mevcut oturumlarını tamamlar. Aksi halde sunucuyu `load_balancing` seçer ve yanıt yeni bir cookie yazar. Başka bir sunucuya giden yeniden deneme de yeni bir cookie yazar.
- r3v3rs3 cookie'yi istekten siler, bu yüzden upstream sunucu cookie'yi almaz.
- Cookie route'un `Path` değerini, `HttpOnly` ve `SameSite=Lax` özelliklerini taşır. HTTPS ve HTTP/3'te `Secure` de eklenir. `sticky.max_age`, `Max-Age` değerini belirler. `max_age` yoksa cookie tarayıcı kapanınca silinir.
- Ad, RFC 6265'teki cookie token kuralına uymalıdır: boşluk ve ayırıcı karakter içermeyen görünür ASCII karakterler.
- r3v3rs3 her başlangıçta yeni bir imza key'i üretir. Yeniden başlatmadan sonra eski cookie'ler geçersiz olur ve her istemci sunucusunu yine `load_balancing` ile alır.
- Cache'ten gelen yanıt cookie yazmaz.

TCP veya UDP proxy ve cookie saklamayan istemciler bunun yerine `load_balancing = "client_ip_hash"` kullanabilir.

```toml
[my-app]
protocol = "http"
vhosts = ["app.example.com"]
sticky = { enabled = true, name = "app_server", max_age = "1h" }
routes = [
  { path = "/", servers = [{ url = "http://10.0.0.1:9000/" }, { url = "http://10.0.0.2:9000/" }] },
]
```

## İstemci IP adresi

CDN veya yük dengeleyici arkasında r3v3rs3'ün TCP peer'ı ziyaretçi değil, edge sunucusudur. r3v3rs3 gerçek istemci IP adresini yalnız peer güvenilirse belirler:

- **Bilinen CDN'ler**: Cloudflare, Fastly, Amazon CloudFront, Bunny CDN, Gcore, KeyCDN, Imperva ve Google Cloud Load Balancing. Bu özellik varsayılan olarak açıktır. Proxy ayarlarındaki "Bilinen CDN'lerin İstemci IP Header'larına Güven" seçeneğiyle kapatabilirsiniz.
- **Güvenilen Proxy'ler**: Proxy'ye eklediğiniz IP adresleri veya CIDR blokları, örneğin yerel bir yük dengeleyici.

Peer güvenilirse r3v3rs3 istemci IP adresini şu sırayla okur:

1. Sağlayıcı header'ı: Cloudflare için `CF-Connecting-IP`, Bunny CDN için `X-Real-IP`, Amazon CloudFront için `CloudFront-Viewer-Address`.
2. `X-Forwarded-For` ve RFC 7239 `Forwarded` header'ı içinde güvenilen bir proxy'ye veya bilinen bir CDN edge'ine ait olmayan en sağdaki adres. `Forwarded` içindeki `for=` elemanları, portlu ve tırnaklı IPv6 adresleri dahil, aynı zincire katılır.

r3v3rs3 bulduğu adresi upstream sunucuya `X-Real-IP` header'ında gönderir. Gelen `X-Forwarded-For` ve `Forwarded` zincirlerine dokunmaz. Peer güvenilir değilse r3v3rs3 `Forwarded`, `X-Forwarded-For`, `X-Real-IP`, `CF-Connecting-IP`, `True-Client-IP`, `CloudFront-Viewer-Address`, `Fastly-Client-IP` ve `Incap-Client-IP` header'larını siler, çünkü istemci bu header'lara sahte değer yazabilir.

CDN IP aralıkları binary'ye gömülüdür ve r3v3rs3 bu listeyi her gün yeniden indirir. Son indirilen liste yapılandırma dizinindeki `cdn-ranges.json` dosyasına yazılır. İndirme başarısız olursa r3v3rs3 son başarılı listeyi kullanmaya devam eder. Listenin durumunu "Ayarlar" bölümünde görebilir, "Şimdi Yenile" butonuyla listeyi hemen yenileyebilirsiniz.

Akamai edge IP aralıklarını yayınlamaz. Akamai kullanıyorsanız Site Shield aralıklarınızı "Güvenilen Proxy'ler" listesine ekleyin.

```toml
[my-app]
protocol = "http"
client_ip = { trust_cdn = true, trusted_proxies = ["10.0.0.0/8"] }
```

## IP filtresi

Her HTTP / HTTPS proxy'sinde istemcileri IP adresine göre engelleyebilir veya yalnız belirli adreslere izin verebilirsiniz. Filtre, "İstemci IP adresi" bölümünde belirlenen adrese bakar. Bu yüzden CDN veya güvenilen bir proxy arkasında da doğru çalışır.

- **Engellenen IP Adresleri**: Bu IP adreslerinden veya CIDR bloklarından gelen istemciler `403 Forbidden` alır.
- **İzin Verilen IP Adresleri**: Liste boş değilse proxy'ye yalnız bu IP adreslerinden veya CIDR bloklarından gelen istemciler erişebilir. Diğer istemciler `403 Forbidden` alır.
- **İzin Verilen Ülkeler**: Virgülle ayrılmış ISO 3166-1 alpha-2 ülke kodları (ör. `TR`, `US`, `DE`). Ayarlar sayfasında veya `config.toml` içinde bir MaxMind GeoIP veritabanı tanımlanmış olmalıdır (`[geoip] database = "/path/to/GeoLite2-Country.mmdb"`).
- **Engellenen Ülkeler**: Virgülle ayrılmış ISO ülke kodları. Engellenen ülkeler izin verilenlerin önündedir.

Engellenen bir adres veya ülke, izin verilenin önüne geçer. İzin verilen CIDR blokları (ör. yerel ağ veya VPN), ülke filtresinden önce değerlendirilerek erişimin kesilmesini önler.

### GeoIP Kural Sihirbazı & Bölgesel Şablonlar

WebUI üzerinde ülke bazlı erişim politikası, proxy ayarları, route geçersiz kılmaları ve erişim listeleri (access lists) sayfalarında yer alan görsel **GeoIP Kural Sihirbazı** kartıyla yönetilir:

- **Uygulama Modları**:
  - `Devre Dışı (Off)`: Ülke filtresi uygulanmaz. İstekler yalnızca IP adres blokları ve CIDR kurallarıyla denetlenir.
  - `İzin Listesi (Yalnızca Seçilenler)`: Yalnızca belirlenen ülkelerden gelen ziyaretçilere izin verilir; diğer ülkeler `403 Forbidden` yanıtı alır.
  - `Engelleme Listesi (Seçilenleri Engelle)`: Belirlenen ülkelerden gelen trafik anında `403 Forbidden` ile kesilir; diğer tüm ülkeler kabul edilir.
  - `Gelişmiş (İzin Ver & Engelle)`: Hem izin verilen hem engellenen ülke listelerini aynı anda yapılandırma imkanı sunar (engellenen ülkeler önceliklidir).
- **Tek Tıkla Bölgesel Şablonlar**:
  - Yaygın coğrafi ülke gruplarını tek tıkla yükleyin: **Türkiye** (`TR`), **Avrupa Birliği** (27 üye ülke), **Kuzey Amerika** (`US`, `CA`, `MX`), **G7 Ülkeleri**, **DACH** (`DE`, `AT`, `CH`), **APAC**, **LATAM**, **MENA**, **Nordic** veya **Temizle**.
- **Görsel Ülke Arama & Rozetler (Chips)**:
  - ISO alpha-2 ülke kodu, Türkçe veya İngilizce ülke adına göre anında arama ve otomatik tamamlama.
  - Seçilen ülkeler bayrak/kod ve yerelleştirilmiş ülke adıyla kaldırılabilir rozetler halinde listelenir.
- **Doğrudan CSV Girişi**:
  - Açılır-kapanır metin kutusu ile virgülle ayrılmış ISO ülke kodlarını toplu olarak kopyalama veya yapıştırma desteği.

Bir route, "Bu Route için Ayrı IP Filtresi Kullan" seçeneğiyle proxy listeleri yerine yalnız kendi listelerini kullanır. Route'un iki listesi de boşsa bu route'a her istemci erişebilir.

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

## Bot Engelleme (Yapay Zeka Kazıyıcı & Bot Koruması)

Web sitelerinizi ve uygulamalarınızı otomatik yapay zeka kazıyıcılarından (AI scraper), büyük dil modeli (LLM) eğitim botlarından ve veri madencilerinden tek bir anahtarla koruyabilirsiniz.

Bir HTTP / HTTPS proxy'sinde **AI Kazıyıcıları & Botları Engelle** (`bot_blocker = true`) etkinleştirildiğinde, r3v3rs3 gelen her isteğin `User-Agent` başlığını sıfır bellek tahsisli (zero-allocation) akış eşleyicisiyle denetler. Bilinen yapay zeka botları ve kazıyıcılar tespit edildiğinde istek upstream sunucunuza iletilmeden anında `403 Forbidden` (`error.bot_blocked`) ile reddedilir.

### Engellenen Yapay Zeka Botları ve Kazıyıcılar

Dahili engelleme listesi popüler ticari yapay zeka veri toplayıcılarını kapsar:
- **OpenAI**: `GPTBot`, `ChatGPT-User`, `OAI-SearchBot`
- **Anthropic**: `ClaudeBot`, `Claude-Web`, `anthropic-ai`
- **ByteDance / TikTok**: `Bytespider`
- **Common Crawl**: `CCBot`
- **Perplexity AI**: `PerplexityBot`
- **Cohere**: `cohere-ai`
- **Diffbot**: `Diffbot`
- **Meta**: `FacebookBot`, `Meta-ExternalAgent`
- **Google**: `Google-Extended`
- **Amazon**: `Amazonbot`
- **Genel Kazıyıcılar**: `Scrapy`, `SemrushBot`

### Route Düzeyinde Geçersiz Kılma

Bir route, "Bu Route için Bot Engelleyiciyi Özelleştir" seçeneğiyle proxy ayarını geçersiz kılabilir:
- Genel API veya webhook uç noktalarında bot engelleyiciyi kapatarak (`false`) otomatik istemcilere izin verebilirsiniz.
- Özel içerik veya hassas HTML sayfalarında proxy ayarı kapalı olsa dahi engelleyiciyi açabilirsiniz (`true`).

```toml
[my-proxy]
protocol = "http"
vhosts = ["example.com"]
bot_blocker = true
routes = [
  { path = "/", servers = [{ url = "http://127.0.0.1:8080/" }] },
  { path = "/api/public", servers = [{ url = "http://127.0.0.1:8080/api" }], bot_blocker = false },
]
```

## gRPC & gRPC-Web Proxy Desteği

r3v3rs3 yerel olarak HTTP/2 tabanlı gRPC trafiğini yönlendirir ve tarayıcı tabanlı web istemcilerinin arka plandaki gRPC servislerine sorunsuz bağlanabilmesi için otomatik **gRPC-Web proxy dönüştürme** (`grpc_web = true`) yeteneği sunar.

Web tarayıcıları doğrudan HTTP/2 çerçevelemesi yapamaz ve HTTP yanıt trailer başlıklarını (`grpc-status`, `grpc-message`) okuyamaz. `grpc_web` etkinleştirildiğinde:
- **CORS Ön Kontrolü (Preflight)**: Tarayıcılardan gelen `OPTIONS` preflight isteklerini gerekli `Access-Control-Allow-Origin`, `Access-Control-Allow-Methods`, `Access-Control-Allow-Headers` (`content-type,x-grpc-web,x-user-agent`) ve `Access-Control-Expose-Headers` (`grpc-status,grpc-message`) başlıklarıyla anında yanıtlar.
- **İstek Dönüştürme (Transpilation)**: Tarayıcıdan gelen `application/grpc-web` ve `application/grpc-web+proto` isteklerini standart `application/grpc` protokolüne dönüştürür ve upstream servise `TE: trailers` başlığını ekler.
- **Trailer Çerçeveleme (Framing)**: Upstream gRPC sunucusunun döndürdüğü `grpc-status` ve `grpc-message` trailer başlıklarını HTTP yanıt akışının sonuna standart gRPC-Web trailer çerçevesi (`0x80` bayt bayrağı + 4-bayt uzunluk + başlık metni) olarak ekler.
- **HTTP/2 Prior Knowledge (h2c)**: Düz HTTP gRPC sunucularına bağlanırken otomatik olarak HTTP/2 prior knowledge (`h2c`) kullanır.

`grpc_web` proxy genelinde tanımlanabilir veya rota (route) bazında geçersiz kılınabilir:

```toml
[grpc-service]
protocol = "https"
vhosts = ["grpc.example.com"]
grpc_web = true
routes = [
  { path = "/helloworld.Greeter/", servers = [{ url = "http://127.0.0.1:50051/" }] },
]
```

### gRPC Server Reflection ve İnteraktif Test Konsolu

r3v3rs3, upstream servisler için dahili **gRPC Server Reflection** (`grpc.reflection.v1` ve `grpc.reflection.v1alpha`) desteği ile WebUI ve Admin REST API üzerinden kullanılabilen interaktif bir **Test Konsolu** sunar.

#### Özellikler
- **Otomatik Servis Keşfi**: Yerel `.proto` dosyalarına ihtiyaç duymadan upstream gRPC sunucularında kayıtlı servisleri, metotları, istek ve yanıt mesaj şemalarını Server Reflection protokolü ile otomatik olarak keşfeder.
- **İnteraktif Metot Çağrısı (Invoke)**: JSON formatında istek gövdesi (payload) ve özel metadata başlıkları (`authorization: Bearer ...`) ile doğrudan unary gRPC metotlarını tetikler.
- **Detaylı Analiz**: Çağrı sonucunda gRPC durum kodu (`OK (0)`, `NOT_FOUND (5)`, `UNAVAILABLE (14)`), hata mesajları, milisaniye cinsinden gidiş-dönüş gecikmesi (latency), biçimlendirilmiş yanıt JSON'ı ve HTTP/2 trailer başlıklarını görüntüler.
- **Entegre ve Bağımsız Kullanım**: Hem proxy yapılandırma ekranı içerisinden doğrudan upstream hedefine hem de bağımsız REST uç noktaları üzerinden herhangi bir gRPC sunucusuna yönelik çalıştırılabilir.

#### Admin API Uç Noktaları
- `POST /api/proxies/{id}/grpc/reflect`: Belirtilen proxy'nin upstream sunucusundan gRPC servislerini keşfeder.
- `POST /api/proxies/{id}/grpc/invoke`: Belirtilen proxy'nin upstream sunucusunda unary bir RPC çağrısı gerçekleştirir.
- `POST /api/grpc/reflect`: Herhangi bir hedef gRPC sunucusunda bağımsız servis keşfi yapar (`{"target_url": "http://127.0.0.1:50051"}`).
- `POST /api/grpc/invoke`: Hedef URL, servis, metot, JSON yükü ve metadata ile bağımsız unary çağrı yürütür.

## Rate limit

Her HTTP / HTTPS proxy'sinde bir istemci IP adresinin gönderebileceği istek sayısını sınırlayabilirsiniz. r3v3rs3 istekleri "İstemci IP adresi" bölümünde belirlenen adrese göre sayar.

- **İstek Sayısı**: Belirlenen süre içinde izin verilen istek sayısı. `0` limiti kapatır.
- **Süre**: Sayacın süresi: saniye, dakika veya saat.
- **Burst**: Bir istemcinin limit devreye girmeden art arda gönderebileceği istek sayısı. `0` girilirse "İstek Sayısı" değeri kullanılır.

Limiti aşan istemci, `Retry-After` header'ıyla birlikte `429 Too Many Requests` alır.

Bir route, "Bu Route için Ayrı Rate Limit Kullan" seçeneğiyle proxy limiti yerine kendi limitini kullanabilir. Bu seçeneği açmayan route'lar her istemci için ortak bir sayaç kullanır. Route ayarında istek sayısı `0` ise o route'ta limit uygulanmaz.

r3v3rs3 sayaçları bellekte tutar. Ayarlar değiştiğinde sayaçlar korunur; yalnız limitin kendisi değişirse sıfırlanır. Sunucuyu yeniden başlatmak da sayaçları sıfırlar.

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

## Eşzamanlı İstek Sınırı (Concurrency Limit)

Her HTTP / HTTPS proxy'sinde aynı anda işlenen (in-flight) eşzamanlı istek sayısını sınırlayabilirsiniz. Rate limit belirli bir zaman dilimindeki toplam istek sayısını kısıtlarken, eşzamanlı istek sınırı yavaş veya paralel isteklerin upstream sunucularını kilitlemesini önler.

- **Maksimum Eşzamanlı İstek**: İstemci başına izin verilen eşzamanlı aktif istek sayısı. `0` limiti kapatır.
- **Header (isteğe bağlı)**: İstekleri istemci IP'si yerine belirtilen bir istek başlığına göre (örneğin `x-api-key`, `authorization` veya `x-client-id`) gruplar. Boş bırakıldığında istekler "İstemci IP adresi" bölümünde belirlenen adrese göre gruplanır.

Limiti aşan istemci, anında `Retry-After: 1` header'ıyla birlikte `429 Too Many Requests` yanıtı alır. İstek tamamlandığında veya bağlantı kesildiğinde yuva otomatik olarak serbest bırakılır.

Bir route, "Bu Route için Ayrı Eşzamanlı İstek Sınırı Kullan" seçeneğiyle proxy limiti yerine kendi eşzamanlı sınırını belirleyebilir. Route ayarında maksimum istek sayısı `0` ise o route'ta eşzamanlılık sınırı uygulanmaz.

```toml
[my-proxy]
protocol = "http"
vhosts = ["example.com"]
concurrency_limit = { max = 20 }
routes = [
  { path = "/", servers = [{ url = "http://127.0.0.1:8080/" }] },
  { path = "/api", servers = [{ url = "http://127.0.0.1:8080/api" }], concurrency_limit = { max = 5, header = "x-api-key" } },
  { path = "/health", servers = [{ url = "http://127.0.0.1:8080/health" }], concurrency_limit = { max = 0 } },
]
```

## Kimlik doğrulama

Her HTTP / HTTPS proxy'sinde "Kimlik Doğrulama" bölümünden kimlik doğrulamayı zorunlu hale getirebilirsiniz. Bir route, "Bu Route için Ayrı Kimlik Doğrulama Kullan" seçeneğiyle proxy ayarı yerine kendi ayarını kullanabilir. O route'u bütün istemcilere açmak için "Yok" seçin.

r3v3rs3 kimlik doğrulamayı IP filtresinden, rate limit'ten ve HTTPS yönlendirmesinden sonra yapar. Bu sayede "HTTP'yi Otomatik Olarak HTTPS'e Yönlendir" seçeneği açıksa tarayıcı kimlik bilgilerini şifreli bağlantı üzerinden gönderir.

### Basic Auth

Geçerli kullanıcı adı ve parola göndermeyen istemciler `WWW-Authenticate: Basic realm="..."` header'ıyla birlikte `401 Unauthorized` alır. Tarayıcı bunun üzerine giriş penceresini açar.

- **Realm**: Tarayıcının giriş penceresinde gösterdiği ad. Boş bırakılırsa `r3v3rs3` kullanılır.
- **Kullanıcılar**: Doğrudan proxy üzerinde tanımlanan kullanıcı adları ve parolalar. Kullanıcı adında iki nokta üst üste bulunamaz.
- **Kullanıcı Listesi Dosyası (`user_file`)**: Disk üzerindeki bir kullanıcı listesi dosyasının mutlak yolu (`username:password` veya `username:$argon2id$...`, her satırda bir kullanıcı; boş satırlar ve `#` ile başlayan satırlar yoksayılır). Statik siteler veya harici kullanıcı listeleri için idealdir. Düz metin parolalar sabit sürede (constant-time) karşılaştırılır; ayrıca Argon2 hash'leri de desteklenir. Dosya diskte güncellendiğinde r3v3rs3 proxy'yi yeniden başlatmaya gerek kalmadan dosyayı otomatik olarak yeniden yükler.

r3v3rs3 `users` içinde tanımlanan parolaları argon2 hash olarak saklar; düz metin parolayı hiçbir zaman kaydetmez. Admin API hash'i döndürmez. Parolası olan kullanıcı için `password_set: true` döndürür. Parolayı değiştirmek istemiyorsanız parola alanını boş bırakın. r3v3rs3, isteği upstream sunucuya göndermeden önce `Authorization` header'ını siler.

Argon2 kasıtlı olarak CPU harcar. r3v3rs3 her kimlik bilgisini bir kez doğrular ve sonucu ayarlar değişene kadar bellekte tutar. Parola denemelerini sınırlamak için rate limit kullanın.

`proxies.toml` dosyasında `password_hash` yerine `password` yazabilirsiniz. r3v3rs3 başlarken bu değeri hash'e çevirir. Ayrıca `user_file` da belirtebilirsiniz:

```toml
[my-proxy]
protocol = "http"
vhosts = ["example.com"]
auth = { type = "basic", realm = "Staff", user_file = "/etc/r3v3rs3/users.txt" }
routes = [
  { path = "/", servers = [{ url = "http://127.0.0.1:8080/" }] },
  { path = "/health", servers = [{ url = "http://127.0.0.1:8080/health" }], auth = { type = "none" } },
]
```

### Bearer Token

İstemci, proxy'de tanımlı token'lardan birini `Authorization: Bearer <token>` header'ıyla göndermelidir. Token göndermeyen istemci `WWW-Authenticate: Bearer realm="r3v3rs3"` ile birlikte `401 Unauthorized` alır. Yanlış token gönderen istemci de aynı yanıtı alır; bu yanıtta ayrıca `error="invalid_token"` bulunur.

- **Ad**: Token'ı tanımak için verdiğiniz ad.
- **Token**: En az 16 karakterlik rastgele bir değer. Örneğin `openssl rand -hex 32` komutuyla üretebilirsiniz.

r3v3rs3 her token'ın SHA-256 özetini saklar; düz metin token'ı hiçbir zaman kaydetmez. Özetleri sabit sürede karşılaştırır. Admin API özeti döndürmez. Değeri olan token için `token_set: true` döndürür. Token'ı değiştirmek istemiyorsanız token alanını boş bırakın. r3v3rs3, isteği upstream sunucuya göndermeden önce `Authorization` header'ını siler. Bu yüzden bearer kimlik doğrulaması kullanan bir route'ta upstream sunucuya kendi bearer token'ı ulaşmaz.

`proxies.toml` dosyasında `token_hash` yerine `token` yazabilirsiniz. r3v3rs3 başlarken bu değeri özete çevirir.

```toml
[my-api]
protocol = "http"
vhosts = ["api.example.com"]
auth = { type = "bearer", tokens = [{ name = "ci", token_hash = "<sha-256 hex digest>" }] }
routes = [{ path = "/", servers = [{ url = "http://127.0.0.1:9000/" }] }]
```

### Forward Auth

r3v3rs3 her isteğe izin verilip verilmeyeceğini oauth2-proxy veya Authelia gibi harici bir servise sorar. Bu özellik nginx'teki `auth_request` gibi çalışır.

r3v3rs3 her istemci isteğinde doğrulama URL'ine bir `GET` isteği gönderir. Bu istek, bağlantı header'ları ve `Host` dışında istemcinin bütün header'larını taşır. Bunlara ek olarak şu header'lar da eklenir:

| Header               | Değer                                                       |
|----------------------|-------------------------------------------------------------|
| `X-Forwarded-Method` | İstemci isteğinin method'u.                                 |
| `X-Forwarded-Proto`  | `http` veya `https`.                                        |
| `X-Forwarded-Host`   | İstemci isteğinin host'u.                                   |
| `X-Forwarded-Uri`    | İstemci isteğinin yol ve sorgu kısmı.                       |
| `X-Forwarded-For`    | "İstemci IP adresi" bölümünde belirlenen istemci IP adresi. |

- **2xx yanıt**: r3v3rs3 isteği upstream sunucuya gönderir. "Kopyalanacak Yanıt Header'ları" alanında listelenen header'ları doğrulama yanıtından upstream isteğine kopyalar. r3v3rs3 önce istemci isteğindeki aynı adlı header'ları siler, böylece istemci bu header'ları kendisi gönderemez.
- **Diğer yanıtlar**: r3v3rs3 doğrulama yanıtını (durum kodu, header'lar ve en fazla 64 KiB gövde) istemciye gönderir. Bu sayede giriş sayfasına yapılan yönlendirmeler de çalışır.
- **`timeout` süresinde (varsayılan `10s`) yanıt gelmezse veya bağlantı hatası olursa**: İstemci `502 Bad Gateway` alır.

Doğrulama isteği, upstream istekleriyle aynı kök sertifikalara güvenir. Proxy'nin istemci sertifikasını da gönderir. Ayrıntılar için "Upstream istemci sertifikaları" bölümüne bakın.

```toml
[my-app]
protocol = "http"
vhosts = ["app.example.com"]
auth = { type = "forward", url = "http://127.0.0.1:4180/oauth2/auth", response_headers = ["X-Auth-Request-User"], timeout = "10s" }
routes = [{ path = "/", servers = [{ url = "http://127.0.0.1:9000/" }] }]
```

### Panel oturumu

İstemciler, yönetim panelinde kullandığınız r3v3rs3 hesaplarıyla giriş yapar. Kendi kimlik doğrulaması olmayan bir web uygulamasını korumak için bu yöntemi kullanabilirsiniz.

- Oturumu olmayan `GET` ve `HEAD` istekleri `302 Found` ile giriş sayfasına yönlendirilir. Giriş yapıldıktan sonra r3v3rs3 tarayıcıyı ilk istenen yola geri gönderir.
- Oturumu olmayan diğer istekler `401 Unauthorized` alır.

Bu kimlik doğrulamayı kullanan her route, kendi yolunun altında şu endpoint'leri sunar. `/` route'unun giriş sayfası `/.r3v3rs3/auth/login` adresindedir. `/admin` route'unda bu adres `/admin/.r3v3rs3/auth/login` olur.

| Endpoint               | Method | İşlem                                                                                       |
|------------------------|--------|---------------------------------------------------------------------------------------------|
| `.r3v3rs3/auth/login`  | `GET`  | Giriş formunu gösterir.                                                                     |
| `.r3v3rs3/auth/login`  | `POST` | Kullanıcı adını, parolayı ve TOTP kodunu kontrol eder, ardından oturum cookie'sini ayarlar. |
| `.r3v3rs3/auth/logout` | `POST` | Oturumu sonlandırır ve oturum cookie'sini siler.                                            |

TOTP kodu yalnız TOTP'si açık hesaplarda istenir. `r3v3rs3_session` cookie'si `HttpOnly` ve `SameSite=Lax` özelliklerini taşır; HTTPS ve HTTP/3 bağlantılarında `Secure` özelliği de eklenir. Cookie'nin `Domain` özelliği yoktur ve r3v3rs3 bir oturumu yalnız istemcinin giriş yaptığı host'ta kabul eder. r3v3rs3, isteği upstream sunucuya göndermeden önce oturum cookie'sini siler.

Yalnız proxy'yi görebilen hesaplar giriş yapar. Proxy listesi olan bir hesap yalnız listesindeki proxy'leri görür. r3v3rs3 hesabı her istekte kontrol eder. Hesap silinirse, hesap değişirse veya proxy hesabın listesinden çıkarılırsa oturum sona erer. r3v3rs3 oturumun hesabını kaydetmeye başlamadan önce açılan oturumlar geçersizdir. Bu durumda istemci yeniden giriş yapar.

`config.toml` dosyasındaki `[admin]` ayarları bu oturumlara da uygulanır:

- `session_expiry`: Oturumun geçerlilik süresi. En az 5 dakika olabilir.
- `max_login_attempts` ve `login_attempts_reset`: Her istemci IP adresi ve kullanıcı adı için başarısız giriş limiti. Engellenen istemci, sıfırlama süresi geçene kadar `429 Too Many Requests` alır.

r3v3rs3 oturumları bellekte tutar. Sunucu yeniden başladığında bütün oturumlar sona erer. Uygulamanıza çıkış butonu eklemek için şu formu kullanabilirsiniz:

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

### Dahili OIDC / OAuth 2.0

oauth2-proxy veya Authelia gibi harici bir proxy aracına ihtiyaç duymadan, uygulamalarınızı OpenID Connect ve OAuth 2.0 kimlik sağlayıcılarıyla (Google, GitHub, Keycloak, Authentik, Okta, GitLab vb.) doğrudan r3v3rs3 üzerinden koruyun.

- Korumalı bir adrese gelen doğrulanmamış bir istek, `/.r3v3rs3/auth/oidc/login` aracılığıyla sağlayıcının giriş sayfasına otomatik olarak yönlendirilir.
- Giriş yapıldıktan sonra sağlayıcı istemciyi `/.r3v3rs3/auth/oidc/callback` adresine geri yönlendirir. r3v3rs3 yetkilendirme kodunu token ile takas eder, kimliği doğrular, izin verilen e-posta listesini kontrol eder, güvenli bir oturum cookie'si (`r3v3rs3_oidc`) oluşturur ve kullanıcıyı asıl gitmek istediği URL'ye yönlendirir.
- Yetkilendirilen istekler upstream sunucuya iletilirken oturum cookie'si kaldırılır ve doğrulanmış kimliği içeren `X-Forwarded-User` ile `X-Forwarded-Email` header'ları eklenir.
- Kullanıcılar `/.r3v3rs3/auth/oidc/logout` adresine giderek diledikleri zaman oturumlarını kapatabilir.

| Alan                | Açıklama                                                                                                                                                                                                                 |
|---------------------|--------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------|
| `issuer_url`        | OIDC sağlayıcısının issuer URL'i (örneğin `https://accounts.google.com` veya `https://auth.example.com/realms/master`). Özel olarak girilmediğinde standart yetkilendirme ve token uç noktaları otomatik olarak çözülür. |
| `authorization_url` | İsteğe bağlı özel yetkilendirme (authorization) uç noktası URL'i.                                                                                                                                                        |
| `token_url`         | İsteğe bağlı özel token uç noktası URL'i.                                                                                                                                                                                |
| `userinfo_url`      | İsteğe bağlı özel kullanıcı bilgisi (userinfo) uç noktası URL'i (kullanıcı bilgisi doğrudan ID token içinde yer almadığında kullanılır).                                                                                 |
| `client_id`         | Sağlayıcıda kayıtlı OAuth 2.0 İstemci Kimliği (Client ID).                                                                                                                                                               |
| `client_secret`     | OAuth 2.0 İstemci Gizli Anahtarı (Client Secret, diskte şifreli saklanır).                                                                                                                                               |
| `scopes`            | Talep edilecek OAuth 2.0 kapsamları (varsayılan: `["openid", "email", "profile"]`).                                                                                                                                      |
| `allowed_emails`    | İzin verilen e-posta adresleri veya alan adı jokerleri (ör. `["admin@example.com", "*@sirket.com"]`). Boş bırakılırsa sağlayıcı tarafından doğrulanan her kullanıcıya izin verilir.                                      |

```toml
[internal-dashboard]
protocol = "http"
vhosts = ["dashboard.example.com"]
auth = { type = "oidc", issuer_url = "https://accounts.google.com", client_id = "your-client-id.apps.googleusercontent.com", client_secret = "your-secret", allowed_emails = ["*@sirket.com"] }
routes = [{ path = "/", servers = [{ url = "http://127.0.0.1:8080/" }] }]
```


## Erişim listeleri

Erişim listesi, bir IP filtresini ve bir kimlik doğrulamayı bir adla tutar. Birden fazla proxy ve route aynı erişim listesini kullanabilir. Liste değişince değişiklik yeniden başlatma olmadan hepsine uygulanır.

WebUI'daki "Erişim Listeleri" sayfası listeleri ekler, değiştirir ve siler. HTTP / HTTPS proxy'nin "Erişim Listesi" alanı proxy için bir liste seçer. Route'un "Erişim Listesi" alanı route için bir liste seçer. Yönetim API'sinde ve `proxies.toml` dosyasında `access_list` alanı bir proxy'nin veya route'un listesini ayarlar.

- Proxy'nin listesi, proxy'nin "IP Filtresi" ve "Kimlik Doğrulama" ayarlarının yerini alır.
- Route'un listesi, route'un IP filtresinin ve kimlik doğrulamasının yerini alır.
- Kendi listesi veya kendi ayarı olmayan route, proxy'nin ayarlarını kullanır. Proxy'nin listesi de bu ayarlara dahildir.
- Listesi olan proxy veya route kendi IP filtresini veya kimlik doğrulamasını ayarlayamaz. Yönetim API'si `400 access_list_conflict` döndürür.
- Bilinmeyen bir listeyi kullanan proxy `400 access_list_not_found` alır. Liste çalışma sırasında yoksa proxy veya route her istemciye `403 Forbidden` döndürür.
- Bir proxy'nin veya route'un kullandığı liste silinemez. Yönetim API'si `400 access_list_in_use` döndürür.

Yapılandırma dizinindeki `access_lists.toml` dosyası listeleri tutar. Dosya parola hash'lerini ve token özetlerini taşır. Bu yüzden r3v3rs3 dosyayı `0600` moduyla yazar. Yönetim API'si, proxy kimlik doğrulamasında olduğu gibi hash'leri döndürmez.

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

| Endpoint                        | İşlem                                                                                                   |
|---------------------------------|---------------------------------------------------------------------------------------------------------|
| `GET /api/access_lists`         | Erişim listelerini listeler.                                                                            |
| `POST /api/access_lists`        | Bir erişim listesi ekler.                                                                               |
| `PUT /api/access_lists/{id}`    | Bir erişim listesini değiştirir. Güncelleme yeni parola veya token taşımıyorsa mevcut hash'ler korunur. |
| `DELETE /api/access_lists/{id}` | Bir erişim listesini siler.                                                                             |

Her hesap listeleri okur. Listeleri yalnız admin veya proxy listesi olmayan editör değiştirir. Ayrıntılar için [Hesaplar](@/accounts.tr.md) sayfasına bakın.

## Header kuralları

Proxy'den geçen isteklerin ve yanıtların header'larını "Header Kuralları" bölümünden değiştirebilirsiniz. Bir route, "Bu Route için Ayrı Header Kuralları Kullan" seçeneğiyle proxy kuralları yerine kendi kurallarını kullanabilir.

Her satıra bir kural yazın:

| Kural                | İşlem                                               |
|----------------------|-----------------------------------------------------|
| `set Name: value`    | Header'ın mevcut değerlerini bu değerle değiştirir. |
| `append Name: value` | Mevcut değerleri koruyarak bir değer ekler.         |
| `remove Name`        | Header'ı siler.                                     |

Boş satırlar ve `#` ile başlayan satırlar atlanır.

- **İstek Header'ları**: Kurallar, r3v3rs3'ün upstream sunucuya gönderdiği isteğe uygulanır. r3v3rs3 önce `Forwarded`, `X-Forwarded-*` ve `Via` header'larını ayarlar, kurallar bundan sonra çalışır. Yani bir kural bu header'ları da değiştirebilir.
- **Yanıt Header'ları**: Kurallar, upstream yanıtı istemciye gitmeden önce uygulanır. Hata sayfaları, yönlendirmeler ve giriş sayfaları gibi r3v3rs3'ün kendi ürettiği yanıtlara uygulanmaz.

Değerlerde şu değişkenleri kullanabilirsiniz. Süslü parantez yazmak için `{{` ve `}}` kullanın.

| Değişken                    | Değer                                                                                                                |
|-----------------------------|----------------------------------------------------------------------------------------------------------------------|
| `{client_ip}`               | "İstemci IP adresi" bölümünde belirlenen istemci IP adresi.                                                          |
| `{host}`                    | İstenen host adı.                                                                                                    |
| `{scheme}`                  | `http` veya `https`.                                                                                                 |
| `{request_id}`              | Rastgele 32 karakterlik hex ID. Aynı isteğin istek ve yanıt kuralları aynı ID'yi kullanır.                           |
| `{route}`                   | Eşleşen route'un yolu, örneğin `/api`.                                                                               |
| `{client_cert_subject}`     | Doğrulanan istemci sertifikasının subject değeri, örneğin `CN=client.example.com`. İstemci sertifikası yoksa boştur. |
| `{client_cert_fingerprint}` | Doğrulanan istemci sertifikasının hex SHA-256 fingerprint'i. İstemci sertifikası yoksa boştur.                       |

İstemci aynı adlı bir header'ı kendisi de gönderebilir. İstemci sertifikası header'ları için `append` yerine `set` kullanın. Böylece kural istemcinin gönderdiği değeri değiştirir.

Kurallar `Connection`, `Content-Length`, `Host`, `Keep-Alive`, `Proxy-Connection`, `TE`, `Trailer`, `Transfer-Encoding` ve `Upgrade` header'larını değiştiremez, çünkü bu header'lar bağlantıyı ve mesajın çerçeve yapısını kontrol eder.

```toml
[my-app]
protocol = "http"
vhosts = ["app.example.com"]
headers = { request = [{ action = "set", name = "X-Request-Id", value = "{request_id}" }, { action = "remove", name = "X-Debug" }], response = [{ action = "set", name = "X-Frame-Options", value = "DENY" }, { action = "remove", name = "Server" }] }
routes = [{ path = "/", servers = [{ url = "http://127.0.0.1:9000/" }] }]
```

## Sıkıştırma

Proxy'den geçen yanıtları "Sıkıştırma" bölümünden sıkıştırabilirsiniz. Bir veya daha fazla encoding seçtiğinizde sıkıştırma açılır. Hiçbir encoding seçili değilse sıkıştırma kapalıdır.

| Encoding  | `Content-Encoding` | Varsayılan Seviye | Uyarlanabilir Seviyeler (< 8 KB / 8–64 KB / > 64 KB veya Stream) |
|-----------|--------------------|-------------------|------------------------------------------------------------------|
| Brotli    | `br`               | Kalite 4          | Kalite 5 / Kalite 4 / Kalite 3                                   |
| Zstandard | `zstd`             | Seviye 3          | Seviye 4 / Seviye 3 / Seviye 1 (En Hızlı)                        |
| Gzip      | `gzip`             | Seviye 6          | Seviye 9 (En İyi) / Seviye 6 / Seviye 1 (En Hızlı)               |

r3v3rs3 isteğin `Accept-Encoding` header'ını okur ve istemcinin kabul ettiği encoding'lerden `q` değeri en yüksek olanı seçer. Birden fazla encoding aynı `q` değerine sahipse `algorithms` listesindeki sıra geçerli olur. Panelde bu sıra, encoding'leri seçtiğiniz sıradır.

`adaptive_level` etkinleştirildiğinde (varsayılan `true`), r3v3rs3 yanıt boyutuna göre sıkıştırma kalitesini otomatik olarak optimize eder: küçük yanıtlar (< 8 KB) CPU maliyetinin önemsiz olduğu durumlarda en yüksek sıkıştırma oranını kullanır, büyük veya akışlı (stream) yanıtlar ise worker thread'lerin doymasını engellemek için maksimum verim (throughput) seviyelerini tercih eder. Ayrıca metin, JSON ve SVG içerikleri için minimum boyut eşiği dinamik olarak 256 byte'a indirilir.

r3v3rs3 bir yanıtı yalnız şu koşulların hepsi sağlanırsa sıkıştırır:

- Durum kodu `1xx`, `204 No Content`, `206 Partial Content` veya `304 Not Modified` değildir.
- Upstream sunucu yanıta encoding uygulamamıştır ve yanıtta `Content-Range` header'ı yoktur.
- `Cache-Control` içinde `no-transform` yoktur.
- `Content-Type` header'ındaki medya türü `mime_types` listesindedir ve doğası gereği zaten sıkıştırılmış ikili formatlardan (PNG, JPEG, WebP, AVIF, MP4, WOFF2 veya ZIP gibi) biri değildir. r3v3rs3 `text/event-stream` türünü hiçbir zaman sıkıştırmaz, çünkü sıkıştırma server-sent event'leri geciktirir.
- `Content-Length` değeri en az `min_size` (veya uyarlanabilir eşik) kadardır. `Content-Length` header'ı olmayan stream yanıtları da sıkıştırılır.

Bu koşulları sağlayan yanıtlarda r3v3rs3 `Vary` header'ına `Accept-Encoding` ekler. Yanıtı sıkıştırdığında `Content-Length` ve `Accept-Ranges` header'larını da siler, güçlü `ETag` değerini zayıf `ETag` değerine çevirir. Yanıt header kuralları sıkıştırmadan önce çalıştığı için bir kural `Cache-Control: no-transform` ayarlayarak sıkıştırmayı engelleyebilir.

| Ayar             | Varsayılan                                                                                                                                                                                                                                  | Açıklama                                                                                                                    |
|------------------|---------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------|-----------------------------------------------------------------------------------------------------------------------------|
| `algorithms`     | Boş. Sıkıştırma kapalıdır.                                                                                                                                                                                                                  | Tercih sırasına göre sıkıştırma algoritmaları listesi (`br`, `zstd`, `gzip`).                                               |
| `min_size`       | `1024` byte                                                                                                                                                                                                                                 | Sıkıştırmanın uygulanması için gereken minimum yanıt boyutu.                                                                |
| `mime_types`     | `text/*`, `application/javascript`, `application/json`, `application/manifest+json`, `application/wasm`, `application/xml`, `application/xhtml+xml`, `application/rss+xml`, `application/atom+xml`, `image/svg+xml`, `font/otf`, `font/ttf` | Sıkıştırmaya uygun medya türleri. Zaten sıkıştırılmış formatlar (PNG, JPEG, WebP, AVIF, WOFF2, ZIP) otomatik filtrelenir.   |
| `adaptive_level` | `true`                                                                                                                                                                                                                                      | Yanıt yük boyutuna göre sıkıştırma seviyesini otomatik ayarlar ve metin/JSON için minimum boyut eşiğini 256 byte'a düşürür. |
| `precompressed`  | `true`                                                                                                                                                                                                                                      | Önceden derlenmiş statik dosyaları (`.zst`, `.br`, `.gz`) sıfır CPU maliyetiyle doğrudan sunar.                             |

```toml
[my-app]
protocol = "http"
vhosts = ["app.example.com"]
compression = { algorithms = ["br", "zstd", "gzip"], min_size = 1024, mime_types = ["text/*", "application/json"], adaptive_level = true, precompressed = true }
routes = [{ path = "/", servers = [{ url = "http://127.0.0.1:9000/" }] }]
```

## Cache

Proxy'den geçen yanıtları "Cache" bölümünden bellekte saklayabilirsiniz. Cache'teki bir yanıt, upstream sunucuya gidilmeden doğrudan istemciye gönderilir. Her proxy'nin ayrı bir cache'i vardır ve proxy'nin bütün route'ları bu cache'i ortak kullanır.

| Ayar                     | Varsayılan          | Açıklama                                                                                                                                                                                                                   |
|--------------------------|---------------------|----------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------|
| `enabled`                | `false`             | Cache'i açar.                                                                                                                                                                                                              |
| `max_size`               | `67108864` (64 MiB) | Saklanan yanıtlar için byte cinsinden bellek limiti. Cache dolunca r3v3rs3 en az kullanılan yanıtları siler.                                                                                                               |
| `max_entry_size`         | `1048576` (1 MiB)   | Gövdesi bu değerden büyük yanıtlar saklanmaz.                                                                                                                                                                              |
| `default_ttl`            | `0s`                | `Cache-Control: max-age`, `s-maxage` veya `Expires` içermeyen yanıtın geçerlilik süresi. Değer `0s` ise r3v3rs3 böyle bir yanıtı yalnız `ETag` veya `Last-Modified` header'ı varsa saklar ve her istekte yeniden doğrular. |
| `stale_while_revalidate` | `0s`                | Süresi dolmuş yanıtın arka planda asenkron olarak yeniden doğrulanırken istemciye hemen sunulacağı (`X-Cache: STALE`) pencere. Upstream `Cache-Control` içindeki `stale-while-revalidate` direktifi önceliklidir.          |

r3v3rs3 cache'i yalnız `Range`, `Upgrade` ve `Cache-Control: no-store` içermeyen `GET` ve `HEAD` isteklerinde kullanır. `HEAD` isteğine saklanan `GET` yanıtı verilir. İstemci `Cache-Control: no-cache` veya `Pragma: no-cache` gönderirse r3v3rs3 cache'e bakmadan isteği upstream sunucuya iletir ve gelen yeni yanıtı saklar. Cache key, istenen host ile isteğin yol ve sorgu değerinden oluşur. Bu yüzden yük dengelemenin seçtiği upstream sunucu key'i değiştirmez.

r3v3rs3 bir yanıtı yalnız şu koşulların hepsi sağlanırsa saklar:

- Durum kodu `200`, `203`, `204`, `300`, `301`, `308`, `404`, `405`, `410`, `414` veya `501` değerlerinden biridir.
- `Cache-Control` içinde `no-store` veya `private` yoktur.
- Yanıtta `Set-Cookie` header'ı yoktur.
- Yanıtta `Vary: *` header'ı yoktur.
- İstekte `Authorization` header'ı varsa `Cache-Control` içinde `public`, `s-maxage` veya `must-revalidate` bulunur.
- Yanıtın geçerlilik süresi veya validator'ı vardır ve gövdesi `max_entry_size` değerini aşmaz.

Geçerlilik süresi için sırasıyla `s-maxage`, `max-age`, `Expires` ve `default_ttl` değerlerine bakılır. `Cache-Control: no-cache` bu süreyi sıfır yapar. Saklanan yanıtın yaşına upstream yanıtındaki `Age` header'ının değeri de eklenir.

Saklanan yanıtın süresi dolmuşsa ve `stale_while_revalidate` (veya upstream `stale-while-revalidate` header direktifi) süresi henüz aşılmamışsa, r3v3rs3 istemciye hemen bayat yanıtı sunar (`X-Cache: STALE`) ve arka planda cache'i tazelemek üzere bloklamayan asenkron bir yeniden doğrulama başlatır. Birden fazla eşzamanlı isteğin aynı anda upstream'e yüklenmesini (dogpiling) engellemek için arka plan doğrulamaları her cache key başına kilitlenir.

Saklanan yanıtın süresi dolmuşsa ve `stale_while_revalidate` penceresinin dışındaysa, yanıtta `ETag` veya `Last-Modified` header'ı varsa r3v3rs3 `If-None-Match` veya `If-Modified-Since` ile koşullu bir istek gönderir. Upstream sunucu `304 Not Modified` dönerse r3v3rs3 saklanan header'ları günceller ve cache'teki yanıtı gönderir. Validator'ı olan yanıt, süresi dolduktan sonra da bir saat cache'te kalır. Eşleşen `If-None-Match` veya `If-Modified-Since` header'ı gönderen istemci, cache'ten `304 Not Modified` alır.

Her cache key için tek bir yanıt saklanır. Yanıtın `Vary` header'ı istek header'larını listeliyorsa saklanan yanıt yalnız bu header'larda aynı değerleri gönderen isteklere verilir.

r3v3rs3, cache'i kullanabilecek isteklerdeki `Accept-Encoding` header'ını siler. Böylece upstream sunucu yanıtları encoding uygulamadan gönderir ve "Sıkıştırma" ayarları yanıtı her istemci için ayrıca sıkıştırır. Bu isteklere verilen yanıtlarda `X-Cache` header'ı bulunur: cache'ten gelen yanıt için `HIT`, upstream sunucudan gelen yanıt için `MISS`, arka plan doğrulaması sırasında sunulan yanıt için `STALE`. Cache'ten gelen yanıtta ayrıca `Age` header'ı vardır.

Cache'teki yanıtları temizlemek için:
- `DELETE /api/proxies/{id}/cache` (veya belirli bir yol ön ekiyle eşleşen kayıtları silmek için `DELETE /api/proxies/{id}/cache?path=/api/v1/`).
- `POST /api/proxies/{id}/cache/purge` isteği (isteğe bağlı JSON gövdesi `{"path": "/on-ek"}`).
- Doğrudan proxy adresine HTTP `PURGE` metodu ile istek (ör. `curl -X PURGE https://app.example.com/data.json`).
- Veya WebUI proxy listesindeki "Temizle" linkine tıklayın.

Saklanan yanıtlar, cache ayarları değişene veya sunucu yeniden başlatılana kadar bellekte kalır.

```toml
[my-app]
protocol = "http"
vhosts = ["app.example.com"]
cache = { enabled = true, max_size = 67108864, max_entry_size = 1048576, default_ttl = "5m", stale_while_revalidate = "1m" }
routes = [{ path = "/", servers = [{ url = "http://127.0.0.1:9000/" }] }]
```

## HTTP/2

r3v3rs3, HTTP ve HTTPS proxy'lerinde hem upstream hem de downstream bağlantılarda HTTP/2 destekler.

Downstream tarafında istemci destekliyorsa HTTP/2 otomatik olarak seçilir. Çoğu web tarayıcısı HTTP/2'yi yalnız TLS üzerinden kullanır, çünkü sunucunun HTTP/2 desteklediğini ALPN (Application-Layer Protocol Negotiation) ile öğrenir.

Upstream tarafında r3v3rs3, HTTPS sunucularına ALPN ile `h2` ve `http/1.1` önerir ve sunucunun seçtiği protokolü kullanır. Düz HTTP bağlantısında protokol seçimi yapılamadığı için düz HTTP sunucularına HTTP/1.1 ile bağlanılır. Proxy'nin düz HTTP sunucuları prior knowledge ile HTTP/2 (h2c) kabul ediyorsa `h2c = true` ayarlayın. WebSocket ve diğer upgrade istekleri her zaman HTTP/1.1 kullanır. Bir portta aynı istemci sertifikasını ve aynı bağlantı timeout'unu kullanan proxy'ler upstream bağlantılarını ortak kullanır. Bu yüzden tek bir HTTP/2 upstream bağlantısı birçok istemcinin isteklerini taşır.

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

r3v3rs3 her yanıtın `Alt-Svc` header'ını proxy'nin HTTPS ve QUIC portlarıyla değiştirir. Böylece tarayıcı HTTP/3'ü ilk HTTPS yanıtından öğrenir.

WebTransport desteklenmez.

## Upstream istemci sertifikaları

Upstream sunucu istemci sertifikası isteyebilir (mutual TLS). Sertifikayı HTTP / HTTPS proxy'sinin veya TCP / TLS üzerinden TCP proxy'sinin "İstemci Sertifikası" alanında seçin. Listede private key'i olan istemci sertifikaları görünür. Ayrıntılar için "İstemci sertifikaları" bölümüne bakın.

- Proxy sertifikayı, sertifika isteyen her TLS upstream sunucusuna gönderir. Düz HTTP veya düz TCP upstream sunucuları sertifikayı kullanmaz.
- HTTP / HTTPS proxy'sinde forward auth isteği de aynı sertifikayı gönderir.
- Sertifika yoksa, istemci sertifikası değilse veya private key'i yoksa r3v3rs3 proxy ayarını reddeder. Bir proxy'nin kullandığı istemci sertifikası silinemez.
- Sertifika geçersiz hale gelirse, örneğin yapılandırma dizininden silinirse, proxy sertifikasız bağlanmaz. HTTP / HTTPS proxy'si `502 Bad Gateway` döner, TCP proxy'si bağlantıyı kapatır.

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

TCP / TLS üzerinden TCP proxy'si istemci adresini upstream sunuculara gönderebilir. Sürümü "PROXY Protocol Gönder" alanında seçin. Bundan sonra her upstream bağlantısı bir PROXY protocol header'ı ile başlar. TLS upstream sunucusunda header TLS handshake'ten önce gider.

- Kaynak adres bağlantının istemci adresidir. Port PROXY protocol alıyorsa bu adres o header'daki adrestir. "PROXY protocol" bölümüne bakın.
- Hedef adres istemcinin bağlandığı adrestir.
- İki adresin ailesi farklıysa ikisi de IPv6 adresi olarak yazılır. IPv4 adresi IPv4-mapped IPv6 adresine dönüşür.
- Aktif sağlık kontrolü sürüm 2 `LOCAL` header'ı gönderir.

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

1. Kendinden imzalı bir sertifika oluşturun.
2. Bir dosyadan sertifika içe aktarın (yalnız PEM formatı).
3. Sertifikayı [ACME](https://letsencrypt.org/how-it-works/) ile otomatik alın.

r3v3rs3, TLS client hello mesajındaki SNI (Server Name Indication) değerine göre uygun sertifikayı otomatik olarak seçer. **Otomatik Wildcard SSL Paylaşımı** ile sunucuda bulunan bir wildcard sertifika (`*.example.com` gibi), proxy'ler ve platform uygulamalarındaki tüm eşleşen alt alan adlarını manuel sertifika seçimi yapmaya gerek kalmadan otomatik olarak korur. Yönetim API'si ayrıca kapsam durumunu raporlamak için `/api/certs/coverage` ve harici özel alan adlarının DNS CNAME yönlendirmesini doğrulamak için `/api/certs/verify_cname` uç noktalarını sunar.

## İstemci sertifikaları

TLS sunucusu, istemciyi doğrulamak için istemci sertifikası isteyebilir. "İstemci Sertifikaları" sekmesinde istemci sertifikasını iki yolla ekleyebilirsiniz:

1. Kendinden imzalı bir sertifika oluşturun ve sertifika türü olarak "İstemci Sertifikası" seçin. r3v3rs3 sertifikaya `clientAuth` extended key usage değerini ekler. Seçilen CA sertifikası sertifikayı imzalar.
2. Sertifika zincirini ve private key'i dosyadan içe aktarın (yalnız PEM formatı). İstemci sertifikası için private key gerekir.

Proxy, istemci sertifikasını upstream sunucularına gönderir. Ayrıntılar için "Upstream istemci sertifikaları" bölümüne bakın.

## Kök sertifikalar

Upstream sunucunuz sistemin güvenmediği sertifikalar kullanıyorsa bu sertifikaları kök sertifika deposuna eklemeniz gerekir. r3v3rs3, sistemin kök sertifikalarına ek olarak bu depodaki kök sertifikaların imzaladığı bütün sertifikalara da otomatik olarak güvenir.

Kendinden imzalı bir sertifika oluşturduğunuzda r3v3rs3 bir CA sertifikası da oluşturur ve onu kök sertifika deposuna ekler.

## Süre uyarıları

Sertifika listesi, "Sertifika Süre Uyarısı" (varsayılan `14days`) süresi içinde sona erecek sertifikayı "Süresi dolmak üzere" ile işaretler. Süresi dolmuş sertifikayı "Süresi doldu" ile işaretler.

## Bildirimler

r3v3rs3 şu olaylar için bildirim webhook'una bir JSON `POST` isteği gönderir:

| Olay                   | Ne zaman                                                                                                                        |
|------------------------|---------------------------------------------------------------------------------------------------------------------------------|
| `certificate_expiring` | Bir sertifika "Sertifika Süre Uyarısı" süresi içinde sona erer.                                                                 |
| `certificate_expired`  | Bir sertifikanın süresi dolmuştur.                                                                                              |
| `acme_order_failed`    | Bir ACME order başarısız olmuştur. Başarısız her order bir olay gönderir.                                                       |
| `test`                 | Bir admin test bildirimi göndermiştir.                                                                                          |
| `deployment_started`   | [Deploy platformunun](@/platform.tr.md) bir deployment'ı başlamıştır.                                                           |
| `deployment_running`   | Bir deployment bitmiştir ve container'ı çalışır.                                                                                |
| `deployment_failed`    | Bir deployment başarısız olmuştur. r3v3rs3 yeniden başlarken bitmemiş her deployment'ı başarısız sayar ve bu olayı da gönderir. |
| `agent_online`         | Bir agent master'a bağlanmıştır. Açık bir bağlantının yerini alan yeni bağlantı olay göndermez.                                 |
| `agent_offline`        | Bir agent'ın bağlantısı kopmuştur. Silinen veya yeniden kaydedilen hedef olay göndermez.                                        |
| `app_alert`            | Bir uygulama yapılandırılan CPU veya bellek uyarı eşiğini aşmıştır.                                                             |

```json
{"event": "certificate_expiring", "time": 1757894400, "node": "proxy-1", "certificate": {"id": "a1b2c3d", "san": ["example.com"], "not_after": 1759104000}}
{"event": "deployment_failed", "time": 1757894400, "deployment": {"id": "kfd-mzq", "app": "bxv-tpw", "app_name": "shop", "trigger": "webhook"}, "error": "the health check failed"}
{"event": "agent_offline", "time": 1757894400, "target": {"id": "fzn-txd", "name": "edge"}}
{"event": "app_alert", "time": 1757894400, "alert": {"app": "bxv-tpw", "app_name": "shop", "metric": "cpu", "current_value": "85.0%", "threshold": "80%"}}
```

- `time` saniye cinsinden Unix zamanıdır. `certificate` alanı sertifika olayının sertifikasını gösterir. `acme` alanı ACME kaydının `id` ve `identifiers` değerlerini taşır. `error` alanı `acme_order_failed` olayının hatasını açıklar.
- `deployment` alanı deployment'ı, uygulamasını ve tetikleyicisini (`manual`, `webhook`, `rollback` veya `api`) gösterir. `error` alanı `deployment_failed` olayının mesajını taşır. `target` alanı agent olayının agent hedefini gösterir. `alert` alanı `app_alert` olayının uygulama id'sini, adını, tetiklenen metriği (`cpu`, `memory_bytes` veya `memory_percent`), anlık değeri ve yapılandırılmış eşiği taşır. Aynı uygulama ve metrik için tekrarlanan uyarılar 5 dakikalık bekleme süresiyle sınırlandırılır.
- Token varsa istek `Authorization: Bearer <token>` header'ını taşır. Admin API token'ı döndürmez.
- 2xx durum kodu başarı sayılır. r3v3rs3 başarısız isteği en fazla üç kez gönderir: 1 saniye sonra bir kez daha, 2 saniye sonra bir kez daha. "Webhook Timeout" (varsayılan `10s`) her denemeyi sınırlar.
- r3v3rs3 sertifikaları her "Arka Plan Görevi Aralığı" süresinde kontrol eder. Bir sertifikanın her olayı bir kez gönderilir. Yenilenen sertifikanın fingerprint'i değişir. Bu yüzden onun olayları yeniden gönderilir. r3v3rs3 gönderilen olayları yapılandırma dizinindeki `notifications.json` dosyasında tutar.
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

Listedeki onay kutularıyla sertifikaları seçin ve "Seçilenleri Sil" butonuna tıklayın. `{"ids": [...]}` gövdesi ile gönderilen `POST /api/certs/delete` isteği en fazla 200 id için aynı işi yapar. Yanıt, istekteki sırayla her id için bir sonuç taşır:

| Sonuç       | Anlamı                                                                                 |
|-------------|----------------------------------------------------------------------------------------|
| `deleted`   | r3v3rs3 sertifikayı sildi.                                                             |
| `in_use`    | Bir port, proxy veya servis keşfi sağlayıcısı sertifikayı kullanıyor. Sertifika kalır. |
| `read_only` | Sertifikayı servis keşfi yönetiyor. Sertifika kalır.                                   |
| `not_found` | Bu id'ye sahip sertifika yok.                                                          |
| `failed`    | Depolama katmanı sertifikayı silmedi. Sunucu log'u nedeni yazar.                       |

İstekte tekrar eden bir id tek bir sonuç alır.

# ACME

r3v3rs3, sertifikaları [ACME](https://letsencrypt.org/docs/client-options/) (Automatic Certificate Management Environment) ile otomatik alabilir. Let's Encrypt, ZeroSSL, BuyPass ve Google Trust Services gibi birçok sertifika otoritesi ACME'yi destekler.

Bir ACME kaydı bir veya daha fazla alan adı içerir. Alan adlarını "Alan Adları" alanına virgülle ayırarak yazın, örneğin `example.com, *.example.com`. Sertifika her alan adını Subject Alternative Name olarak içerir. r3v3rs3 sertifikayı süresi dolmadan otomatik yeniler. Bir order başarısız olursa r3v3rs3 bir saat sonra yeniden order oluşturur. ACME sekmesindeki kaydın **Yeniden iste** işlemi ve `POST /api/acme/{id}/order`, kaydın sertifikalarını yenileme zamanını ve yeniden denemeyi beklemeden hemen ister.

Sertifika otoritesini seçin (Let's Encrypt, Google Trust Services, ZeroSSL, BuyPass, BuyPass Test veya "Sunucu URL'i" ile "Özel") ve "E-posta Adresi" girin. External Account Binding (EAB) kullanan sertifika otoriteleri (örneğin Google Trust Services veya ZeroSSL), hesabınızın "EAB Key ID" ve "EAB HMAC Key" değerlerini ister. r3v3rs3 yeni sertifikayı son sertifikadan "Yenileme Aralığı (Gün)" (`renewal_days`, varsayılan `60`) gün sonra ister.

## Challenge'lar

Sertifika otoritesi, her alan adını sizin yönettiğinizi bir challenge ile doğrular. Challenge'ı "Challenge" alanından seçin.

| Challenge   | Nasıl çalışır                                                                                                                                            | Gereksinimler                                                                                                                  |
|-------------|----------------------------------------------------------------------------------------------------------------------------------------------------------|--------------------------------------------------------------------------------------------------------------------------------|
| HTTP-01     | Sertifika otoritesi `http://<domain>/.well-known/acme-challenge/<token>` adresine istek gönderir ve r3v3rs3 yanıt verir.                                 | Her alan adı r3v3rs3'e çözümlenmeli, TCP 80 portu açık ve internetten erişilebilir olmalıdır. Wildcard alan adı kullanılamaz.  |
| TLS-ALPN-01 | Sertifika otoritesi, alan adının 443 portuna `acme-tls/1` ALPN protokolüyle bir TLS bağlantısı açar ve r3v3rs3 bir challenge sertifikasıyla yanıt verir. | Her alan adı r3v3rs3'e çözümlenmeli, TCP 443 portu açık ve internetten erişilebilir olmalıdır. Wildcard alan adı kullanılamaz. |
| DNS-01      | r3v3rs3, DNS sağlayıcınızın API'si ile `_acme-challenge.<domain>` TXT kaydını oluşturur.                                                                 | Aşağıdaki tablodaki DNS sağlayıcılarından biri ve zone'u düzenleyebilen bir API kimlik bilgisi.                                |

`*.example.com` gibi bir wildcard alan adı DNS-01 gerektirir. r3v3rs3, HTTP-01 veya TLS-ALPN-01 ile girilen wildcard alan adını reddeder.

TLS-ALPN-01 challenge'ı sürerken her TLS portu ve her HTTPS portu, yalnız `acme-tls/1` sunan bir istemciye challenge sertifikasıyla yanıt verir. Diğer istemciler portun sertifikasını alır. TLS portu, challenge bağlantısı için upstream sunucusuna bağlanmaz. Hiçbir TCP veya HTTP portu "TLS-ALPN Challenge Adresi" ayarındaki portu kullanmıyorsa r3v3rs3, challenge'lar bitene kadar bu adresi dinler. 443 portundaki TLS'siz bir HTTP portu challenge'a yanıt veremez.

## DNS-01

r3v3rs3 her alan adı için şu adımları uygular:

1. Sağlayıcının API'si ile alan adının zone'unu bulur. Adı içeren en uzun zone kullanılır.
2. `_acme-challenge.<domain>` TXT kaydını 60 saniyelik TTL ile oluşturur. Sağlayıcıya göre TTL farklıdır: Linode ve Gandi kaydı kendi en düşük değerleri olan 300 saniyeyle, deSEC ise alan adının minimum TTL'i olan 3600 saniyeyle yazar. Porkbun'a TTL gönderilmez, bu yüzden kayıt hesabın en düşük TTL değerini alır. Gandi, deSEC, Azure DNS ve Google Cloud DNS bir adın bütün kayıt kümesini yazar. Bu yüzden r3v3rs3 kendi değerlerini mevcut TXT değerlerine ekler ve yalnız kendi değerlerini siler. `*.example.com` için kayıt adı `example.com` ile aynıdır: `_acme-challenge.example.com`. Bu yüzden kayıt iki değer taşır.
3. TXT değerleri görünene kadar DNS'i 5 saniyede bir sorgular, en fazla 5 dakika bekler. Sorgulanan DNS sunucusunu "DNS Challenge Çözümleyicisi" ayarı belirler. Ayar boşsa r3v3rs3 sistemin çözümleyicisini kullanır.
4. Sertifika otoritesine challenge'ların hazır olduğunu bildirir ve doğrulama için en fazla 3 dakika bekler.
5. TXT kayıtlarını siler. Order başarısız olsa da kayıtları siler.

Sistemin çözümleyicisi cache'teki eski yanıtları döndürebilir. Yayılma kontrolü sık başarısız oluyorsa "DNS Challenge Çözümleyicisi" ayarına `1.1.1.1:53` gibi herkese açık bir çözümleyici veya zone'un yetkili (authoritative) DNS sunucusunu yazın.

| DNS sağlayıcısı  | Kimlik bilgileri                                                | Gereken izinler                                                                                                                                                                                                                                                                                   |
|------------------|-----------------------------------------------------------------|---------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------|
| Cloudflare       | API Token                                                       | Zone için `Zone:Read` ve `DNS:Edit`.                                                                                                                                                                                                                                                              |
| Route 53         | Access Key ID, Secret Access Key                                | `route53:ListHostedZones` ve `route53:ChangeResourceRecordSets`. Private hosted zone'lar atlanır.                                                                                                                                                                                                 |
| Azure DNS        | Tenant ID, Client ID, Client Secret, Subscription ID            | Zone'larda DNS Zone Contributor rolü olan bir service principal. r3v3rs3, subscription'daki DNS zone'larını listeler ve resource group'u zone ID'sinden alır.                                                                                                                                     |
| Google Cloud DNS | Service Account Key (JSON), Project ID                          | Zone'ların projesinde DNS Administrator rolü (`roles/dns.admin`) olan bir service account'un JSON key dosyası. Project ID boşsa r3v3rs3 key'in projesini kullanır. Private zone'lar atlanır.                                                                                                      |
| deSEC            | API Token                                                       | Hesabın bir token'ı. Policy ile sınırlanmış bir token, `_acme-challenge` TXT kayıt kümelerine yazma izni vermelidir.                                                                                                                                                                              |
| DigitalOcean     | API Token                                                       | Alan adlarını okuyabilen, alan adı kayıtlarını oluşturup silebilen bir token.                                                                                                                                                                                                                     |
| Gandi            | API Token                                                       | Alan adlarını okuyabilen ve LiveDNS kayıtlarını değiştirebilen bir personal access token.                                                                                                                                                                                                         |
| Hetzner Cloud    | API Token                                                       | Okuma ve yazma yetkisi olan bir Hetzner Cloud proje token'ı. Zone, Hetzner Cloud DNS'te olmalıdır.                                                                                                                                                                                                |
| Linode           | API Token                                                       | Domains için okuma ve yazma yetkisi olan bir personal access token.                                                                                                                                                                                                                               |
| Vultr            | API Key                                                         | Hesabın API key'i.                                                                                                                                                                                                                                                                                |
| Porkbun          | API Key, Secret API Key                                         | Porkbun alan adı yönetiminde alan adı için "API Access" açık olmalıdır.                                                                                                                                                                                                                           |
| OVHcloud         | API Endpoint, Application Key, Application Secret, Consumer Key | `GET /domain/zone`, `POST /domain/zone/*` ve `DELETE /domain/zone/*` yetkileri olan bir consumer key. Endpoint `ovh-eu`, `ovh-ca`, `ovh-us`, `kimsufi-eu`, `kimsufi-ca`, `soyoustart-eu` veya `soyoustart-ca` olabilir. r3v3rs3, kayıtları oluşturduktan sonra ve sildikten sonra zone'u yeniler. |
| Webhook          | Webhook URL, Bearer Token                                       | TXT kayıtlarını oluşturan ve silen kendi servisiniz. Aşağıdaki "DNS webhook" bölümüne bakın.                                                                                                                                                                                                      |
| Exec             | Program Yolu                                                    | r3v3rs3 host'unda TXT kayıtlarını oluşturan ve silen bir program. Aşağıdaki "DNS exec" bölümüne bakın.                                                                                                                                                                                            |
| RFC 2136         | DNS Sunucusu, Zone, TSIG Key Adı, TSIG Algoritması, TSIG Secret | Dinamik güncelleme (dynamic update) kabul eden bir DNS sunucusu, örneğin BIND, Knot DNS veya PowerDNS. Aşağıdaki "RFC 2136" bölümüne bakın.                                                                                                                                                       |

r3v3rs3 bu API'lerin mock sunucularıyla ve [Pebble](https://github.com/letsencrypt/pebble) test sertifika otoritesiyle test edilir. Gerçek sağlayıcı hesaplarıyla test edilmez.

## DNS webhook

Webhook sağlayıcısı TXT kayıtlarını bir DNS barındırma servisinin API'si yerine kendi servisinize gönderir. r3v3rs3 her challenge adı için webhook URL'ine bu JSON gövdesiyle bir `POST` isteği gönderir:

```json
{"action": "add", "fqdn": "_acme-challenge.example.com", "values": ["<TXT değeri>"]}
```

- `action` doğrulamadan önce `add`, doğrulamadan sonra `remove` olur. Bir `add` isteği başarısız olursa r3v3rs3 `remove` isteği de gönderir.
- `fqdn`, sondaki nokta olmadan TXT kaydının adıdır. `values`, adın bütün TXT değerlerini taşır.
- Servis, adın diğer TXT değerlerini korumalıdır.
- Bearer token varsa istek `Authorization: Bearer <token>` header'ını taşır.
- 2xx durum kodu başarı sayılır. r3v3rs3 yanıt için en fazla 30 saniye bekler.

Webhook URL'i HTTPS kullanmalıdır. HTTP yalnız loopback adresinde kullanılabilir, örneğin `http://127.0.0.1:8080/acme`.

## DNS exec

Exec sağlayıcısı her TXT değeri için r3v3rs3 host'unda bir program çalıştırır:

```
<program> add <fqdn> <değer>
<program> remove <fqdn> <değer>
```

- r3v3rs3 programı shell kullanmadan doğrudan başlatır. Program hiçbir ortam değişkeni almaz ve standart girdi almaz.
- `fqdn`, sondaki nokta olmadan TXT kaydının adıdır.
- Program, adın diğer TXT değerlerini korumalıdır.
- Çıkış kodu 0 başarı sayılır. Diğer her çıkış kodu başarısızlıktır ve hata mesajı standart hata çıktısının başını taşır.
- Bir `add` çağrısı başarısız olursa r3v3rs3, adın eklediği değerleri ve başarısız değer için `remove` çalıştırır.
- Timeout dolunca r3v3rs3 programı durdurur ve çağrı başarısız olur.

Program, exec DNS provider'ının izin listesinde olmalıdır. Ayarlar sayfası bu listeyi **Exec DNS Provider** kartında sunucu çalışırken tutar, `config.toml` aynı bölümü taşır:

```toml
[acme_exec]
programs = ["/usr/local/bin/r3v3rs3-dns-hook"]
timeout = "30s"
```

- Sağlayıcının programı ve `programs` listesindeki her giriş mutlak yol olmalıdır. r3v3rs3 sembolik link'leri çözer ve çözülen yolları karşılaştırır.
- r3v3rs3 programı ACME girişini eklediğinizde ve her çalıştırmadan önce kontrol eder.
- `timeout`, tek bir çağrının en uzun çalışma süresidir. Varsayılan değer `30s`.

## RFC 2136

RFC 2136 sağlayıcısı, zone'un birincil (primary) DNS sunucusuna dinamik güncelleme (dynamic update) gönderir. Her mesaj bir TSIG imzası taşır.

- **DNS Sunucusu**, birincil sunucunun `host:port` biçimindeki adresidir, örneğin `ns1.example.com:53`. r3v3rs3 mesajları TCP üstünden gönderir.
- **Zone**, adların zone'udur, örneğin `example.com`. Boş olursa r3v3rs3 her TXT adı için sunucudan SOA kaydını ister ve bu kaydın sahibi olan adı (owner name) kullanır.
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

## Worker Süreçleri

Varsayılan olarak admin API'yi çalıştıran aynı süreç her portu da servis eder. `config.toml` içinde `worker_mode = "workers"` ayarıyla r3v3rs3 portları sunan bir worker süreci başlatır. Master süreç durumu, admin API'yi, WebUI'yi ve deployment platformunu tutar. Worker'ın çökmesi admin API'yi düşürmez ve master worker'ı bir backoff ile yeniden başlatır.

```toml
worker_mode = "workers"
```

Master, güncel yapılandırmayı ve dinleme socket'lerinin descriptor'larını worker'a özel bir Unix socket üzerinden gönderir, bu yüzden worker'ın yapılandırma dizinine erişimi gerekmez. Worker'ın raporladığı durum, process id'si, uptime'ı ve açık bağlantı sayısı WebUI ayarlar sayfasında ve `GET /api/workers` üzerinde görünür. Yanıt vermeyi bırakan bir worker, durumu olmadan listede kalır.

## Kesintisiz Yükseltme

`kill -USR2 <pid>` dinleyicileri yeni bir sürece devreder. Eski süreç yeni binary'yi başlatır, yeni sürecin durumunu yüklemesini bekler, dinleme socket'lerinin descriptor'larını bir Unix socket üzerinden geçirir ve ardından açık bağlantılarını `shutdown_graceful_timeout` (varsayılan 30 saniye) süresince boşaltıp 0 çıkış koduyla çıkar.

Yeni süreç aynı dizinden yapılandırmayı okur, bu yüzden yükseltme `config.toml` içindeki her ayar için geçerli olur. `worker_mode = "workers"` ile portları worker'lar tuttuğu için yükseltme devredilecek bir şey bulmaz ve master'ın sıralı yeniden başlatması worker'ın açık bağlantılarına dokunmadan olur.

systemd ile `install.sh`, servisi `Type=notify` ve `ExecReload=/bin/kill -USR2 $MAINPID` olarak yazar; `systemctl reload r3v3rs3` yeniden başlatmadan yükseltir. Sunucu hazır olduğunda systemd'ye haber verir ve devir, ana süreci yeni sürece taşır, böylece eski süreç bağlantılarını boşaltırken systemd servisi takip etmeye devam eder.

Devir tek bir kurulumun içinde çalışır: eski süreç yeni binary'yi aynı makineden başlatır ve descriptor'ları ona geçirir. Docker yükseltmesi container'ın bütünüyle yeni image ile değiştirir, bu yüzden devredecek bir şey bulamaz ve bağlantılar her durumda yeniden başlar. Docker'da kesintisiz yükseltme için iki r3v3rs3 container'ını bir load balancer arkasında çalıştırın ve bunları sırayla güncelleyin. Container yeniden başlaması yine de açık bağlantıları `shutdown_graceful_timeout` süresince düzgün kapatır.

## Panelden yükseltme

Admin, shell'e girmeden yükseltebilir: "Ayarlar" sayfasında "Sunucu Yükseltmesi" kartı durur. Düğme en yeni GitHub release'ini okur, bu platformun arşivini seçer, release API'sinin bildirdiği sha256 özetini doğrular, çalışan binary'yi değiştirir ve dinleyicileri yukarıdaki kesintisiz yükseltme yoluyla yeni sürece devreder. Panel birkaç saniye sonra yeni sürüme yenilenir. Yükseltme çalışırken bir dialog paneli kapatır ve yükseltmenin adımını gösterir; adımı `GET /api/upgrade/status` bildirir. Daha yeni bir release bulamayan yükseltme hata yanıtlar ve çalışan sunucu değişmez. Denetim kaydı yükseltmeyi yazar.

Sunucu daha yeni bir release'i kendi başına da kontrol eder. "Ayarlar" sayfasının düzenlediği `upgrade.check_interval` ayarı, sunucunun release API'sini hangi sıklıkla sorduğunu belirler (varsayılan bir saat, en az bir dakika). Daha yeni bir release varken panelin her sayfasındaki banner onu adlandırır ve "Ayarlar" sayfasına bağlanır; sunucu yeniden en yeni sürümü çalıştırdığında banner kaybolur.

## HTTP/2 ve HTTP/3 limitleri

Sunucu tarafındaki her bağlantı flood limitleri taşır. `config.toml`'un `[http2]` ve `[http3]` bölümleri, "Ayarlar" sayfasının düzenlediği, her port için varsayılanları tutar:

| Ayar                                     | Varsayılan | Aralık           | Sınırladığı şey                                                                                   |
|------------------------------------------|------------|------------------|---------------------------------------------------------------------------------------------------|
| `http2.max_header_list_size`             | 65536      | 1024 ila 1048576 | Bir isteğin `HEADERS` artı CONTINUATION frame'lerinin bayt cinsinden boyutu (CVE-2024-27316).     |
| `http2.max_pending_accept_reset_streams` | 128        | 1 ila 65536      | İstemcinin `GOAWAY` bağlantıyı kapatmadan önce reset'leyebileceği stream sayısı (CVE-2023-44487). |
| `http3.max_concurrent_bidi_streams`      | 100        | 1 ila 100000     | Bir QUIC bağlantısının açabileceği istek stream'leri.                                             |
| `http3.max_concurrent_uni_streams`       | 10         | 1 ila 100000     | Bir QUIC bağlantısının açabileceği kontrol stream'leri.                                           |

Header limitinin üstündeki bir isteğin stream'i reset'lenir ve port yeni bağlantılara hizmet vermeye devam eder.

Bir port, kendi `http2_limits` ve `http3_limits` seçenekleriyle varsayılanları geçersiz kılabilir; port formu bu bölümleri HTTP ve QUIC protokolleri için gösterir. Kapalı kalan bir bölüm sunucu ayarlarındaki limitleri kullanır.

```toml
[http2]
max_header_list_size = 65536
max_pending_accept_reset_streams = 128

[http3]
max_concurrent_bidi_streams = 100
max_concurrent_uni_streams = 10
```

Varsayılanlarda yapılan bir değişiklik her portu yeniden kurar, yani restart olmadan geçerli olur. Bir yükseltmenin başka bir süreçten devraldığı QUIC endpoint'leri global varsayılanları kullanır.

## Metrics

`GET /api/metrics` çalışan sürecin sayaçlarını Prometheus text formatında (`text/plain; version=0.0.4`) döndürür:

- `r3v3rs3_http_requests_total`, `r3v3rs3_h3_requests_total`: TCP/TLS ve QUIC üzerinde proxy'lenen istekler.
- `r3v3rs3_rx_bytes_total`, `r3v3rs3_tx_bytes_total`: istemcilerden alınan ve istemcilere iletilen toplam ağ bant genişliği (bayt).
- `r3v3rs3_http_responses_by_status_total`: durum sınıfına göre ayrılmış HTTP yanıtları (`2xx`, `3xx`, `4xx`, `5xx`).
- `r3v3rs3_upstream_errors_total`: başarılı bir upstream yanıtı olmadan yanıtlanan istekler.
- `r3v3rs3_upstream_errors_by_kind_total`: hata türüne göre ayrılmış upstream hataları (`timeout`, `connect`, `circuit_open`, `no_route`, `other`).
- `r3v3rs3_http_request_duration_seconds`: standart sepetlerle (`0.005s` - `10s`), toplam süre ve sayaç içeren istek gecikme histogramı.
- `r3v3rs3_http_request_duration_quantiles_seconds`: son isteklerin saniye cinsinden gecikme dilimleri (`p50`, `p95`, `p99`).
- `r3v3rs3_tls_handshakes_total`, `r3v3rs3_tls_handshake_errors_total`: tamamlanan ve başarısız olan toplam TLS el sıkışmaları.
- `r3v3rs3_tls_handshake_duration_seconds`: sepetlerle (`0.001s` - `2.5s`), toplam süre ve sayaç içeren TLS el sıkışma gecikme histogramı.
- `r3v3rs3_tls_handshake_duration_quantiles_seconds`: son TLS el sıkışmalarının saniye cinsinden gecikme dilimleri (`p50`, `p95`, `p99`).
- `r3v3rs3_cache_hits_total`, `r3v3rs3_cache_misses_total`, `r3v3rs3_cache_bypasses_total`: cache kararları (isabet, kaçırma ve atlama).
- `r3v3rs3_open_connections`: açık istemci bağlantıları.
- `r3v3rs3_process_open_fds`, `r3v3rs3_process_max_fds`: açık ve izin verilen maksimum dosya tanımlayıcı (fd) sayısı.
- `r3v3rs3_process_resident_memory_bytes`, `r3v3rs3_process_virtual_memory_bytes`: bayt cinsinden yerleşik (RSS) ve sanal bellek kullanımı.
- `r3v3rs3_event_loop_lag_seconds`: saniye cinsinden anlık Tokio event loop gecikmesi.

`Accept: application/json` başlığı gönderildiğinde sistem öz izleme (self-monitoring) metrikleri JSON formatında döner.

İsteği bir admin session cookie'si veya geçerli bir MCP / API Bearer token'ı yetkilendirir. Bir Prometheus job'ı `bearer_token` ile scrape ederken WebUI cookie'sini kullanmaya devam edebilir. Sayaçlar tek süreci sayar: worker modunda portları worker sunar ve kendi sayaçlarını tutar, worker durumu ayrıca `GET /api/workers` üzerinden raporlanır.

### Grafana Panosu

r3v3rs3 ile doğrudan paketlenmiş, üretime hazır bir Grafana panosu sunulur. Bu panoyu `GET /api/metrics/grafana` uç noktasından veya WebUI Canlı Trafik sayfasındaki **Grafana Panosu** butonu üzerinden doğrudan JSON olarak indirebilirsiniz.

Pano 5 görsel bölüm içerir:
1. **Temel Performans Göstergeleri (KPI)**: Toplam istek akış hızı, aktif bağlantı sayısı, p95 istek gecikmesi, önbellek isabet oranı (cache hit ratio), TLS el sıkışma başarı oranı ve Tokio event loop gecikmesi.
2. **Trafik & Akış Hızı**: Protokol karşılaştırması (HTTP/1.1 & HTTP/2 ile HTTP/3), HTTP yanıt durum dağılımı (2xx, 3xx, 4xx, 5xx) ve gelen/giden ağ bant genişliği (RX/TX).
3. **Gecikme & TLS Performansı**: İstek gecikme dilimleri (p50, p95, p99), TLS el sıkışma süresi dilimleri ve TLS el sıkışma başarı/hata oranları.
4. **Upstream Güvenilirliği & Önbellekleme**: Hata türüne göre upstream hataları (zaman aşımı, bağlantı hatası, devre kesici, rota bulunamadı) ve önbellek kararları.
5. **Sistem Sağlığı & Kaynaklar**: Yerleşik (RSS) ve sanal bellek kullanımı, açık dosya tanımlayıcıları ile sınırları ve olay döngüsü gecikmesi.

Örnek Prometheus scrape yapılandırması:

```yaml
scrape_configs:
  - job_name: 'r3v3rs3'
    metrics_path: '/api/metrics'
    scheme: 'http'
    static_configs:
      - targets: ['127.0.0.1:46492']
    authorization:
      type: Bearer
      credentials: 'YOUR_API_TOKEN'
```

### Prometheus Anomali Alarmları ve Bildirim Merkezi

r3v3rs3, canlı Prometheus trafik ve çalışma zamanı metriklerini izleyen, anomalileri tespit edip alarmlar üreten, webhook bildirimleri gönderen ve WebUI üst çubuğundaki **Bildirim Merkezi** üzerinde durumları anlık gösteren yerleşik bir anomali tespit motoru içerir:

- **Gerçek Zamanlı Değerlendirme Döngüsü**: Sunucu her 10 saniyede bir son trafik metriklerini ve çalışma zamanı sağlığını yapılandırılan eşik değerlerine göre değerlendirir:
  - **P99 Gecikme Anomalisi (`max_p99_latency_ms`)**: 99. yüzdelik dilim yanıt süresi eşik değerini aştığında tetiklenir (varsayılan `1000ms`).
  - **5xx Hata Oranı Anomalisi (`max_5xx_percent`)**: Son değerlendirme penceresindeki 5xx sunucu yanıt hatası yüzdesi eşiği aştığında tetiklenir (varsayılan `%5`).
  - **Trafik Artışı / Ani İstek Dalgası (`max_request_rate_rps`)**: Gelen saniye başına istek hacmi (RPS) yapılandırılan sınırı aştığında tetiklenir.
  - **Event Loop Gecikmesi Anomalisi (`max_event_loop_lag_ms`)**: Tokio asenkron çalışma zamanı yürütme gecikmesi eşiği aştığında tetiklenir (varsayılan `150ms`).
- **WebUI Bildirim Merkezi**:
  - Üst gezinme çubuğundaki zil simgesi, aktif anomali sayısını canlı kırmızı bir rozetle gösterir.
  - Zile tıklandığında açılan menü; aktif anomalileri, önem derecelerini (`CRITICAL` veya `WARNING`), anlık ölçülen değerleri ve eşik sınırlarını gösterir; "Alarmları Temizle" butonuyla aktif bildirimler temizlenebilir.
- **Webhook Bildirimleri ve Fırtına Koruması**:
  - Tespit edilen anomaliler, yapılandırılmış bildirim uç noktalarına webhook olayları (`system.high_latency`, `system.high_error_rate`, `system.traffic_spike`, `system.event_loop_lag`) olarak iletilir.
  - Olay türü başına 5 dakikalık otomatik bekleme süresi (cooldown) ile kesintisiz arıza durumlarında bildirim fırtınaları engellenir.
- **WebUI Üzerinden Eşik Yapılandırması**:
  - **Ayarlar** sayfasındaki **Anomali Alarm Eşikleri** kartından otomatik kontroller etkinleştirilebilir/devre dışı bırakılabilir ve sayısal eşik sınırları güncellenebilir.

#### Admin API

- `GET /api/system/anomalies`: Aktif anomalileri, son anomali geçmişini, canlı metrikleri ve mevcut kural eşiklerini döndürür.
- `GET /api/system/anomalies/rules`: Mevcut anomali alarm eşik kurallarını döndürür.
- `PUT /api/system/anomalies/rules`: Anomali alarm eşik kurallarını günceller.
- `POST /api/system/anomalies/dismiss`: Aktif durumdaki tüm anomali alarmlarını temizler.

### Tehdit Algılama ve Otomatik Karantina

r3v3rs3, gelen canlı HTTP ve TCP trafiğini izleyerek dış tehdit istihbaratı API'lerine ihtiyaç duymadan, sıfır gecikmeyle tarayıcı saldırılarını ve ardışık hata durumlarını tespit eder:

- **Yerel Davranışsal Tehdit Tespiti**:
  - `/.env`, `/.git`, `/.aws`, `/wp-admin`, `/wp-login.php`, `/xmlrpc.php`, `/phpmyadmin` ve `/config.json` gibi tarayıcı ve sonda kalıplarını otomatik olarak yakalar.
  - Kayan bir zaman penceresinde (varsayılan: 60 saniye) peş peşe alınan HTTP durum ihlallerini (`404`, `403`, `401`) biriktirir.
  - İhlaller belirlenen eşiğe ulaştığında (varsayılan: 5 ihlal) ilgili client IP adresini anında karantinaya alır.
- **TCP Drop Yaptırımı**:
  - Karantinaya alınan IP'lerden gelen bağlantılar, HTTP istek çözümlemesi veya TLS handshake yapılmadan TCP dinleyici seviyesinde anında kapatılır (TCP Drop / Reset). Sunucu CPU ve bellek tüketimi sıfıra indirilir.
- **Webhook Bildirimi**:
  - Bir IP karantinaya alındığında otomatik olarak `security.threat_detected` webhook olayı tetiklenir.
- **Yönetim ve WebUI Kontrolleri**:
  - Karantinadaki IP'leri, ihlal sayılarını ve kalan karantina sürelerini **Ayarlar > Karantinaya Alınan IP Adresleri** kartında görüntüleyin.
  - WebUI üzerinden tek tıkla herhangi bir IP adresini manuel karantinaya alın veya serbest bırakın.

#### Admin API

- `GET /api/security/quarantine`: Aktif durumdaki karantinalı IP kayıtlarını listeler.
- `POST /api/security/quarantine`: Bir IP adresini manuel olarak karantinaya alır (`{"ip": "...", "reason": "...", "duration_seconds": 3600}`).
- `DELETE /api/security/quarantine/{ip}`: Bir IP adresini karantinadan erken çıkarır.
- `GET /api/security/quarantine/config`: Mevcut otomatik karantina yapılandırmasını döndürür.
- `PUT /api/security/quarantine/config`: Otomatik karantina yapılandırmasını günceller ve kalıcı olarak kaydeder.

## MCP Sunucusu

Admin API, araç setini `/api/mcp` üzerinde Model Context Protocol (Streamable HTTP, 2025-03-26 spesifikasyonu) ile yayınlar. **%100 MCP Yönetim Eşitliği (MCP Parity)** ile ters proxy rotaları, dinleyici portlar, ACME sertifikaları, PaaS uygulamaları, Docker Compose yığınları, tek tıkla kurulan App Store katalog uygulamaları, yönetilen veritabanları, otomatik yedeklemeler, uzak agent düğümleri ve felaket kurtarma anlık görüntüleri dahil sistemdeki her bir özellik, bir insanın WebUI'yi açmasına gerek kalmadan bir yapay zeka ajanı tarafından otonom olarak %100 yönetilebilir.

### Otonom MCP Araç Seti

| Kategori | Araçlar ve İşlemler | Açıklama |
|---|---|---|
| **Sistem ve İzleme** | `server_status`, `get_config`, `list_audit_log`, `get_system_metrics`, `get_system_anomalies`, `list_banned_ips`, `unban_ip`, `list_quarantined_ips`, `quarantine_ip`, `unban_quarantined_ip`, `get_quarantine_config`, `update_quarantine_config` | Çalışma zamanı sürümü, worker süreçleri, maskelenmiş sistem yapılandırması, değiştirilemez denetim kaydı (audit log), süreç dosya tanıtıcıları/bellek/lag, gerçek zamanlı anomali alarmları, fail2ban kaba kuvvet IP engeli ve tehdit otomatik karantina yönetimi. |
| **Portlar** | `list_ports`, `get_port`, `get_port_status`, `add_port`, `update_port`, `delete_port`, `reset_port`, `list_network_interfaces` | Dinleyici port yaşam döngüsü: dinleme adresleri, TLS sonlandırma ayarları, soket yeniden başlatma/yenileme ve sunucu ağ arayüzlerini listeleme. |
| **Proxy'ler** | `list_proxies`, `get_proxy`, `get_proxy_status`, `add_proxy`, `update_proxy`, `delete_proxy`, `purge_proxy_cache` | HTTP, WebSocket, TCP, UDP, TLS ve HTTP/3 için ters proxy yapılandırması, upstream sağlık durumu ve yol bazlı önbellek temizleme (purge). |
| **Sertifikalar ve ACME** | `list_certs`, `get_cert`, `check_cert_coverage`, `delete_cert`, `list_acme`, `get_acme`, `add_acme`, `delete_acme`, `request_acme`, `list_dns_credentials` | TLS sertifikası inceleme, alan adı kapsam kontrolü, otomatik ACME sertifika talepleri (HTTP-01, TLS-ALPN-01, DNS-01), anlık sertifika yenileme ve DNS sağlayıcı kimlik bilgileri yönetimi. |
| **Erişim Listeleri** | `list_access_lists`, `add_access_list`, `update_access_list`, `delete_access_list` | CIDR IP filtreleme, HTTP basic authentication ve özel bearer token'lar içeren erişim kontrol listeleri. |
| **PaaS Uygulamaları** | `list_apps`, `get_app`, `add_app`, `update_app`, `delete_app`, `deploy_app`, `get_app_logs`, `get_app_env`, `update_app_env`, `list_app_deployments`, `rollback_deployment`, `check_app_update` | Otonom PaaS konteyner yaşam döngüsü: Docker image/git/compose uygulaması dağıtımı, konteyner log akışı, ortam değişkeni yönetimi, önceki dağıtıma geri alma (rollback) ve Docker registry güncelleme kontrolü. |
| **App Store Kataloğu** | `get_store`, `refresh_store`, `install_store_app` | Küratörlü tek tıkla kurulum şablonlarının (veritabanları, paneller, CMS, analitik) katalogdan doğrudan tam yapılandırılmış ve yönlendirilmiş uygulamalar olarak otonom kurulumu. |
| **Yönetilen Veritabanları ve Yedekler** | `list_databases`, `list_database_engines`, `create_database`, `list_database_backups`, `create_database_backup`, `delete_database_backup`, `get_backup_config`, `update_backup_config` | Otomatik üretilen kimlik bilgileriyle PostgreSQL, MySQL, Redis, MariaDB ve MongoDB sağlama, anlık yedek alma, yedek silme ve S3/SFTP/rsync uzak depolama ayarları. |
| **Agent Hedef Düğümleri** | `list_targets`, `add_target`, `delete_target` | Çok düğümlü Docker agent altyapısı, tek kullanımlık kayıt token'ı üretme ve hedef düğümleri kaldırma. |
| **Projeler ve Ortam Grupları** | `list_projects`, `create_project`, `delete_project`, `list_env_groups`, `create_env_group`, `delete_env_group` | Proje organizasyonu, çoklu ortam izolasyonu ve paylaşılan ortak ortam değişkeni grupları. |
| **Felaket Kurtarma** | `list_dr_backups`, `create_dr_backup` | İsteğe bağlı AES-256-GCM şifrelemeli ve otomatik S3 replikasyonlu tam sistem yedek anlık görüntüleri. |

MCP sunucusu varsayılan olarak kapalıdır. Panelin ayarlar sayfası onu sunucu çalışırken açar ve giriş yapmasına izin verilecek client'ları tutar. `config.toml` aynı bölümü taşır:

```toml
[mcp]
enabled = true

[[mcp.clients]]
id = "r3v3rs3-cli"
name = "r3v3rs3 MCP client"
redirect_uris = ["http://127.0.0.1:8765/callback"]
```

Client, PKCE'li OAuth 2.1 authorization code akışıyla giriş yapar: client `/oauth/authorize` adresini tarayıcıda açar, oturum açmış hesap onay formundan client'a izin verir ve client tek kullanımlık kodu `/oauth/token` üzerinde access token ile değiştirir. Token, onayın kapsamını taşır: `read` durumu görür, `edit` ayrıca proxy'leri değiştirir. Metadata belgeleri `/.well-known/oauth-authorization-server` ve `/.well-known/oauth-protected-resource` adreslerindedir.

Token'lar ve dinamik olarak kaydedilen client'lar SQLite (`r3v3rs3.db`) veritabanında kalıcı olarak saklanır. Sunucunun yeniden başlaması veya güncellenmesi yetkilendirilmiş oturumları korur; client'lar 30 günlük refresh token'larını kullanarak kullanıcıya yeniden giriş sormadan access token'larını arka planda sessizce yeniler. Aktif oturumlar WebUI Ayarlar sayfasındaki "Aktif MCP Oturumları" kartından veya `GET /api/mcp/sessions` ve `DELETE /api/mcp/sessions/{id}` endpoint'leri üzerinden incelenebilir ve sonlandırılabilir (revoke).

Bağlanma yolu client'a göre değişir. MCP authorization akışını konuşan bir AI host, örneğin Claude Code veya Cursor, ekstra bir programa ihtiyaç duymaz: bu sunucuyu bir HTTP MCP sunucusu olarak ekler, örneğin `claude mcp add --transport http --scope user r3v3rs3 https://panel.example.com/api/mcp` komutuyla, ve `/mcp` komutunun `Authenticate` adımı aynı authorization akışını tarayıcıda çalıştırır. Aşağıdaki komut satırı client'ı diğer durumlar içindir: script'ler, cron job'ları ve tarayıcı oturumu olmayan sunucular; tek komut bir kez giriş yapar ve token'larını sonraki her çağrı için saklar.

Depo, `r3` (`r3v3rs3-cli` paketi) içinde bir komut satırı client'ı ile gelir. Kurulum script'i en yeni release'in derlenmiş binary'sini GitHub'dan indirir, sha256 özetini doğrular ve kurar:

```sh
# Linux ve macOS:
curl -fsSL https://raw.githubusercontent.com/KilimcininKorOglu/r3v3rs3/main/install-cli.sh | sh
```

```powershell
# Windows (PowerShell):
irm https://raw.githubusercontent.com/KilimcininKorOglu/r3v3rs3/main/install-cli.ps1 | iex
```

Windows'ta binary `%LOCALAPPDATA%\Programs\r3` içine düşer; onu `PATH`'e eklersiniz. Script Linux ve macOS'ta `/usr/local/bin`'e kurar, `INSTALL_DIR` bu dizini değiştirir. Belirli bir release `--version vX.Y.Z` ile kurulur; her platformun arşivi ayrıca [releases sayfasında](https://github.com/KilimcininKorOglu/r3v3rs3/releases) durur.

Sonra:

```sh
r3 http://127.0.0.1:46492 login
r3 http://127.0.0.1:46492 platform status
```

`login`, onay sayfasını tarayıcıda açar ve token'ları yapılandırma dizinine kaydeder. Bir script, access token'ı doğrudan `--token` ile veya `R3V3RS3_TOKEN` ortam değişkeniyle verebilir. `update` komutu, çalışan binary'nin yerine daha yeni bir release kurar ve sha256 özetini release'in checksum'larıyla doğrular.

### Tam kaynak yönetimi

`r3`, ters proxy ve PaaS platformunun tüm bileşenlerini doğrudan yönetmek için birinci sınıf alt komutlar sunar:

```sh
# Portlar
r3 <server> port list
r3 <server> port get <id>
r3 <server> port add <spec.json|yaml|->
r3 <server> port delete <id>
r3 <server> port reset <id>

# Proxy'ler
r3 <server> proxy list
r3 <server> proxy get <id>
r3 <server> proxy add <proxy.json|yaml|->
r3 <server> proxy status <id>
r3 <server> proxy purge-cache <id> [--path /static]
r3 <server> proxy delete <id>

# Sertifikalar ve ACME
r3 <server> cert list
r3 <server> cert get <id>
r3 <server> cert upload --cert server.crt --key server.key
r3 <server> cert coverage
r3 <server> cert delete <id>
r3 <server> acme list
r3 <server> acme get <id>
r3 <server> acme add <acme.json|yaml|->
r3 <server> acme order <id>
r3 <server> acme delete <id>

# Erişim Listeleri (ACL) ve DNS Sağlayıcıları
r3 <server> acl list
r3 <server> acl get <id>
r3 <server> acl add <acl.json|yaml|->
r3 <server> acl delete <id>
r3 <server> dns list
r3 <server> dns get <id>
r3 <server> dns add <dns.json|yaml|->
r3 <server> dns delete <id>

# Dağıtılan Uygulamalar
r3 <server> app list
r3 <server> app get <id>
r3 <server> app add <app.json|yaml|->
r3 <server> app deploy <id>
r3 <server> app rollback <id>
r3 <server> app start <id>
r3 <server> app stop <id>
r3 <server> app restart <id>
r3 <server> app logs <id> [--tail 100]
r3 <server> app log-size <id>
r3 <server> app vacuum-logs <id>
r3 <server> app env <id>
r3 <server> app set-env <id> KEY=VAL [KEY2=VAL2...]
r3 <server> app delete <id>

# Yönetilen Veritabanları
r3 <server> db list
r3 <server> db get <id>
r3 <server> db create --name my-db --engine postgresql [--version 17] [--password secret]
r3 <server> db clone <id> --name my-db-clone
r3 <server> db replica <id> --name my-db-replica
r3 <server> db logs <id> [--tail 100]
r3 <server> db delete <id>

# Yedekler
r3 <server> backup config
r3 <server> backup list <database-id>
r3 <server> backup create <database-id>
r3 <server> backup restore <database-id> <backup-id>
r3 <server> backup delete <database-id> <backup-id>

# Çoklu Düğüm Agent Hedefleri
r3 <server> target list
r3 <server> target get <id>
r3 <server> target add --name worker-1 --host 10.0.0.5 --port 46493
r3 <server> target drain <id>
r3 <server> target undrain <id>
r3 <server> target token
r3 <server> target delete <id>

# Projeler ve Ortamlar
r3 <server> project list
r3 <server> project get <id>
r3 <server> project add --name my-project [--description "Ana servisler"]
r3 <server> project sync-env <project-id> --from staging --to production
r3 <server> project delete <id>

# Uygulama Kataloğu (App Store)
r3 <server> store list
r3 <server> store refresh
r3 <server> store install <store-app-id> --name my-service [--domain service.example.com]

# Güvenlik, Yasaklı & Karantinalı IP'ler
r3 <server> security banned-ips
r3 <server> security unban <ip>
r3 <server> security quarantined-ips
r3 <server> security quarantine <ip> [--reason "Tarayıcı sondası"] [--duration-seconds 3600]
r3 <server> security unquarantine <ip>
r3 <server> security quarantine-config
r3 <server> security set-quarantine-config [--enabled true] [--max-violations 5] [--window-seconds 60] [--ban-duration-seconds 3600]

# Webhook'lar ve Zamanlanmış Görevler (Cron)
r3 <server> webhook list
r3 <server> webhook get <id>
r3 <server> webhook add <webhook.json|yaml|->
r3 <server> webhook test <id>
r3 <server> webhook delete <id>
r3 <server> cron list <app-id>
r3 <server> cron add <app-id> --cron "0 * * * *" --command "php artisan schedule:run" [--name hourly]
r3 <server> cron run <app-id> <job-id>
r3 <server> cron delete <app-id> <job-id>

# Platform Durumu ve Bakım
r3 <server> platform status
r3 <server> platform metrics
r3 <server> platform prune
r3 <server> platform vacuum-logs [--threshold-mb 50]
r3 <server> platform rotate-key
```

### Deklaratif tanımlar (`apply -f`)

CI/CD pipeline'ları ve betikli yönetim için `r3 apply -f <dosya>`, portları, proxy'leri ve uygulamaları tek hamlede ve idempotent biçimde tanımlamak üzere YAML veya JSON manifest uygular:

```sh
# Dosyadan uygula
r3 http://127.0.0.1:46492 apply -f infrastructure.yaml

# Standart girdiden uygula
cat infrastructure.yaml | r3 http://127.0.0.1:46492 apply -f -
```

Örnek bir YAML manifesti:

```yaml
ports:
  - id: http
    port:
      listen: /ip4/0.0.0.0/tcp/80
      active: true

proxies:
  - id: web
    name: "Web frontend"
    protocol: http
    ports: ["http"]
    routes:
      - host: "example.com"
        servers:
          - url: "http://127.0.0.1:3000"

apps:
  - spec:
      name: "my-app"
      domains: ["app.example.com"]
      target:
        type: image
        image: "nginx:alpine"
        port: 80
```

## Saklanan veriler

r3v3rs3, ACME kayıtlarını yapılandırma dizinindeki `acme.toml` dosyasında saklar. Dosya, her ACME hesabının private key'ini ve DNS sağlayıcısının kimlik bilgilerini düz metin olarak içerir. Unix'te r3v3rs3 dosyayı `0600` izniyle oluşturur ve yazar. Böylece dosyayı yalnız sürecin sahibi okuyabilir. Yönetim API'si ve WebUI kimlik bilgilerini hiçbir zaman döndürmez. ACME listesi yalnız sağlayıcının adını gösterir, örneğin `Let's Encrypt (DNS-01, Cloudflare)`.

ACME kayıtlarını WebUI'dan oluşturun. r3v3rs3, ACME hesabını kayıt eklendiğinde oluşturur. `acme.toml` içindeki bir kayıt şöyle görünür:

```toml
version = "1.0.1"

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

## DNS Kimlik Bilgileri

WebUI'daki "Sertifikalar" altındaki "DNS Kimlik Bilgileri" sayfası, DNS sağlayıcılarının kimlik bilgilerini bir ad altında saklar. Bir sağlayıcı birden çok kimlik bilgisi tutabilir, örneğin hesap başına bir Cloudflare token'ı. ACME formu, DNS-01 challenge için provider bilgilerini yeniden yazmak yerine saklanmış bir kimlik bilgisi seçer. Kimlik bilgisini kullanan her kayıt, token değişikliğini onunla birlikte alır.

Yönetim API'si ve WebUI saklanan kimlik bilgilerini hiçbir zaman döndürmez. Liste, bir kimlik bilgisinin yalnız adını ve sağlayıcısını gösterir. r3v3rs3, kimlik bilgilerini yapılandırma dizinindeki `r3v3rs3.key` dosyasındaki anahtar ile mühürler ve bu anahtar henüz yoksa ilk kimlik bilgisinde onu oluşturur. Kimlik bilgileri şifreli olarak `r3v3rs3.db` veritabanında saklanır.

`acme.toml` içindeki bir ACME kaydı, bir kimlik bilgisine id'siyle referans verir ve kendi provider'ını taşımaz:

```toml
[acme1]
provider = "Let's Encrypt"
renewal_days = 60
identifiers = ["example.com", "*.example.com"]
challenge_type = "dns-01"
dns_credential = "<kimlik bilgisi id'si>"
```

Bir kayıt ya `dns_provider` ya `dns_credential` taşır, ikisini birden taşıyamaz. İçinde `dns_provider` olan kayıtlar çalışmayı sürdürür. Panel, bir kimlik bilgisine referans veren kaydı secret'ları göstermeden düzenler. Kimlik bilgisinin alanlarını boş bırakan bir düzenleme saklanan değerleri korur.

Bir ACME kaydının hâlâ referans verdiği kimlik bilgi silinemez. Önce kayıttaki referansı silin.

# Ayarlar

WebUI'daki "Ayarlar" bölümünden, veritabanında (`r3v3rs3.db`) saklanan ve bütün sunucuyu etkileyen seçenekleri değiştirebilirsiniz. Değişiklikler hemen uygulanır ve kaydedilir.

| Ayar                            | Varsayılan    | Açıklama                                                                                                                                                                                                                                                 |
|---------------------------------|---------------|----------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------|
| Oturum Süresi                   | `1h`          | Yönetim paneli oturumunun geçerlilik süresi. En az 5 dakika olabilir.                                                                                                                                                                                    |
| Görüntüleme Saat Dilimi         | `UTC`         | Panelin saklanan zamanları gösterdiği IANA saat dilimi, örneğin Europe/Istanbul. Zamanlar UTC olarak saklanmaya devam eder.                                                                                                                              |
| Maksimum Giriş Denemesi         | `10`          | Her istemci IP adresi ve kullanıcı adı için izin verilen başarısız giriş sayısı.                                                                                                                                                                         |
| Giriş Denemesi Sıfırlama Süresi | `15m`         | Limite ulaşıldıktan sonraki bekleme süresi.                                                                                                                                                                                                              |
| Arka Plan Görevi Aralığı        | `1h`          | Sertifika yenileme ve log temizleme görevlerinin çalışma aralığı.                                                                                                                                                                                        |
| HTTP Challenge Adresi           | `0.0.0.0:80`  | ACME HTTP challenge'larının dinlendiği adres.                                                                                                                                                                                                            |
| TLS-ALPN Challenge Adresi       | `0.0.0.0:443` | Hiçbir port bu portu kullanmıyorsa ACME TLS-ALPN-01 challenge'larının dinlendiği adres.                                                                                                                                                                  |
| DNS Challenge Çözümleyicisi     | boş           | r3v3rs3'ün DNS-01 challenge'ının TXT kayıtları görünene kadar sorguladığı DNS sunucusu, örneğin `1.1.1.1:53`. Boş bırakılırsa sistemin çözümleyicisi kullanılır.                                                                                         |
| Upstream DNS Çözümleyicisi      | boş           | `http+srv` ve `https+srv` sunucu URL'lerinin SRV sorgularını yanıtlayan DNS sunucusu, örneğin Consul için `127.0.0.1:8600`. Boş bırakılırsa sistemin çözümleyicisi kullanılır. Ayrıntılar için [DNS SRV sunucuları](#dns-srv-sunuculari) bölümüne bakın. |
| Veritabanı Log Saklama Süresi   | `3months`     | Log'ların log veritabanında ne kadar tutulacağı.                                                                                                                                                                                                         |
| Denetim Kaydı Saklama Süresi    | `1year`       | Denetim kaydındaki bir girdinin ne kadar tutulacağı. Ayrıntılar için [Denetim kaydı](#denetim-kaydi) bölümüne bakın.                                                                                                                                     |
| Sertifika Süre Uyarısı          | `14days`      | Sertifika listesi bu süre içinde sona erecek sertifikayı işaretler. Webhook bu sertifika için bildirim alır. Ayrıntılar için [Bildirimler](#bildirimler) bölümüne bakın.                                                                                 |
| Webhook URL                     | boş           | Bildirim webhook'u. Boşsa bildirim gönderilmez.                                                                                                                                                                                                          |
| Webhook Token                   | boş           | Webhook isteklerinin bearer token'ı. Admin API bu değeri döndürmez.                                                                                                                                                                                      |
| Webhook Timeout                 | `10s`         | Tek bir webhook isteğinin en uzun süresi.                                                                                                                                                                                                                |

Süreleri `30s`, `15m`, `1h` veya `7days` gibi okunabilir bir biçimde yazın.

# Yapılandırma dosyaları

r3v3rs3 yapılandırmasını `$XDG_CONFIG_HOME/r3v3rs3` veya `$HOME/.config/r3v3rs3` dizinindeki TOML dosyalarında saklar.

Varsayılan konumu `R3V3RS3_CONFIG_DIR` ortam değişkeni veya `--config-dir` komut satırı seçeneğiyle değiştirebilirsiniz.

Bu dosyaları elle de düzenleyebilirsiniz. Ancak r3v3rs3 yapılandırma dosyalarındaki değişiklikleri kendiliğinden algılamaz. Değişikliklerin geçerli olması için dosyayı düzenledikten sonra sunucuyu yeniden başlatın.

# WebUI

r3v3rs3 bir WebUI ile birlikte gelir. WebUI varsayılan olarak `127.0.0.1:46492` adresinde çalışır. Başka bir dinleme adresini, örneğin `0.0.0.0:46492`, `R3V3RS3_WEBUI` ortam değişkeni veya `--webui` komut satırı seçeneğiyle ayarlayın. `R3V3RS3_NO_WEBUI=1` ortam değişkeni veya `--no-webui` komut satırı seçeneği WebUI'ı ve yönetim API'sini kapatır.

WebUI menüsü soldaki kenar çubuğundadır ve üç gruptan oluşur:

- **Proxy**: **Portlar**, **Proxy'ler**, **Erişim Listeleri** ve **Sertifikalar**.
- **Platform**: **Uygulamalar**, **Hedefler** ve **Git Sağlayıcıları**, yani [deploy platformu](@/platform.tr.md). Bu grup yalnız platform açıkken görünür ve proxy listesi olmayan hesap tarafından görülür. **Git Sağlayıcıları** öğesini yalnız admin veya editör görür.
- **Yönetim**: **Hesaplar**, **Denetim Kaydı** ve **Ayarlar**. Bu grubu yalnız admin hesabı görür.

Üst çubuk logoyu, dil menüsünü, tema menüsünü ve **Çıkış Yap** butonunu içerir. Dar bir ekranda kenar çubuğu ve **Çıkış Yap** yerine üst çubukta **Menü** butonu görünür. Bu buton aynı menüyü açar ve menünün sonunda **Çıkış Yap** bulunur.

WebUI dilini üst çubuktaki bayrak menüsünden seçebilirsiniz: İngilizce veya Türkçe. Tema menüsünde Sistem, Açık ve Koyu seçenekleri bulunur. WebUI bu seçimleri `r3v3rs3_lang` ve `r3v3rs3_theme` cookie'lerinde saklar. Bu cookie'ler yoksa WebUI İngilizce ve sistem temasıyla açılır.

r3v3rs3'ün hata sayfaları ve Panel Oturumu giriş sayfası da bu cookie'lere bakar. Ancak tarayıcı bu cookie'leri yalnız WebUI'ın host'una gönderir. Bu yüzden başka bir host'taki proxy'nin sayfaları İngilizce ve sistem temasıyla açılır.

### Standart İkonik Eylem Dili

WebUI genelindeki tüm tablo ve liste satırları (Proxy'ler, Uygulamalar, Portlar, Sertifikalar, Hedefler, Hesaplar ve Projeler), yer kaplayan metin bağlantıları yerine kompakt ve standartlaştırılmış ikon butonları kullanır. Her eylem butonu yerelleştirilmiş alt bilgi ipuçlarına (`title`) ve `aria-label` ekran okuyucu etiketlerine sahip olup semantik bir renk paletini izler:

- **Düzenle / Ayarlar**: Mavi (`#2563eb`)
- **Görüntüle / İncele**: Nötr Gri (`#737373`)
- **Terminal / Shell**: Zümrüt Yeşil (`#059669`)
- **Volume / Dosyalar**: Amber Klasör (`#d97706`)
- **Metrikler / Trafik Dağılımı**: Cyan Grafik (`#0891b2`)
- **Loglar / Denetim**: İndigo Belge (`#4f46e5`)
- **Dağıt / Başlat**: Zümrüt Oynat (`#059669`)
- **Dondur / Bakım Modu**: Turuncu Duraklat (`#ea580c`)
- **İndir / Dışa Aktar**: Nötr İndirme (`#525252`)
- **Geri Yükle / Test Et**: Teal Yenileme (`#0d9488`)
- **Sil / Temizle**: Kırmızı Çöp Kutusu (`#dc2626`)

# Yönetim API'si

WebUI, `/api` altındaki yönetim API'sini kullanır. r3v3rs3 bu API'nin OpenAPI dokümanını sunucu kodundan üretir. Bu yüzden doküman, çalışan sürümün route'larını listeler.

- OpenAPI dokümanı: `http://localhost:46492/api/openapi.json`
- Swagger UI: `http://localhost:46492/api/docs/`

İki adres de oturum ister. Önce WebUI'a giriş yapın, sonra adresleri aynı tarayıcıda açın. WebUI'ın alt bilgisindeki API linki de Swagger UI'ı açar.

Bir script, `POST /api/login` ile giriş yapar ve yanıttaki `token` cookie'sini sonraki isteklerle gönderir:

```bash
$ curl -c cookies.txt -H 'Content-Type: application/json' \
    -d '{"username":"admin","method":"password","password":"passw0rd","insecure":true}' \
    http://localhost:46492/api/login
$ curl -b cookies.txt http://localhost:46492/api/ports
```

`"insecure": true` değeri cookie'den `Secure` özelliğini kaldırır. Yönetim paneli düz HTTP kullanıyorsa bu değeri gönderin.

TOTP'si açık hesapta ilk yanıt `"totp_required"` olur. Kodu, ilk isteğin cookie'siyle birlikte ikinci bir istekte gönderin:

```bash
$ curl -b cookies.txt -c cookies.txt -H 'Content-Type: application/json' \
    -d '{"username":"admin","method":"totp","token":"123456","insecure":true}' \
    http://localhost:46492/api/login
```

# Denetim kaydı

r3v3rs3, bir hesabın WebUI veya yönetim API'si ile yaptığı değişiklikleri kaydeder: portlar (sıfırlamalar dahil), proxy'ler (cache temizlemeleri dahil), erişim listeleri, sertifikalar, ACME kayıtları, ayarlar, CDN IP aralığı yenilemeleri ve hesaplar. [Deploy platformu](@/platform.tr.md) için uygulamaları, ortam değişkenlerini, deployment'ları ve geri almaları, Git token'larını ve webhook secret'larını, hedefleri ve agent kayıtlarını, Git sağlayıcı bağlantılarını, yeniden kurulan webhook'ları ve platform ayarlarını kaydeder. İmzalı bir Git sağlayıcı webhook'unun başlattığı deployment da kaydedilir. Yönetim paneline her giriş, her başarısız giriş denemesi ve her çıkış da kaydedilir. r3v3rs3'ün kendi yaptığı değişiklikler kaydedilmez. Sertifika yenileme ve keşfedilen proxy'ler buna örnektir.

Her kayıtta zaman, hesap, istemci IP adresi, işlem, değişen kaynağın id'si ve kısa bir özet bulunur. Özet isimleri, adresleri ve rolleri içerir. Parola, token veya key içermez.

Tek sunucu denetim kaydını log dizinindeki `log.db` dosyasının `audit_log` tablosunda tutar.

Bir kaydın ne kadar tutulacağını "Denetim Kaydı Saklama Süresi" ayarı belirler. Varsayılan değer `1year` olur.

Denetim kaydına yazma başarısız olursa değişiklik geri alınmaz. r3v3rs3 hatayı log'a yazar.

WebUI'daki Denetim Kaydı sayfası kayıtları en yeni kayıttan başlayarak listeler. Bu sayfayı yalnız admin hesabı açar. Sayfa kayıtları metin araması, eylem, hesap, kaynak ve döneme göre filtreler ve en fazla 500 kayıt gösterir. Filtrelenmiş kayıtları indirmek için bir **CSV İndir** butonu ve saklama süresini doğrudan sayfa üzerinden görüp ayarlamak için bir **Saklama Süresi** seçicisi sunar.

`GET /api/audit` aynı kayıtları döner. Bu endpoint'i yalnız admin hesabı çağırabilir. Sorgu parametrelerinin hepsi isteğe bağlıdır:

| Parametre     | Açıklama                                                                                         |
|---------------|--------------------------------------------------------------------------------------------------|
| `since`       | Unix milisaniye cinsinden en erken zaman. Varsayılan değer, `until` değerinden 31 gün öncesidir. |
| `until`       | Unix milisaniye cinsinden en geç zaman. Varsayılan değer şu andır.                               |
| `username`    | Kayıtların ait olduğu hesap.                                                                     |
| `resource_id` | Değişen kaynağın id'si veya değişen kullanıcı adı.                                               |
| `action`      | Kayıtların eylemi (örneğin `login`, `deploy_app`).                                               |
| `search`      | Hesap, kaynak id veya özet içinde serbest metin araması.                                         |
| `limit`       | Yanıttaki en fazla kayıt sayısı. Varsayılan değer `100`, en yüksek değer `500` olur.             |

# Log

r3v3rs3 varsayılan olarak log'ları standart çıktıya yazar. Bunu `R3V3RS3_LOG`, `R3V3RS3_ACCESS_LOG` ortam değişkenleriyle veya `--log`, `--access-log` komut satırı seçenekleriyle değiştirebilirsiniz.

```bash
$ r3v3rs3 start --log /var/log/r3v3rs3/r3v3rs3.log --access-log /var/log/r3v3rs3/access.log
```

Dizin içeren bir yol o dizine yazar. Yalnız dosya adı verilirse dosya log dizinine yazılır. r3v3rs3 her saat yeni bir dosya açar ve ada tarihle saati ekler, örneğin `r3v3rs3.log.2026-09-25-10`.

Log dizini bu dosyaları ve log sayfasıyla denetim kaydının veritabanı olan `log.db` dosyasını tutar. Dizini `R3V3RS3_LOG_DIR` ortam değişkeni veya `--log-dir` komut satırı seçeneği belirler. Varsayılan dizin `$XDG_DATA_HOME/r3v3rs3/logs` veya `$HOME/.local/share/r3v3rs3/logs` olur.

Log seviyesini değiştirmek için `R3V3RS3_LOG_LEVEL`, `R3V3RS3_ACCESS_LOG_LEVEL` ortam değişkenlerini veya `--log-level`, `--access-log-level` komut satırı seçeneklerini kullanın. Varsayılan seviye `info` olur. `off` log'u kapatır. `R3V3RS3_LOG_FORMAT` veya `--log-format` iki log için `text` (varsayılan) veya `json` biçimini seçer.

# Gerçek Zamanlı Trafik Akışı ve Canlı Erişim Logu

r3v3rs3, dosya loglarının yanı sıra bellek içi (in-memory) gerçek zamanlı bir erişim logu halka arabelleği (ring buffer) ve canlı akış motoru barındırır. Ters proxy üzerinden geçen HTTP ve WebSocket istekleri; durum kodları, gecikme süreleri, istemci IP'leri, host, yol ve HTTP metotlarıyla anlık olarak yakalanır.

## WebUI Trafik Akışı Paneli

Yönetim paneli üzerinde `/traffic` adresinden ve proxy detay sayfalarından erişilebilen etkileşimli bir canlı trafik paneli bulunur:
- **Gerçek Zamanlı Akış**: Sunucu Tarafından Gönderilen Olaylar (SSE) ile yeni istekler geldiği anda tabloya gerçek zamanlı olarak akar.
- **Durum Kodları Renklendirmesi**: `2xx` (başarılı), `3xx` (yönlendirme), `4xx` (istemci hatası) ve `5xx` (sunucu hatası) için renkli rozetler.
- **Gecikme Süresi İzleme**: Anlık yanıt gecikme süreleri görsel rozetlerle (< 50ms yeşil, 50–300ms sarı, > 300ms kırmızı) listelenir.
- **Etkileşimli GeoIP Dünya Haritası ve Yoğunluk Isı Haritası**: Gelen istemci IP adresleri gömülü MaxMind GeoIP veritabanı üzerinden ISO ülke kodlarına çözümlenir. Etkileşimli SVG dünya haritası, renk yoğunluğuna dayalı ısı haritası ile gerçek zamanlı coğrafi dağılımı görselleştirir. Ülkelerin üzerine gelindiğinde istek sayısı ve yüzdesi görüntülenir; haritadaki bir ülkeye veya sıralama çubuğuna tıklandığında canlı trafik akışı anında o ülkeye filtrelenir.
- **En Çok İstek Alan Ülkeler Sıralaması**: En çok trafik alan ilk 10 ülkeyi ilerleme çubukları (progress bars), istek sayıları ve yüzde paylarıyla listeler.
- **Coğrafi ve Metin Filtreleme**: Ülkeye, HTTP durum koduna, URL yoluna, istemci IP'sine, host'a veya HTTP metoduna göre anında filtreleme.
- **Kontroller**: Akışı duraklatma/devam ettirme, otomatik kaydırma (auto-scroll), harita görünümünü açma/kapatma, arabelleği temizleme ve JSON formatında dışa aktarma.
- **Canlı İstek İnceleyici ve Detay Çekmecesi (Inspector Drawer)**: Canlı trafik tablosundaki herhangi bir satıra tıklandığında derinlemesine inceleme sekmeleri sunan bir yan çekmece açılır:
  - **Genel Bakış (Overview)**: İstemci IP adresi, GeoIP ülkesi, HTTP protokolü, yanıt gecikmesi, zaman damgası, durum kodu ve eşleşen proxy rotası gibi temel istek meta verileri.
  - **Başlıklar (Headers)**: Gelen istemci istek başlıklarının ve giden sunucu yanıt başlıklarının yapılandırılmış anahtar-değer tablosu.
  - **Yük/Gövde (Payload)**: Arabelleğe alınan istek gövdelerini (16 KB'a kadar) biçimlendirilmiş JSON ve ham metin görünümleri arasında tek tıkla geçiş yaparak inceleme.
  - **Tekrarlama & cURL Konsolu (Replay)**: Uç birim için güvenli kaçış karakterleriyle oluşturulmuş **cURL Olarak Kopyala** butonu ve yakalanan istekleri doğrudan kaynak sunuculara veya özel bir URL'ye yeniden gönderen, yanıt durumunu ve gidiş-dönüş gecikme süresini anında ölçen **İsteği Tekrarla** konsolu.

## Yönetim API'si (Admin API)

Trafik uç noktaları oturum doğrulaması ile korunur ve çok kiracılı proxy erişim sınırlarını (scoping) uygular:

- `GET /api/traffic`: İsteğe bağlı filtrelerle (`status`, `search`, `country`, `resource_id`, `limit`, `since_id`) eşleşen son erişim loglarını döner.
- `GET /api/traffic/live`: Gelen erişim loglarını `text/event-stream` (Server-Sent Events) olarak gerçek zamanlı akıtır.
- `GET /api/traffic/geo`: Canlı erişim trafiğinin coğrafi dağılımını toplu olarak döner; toplam çözümlenen istek sayısını, ilk 10 ülkeyi (`country`, `count`, `percentage`) ve tüm ISO ülke kodlarının istek sayılarını içerir.
- `GET /api/traffic/{id}`: Belirtilen kimliğe (ID) sahip erişim kaydının yakalanan istek/yanıt başlıkları, gövde içeriği, içerik türü ve WebSocket bayrakları dahil tüm detaylarını döner.
- `POST /api/traffic/replay`: Belirtilen hedef URL'ye özel başlıklar, gövde ve zaman aşımı süresi (500ms ile 30s arasında sınırlandırılmış) ile bir HTTP veya WebSocket el sıkışma isteğini yeniden gönderir; yanıt durumu, gecikme süresi, hata detayı ve yanıt gövdesini döner.
