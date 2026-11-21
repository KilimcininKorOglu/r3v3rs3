+++
title = "Sunucu güvenlik duvarı"
description = "r3v3rs3'ün sunucuda tuttuğu nftables tablosu"
weight = 8
+++

## Sunucu güvenlik duvarı

`[firewall] enabled = true` (veya `R3V3RS3_FIREWALL_ENABLED=true`) ayarıyla r3v3rs3, sunucuda `inet r3v3rs3` adlı tek bir nftables tablosu tutar. Ayar varsayılan olarak kapalıdır. Tablo IPv4 ve IPv6'yı kapsar ve şunlar dışındaki bütün gelen bağlantıları düşürür:

- host'un açtığı bağlantıların yanıtları, loopback arayüzü, ICMP ve ICMPv6;
- `/etc/ssh/sshd_config` ve `sshd_config.d/*.conf` dosyalarındaki SSH portları ve her zaman port 22;
- `127.0.0.1` veya `::1` dışındaki bir adreste dinleyen her port (TCP; UDP ve QUIC portları için UDP);
- bu sunucuda çalışan uygulamaların `host_ports` alanı (platform sayfasına bakın);
- `["udp/51820", "tcp/8000-8010"]` gibi `extra_ports` değerleri.

```toml
[firewall]
enabled = true
extra_ports = ["udp/51820"]
```

Ayarlar sayfası, MCP aracı `update_firewall` ve `r3 security firewall update --enabled true --extra-ports udp/51820` komutu aynı iki ayarı değiştirir.

Bir port, bir uygulama veya ayar her değiştiğinde r3v3rs3 tabloyu tek bir `nft -f` işlemiyle değiştirir. Bu yüzden yarım yazılmış bir tablo oluşmaz. Ayar kapatılınca tablo silinir. Tablo yalnız `input` hook'unu filtreler. Docker'ın NAT ve forward kuralları değişmez. Agent hedefleri de kendi host'larında aynı tabloyu, kendi uygulamalarının `host_ports` değerleriyle tutar.

- Sunucunun `CAP_NET_ADMIN` yetkisine ihtiyacı vardır. `install.sh` dosyasının yazdığı unit bu yetkiyi verir.
- Trafiği düşüren tek güvenlik duvarı r3v3rs3 olmalıdır. nftables her tablonun input zincirini çalıştırır. Bu yüzden firewalld, ufw veya `iptables-persistent` tablosundaki bir drop kuralı, bu tablonun açtığı bir portu yine engeller. Önce diğer aracı durdurun (`systemctl disable --now firewalld`, `ufw disable` veya `systemctl disable --now netfilter-persistent`). Ayarlar sayfasındaki **Sunucu Güvenlik Duvarı** kartı, `GET /api/firewall`, MCP aracı `get_firewall_status` ve `r3 security firewall get-status` böyle her tabloyu bir uyarı olarak listeler. Port listesi de **Güvenlik duvarı açık, başka bir güvenlik duvarı var** gösterir. Tablo açıkken r3v3rs3 bu uyarıları dakikada bir yeniden okur. Bu yüzden diğer aracı durdurduğunuzda uyarı en geç bir dakika içinde kalkar. ufw, yalnızca `/etc/ufw/ufw.conf` içinde `ENABLED=yes` olduğunda uyarı verir.
- Docker ile port yayınlayan bir container'a (`-p 8080:80`) forward hook'u üzerinden ulaşılır. Bu yüzden bu tablo onu engellemez.

Önceki güvenlik duvarını durdurmadan önce, onun input zinciri dışındaki kurallarını okuyun (`iptables-save` ve `nft list ruleset`): `OUTPUT`, `FORWARD`, `nat` ve `raw` tabloları. Bir bulut image'ı veya bir VPN bu kurallara bağlı olabilir. Örnekler, sağlayıcının metadata servisi için bir output kuralı veya bir VPN'in NAT kuralıdır. Aracı durdurmak bu kuralları da siler ve r3v3rs3 bunları yeniden yazmaz. Bu kuralları iki yoldan biriyle koruyun:

- Aracı çalışır bırakın, ama kayıtlı kurallarından (örneğin `iptables-persistent` için `/etc/iptables/rules.v4`) `INPUT` kurallarını çıkarın. Böylece gelen trafiği yalnız r3v3rs3 filtreler.
- Bu kuralları kendinize ait ayrı bir nftables tablosuna taşıyın. `FORWARD` reject kuralını taşımayın. Kendi tablosunda bu kural, Docker'ın yayınladığı container'ları engeller.

Host kurallarını `nftables.service` ile yüklüyorsa, varsayılan unit `nft flush ruleset` ile durur. Bu komut r3v3rs3 tablosunu ve Docker tablolarını da siler. Unit'in `ExecStop` satırını yalnız kendi tablonuzu silecek şekilde değiştirin, örneğin `nft delete table inet custom`.

Geri dönüş: `enabled = false` yapın veya host'ta `nft delete table inet r3v3rs3` çalıştırın. Sonra önceki güvenlik duvarını yeniden başlatın. Önce onun kurallarını kaydedin (`nft list ruleset > /root/ruleset.nft`) ve `nft -f /root/ruleset.nft` ile geri yükleyin.
