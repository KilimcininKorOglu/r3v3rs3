+++
title = "Proxy'ler"
description = "Yönlendirme, yük dengeleme, güvenlik, cache ve diğer tüm proxy seçenekleri"
weight = 3
+++

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

- İstekleri en düşük öncelik değerine sahip kayıtlar alır. RFC 2782'nin istediği gibi daha yüksek değerli kayıt yedektir: Yalnız daha düşük değerli bütün sunucular sağlıksızsa ya da isteği karşılayamadıysa istek alır.
- Kaydın SRV ağırlığı, sunucunun `weight` değeri olur. `0` ağırlığı `1` olur, çünkü r3v3rs3'te ağırlığı `0` olan sunucu yeni istek almaz. SRV sunucusunun kendisine `weight` verilemez.
- r3v3rs3, yanıtın TTL süresi bitince adı yeniden çözer. En erken 5 saniye sonra çözer. Hedefler değişince route yeni sunucuları yeniden başlatmadan alır. Kalan sunucuların sağlık durumu korunur. Başarısız bir sorgu (örneğin SERVFAIL yanıtı ya da `timeout`) son hedefleri korur ve 5 saniye sonra yeniden denenir. Adın olmadığını (NXDOMAIN) ya da SRV kaydı bulunmadığını bildiren yanıt geçerli bir yanıttır (RFC 8767), bu yüzden hedefleri kaldırır. Sonraki sorgu yanıtın negatif TTL süresine göre, en erken 5 saniye sonra yapılır.
- SRV adının hedefi olmayan route 502 döner. Route'un başka sunucuları varsa istekleri onlar alır.
- "Upstream DNS Çözümleyicisi" ayarı ([Ayarlar](@/reference/administration.tr.md) bölümüne bakın) SRV sorgularının DNS sunucusunu seçer, örneğin Consul DNS için `127.0.0.1:8600`. Ayar boşsa r3v3rs3 sistemin çözümleyicisini kullanır. Hedeflerin host adlarını bağlantı sırasında sistemin çözümleyicisi çözer.

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
- `error_page` (boolean, varsayılan `false`): `body` yerine, proxy'nin [hata sayfası temasındaki](#merkezi-hata-sayfasi-temalari-havuzu) `status` koduna ait sayfayla, ziyaretçinin dilinde ve renk temasında yanıt verir. Proxy formunda **Hata sayfası temasını kullan** olarak görünür. [Durdurulmuş bir platform uygulamasının](@/reference/platform.tr.md#durdurma-baslatma-ve-yeniden-baslatma) proxy'si bu seçeneği `503` durum koduyla kullanır.

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
error_page_theme = "a1b2c3d4"
hide_branding = true
error_pages = [
  { status = 404, body = "<h1>{{host}} üzerinde Sayfa Bulunamadı</h1><p>Aradığınız sayfa mevcut değil.</p>" },
  { status = 502, body = "<h1>Hizmet Kullanılamıyor</h1><p>Sunucularımız şu anda yanıt vermiyor. (İzleme: {{request_id}})</p>" },
]
routes = [{ path = "/", servers = [{ url = "http://127.0.0.1:8080/" }] }]
```

### Merkezi Hata Sayfası Temaları Havuzu

Proxy düzeyinde her seferinde ayrı ayrı HTML sayfaları girmek yerine r3v3rs3, WebUI üzerinde **Proxies > Hata Sayfası Temaları** altında platform genelinde yönetilen merkezi bir **Hata Sayfası Temaları (Error Page Themes)** havuzu sunar:

- **Yerleşik Temalar**: Tüm standart hata durum kodları (`400`, `401`, `403`, `404`, `405`, `408`, `429`, `500`, `502`, `503`, `504`) için hazır modern temalarla (`Minimalist Dark`, `Modern Glassmorphism`, `Developer Terminal`, `Illustration Card`, `Maintenance Notice`) birlikte gelir.
- **Etkileşimli Görsel Editör Modal'ı**: WebUI üzerinden canlı responsive önizleme görünümleri (Masaüstü, Tablet, Mobil), HTML kod editörü ve durum kodu şablonlarıyla temaları doğrudan düzenleme ve test etme imkânı.
- **Özel Tema Yönetimi**: WebUI üzerinden `+ Yeni Tema` modal'ı veya CLI (`r3 error-page-theme create` / `r3 error-page-theme update`) ile benzersiz kimlikli özel temalar oluşturma ve düzenleme.
- **Proxy Tema Ataması**: `error_page_theme = "short_id"` ile herhangi bir proxy'ye WebUI, CLI (`r3 proxy update <id> ...`) veya REST API üzerinden tek tıkla hata sayfası teması bağlanabilir.
- **Proxy Bazlı İnce Ayar / Geçersiz Kılma**: Proxy yapılandırmasında tanımlanan özel `error_pages` girdileri, o durum kodu için atanan temayı geçersiz kılarak öncelik alır.
- **Yazılım İmzasını Gizleme**: HTTP proxy üzerinde `hide_branding = true` ayarlanarak varsayılan hata sayfalarındaki "Powered by r3v3rs3" bağlantısı gizlenebilir.
- **Varsayılanlara Sıfırlama**: WebUI, CLI (`r3 error-page-theme reset`) veya MCP (`reset_error_page_themes`) üzerinden yerleşik temalar her zaman varsayılan şablonlara geri döndürülebilir.

### Dinamik Şablon Değişkenleri

Hata sayfası şablonları çalışma zamanında dinamik değişken değişimini destekler. İstemciye hata sayfası yanıtı gönderilmeden önce r3v3rs3 aşağıdaki belirteçleri otomatik olarak doldurur:

| Değişken | Açıklama | Örnek |
|---|---|---|
| `{{status}}` | HTTP durum kodu | `502` |
| `{{message}}` | Standart durum ifadesi veya hata açıklaması | `Bad Gateway` |
| `{{request_id}}` | İstek için benzersiz izleme (tracing) kimliği | `184f9a0b123` |
| `{{host}}` | İstenen Host başlığı veya sunucu adı | `app.example.com` |
| `{{client_ip}}` | İstemci IP adresi (veya güvenilen CDN proxy IP'si) | `203.0.113.195` |

## UDP oturumları

UDP proxy her istemci adresi için ayrı bir oturum açar. Her oturumun upstream sunucuya giden kendi socket'i vardır. Bu yüzden upstream sunucu her istemciyi farklı bir kaynak porttan görür. r3v3rs3 upstream sunucunun yanıtlarını dinlediği porttan istemciye geri gönderir.

İki yönde de `session_idle_timeout` süresince (varsayılan `60s`) paket geçmezse oturum kapanır. Upstream socket hata verdiğinde veya portun upstream sunucuları ya da boşta kalma timeout'u değiştiğinde de oturum kapanır. İstemcinin sonraki paketi yeni bir oturum açar. Bir port en fazla 10.000 oturum tutar. Bu sınıra ulaşılınca r3v3rs3 yeni istemcilerin paketlerini düşürür.

`max_datagram_size`, UDP paketleri için bayt cinsinden maksimum yük boyutunu belirler (512 ile 65527 arasında, varsayılan `65527`). Bu boyuttan büyük paket iki yönde de atılır, çünkü kırpılan paket karşı tarafa bozuk veri olarak ulaşır.

## Upstream timeout'ları

r3v3rs3 upstream sunucuyu sınırlı bir süre bekler. Timeout değerlerini `500ms`, `10s` veya `1m` gibi yazın.

- HTTP / HTTPS proxy'sinde `timeouts.connect`, TCP / TLS üzerinden TCP proxy'sinde `connect_timeout` değeri yeni bir upstream bağlantısının DNS sorgusunu, TCP bağlantısını ve TLS handshake'ini sınırlar. Varsayılan değer `10s`'dir.
- HTTP / HTTPS proxy'sinde `timeouts.request` değeri, istek başladıktan yanıt header'ları gelene kadar geçen süreyi sınırlar. Yeni bağlantının kurulma süresi de buna dahildir. Varsayılan değer `60s`'dir. `0s` limiti kapatır. Yanıt gövdesi, WebSocket ve diğer upgrade edilmiş bağlantılar için limit yoktur. r3v3rs3'ün kendi WebUI adresine giden route'ta da istek limiti yoktur. Panel aynı process içinde çalışır. Bu yüzden yedek alma, geri yükleme veya panelin başka uzun bir işi, iş bitince yanıt verir.
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
- `geo_proximity` trafiği MaxMind GeoIP ülke ve bölgesel şablonlarına (ör. `eu`, `na`, `mena`, `apac`, `turkey`) göre istemcinin coğrafi konumuna en yakın upstream sunucuya yönlendirir. En yakın sunucu sağlıksız veya devresi açıksa trafik otomatik olarak bir sonraki en yakın veya genel yedek sunuculara aktarılır.
- `latency` trafiği ölçülen en düşük gidiş-dönüş yanıt gecikmesine (RTT) sahip sağlıklı upstream sunucusuna yönlendirir.

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

- `health_check.grpc_service` tanımlanan HTTP proxy, standart gRPC Health Checking Protocol (`grpc.health.v1.Health/Check`) üzerinden HTTP/2 ile (`h2c` prior knowledge veya TLS ALPN `h2`) upstream sunucuları yoklar. `grpc_service = ""` değeri genel sunucu sağlığını sorgular; spesifik bir servis adı da verilebilir (örn. `grpc_service = "helloworld.Greeter"`). Yalnızca `SERVING` durum kodu kontrolü geçer; `NOT_SERVING`, `UNKNOWN`, `SERVICE_UNKNOWN` veya sıfırdan farklı gRPC hata kodları sunucuyu sağlıksız olarak işaretler.
- `health_check.path` verilen HTTP proxy, her sunucunun kök adresinden bu yola `GET` gönderir. 2xx veya 3xx durum kodu kontrolü geçer. Yol `/` ile başlamalıdır ve yolu yalnız HTTP proxy kullanır.
- Yolu veya `grpc_service` ayarı olmayan HTTP proxy ve TCP proxy her sunucuya TCP bağlantısı açar.
- UDP proxy her sunucunun host adını çözümler.

Anlık gRPC sağlık kontrolü komutları:
- **CLI**: `r3 proxy check-grpc-health <PROXY_ID> [--service <SERVICE>]`
- **MCP Aracı**: `check_proxy_grpc_health` (parametreler: `proxy_id` veya `target_url`, `service`)
- **REST API**: `POST /api/proxies/{id}/grpc/health` veya `POST /api/grpc/health`

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
- İmza key'i `r3v3rs3.key` dosyasındaki ana şifreleme anahtarından türetilir. Sticky cookie'ler süreç yeniden başlatmalarında, kesintisiz yükseltmelerde ve worker yenilenmelerinde geçerliliğini korur.
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
2. `X-Forwarded-For` içinde güvenilen bir proxy'ye veya bilinen bir CDN edge'ine ait olmayan en sağdaki adres. Bu adresin solundaki girişleri istemci yazar. r3v3rs3 RFC 7239 `Forwarded` header'ını hiç okumaz. Çünkü bu header'ı yazmayan güvenilen bir proxy, istemcinin gönderdiği değeri olduğu gibi iletir ve istemci böylece istediği adresi yazabilir.

r3v3rs3 bulduğu adresi upstream sunucuya `X-Real-IP` header'ında gönderir. Gelen `X-Forwarded-For` zincirinin sonuna peer adresini ekler. `Forwarded` header'ını bu zincirden kendisi oluşturur ve gelen `Forwarded` header'ını siler. Çünkü bu header'ı yazmayan güvenilen bir proxy, istemcinin gönderdiği değeri olduğu gibi iletir. Peer güvenilir değilse r3v3rs3 `Forwarded`, `X-Forwarded-For`, `X-Real-IP`, `CF-Connecting-IP`, `True-Client-IP`, `CloudFront-Viewer-Address`, `Fastly-Client-IP` ve `Incap-Client-IP` header'larını siler, çünkü istemci bu header'lara sahte değer yazabilir.

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
- **İzin Verilen Ülkeler**: Virgülle ayrılmış ISO 3166-1 alpha-2 ülke kodları (ör. `TR`, `US`, `DE`). Güvenlik sayfasında (`/security`) veya `config.toml` içinde bir MaxMind GeoIP veritabanı tanımlanmış olmalıdır (`[geoip] database = "/path/to/GeoLite2-Country.mmdb"`).
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

### Otomatik GeoIP Veritabanı Güncelleme & MaxMind Lisans Yönetimi

r3v3rs3; MaxMind GeoLite2-Country veritabanları için SHA256 bütünlük doğrulaması, güvenli arşiv çıkarma ve lock-free bellek içi hot-reload (`ArcSwapOption`) özelliklerine sahip otonom, sıfır kesintili bir güncelleyici içerir.

- **MaxMind Hesap ve Lisans Entegrasyonu**: Ayarlar üzerinden veya API/CLI/MCP ile MaxMind Hesap ID'nizi ve Lisans Anahtarınızı yapılandırın. Secret'lar güvenli bir biçimde mühürlenir ve yanıtlarda maskelenir.
- **Yapılandırılabilir Arka Plan Zamanlaması**: `auto_update = true` ve `update_interval_days = 7` (1-90 gün) olarak belirleyin. Leader node, proxy performansını etkilemeden her 12 saatte bir arka planda güncelleme kontrolü gerçekleştirir.
- **Giden Webhook Bildirimleri**:
  - `system.geoip_updated`: Yeni bir veritabanı indirilip doğrulandığında ve belleğe hot-swap yapıldığında tetiklenir.
  - `system.geoip_update_failed`: İndirme, checksum doğrulaması veya arşiv açma başarısız olduğunda hata detaylarıyla tetiklenir.
- **WebUI, CLI ve MCP Arasında %100 Eşitlik**:
  - **WebUI**: Güvenlik sayfasında (`/security`) "GeoIP" sekmesinde yüklü durumunu, veritabanı dosya yolunu, boyutunu, derleme tarihini, lisans durumunu gösteren canlı durum kartı ve tek tıkla "Update Database Now" butonu.
  - **CLI**:
    - `r3 security geoip get-status`: Yüklü veritabanı durumunu, derleme zamanını, boyutunu ve otomatik güncelleme takvimini gösterir.
    - `r3 security geoip update-database`: En güncel GeoLite2-Country MMDB dosyasını indirir ve anında hot-reload eder.
    - `r3 security geoip update-config --license-key <KEY> --auto-update true --interval-days 7`: GeoIP ayarlarını günceller.
  - **MCP Tool'ları**: `get_geoip_status`, `update_geoip_database`, `update_geoip_config`.
  - **REST API**:
    - `GET /api/security/geoip/status`
    - `PUT /api/security/geoip/config`
    - `POST /api/security/geoip/update`

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
- Proxy isteğindeki `target_url`, o proxy'nin upstream sunucularından biri olmalıdır. Başka bir adres `403 forbidden` ile reddedilir. Bu yüzden bu endpoint'ler proxy dışındaki bir adrese ulaşamaz.
- `POST /api/grpc/reflect`: Herhangi bir hedef gRPC sunucusunda bağımsız servis keşfi yapar (`{"target_url": "http://127.0.0.1:50051"}`).
- `POST /api/grpc/invoke`: Hedef URL, servis, metot, JSON yükü ve metadata ile bağımsız unary çağrı yürütür.
- Bağımsız uçlar (`/api/grpc/reflect`, `/api/grpc/invoke`, `/api/grpc/health`) her adresi kabul eder. Bu yüzden bu uçları yalnız admin hesapları çağırabilir. Editörler proxy uçlarını (`/api/proxies/{id}/grpc/...`) kullanır. Bu uçlarda hedef, proxy'nin upstream sunucularından biri olmalıdır.

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

Route'u yönetim adresine giden bir r3v3rs3 proxy'si isteği TCP bağlantısı açmadan, process içinde panele verir. Panel istemci adresini, HTTPS durumunu ve `Host` değerini proxy'nin çözdüğü hâliyle alır. Proxy'nin [istemci IP adresi](#istemci-ip-adresi) ayarları da buna dahildir ve hiçbir istemci bu değerleri sahte gönderemez. Bu proxy için aşağıdaki ayar gerekmez.

`[admin]` bölümündeki `trusted_proxies` ayarı (**Ayarlar** sayfasında **Güvenilen Proxy'ler**) panelin önündeki diğer reverse proxy'lerin adreslerini veya CIDR bloklarını listeler. Örnek: yönetim adresine TCP ile bağlanan bir nginx veya bir tünel. Panel, audit log'un, rate limit'lerin ve MCP token ban'ının istemci adresini `X-Forwarded-For` içinde bu adreslere ait olmayan en sağdaki girişten alır (soldaki girişleri istemci yazar). `X-Real-IP` gibi başka bir header okunmaz. Oturum cookie'sinin HTTPS durumunu `X-Forwarded-Proto` header'ından alır. Panel bu değerleri yalnız istek bu adreslerden birinden gelirse okur. Aksi hâlde bağlantının adresini kaydeder. Bir loopback adresine kendiliğinden güvenilmez, çünkü host'taki her process oradan bağlanabilir. Worker modu (`worker_mode = "workers"`) port'ları ayrı bir process'te çalıştırır, bu yüzden oradaki proxy panele TCP ile bağlanır. Bu modda `127.0.0.1/32` girin. CDN adres blokları listesi her proxy'nin ayrı bir ayarıdır ve panele uygulanmaz.

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
- Kullanıcılar `/.r3v3rs3/auth/oidc/logout` adresine `POST` isteği göndererek diledikleri zaman oturumlarını kapatabilir (cross-site isteklere karşı korunur).

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

Her hesap listeleri okur. Listeleri yalnız admin veya proxy listesi olmayan editör değiştirir. Ayrıntılar için [Hesaplar](@/reference/accounts.tr.md) sayfasına bakın.

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

### Host header'ı

r3v3rs3 varsayılan olarak `Host` header'ında upstream sunucunun adresini gönderir, örneğin `127.0.0.1:8080`. Bunun yerine istemcinin istediği host adını göndermek için "İstemcinin Host Header'ını Koru" (`preserve_host = true`) seçeneğini açın. `Origin` header'ını `Host` header'ı ile karşılaştıran bir uygulama bu ayara ihtiyaç duyar, örneğin bir CSRF kontrolünde. HTTP/2 kullanan bir upstream sunucu (h2 seçen bir HTTPS sunucusu veya h2c ile düz bir HTTP sunucusu) istemcinin host adını isteğin `:authority` değeri olarak alır, çünkü HTTP/2'de ayrı bir `Host` header'ı yoktur. Bağlantı ve TLS sertifika kontrolü yine upstream sunucunun adresini kullanır. Deploy platformu uygulamalarının proxy'leri istemcinin host adını her zaman korur. Keşfedilen bir proxy bu ayarı `r3v3rs3.http.<name>.preserve_host=true` label'ı ile açar.

```toml
[my-app]
protocol = "http"
vhosts = ["app.example.com"]
preserve_host = true
routes = [{ path = "/", servers = [{ url = "http://127.0.0.1:9000/" }] }]
```

```toml
[my-app]
protocol = "http"
vhosts = ["app.example.com"]
headers = { request = [{ action = "set", name = "X-Request-Id", value = "{request_id}" }, { action = "remove", name = "X-Debug" }], response = [{ action = "set", name = "X-Frame-Options", value = "DENY" }, { action = "remove", name = "Server" }] }
routes = [{ path = "/", servers = [{ url = "http://127.0.0.1:9000/" }] }]
```

### W3C Trace Context ve İstek İlişkilendirme Kimlikleri

r3v3rs3, W3C Trace Context standardına uygun olarak dağıtık izleme (distributed tracing) yayılımını destekler:

- **W3C Traceparent Çıkarımı ve İletimi**: Gelen istek geçerli bir `traceparent` başlığı taşıdığında (biçim: `00-{trace_id}-{parent_id}-{flags}`), r3v3rs3 trace ID'sini çıkarır ve upstream servise iletmeden önce yeni bir child span ID üretir.
- **Otomatik Üretim**: `traceparent` bulunmadığında veya geçersiz olduğunda r3v3rs3 otomatik olarak yeni ve geçerli bir W3C traceparent başlığı (`00-{trace_id}-{span_id}-01`) üretir.
- **Tracestate İletimi**: Gelen `tracestate` başlıkları değiştirilmeden korunur ve upstream servise iletilir.
- **İstek ID Senkronizasyonu**: İlişkilendirme kimliği (`{request_id}`), gelen `X-Request-Id` değeriyle veya 32 karakterlik hex trace ID ile eşleşir. Gelen istekte `X-Request-Id` yoksa r3v3rs3 upstream isteğine trace ID değerini içeren `X-Request-Id` başlığını ekler.
- **Hata Yanıtlarında İlişkilendirme**: r3v3rs3 tarafından üretilen tüm hata yanıtları (örneğin 502 Bad Gateway, 504 Gateway Timeout veya 404 Not Found) `X-Request-Id` HTTP yanıt başlığını ekler ve hata ayıklama için varsayılan ve özel hata sayfalarında istek kimliğini gösterir.

### Güvenlik Üstbilgisi Sertleştirme Profilleri

r3v3rs3, WebUI, MCP ve CLI üzerinden tek tıkla uygulanabilen yerleşik OWASP güvenlik üstbilgisi profilleri ve otomatik bir karne denetim motoru (A+'dan F'ye) sunar:

- **Katı (`strict`)**: Yüksek güvenlik gerektiren uygulamalar için derinlemesine savunma profili (`max-age=63072000; includeSubDomains; preload`, `X-Content-Type-Options: nosniff`, `X-Frame-Options: DENY`, `Referrer-Policy: no-referrer`, kısıtlayıcı `Permissions-Policy`, katı `Content-Security-Policy` ve yalıtım üstbilgileri `Cross-Origin-Opener-Policy: same-origin`, `Cross-Origin-Resource-Policy: same-origin`).
- **Dengeli (`balanced`)**: Tipik CMS ve çok sayfalı web uygulamaları için önerilen standart profil (`max-age=31536000; includeSubDomains`, `X-Frame-Options: SAMEORIGIN`, `Referrer-Policy: strict-origin-when-cross-origin`, inline stilleri ve yaygın CDN'leri destekleyen dengeli CSP).
- **API Uç Noktası (`api_only`)**: Saf REST, JSON ve gRPC endpoint'leri için optimize edilmiş profil (`X-Frame-Options: DENY`, `Cache-Control: no-store, max-age=0`, kısıtlayıcı CSP, `Referrer-Policy: no-referrer`).
- **SPA Ön Uç (`spa_frontend`)**: Vite, React, Vue, Svelte ve modern Tek Sayfa Uygulamaları için worker, blob ve WebSocket destekli optimize edilmiş profil.

#### CLI Komutları

```bash
# Mevcut güvenlik profillerini listele
r3 proxy security-headers list-profiles

