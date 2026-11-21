+++
title = "Yönetim"
description = "Ayarlar, WebUI, yönetim API'si, denetim kaydı ve loglama"
weight = 11
+++

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
| Upstream DNS Çözümleyicisi      | boş           | `http+srv` ve `https+srv` sunucu URL'lerinin SRV sorgularını yanıtlayan DNS sunucusu, örneğin Consul için `127.0.0.1:8600`. Boş bırakılırsa sistemin çözümleyicisi kullanılır. Ayrıntılar için [DNS SRV sunucuları](@/reference/proxies.tr.md#dns-srv-sunuculari) bölümüne bakın. |
| Veritabanı Log Saklama Süresi   | `3months`     | Log'ların log veritabanında ne kadar tutulacağı.                                                                                                                                                                                                         |
| Denetim Kaydı Saklama Süresi    | `1year`       | Denetim kaydındaki bir girdinin ne kadar tutulacağı. Ayrıntılar için [Denetim kaydı](#denetim-kaydi) bölümüne bakın.                                                                                                                                     |
| Sertifika Süre Uyarısı          | `14days`      | Sertifika listesi bu süre içinde sona erecek sertifikayı işaretler. Webhook bu sertifika için bildirim alır. Ayrıntılar için [Bildirimler](@/reference/certificates.tr.md#bildirimler) bölümüne bakın.                                                                                 |
| Webhook URL                     | boş           | Bildirim webhook'u. Boşsa bildirim gönderilmez.                                                                                                                                                                                                          |
| Webhook Token                   | boş           | Webhook isteklerinin bearer token'ı. Admin API bu değeri döndürmez.                                                                                                                                                                                      |
| Webhook Timeout                 | `10s`         | Tek bir webhook isteğinin en uzun süresi.                                                                                                                                                                                                                |

Süreleri `30s`, `15m`, `1h` veya `7days` gibi okunabilir bir biçimde yazın.


## WebUI

r3v3rs3 bir WebUI ile birlikte gelir. WebUI varsayılan olarak `127.0.0.1:46492` adresinde çalışır. Başka bir dinleme adresini, örneğin `0.0.0.0:46492`, `R3V3RS3_WEBUI` ortam değişkeni veya `--webui` komut satırı seçeneğiyle ayarlayın. `R3V3RS3_NO_WEBUI=1` ortam değişkeni veya `--no-webui` komut satırı seçeneği WebUI'ı ve yönetim API'sini kapatır.

Route'u WebUI adresine, örneğin `http://127.0.0.1:46492` adresine giden bir r3v3rs3 proxy'si paneli process içinde servis eder (bkz. [Panel oturumu](@/reference/proxies.tr.md#panel-oturumu)). Bu durumda `R3V3RS3_WEBUI_LISTEN=false` ortam değişkeni veya `--webui-listen=false` komut satırı seçeneği TCP listener'ı kapatır. WebUI adresinde hiçbir şey yanıt vermez. Host'taki başka bir process proxy'nin korumalarını atlayıp yönetim API'sine ulaşamaz, paneli yalnız o proxy servis eder. Listener'ı kapatmadan önce panele başka bir giriş yolu bırakın, çünkü bozuk bir proxy sizi panelin dışında da bırakır. `install.sh` kullanıyorsanız ayarı bir drop-in dosyasına yazın, çünkü script her yükseltmede unit dosyasını yeniden yazar:

```sh
$ sudo mkdir -p /etc/systemd/system/r3-server.service.d
$ printf '[Service]\nEnvironment=R3V3RS3_WEBUI_LISTEN=false\n' | sudo tee /etc/systemd/system/r3-server.service.d/webui-listen.conf
$ sudo systemctl daemon-reload && sudo systemctl restart r3-server
```

Reload ayarı uygulamaz, çünkü kesintisiz yükseltmenin yeni process'i eski process'in ortam değişkenlerini alır. Listener'ı yeniden açmak için dosyayı silin ve servisi yeniden başlatın. Worker modu (`worker_mode = "workers"`) listener olmadan başlamaz, çünkü onun proxy'leri başka process'lerde çalışır ve panele TCP ile bağlanır.

WebUI menüsü soldaki kenar çubuğundadır ve üç gruptan oluşur:

- **Proxy**: **Portlar**, **Proxy'ler**, **Erişim Listeleri** ve **Sertifikalar**.
- **Platform**: **Uygulamalar**, **Hedefler** ve **Git Sağlayıcıları**, yani [deploy platformu](@/reference/platform.tr.md). Bu grup yalnız platform açıkken görünür ve proxy listesi olmayan hesap tarafından görülür. **Git Sağlayıcıları** öğesini yalnız admin veya editör görür.
- **Yönetim**: **Hesaplar**, **Denetim Kaydı** ve **Ayarlar**. Bu grubu yalnız admin hesabı görür.

Üst çubuk logoyu, dil menüsünü, tema menüsünü ve **Çıkış Yap** butonunu içerir. Dar bir ekranda kenar çubuğu ve **Çıkış Yap** yerine üst çubukta **Menü** butonu görünür. Bu buton aynı menüyü açar ve menünün sonunda **Çıkış Yap** bulunur.

WebUI dilini üst çubuktaki bayrak menüsünden seçebilirsiniz: İngilizce veya Türkçe. Tema menüsünde Sistem, Açık ve Koyu seçenekleri bulunur. WebUI bu seçimleri `r3v3rs3_lang` ve `r3v3rs3_theme` cookie'lerinde saklar. Bu cookie'ler yoksa WebUI İngilizce ve sistem temasıyla açılır.

r3v3rs3'ün hata sayfaları ve Panel Oturumu giriş sayfası da bu cookie'lere bakar. Ancak tarayıcı bu cookie'leri yalnız WebUI'ın host'una gönderir. Bu yüzden başka bir host'taki proxy'nin sayfaları İngilizce ve sistem temasıyla açılır.

#### Standart İkonik Eylem Dili

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

## Yönetim API'si

WebUI, `/api` altındaki yönetim API'sini kullanır. r3v3rs3 bu API'nin OpenAPI dokümanını sunucu kodundan üretir. Bu yüzden doküman, çalışan sürümün route'larını listeler.

- OpenAPI dokümanı: `http://localhost:46492/api/openapi.json`
- Swagger UI: `http://localhost:46492/api/docs/`

İki adres de oturum ister. Önce WebUI'a giriş yapın, sonra adresleri aynı tarayıcıda açın. WebUI'ın alt bilgisindeki API linki de Swagger UI'ı açar.

Bir script, `POST /api/login` ile giriş yapar ve yanıttaki `token` cookie'sini sonraki isteklerle gönderir:

```bash
$ curl -c cookies.txt -H 'Content-Type: application/json' \
    -d '{"username":"admin","method":"password","password":"passw0rd"}' \
    http://localhost:46492/api/login
$ curl -b cookies.txt http://localhost:46492/api/ports
```

Sunucu `Secure` özelliğini HTTPS bağlantılarında ve güvenilen ters proxy'lerde otomatik olarak ekler, yerel loopback HTTP bağlantılarında ise kapalı tutar.

TOTP'si açık hesapta ilk yanıt `"totp_required"` olur. Kodu, ilk isteğin cookie'siyle birlikte ikinci bir istekte gönderin:

```bash
$ curl -b cookies.txt -c cookies.txt -H 'Content-Type: application/json' \
    -d '{"username":"admin","method":"totp","token":"123456"}' \
    http://localhost:46492/api/login
```

## Denetim kaydı

r3v3rs3, bir hesabın WebUI veya yönetim API'si ile yaptığı değişiklikleri kaydeder: portlar (sıfırlamalar dahil), proxy'ler (cache temizlemeleri dahil), erişim listeleri, sertifikalar, ACME kayıtları, ayarlar, CDN IP aralığı yenilemeleri ve hesaplar. [Deploy platformu](@/reference/platform.tr.md) için uygulamaları, ortam değişkenlerini, deployment'ları ve geri almaları, Git token'larını ve webhook secret'larını, hedefleri ve agent kayıtlarını, Git sağlayıcı bağlantılarını, yeniden kurulan webhook'ları ve platform ayarlarını kaydeder. İmzalı bir Git sağlayıcı webhook'unun başlattığı deployment da kaydedilir. Yönetim paneline her giriş, her başarısız giriş denemesi ve her çıkış da kaydedilir. r3v3rs3'ün kendi yaptığı değişiklikler kaydedilmez. Sertifika yenileme ve keşfedilen proxy'ler buna örnektir.

Her kayıtta zaman, hesap, istemci IP adresi, işlem, değişen kaynağın id'si ve kısa bir özet bulunur. Özet isimleri, adresleri ve rolleri içerir. Parola, token veya key içermez.

Bir kaydın işlem adı, değişikliği yapan API işleminin operation id'sidir. Bu ad, işlemin MCP aracının adıyla aynıdır: `create_proxy`, `upload_app_file`, `run_prune`. İki işlemin kendine ait bir API işlemi yoktur: `login_failed` (reddedilen bir oturum açma denemesi) ve `enroll_agent` (kendini kaydeden bir agent). Eski sürümlerin `add_proxy` veya `backup_database` gibi önceki adlarla yazdığı kayıtlar, sunucu `log.db` dosyasını açtığında güncel adlara taşınır. Birden fazla işlemi kapsayan eski bir ad, bu işlemlerin ilkine taşınır.

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

## Log

r3v3rs3 varsayılan olarak log'ları standart çıktıya yazar. Bunu `R3V3RS3_LOG`, `R3V3RS3_ACCESS_LOG` ortam değişkenleriyle veya `--log`, `--access-log` komut satırı seçenekleriyle değiştirebilirsiniz.

```bash
$ r3v3rs3 start --log /var/log/r3v3rs3/r3v3rs3.log --access-log /var/log/r3v3rs3/access.log
```

Dizin içeren bir yol o dizine yazar. Yalnız dosya adı verilirse dosya log dizinine yazılır. r3v3rs3 her saat yeni bir dosya açar ve ada tarihle saati ekler, örneğin `r3v3rs3.log.2026-09-25-10`.

Log dizini bu dosyaları ve log sayfasıyla denetim kaydının veritabanı olan `log.db` dosyasını tutar. Dizini `R3V3RS3_LOG_DIR` ortam değişkeni veya `--log-dir` komut satırı seçeneği belirler. Varsayılan dizin `$XDG_DATA_HOME/r3v3rs3/logs` veya `$HOME/.local/share/r3v3rs3/logs` olur. Periyodik log veritabanı temizliği ana sunucu event loop'u dışına alınarak arka planda asenkron çalışır, böylece döngü tıkanması ve bağlantı gecikmesi sıçramaları önlenir.

Log seviyesini değiştirmek için `R3V3RS3_LOG_LEVEL`, `R3V3RS3_ACCESS_LOG_LEVEL` ortam değişkenlerini veya `--log-level`, `--access-log-level` komut satırı seçeneklerini kullanın. Varsayılan seviye `info` olur. `off` log'u kapatır. `R3V3RS3_LOG_FORMAT` veya `--log-format` iki log için `text` (varsayılan) veya `json` biçimini seçer.

## Gerçek Zamanlı Trafik Akışı ve Canlı Erişim Logu

r3v3rs3, dosya loglarının yanı sıra bellek içi (in-memory) gerçek zamanlı bir erişim logu halka arabelleği (ring buffer) ve canlı akış motoru barındırır. Ters proxy üzerinden geçen HTTP ve WebSocket istekleri; durum kodları, gecikme süreleri, istemci IP'leri, host, yol ve HTTP metotlarıyla anlık olarak yakalanır.

### WebUI Trafik Akışı Paneli

Yönetim paneli üzerinde `/traffic` adresinden ve proxy detay sayfalarından erişilebilen etkileşimli bir canlı trafik paneli bulunur:
- **Gerçek Zamanlı Akış**: Sunucu Tarafından Gönderilen Olaylar (SSE) ile yeni istekler geldiği anda tabloya gerçek zamanlı olarak akar.
- **Durum Kodları Renklendirmesi**: `2xx` (başarılı), `3xx` (yönlendirme), `4xx` (istemci hatası) ve `5xx` (sunucu hatası) için renkli rozetler.
- **Gecikme Süresi İzleme**: Anlık yanıt gecikme süreleri görsel rozetlerle (< 50ms yeşil, 50–300ms sarı, > 300ms kırmızı) listelenir.
- **Etkileşimli GeoIP Dünya Haritası ve Yoğunluk Isı Haritası**: Gelen istemci IP adresleri gömülü MaxMind GeoIP veritabanı üzerinden ISO ülke kodlarına çözümlenir. Harita doğrudan arka plandaki `GET /api/traffic/geo` endpoint'ine bağlanarak tüm geçmiş ve kalıcı coğrafi dağılımı renk yoğunluğuna dayalı ısı haritası ile görselleştirir. Ülkelerin üzerine gelindiğinde istek sayısı ve yüzdesi görüntülenir; haritadaki bir ülkeye veya sıralama çubuğuna tıklandığında canlı trafik akışı anında o ülkeye filtrelenir.
- **En Çok İstek Alan Ülkeler Sıralaması**: Arka uç dağıtım verilerinden beslenen en çok trafik alan ilk 10 ülkeyi ilerleme çubukları (progress bars), istek sayıları ve yüzde paylarıyla listeler.
- **Coğrafi ve Metin Filtreleme**: Ülkeye, HTTP durum koduna, URL yoluna, istemci IP'sine, host'a veya HTTP metoduna göre anında filtreleme.
- **Kontroller**: Akışı duraklatma/devam ettirme, otomatik kaydırma (auto-scroll), harita görünümünü açma/kapatma, arabelleği temizleme ve JSON formatında dışa aktarma.
- **Canlı İstek İnceleyici ve Detay Çekmecesi (Inspector Drawer)**: Canlı trafik tablosundaki herhangi bir satıra tıklandığında derinlemesine inceleme sekmeleri sunan bir yan çekmece açılır:
  - **Genel Bakış (Overview)**: İstemci IP adresi, GeoIP ülkesi, HTTP protokolü, yanıt gecikmesi, zaman damgası, durum kodu ve eşleşen proxy rotası gibi temel istek meta verileri.
  - **Başlıklar (Headers)**: Gelen istemci istek başlıklarının ve giden sunucu yanıt başlıklarının yapılandırılmış anahtar-değer tablosu.
  - **Yük/Gövde (Payload)**: Arabelleğe alınan istek gövdelerini (16 KB'a kadar) biçimlendirilmiş JSON ve ham metin görünümleri arasında tek tıkla geçiş yaparak inceleme.
  - **Gizlenen secret değerleri**: Access log'u Read yetkisi olan her hesap görür. Bu yüzden r3v3rs3 bir secret yerine `[REDACTED]` saklar. Bu kural `Authorization`, `Proxy-Authorization`, `Cookie`, `Set-Cookie`, `X-API-Key`, `API-Key` header'larının ve adında `password`, `secret`, `token`, `apikey`, `api_key` veya `credential` geçen her header'ın değerine uygulanır. Sorgu dizgisinde, form gövdesinde veya JSON gövdesinde bu adları taşıyan her alanın değeri de gizlenir. Alan adı denetlenmeden önce percent-decode edilir.
  - **Tekrarlama & cURL Konsolu (Replay)**: Uç birim için güvenli kaçış karakterleriyle oluşturulmuş **cURL Olarak Kopyala** butonu ve yakalanan istekleri doğrudan kaynak sunuculara veya özel bir URL'ye yeniden gönderen, yanıt durumunu ve gidiş-dönüş gecikme süresini anında ölçen **İsteği Tekrarla** konsolu.

### Yönetim API'si (Admin API)

Trafik uç noktaları oturum doğrulaması ile korunur ve çok kiracılı proxy erişim sınırlarını (scoping) uygular:

- `GET /api/traffic`: İsteğe bağlı filtrelerle (`status`, `search`, `country`, `resource_id`, `limit`, `since_id`) eşleşen son erişim loglarını döner.
- `GET /api/traffic/live`: Gelen erişim loglarını `text/event-stream` (Server-Sent Events) olarak gerçek zamanlı akıtır.
- `GET /api/traffic/geo`: Canlı erişim trafiğinin coğrafi dağılımını toplu olarak döner; toplam çözümlenen istek sayısını, ilk 10 ülkeyi (`country`, `count`, `percentage`) ve tüm ISO ülke kodlarının istek sayılarını içerir.
- `GET /api/traffic/{id}`: Belirtilen kimliğe (ID) sahip erişim kaydının yakalanan istek/yanıt başlıkları, gövde içeriği, içerik türü ve WebSocket bayrakları dahil tüm detaylarını döner.
- `POST /api/traffic/replay`: Belirtilen hedef URL'ye özel başlıklar, gövde ve zaman aşımı süresi (500ms ile 30s arasında sınırlandırılmış) ile bir HTTP veya WebSocket el sıkışma isteğini yeniden gönderir; yanıt durumu, gecikme süresi, hata detayı ve yanıt gövdesini döner.

### Canlı Bağlantı İzleyici ve WebSocket/SSE Akış Denetçisi

Kısa ömürlü istek loglarına ek olarak r3v3rs3, reverse proxy'ler üzerindeki aktif uzun ömürlü bağlantıları ve akışları gerçek zamanlı olarak takip eder:
- **İzlenen Protokoller**: WebSocket bağlantıları, HTTP Upgrades, Server-Sent Events (SSE) ve ham TCP proxy oturumları.
- **Gerçek Zamanlı Telemetri**: İstemci adresi, hedef upstream, aktif süre, gönderilen bayt, alınan bayt ve anlık aktarım hızı (saniye başına bayt).
- **Zarif Kapatma**: Güvenliği tehdit eden veya kaynak tüketen bağlantıları WebUI, CLI veya MCP üzerinden derhal sonlandırın. WebSocket akışlarına temiz kapatma frame'leri gönderilir, TCP soketleri iki yönlü kapatılır.

#### WebUI

`/traffic` adresindeki Canlı Trafik paneli özel bir **Aktif Bağlantılar** sekmesi sunar:
- Renkli protokol rozetleriyle (WebSocket için mor, TCP için mavi, SSE için yeşil, HTTP Upgrade için çivit mavisi) aktif akışları listeler.
- Bağlantı ID, istemci adresi, upstream hedefi, aktif süre, aktarılan bayt ve anlık hız bilgilerini içeren canlı tablo.
- SweetAlert2 onay penceresiyle kırmızı sonlandırma butonu üzerinden tek tıkla bağlantı sonlandırma.

#### CLI

Uzun ömürlü bağlantıları terminalden %100 eşitlikle yönetin:

```bash
# Tüm aktif uzun ömürlü bağlantıları listele
r3 traffic connections list

# Aktif bağlantıları proxy ID ile filtrele
r3 traffic connections list --proxy prx_01j7xyz

# Bir aktif bağlantıyı ID ile sonlandır
r3 traffic connections terminate con_01j7xyz
```

#### Model Context Protocol (MCP)

Yapay zeka agent'ları aktif bağlantıları MCP araçlarıyla otonom olarak denetleyebilir ve yönetebilir:
- `list_traffic_connections`: Protokol, istemci IP, hedef, süre, aktarılan bayt ve hız metrikleriyle tüm aktif uzun ömürlü bağlantıları listeler. İsteğe bağlı `proxy_id` filtresi destekler.
- `terminate_traffic_connection`: Belirtilen ID'ye sahip aktif bağlantıyı derhal sonlandırır. `edit` yetkisi gerektirir.

#### Yönetim API'si (Admin API)
