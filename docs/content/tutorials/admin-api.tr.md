+++
title = "r3v3rs3'ü script ile yönetme"
description = "Admin API'ye giriş yapın, port ve proxy oluşturun, CI'dan cache'i temizleyin"
weight = 12
+++

# r3v3rs3'ü script ile yönetme

WebUI'da, WebUI'ın kendisinin kullanmadığı hiçbir düğme yoktur. Yaptığı her şey `/api` altındaki admin API üzerinden gider, yani bir script de aynısını yapabilir. Bu rehber giriş yapar, port ve proxy oluşturur, status okur, deploy işinden cache temizler ve o işe başka hiçbir şey yapamayan bir hesap verir.

Örnekler `curl` ile `http://localhost:46492` adresine gider. Bu, admin panelinin varsayılan adresidir.

## Adım 1: Giriş yapın

`POST /api/login` bir session cookie'si döner:

```bash
$ curl -s -c cookies.txt \
    -H 'Content-Type: application/json' \
    -d '{"username":"admin","method":"password","password":"passw0rd","insecure":true}' \
    http://localhost:46492/api/login
"success"
```

Response cookie'yi taşır:

```
set-cookie: token=QWZ1uBQz5A9gAnmK2b18EZ7tD7CpkJ9I; HttpOnly; SameSite=Strict
```

`"insecure": true`, cookie'den `Secure` attribute'unu kaldırır, böylece cookie düz HTTP'de de çalışır. Admin paneli HTTPS arkasındaysa bu alanı yazmayın.

Diğer her endpoint bu cookie'yi ister:

```bash
$ curl -s -b cookies.txt http://localhost:46492/api/ports
[{"id":"dpv-ylj","name":"web","listen":"/ip4/127.0.0.1/tcp/8473/http"}]
$ curl -s -o /dev/null -w '%{http_code}\n' http://localhost:46492/api/ports
401
```

Yanlış parola 400 döner:

```json
{"message":"invalid login credentials","error":{"message":"invalid_login_credentials"}}
```

`GET /api/session` kim olduğunuzu söyler, `GET /api/logout` session'ı bitirir. Logout'tan sonra cookie işe yaramaz:

```bash
$ curl -s -b cookies.txt http://localhost:46492/api/session
{"username":"admin","role":"admin","cert_expiry_warning":"14days"}
```

Session'lar memory'dedir. r3v3rs3 yeniden başlayınca her session biter, bu yüzden uzun süre çalışan bir script 401 alınca yeniden giriş yapmalıdır.

## Adım 2: API dokümanını okuyun

r3v3rs3 OpenAPI dokümanını sunucu kodundan üretir, yani doküman her zaman çalışan sürümü anlatır:

- OpenAPI dokümanı: `http://localhost:46492/api/openapi.json`
- Swagger UI: `http://localhost:46492/api/docs/`

İkisi de session cookie'si ister. WebUI'a giriş yaptığınız tarayıcıda açın; WebUI'ın alt kısmındaki API bağlantısı Swagger UI'ı açar.

```bash
$ curl -s -b cookies.txt http://localhost:46492/api/openapi.json | jq '.info.version, (.paths | length)'
"1.0.1"
34
```

Dokümanı bir client üretmek için veya bir endpoint'in body'sini tahmin etmek yerine görmek için kullanın.

## Adım 3: Port oluşturun

Port bir ad ve bir multiaddr alır:

```bash
$ curl -s -b cookies.txt -X POST http://localhost:46492/api/ports \
    -H 'Content-Type: application/json' \
    -d '{"name":"api-demo","listen":"/ip4/127.0.0.1/tcp/8475/http"}'
null
```

`null` başarı body'sidir. Id'yi listeden okuyun:

```bash
$ curl -s -b cookies.txt http://localhost:46492/api/ports | jq -r '.[] | select(.name=="api-demo") | .id'
smc-gzh
```

Id'yi r3v3rs3 üretir. Elle id yazmayın: var olmayan bir id ile yapılan `PUT` kabul edilir ve proxy'lerinizin gösterdiği hiçbir şeyi oluşturmaz.

`PUT /api/ports/{id}` bir portu değiştirir, `DELETE /api/ports/{id}` siler, `GET /api/ports/{id}/status` socket'in dinleyip dinlemediğini söyler.

## Adım 4: Proxy oluşturun

Proxy; çalıştığı portları, protokolünü ve route'larını taşır:

```bash
$ curl -s -b cookies.txt -X POST http://localhost:46492/api/proxies \
    -H 'Content-Type: application/json' \
    -d '{
      "name": "api-demo-app",
      "ports": ["smc-gzh"],
      "protocol": "http",
      "vhosts": ["demo.example.com"],
      "routes": [{ "path": "/", "servers": [{ "url": "http://127.0.0.1:9000/" }] }]
    }'
null
```

Proxy restart olmadan, hemen cevap verir:

```bash
$ curl -s -o /dev/null -w '%{http_code}\n' -H 'Host: demo.example.com' http://127.0.0.1:8475/
200
```

`protocol` alanı proxy tipini seçer: `http`, `tcp` veya `udp`. TCP ve UDP proxy'de `routes` yerine `upstream_servers` bulunur.

