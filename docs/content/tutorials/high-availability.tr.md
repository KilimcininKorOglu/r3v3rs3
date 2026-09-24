+++
title = "Yüksek erişilebilirlik"
description = "Yük dengeleyici arkasında üç düğümü adım adım kurun"
weight = 4
aliases = ["tr/high-availability/"]
+++

# Yüksek erişilebilirlik

r3v3rs3'ü tek arıza noktası olmadan çalıştırmanın tek yolu cluster modudur. Birkaç düğüm, durum bilgisini etcd'de veya Consul'un key-value store'unda paylaşır. Önlerindeki yük dengeleyici her isteği herhangi birine gönderir. Başka bir yedeğe geçiş mekanizması yoktur.

Bu sayfa bir kurulum rehberidir. Baştan sona izlerseniz çalışan bir kurulum elde edersiniz. [Cluster](@/cluster.tr.md) ise referans sayfasıdır: her ayarı, veri deposundaki her key'i ve her arıza durumunu anlatır.

## Ne kuruyorsunuz

```
                    istemciler
                       |
                 10.0.0.10 (VIP)
                       |
          +------------+------------+
          |            |            |
     10.0.0.11    10.0.0.12    10.0.0.13     r3v3rs3 düğümleri
          |            |            |
          +------------+------------+
                       |
          +------------+------------+
          |            |            |
     10.0.2.11    10.0.2.12    10.0.2.13     etcd veya Consul
```

| Arıza | Sonuç |
|---|---|
| Bir r3v3rs3 düğümü durur | keepalived VIP'i başka bir düğüme taşır. Diğer düğümler aynı durum bilgisini tutar. |
| Lider durur | Duran lider kilidi bırakır ve başka bir düğüm `lock_ttl / 3` içinde lider olur. Çökmeden sonra lease bitince, yaklaşık `lock_ttl + lock_ttl / 3` içinde başka bir düğüm lider olur. ACME siparişleri ve arka plan görevleri orada devam eder. |
| Bir veri deposu düğümü durur | Veri deposu quorum'unu korur. r3v3rs3 için hiçbir şey değişmez. |
| Veri deposu quorum'unu kaybeder | Her düğüm `degraded` olur. İstekler son durum bilgisiyle karşılanmaya devam eder, değişiklikler reddedilir. |

Üç düğüm işe yarayan en küçük boyuttur. İki düğüm veri deposunda quorum sağlamaz ve bütün cluster'ın erişilebilirliğine veri deposu karar verir.

## Başlamadan önce

| Sunucu | Adres | Görev |
|---|---|---|
| `proxy-1` | `10.0.0.11` | r3v3rs3 düğümü |
| `proxy-2` | `10.0.0.12` | r3v3rs3 düğümü |
| `proxy-3` | `10.0.0.13` | r3v3rs3 düğümü |
| `store-1` | `10.0.2.11` | etcd veya Consul |
| `store-2` | `10.0.2.12` | etcd veya Consul |
| `store-3` | `10.0.2.13` | etcd veya Consul |
| VIP | `10.0.0.10` | DNS kayıtlarınızın adresi |

Gereksinimler:

- Her sunucuda Linux. r3v3rs3 yalnız Linux'u destekler.
- Her sunucuda NTP. Düğümler oturumlar, rate limit pencereleri ve cache süreleri için Unix zamanlarını karşılaştırır. Bakınız [Saatler](@/cluster.tr.md#saatler).
- Düğümler veri deposuna `2379` (etcd) veya `8500` (Consul) portundan erişir.
- keepalived kullanırsanız düğümler birbirine VRRP (protokol 112) ile erişir.
- İstemciler VIP'e proxy portlarınızdan erişir, örneğin `80` ve `443`.

Veri deposu sertifikalarınızı ve parolalarınızı şifreli biçimde tutar. Onu özel bir ağa koyun.

## Adım 1: Veri deposunu kurun

etcd veya Consul'dan birini kurun. r3v3rs3 için ikisi de aynı şekilde çalışır.

### etcd

Bu unit'i üç veri deposu sunucusunun her birinde, o sunucunun `<NAME>` ve `<IP>` değerleriyle çalıştırın:

```ini
[Unit]
Description=etcd
After=network.target

[Service]
ExecStart=/usr/local/bin/etcd \
  --name <NAME> \
  --data-dir /var/lib/etcd \
  --listen-client-urls http://<IP>:2379 \
  --advertise-client-urls http://<IP>:2379 \
  --listen-peer-urls http://<IP>:2380 \
  --initial-advertise-peer-urls http://<IP>:2380 \
  --initial-cluster store-1=http://10.0.2.11:2380,store-2=http://10.0.2.12:2380,store-3=http://10.0.2.13:2380 \
  --initial-cluster-state new
Restart=always

[Install]
WantedBy=multi-user.target
```

Veri deposunu kontrol edin:

```bash
$ etcdctl --endpoints=http://10.0.2.11:2379,http://10.0.2.12:2379,http://10.0.2.13:2379 endpoint health
```

Her endpoint `is healthy` demelidir.

### Consul

Bu yapılandırmayı üç veri deposu sunucusunun her birinde, o sunucunun `<NAME>` ve `<IP>` değerleriyle çalıştırın:

```json
{
  "node_name": "<NAME>",
  "server": true,
  "bootstrap_expect": 3,
  "bind_addr": "<IP>",
  "client_addr": "0.0.0.0",
  "data_dir": "/var/lib/consul",
  "retry_join": ["10.0.2.11", "10.0.2.12", "10.0.2.13"],
  "acl": { "enabled": true, "default_policy": "deny" }
}
```

ACL sistemini bir sunucuda bootstrap edin ve yönetim token'ını saklayın:

```bash
$ consul acl bootstrap
$ consul members
```

`consul members` üç sunucuyu `alive` durumunda listelemelidir.

## Adım 2: Kimlik bilgilerini oluşturun

Düğümler, yalnız kendi öneklerinin altındaki key'leri okuyup yazan bir hesaba ihtiyaç duyar.

### etcd

```bash
$ etcdctl role add r3v3rs3
$ etcdctl role grant-permission --prefix=true r3v3rs3 readwrite r3v3rs3/
$ etcdctl user add r3v3rs3 --new-user-password=<parola>
$ etcdctl user grant-role r3v3rs3 r3v3rs3
```

`--new-user-password` olmadan komut parolayı terminalden sorar. etcd'nin kimlik doğrulamasını bir `root` kullanıcısı oluşturduktan sonra açın, çünkü kimlik doğrulama kapalıyken etcd hiçbir izni kontrol etmez:

```bash
$ etcdctl user add root --new-user-password=<root parolası>
$ etcdctl user grant-role root root
$ etcdctl auth enable
```

Yeni kullanıcının önek dışına yazamadığını kontrol edin:

```bash
$ etcdctl --user r3v3rs3:<parola> put r3v3rs3/probe ok
OK
$ etcdctl --user r3v3rs3:<parola> put other/probe ok
Error: etcdserver: permission denied
$ etcdctl --user r3v3rs3:<parola> del r3v3rs3/probe
```

### Consul

Politikayı `r3v3rs3.hcl` dosyasına yazın:

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

Yeni token'ın `SecretID` değerini saklayın. Düğümlerin `token` alanı budur.

## Adım 3: Her düğüme r3v3rs3 kurun

```bash
$ curl -fsSL https://raw.githubusercontent.com/KilimcininKorOglu/r3v3rs3/main/install.sh | sudo bash
```

Script binary'yi kurar, `/etc/r3v3rs3` dizinini oluşturur, bir admin kullanıcı adı ile parola sorar ve r3v3rs3'ü systemd servisi olarak çalıştırır. Bu hesap `/etc/r3v3rs3/accounts.toml` dosyasına yazılır. Cluster'ı açık olan bir düğüm bu dosyayı okumaz, bu yüzden Adım 6 cluster'ın hesabını yeniden oluşturur. Script yönetim WebUI adresini de sorar. `0.0.0.0:46492` veya düğümün adresini girin, çünkü Adım 7 her düğümün WebUI'ını ağ üzerinden açar.

Cluster yapılandırması hazır olana kadar servisi her düğümde durdurun:

```bash
$ sudo systemctl stop r3v3rs3
```

Docker kullanıyorsanız her düğümde repository'nin `docker-compose.yml` dosyasını kullanın ve `/etc/r3v3rs3` dizinini `/root/.config/r3v3rs3` yoluna bağlayın. `config.toml` içinde container içindeki yolları yazın, örneğin `encryption_key_files = ["/root/.config/r3v3rs3/cluster.key"]`. `network_mode: host` satırını koruyun, çünkü proxy portları host üzerinde dinler.

## Adım 4: Şifreleme key'ini oluşturun

Düğümler veri deposundaki her değeri şifreler. Tek bir key oluşturup her düğüme kopyalayın:

```bash
$ sudo r3v3rs3 cluster keygen /etc/r3v3rs3/cluster.key
Wrote the encryption key 855523573514e205 to /etc/r3v3rs3/cluster.key.
Keep a copy of the key file. The cluster data cannot be decrypted when every key file is lost.
```

Dosya `0600` izinleriyle oluşur. Komut var olan bir dosyanın üzerine yazmaz.

Aynı dosyayı `proxy-2` ve `proxy-3` sunucularına kopyalayın ve düğümlerin dışında da bir kopyasını saklayın. Bütün kopyalar kaybolursa veri deposundaki sertifikaları, parolaları ve proxy'leri kimse bir daha okuyamaz.

## Adım 5: Her düğümü yapılandırın

`/etc/r3v3rs3/config.toml` dosyasına `[cluster]` bölümünü ekleyin. Düğümler arasında yalnız `node_name` farklıdır.

etcd:

```toml
[cluster]
enabled = true
backend = "etcd"
endpoints = ["http://10.0.2.11:2379", "http://10.0.2.12:2379", "http://10.0.2.13:2379"]
username = "r3v3rs3"
password = "<parola>"
node_name = "proxy-1"
encryption_key_files = ["/etc/r3v3rs3/cluster.key"]
```

Consul:

```toml
[cluster]
enabled = true
backend = "consul"
endpoints = ["http://10.0.2.11:8500", "http://10.0.2.12:8500", "http://10.0.2.13:8500"]
token = "<SecretID>"
node_name = "proxy-1"
encryption_key_files = ["/etc/r3v3rs3/cluster.key"]
```

Canlı ortamda `https://` endpoint'lerini ve `tls` alanını kullanın. [Ayarlar](@/cluster.tr.md#ayarlar) her alanı varsayılanıyla listeler.

## Adım 6: Durum bilgisini içe aktarın ve admin hesabını ekleyin

İçe aktarmayı bir kez, yalnız bir düğümde çalıştırın:

```bash
$ sudo r3v3rs3 cluster import --config-dir /etc/r3v3rs3
Imported the config, 0 ports, 0 proxies, 0 access lists, 0 certificates, 0 ACME entries and 0 accounts.
```

Veri deposunun öneki boş olmalıdır. İçe aktarma `schema` key'ini en son yazar, bu yüzden yarıda kalan bir içe aktarma kullanılabilir bir önek bırakmaz: önekin altındaki key'leri silin ve içe aktarmayı yeniden çalıştırın.

Admin hesabını içe aktarmadan sonra ekleyin. Cluster açıkken hesap veri deposuna gider ve her düğüm onu görür:

```bash
$ sudo r3v3rs3 add-user admin --config-dir /etc/r3v3rs3
```

Komut parolayı sorar. Ad veri deposunda zaten varsa hesabın üzerine yazar, bu yüzden aynı komutla yeni parola da verebilirsiniz.

## Adım 7: Her düğümü başlatın

```bash
$ sudo systemctl start r3v3rs3
```

Her düğümün WebUI'ını açıp giriş yapın. **Ayarlar** sayfası düğümün durumunu, rolünü ve revizyonunu gösterir. Aynı veri yönetim API'sinden de gelir:

```bash
$ curl -b session.txt http://10.0.0.11:46492/api/cluster/status
{"state":"synced","node_name":"proxy-1","leader":false,"revision":247}
```

Her düğüm `synced` demelidir ve tam bir düğüm `"leader":true` demelidir. Başlamayan veya `degraded` diyen bir düğüm veri deposunu okuyamıyordur: log'unda kimlik bilgilerini ve endpoint'leri kontrol edin.

## Adım 8: Sağlık kontrolü route'u ekleyin

Yük dengeleyicinin, giriş yapmadan sorgulayabileceği bir adrese ihtiyacı vardır. Yönetim API'si oturum cookie'sinin arkasındadır, bu yüzden bu işi göremez. Bunun yerine sabit durum kodu döndüren bir route ekleyin.

**Portlar** sayfasında bir HTTP port ekleyin, örneğin `/ip4/0.0.0.0/tcp/80/http`. **Proxy'ler** sayfasında o port üzerinde tek route'lu bir HTTP proxy ekleyin:

- **Yol**: `/healthz`
- **Route Türü**: `Sabit durum kodu`, **Durum Kodu** `200`, **Gövde** `ok`

Değişiklik veri deposu üzerinden her düğüme ulaşır, bu yüzden tek bir WebUI yeterlidir. Her düğümü kendi adresinden kontrol edin:

```bash
$ curl -i http://10.0.0.11/healthz
HTTP/1.1 200 OK
...
ok
```

Route, düğüm istekleri karşıladığı sürece yanıt verir, düğüm `degraded` olsa bile. Yük dengeleyicinin ihtiyacı budur: duran bir düğümü listeden çıkarır, veri deposunu kaybeden bir düğümü değil.

## Adım 9: Öne yük dengeleyici koyun

### keepalived

keepalived `10.0.0.10` VIP'ini yanıt veren bir düğüme taşır. Üç düğüme de kurun.

`proxy-1` sunucusunda `/etc/keepalived/keepalived.conf`:

```
vrrp_script check_r3v3rs3 {
    script "/usr/bin/curl -sf -o /dev/null http://127.0.0.1/healthz"
    interval 2
    timeout 2
    rise 2
    fall 2
}

vrrp_instance r3v3rs3 {
    state MASTER
    interface eth0
    virtual_router_id 51
    priority 150
    advert_int 1
    authentication {
        auth_type PASS
        auth_pass <paylaşılan secret>
    }
    virtual_ipaddress {
        10.0.0.10/24
    }
    track_script {
        check_r3v3rs3
    }
}
```

`proxy-2` ve `proxy-3` sunucularında `state` değerini `BACKUP`, `priority` değerini `140` ve `130` yapın. `virtual_router_id` ve `auth_pass` üçünde de aynı kalmalıdır.

Servisi başlatmadan önce yapılandırmayı kontrol edin:

```bash
$ keepalived -t -f /etc/keepalived/keepalived.conf
```

DNS kayıtlarınızı `10.0.0.10` adresine yöneltin.

Bu kurulumda her isteği tek düğüm karşılar. Diğer ikisi aynı durum bilgisini tutar ve iki başarısız kontrolden sonra, yani `interval` çarpı `fall` saniye sonra görevi devralır.

### DNS round robin

Kaydınıza üç düğümün adresini birden verin:

```
proxy.example.com. 60 IN A 10.0.0.11
proxy.example.com. 60 IN A 10.0.0.12
proxy.example.com. 60 IN A 10.0.0.13
```

Bu, yükü VIP olmadan üç düğüme dağıtır. Tek sınırı vardır: DNS duran bir düğümü listeden çıkarmaz. İstemci TTL bitene kadar adresi kullanmayı sürdürür, bazı istemciler adresi daha da uzun süre cache'te tutar. İstemcileriniz başka bir adresi yeniden deniyorsa bunu kullanın, denemiyorsa keepalived kullanın.

## Cluster'ı doğrulayın

1. Bir düğümde proxy ekleyin. Bir saniye içinde diğer düğümlerde görünür.
2. Lideri durdurun: `sudo systemctl stop r3v3rs3`. Lider kilidi bırakır ve başka bir düğüm `lock_ttl / 3` içinde, varsayılan olarak 5 saniyede `"leader":true` der.
3. VIP'i kontrol edin: diğer düğümlerde `ip addr show eth0`. Biri artık `10.0.0.10` adresini tutar.
4. Durdurduğunuz düğümü yeniden başlatın. Bütün durum bilgisini veri deposundan okur ve `synced` der.
5. VIP üzerinden giriş yapın, örneğin `http://10.0.0.10:46492/`. VIP'i tutan düğümü durdurun ve sayfayı yenileyin. Oturum yeni düğümde de geçerlidir. Tarayıcı oturum cookie'sini yalnız onu yazan adrese gönderir. Bu yüzden kendi adresiyle açtığınız bir düğüm yeniden giriş ister.

## Cluster'ı işletin

**Düğüm ekleme.** r3v3rs3'ü kurun, `cluster.key` dosyasını kopyalayın, aynı `[cluster]` bölümünü yeni bir `node_name` ile yazın ve başlatın. İçe aktarmayı yeniden çalıştırmayın.

**Düğüm çıkarma.** Servisi durdurun. Düğümün `nodes/` key'i lease'i ile birlikte biter ve diğer düğümler onu saymayı bırakır. Düğümün keepalived yapılandırmasını da kaldırın.

**Yükseltme.** Önce takipçileri teker teker, en son lideri yükseltin. Her düğüm başlarken durum bilgisini yeniden okur.

**Şifreleme key'ini değiştirme.** [Şifreleme](@/cluster.tr.md#sifreleme) bölümünü izleyin. Sıra önemlidir: her düğüm key dosyalarını başlarken okur.

## Yedeğe geçişin çözmediği durumlar

- Veri deposunu kaybeden düğüm `degraded` olur. Son durum bilgisiyle istekleri karşılar, yönetim API'si değişiklikleri `503 cluster_unavailable` ile reddeder.
- Yeni giriş ve yeni proxy oturumu veri deposuna ihtiyaç duyar. Veri deposu yoksa `503` ile başarısız olur.
- Periyodu kısa olan rate limit düğümler arasında bölünür, bu yüzden tek düğüme giden bir istemci limitin tamamını alamaz. Bakınız [Rate limit doğruluğu](@/cluster.tr.md#rate-limit-dogrulugu).
- Yanıt cache'i, `share_cache = true` yazmadığınız sürece yereldir. Bakınız [Paylaşılan cache](@/cluster.tr.md#paylasilan-cache).

[Hata durumları](@/cluster.tr.md#hata-durumlari) her durumu ayrıntısıyla anlatır.