# Proxy güvenlik üstbilgilerini denetle, skoru ve önerileri incele
r3 proxy security-headers get <PROXY_ID>

# Bir proxy'ye OWASP güvenlik sertleştirme profili uygula
r3 proxy security-headers apply-profile <PROXY_ID> strict
```

#### MCP Araçları

Yapay zeka ajanları `list_security_header_profiles`, `get_proxy_security_headers` ve `apply_proxy_security_header_profile` araçlarını kullanarak güvenlik profillerini inceleyebilir ve uygulayabilir.

## Sıkıştırma

Proxy'den geçen yanıtları "Sıkıştırma" bölümünden sıkıştırabilirsiniz. Bir veya daha fazla encoding seçtiğinizde sıkıştırma açılır. Hiçbir encoding seçili değilse sıkıştırma kapalıdır.

| Encoding             | `Content-Encoding` | Varsayılan Seviye | Uyarlanabilir Seviyeler (< 8 KB / 8–64 KB / > 64 KB veya Stream) |
|----------------------|--------------------|-------------------|------------------------------------------------------------------|
| Brotli               | `br`               | Kalite 4          | Kalite 5 / Kalite 4 / Kalite 3                                   |
| Zstandard            | `zstd`             | Seviye 3          | Seviye 4 / Seviye 3 / Seviye 1 (En Hızlı)                        |
| Gzip                 | `gzip`             | Seviye 6          | Seviye 9 (En İyi) / Seviye 6 / Seviye 1 (En Hızlı)               |
| Zstandard Dictionary | `dcz`              | Seviye 3          | Seviye 4 / Seviye 3 / Seviye 1 (En Hızlı)                        |
| Brotli Dictionary    | `dcb`              | Kalite 4          | Kalite 5 / Kalite 4 / Kalite 3                                   |

r3v3rs3 isteğin `Accept-Encoding` header'ını okur ve istemcinin kabul ettiği encoding'lerden `q` değeri en yüksek olanı seçer. Birden fazla encoding aynı `q` değerine sahipse `algorithms` listesindeki sıra geçerli olur. Panelde bu sıra, encoding'leri seçtiğiniz sıradır.

`adaptive_level` etkinleştirildiğinde (varsayılan `true`), r3v3rs3 yanıt boyutuna göre sıkıştırma kalitesini otomatik olarak optimize eder: küçük yanıtlar (< 8 KB) CPU maliyetinin önemsiz olduğu durumlarda en yüksek sıkıştırma oranını kullanır, büyük veya akışlı (stream) yanıtlar ise worker thread'lerin doymasını engellemek için maksimum verim (throughput) seviyelerini tercih eder. Ayrıca metin, JSON ve SVG içerikleri için minimum boyut eşiği dinamik olarak 256 byte'a indirilir.

### Paylaşılan Sıkıştırma Sözlükleri (RFC 9842)

r3v3rs3, Chromium ve uyumlu modern HTTP istemcileri için Sıkıştırma Sözlüğü Aktarımı (RFC 9842) standardını destekler:
- **Sözlük Sunumu**: HTTP proxy'sinde yapılandırılan sözlükler, belirlenen `path` yolunda r3v3rs3 tarafından doğrudan `Content-Type: application/octet-stream`, `Use-As-Dictionary: match="<match_pattern>", id="<id>"`, `Cache-Control: public, max-age=31536000, immutable` ve `ETag: "<sha256>"` başlıklarıyla sunulur.
- **İçerik Müzakeresi ve Kodlama**: Gelen istekte yapılandırılmış bir sözlüğün SHA-256 hash'i ve yoluyla eşleşen `Available-Dictionary` (veya `Sec-Available-Dictionary`) başlığı bulunduğunda ve istemci `dcz` ya da `dcb` encoding'ini kabul ettiğinde, r3v3rs3 yanıtı sözlükle sıkıştırır. Yanıta `Content-Encoding: dcz` (veya `dcb`) ve `Vary: Available-Dictionary, Accept-Encoding` eklenir. Gövde, encoding'in sabit başlığı ve sözlüğün SHA-256 özetiyle başlar. İstemci bu özetle doğru sözlüğü kullandığını denetler. RFC 9842 sözlükle sıkıştırmaya yalnız güvenli bağlantıda izin verir. Bu yüzden düz HTTP üzerinden gelen istekler standart algoritmalarla sıkıştırılır.
- **Bant Genişliği Tasarrufu**: Statik JSON şemalarını, yinelenen nesne anahtarlarını ve tekrarlayan HTML kalıplarını ortadan kaldırarak yinelenen API trafiğinde %80 ile %95 arasında bant genişliği tasarrufu sağlar.
- **Yönetim**: Sıkıştırma sözlükleri WebUI Proxy Yapılandırması sayfasından, API'den (`/api/proxies/{id}/dictionaries` ve `/api/proxies/{id}/dictionaries/{dict_id}`), MCP araçlarından (`list_proxy_compression_dictionaries`, `get_proxy_compression_dictionary`, `create_proxy_compression_dictionary`, `delete_proxy_compression_dictionary`) veya CLI üzerinden (`r3 proxy dictionary list/get/create/delete`) yönetilebilir. Yeni sözlüğün id'sini ve hash değerini sunucu belirler. TCP ve UDP proxy'lerinde sunucu sözlüğü reddeder.

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
- Gövde eksiksiz gelir: gövde ya `Content-Length` kadar bayttır ya da chunked gövdenin son parçasıyla biter. Upstream sunucunun yarıda kestiği bir gövde saklanmaz.

Geçerlilik süresi için sırasıyla `s-maxage`, `max-age`, `Expires` ve `default_ttl` değerlerine bakılır. `Cache-Control: no-cache` bu süreyi sıfır yapar. Saklanan yanıtın yaşına upstream yanıtındaki `Age` header'ının değeri de eklenir.

Saklanan yanıtın süresi dolmuşsa ve `stale_while_revalidate` (veya upstream `stale-while-revalidate` header direktifi) süresi henüz aşılmamışsa, r3v3rs3 istemciye hemen bayat yanıtı sunar (`X-Cache: STALE`) ve arka planda cache'i tazelemek üzere bloklamayan asenkron bir yeniden doğrulama başlatır. Birden fazla eşzamanlı isteğin aynı anda upstream'e yüklenmesini (dogpiling) engellemek için arka plan doğrulamaları her cache key başına kilitlenir.

Saklanan yanıtın süresi dolmuşsa ve `stale_while_revalidate` penceresinin dışındaysa, yanıtta `ETag` veya `Last-Modified` header'ı varsa r3v3rs3 `If-None-Match` veya `If-Modified-Since` ile koşullu bir istek gönderir. Upstream sunucu `304 Not Modified` dönerse r3v3rs3 saklanan header'ları günceller ve cache'teki yanıtı gönderir. Validator'ı olan yanıt, süresi dolduktan sonra da bir saat cache'te kalır. Eşleşen `If-None-Match` veya `If-Modified-Since` header'ı gönderen istemci, cache'ten `304 Not Modified` alır.

Her cache key için tek bir yanıt saklanır. Yanıtın `Vary` header'ı istek header'larını listeliyorsa saklanan yanıt yalnız bu header'larda aynı değerleri gönderen isteklere verilir.

r3v3rs3, cache'i kullanabilecek isteklerdeki `Accept-Encoding` header'ını siler. Böylece upstream sunucu yanıtları encoding uygulamadan gönderir ve "Sıkıştırma" ayarları yanıtı her istemci için ayrıca sıkıştırır. Bu isteklere verilen yanıtlarda `X-Cache` header'ı bulunur: cache'ten gelen yanıt için `HIT`, upstream sunucudan gelen yanıt için `MISS`, arka plan doğrulaması sırasında sunulan yanıt için `STALE`. Cache'ten gelen yanıtta ayrıca `Age` header'ı vardır.

Cache'teki yanıtları temizlemek için:
- `DELETE /api/proxies/{id}/cache` (veya belirli bir yol ön ekiyle eşleşen kayıtları silmek için `DELETE /api/proxies/{id}/cache?path=/api/v1/`).
- `POST /api/proxies/{id}/cache/purge` isteği (isteğe bağlı JSON gövdesi `{"path": "/on-ek"}`).
- Doğrudan proxy adresine HTTP `PURGE` metodu ile istek (ör. `curl -X PURGE https://app.example.com/data.json`). Proxy bu isteği yalnız iki durumda kabul eder: route'ta bir kimlik doğrulama vardır ve istek bunu geçmiştir ya da istemci adresi bir loopback adresidir. Güvenilen bir proxy veya CDN üzerinden gelen istek güvenilir sayılmaz, çünkü güvenilen proxy internetteki her istemcinin isteğini iletir. Diğer istemciler `403 Forbidden` alır.
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

TCP / TLS üzerinden TCP proxy'si upstream sunucusuna yeni bir bağlantı açar. Bu yüzden PROXY protocol header'ı olmadan upstream sunucusu istemci olarak r3v3rs3'ün adresini görür: aynı sunucudaki bir platform uygulaması için bu adres `127.0.0.1` olur. Gerçek istemci adresine ihtiyaç duyan bir sunucu PROXY protocol okumalıdır. Örneğin r3v3rs3'ün bir TLS portunun arkasındaki TURN sunucusu böyledir.

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