`GET /api/proxies/{id}/status` upstream sunucuları gösterir:

```bash
$ curl -s -b cookies.txt http://localhost:46492/api/proxies/jzr-pgf/status
{"state":"active","upstreams":[{"addr":"http://127.0.0.1:9000/","weight":1,"healthy":true,"failures":0}]}
```

## Adım 5: Deploy sonrası cache'i temizleyin

Statik dosyaları değiştiren bir deploy, HTTP cache'inde eski body'ler bırakır. Tek bir çağrı bir proxy'nin cache'ini temizler:

```bash
$ curl -s -b cookies.txt -X DELETE http://localhost:46492/api/proxies/jzr-pgf/cache
null
```

Deploy işinin tamamı üç satırdır:

```bash
#!/bin/sh
set -eu
curl -sf -c "$PWD/j.txt" -H 'Content-Type: application/json' \
  -d "{\"username\":\"$R3_USER\",\"method\":\"password\",\"password\":\"$R3_PASS\"}" \
  "$R3_URL/api/login" > /dev/null
curl -sf -b "$PWD/j.txt" -X DELETE "$R3_URL/api/proxies/$R3_PROXY/cache" > /dev/null
curl -sf -b "$PWD/j.txt" "$R3_URL/api/logout" > /dev/null
```

Parolayı CI'ın secret store'unda tutun, depoda değil. `-f` option'ı 4xx ve 5xx status'te işi başarısız yapar.

## Adım 6: İşe kendi hesabını verin

Deploy işine admin parolasını vermeyin. İşin ihtiyacı olan role sahip bir hesap oluşturun:

```bash
$ curl -s -b cookies.txt -X POST http://localhost:46492/api/accounts \
    -H 'Content-Type: application/json' \
    -d '{"username":"ci","password":"uzun-bir-parola","role":"editor"}'
```

Üç rol vardır:

| Rol | Ne yapabilir |
|---|---|
| `admin` | Her şeyi: hesaplar, ayarlar ve audit log dahil. |
| `editor` | Kendi proxy listesindeki port ve proxy'leri değiştirir. |
| `viewer` | Yalnız okur. |

`viewer` rolü proxy listesini okur, ama hiçbir şeyi değiştiremez:

```bash
$ curl -s -b ci.txt -X DELETE http://localhost:46492/api/proxies/jzr-pgf/cache
{"message":"the role of the account does not allow this action","error":{"message":"forbidden"}}
```

Bu request 403 döner. Cache temizleme en az `editor` rolü ister. `editor` rolü audit log'u ve hesapları okuyamaz, ikisi de 403 döner. Hesap başına proxy listesi ve TOTP için [Hesaplar](@/accounts.tr.md) sayfasına bakın.

## Adım 7: Audit log'u okuyun

WebUI veya API üzerinden yapılan her değişiklik kaydedilir. Kaydı yalnız admin hesabı okuyabilir:

```bash
$ curl -s -b cookies.txt 'http://localhost:46492/api/audit?limit=5' | jq -c '.[]'
{"time":1789647517635,"username":"admin","client":"127.0.0.1","action":"purge_proxy_cache","resource_id":"jzr-pgf"}
{"time":1789647512463,"username":"admin","client":"127.0.0.1","action":"add_proxy","resource_id":"jzr-pgf","summary":"api-demo-app"}
{"time":1789647508125,"username":"admin","client":"127.0.0.1","action":"add_port","resource_id":"smc-gzh","summary":"api-demo /ip4/127.0.0.1/tcp/8475/http"}
{"time":1789647489459,"username":"admin","client":"127.0.0.1","action":"login"}
```

`time` alanı Unix epoch'tan bu yana milisaniyedir. Summary hiçbir zaman parola, token veya key taşımaz. Query parametreleri için [Audit log](@/configuration.tr.md#audit-log) bölümüne bakın.

## Adım 8: Değişiklikleri izleyin

`GET /api/events`, WebUI'ın kendini yenilemek için kullandığı bir server-sent event stream'idir. Her değişiklik bir satır gönderir:

```bash
$ curl -N -b cookies.txt http://localhost:46492/api/events
data: {"event":"proxies_updated","entries":[...]}

data: {"event":"proxy_status_updated","id":"pdb-khh","status":{"state":"active"}}

data: {"event":"port_table_updated","entries":[...]}

data: {"event":"port_status_updated","id":"dpv-ylj","status":{"state":{"socket":"listening","tls":null}}}
```

Status endpoint'lerini döngüde sormak yerine bir dashboard veya alarm için bunu kullanın.

## Referans

- [Admin API](@/configuration.tr.md#yonetim-api-si): doküman adresi ve giriş.
- [Audit log](@/configuration.tr.md#audit-log): alanlar ve saklama süresi.
- [Hesaplar](@/accounts.tr.md): roller, proxy listeleri ve TOTP.

## Sonraki adımlar

- [Load balancing ve health check](@/tutorials/load-balancing.tr.md): status endpoint'inin kullanımı.
- [Cache ve compression](@/tutorials/cache-and-compression.tr.md): purge'ün ne sildiği.
- [Yüksek erişilebilirlik](@/tutorials/high-availability.tr.md): cluster'daki her node'da aynı API.
