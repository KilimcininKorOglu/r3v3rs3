+++
title = "Sertifikalar ve ACME"
description = "Sunucu, istemci ve kök sertifikalar, ACME, DNS kimlik bilgileri ve CT izleme"
weight = 4
+++

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
| `deployment_started`   | [Deploy platformunun](@/reference/platform.tr.md) bir deployment'ı başlamıştır.                                                           |
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
- r3v3rs3 bildirimleri 16 eşzamanlı worker ile arka planda gönderir; böylece yavaş webhook endpoint'lerinin diğer uyarıları kuyrukta tıkaması (head-of-line blocking) engellenir. Kuyruk en fazla 1024 bildirim tutar. Kuyruk dolarsa kritik alarmlar (sertifika süresi bitimleri, ACME hataları ve deployment arızaları) doğrudan arka plan görevine devredilerek sessizce düşürülmeden iletilir.
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

## ACME

r3v3rs3, sertifikaları [ACME](https://letsencrypt.org/docs/client-options/) (Automatic Certificate Management Environment) ile otomatik alabilir. Let's Encrypt, ZeroSSL, BuyPass ve Google Trust Services gibi birçok sertifika otoritesi ACME'yi destekler.

Bir ACME kaydı bir veya daha fazla alan adı içerir. Alan adlarını "Alan Adları" alanına virgülle ayırarak yazın, örneğin `example.com, *.example.com`. Sertifika her alan adını Subject Alternative Name olarak içerir. r3v3rs3 sertifikayı süresi dolmadan otomatik yeniler. Bir order başarısız olursa r3v3rs3 bir saat sonra yeniden order oluşturur. ACME sekmesindeki kaydın **Yeniden iste** işlemi ve `POST /api/acme/{id}/order`, kaydın sertifikalarını yenileme zamanını ve yeniden denemeyi beklemeden hemen ister.

Sertifika otoritesini seçin (Let's Encrypt, Google Trust Services, ZeroSSL, BuyPass, BuyPass Test veya "Sunucu URL'i" ile "Özel") ve "E-posta Adresi" girin. External Account Binding (EAB) kullanan sertifika otoriteleri (örneğin Google Trust Services veya ZeroSSL), hesabınızın "EAB Key ID" ve "EAB HMAC Key" değerlerini ister. r3v3rs3 yeni sertifikayı son sertifikadan "Yenileme Aralığı (Gün)" (`renewal_days`, varsayılan `60`) gün sonra ister.

### Challenge'lar

Sertifika otoritesi, her alan adını sizin yönettiğinizi bir challenge ile doğrular. Challenge'ı "Challenge" alanından seçin.

| Challenge   | Nasıl çalışır                                                                                                                                            | Gereksinimler                                                                                                                  |
|-------------|----------------------------------------------------------------------------------------------------------------------------------------------------------|--------------------------------------------------------------------------------------------------------------------------------|
| HTTP-01     | Sertifika otoritesi `http://<domain>/.well-known/acme-challenge/<token>` adresine istek gönderir ve r3v3rs3 yanıt verir.                                 | Her alan adı r3v3rs3'e çözümlenmeli, TCP 80 portu açık ve internetten erişilebilir olmalıdır. Wildcard alan adı kullanılamaz.  |
| TLS-ALPN-01 | Sertifika otoritesi, alan adının 443 portuna `acme-tls/1` ALPN protokolüyle bir TLS bağlantısı açar ve r3v3rs3 bir challenge sertifikasıyla yanıt verir. | Her alan adı r3v3rs3'e çözümlenmeli, TCP 443 portu açık ve internetten erişilebilir olmalıdır. Wildcard alan adı kullanılamaz. |
| DNS-01      | r3v3rs3, DNS sağlayıcınızın API'si ile `_acme-challenge.<domain>` TXT kaydını oluşturur.                                                                 | Aşağıdaki tablodaki DNS sağlayıcılarından biri ve zone'u düzenleyebilen bir API kimlik bilgisi.                                |

`*.example.com` gibi bir wildcard alan adı DNS-01 gerektirir. r3v3rs3, HTTP-01 veya TLS-ALPN-01 ile girilen wildcard alan adını reddeder.

TLS-ALPN-01 challenge'ı sürerken her TLS portu ve her HTTPS portu, yalnız `acme-tls/1` sunan bir istemciye challenge sertifikasıyla yanıt verir. Diğer istemciler portun sertifikasını alır. TLS portu, challenge bağlantısı için upstream sunucusuna bağlanmaz. Hiçbir TCP veya HTTP portu "TLS-ALPN Challenge Adresi" ayarındaki portu kullanmıyorsa r3v3rs3, challenge'lar bitene kadar bu adresi dinler. 443 portundaki TLS'siz bir HTTP portu challenge'a yanıt veremez.

### DNS-01

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
| Route 53         | Access Key ID, Secret Access Key                                | `route53:ListHostedZones` ve `route53:ChangeResourceRecordSets`. Private hosted zone'lar atlanır, zone yanıtlarındaki XML entity'leri otomatik olarak çözülür.                                                                                                    |
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

r3v3rs3 bu API'lerin mock sunucularıyla ve [Pebble](https://github.com/letsencrypt/pebble) test sertifika otoritesiyle test edilir. Gerçek sağlayıcı hesaplarıyla test edilmez. Harici DNS ve Git sağlayıcı API'leri bir HTTP 429 Too Many Requests yanıtı döndürdüğünde, r3v3rs3 hız sınırlarını (rate limit) otomatik olarak yönetir: `Retry-After` başlığını ayrıştırır (en fazla 30 saniye) veya üstel geri çekilme uygulayarak isteği başarısız olmadan önce 3 defaya kadar yeniden dener.

#### DNS-01 CNAME Yetkilendirmesi / Alias Modu (RFC 8555)

Harici müşteriler veya üçüncü taraf alan adları için sertifika düzenlerken, müşterilerin ana DNS sağlayıcı API token'larını r3v3rs3'e teslim etmeleri genellikle mümkün veya güvenli değildir. RFC 8555 DNS alias modunda, harici alan adları ACME DNS-01 doğrulamalarını r3v3rs3 tarafından yönetilen bir DNS bölgesine tek seferlik statik bir CNAME kaydı ile devreder.

##### Nasıl çalışır

1. **`dns_alias` yapılandırması:** DNS-01 ile bir ACME siparişi oluştururken `dns_alias` alanını yetki verilen temel zone'a (örn. `auth.example.com`) veya açık şablona (örn. `_acme-challenge.{domain}.auth.example.com`) ayarlayın.
2. **Müşteri CNAME Kurulumu:** Alan adı sahibi, challenge alt alan adını alias hedefine yönlendiren tek bir CNAME kaydı ekler:
   - **Kaynak:** `_acme-challenge.client.com`
   - **Tür:** `CNAME`
   - **Hedef:** `_acme-challenge.client.com.auth.example.com`
3. **Otomatik Oluşturma ve Yenileme:** Doğrulama sırasında r3v3rs3, kendi yapılandırılmış DNS sağlayıcısını (örn. Cloudflare veya Route 53) kullanarak TXT doğrulama kaydını `_acme-challenge.client.com.auth.example.com` üzerinde yayınlar. ACME doğrulama otoritesi `client.com` üzerindeki CNAME kaydını takip ederek `auth.example.com` altındaki TXT kaydını okur ve sertifikayı üretir.

CNAME talimatlarını ve doğrulama durumunu CLI veya MCP üzerinden inceleyin:
```bash
r3 acme get-cname-instructions <ACME_ID>
```

### DNS webhook

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

### DNS exec

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

### RFC 2136

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

## Sertifika Şeffaflığı (CT) İzleme

r3v3rs3, alan adlarınız için genel Sertifika Şeffaflığı (Certificate Transparency) günlüklerini tarayan ve sahte, yetkisiz veya güvenliği ihlal edilmiş SSL sertifikalarını gerçek zamanlı olarak tespit eden bir arka plan izleme motoru içerir.

### Arka Plan İzleme ve Tespit

İzleme motoru yapılandırılabilir bir zamanlamayla (varsayılan: 6 saatte bir) çalışır ve aktif sunucu sertifikaları ile ACME konfigürasyonlarından alan adlarını otomatik olarak keşfeder veya yönetici tarafından belirlenen özel alan adlarını izler:

- **Otomatik Keşif**: Yüklenen tüm sertifikalardan ve ACME taleplerinden kök alan adlarını ve SAN değerlerini otomatik olarak toplar.
- **İzin Verilen Sertifika Otoriteleri**: Önceden tanımlanmış güvenilir genel CA sağlayıcıları (Let's Encrypt, Google Trust Services, DigiCert, Sectigo, ZeroSSL, Cloudflare) veya özel güvenilir sağlayıcı listeleri.
- **Sahte Sertifika Tespiti**: Genel CT günlüklerinde keşfedilen ve tanınmayan veya yetkisiz bir CA tarafından düzenlenen tüm sertifikalar anında `investigating` durumuyla sahte (`is_rogue: true`) olarak işaretlenir.
- **Alarmlar ve Webhook Bildirimleri**: Sertifika metadata bilgileriyle (sağlayıcı, seri numarası, common name, SAN listesi, kayıt zamanı) birlikte `cert.rogue_detected` webhook olayını tetikler.
- **Sorgu Sınırları**: Bir kontrol, CT günlüğünü aynı anda dört alan adı için sorgular. crt.sh bir yanıt için 20 saniyeden fazla süre isteyebildiğinden bir sorgu 120 saniye sürebilir. Günlük sunucusuna bağlantı için yalnız 10 saniye vardır. Sorgusu başarısız olan bir alan adı durumu `partial_error` yapar ve `last_error` içinde adıyla yer alır.

### CLI Yönetimi

CT günlüklerini `r3 cert ct` komut satırı aracı ile doğrudan yönetin:

```bash
# İzleme durumunu ve aktif alarm sayısını görüntüleme
r3 cert ct get-status

# Güncel konfigürasyonu görüntüleme
r3 cert ct get-config

# Konfigürasyonu güncelleme
r3 cert ct update-config --enable true --domains "example.com,api.example.com" --interval 7200

# İsteğe bağlı anlık tarama başlatma
r3 cert ct check-now --domain example.com

# Keşfedilen CT kayıtlarını ve sahte sertifikaları listeleme
r3 cert ct list-entries --rogue-only

# Bir alarmı kapatma
r3 cert ct dismiss-alert <ENTRY_ID>
```

### Model Context Protocol (MCP)

Yapay zeka agent'ları CT günlüklerini MCP araçları üzerinden otonom olarak denetleyebilir ve yönetebilir:
- `get_ct_config`: Güncel izleme konfigürasyonunu alır.
- `update_ct_config`: Tarama aralığını, alan adlarını ve izin verilen CA listesini günceller.
- `get_ct_status`: Gerçek zamanlı durumu, aktif alarmları ve son tarama zamanını getirir.
- `list_ct_entries`: Keşfedilen kayıtları filtrelerle listeler.
- `check_ct_now`: Anlık bir günlük sorgusu başlatır.
- `dismiss_ct_alert`: Bir sahte sertifika alarmını onaylar veya kapatır.


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
