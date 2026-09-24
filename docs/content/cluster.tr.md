+++
title = "Cluster"
description = "etcd veya Consul ile yüksek erişilebilirlik"
weight = 0
+++

# Cluster

Birden fazla r3v3rs3 düğümü etcd'de veya Consul'un key-value store'unda tek bir durum bilgisini paylaşabilir. Her düğüm aynı portlar, proxy'ler, erişim listeleri, sertifikalar, ACME kayıtları, admin hesapları ve ayarlarla istekleri karşılar. Bir düğümdeki değişiklik yeniden başlatma olmadan diğer düğümlere ulaşır.

Bu sayfa referanstır. Sıfırdan kuruyorsanız [Yüksek erişilebilirlik](@/tutorials/high-availability.tr.md) sayfasını izleyin: veri deposunun kurulumundan yük dengeleyiciye kadar bütün adımları anlatır.

## Mimari

- `[cluster]` bölümü her düğümün `config.toml` dosyasında durur. Yönetim API'si ve WebUI bu bölümü değiştirmez. Veri deposu bu bölümü tutmaz.
- Durum bilgisinin geri kalanı veri deposunda durur: ayarlar, portlar, proxy'ler, erişim listeleri, sertifikalar, ACME kayıtları, admin hesapları ve CDN IP aralıkları. Denetim kaydı ve gönderilen sertifika bildirimleri de veri deposunda durur. Cluster açık olan düğüm `ports.toml`, `proxies.toml`, `access_lists.toml`, `acme.toml`, `accounts.toml`, `notifications.json` ve sertifika dosyalarını okumaz.
- Her düğüm veri deposunu izler ve her değişikliği uygular. Bir düğümün yönetim API'sinden gelen değişiklik önce veri deposuna yazılır. Daha yeni bir değer bulan yazma `409 cluster_write_conflict` ile başarısız olur ve düğüm yeni değeri veri deposundan alır.
- Düğümler admin oturumlarını, proxy oturumlarını, rate limit sayılarını ve `share_cache` açıksa cache'lenen yanıtları paylaşır.
- Düğümlerden biri liderdir. ACME sertifikalarını yalnız lider sipariş eder. Sertifika bildirimlerini yalnız lider gönderir. Eski denetim kayıtlarını, süresi dolan sertifikaları, süresi dolan oturumları ve paylaşılan yanıtları yalnız lider siler. İndirilen CDN IP aralıklarını veri deposuna yalnız lider yazar. Lider bu işleri lider olduğu anda ve sonra her `background_task_interval` sürede çalıştırır.
- Düğümlerin önündeki bir yük dengeleyici her isteği herhangi bir düğüme gönderebilir.

WebUI'daki **Ayarlar** sayfası düğümün durumunu, rolünü ve uyguladığı revizyonu gösterir. `GET /api/cluster/status` aynı veriyi döner.

| Durum | Anlamı |
|---|---|
| `disabled` | Düğüm cluster veri deposu kullanmıyor. |
| `syncing` | Düğüm veri deposunu ilk kez okuyor. |
| `synced` | Düğüm veri deposundaki değişiklikleri uyguluyor. |
| `degraded` | Düğüm veri deposuyla bağlantısını kaybetti. Son uygulanan durumla istekleri karşılar ve değişiklikleri reddeder. |

## Cluster kurulumu

Bir cluster'ı dört komut kurar: `r3v3rs3 cluster keygen` şifreleme key'ini yazar, `config.toml` dosyasındaki `[cluster]` bölümü her düğümde veri deposunu tanımlar, `r3v3rs3 cluster import` bir düğümün dosyalarını boş bir öneke kopyalar ve `r3v3rs3 start` her düğümü çalıştırır.

[Yüksek erişilebilirlik](@/tutorials/high-availability.tr.md) sayfası her adımı komutlarıyla, veri deposu kurulumuyla, kimlik bilgileriyle ve yük dengeleyici ile birlikte anlatır.

## Ayarlar

Bu alanları yalnız `config.toml` belirler.

