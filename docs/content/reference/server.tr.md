+++
title = "Sunucu işletimi"
description = "Worker süreçleri, yükseltmeler, HTTP limitleri, akış önceliği ve metrikler"
weight = 9
+++

## Worker Süreçleri

Varsayılan olarak admin API'yi çalıştıran aynı süreç her portu da servis eder. `config.toml` içinde `worker_mode = "workers"` ayarıyla r3v3rs3 portları sunan bir worker süreci başlatır. Master süreç durumu, admin API'yi, WebUI'yi ve deployment platformunu tutar. Worker'ın çökmesi admin API'yi düşürmez ve master worker'ı bir backoff ile yeniden başlatır.

```toml
worker_mode = "workers"
```

Master, güncel yapılandırmayı ve dinleme socket'lerinin descriptor'larını worker'a özel bir Unix socket üzerinden gönderir, bu yüzden worker'ın yapılandırma dizinine erişimi gerekmez. Worker'ın raporladığı durum, process id'si, uptime'ı ve açık bağlantı sayısı WebUI ayarlar sayfasında ve `GET /api/workers` üzerinde görünür. Yanıt vermeyi bırakan bir worker, durumu olmadan listede kalır.

## Kesintisiz Yükseltme

`kill -USR2 <pid>` dinleyicileri yeni bir sürece devreder. Eski süreç yeni binary'yi başlatır, yeni sürecin durumunu yüklemesini bekler, dinleme socket'lerinin descriptor'larını bir Unix socket üzerinden geçirir ve ardından açık bağlantılarını ve aktif platform arka plan görevlerini (veritabanı dump'ları, deployment'lar ve zamanlanmış komutlar) `shutdown_graceful_timeout` (varsayılan 30 saniye) süresince koordine ederek boşaltıp 0 çıkış koduyla çıkar. Kapanma sırasında cancellation token'lar zamanlayıcıları ve çalışan deployment'ları uyararak güvenli temizlik sağlar, bozuk container durumlarını veya hasarlı yedek dosyalarını engeller.

Yeni süreç aynı dizinden yapılandırmayı okur, bu yüzden yükseltme `config.toml` içindeki her ayar için geçerli olur. `worker_mode = "workers"` ile portları worker'lar tuttuğu için yükseltme devredilecek bir şey bulmaz ve master'ın sıralı yeniden başlatması worker'ın açık bağlantılarına dokunmadan olur.

systemd ile `install.sh`, servisi `Type=notify` ve `ExecReload=/bin/kill -USR2 $MAINPID` olarak yazar; `systemctl reload r3-server` yeniden başlatmadan yükseltir. Sunucu hazır olduğunda systemd'ye haber verir ve devir, ana süreci yeni sürece taşır, böylece eski süreç bağlantılarını boşaltırken systemd servisi takip etmeye devam eder.

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

## Uyarlamalı Akış Önceliklendirme (RFC 9218)

