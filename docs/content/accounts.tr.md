+++
title = "Hesaplar"
description = "Panel hesaplarının rolleri ve proxy listeleri"
weight = 0
+++

# Hesaplar

Her panel hesabının bir rolü vardır. Editör ve izleyici hesaplarının bir proxy listesi de olabilir. Yönetim paneli, yönetim API'si ve proxy'lerin [Panel Oturumu](@/configuration.tr.md#panel-oturumu) kimlik doğrulaması aynı hesapları kullanır.

## Roller

| İşlem | Admin | Editör | Proxy listesi olan editör | İzleyici |
|---|---|---|---|---|
| Proxy'leri okuma | bütün proxy'ler | bütün proxy'ler | listesindeki proxy'ler | bütün proxy'ler veya listesindeki proxy'ler |
| Proxy ekleme | evet | evet | evet, yeni proxy listesine eklenir | hayır |
| Proxy'yi değiştirme, silme, cache'ini temizleme | evet | evet | listesindeki proxy'ler | hayır |
| Portları, sertifikaları, ACME kayıtlarını ve erişim listelerini okuma | evet | evet | evet | evet |
| Portları, sertifikaları, ACME kayıtlarını ve erişim listelerini değiştirme, sertifika indirme, CDN IP aralıklarını yenileme | evet | evet | hayır | hayır |
| Ayarları ve hesapları okuma veya değiştirme, denetim kaydını okuma | evet | hayır | hayır | hayır |
| Deploy platformunun uygulamalarını, hedeflerini, deployment'larını ve container log'larını okuma | evet | evet | hayır | evet, proxy listesi yoksa |
| Uygulama ekleme, değiştirme veya silme, ortam değişkenlerini okuma veya değiştirme, Git token'ını ayarlama veya silme, webhook secret'ını oluşturma veya silme, uygulamayı deploy etme, deployment'ı geri alma | evet | evet | hayır | hayır |
| Agent hedefi ekleme, kayıt token'ını oluşturma, hedefi silme | evet | hayır | hayır | hayır |
| Git sağlayıcı bağlantılarını ve genel adresi okuma, bir bağlantının repository'lerini ve branch'lerini listeleme, uygulamanın webhook'unu yeniden kurma | evet | evet | hayır | hayır |
| Git sağlayıcı bağlantısı ekleme, değiştirme, bağlama veya silme, GitHub App oluşturma veya kurma, genel adresi değiştirme | evet | hayır | hayır | hayır |

- Proxy listesi olmayan hesap bütün proxy'leri görür. Admin her zaman bütün proxy'leri görür. Bu yüzden admin hesabının proxy listesi olamaz.
- Hesabın listesinde olmayan proxy, bu hesap için yoktur. Yönetim API'si `404 id_not_found` döndürür.
- Rolün izin vermediği işlem `403 forbidden` alır.
- WebUI, rolün açamadığı sayfaları gizler.
- Proxy listesi olan editör, kendi proxy'leri için bir erişim listesi seçebilir. Erişim listesinin içeriğini yalnız erişim listelerini değiştirebilen hesap değiştirir.
- Bir hesap proxy'yi silince r3v3rs3 proxy'yi her hesabın proxy listesinden kaldırır.
- Proxy listesi olan hesap, deploy platformuna gönderdiği her istek için `403 forbidden` alır.
- Yönetim API'si secret ortam değişkeninin değerini hiçbir zaman döndürmez.
- Uygulama değiştirebilen hesap, bir [Compose uygulamasının](@/platform.tr.md#compose-kaynagi) Compose dosyası üzerinden host'u ele geçirebilir, çünkü r3v3rs3 Compose dosyasının ayarlarını kısıtlamaz. Bir [agent hedefinde](@/platform.tr.md#agent-hedefleri) bu host, agent'ın host'udur.

## Kurallar

- Parola en az 8 karakter olmalıdır.
- Kullanıcı adı 1 ile 64 karakter arasındadır. `:`, `/`, boşluk veya kontrol karakteri içeremez.
- En az bir admin hesabı kalır. Son admin'i kaldıran değişiklik `400 last_admin` alır.
- Bir hesap kendini silemez ve kendi rolünü değiştiremez. Yönetim API'si `400 cannot_change_own_account` döndürür.
- Rol, proxy listesi veya parola değişince değişiklikten önce açılan oturumlar sona erer. Bu kural yönetim paneli oturumlarına ve proxy'lerin Panel Oturumu girişlerine uygulanır.
- Silinen hesabın oturumları sona erer.

Config dizinindeki `accounts.toml` dosyası hesapları parola hash'leriyle tutar. r3v3rs3 bu dosyayı `0600` moduyla yazar. Cluster, hesapları veri deposunda şifreli tutar.

## Hesap oluşturma

WebUI'daki "Hesaplar" sayfası hesapları listeler, ekler, değiştirir ve siler. Bu sayfayı yalnız admin açar. TOTP'si açık bir hesap eklediğinizde sayfa TOTP secret'ını bir kez gösterir. Secret'ı o anda doğrulama uygulamanıza ekleyin.

Komut satırında `add-user` bir hesap ekler. `--role` verilmezse hesap admin olur:

```bash
$ r3v3rs3 add-user alice --role editor
$ r3v3rs3 add-user bob --role viewer --totp
```

`--role` şu değerleri kabul eder: `admin`, `editor` veya `viewer`. `--password` verilmezse komut parolayı sorar. Komut satırından eklenen hesabın proxy listesi olmaz. Proxy listesini "Hesaplar" sayfasında veya yönetim API'si ile ayarlayın.

## Yönetim API'si

Bu endpoint'leri yalnız admin hesabı çağırabilir.

| Endpoint | İşlem |
|---|---|
| `GET /api/accounts` | Hesapları rol, proxy listesi ve TOTP durumuyla listeler. |
| `POST /api/accounts` | Bir hesap ekler. TOTP açıksa yanıt `totp_secret` alanını taşır. |
| `PUT /api/accounts/{username}` | Rolü, proxy listesini ve isteğe bağlı olarak parolayı değiştirir. |
| `DELETE /api/accounts/{username}` | Hesabı siler. |

```bash
$ curl -b cookies.txt -H 'Content-Type: application/json' \
    -d '{"username":"alice","password":"correct horse","role":"editor","proxies":["a1b2c3d"],"totp":false}' \
    http://localhost:46492/api/accounts
$ curl -b cookies.txt -X PUT -H 'Content-Type: application/json' \
    -d '{"role":"viewer"}' \
    http://localhost:46492/api/accounts/alice
```

`proxies` alanı olmayan güncelleme proxy listesini kaldırır. `password` alanı olmayan güncelleme parolayı korur.

| Yanıt | Neden |
|---|---|
| `400 password_too_short` | Parola 8 karakterden kısadır. |
| `400 invalid_username` | Kullanıcı adı kurallara uymuyor. |
| `400 invalid_account_scope` | Admin hesabına proxy listesi verilmiştir. |
| `409 account_exists` | Bu kullanıcı adına sahip bir hesap var. |
| `404 account_not_found` | Bu kullanıcı adına sahip hesap yok. |

`GET /api/session`, giriş yapmış hesabın kullanıcı adını, rolünü ve proxy listesini döndürür.

[Denetim kaydı](@/configuration.tr.md#denetim-kaydi) her hesap değişikliğini ve her girişi kaydeder.
