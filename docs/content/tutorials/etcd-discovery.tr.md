+++
title = "etcd'den proxy'ler"
description = "Bir etcd cluster'ının key'lerinden proxy kurun"
weight = 13
+++

# etcd'den proxy'ler

etcd key'leri yazan bir deploy işi, kendi proxy tanımını da taşıyabilir. r3v3rs3 bir önekin altındaki key'leri okur ve proxy'leri yeniden başlatmadan, WebUI'de tek tıklama yapmadan kurar. Değişen bir key, onun proxy'sini yaklaşık bir saniye içinde günceller.

[Başlangıç](@/tutorials/getting-started.tr.md) rehberindeki bir port gerekir. Bu rehber `web` adlı bir HTTP portu kullanır. Sağlayıcı hiçbir zaman port açmaz, yalnız var olan bir portun adını kullanır.

## Adım 1: Sağlayıcıyı açın

**Ayarlar** sayfasında **etcd Servis Keşfi** alanlarını doldurun veya `config.toml` dosyasını düzenleyin:

```toml
[discovery.etcd]
enabled = true
endpoints = ["http://10.0.0.1:2379", "http://10.0.0.2:2379"]
username = "r3v3rs3"
password = "<password>"
prefix = "r3v3rs3"
```

Bir bağlantı başarısız olursa r3v3rs3 bir sonraki adrese bağlanır. etcd authentication kullanıyorsanız önek için salt okunur bir kullanıcı oluşturun:

```bash
$ etcdctl role add r3v3rs3-reader
$ etcdctl role grant-permission r3v3rs3-reader --prefix=true read r3v3rs3/
$ etcdctl user add r3v3rs3
$ etcdctl user grant-role r3v3rs3 r3v3rs3-reader
```

Sağlayıcının durumu proxy listesinde ve `GET /api/discovery` yanıtında görünür:

```bash
$ curl -s -b cookies.txt http://localhost:46492/api/discovery
[{"provider":"etcd","state":"running","proxies":0,"updated_at":1789647725}]
```

`connecting`, ilk okumanın bitmediği anlamına gelir. `running`, sağlayıcının değişiklikleri izlediği anlamına gelir. `error`, okuyamadığı anlamına gelir; son okumanın proxy'leri etkin kalır.

Ayar değişikliği yalnız sağlayıcıyı yeniden başlatır, r3v3rs3 yeniden başlamaz.

## Adım 2: Proxy'yi yazın

Önekin altındaki her key bir etikettir; `.` yerine `/` kullanılır:

```bash
$ etcdctl put r3v3rs3/http/etcdapp/ports web
$ etcdctl put r3v3rs3/http/etcdapp/vhosts etcd.example.com
$ etcdctl put r3v3rs3/http/etcdapp/routes/0/servers/0/url http://10.0.0.9:8080/
```

Yaklaşık bir saniye içinde proxy listesinde yeni bir proxy belirir:

```bash
$ curl -s -b cookies.txt http://localhost:46492/api/proxies | jq -c '.[] | {id, name, source}'
{"id":"cfr-kpm","name":"etcdapp","source":{"provider":"etcd","resource":"http/etcdapp"}}
```

Key set'i `ports`, `vhosts` ve bir sunucu URL'i ayarlar. Key adları, yönetim API'sinin proxy modelinin alanlarıdır; tam liste için [Etiketler](@/discovery.tr.md#etiketler) bölümüne bakın.

r3v3rs3 önekin altındaki her proxy'yi okur. Bu yüzden key set'i `r3v3rs3.enable` gerektirmez. Key'in adresi yoktur; bu yüzden `port` ve `scheme` kullanılamaz, sunucu URL'ini yazın. Bir key'in parçası `.` içeremez; böyle bir key sorun sayılır.

Watch stream her değişikliği raporlar. Sunucu URL'ini değiştirin, proxy yaklaşık bir saniye içinde güncellenir:

```bash
$ etcdctl put r3v3rs3/http/etcdapp/routes/0/servers/0/url http://10.0.0.10:8080/
```

etcd'nin password'ü yönetim API'sinden geri dönmez. Kayıtlı bir password varsa `GET /api/config` yanıtında `password_set: true` döner. `password` alanı olmayan bir `PUT /api/config` isteği kayıtlı password'ü korur. Boş string password'ü siler. `config.toml` password'ü düz metin olarak saklar; bu yüzden yapılandırma dizinini yalnız r3v3rs3 kullanıcısı okuyabilmelidir.

## Adım 3: Sorunları okuyun

Proxy olmayan bir tanım sorun sayılır. Proxy listesi ve `GET /api/discovery`, kaynağı ve nedeni bildirir:

```bash
$ etcdctl put r3v3rs3/http/badapp/ports nosuchport
$ etcdctl put r3v3rs3/http/badapp/vhosts bad.example.com
```

```json
{"provider":"etcd","state":"running","proxies":1,
 "issues":[{"resource":"r3v3rs3","message":"http.badapp: missing field `routes`"}]}
```

Eksik alanı ekleyin; mesaj bu kez bir sonraki sorunu bildirir:

```json
{"issues":[{"resource":"r3v3rs3/http/badapp","message":"http.badapp: port not found: nosuchport"}]}
```

Her değişiklikten sonra sorunları okuyun. Sorunu olan bir proxy eklenmez ve tanımın yanlış olduğunu başka hiçbir şey bildirmez.

## Adım 4: Yapamayacaklarınızı bilin

Keşfedilen bir proxy salt okunurdur:

```bash
$ curl -s -b cookies.txt -X DELETE http://localhost:46492/api/proxies/cfr-kpm
{"message":"proxy is managed by service discovery and cannot be changed: cfr-kpm","error":{"message":"proxy_read_only","id":"cfr-kpm"}}
```

WebUI, proxy'yi kaynağıyla ve düzenleme eylemleri olmadan gösterir. Değiştirmek için key'i değiştirin.

- r3v3rs3 keşfedilen proxy'leri `proxies.toml` dosyasına kaydetmez. Sağlayıcı, sunucu yeniden başladığında onları yeniden gönderir.
- id, sağlayıcıdan ve tanımın key'inden gelir; yeniden başlatmada korunur.
- Sağlayıcı port açmaz. Bir port adı tam olarak bir portu seçmelidir ve port, proxy'nin protokolünü kabul etmelidir.
- Key içindeki düz metin bir password veya token, kaynakta düz metin olarak kalır. `password_hash` ve `token_hash` kullanın.

## Referans

- [Etiketler](@/discovery.tr.md#etiketler): her key, her değer türü ve numaralı listeler.
- [etcd](@/discovery.tr.md#etcd): ayarlar ve varsayılan değerleri.
- [ACME sertifikaları](@/discovery.tr.md#acme-sertifikalari): keşfedilen bir proxy için sertifika.

## Sonraki adımlar

- [Docker'dan proxy'ler](@/tutorials/docker-discovery.tr.md): aynı key'ler container etiketi olarak.
- [Yüksek erişilebilirlik](@/tutorials/high-availability.tr.md): tek bir durum bilgisini paylaşan birkaç düğüm.