r3v3rs3, çoklu çoğullanan (multiplexed) HTTP/2 ve HTTP/3 bağlantılarını optimize etmek için RFC 9218 Genişletilebilir Öncelikler (Extensible Prioritization Scheme for HTTP) standardını uygular. Yüksek eşzamanlılığa sahip ortamlarda, kritik sayfa işleme kaynakları (HTML belgeleri, CSS, çekirdek JavaScript ve etkileşimli API'ler), toplu arka plan indirmeleri, resimler veya medya akışlarına göre önceliklendirilir.

### Öncelik Kademeleri ve Yapılandırılmış Alanlar

RFC 9218, önceliği `Priority` HTTP header'ı ile aciliyet (`u=0..7`) ve artımlı bayrak (`i`) kullanarak iletir. İstemcinin gönderdiği geçerli bir `Priority` header'ı aşağıdaki kurallardan önce gelir. RFC 9218'in istediği gibi r3v3rs3, aralık dışı ya da yanlış türde değer taşıyan üyeyi (örneğin `u=8` veya `i=1`) yok sayar. Geçerli üyesi olmayan header hiç gönderilmemiş sayılır.

| Aciliyet (`u`) | Sınıf | Kaynak Türleri | Ağırlık | İletim Davranışı |
|---|---|---|---|---|
| `0` | Kritik | HTML başlangıç noktaları, kritik güvenlik/ödeme API'leri | 256 | Tam bant genişliğiyle derhal iletilir |
| `1` | Yüksek | CSS stilleri, kritik JS, REST API | 128 | Yüksek eşzamanlılık, medyadan önce işlenir |
| `2` | Orta-Yüksek | Web fontları (`woff2`), kritik veri çekme | 64 | Görsel medyadan önce teslim edilir |
| `3` | Normal | Varsayılan HTTP istekleri, genel API | 32 | Standart eşit paylaşımlı çoğullama |
| `4` | Orta-Düşük | Favicon'lar, kritik olmayan alt kaynaklar | 16 | Kapasite elverdikçe iletilir |
| `5` | Düşük | Resimler (`png`, `webp`, `avif`, `jpg`) | 8 | Artımlı parçalama (`i=?1`) |
| `6` | Arka Plan | Video ve ses akışları (`mp4`, `webm`) | 4 | Hız ayarlı akış parçaları |
| `7` | En Düşük | Telemetri, arşivler, arka plan indirmeleri | 2 | Sistem sıkışıklığında hız kısıtlanır |

### Öncelik Yapılandırması

Akış önceliklendirmesi her HTTP proxy'sinde etkinleştirilebilir ve port başına geçersiz kılınabilir:

- **Priority Header Enjeksiyonu (`inject_header`)**: Upstream HTTP/2 ve HTTP/3 sunucularının multiplexed çerçeveleri verimli bir şekilde zamanlayabilmesi için normalize edilmiş `Priority: u=..., i` header'larını iletir.
- **Uyarlamalı Eşzamanlılık (`adaptive_concurrency`)**: Her istemci aynı anda en fazla 32 arka plan akışı (`u >= 5`) açabilir. O istemcinin bundan sonraki arka plan isteği `Retry-After: 1` ile `429 Too Many Requests` yanıtı alır. Diğer istemciler bundan etkilenmez. Güvenilir bir proxy arkasında istemci, istemci IP ayarlarının çözdüğü adrestir.
- **Özel MIME ve Yol Kuralları (`custom_rules`)**: Belirli route'ları belirlenmiş aciliyet seviyelerine eşleyen özel kalıp kuralları (örn. `/api/critical/*`, `image/svg+xml`). Aciliyeti 0 ile 7 arasında olmayan kural `invalid_priority_urgency` hatasıyla reddedilir.

CLI komutları:
```sh
# Proxy akış önceliklendirme yapılandırmasını görüntüleme
r3 proxy get-stream-priority <PROXY_ID>

# Header iletimiyle birlikte uyarlamalı önceliklendirmeyi etkinleştirme
r3 proxy update-stream-priority <PROXY_ID> --enable true --inject-header true --adaptive-concurrency true

# Genel akış öncelik ve geri basınç istatistiklerini görüntüleme
r3 traffic get-priority-stats
```

MCP araçları: `get_proxy_stream_priority`, `update_proxy_stream_priority`, `get_traffic_priority_stats`.

### WebUI Yönetimi ve Telemetri

Akış önceliklendirmesi doğrudan WebUI üzerinden yönetilebilir:
- **Proxy Yapılandırması**: Proxy gelişmiş ayarları altında *Akış Önceliklendirme (RFC 9218)* seçeneğini etkinleştirin, *Priority Başlığı Ekle* ve *Uyarlamalı Eşzamanlılık* alt seçeneklerini yapılandırın.
- **Port Ayarları**: HTTP/2 veya HTTP/3 dinleyen herhangi bir portta *Akış Önceliklendirme Optimizasyonu* ayarını etkinleştirin.
- **Canlı Trafik Telemetrisi**: `/traffic` sayfası, aktif çoklu akışları, öncelik sınıflandırma dağılımını (Kritik, Normal, Arka Plan), geri basınç geciktirme sayaçlarını ve görsel dağılım çubuğunu gösteren özel bir *Akış Önceliklendirmesi* sekmesi içerir.

## Metrics

`GET /api/metrics` çalışan sürecin sayaçlarını Prometheus text formatında (`text/plain; version=0.0.4`) döndürür:

- `r3v3rs3_http_requests_total`, `r3v3rs3_h3_requests_total`: TCP/TLS ve QUIC üzerinde proxy'lenen istekler, proxy ID (`proxy="..."`) ve route (`route="..."`) etiketleriyle.
- `r3v3rs3_rx_bytes_total`, `r3v3rs3_tx_bytes_total`: tüm aktarım kodlamalarında istemcilerden alınan ve istemcilere iletilen toplam payload akış baytları.
- `r3v3rs3_http_responses_by_status_total`: durum sınıfına göre ayrılmış HTTP yanıtları (`2xx`, `3xx`, `4xx`, `5xx`).
- `r3v3rs3_upstream_errors_total`: başarılı bir upstream yanıtı olmadan yanıtlanan istekler, proxy ID (`proxy="..."`) ve route (`route="..."`) etiketleriyle.
- `r3v3rs3_upstream_errors_by_kind_total`: hata türüne göre ayrılmış upstream hataları (`timeout`, `connect`, `circuit_open`, `no_route`, `other`).
- `r3v3rs3_http_request_duration_seconds`: proxy ID (`proxy="..."`) ve route (`route="..."`) etiketli, standart sepetlerle (`0.005s` - `10s`), toplam süre ve sayaç içeren istek gecikme histogramı.
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

İsteği bir admin session cookie'si veya geçerli bir MCP / API Bearer token'ı yetkilendirir. Bir Prometheus job'ı `bearer_token` ile scrape ederken WebUI cookie'sini kullanmaya devam edebilir. Proxy listesi olan bir hesap, `proxy` etiketli serileri (`r3v3rs3_http_requests_total`, `r3v3rs3_upstream_errors_total`, `r3v3rs3_http_request_duration_seconds`) yalnız kendi proxy'leri için alır. `proxy` etiketi olmayan seriler sürecin toplamı olarak kalır. Sayaçlar tek süreci sayar: worker modunda portları worker sunar ve kendi sayaçlarını tutar, worker durumu ayrıca `GET /api/workers` üzerinden raporlanır.

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

### Liveness ve Readiness Probları

r3v3rs3, container orkestratörleri (Kubernetes, Docker Swarm, Nomad), yük dengeleyiciler ve systemd servisleri için kimlik doğrulama gerektirmeyen standart sağlık probu endpoint'leri sunar:

- `GET /healthz`: Liveness probu. Admin HTTP sunucusu çalıştığı sürece HTTP `200 OK` ve `{"status":"ok"}` döner. Orkestratörlerin kilitlenen bir container sürecini yeniden başlatması için kullanılır.
- `GET /readyz`: Readiness probu. Dahili sunucu event loop döngüsünün yanıt verdiğini ve veritabanı depolama bağlantısının sağlıklı çalıştığını doğrular. Trafik kabul etmeye hazır olduğunda HTTP `200 OK` ve `{"status":"ready"}`, başlatma aşamasında, veritabanı kesintisinde veya dahili kilitlenmede HTTP `503 Service Unavailable` ve `{"status":"not_ready","message":"..."}` döner.

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
  - `/.env`, `/.git`, `/.aws`, `/wp-admin`, `/wp-login.php`, `/xmlrpc.php`, `/phpmyadmin` ve `/config.json` gibi tarayıcı ve sonda kalıplarını otomatik olarak yakalar. Bir kalıp yalnız isteğin path ve query kısmıyla karşılaştırılır, host ile karşılaştırılmaz. Bu yüzden `phpmyadmin.example.com` gibi bir host `/phpmyadmin` kalıbına takılmaz. Bir kalıp yalnız tam path segmentleriyle eşleşir: path'in sonunda ya da `/`, `.`, `?`, `&` veya `#` karakterinden önce biter. `/.git` kalıbı `/.git/config` ile, `/wp-login` kalıbı `/wp-login.php` ile eşleşir, ama `/.git` kalıbı `/.github` ile eşleşmez. Eşleşen her istek, engellenen bir durum kodu gibi bir ihlal sayılır.
  - Kayan bir zaman penceresinde (varsayılan: 60 saniye) peş peşe alınan HTTP durum ihlallerini (`404`, `403`) biriktirir. Yalnız r3v3rs3'ün kendi oluşturduğu yanıtlar sayılır: bilinmeyen bir host, IP filtresinin reddettiği bir istek, başarısız bir giriş veya başka bir route politikası. Upstream uygulamanın döndürdüğü `403` veya `404` hiçbir zaman sayılmaz. Böylece bir uygulamanın giriş sayfası kullanıcılarını karantinaya almaz.
  - İhlaller belirlenen eşiğe ulaştığında (varsayılan: 5 ihlal) ilgili client IP adresini anında karantinaya alır.
- **İzinli Adresler**:
  - `allowed_ips` listesindeki adresler ve CIDR ağları hiçbir zaman karantinaya alınmaz, örneğin yöneticilerin ağı. Karantinadaki bir adresi listeye eklemek karantinasını hemen kaldırır. İzinli bir adresi elle karantinaya alma isteği `quarantine_allowed_ip` hatasıyla reddedilir.
- **Her Portta Drop Yaptırımı**:
  - Karantinaya alınan IP'lerden gelen bağlantılar, HTTP istek çözümlemesi veya TLS handshake yapılmadan TCP dinleyici seviyesinde anında kapatılır (TCP Drop / Reset). Sunucu CPU ve bellek tüketimi sıfıra indirilir.
  - UDP portunda karantinadaki IP'nin paketleri atılır ve açık session'ı kapanır. Upstream sunucu bu paketleri almaz ve cevap vermez.
  - PROXY protocol header'ı okuyan bir TCP portunda client, header'daki kaynak adrestir. Header'ı karantinadaki bir IP'yi gösteren bağlantı, upstream sunucuya ulaşmadan kapanır.
- **Webhook Bildirimi**:
  - Bir IP karantinaya alındığında otomatik olarak `security.threat_detected` webhook olayı tetiklenir.
- **Yönetim ve WebUI Kontrolleri**:
  - Karantinadaki IP'leri, ihlal sayılarını ve kalan karantina sürelerini **Güvenlik > Karantinadaki IP'ler** sekmesinde görüntüleyin.
  - WebUI üzerinden tek tıkla herhangi bir IP adresini manuel karantinaya alın veya serbest bırakın.
  - Sunucuda `sudo r3-server unblock <ip> --config-dir /etc/r3v3rs3` komutu, panel oturumu olmadan bir adresi karantinadan ve giriş yasağından çıkarır (bkz. [Engellenen bir adresi serbest bırakma](@/tutorials/install-linux.tr.md#engellenen-bir-adresi-serbest-birakma)).
  - Eşiği, pencereyi, süreyi, engellenen durum kodlarını, tarayıcı yollarını ve izinli adresleri aynı sekmedeki **Otomatik Karantina Ayarları** kartında değiştirin.

```toml
[auto_quarantine]
enabled = true
max_violations = 5
window_seconds = 60
ban_duration_seconds = 3600
block_status_codes = [404, 403]
allowed_ips = ["198.51.100.0/24"]
```

#### Admin API

- `GET /api/security/quarantine`: Aktif durumdaki karantinalı IP kayıtlarını listeler.
- `POST /api/security/quarantine`: Bir IP adresini manuel olarak karantinaya alır (`{"ip": "...", "reason": "...", "duration_seconds": 3600}`).
- `DELETE /api/security/quarantine/{ip}`: Bir IP adresini karantinadan erken çıkarır.
- `GET /api/security/quarantine/config`: Mevcut otomatik karantina yapılandırmasını döndürür.
- `PUT /api/security/quarantine/config`: Otomatik karantina yapılandırmasını günceller ve kalıcı olarak kaydeder.
