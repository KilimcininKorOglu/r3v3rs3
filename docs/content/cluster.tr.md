+++
title = "Cluster"
description = "etcd veya Consul ile yüksek erişilebilirlik"
weight = 0
+++

# Cluster

Birden fazla r3v3rs3 node'u etcd'de veya Consul'un key-value store'unda tek bir state paylaşabilir. Her node aynı portlar, proxy'ler, access list'ler, sertifikalar, ACME kayıtları, admin hesapları ve ayarlarla trafik alır. Bir node'daki değişiklik restart olmadan diğer node'lara ulaşır.

## Mimari

- `[cluster]` bölümü her node'un `config.toml` dosyasında durur. Admin API ve WebUI bu bölümü değiştirmez. Store bu bölümü tutmaz.
- State'in geri kalanı store'da durur: ayarlar, portlar, proxy'ler, access list'ler, sertifikalar, ACME kayıtları, admin hesapları ve CDN IP aralıkları. Audit log ve gönderilen sertifika bildirimleri de store'da durur. Cluster açık olan node `ports.toml`, `proxies.toml`, `access_lists.toml`, `acme.toml`, `accounts.toml`, `notifications.json` ve sertifika dosyalarını okumaz.
- Her node store'u izler ve her değişikliği uygular. Bir node'un admin API'sinden gelen değişiklik önce store'a yazılır. Daha yeni bir değer bulan yazma `409 cluster_write_conflict` ile başarısız olur ve node yeni değeri store'dan alır.
- Node'lar admin session'larını, proxy session'larını, rate limit sayılarını ve `share_cache` açıksa cache'lenen response'ları paylaşır.
- Node'lardan biri leader'dır. ACME sertifikalarını yalnız leader order eder. Sertifika bildirimlerini yalnız leader gönderir. Eski audit log kayıtlarını, süresi dolan sertifikaları, süresi dolan session'ları ve paylaşılan response'ları yalnız leader siler. İndirilen CDN IP aralıklarını store'a yalnız leader yazar. Leader bu işleri leader olduğu anda ve sonra her `background_task_interval` sürede çalıştırır.
- Node'ların önündeki bir load balancer her request'i herhangi bir node'a gönderebilir.

WebUI'daki **Ayarlar** sayfası node'un durumunu, rolünü ve uyguladığı revision'ı gösterir. `GET /api/cluster/status` aynı veriyi döner.

| Durum | Anlamı |
|---|---|
| `disabled` | Node cluster store kullanmıyor. |
| `syncing` | Node store'u ilk kez okuyor. |
| `synced` | Node store'daki değişiklikleri uyguluyor. |
| `degraded` | Node store bağlantısını kaybetti. Son uygulanan state ile trafik alır ve değişiklikleri reddeder. |

## Cluster kurulumu

1. Bir encryption key dosyası oluşturun. Aynı dosyayı her node'a kopyalayın.

   ```bash
   $ r3v3rs3 cluster keygen /etc/r3v3rs3/cluster.key
   ```

   Komut `0600` izinli yeni bir dosya yazar. Var olan bir dosyanın üstüne yazmaz. Key dosyasının bir kopyasını güvenli bir yerde tutun. Key'in bütün kopyaları kaybolursa store'daki veri kimse tarafından çözülemez.

2. Her node'da `config.toml` dosyasına `[cluster]` bölümünü ekleyin. Node'lar arasında yalnız `node_name` farklıdır.

   ```toml
   [cluster]
   enabled = true
   backend = "etcd"
   endpoints = ["https://10.0.0.1:2379", "https://10.0.0.2:2379", "https://10.0.0.3:2379"]
   username = "r3v3rs3"
   password = "<etcd parolası>"
   node_name = "proxy-1"
   encryption_key_files = ["/etc/r3v3rs3/cluster.key"]
   tls = { ca_file = "/etc/r3v3rs3/etcd-ca.pem" }
   ```

