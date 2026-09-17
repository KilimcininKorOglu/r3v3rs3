+++
title = "Consul ve etcd'den proxy'ler"
description = "Consul catalog'undan, Consul key-value store'undan ve etcd key'lerinden proxy kurun"
weight = 13
+++

# Consul ve etcd'den proxy'ler

Kendini Consul'a kaydeden bir servis veya etcd key'leri yazan bir deploy işi, kendi proxy tanımını da taşıyabilir. r3v3rs3 bu tanımı okur ve proxy'yi restart olmadan, WebUI'da tek tıklama yapmadan kurar.

Bu rehber üç kaynağı da kullanır:

- Consul catalog'u: proxy'yi servis instance'ının tag'leri tanımlar.
- Consul key-value store'u: hiçbir servisin kaydetmediği bir proxy için.
- etcd key'leri: aynı düzen.

[Başlangıç](@/tutorials/getting-started.tr.md) rehberindeki bir port gerekir. Bu rehber `web` adlı bir HTTP portu kullanır. Provider hiçbir zaman port açmaz, yalnız var olan bir portun adını kullanır.

## Adım 1: Provider'ı açın

**Ayarlar** sayfasında **Consul Service Discovery** alanlarını doldurun veya `config.toml` dosyasını düzenleyin:

```toml
[discovery.consul]
enabled = true
address = "http://127.0.0.1:8500"
token = "<ACL token>"
catalog = true
kv = true
prefix = "r3v3rs3"
exposed_by_default = false
```

`exposed_by_default = false` yalnız `r3v3rs3.enable=true` tag'ini taşıyan servisleri okur. `true` ise `r3v3rs3.` tag'i olan her servis okunur.

Token üç izne ihtiyaç duyar:

```hcl
service_prefix "" { policy = "read" }
node_prefix "" { policy = "read" }
key_prefix "r3v3rs3/" { policy = "read" }
```

Catalog `service:read` ve `node:read`, key-value store ise prefix üzerinde `key:read` ister. Yalnız okuma yetkisi olan bir token verin; r3v3rs3 Consul'a hiçbir şey yazmaz.

Provider durumu proxy listesinde ve `GET /api/discovery` yanıtında görünür:

```bash
$ curl -s -b cookies.txt http://localhost:46492/api/discovery
[{"provider":"consul","state":"running","proxies":0,"updated_at":1789647725}]
```

`connecting`, ilk okumanın bitmediği anlamına gelir. `running`, provider'ın değişiklikleri izlediğini gösterir. `error` ise okuyamadığını gösterir; son okumanın proxy'leri aktif kalır.

Ayar değişikliği provider'ı yeniden başlatır. r3v3rs3'ün kendisi yeniden başlamaz.

## Adım 2: Catalog'da proxy tanımlayın

Servisi `r3v3rs3.` tag'leriyle kaydedin:

```json
{
  "Name": "whoami",
  "ID": "whoami-1",
  "Address": "10.0.0.5",
  "Port": 8080,
  "Tags": [
    "r3v3rs3.enable=true",
    "r3v3rs3.http.whoami.ports=web",
    "r3v3rs3.http.whoami.vhosts=whoami.example.com"
  ],
  "Check": { "HTTP": "http://10.0.0.5:8080/", "Interval": "10s" }
}
```

```bash
$ consul services register whoami.json
```

Yaklaşık bir saniye içinde proxy listesinde yeni bir proxy belirir:

```bash
$ curl -s -b cookies.txt http://localhost:46492/api/proxies | jq -c '.[] | {id, name, source}'
{"id":"cfr-kpm","name":"whoami","source":{"provider":"consul","resource":"whoami"}}
```

Proxy'de `routes` tag'i yoktur, bu yüzden servis instance'ının adresini ve portunu tek upstream sunucu olarak kullanır. Tag key'leri, admin API'deki proxy modelinin alanlarıdır; tam liste için [Labels](@/discovery.tr.md#label-lar) bölümüne bakın.

`=` içermeyen bir `r3v3rs3.` tag'i issue'dur. r3v3rs3 yalnız health check'ini geçen instance'ları okur, yani check'i başarısız olan servis proxy'sini kaybeder.

## Adım 3: Instance'lar yükü paylaşsın

Aynı servisin ikinci instance'ını başka bir adrese kaydedin:

```bash
$ consul services register whoami-2.json
```

Instance'lar proxy'yi paylaşır. Her biri kendi sunucusunu route'lara ekler:

```bash
$ curl -s -b cookies.txt http://localhost:46492/api/proxies/cfr-kpm/status
{"state":"active","upstreams":[
  {"addr":"http://10.0.0.5:8080/","weight":1,"healthy":true,"failures":0},
  {"addr":"http://10.0.0.6:8080/","weight":1,"healthy":true,"failures":0}]}
```

Round robin bundan sonra `10.0.0.5, 10.0.0.6, 10.0.0.5, 10.0.0.6` sırasıyla cevap verir. Bir instance'ın kaydını silin, sunucusu yaklaşık bir saniye içinde listeden çıkar. Sizin hiçbir ayar değiştirmeniz gerekmez.

Catalog'un amacı budur: servisi ölçeklersiniz, proxy takip eder.

## Adım 4: Servissiz proxy

Kimsenin kaydetmediği bir servis, örneğin Consul dışındaki bir makine, key-value store ister. Prefix altındaki bir key, `.` yerine `/` kullanan bir label'dır:

```bash
$ consul kv put r3v3rs3/http/kvapp/ports web
$ consul kv put r3v3rs3/http/kvapp/vhosts kv.example.com
$ consul kv put r3v3rs3/http/kvapp/routes/0/servers/0/url http://10.0.0.9:8080/
```

r3v3rs3 prefix altındaki her proxy'yi okur, bu yüzden key'lerde `r3v3rs3.enable` gerekmez. Key'in adresi yoktur, bu yüzden `port` ve `scheme` kullanılamaz; sunucu URL'ini yazın.

Key'in bir parçası `.` içeremez. Böyle bir key issue'dur.

## Adım 5: Aynı key'ler etcd'de

etcd aynı düzeni kullanır. **Ayarlar** sayfasında **etcd Service Discovery** alanlarını doldurun veya `config.toml` dosyasını düzenleyin:

```toml
[discovery.etcd]
enabled = true
endpoints = ["http://10.0.0.1:2379", "http://10.0.0.2:2379"]
username = "r3v3rs3"
password = "<parola>"
prefix = "r3v3rs3"
```

Bir bağlantı kurulamazsa r3v3rs3 sıradaki adrese bağlanır. etcd authentication açıksa prefix için yalnız okuma yetkisi olan bir kullanıcı oluşturun:

```bash
$ etcdctl role add r3v3rs3-reader
$ etcdctl role grant-permission r3v3rs3-reader --prefix=true read r3v3rs3/
$ etcdctl user add r3v3rs3
$ etcdctl user grant-role r3v3rs3 r3v3rs3-reader
```

Sonra proxy'yi yazın:

```bash
$ etcdctl put r3v3rs3/http/etcdapp/ports web
$ etcdctl put r3v3rs3/http/etcdapp/vhosts etcd.example.com
$ etcdctl put r3v3rs3/http/etcdapp/routes/0/servers/0/url http://10.0.0.9:8080/
```

Proxy yaklaşık bir saniye içinde cevap verir:

```bash
$ curl -s -b cookies.txt http://localhost:46492/api/discovery
[{"provider":"consul","state":"running","proxies":2,"updated_at":1789647747},
 {"provider":"etcd","state":"running","proxies":1,"updated_at":1789647767}]
```

Ne Consul token'ı ne de etcd parolası admin API'den geri döner. `GET /api/config` yanıtı `token_set: true` ve `password_set: true` içerir. Alanı yazmayan bir `PUT /api/config` kayıtlı değeri korur, boş string ise değeri siler. `config.toml` ikisini de düz metin tutar, bu yüzden config dizinini yalnız r3v3rs3 kullanıcısı okuyabilsin.

## Adım 6: Issue'ları okuyun

Proxy'ye dönüşmeyen tanım bir issue'dur. Proxy listesi ve `GET /api/discovery` kaynağı ve sebebi yazar:

```bash
$ etcdctl put r3v3rs3/http/badapp/ports nosuchport
$ etcdctl put r3v3rs3/http/badapp/vhosts bad.example.com
```

```json
{"provider":"etcd","state":"running","proxies":1,
 "issues":[{"resource":"r3v3rs3","message":"http.badapp: missing field `routes`"}]}
```

Eksik alanı ekleyin, mesaj sıradaki sorunu yazar:

```json
{"issues":[{"resource":"r3v3rs3/http/badapp","message":"http.badapp: port not found: nosuchport"}]}
```

Her değişiklikten sonra issue'ları okuyun. Issue'su olan proxy eklenmez ve tanımın yanlış olduğunu size başka hiçbir şey söylemez.

## Adım 7: Neyi yapamayacağınızı bilin

Discovery ile gelen proxy read-only'dir:

```bash
$ curl -s -b cookies.txt -X DELETE http://localhost:46492/api/proxies/cfr-kpm
{"message":"proxy is managed by service discovery and cannot be changed: cfr-kpm","error":{"message":"proxy_read_only","id":"cfr-kpm"}}
```

WebUI proxy'yi kaynağıyla birlikte ve düzenleme işlemleri olmadan gösterir. Değiştirmek için tag'i veya key'i değiştirin.

- r3v3rs3 discovery proxy'lerini `proxies.toml` dosyasına yazmaz. Provider onları restart'tan sonra yeniden gönderir.
- Id, provider'dan ve tanımın key'inden gelir, bu yüzden restart'tan sonra da aynı kalır.
- Provider port açmaz. Port adı tam olarak bir portu seçmelidir ve port, proxy'nin protokolünü kabul etmelidir.
- Tag veya key içindeki düz metin parola ve token kaynakta düz metin kalır. `password_hash` ve `token_hash` kullanın.

## Referans

- [Labels](@/discovery.tr.md#label-lar): her key, her değer tipi ve numaralı listeler.
- [Consul](@/discovery.tr.md#consul) ve [etcd](@/discovery.tr.md#etcd): ayarlar ve varsayılanları.
- [ACME sertifikaları](@/discovery.tr.md#acme-sertifikalari): discovery proxy'si için sertifika.

## Sonraki adımlar

- [Docker'dan proxy'ler](@/tutorials/docker-discovery.tr.md): aynı key'ler container label'ı olarak.
- [Kubernetes'te r3v3rs3](@/tutorials/kubernetes.tr.md): Ingress kaynakları ve R3v3rs3Proxy kaynağı.
- [Yüksek erişilebilirlik](@/tutorials/high-availability.tr.md): cluster store olarak aynı Consul veya etcd.
