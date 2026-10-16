+++
title = "r3v3rs3"
sort_by = "weight"
+++

[![Crates.io](https://img.shields.io/crates/v/r3v3rs3.svg)](https://crates.io/crates/r3v3rs3)
[![GitHub license](https://img.shields.io/github/license/KilimcininKorOglu/r3v3rs3.svg)](https://github.com/KilimcininKorOglu/r3v3rs3/blob/main/LICENSE)
[![Rust](https://github.com/KilimcininKorOglu/r3v3rs3/actions/workflows/rust.yml/badge.svg)](https://github.com/KilimcininKorOglu/r3v3rs3/actions/workflows/rust.yml)
[![dependency status](https://deps.rs/crate/r3v3rs3/latest/status.svg)](https://deps.rs/crate/r3v3rs3)

r3v3rs3, Rust ile yazılmış bir reverse proxy sunucusudur. TCP, UDP, TLS, HTTP ve WebSocket bağlantılarını proxy'ler, gelen HTTP/3 bağlantılarını da kabul eder. Uygulamaları container olarak deploy eder ve alan adlarını kendi proxy'leri üzerinden yönlendirir. [Taxy](https://github.com/picoHz/taxy) projesinin bir fork'udur.

Ayarları tarayıcıdan yaparsınız. Tek bir binary hem proxy'yi hem de İngilizce ve Türkçe WebUI'yi taşır; her port, proxy, sertifika ve hesap bu WebUI'de bir formdur. Değişiklik yeniden başlatmadan uygulanır. Aynı işlemler yönetim API'si üzerinden de yapılır. r3v3rs3 proxy'lerini Docker etiketlerinden veya etcd key'lerinden de oluşturabilir.

# Temel özellikler

## Proxy

- TCP, UDP, TLS, HTTP/1.1 ve HTTP/2 proxy'leri. HTTP upgrade ve WebSocket bağlantıları da buna dahildir.
- HTTP/3 desteği kısmidir: yalnız gelen QUIC bağlantıları kabul edilir. Bir `alt-svc` header'ı tarayıcıya QUIC portunu bildirir. Upstream bağlantıları HTTP/2 veya HTTP/1.1 kullanır. WebTransport desteği yoktur.
- Host adına (tam, wildcard veya regex) ve yola göre routing. Porttaki bütün proxy'ler içinde yolu en uzun eşleşen route kazanır, yani proxy'lerin ve route'ların sırası sonucu değiştirmez.
- Yol yeniden yazma (prefix silme, regex değiştirme, prefix ekleme), yönlendirme kuralları ve sabit yanıtlar: yönlendirme host'u, durum kodu host'u ve 404 host'u şablonları.
- Bir route, proxy'sinin ayarlarının yerine kendi ayarını koyabilir: timeout'lar, yeniden deneme, rate limit, IP filtresi, kimlik doğrulama, header kuralları ve gövde boyutu.
- Round robin (smooth weighted), rastgele, ilk sağlıklı sunucu ve istemci IP hash'i ile yük dengeleme. Sunucuyu devre dışı bırakmak için 0 ile 65535 arası ağırlık.
- Yeniden deneme: aktif ve pasif sağlık kontrolü, half-open denemesiyle circuit breaker, idempotent method kuralları ve gövde replay limiti. HMAC imzalı cookie ile sticky session ve yüzde ile istek yansıtma.
- Proxy ve route başına upstream connect ve request timeout'ları. TTL bitince yenilenen DNS SRV kayıtlarından gelen upstream sunucuları (`http+srv://` URL'leri).
- İstemci başına UDP oturumları: boşta kalma timeout'u ve port başına en çok 10.000 oturum.
- Düz HTTP sunuculara gRPC için h2c ile HTTP/2. Bir HTTP/2 upstream bağlantısını çok sayıda istemciye taşıyan paylaşılan upstream bağlantı havuzları.
- Genel ve port başına ayarlanabilir HTTP/2 ve HTTP/3 flood limitleri.
- Güvenilen yük dengeleyicilerden kabul edilen ve upstream sunuculara gönderilen PROXY protocol v1 ve v2.

## Güvenlik ve istek kontrolü

- Proxy ve route başına IP izin ve engelleme listeleri. Tek bir adın altında IP filtresi ve kimlik doğrulamayı taşıyan, paylaşılan erişim listeleri.
- Saniye, dakika veya saat periyotlu, burst'ü ve `Retry-After` header'ı olan istemci başına rate limit. `413` ile istek gövdesi boyut limiti.
- Kimlik doğrulama: Argon2 hash'iyle Basic auth, Bearer token'ları, forward auth (oauth2-proxy, Authelia) ve TOTP'li yönetim paneli oturumu. Bearer token'lar SHA-256 özeti olarak saklanır ve sabit sürede karşılaştırılır. Giriş denemesi limitleri de vardır.
- Sekiz bilinen CDN'in (Cloudflare, Fastly, Amazon CloudFront, Bunny, Gcore, KeyCDN, Imperva, Google Cloud Load Balancing) ve güvenilen proxy'lerin arkasındaki gerçek istemci IP'si bulunur. CDN aralıkları her gün yenilenir. Güvenilmeyen karşı uçlardan sahte istemci IP header'ları silinir.
- Değişken kullanan istek ve yanıt header kuralları: `{client_ip}`, `{host}`, `{request_id}`, `{route}`, `{client_cert_subject}` ve `{client_cert_fingerprint}`.
- Yanıt sıkıştırma (brotli, zstd ve gzip) ve ETag ve Last-Modified yenilemesi, `304` yanıtları, `X-Cache` ve `Age` header'ları ve purge API'si olan bellekte tutulan HTTP cache.
- Mutual TLS: TLS portlarında istemci sertifikası doğrulaması (kapalı, isteğe bağlı veya zorunlu) ve upstream sunuculara istemci sertifikası.

## Sertifikalar

- Yüklenen veya kendinden imzalı sunucu, istemci ve kök sertifikaları.
- HTTP-01, TLS-ALPN-01 ve DNS-01 challenge'larıyla ACME v2 (örneğin Let's Encrypt). DNS-01 wildcard sertifikaları da alır. DNS-01 üç yolla çalışır: 12 DNS sağlayıcısının API'si, program allowlist'i olan bir exec komutu veya TSIG'li RFC 2136.
- Sertifika süresi uyarıları ve webhook bildirimleri. Webhook'lar bearer token taşır, yeniden dener ve sertifika, deployment ile agent olaylarını bildirir.

## Yönetim

- WebUI'ı içinde taşıyan tek bir binary. WebUI İngilizce ve Türkçedir ve açık, koyu ve sistem temaları vardır. Ayar değişiklikleri yeniden başlatmadan uygulanır.
- OpenAPI dokümanı ve Swagger UI sunan yönetim API'si ([Yönetim API'si](@/configuration.tr.md#yonetim-api-si)).
- İstek, cache ve bağlantı sayaçlarıyla Prometheus metrics endpoint'i: `GET /api/metrics` ([Metrics](@/configuration.tr.md#metrics)).
- OAuth 2.1 ve PKCE ile `read` ve `edit` scope'lu `/api/mcp` üzerindeki MCP sunucusu; adresi doğrudan bağlanan Claude Code gibi AI host'ları için, ayrıca script'ler ve terminal için `r3v3rs3-cli` komut satırı istemcisi ([MCP sunucusu](@/configuration.tr.md#mcp-sunucusu)).
- `admin`, `editor` ve `viewer` rolleri olan hesaplar, hesap başına proxy listesi, TOTP ve saklama süreli denetim kaydı ([Hesaplar](@/accounts.tr.md)).
- Saatlik dosya döndürmesi, text veya JSON biçimi olan erişim ve sunucu log'ları. Log ve denetim sayfalarının arkasındaki `log.db` veritabanı ([Loglama](@/configuration.tr.md#log)).
- Portları sunan, master admin API'sini tutan isteğe bağlı worker süreci. Worker çökmesi paneli düşürmez ([Worker süreçleri](@/configuration.tr.md#worker-surecleri)).
- SIGUSR2 ile kesintisiz yükseltme (`systemctl reload r3v3rs3`), sha256 özetini doğrulayan panel içi yükseltme butonu ve panelin her sayfasında banner'la saat başı daha yeni bir release kontrolü ([Kesintisiz yükseltme](@/configuration.tr.md#kesintisiz-yukseltme), [Panelden yükseltme](@/configuration.tr.md#panelden-yukseltme)).
- Docker etiketleri ve etcd ile servis keşfi. Keşfedilen proxy'ler salt okunurdur ve proxy olamayan bir tanım, proxy listesinde sorun olarak raporlanır ([Servis keşfi](@/discovery.tr.md)).

## Deploy platformu

- Registry image'ından, Dockerfile'ı olan bir Git repository'sinden veya bir Git repository'sindeki Docker Compose dosyasından uygulamalar.
- Katalogdaki bir image app'i tek tıkla kuran, günde bir kez depodan yenilenen App Store.
- Proxy'yi ancak yeni container sağlık kontrolünden geçince değiştiren blue-green deployment'lar ve önceki bir deployment'a geri alma.
- r3v3rs3 uygulamaların alan adlarını kendisi yönlendirir ve sertifikalarını ACME ile alır.
- Şifreli ortam değişkenleri. Her uygulamanın deployment'ları ve container log'u WebUI'da ve yönetim API'sinde görünür.
- Git sağlayıcı bağlantıları: r3v3rs3'ün bir manifest'ten oluşturduğu GitHub App (GitHub Enterprise Server'da da) ve GitLab ile Gitea için OAuth uygulamaları. Bağlantı repository'leri ve branch'leri listeler. Private repository'leri clone eder ve push webhook'unu kurar.
- GitHub, GitLab, Gitea, Forgejo veya genel bir HMAC göndericisinden gelen push webhook'ları deployment başlatır.
- Agent hedefleri uygulamaları başka sunucularda çalıştırır. Agent, tek seferlik kayıttan sonra master'a mTLS ile bağlanır ([Deploy platformu](@/platform.tr.md)).

# Kurulum

```bash
curl -fsSL https://raw.githubusercontent.com/KilimcininKorOglu/r3v3rs3/main/install.sh | sudo bash
```

[Linux sunucuya kurulum](@/tutorials/install-linux.tr.md) rehberi seçenekleri, yolları, yükseltmeyi, kaldırmayı ve diğer kurulum yollarını anlatır: cargo-binstall, `cargo install` ve release arşivleri.

# Geliştirme

Ayrıntılar için [Geliştirme](@/development.tr.md) bölümüne bakın.

# İlk kurulum

Admin hesabı oluşturmak, sunucuyu başlatmak ve ilk proxy'nizi kurmak için [Başlangıç](@/tutorials/getting-started.tr.md) rehberini izleyin.
