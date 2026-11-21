+++
title = "Portlar"
description = "Dinlenen portlar, TLS, PROXY protokolü, port aralıkları ve şeffaf mod"
weight = 2
+++

Proxy eklemeden önce dinlenecek bir port bağlamanız gerekir. Bunu "Portlar" bölümünden yapabilirsiniz.

r3v3rs3 altı port türünü destekler:

- HTTP
- HTTPS (TLS üzerinden HTTP)
- QUIC üzerinden HTTP (HTTP/3)
- TCP
- TLS üzerinden TCP
- UDP

Portun `listen` adresi `/ip4/<adres>` veya `/ip6/<adres>` ile başlar. Ardından `/tcp/<port>` (TCP), `/tcp/<port>/tls` (TLS üzerinden TCP), `/tcp/<port>/http` (HTTP), `/tcp/<port>/https` (HTTPS), `/udp/<port>` (UDP) veya `/udp/<port>/quic/http` (QUIC üzerinden HTTP) gelir. TCP veya UDP portu tek bir port yerine `/udp/49160-49240` gibi bir aralık alabilir ("Port aralıkları" bölümüne bakın). `name` portun isteğe bağlı adıdır. Bir portu veya proxy'yi silmeden durdurmak için "Aktif" seçeneğini kapatın (`active = false`).

## Sunucu adları

HTTPS, QUIC üzerinden HTTP ve TLS üzerinden TCP portlarında "Sunucu Adları" alanı bulunur. r3v3rs3, sunucu sertifikasını istemcinin SNI (Server Name Indication) değerine göre seçer. İstemci SNI göndermediğinde r3v3rs3, bütün sunucu adlarını taşıyan geçerli bir sunucu sertifikası seçer. Liste boşsa ilk geçerli sunucu sertifikasını seçer.

```toml
[my-port]
listen = "/ip4/0.0.0.0/tcp/443/https"
tls_termination = { server_names = ["example.com", "*.example.com"] }
```

## TLS istemci kimlik doğrulaması

HTTPS, QUIC üzerinden HTTP ve TLS üzerinden TCP portları istemcinin sertifikasını doğrulayabilir (mutual TLS). Modu "İstemci Kimlik Doğrulaması" alanından seçin:

| Mod          | Davranış                                                                                                               |
|--------------|------------------------------------------------------------------------------------------------------------------------|
| Kapalı       | Port istemci sertifikası istemez. Varsayılan mod budur.                                                                |
| İsteğe bağlı | Port sertifika ister, fakat sertifikasız istemcileri de kabul eder. İstemcinin gönderdiği sertifika geçerli olmalıdır. |
| Zorunlu      | İstemci geçerli bir sertifika göndermezse TLS handshake başarısız olur.                                                |

İsteğe bağlı ve Zorunlu modlarda "İstemci CA Sertifikaları" listesinden bir veya daha fazla kök sertifika seçin. İstemci sertifikası bunlardan biriyle imzalanmış olmalıdır. Sistemin kök sertifikaları kullanılmaz. r3v3rs3, kök sertifika seçilmemiş veya kök olmayan bir sertifika seçilmiş port ayarını reddeder. Bir portun kullandığı kök sertifika silinemez.

İstemci kimlik doğrulaması ayarı geçersiz hale gelirse (örneğin kök sertifika yapılandırma dizininden silinirse) port bütün bağlantıları kapatır ve port listesi "TLS Hatası" gösterir.

HTTPS ve QUIC üzerinden HTTP portlarında header kuralları, doğrulanan sertifikayı `{client_cert_subject}` ve `{client_cert_fingerprint}` değişkenleriyle upstream sunucuya gönderebilir. "Header kuralları" bölümüne bakın.

```toml
[my-port]
listen = "/ip4/0.0.0.0/tcp/443/https"
tls_termination = { server_names = ["example.com"], client_auth = "required", client_ca_certs = ["a1b2c3d"] }
```

### Route Başına İstemci Sertifikası Zorunluluğu

Bir HTTPS veya QUIC üzerinden HTTP portu `Optional` istemci doğrulaması kullandığında, route bazında `require_client_cert = true` ile istemci sertifikası zorunlu kılınabilir. `require_client_cert = true` olarak yapılandırılmış bir route'a doğrulanmış bir istemci sertifikası olmadan gelen istekler doğrudan `403 Forbidden` ile reddedilir. Böylece aynı port üzerinde hem genel erişime açık route'lar (`/`, `/docs`) hem de mTLS ile kilitlenmiş özel route'lar (`/admin`, `/internal`) birlikte sunulabilir:

```toml
[my-app]
protocol = "http"
vhosts = ["app.example.com"]
routes = [
  { path = "/public", servers = [{ url = "http://127.0.0.1:8080/public" }] },
  { path = "/admin", servers = [{ url = "http://127.0.0.1:8080/admin" }], require_client_cert = true },
]
```

### TLS 1.3 0-RTT Erken Veri & Anti-Replay Koruması

Mobil ve yüksek gecikmeli istemcilerde bağlantı gecikmesini en aza indirmek için r3v3rs3, TLS termination portlarında TLS 1.3 0-RTT Early Data desteği sunar (`early_data = true`). Etkinleştirildiğinde, geçerli bir oturum bileti (session resumption ticket) taşıyan istemciler ilk el sıkışma paketinde (ClientHello) veri gönderebilir ve el sıkışma gecikmesi sıfır tura (0-RTT) iner:

- **RFC 8470 Yalnızca Güvenli Metodlar**: RFC 8470 standardına uygun olarak yalnızca güvenli HTTP metodlarına (`GET`, `HEAD`, `OPTIONS`, `TRACE`) early data içinde izin verilir. Early data ile gönderilen durum değiştiren istekler (`POST`, `PUT`, `DELETE`, `PATCH`), replay saldırılarına karşı `425 Too Early` durum kodu ve `Retry-After: 0` başlığıyla reddedilir.
- **Bellek İçi Anti-Replay Arabelleği**: 60 saniyelik kayan zaman penceresinde 0-RTT istek özetleri takip edilir. Tekrar yakalanan paketler anında `425 Too Early` ile durdurulur.
- **Upstream İletimi**: 0-RTT üzerinden gelen isteklere `Early-Data: 1` header'ı eklenerek upstream sunucuya iletilir. İstemcinin standart bir bağlantıda gönderdiği `Early-Data` header'ı upstream'e değişmeden ulaşır. Çünkü RFC 8470 Bölüm 5.1, aracının bu header'ı silmesini yasaklar.
- **0-RTT Bağlantısında Tek İstek**: Early data taşıyan bağlantı ilk yanıttan sonra kapanır. Sonraki istek yeni bir bağlantı açar. Böylece yalnız early data içinde gelen istekler early sayılır.

```toml
[my-port]
listen = "/ip4/0.0.0.0/tcp/443/https"
tls_termination = { server_names = ["example.com"], early_data = true }
```

API (`{"enabled": true}` gövdesiyle `PUT /api/ports/{id}/early_data`), CLI veya MCP (`update_port_early_data`) üzerinden. TLS sonlandırması olmayan bir portta sunucu early data'yı açmayı reddeder:
```bash
r3 port update-early-data <PORT_ID> --enable true
r3 port update-early-data <PORT_ID> --enable false
```

## PROXY protocol

r3v3rs3 önündeki bir yük dengeleyici, istemci adresini PROXY protocol header'ı (sürüm 1 veya 2) ile gönderebilir. TCP, TLS üzerinden TCP, HTTP ve HTTPS portları bu header'ı okuyabilir. UDP ve QUIC üzerinden HTTP portları okuyamaz.

"PROXY Protocol Kabul Et" seçeneğini açın. Yük dengeleyicilerin IP adreslerini veya CIDR bloklarını "Güvenilen Yük Dengeleyiciler" alanına yazın:

- Güvenilen bir adresten gelen bağlantı geçerli bir header ile başlamalıdır. Header geçersizse, sürümü "Kabul Edilen Sürümler" içinde yoksa veya header "Header Timeout (Saniye)" alanındaki sürede gelmezse r3v3rs3 bağlantıyı kapatır. Varsayılan timeout 5 saniyedir.
- r3v3rs3 diğer adreslerden header okumaz. Bu bağlantıların istemci adresi peer adresidir.
- Sürüm 2 `LOCAL` header'ı ve sürüm 1 `UNKNOWN` header'ı peer adresini korur. Yük dengeleyiciler bunları sağlık kontrollerinde gönderir.

r3v3rs3 header'ı TLS handshake'ten önce okur. Header'daki adres bağlantının istemci adresi olur. IP filtreleri, rate limit'ler, istemci IP hash'i, `Forwarded` ve `X-Forwarded-For` header'ları ve access log bu adresi kullanır. Access log peer adresini de `peer` alanına yazar.

PROXY protocol header'ı ile başlayan bir ACME HTTP-01 challenge isteği challenge yanıtını almaz. Böyle bir yük dengeleyicinin sunduğu sertifika için DNS-01 challenge kullanın.

```toml
[my-port]
listen = "/ip4/0.0.0.0/tcp/443/https"
tls_termination = { server_names = ["example.com"] }
proxy_protocol = { trusted = ["10.0.0.0/8"], accept = "v2", timeout = "5s" }
```