| Alan | Varsayılan | Açıklama |
|---|---|---|
| `enabled` | `false` | Cluster'ı açar. |
| `backend` | `etcd` | `etcd` veya `consul`. |
| `endpoints` | yok | Veri deposunun HTTP API adresleri: `http://<host>:<port>`, `https://<host>:<port>` veya `unix://<path>`. Bağlantı hatası bir sonraki adresi seçer. |
| `username`, `password` | boş | etcd kullanıcısı. Kullanıcı boşsa kimlik bilgisi gönderilmez. |
| `token` | yok | Consul ACL token'ı. |
| `datacenter` | boş | Consul veri merkezi. Boşsa agent'ın veri merkezi kullanılır. |
| `prefix` | `r3v3rs3` | Cluster'ın her key'i `<prefix>/v1/` ile başlar. Farklı öneklerle birden fazla cluster aynı veri deposunu paylaşabilir. |
| `node_name` | yok | Düğümün adı. Zorunludur ve her düğümün adı farklı olmalıdır. |
| `tls` | yok | `ca_file` veri deposunu doğrular. Bu alan yoksa sistemin kök sertifikaları veri deposunu doğrular. `cert_file` ve `key_file` istemci sertifikası gönderir. |
| `encryption_key_files` | yok | Key dosyaları. İlk key şifreler. Her key çözer. |
| `lock_ttl` | `15s` | Lider kilidinin ve düğüm varlık kaydının TTL'i. etcd en az `1s`, Consul en az `10s` ister. |
| `startup_timeout` | `30s` | Düğüm başlarken veri deposu için en uzun bekleme süresi. |
| `rate_limit_sync_interval` | `1s` | Düğümün rate limit sayılarını yayınlama sıklığı. En düşük değer `100ms`'dir. |
| `share_cache` | `false` | Cache'lenen yanıtları diğer düğümler için veri deposuna yazar. |
| `cache_max_value_size` | `1048576` | Düğümün diğer düğümler için veri deposuna yazdığı en büyük cache'lenmiş yanıtın byte cinsinden boyutu. |

Yönetim API'si `password` ve `token` değerlerini döndürmez.

## Veri deposundaki key'ler

Her key `<prefix>/v1/` ile başlar.

| Key | İçerik | Şifreli |
|---|---|---|
| `schema` | Veri düzeninin sürümü. İçe aktarma bu key'i en son yazar. | hayır |
| `state/config` | Ayarlar. | evet |
| `state/ports/<id>` | Portlar. | hayır |
| `state/proxies/<id>` | Proxy'ler. | evet |
| `state/access-lists/<id>` | Parola hash'leri ve token özetleriyle erişim listeleri. | evet |
| `state/certs/<kind>/<id>` | Sertifikalar ve private key'leri. | evet |
| `state/acme/<id>` | Hesap key'leri ve DNS sağlayıcısı kimlik bilgileriyle ACME kayıtları. | evet |
| `state/accounts/<hex ad>` | Admin hesapları. | evet |
| `state/cdn` | CDN IP aralıkları. | hayır |
| `state/challenges/http/<hex token>`, `state/challenges/tls-alpn/<hex domain>` | Her düğümün sunduğu ACME challenge'ları. ACME sunucusu bu değerleri zaten yayınlar. | hayır |
| `state/cache-purges/<proxy id>` | Bir proxy'nin son cache temizleme zamanı. | hayır |
| `lock/leader` | Liderin lease'ine bağlı lider kilidi. | evet |
| `nodes/<hex ad>` | Bir düğümün lease'ine bağlı varlık kaydı. | hayır |
| `acks/<hex ad>` | Bir düğümün sunduğu challenge'ların özeti. | hayır |
| `sessions/<scope>/<token'ın SHA-256'sı>` | Admin ve proxy oturumları. Veri deposu hiçbir zaman token'ın kendisini tutmaz. | evet |
| `ratelimit/<hex ad>` | Bir düğümün istemci IP adresleriyle rate limit sayıları. | evet |
| `cache/<proxy id>/<cache key'in SHA-256'sı>` | Paylaşılan bir cache'lenmiş yanıt. | evet |
| `audit/<YYYY-MM-DD>/<Unix ms>-<rastgele>` | Gününün key'i altındaki bir denetim kaydı girdisi. | evet |
| `notify` | Webhook'un aldığı sertifika olayları. Bu key'i yalnız lider yazar. | evet |