3. Bir node'un dosyalarını store'a kopyalayın. Store'daki prefix boş olmalıdır.

   ```bash
   $ r3v3rs3 cluster import --config-dir /etc/r3v3rs3
   ```

   Import `schema` key'ini en son yazar. Bir import yarıda kalırsa prefix'in altındaki key'leri silin ve import'u tekrar çalıştırın.

4. Her node'u `r3v3rs3 start` ile başlatın. Store'da `schema` key'i yoksa node başlamaz ve `r3v3rs3 cluster import` çalıştırmanızı ister. Import'tan sonra `r3v3rs3 add-user` hesabı store'a yazar.

## Ayarlar

Bu alanları yalnız `config.toml` belirler.

| Alan | Varsayılan | Açıklama |
|---|---|---|
| `enabled` | `false` | Cluster'ı açar. |
| `backend` | `etcd` | `etcd` veya `consul`. |
| `endpoints` | yok | Store'un HTTP API adresleri: `http://<host>:<port>`, `https://<host>:<port>` veya `unix://<path>`. Bağlantı hatası bir sonraki adresi seçer. |
| `username`, `password` | boş | etcd kullanıcısı. Kullanıcı boşsa credential gönderilmez. |
| `token` | yok | Consul ACL token'ı. |
| `datacenter` | boş | Consul datacenter'ı. Boşsa agent'ın datacenter'ı kullanılır. |
| `prefix` | `r3v3rs3` | Cluster'ın her key'i `<prefix>/v1/` ile başlar. Farklı prefix'lerle birden fazla cluster aynı store'u paylaşabilir. |
| `node_name` | yok | Node'un adı. Zorunludur ve her node'un adı farklı olmalıdır. |
| `tls` | yok | `ca_file` store'u doğrular. Bu alan yoksa sistemin root sertifikaları store'u doğrular. `cert_file` ve `key_file` client sertifikası gönderir. |
| `encryption_key_files` | yok | Key dosyaları. İlk key şifreler. Her key çözer. |
| `lock_ttl` | `15s` | Leader lock'unun ve node varlık kaydının TTL'i. etcd en az `1s`, Consul en az `10s` ister. |
| `startup_timeout` | `30s` | Node başlarken store için en uzun bekleme süresi. |
| `rate_limit_sync_interval` | `1s` | Node'un rate limit sayılarını yayınlama sıklığı. En düşük değer `100ms`'dir. |
| `share_cache` | `false` | Cache'lenen response'ları diğer node'lar için store'a yazar. |
| `cache_max_value_size` | `1048576` | Node'un diğer node'lar için store'a yazdığı en büyük cache'lenmiş response'un byte cinsinden boyutu. |

Admin API `password` ve `token` değerlerini döndürmez.

## Store'daki key'ler

Her key `<prefix>/v1/` ile başlar.

| Key | İçerik | Şifreli |
|---|---|---|
| `schema` | Veri düzeninin sürümü. Import bu key'i en son yazar. | hayır |
| `state/config` | Ayarlar. | evet |
| `state/ports/<id>` | Portlar. | hayır |
| `state/proxies/<id>` | Proxy'ler. | evet |
| `state/access-lists/<id>` | Parola hash'leri ve token digest'leriyle access list'ler. | evet |
| `state/certs/<kind>/<id>` | Sertifikalar ve private key'leri. | evet |
| `state/acme/<id>` | Account key'leri ve DNS provider credential'larıyla ACME kayıtları. | evet |
| `state/accounts/<hex ad>` | Admin hesapları. | evet |
| `state/cdn` | CDN IP aralıkları. | hayır |
| `state/challenges/http/<hex token>`, `state/challenges/tls-alpn/<hex domain>` | Her node'un sunduğu ACME challenge'ları. ACME sunucusu bu değerleri zaten yayınlar. | hayır |
| `state/cache-purges/<proxy id>` | Bir proxy'nin son cache purge zamanı. | hayır |
| `lock/leader` | Leader'ın lease'ine bağlı leader lock'u. | evet |
| `nodes/<hex ad>` | Bir node'un lease'ine bağlı varlık kaydı. | hayır |
| `acks/<hex ad>` | Bir node'un sunduğu challenge'ların digest'i. | hayır |
| `sessions/<scope>/<token'ın SHA-256'sı>` | Admin ve proxy session'ları. Store hiçbir zaman token'ın kendisini tutmaz. | evet |
| `ratelimit/<hex ad>` | Bir node'un client IP adresleriyle rate limit sayıları. | evet |
| `cache/<proxy id>/<cache key'in SHA-256'sı>` | Paylaşılan bir cache'lenmiş response. | evet |
| `audit/<YYYY-MM-DD>/<Unix ms>-<rastgele>` | Gününün key'i altındaki bir audit log kaydı. | evet |
| `notify` | Webhook'un aldığı sertifika olayları. Bu key'i yalnız leader yazar. | evet |