`accept` değeri `any` (varsayılan), `v1` veya `v2` olur.

## Port aralıkları

TCP veya UDP portu bir port aralığında dinleyebilir. Örneğin bir TURN sunucusunun relay portları için `/ip4/0.0.0.0/udp/49160-49240` kullanılır. Bir aralık en fazla 16384 port içerir. TLS üzerinden TCP, HTTP, HTTPS ve QUIC üzerinden HTTP portları yalnız tek port alır.

- Aralıktaki her port, upstream aralığında aynı sıradaki porta gider. Aralık `49160-49240` ve upstream `/ip4/10.0.0.5/udp/50000-50080` ise 49165 portuna gelen paket 50005 portuna gider. Proxy'nin her upstream sunucusu aynı uzunlukta bir aralık içermelidir. r3v3rs3 tek bir upstream portunu veya farklı uzunluktaki bir aralığı reddeder.
- Bir aralık, aynı protokoldeki başka bir TCP veya UDP portunun ya da SSH sunucusunun dinlediği portu içeremez.
- r3v3rs3 ilk portta tek bir socket açar. Ardından bir nftables tablosu (`inet r3v3rs3_tproxy`) ve bir policy route (firewall işareti `0x7233`, routing tablosu 7233) kurar. Bu kurallar bütün aralığı TPROXY ile bu socket'e gönderir. UDP yanıtı, istemcinin gönderdiği porttan çıkar.
- Tablo ve route `CAP_NET_ADMIN` yetkisi ister. `install.sh` bu yetkiyi `r3-server` unit'ine verir. Son aralık veya şeffaf port silinince r3v3rs3 tabloyu ve route'u kaldırır.
- TPROXY kuralı Docker port yönlendirmesinden önce çalışır. Bu yüzden aralık, içindeki yayınlanmış bir container portunun paketlerini de alır.
- Sunucu güvenlik duvarı tablosu açıksa bütün aralığı açar.

```toml
[turn-relay]
listen = "/ip4/0.0.0.0/udp/49160-49240"
```

## Şeffaf mod

Upstream sunucusu varsayılan olarak her bağlantının kaynağını r3v3rs3'ün adresi olarak görür. Upstream istemcinin adresini görmeliyse TCP, TLS üzerinden TCP veya UDP portunda "Şeffaf Mod" seçeneğini açın (`transparent = true`). Örneğin bir TURN sunucusu relay adreslerini istemcinin adresinden oluşturur. Bu seçenek tek portlu portlarda da çalışır. HTTP, HTTPS ve QUIC üzerinden HTTP portları bu seçeneği reddeder. Bu portlar istemcinin adresini `Forwarded` ve `X-Forwarded-For` header'larıyla iletir.

- r3v3rs3 her upstream bağlantısını istemcinin adresinden açar ve işaretler (firewall işareti `0x7234`). `inet r3v3rs3_tproxy` tablosu ve policy route yanıtları r3v3rs3'e geri getirir.
- Upstream yanıtlarını bu sunucu üzerinden göndermelidir. Upstream bu sunucuda çalışır (bir container veya host ağındaki bir servis) ya da istemcilere dönüş yolu olarak bu sunucuyu kullanır.
- Loopback adresindeki (`127.0.0.1`, `::1`) bir upstream çalışmaz. Linux, kaynağı başka bir adres olan paketi loopback adresine göndermez. r3v3rs3 şeffaf bir portta böyle bir upstream'i reddeder. Upstream'i sunucunun başka bir adresine bağlayın.
- İstemci ve upstream sunucusu aynı adres ailesinde (IPv4 veya IPv6) olmalıdır.

```toml
[turn-relay]
listen = "/ip4/0.0.0.0/udp/49160-49240"
transparent = true
```

API (`{"enabled": true}` gövdesiyle `PUT /api/ports/{id}/transparent`), CLI veya MCP (`update_port_transparent`) üzerinden:
```bash
r3 port update-transparent <PORT_ID> --enable true
r3 port update-transparent <PORT_ID> --enable false
```

r3v3rs3 kaldırıldıktan sonra ağ kurallarını elle silmek için:
```bash
nft delete table inet r3v3rs3_tproxy
ip rule del fwmark 0x7233 lookup 7233
ip route flush table 7233
ip -6 rule del fwmark 0x7233 lookup 7233
ip -6 route flush table 7233
```

## Portu sıfırlama

Port ayarını değiştirdiğinizde açık bağlantılar etkilenmez; bu bağlantılar eski ayarla çalışmaya devam eder. Açık bağlantıları kapatmak için portu sıfırlayın.
