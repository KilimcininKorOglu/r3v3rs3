+++
title = "Yapılandırma ve veri depolama"
description = "r3v3rs3'ün durumunu, yedeklerini ve felaket kurtarmayı nerede tuttuğu"
weight = 1
+++

r3v3rs3; sunucu yapılandırmasını, ters proxy kurallarını, portları, erişim listelerini, ACME ayarlarını, kullanıcı hesaplarını ve deployment platformu verilerini yapılandırma dizininde (varsayılan `/etc/r3v3rs3`) yer alan birleşik bir SQLite veritabanında (`r3v3rs3.db`) saklar.

- **Birleşik SQLite Veritabanı (`r3v3rs3.db`)**: Atomik transaction'lar ve WAL modu sayesinde yüksek eşzamanlılık, kesintisiz veri bütünlüğü ve restart sonrasında sıfır kayıp güvencesi sunar. Portlar, ters proxyler ve erişim listelerindeki yapılandırma değişiklikleri parçalı upsert ve hedefli silme işlemleriyle yürütülür; tüm tabloyu silip yeniden oluşturma yükünü önler ve değişmeyen kayıtların güncellenme zaman damgalarını korur.
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

**Yedekler** sayfasını (`/backups`) açın ve **Yapılandırma Yedeği** sekmesine gidin:
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

# Ayarları gösterme, ardından düzenlenmiş kopyayı kaydetme (JSON veya YAML dosyası, standart girdi için '-')
$ r3 config get > settings.json
$ r3 config update settings.json
```

### MCP

`backup_config` ve `restore_config` araçları aynı işi yapar. `backup_config` aracının yanıtını `restore_config` aracına `backup` argümanı olarak verin. `test_notification` aracı ve `r3 config test-notification` komutu, kayıtlı bildirim webhook'una bir test mesajı gönderir.

### REST API

- `GET /api/config/backup`: Tam `ConfigBackup` nesnesini döner. `admin` yetkisi gerektirir.
- `POST /api/config/restore`: `ConfigBackup` JSON yükünden yapılandırmayı geri yükler. Bütün portları, proxy'leri, erişim listelerini ve ayarları kesinti olmadan anında yeniden yükler. `admin` yetkisi gerektirir ve denetim kaydına (`RestoreConfig`) işlenir.

## Felaket Kurtarma & Şifreli Platform Yedekleri (S3)

Standart JSON yapılandırma yedeklerinden farklı olarak, **Felaket Kurtarma Paketleri** (`.r3bundle`), r3v3rs3'ün ve Coolify benzeri uygulama dağıtım platformunun tüm çalışma durumunu AES-256-GCM ile şifrelenmiş tek bir arşivde saklar:

- **Tüm Durum Dahil**: `r3v3rs3.db` (tüm ayarları, portları, proxy'leri, erişim listelerini, ACME sertifikalarını, kullanıcı hesaplarını, uygulamaları, dağıtımları, veritabanlarını ve ortam değişkenlerini barındıran birleşik SQLite veritabanı), `r3v3rs3.key`, kümeleme mühür anahtarları (`keys/`), TLS sertifikaları ve özel anahtarları (`certs/`) ve varsa eski yedek dosyaları eksiksiz olarak arşivlenir.
- **AES-256-GCM Şifreleme**: Paket, zorunlu bir parolayla AES-256-GCM standardında şifrelenir. Anahtarı Argon2id, rastgele bir tuzla bu paroladan türetir. Eski sürümlerin SHA-256 anahtar türetimiyle oluşturduğu paketler de geri yüklenir. Parolasız bir yedekleme veya geri yükleme isteği `400 disaster_recovery_passphrase_required` ile reddedilir. Şifrelenmemiş veriler asla sunucu belleği dışına çıkmaz.
- **Otomatik S3 Senkronizasyonu**: Veritabanı yedekleme ayarlarında S3 uzak depolama yapılandırıldığında, oluşturulan felaket kurtarma yedekleri otomatik olarak uzak bucket içindeki `{prefix}/disaster-recovery/` altına yüklenir.
- **Tek Komutla Felaket Kurtarma**: Sıfırdan kurulan yeni bir sunucuya tek bir komutla tüm platformu ve verileri eksiksiz aktarabilme imkanı sunar.

### WebUI

**Yedekler** sayfasını (`/backups`) açın ve **Platform Felaket Kurtarma** sekmesine gidin:
- Şifreleme **Parolasını** girin ve **Uzak S3 depolamasına yükle** seçeneğini işaretleyin.
- **Şifreli Yedek Oluştur** düğmesine tıklayarak anlık bir `.r3bundle` paketi oluşturun.
- Tablo üzerinden standart eylem simgeleriyle mevcut yedekleri indirin, geri yükleyin veya silin.
- **.r3bundle Yükle ve Geri Yükle** seçeneğiyle harici bir yedek dosyasını yükleyip tüm platform durumunu onay iletişim kutusuyla geri yükleyin.

### Uzaktan Yönetim CLI Komutları

`r3` CLI istemcisi felaket kurtarma için tam uzaktan yönetim imkanı sunar:

```bash
# Tüm felaket kurtarma snapshot paketlerini listele
$ r3 dr list

# Şifreli snapshot paketi oluştur ve uzak S3 depolamasına eşitle
$ r3 dr create --passphrase "gizli-parolam"

# Bir snapshot ID'sinden platform durumunu geri yükle
$ r3 dr restore <id> --passphrase "gizli-parolam"

# Snapshot paketini (.r3bundle) yerel diske indir
$ r3 dr download <id> -o backup.r3bundle

# Yerel bir paket dosyasından doğrudan geri yükle
$ r3 dr restore-upload -f backup.r3bundle --passphrase "gizli-parolam"
```

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
- `POST /api/disaster-recovery/backups`: Zorunlu parola ve isteğe bağlı S3 yüklemesiyle yeni bir şifreli yedek oluşturur.
- `GET /api/disaster-recovery/backups/{id}/download`: `.r3bundle` dosyasını indirir.
- `DELETE /api/disaster-recovery/backups/{id}`: Bir felaket kurtarma yedeğini siler.
- `POST /api/disaster-recovery/backups/{id}/restore`: Saklanan bir yedek kimliğinden geri yükleme yapar.
- `POST /api/disaster-recovery/restore-upload`: `X-Passphrase` başlığıyla yüklenen bir `.r3bundle` paketinden geri yükleme yapar.


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


## Yapılandırma dosyaları

r3v3rs3 yapılandırmasını `$XDG_CONFIG_HOME/r3v3rs3` veya `$HOME/.config/r3v3rs3` dizinindeki TOML dosyalarında saklar.

Varsayılan konumu `R3V3RS3_CONFIG_DIR` ortam değişkeni veya `--config-dir` komut satırı seçeneğiyle değiştirebilirsiniz.

Bu dosyaları elle de düzenleyebilirsiniz. Ancak r3v3rs3 yapılandırma dosyalarındaki değişiklikleri kendiliğinden algılamaz. Değişikliklerin geçerli olması için dosyayı düzenledikten sonra sunucuyu yeniden başlatın.
