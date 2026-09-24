+++
title = "Bir uygulamayı korumaya alma"
description = "IP filtresi, kimlik doğrulama, rate limit ve denetim kaydı tek bir kurulumda"
weight = 7
+++

# Bir uygulamayı korumaya alma

Bu rehber iç ağdaki bir uygulamayı r3v3rs3 arkasına alır ve yalnız erişmesi gerekenleri içeri alır. Dört mekanizmayı birlikte kullanır:

| Mekanizma | Geçemeyen istemciye verilen yanıt |
|---|---|
| IP filtresi | `403 Forbidden` |
| Rate limit | `Retry-After` header'ı ile `429 Too Many Requests` |
| Kimlik doğrulama | `401 Unauthorized` |
| Denetim kaydı | Yapılandırmayı kimin değiştirdiğini kaydeder |

r3v3rs3 bunları şu sırayla uygular: IP filtresi, rate limit, HTTPS yönlendirmesi, yönlendirme kuralları, kimlik doğrulama. Engellenen bir adres parola kontrolüne hiç gelmez, limiti aşan bir istemci upstream sunucuya hiç ulaşmaz.

Önce [Başlangıç](@/tutorials/getting-started.tr.md) rehberini izleyin. Bu rehber çalışan bir port ve çalışan bir uygulama ile devam eder.

## Adım 1: Erişim listesi oluşturun

Erişim listesi bir IP filtresini ve bir kimlik doğrulamayı tek bir adın altında tutar. Birkaç proxy ve route aynı listeyi kullanır, tek bir değişiklik hepsine uygulanır.

1. Menüde **Erişim Listeleri** linkine, sonra **Ekle** butonuna tıklayın.
2. Ad alanına `Office` yazın.
3. **İzin Verilen IP Adresleri** alanına ofis ağınızı yazın, örneğin `203.0.113.0/24`. Proxy'ye yalnız bu istemciler ulaşır. Bütün adreslere izin vermek için alanı boş bırakın.
4. Kimlik doğrulama bölümünde **Basic Auth** seçin.
5. **Realm** alanına `Staff` yazın. Tarayıcı bu adı giriş kutusunda gösterir.
6. **Kullanıcı Ekle** butonuna tıklayın, bir kullanıcı adı ve parola yazın.
7. Kaydedin.

r3v3rs3 her parolayı argon2 hash olarak saklar. `access_lists.toml` dosyası listeyi `0600` izinleriyle tutar, yönetim API'si hash yerine `password_set: true` döndürür.

Proxy'ye ulaşmaması gereken tek bir adres için **Engellenen IP Adresleri** alanını kullanın. Engellenen bir adres, izin verilen adresin önüne geçer.

## Adım 2: Listeyi proxy'ye bağlayın

1. Proxy'yi açın ve **Erişim Listesi** alanını bulun.
2. `Office` listesini seçin.
3. Kaydedin.

Liste, proxy'nin **IP Filtresi** ve **Kimlik Doğrulama** ayarlarının yerini alır. Bir proxy ikisini birden tutamaz, yönetim API'si bu durumda `400 access_list_conflict` döndürür.

Sonucu kontrol edin:

```bash
$ curl -i https://app.example.com/
HTTP/2 401
www-authenticate: Basic realm="Staff", charset="UTF-8"

$ curl -i -u alice:<parola> https://app.example.com/
HTTP/2 200
```

İzin verilen adreslerin dışındaki bir istemci `403 Forbidden` alır ve giriş kutusunu hiç görmez.

## Adım 3: Rate limit ekleyin

Rate limit proxy üzerinde kalır, çünkü erişim listesi limit tutmaz.

1. Proxy'yi açın ve **Rate Limit** bölümünü bulun.
2. **İstek Sayısı** alanına `60` yazın, **Süre** alanında `dakika` seçin.
3. **Burst** alanına `10` yazın, böylece on dosyayı aynı anda yükleyen bir sayfa geçer.
4. Kaydedin.

```bash
$ curl -i -u alice:<parola> https://app.example.com/
HTTP/2 429
retry-after: 14
```

- r3v3rs3 her istemci IP adresini ayrı sayar.
- Limit parola kontrolünü de korur: argon2 bilerek CPU zamanı harcar, limit parola denemesini yavaşlatır.
- Sayaçlar bellekte durur. Yeniden başlatma onları sıfırlar. Cluster'da düğümler sayıları paylaşır, bkz. [Rate limit doğruluğu](@/cluster.tr.md#rate-limit-dogrulugu).

Bir route kendi limitini tutabilir. Uygulamanızın giriş yoluna daha dar bir limit verin:

1. Route'u açın ve **Bu Route için Ayrı Rate Limit Kullan** seçeneğini açın.
2. **İstek Sayısı** alanına `5` yazın ve `dakika` seçin.

## Adım 4: Kimlik doğrulama yöntemini seçin

Basic Auth başka bir servis istemez, ama tarayıcı kutusu gösterir ve çıkış yapma yolu yoktur. Diğer üç yöntem başka durumlara uyar:

| Yöntem | Ne zaman kullanılır |
|---|---|
| **Basic Auth** | Birkaç kişi tek bir parolayı paylaşır ve tarayıcı kutusu yeter. |
| **Bearer Token** | Bir script veya CI işi bir API'yi çağırır. Token'ı `openssl rand -hex 32` ile oluşturun. |
| **Panel Oturumu** | Uygulamanızın kendi girişi yoktur ve kişilerin zaten r3v3rs3 panel hesabı vardır. |
| **Forward Auth** | Kararı dış bir servis verir, örneğin bir kimlik sağlayıcısı ile oauth2-proxy veya Authelia. |

**Panel Oturumu**, route yolunun altındaki `/.r3v3rs3/auth/login` adresinde bir giriş sayfası sunar. Yalnız proxy'yi gören bir hesabı kabul eder ve TOTP'si olan bir hesaptan TOTP kodunu ister. Uygulamanıza bir çıkış butonu ekleyin:

```html
<form method="post" action="/.r3v3rs3/auth/logout"><button>Çıkış Yap</button></form>
```

Her yöntemi alanlarıyla ve yanıtlarıyla [Kimlik doğrulama](@/configuration.tr.md#kimlik-dogrulama) bölümü anlatır.

## Adım 5: Bir yolu herkese açın

Bir sağlık kontrolü veya bir webhook kimlik bilgisi istemez. Ona kendi route'unu verin:

1. Proxy'yi açın ve `/healthz` yolu ile bir route ekleyin.
2. **Bu Route için Ayrı Kimlik Doğrulama Kullan** seçeneğini açın ve **Yok** seçin.
3. **Bu Route için Ayrı IP Filtresi Kullan** seçeneğini açın ve iki listeyi de boş bırakın. Route böylece bütün istemcilere açılır.

Route'ların sırası önemli değildir. r3v3rs3 her isteği yolu en uzun eşleşen route'a gönderir, bu yüzden o yolda `/healthz` route'u `/` route'unun önüne geçer.

## Adım 6: Gerçek istemci IP adresini alın

CDN veya yük dengeleyici arkasında TCP bağlantısının karşı ucu edge sunucudur. Doğru ayar olmadan IP filtresi ve rate limit bütün ziyaretçiler için tek bir adres görür.

- r3v3rs3 sekiz bilinen CDN'in IP aralıklarına varsayılan olarak güvenir, aralarında Cloudflare, Fastly ve Amazon CloudFront vardır. Sağlayıcının header'ını okur, örneğin `CF-Connecting-IP`.
- Kendi yük dengeleyiciniz için adresini proxy'nin **Güvenilen Proxy'ler** alanına yazın.
- Güvenilmeyen bir karşı uç için r3v3rs3 `X-Forwarded-For`, `X-Real-IP` ve diğer istemci IP header'larını siler, çünkü istemci onları uydurabilir.

Çözümlenen adresi uygulamanızın erişim log'unda görün. r3v3rs3 adresi `X-Real-IP` header'ında gönderir. Header sırasını [İstemci IP adresi](@/configuration.tr.md#istemci-ip-adresi) bölümü anlatır.

## Adım 7: Denetim kaydını okuyun

Denetim kaydı, bir hesabın yaptığı her yapılandırma değişikliğini ve panele her girişini kaydeder. Ziyaretçilerinizin isteklerini kaydetmez.

1. Menüde **Denetim Kaydı** linkine tıklayın. Sayfayı yalnız admin hesabı açar.
2. Hesaba, kaynağa veya döneme göre filtreleyin.

```bash
$ curl -b session.txt 'http://127.0.0.1:46492/api/audit?limit=5'
[{"time":1789643174264,"username":"admin","client":"127.0.0.1","action":"update_access_list","resource_id":"tkx-xqy","summary":"Office"}]
```

Her kayıt zamanı, hesabı, istemci IP adresini, işlemi, kaynağın id'sini ve kısa bir özeti tutar. Özet hiçbir zaman parola, token veya key tutmaz. **Denetim Kaydı Saklama Süresi** ayarı bir kaydı varsayılan olarak bir yıl tutar.

## Bunlar neyi kapsamaz

- Parola kontrolü proxy'yi korur, uygulamayı değil. Uygulamaya başka bir porttan ulaşan bir istemci r3v3rs3'ü atlar. Uygulamayı `127.0.0.1` adresine veya bir iç ağa bağlayın.
- r3v3rs3 `Authorization` header'ını upstream sunucuya göndermeden önce siler, yani uygulama proxy'nin kimlik bilgilerini görmez.
- Rate limit istek sayar, byte saymaz. Yükleme limiti için **İstek Gövdesi Limiti** alanını kullanın. Limiti aşan bir istek, kimlik doğrulamadan önce `413 Payload Too Large` alır.

## Sonraki adımlar

- [Erişim listeleri](@/configuration.tr.md#erisim-listeleri): liste modeli ve hataları.
- [IP filtresi](@/configuration.tr.md#ip-filtresi) ve [Rate limit](@/configuration.tr.md#rate-limit): bütün alanlar.
- [Hesaplar](@/accounts.tr.md): roller, proxy listeleri ve TOTP hesapları.