## Şifreleme

Node her değeri yazmadan önce AES-256-GCM ile şifreler. Değerin KV key'i associated data olarak kullanılır. Bu yüzden başka bir key'e taşınan değer çözülmez. Şifreli değer, key'inin id'si ile başlar: key'in SHA-256 digest'inin ilk 8 byte'ı.

Bir key'i değiştirmek için:

1. `r3v3rs3 cluster keygen` ile yeni bir key dosyası oluşturun.
2. Yeni dosyayı `encryption_key_files` listesinin başına koyun ve eski dosyayı listede bırakın. Bunu her node'da yapın ve node'ları yeniden başlatın. Node key dosyalarını başlarken okur.
3. Bir node'da `r3v3rs3 cluster rekey --config-dir <dir>` komutunu çalıştırın. Komut her değeri ilk key ile yeniden şifreler ve değiştirdiği değerleri raporlar.
4. Eski dosyayı her node'da `encryption_key_files` listesinden çıkarın ve node'ları yeniden başlatın.

Store'u da koruyun. Şifreleme değerleri gizler, ama key adları portların, proxy'lerin ve sertifikaların id'lerini gösterir.

## etcd izinleri

Node'ların kullanıcısı prefix'in altındaki key'leri okuyup yazabilmelidir:

```bash
$ etcdctl role add r3v3rs3
$ etcdctl role grant-permission --prefix=true r3v3rs3 readwrite r3v3rs3/
$ etcdctl user add r3v3rs3
$ etcdctl user grant-role r3v3rs3 r3v3rs3
```

Başka bir `prefix` kullanıyorsanız izni `<prefix>/` için verin. etcd authentication token'ını iptal ederse node yeni bir token alır.

## Consul izinleri

Node'ların token'ı prefix'in altındaki key'lere yazabilmeli ve session oluşturabilmelidir:

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

`make test-cluster-e2e` hedefi node'ları tam olarak bu izinlerle çalıştırır.

## Leader ve ACME

Leader, `lock_ttl` süreli bir lease ile `lock/leader` key'ini tutar. Lease'i her `lock_ttl` süresinde üç kez yeniler. Leader'ın bir store çağrısı en fazla `lock_ttl` süresinin üçte biri kadar bekler. Bu yüzden leader, lease'i store'da bitmeden leader'lığı bırakır. Leader dururken lock'u bırakır ve başka bir node bir sonraki kontrolünde leader olur.

HTTP-01 veya TLS-ALPN-01 challenge'ında leader challenge'ı store'a yazar. Her node challenge'ı sunar. Her node, sunduğu challenge'ların digest'ini `acks/` altına yazar. Leader, var olan her node yeni challenge'ları bildirene kadar en fazla 10 saniye bekler. Sonra ACME sunucusundan doğrulama ister.

## Hata durumları

