+++
title = "Yüksek erişilebilirlik"
description = "Load balancer arkasında üç node'u adım adım kurun"
weight = 0
+++

# Yüksek erişilebilirlik

r3v3rs3'ü tek arıza noktası olmadan çalıştırmanın tek yolu cluster mode'dur. Birkaç node, state'i etcd'de veya Consul'un key-value store'unda paylaşır. Önlerindeki load balancer her request'i herhangi birine gönderir. Başka bir failover mekanizması yoktur.

Bu sayfa bir kurulum rehberidir. Baştan sona izlerseniz çalışan bir kurulum elde edersiniz. [Cluster](@/cluster.tr.md) ise referans sayfasıdır: her ayarı, store'daki her key'i ve her arıza durumunu anlatır.

## Ne kuruyorsunuz

```
                    client'lar
                       |
                 10.0.0.10 (VIP)
                       |
          +------------+------------+
          |            |            |
     10.0.0.11    10.0.0.12    10.0.0.13     r3v3rs3 node'ları
          |            |            |
          +------------+------------+
                       |
          +------------+------------+
          |            |            |
     10.0.2.11    10.0.2.12    10.0.2.13     etcd veya Consul
```

| Arıza | Sonuç |
|---|---|
| Bir r3v3rs3 node'u durur | keepalived VIP'i başka bir node'a taşır. Diğer node'lar aynı state'i tutar. |
| Leader durur | `lock_ttl` içinde başka bir node lead alır. ACME order'ları ve background task'lar orada devam eder. |
| Bir store node'u durur | Store quorum'unu korur. r3v3rs3 için hiçbir şey değişmez. |
| Store quorum'unu kaybeder | Her node `degraded` olur. Trafik son state ile devam eder, değişiklikler reddedilir. |

Üç node işe yarayan en küçük boyuttur. İki node store'da quorum vermez ve bütün cluster'ın erişilebilirliğine store karar verir.

## Başlamadan önce

| Sunucu | Adres | Görev |
|---|---|---|
| `proxy-1` | `10.0.0.11` | r3v3rs3 node'u |
| `proxy-2` | `10.0.0.12` | r3v3rs3 node'u |
| `proxy-3` | `10.0.0.13` | r3v3rs3 node'u |
| `store-1` | `10.0.2.11` | etcd veya Consul |
| `store-2` | `10.0.2.12` | etcd veya Consul |
| `store-3` | `10.0.2.13` | etcd veya Consul |
| VIP | `10.0.0.10` | DNS kayıtlarınızın adresi |

Gereksinimler:

- Her sunucuda Linux. r3v3rs3 yalnız Linux'u destekler.
- Her sunucuda NTP. Node'lar session'lar, rate limit pencereleri ve cache expiry için Unix zamanlarını karşılaştırır. Bakınız [Saatler](@/cluster.tr.md#saatler).
- Node'lar store'a `2379` (etcd) veya `8500` (Consul) portundan erişir.
- keepalived kullanırsanız node'lar birbirine VRRP (protokol 112) ile erişir.
- Client'lar VIP'e proxy portlarınızdan erişir, örneğin `80` ve `443`.

Store sertifikalarınızı ve parolalarınızı şifreli biçimde tutar. Onu özel bir ağa koyun.

## Adım 1: Store'u kurun

etcd veya Consul'dan birini kurun. r3v3rs3 için ikisi de aynı şekilde çalışır.

### etcd

Bu unit'i üç store sunucusunun her birinde, o sunucunun `<NAME>` ve `<IP>` değerleriyle çalıştırın:

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

Store'u kontrol edin:

```bash
$ etcdctl --endpoints=http://10.0.2.11:2379,http://10.0.2.12:2379,http://10.0.2.13:2379 endpoint health
```

Her endpoint `is healthy` demelidir.

### Consul

Bu config'i üç store sunucusunun her birinde, o sunucunun `<NAME>` ve `<IP>` değerleriyle çalıştırın:

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

ACL sistemini bir sunucuda bootstrap edin ve management token'ını saklayın:

```bash
$ consul acl bootstrap
$ consul members
```

`consul members` üç server'ı `alive` durumunda listelemelidir.

## Adım 2: Kimlik bilgilerini oluşturun

Node'lar, yalnız kendi prefix'i altındaki key'leri okuyup yazan bir hesaba ihtiyaç duyar.

### etcd

```bash
$ etcdctl role add r3v3rs3
$ etcdctl role grant-permission --prefix=true r3v3rs3 readwrite r3v3rs3/
$ etcdctl user add r3v3rs3 --new-user-password=<parola>
$ etcdctl user grant-role r3v3rs3 r3v3rs3
```

`--new-user-password` olmadan komut parolayı terminalden sorar. etcd'nin authentication'ını bir `root` kullanıcısı oluşturduktan sonra açın, çünkü authentication kapalıyken etcd hiçbir izni kontrol etmez:

```bash
$ etcdctl user add root --new-user-password=<root parolası>
$ etcdctl user grant-role root root
$ etcdctl auth enable
```

Yeni kullanıcının prefix dışına yazamadığını kontrol edin:

```bash
$ etcdctl --user r3v3rs3:<parola> put r3v3rs3/probe ok
OK
$ etcdctl --user r3v3rs3:<parola> put other/probe ok
Error: etcdserver: permission denied
$ etcdctl --user r3v3rs3:<parola> del r3v3rs3/probe
```

### Consul

Policy'yi `r3v3rs3.hcl` dosyasına yazın:

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

Yeni token'ın `SecretID` değerini saklayın. Node'ların `token` alanı budur.

## Adım 3: Her node'a r3v3rs3 kurun

```bash
$ curl -fsSL https://raw.githubusercontent.com/KilimcininKorOglu/r3v3rs3/main/install.sh | sudo bash
```

Script binary'yi kurar, `/etc/r3v3rs3` dizinini oluşturur, bir admin kullanıcı adı ile parola sorar ve r3v3rs3'ü systemd servisi olarak çalıştırır. Bu hesap `/etc/r3v3rs3/accounts.toml` dosyasına yazılır. Cluster'ı açık olan bir node bu dosyayı okumaz, bu yüzden Adım 6 cluster'ın hesabını yeniden oluşturur.

Cluster config'i hazır olana kadar servisi her node'da durdurun:

```bash
$ sudo systemctl stop r3v3rs3
```

Docker kullanıyorsanız her node'da deponun `docker-compose.yml` dosyasını kullanın ve `/etc/r3v3rs3` dizinini config volume'u olarak ekleyin. `network_mode: host` satırını koruyun, çünkü proxy portları host üzerinde dinler.

## Adım 4: Encryption key'i oluşturun

Node'lar store'daki her değeri şifreler. Tek bir key oluşturup her node'a kopyalayın:

```bash
$ sudo r3v3rs3 cluster keygen /etc/r3v3rs3/cluster.key
Wrote the encryption key 855523573514e205 to /etc/r3v3rs3/cluster.key.
Keep a copy of the key file. The cluster data cannot be decrypted when every key file is lost.
```

Dosya `0600` mode'u ile oluşur. Komut var olan bir dosyanın üzerine yazmaz.

Aynı dosyayı `proxy-2` ve `proxy-3` sunucularına kopyalayın ve node'ların dışında da bir kopyasını saklayın. Bütün kopyalar kaybolursa store'daki sertifikaları, parolaları ve proxy'leri kimse bir daha okuyamaz.

## Adım 5: Her node'u yapılandırın

`/etc/r3v3rs3/config.toml` dosyasına `[cluster]` bölümünü ekleyin. Node'lar arasında yalnız `node_name` farklıdır.

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

Production'da `https://` endpoint'leri ve `tls` alanını kullanın. [Ayarlar](@/cluster.tr.md#ayarlar) her alanı varsayılanıyla listeler.

## Adım 6: State'i import edin ve admin hesabını ekleyin

Import'u bir kez, yalnız bir node'da çalıştırın:

```bash
$ sudo r3v3rs3 cluster import --config-dir /etc/r3v3rs3
Imported the config, 0 ports, 0 proxies, 0 access lists, 0 certificates, 0 ACME entries and 0 accounts.
```

Store'un prefix'i boş olmalıdır. Import `schema` key'ini en son yazar, bu yüzden yarıda kalan bir import kullanılabilir bir prefix bırakmaz: prefix altındaki key'leri silin ve import'u yeniden çalıştırın.

Admin hesabını import'tan sonra ekleyin. Cluster açıkken hesap store'a gider ve her node onu görür:

```bash
$ sudo r3v3rs3 add-user admin --config-dir /etc/r3v3rs3
```

Komut parolayı sorar. Ad store'da zaten varsa hesabın üzerine yazar, bu yüzden aynı komutla yeni parola da verebilirsiniz.

## Adım 7: Her node'u başlatın

```bash
$ sudo systemctl start r3v3rs3
```

Her node'un WebUI'ını açıp giriş yapın. **Ayarlar** sayfası node'un durumunu, rolünü ve revision değerini gösterir. Aynı veri admin API'sinden de gelir:

```bash
$ curl -b session.txt http://10.0.0.11:46492/api/cluster/status
{"state":"synced","node_name":"proxy-1","leader":false,"revision":247}
```

Her node `synced` demelidir ve tam bir node `"leader":true` demelidir. Uzun süre `syncing` diyen bir node store'u okuyamıyordur: log'unda kimlik bilgilerini ve endpoint'leri kontrol edin.

## Adım 8: Health check route'u ekleyin

Load balancer'ın, giriş yapmadan sorgulayabileceği bir adrese ihtiyacı vardır. Admin API'si session cookie'sinin arkasındadır, bu yüzden bu işi göremez. Bunun yerine sabit status kodu döndüren bir route ekleyin.

**Portlar** sayfasında bir HTTP port ekleyin, örneğin `/ip4/0.0.0.0/tcp/80/http`. **Proxy'ler** sayfasında o port üzerinde tek route'lu bir HTTP proxy ekleyin:

- **Path**: `/healthz`
- **Route Tipi**: `Sabit status kodu`, **Status** `200`, **Body** `ok`

Değişiklik store üzerinden her node'a ulaşır, bu yüzden tek bir WebUI yeterlidir. Her node'u kendi adresinden kontrol edin:

```bash
$ curl -i http://10.0.0.11/healthz
HTTP/1.1 200 OK
...
ok
```

Route, node trafik sunduğu sürece yanıt verir, node `degraded` olsa bile. Load balancer'ın ihtiyacı budur: duran bir node'u listeden çıkarır, store'u kaybeden bir node'u değil.

## Adım 9: Öne load balancer koyun

### keepalived

keepalived `10.0.0.10` VIP'ini yanıt veren bir node'a taşır. Üç node'a da kurun.

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

Servisi başlatmadan önce config'i kontrol edin:

```bash
$ keepalived -t -f /etc/keepalived/keepalived.conf
```

DNS kayıtlarınızı `10.0.0.10` adresine yöneltin.

Bu kurulumda her request'i tek node sunar. Diğer ikisi aynı state'i tutar ve iki başarısız check'ten sonra, yani `interval` çarpı `fall` saniye sonra devralır.

### DNS round robin

Kaydınıza üç node'un adresini birden verin:

```
proxy.example.com. 60 IN A 10.0.0.11
proxy.example.com. 60 IN A 10.0.0.12
proxy.example.com. 60 IN A 10.0.0.13
```

Bu, yükü VIP olmadan üç node'a dağıtır. Tek sınırı vardır: DNS duran bir node'u listeden çıkarmaz. Client TTL bitene kadar adresi kullanmayı sürdürür, bazı client'lar daha da uzun süre cache'ler. Client'larınız başka bir adresi deniyorsa bunu kullanın, denemiyorsa keepalived kullanın.

## Cluster'ı doğrulayın

1. Bir node'da proxy ekleyin. Bir saniye içinde diğer node'larda görünür.
2. Leader'ı durdurun: `sudo systemctl stop r3v3rs3`. Başka bir node `lock_ttl` içinde, varsayılan olarak 15 saniyede `"leader":true` der.
3. VIP'i kontrol edin: diğer node'larda `ip addr show eth0`. Biri artık `10.0.0.10` adresini tutar.
4. Durdurduğunuz node'u yeniden başlatın. Bütün state'i store'dan okur ve `synced` der.
5. Bir node'da giriş yapın ve aynı tarayıcıyla başka bir node'un WebUI'ını açın. Session orada da geçerlidir.

## Cluster'ı işletin

**Node ekleme.** r3v3rs3'ü kurun, `cluster.key` dosyasını kopyalayın, aynı `[cluster]` bölümünü yeni bir `node_name` ile yazın ve başlatın. Import'u yeniden çalıştırmayın.

**Node çıkarma.** Servisi durdurun. `nodes/` key'i lease'i ile birlikte biter ve diğer node'lar onu saymayı bırakır. keepalived config'ini de kaldırın.

**Yükseltme.** Önce follower'ları teker teker, en son leader'ı yükseltin. Her node başlarken state'i yeniden okur.

**Encryption key değiştirme.** [Şifreleme](@/cluster.tr.md#sifreleme) bölümünü izleyin. Sıra önemlidir: her node key dosyalarını başlarken okur.

## Neyin failover'ı yoktur

- Store'u kaybeden node `degraded` olur. Son state ile trafik sunar, admin API'si değişiklikleri `503 cluster_unavailable` ile reddeder.
- Yeni giriş ve yeni proxy session'ı store'a ihtiyaç duyar. Store olmadan `503` ile biter.
- Kısa periyotlu rate limit node'lar arasında bölünür, bu yüzden tek node'a giden bir client limitin tamamını alamaz. Bakınız [Rate limit doğruluğu](@/cluster.tr.md#rate-limit-dogrulugu).
- Response cache'i, `share_cache = true` yazmadığınız sürece yereldir. Bakınız [Paylaşılan cache](@/cluster.tr.md#paylasilan-cache).

[Hata durumları](@/cluster.tr.md#hata-durumlari) her durumu ayrıntısıyla anlatır.