## Şifreleme

Düğüm her değeri yazmadan önce AES-256-GCM ile şifreler. Değerin KV key'i associated data olarak kullanılır. Bu yüzden başka bir key'e taşınan değer çözülmez. Şifreli değer, key'inin id'si ile başlar: key'in SHA-256 özetinin ilk 8 byte'ı.

Bir key'i değiştirmek için:

1. `r3v3rs3 cluster keygen` ile yeni bir key dosyası oluşturun.
2. Yeni dosyayı `encryption_key_files` listesinin başına koyun ve eski dosyayı listede bırakın. Bunu her düğümde yapın ve düğümleri yeniden başlatın. Düğüm key dosyalarını başlarken okur.
3. Bir düğümde `r3v3rs3 cluster rekey --config-dir <dir>` komutunu çalıştırın. Komut her değeri ilk key ile yeniden şifreler ve değiştirdiği değerleri raporlar.
4. Eski dosyayı her düğümde `encryption_key_files` listesinden çıkarın ve düğümleri yeniden başlatın.

Veri deposunu da koruyun. Şifreleme değerleri gizler, ama key adları portların, proxy'lerin ve sertifikaların id'lerini gösterir.

## etcd izinleri

Düğümlerin kullanıcısı önekin altındaki key'leri okuyup yazabilmelidir:

```bash
$ etcdctl role add r3v3rs3
$ etcdctl role grant-permission --prefix=true r3v3rs3 readwrite r3v3rs3/
$ etcdctl user add r3v3rs3
$ etcdctl user grant-role r3v3rs3 r3v3rs3
```

Başka bir `prefix` kullanıyorsanız izni `<prefix>/` için verin. etcd kimlik doğrulama token'ını iptal ederse düğüm yeni bir token alır.

## Consul izinleri

Düğümlerin token'ı önekin altındaki key'lere yazabilmeli ve oturum oluşturabilmelidir:

```hcl
key_prefix "r3v3rs3/" {
  policy = "write"
}

session_prefix "" {
  policy = "write"
}
```

```bash
$ consul acl policy create -name r3v3rs3 -rules @r3v3rs3.hcl
$ consul acl token create -description "r3v3rs3 nodes" -policy-name r3v3rs3
```

`make test-cluster-e2e` hedefi düğümleri tam olarak bu izinlerle çalıştırır.

## Lider ve ACME

Lider, `lock_ttl` süreli bir lease ile `lock/leader` key'ini tutar. Lease'i her `lock_ttl` süresinde üç kez yeniler. Liderin bir veri deposu çağrısı en fazla `lock_ttl` süresinin üçte biri kadar bekler. Bu yüzden lider, lease'i veri deposunda bitmeden liderliği bırakır. Lider dururken kilidi bırakır ve başka bir düğüm bir sonraki kontrolünde lider olur.

HTTP-01 veya TLS-ALPN-01 challenge'ında lider challenge'ı veri deposuna yazar. Her düğüm challenge'ı sunar. Her düğüm, sunduğu challenge'ların özetini `acks/` altına yazar. Lider, var olan her düğüm yeni challenge'ları bildirene kadar en fazla 10 saniye bekler. Sonra ACME sunucusundan doğrulama ister.

## Hata durumları

- Düğüm veri deposunu her `lock_ttl` süresinde üç kez kontrol eder. Düğüm `lock_ttl` süresince başarılı bir okuma yapamazsa durumu `degraded` olur.
- `degraded` durumundaki düğüm son uygulanan durumla istekleri karşılamaya devam eder. Yönetim API'si değişiklikleri `503 cluster_unavailable` ile reddeder. WebUI'ın her sayfası bir uyarı gösterir.
- Veri deposu çağrıları başarısız olan lider, liderliği bırakır. Bu yüzden `degraded` durumundaki düğüm lider olmaz.
- Yeni bir giriş ve yeni bir proxy oturumu veri deposuna ihtiyaç duyar. Veri deposu yoksa `503` ile başarısız olur.
- Veri deposu geri gelince düğüm bütün durum bilgisini yeniden okur ve durumu `synced` olur. Düğüm yerel durum bilgisini veri deposuna yazmaz.
- Başlarken veri deposuna ulaşamayan düğüm `startup_timeout` sonunda durur.

