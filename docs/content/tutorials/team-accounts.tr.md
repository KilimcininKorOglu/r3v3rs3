+++
title = "Ekip için hesaplar"
description = "Herkese kendi proxy'lerini verin, TOTP açın ve kimin neyi değiştirdiğini okuyun"
weight = 14
+++

# Ekip için hesaplar

Beş kişinin paylaştığı tek bir admin parolası bir yetki modeli değildir. Bu rehber herkese ihtiyacı olan rolü veren bir hesap açar, bir hesabı kendi proxy'leriyle sınırlar, TOTP açar, aynı hesaplarla bir uygulamayı korur ve kimin neyi değiştirdiğini gösterir.

[Başlangıç](@/tutorials/getting-started.tr.md) rehberindeki bir admin hesabı gerekir.

## Adım 1: Rolü seçin

Üç rol vardır. Proxy listesi, editor ve viewer rolünü daha da daraltır:

| İşlem | Admin | Editor | Proxy listesi olan editor | Viewer |
|---|---|---|---|---|
| Proxy'leri okumak | her proxy | her proxy | listesindeki proxy'ler | her proxy veya listesindekiler |
| Proxy eklemek | evet | evet | evet, yeni proxy listesine girer | hayır |
| Proxy değiştirmek, silmek, cache'ini temizlemek | evet | evet | listesindeki proxy'ler | hayır |
| Port, sertifika, ACME kaydı ve erişim listesi okumak | evet | evet | evet | evet |
| Port, sertifika, ACME kaydı ve erişim listesi değiştirmek | evet | evet | hayır | hayır |
| Ayarları ve hesapları okumak veya değiştirmek, audit log okumak | evet | hayır | hayır | hayır |

Admin her zaman bütün proxy'leri görür, bu yüzden admin hesabının proxy listesi olamaz. Listesi olmayan hesap her proxy'yi görür.

## Adım 2: Hesap oluşturun

WebUI'da **Hesaplar** sayfasını açın. Sayfayı yalnız admin görür. Komut satırında:

```bash
$ r3v3rs3 add-user alice --role editor
$ r3v3rs3 add-user bob --role viewer --totp
```

`--role` verilmezse hesap admin olur. `--password` verilmezse komut parolayı sorar. Komut satırından açılan hesabın proxy listesi olmaz; listeyi **Hesaplar** sayfasından veya API'den verin.

API'de tek çağrı hesabı listesiyle birlikte oluşturur:

```bash
$ curl -s -b cookies.txt -X POST http://localhost:46492/api/accounts \
    -H 'Content-Type: application/json' \
    -d '{"username":"alice","password":"alice-uzun-parola","role":"editor","proxies":["pdb-khh"],"totp":false}'
{}
```

Parola en az 8 karakter olmalıdır. Kullanıcı adı 1 ile 64 karakter arasındadır ve `:`, `/`, boşluk veya kontrol karakteri içeremez.

## Adım 3: Proxy listesi ne yapar

Alice giriş yapar ve yalnız kendi proxy'sini görür:

```bash
$ curl -s -b alice.txt http://localhost:46492/api/proxies | jq -r '.[].id'
pdb-khh
$ curl -s -b alice.txt http://localhost:46492/api/session
{"username":"alice","role":"editor","proxies":["pdb-khh"],"cert_expiry_warning":"14days"}
```

Listesinde olmayan proxy onun için yoktur:

```bash
$ curl -s -b alice.txt http://localhost:46492/api/proxies/cfr-kpm
{"message":"port id not found: cfr-kpm","error":{"message":"id_not_found","id":"cfr-kpm"}}
```

Bu 404'tür, 403 değil. Proxy listesi olan editor portları da değiştiremez:

```bash
$ curl -s -o /dev/null -w '%{http_code}\n' -b alice.txt -X POST http://localhost:46492/api/ports \
    -H 'Content-Type: application/json' -d '{"name":"x","listen":"/ip4/127.0.0.1/tcp/8479/http"}'
403
```

Kendi oluşturduğu proxy listesine kendiliğinden girer:

```bash
$ curl -s -b cookies.txt http://localhost:46492/api/accounts | jq -c '.[] | select(.username=="alice")'
{"username":"alice","role":"editor","proxies":["pdb-khh","tpq-gcv"],"totp":false}
```

Yani proxy listesi olan bir editor, başka bir ekibin proxy'lerini hiç görmeden kendi servisini kurar ve yönetir.

## Adım 4: TOTP açın

Hesabı `"totp": true` ile oluşturun veya sonradan açın. Secret yanıtta bir kez döner:

```bash
$ curl -s -b cookies.txt -X POST http://localhost:46492/api/accounts \
    -H 'Content-Type: application/json' \
    -d '{"username":"bob","password":"bob-uzun-parola","role":"viewer","totp":true}'
{"totp_secret":"V2J27IEPLYHJ3Y62XSZM4GJBQSFTFSRF"}
```

**Hesaplar** sayfası da secret'ı bir kez gösterir. Authenticator uygulamasına o anda ekleyin; r3v3rs3 secret'ı bir daha göstermez.

Giriş bundan sonra iki çağrıdır. İlki `totp_required` döner:

```bash
$ curl -s -c bob.txt -X POST http://localhost:46492/api/login \
    -H 'Content-Type: application/json' \
    -d '{"username":"bob","method":"password","password":"bob-uzun-parola"}'
"totp_required"
$ curl -s -b bob.txt -c bob.txt -X POST http://localhost:46492/api/login \
    -H 'Content-Type: application/json' \
    -d '{"username":"bob","method":"totp","token":"418205"}'
"success"
```

Yanlış kod, yanlış parola ile aynı yanıtı verir: 400 `invalid_login_credentials`. `max_login_attempts` (varsayılan 10) bir client IP adresi ve kullanıcı adı çiftini o kadar hatadan sonra engeller, `login_attempts_reset` (varsayılan 15 dakika) engeli kaldırır.

## Adım 5: Aynı hesaplarla bir uygulamayı koruyun

Kendi girişi olmayan bir uygulama bu hesapları kullanabilir. Proxy'nin kimlik doğrulamasını **Panel Session** yapın:

```bash
$ curl -s -b cookies.txt -X PUT http://localhost:46492/api/proxies/tpq-gcv \
    -H 'Content-Type: application/json' -d '{ ... , "auth": {"type":"session"} }'
```

Session'ı olmayan tarayıcı request'i yönlendirilir:

```
HTTP/1.1 302 Found
location: /.r3v3rs3/auth/login?redirect=%2F
```

Diğer method'lar 401 alır. Giriş formu `username`, `password`, `totp` ve `redirect` alanlarını alır:

```bash
$ curl -s -o /dev/null -w '%{http_code}\n' -c app.txt -X POST \
    http://app.example.com/.r3v3rs3/auth/login \
    -d 'username=alice&password=alice-uzun-parola&redirect=/'
303
```

Bundan sonra uygulama her zamanki gibi cevap verir. `POST /.r3v3rs3/auth/logout` session'ı bitirir ve sonraki request yeniden yönlendirilir.

**Giriş yapabilmek için hesabın o proxy'yi görmesi gerekir.** Proxy listesinde bu proxy olmayan bir hesap, parolası doğru olsa da giriş formunda 401 alır. İnsanları şaşırtan nokta budur: proxy listesi yalnız paneli değil, uygulama erişimini de belirler.

r3v3rs3 hesabı her request'te kontrol eder. Hesap silinince, hesap değişince veya proxy hesabın listesinden çıkınca session biter. Session'lar memory'dedir, yani restart bütün client'ları çıkarır.

## Adım 6: Hesabı değiştirin ve silin

Rol, proxy listesi veya parola değişikliği, değişiklikten önce başlayan session'ları bitirir:

```bash
$ curl -s -b cookies.txt -X PUT http://localhost:46492/api/accounts/alice \
    -H 'Content-Type: application/json' -d '{"role":"viewer","proxies":["pdb-khh"]}'
null
$ curl -s -o /dev/null -w '%{http_code}\n' -b alice.txt http://localhost:46492/api/proxies
401
```

Bu kural hem panel session'ları hem de Adım 5'teki uygulama girişleri için geçerlidir. Ekipten ayrılan kişi ikisini birden kaybeder.

İki kural kendinizi dışarıda bırakmanızı engeller:

```bash
$ curl -s -b cookies.txt -X DELETE http://localhost:46492/api/accounts/admin
{"message":"an account cannot delete itself or change its own role","error":{"message":"cannot_change_own_account"}}
```

- Hesap kendini silemez ve kendi rolünü değiştiremez.
- En az bir admin hesabı kalır. Son admin'i kaldıran değişiklik 400 `last_admin` döner.

`proxies` alanı olmayan bir update proxy listesini siler, `password` alanı olmayan bir update parolayı korur.

## Adım 7: Kimin neyi değiştirdiğini okuyun

Her değişiklik ve her giriş kaydedilir. Kaydı yalnız admin okur:

```bash
$ curl -s -b cookies.txt 'http://localhost:46492/api/audit?limit=3' | jq -c '.[]'
{"time":1789647512463,"username":"admin","client":"127.0.0.1","action":"add_proxy","resource_id":"jzr-pgf","summary":"api-demo-app"}
{"time":1789647489459,"username":"admin","client":"127.0.0.1","action":"login"}
```

WebUI'ın **Audit Log** sayfası kayıtları hesaba, kaynağa ve döneme göre filtreler ve en çok 500 kayıt gösterir. Summary; ad, adres ve rol taşır, hiçbir zaman parola, token veya key taşımaz. Varsayılan saklama süresi bir yıldır.

r3v3rs3'ün kendi yaptığı değişiklikler, örneğin sertifika yenileme veya discovery ile gelen proxy, kaydedilmez.

## Hesaplar nerede durur

Config dizinindeki `accounts.toml` hesapları parola hash'leriyle tutar ve `0600` mode ile yazılır. Cluster hesapları store'da şifreli tutar, böylece her node aynı hesapları paylaşır.

## Referans

- [Hesaplar](@/accounts.tr.md): bütün kurallar, hata kodları ve API.
- [Panel Session](@/configuration.tr.md#panel-session): endpoint'ler ve cookie.
- [Audit log](@/configuration.tr.md#audit-log): alanlar ve query parametreleri.

## Sonraki adımlar

- [Bir uygulamayı korumaya alma](@/tutorials/protect-an-app.tr.md): IP filtresi, basic auth ve rate limit.
- [r3v3rs3'ü script ile yönetme](@/tutorials/admin-api.tr.md): deploy işi için hesap.