- Node store'u her `lock_ttl` süresinde üç kez kontrol eder. Node `lock_ttl` süresince başarılı bir okuma yapamazsa durumu `degraded` olur.
- Degraded node son uygulanan state ile trafik almaya devam eder. Admin API değişiklikleri `503 cluster_unavailable` ile reddeder. WebUI'ın her sayfası bir uyarı gösterir.
- Store çağrıları başarısız olan leader, leader'lığı bırakır. Bu yüzden degraded node leader olmaz.
- Yeni bir login ve yeni bir proxy session'ı store'a ihtiyaç duyar. Store yoksa `503` ile başarısız olur.
- Store geri gelince node bütün state'i yeniden okur ve durumu `synced` olur. Node yerel state'ini store'a yazmaz.
- Başlarken store'a ulaşamayan node `startup_timeout` sonunda durur.

## Session'lar

Admin session'ları ve proxy authentication session'ları store'da durur. Bu yüzden bir node'un başlattığı session her node'da geçerlidir. Logout session'ı her node için siler. Node, session taşıyan her request için store'u okur. Store yoksa proxy session'ı geçersiz sayılır ve admin API `503 cluster_unavailable` döner.

## Rate limit doğruluğu

Node her request için store'u okumaz. Her node her client'ın request'lerini sayar. Her `rate_limit_sync_interval` sürede her limit için en yoğun 2048 client'ın sayılarını yayınlar. Node, diğer node'ların yeni sayılarını kendi sayılarına ekler. Üç interval'dan eski sayılar sayılmaz. Üç interval 2 saniyeden kısaysa sınır 2 saniyedir.

- Periyodu `rate_limit_sync_interval` değerinin en az 10 katı olan limit, bütün node'ların sayılarını kullanır. Diğer node'ların sayıları en fazla bir interval eskidir. Her node'a aynı anda ulaşan bir burst, limitin `(N − 1) × burst` kadar üstüne çıkabilir. `N` node sayısıdır.
- Periyodu daha kısa olan limit, limiti var olan node'lar arasında böler. Her node `ceil(limit / N)` request'e izin verir. Bir client'ı tek node'a gönderen load balancer, o client'a limitten daha az hak verir.
- Store yoksa her node, bilinen son node sayısıyla bölünmüş limiti kullanır.

## Paylaşılan cache

`share_cache = true` iken node, en az 60 saniye fresh kalan her cache'lenmiş response'u store'a yazmak için kuyruğa koyar. Node yazmayı beklemez. Yerel miss durumunda node store'u en fazla 100 milisaniye okur.

- Encode edilmiş boyutu `cache_max_value_size`, etcd için 1 MiB veya Consul için 350 KiB değerlerinden küçük olanını aşan response, kendi node'unda kalır. Body base64 olarak saklanır, bu yüzden saklanan boyut body boyutunun yaklaşık 4/3'üdür. Node bir response'u birden fazla key'e bölmez.
- Bir proxy'nin cache purge işlemi `state/cache-purges/<proxy id>` key'ini yazar ve proxy'nin paylaşılan response'larını siler. Her node değişiklikten sonra kendi yerel cache'ini temizler.
- Paylaşılan response stale olduktan sonra bir saat daha store'da kalır, böylece bir node onu revalidate edebilir. Leader süresi dolan response'ları her `background_task_interval` sürede siler. Silme işlemi paylaşılan her response'u okur ve çözer. Bu yüzden maliyeti saklanan response sayısıyla büyür.
- Saklanan her response store'a bir yazmadır. Store her yazmayı compaction'a kadar geçmişinde tutar. Bu yüzden yoğun bir cache, compaction'lar arasında etcd'nin boyutunu büyütür.

## Saatler

Node'lar session süresi, rate limit pencereleri, rate limit sayılarının yeniliği ve paylaşılan response'ların süresi için Unix zamanlarını karşılaştırır. Node'ların saatlerini NTP ile senkron tutun. Saniyeler düzeyinde ileride veya geride olan bir saat rate limit kararlarını ve cache sürelerini değiştirir.