## Oturumlar

Admin oturumları ve proxy kimlik doğrulama oturumları veri deposunda durur. Bu yüzden bir düğümün başlattığı oturum her düğümde geçerlidir. Çıkış yapmak oturumu her düğüm için siler. Düğüm, oturum taşıyan her istek için veri deposunu okur. Veri deposu yoksa proxy oturumu geçersiz sayılır ve yönetim API'si `503 cluster_unavailable` döner.

## Rate limit doğruluğu

Düğüm her istek için veri deposunu okumaz. Her düğüm her istemcinin isteklerini sayar. Her `rate_limit_sync_interval` sürede her limit için en yoğun 2048 istemcinin sayılarını yayınlar. Düğüm, diğer düğümlerin yeni sayılarını kendi sayılarına ekler. Üç aralıktan eski sayılar sayılmaz. Üç aralık 2 saniyeden kısaysa sınır 2 saniyedir.

- Periyodu `rate_limit_sync_interval` değerinin en az 10 katı olan limit, bütün düğümlerin sayılarını kullanır. Diğer düğümlerin sayıları en fazla bir aralık eskidir. Her düğüme aynı anda ulaşan bir burst, limitin `(N − 1) × burst` kadar üstüne çıkabilir. `N` düğüm sayısıdır.
- Periyodu daha kısa olan limit, limiti var olan düğümler arasında böler. Her düğüm `ceil(limit / N)` isteğe izin verir. Bir istemciyi tek düğüme gönderen yük dengeleyici, o istemciye limitten daha az hak verir.
- Veri deposu yoksa her düğüm, bilinen son düğüm sayısıyla bölünmüş limiti kullanır.

## Paylaşılan cache

`share_cache = true` iken düğüm, en az 60 saniye taze kalan her cache'lenmiş yanıtı veri deposuna yazmak için kuyruğa koyar. Düğüm yazmayı beklemez. Yanıt yerel cache'te yoksa düğüm veri deposunu en fazla 100 milisaniye okur.

- Kodlanmış boyutu `cache_max_value_size`, etcd için 1 MiB veya Consul için 350 KiB değerlerinden küçük olanını aşan yanıt, kendi düğümünde kalır. Gövde base64 olarak saklanır, bu yüzden saklanan boyut gövde boyutunun yaklaşık 4/3'üdür. Düğüm bir yanıtı birden fazla key'e bölmez.
- Bir proxy'nin cache temizleme işlemi `state/cache-purges/<proxy id>` key'ini yazar ve proxy'nin paylaşılan yanıtlarını siler. Her düğüm değişiklikten sonra kendi yerel cache'ini temizler.
- Paylaşılan yanıt bayatladıktan sonra bir saat daha veri deposunda kalır, böylece bir düğüm onu yeniden doğrulayabilir. Lider süresi dolan yanıtları her `background_task_interval` sürede siler. Silme işlemi paylaşılan her yanıtı okur ve çözer. Bu yüzden maliyeti saklanan yanıt sayısıyla büyür.
- Saklanan her yanıt veri deposuna bir yazmadır. Veri deposu her yazmayı compaction'a kadar geçmişinde tutar. Bu yüzden yoğun bir cache, compaction'lar arasında etcd'nin boyutunu büyütür.

## Saatler

Düğümler oturum süresi, rate limit pencereleri, rate limit sayılarının yeniliği ve paylaşılan yanıtların süresi için Unix zamanlarını karşılaştırır. Düğümlerin saatlerini NTP ile senkron tutun. Saniyeler düzeyinde ileride veya geride olan bir saat rate limit kararlarını ve cache sürelerini değiştirir.
