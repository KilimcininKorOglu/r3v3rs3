+++
title = "MCP ve CLI"
description = "MCP sunucusu, tool'ları ve r3 komut satırı istemcisi"
weight = 10
+++

## MCP Sunucusu

Admin API, araç setini `/api/mcp` üzerinde Model Context Protocol (Streamable HTTP, 2025-03-26 spesifikasyonu) ile yayınlar. **%100 MCP Yönetim Eşitliği (MCP Parity)** ile ters proxy rotaları, dinleyici portlar, ACME sertifikaları, PaaS uygulamaları, Docker Compose yığınları, tek tıkla kurulan App Store katalog uygulamaları, yönetilen veritabanları, otomatik yedeklemeler, uzak agent düğümleri ve felaket kurtarma anlık görüntüleri dahil sistemdeki her bir özellik, bir insanın WebUI'yi açmasına gerek kalmadan bir yapay zeka ajanı tarafından otonom olarak %100 yönetilebilir.

[MCP ve komut satırı istemcisi](@/tutorials/mcp-and-cli.tr.md) rehberi MCP'yi, bir AI istemcisini ve `r3`'ü adım adım kurar.

### Otonom MCP Araç Seti

| Kategori | Araçlar ve İşlemler | Açıklama |
|---|---|---|
| **Sistem ve İzleme** | `server_status`, `get_config`, `list_audit_log`, `get_system_metrics`, `get_system_anomalies`, `list_banned_ips`, `unban_ip`, `list_quarantined_ips`, `quarantine_ip`, `unquarantine_ip`, `get_quarantine_config`, `update_quarantine_config`, `get_geoip_status`, `update_geoip_config`, `update_geoip_database`, `get_upgrade_status`, `upgrade_server`, `get_workers`, `update_config`, `get_traffic`, `get_traffic_entry`, `get_traffic_geo`, `replay_traffic_request`, `get_anomaly_rules`, `update_anomaly_rules`, `dismiss_anomalies`, `analyze_security_headers`, `preview_proxy`, `get_cdn_status`, `refresh_cdn_ranges`, `list_discovery_status`, `get_logs`, `get_dashboard`, `check_grpc_health`, `invoke_grpc`, `reflect_grpc`, `invoke_proxy_grpc`, `reflect_proxy_grpc`, `rotate_platform_key`, `list_mcp_sessions`, `revoke_mcp_session`, `get_app_info`, `get_metrics`, `get_grafana_dashboard` | Çalışma zamanı sürümü, worker süreçleri, maskelenmiş sistem yapılandırması, değiştirilemez denetim kaydı (audit log), süreç dosya tanıtıcıları/bellek/lag, gerçek zamanlı anomali alarmları, fail2ban kaba kuvvet IP engeli, tehdit otomatik karantina yönetimi, otomatik GeoIP MMDB veritabanı yönetimi ve kesintisiz canlı sunucu kendini güncelleme işlemi. |
| **Portlar** | `list_ports`, `get_port`, `get_port_status`, `create_port`, `update_port`, `delete_port`, `reset_port`, `list_network_interfaces` | Dinleyici port yaşam döngüsü: dinleme adresleri, TLS sonlandırma ayarları, soket yeniden başlatma/yenileme ve sunucu ağ arayüzlerini listeleme. |
| **Proxy'ler** | `list_proxies`, `get_proxy`, `get_proxy_status`, `create_proxy`, `update_proxy`, `delete_proxy`, `purge_proxy_cache` | HTTP, WebSocket, TCP, UDP, TLS ve HTTP/3 için ters proxy yapılandırması, upstream sağlık durumu ve yol bazlı önbellek temizleme (purge). |
| **Sertifikalar ve ACME** | `list_certs`, `get_cert`, `check_cert_coverage`, `delete_cert`, `list_acme`, `get_acme`, `create_acme`, `delete_acme`, `request_acme`, `get_acme_cname_instructions`, `list_dns_credentials`, `get_dns_credential`, `create_dns_credential`, `update_dns_credential`, `delete_dns_credential`, `get_ct_config`, `update_ct_config`, `get_ct_status`, `list_ct_entries`, `check_ct_now`, `dismiss_ct_alert`, `update_acme`, `create_self_signed_cert`, `upload_cert`, `download_cert` (yalnız zincir, private key asla dönmez), `delete_many_certs`, `verify_cname` | TLS sertifikası inceleme, alan adı kapsam kontrolü, otomatik ACME sertifika talepleri (HTTP-01, TLS-ALPN-01, DNS-01), RFC 8555 CNAME alias delegasyonu, anlık sertifika yenileme, DNS sağlayıcı kimlik bilgileri yönetimi ve Sertifika Şeffaflığı sahte sertifika denetimi. |
| **Erişim Listeleri** | `list_access_lists`, `get_access_list`, `create_access_list`, `update_access_list`, `delete_access_list` | CIDR IP filtreleme, HTTP basic authentication ve özel bearer token'lar içeren erişim kontrol listeleri. |
| **PaaS Uygulamaları** | `list_apps`, `get_app`, `create_app`, `update_app`, `delete_app`, `deploy_app`, `stop_app`, `start_app`, `restart_app`, `get_app_logs`, `get_app_env`, `update_app_env`, `list_app_deployments`, `rollback_deployment`, `check_app_update`, `list_app_scheduled_commands`, `create_app_scheduled_command`, `update_app_scheduled_command`, `delete_app_scheduled_command`, `run_app_scheduled_command`, `get_deployment`, `approve_deployment`, `reject_deployment`, `get_app_metrics`, `get_app_services`, `get_app_topology`, `upgrade_app_catalog`, `update_app_git_token`, `delete_app_git_token`, `create_app_webhook_secret`, `delete_app_webhook_secret`, `install_app_hook`, `list_app_files`, `download_app_file` (en çok 1 MiB UTF-8 metin), `upload_app_file` (en çok 1 MiB UTF-8 metin), `delete_app_file`, `list_volume_backups`, `create_volume_backup`, `restore_volume_backup`, `delete_volume_backup` | Otonom PaaS konteyner yaşam döngüsü: Docker image/git/compose uygulaması dağıtımı, konteyner log akışı, ortam değişkeni yönetimi, önceki dağıtıma geri alma (rollback), zamanlanmış cron komutları yönetimi ile konteyner içinde anlık çalıştırma ve Docker registry güncelleme kontrolü. |
| **App Store Kataloğu** | `get_store`, `refresh_store`, `install_store_app` | Küratörlü tek tıkla kurulum şablonlarının (veritabanları, paneller, CMS, analitik) katalogdan doğrudan tam yapılandırılmış ve yönlendirilmiş uygulamalar olarak otonom kurulumu. |
| **Yönetilen Veritabanları ve Yedekler** | `list_databases`, `list_database_engines`, `create_database`, `get_database_credentials`, `list_database_tenants`, `reset_database_password`, `delete_database_tenant`, `list_database_backups`, `create_database_backup`, `delete_database_backup`, `get_backup_config`, `update_backup_config`, `test_database_backup`, `get_database_pitr_range`, `restore_database_pitr`, `archive_database_wal`, `list_database_wal_segments`, `delete_database`, `clone_database`, `list_database_replicas`, `create_database_replica`, `restore_database_backup`, `run_database_backup_now`, `list_all_backups`, `get_backups_overview`, `list_s3_database_backups`, `restore_s3_database_backup`, `refresh_database_engines` | Otomatik üretilen kimlik bilgileriyle PostgreSQL, MySQL, Redis, MariaDB ve MongoDB sağlama, anlık yedek alma, yedek silme, yedek bütünlüğü testi, dallanabilir PITR belirli bir zamana geri dönme, WAL arşivleme ve uzak depolama ayarları. |
| **Agent Hedef Düğümleri** | `list_targets`, `get_target`, `create_target`, `delete_target`, `update_target_region`, `drain_target`, `undrain_target`, `create_target_token`, `upgrade_target`, `upgrade_all_targets` | Çok düğümlü Docker agent altyapısı, tek kullanımlık kayıt token'ı üretme, hedef düğümleri kaldırma, coğrafi bölge atama, trafik boşaltma/geri alma ve uzak hedef ikili dosya güncellemeleri. |
| **Projeler ve Ortam Grupları** | `list_projects`, `create_project`, `delete_project`, `list_env_groups`, `create_env_group`, `delete_env_group`, `list_project_environments`, `create_environment`, `update_environment_env`, `sync_environment_env`, `get_project`, `update_project`, `get_environment`, `update_environment`, `delete_environment`, `get_environment_env`, `diff_environments`, `get_env_group`, `update_env_group` | Proje organizasyonu, çoklu ortam izolasyonu, paylaşılan ortam değişkeni grupları ve seçici anahtar kopyalama ile ortamlar arası yapılandırma senkronizasyonu. |
| **Dışa Dönük Webhook'lar** | `list_outbound_webhooks`, `get_outbound_webhook`, `create_outbound_webhook`, `update_outbound_webhook`, `delete_outbound_webhook`, `test_outbound_webhook` | Dışa dönük olay bildirim webhook dağıtıcıları: uç nokta yönetimi, HMAC-SHA256 imza sırrı doğrulaması, ayrıntılı olay abonelikleri ve uçtan uca webhook teslimat testi. |
| **Konteyner Registry'leri** | `list_registries`, `create_registry`, `update_registry`, `delete_registry` | Özel Docker ve OCI konteyner registry kimlik bilgileri: güvenli kimlik doğrulama, pull izinleri ve sunucu hedef uç noktaları. |
| **Git Bağlantıları** | `list_git_connections`, `get_git_connection`, `create_git_connection`, `delete_git_connection`, `list_git_repositories`, `list_git_branches`, `update_git_connection`, `authorize_git_connection`, `create_github_app`, `install_github_app` | Git sağlayıcı bağlantıları (GitHub App, GitLab, Gitea): OAuth bağlantı yönetimi, erişilebilir depo tarama ve uzak dal (branch) listeleme. |
| **Hesaplar ve Yetki Token'ları** | `list_accounts`, `create_account`, `update_account`, `delete_account`, `list_tokens`, `create_token`, `delete_token` | Kullanıcı hesap yönetimi: roller (admin, editor, viewer), proxy erişim kapsamları, parola yönetimi ve kalıcı yetkilendirilmiş API token üretimi. |
| **Felaket Kurtarma** | `list_disaster_recovery_backups`, `create_disaster_recovery_backup`, `restore_disaster_recovery_backup`, `delete_disaster_recovery_backup` | İsteğe bağlı AES-256-GCM şifrelemeli ve otomatik S3 replikasyonlu tam sistem yedek anlık görüntüleri. |

MCP sunucusu varsayılan olarak kapalıdır. Panelin ayarlar sayfası onu sunucu çalışırken açar ve giriş yapmasına izin verilecek client'ları tutar. `config.toml` aynı bölümü taşır:

```toml
[mcp]
enabled = true

[[mcp.clients]]
id = "r3v3rs3-cli"
name = "r3v3rs3 MCP client"
redirect_uris = ["http://127.0.0.1:8765/callback"]
```

Client, PKCE'li OAuth 2.1 authorization code akışıyla giriş yapar: client `/oauth/authorize` adresini tarayıcıda açar, oturum açmış hesap onay formundan client'a izin verir ve client tek kullanımlık kodu `/oauth/token` üzerinde access token ile değiştirir. Token, onayın kapsamını taşır: `read` durumu görür, `edit` ayrıca proxy'leri değiştirir. Onay formu istenen her scope için işaretli bir onay kutusu gösterir. Hesap vermek istemediği scope'ların işaretini kaldırır. `scope` parametresi olmayan bir istek hem `read` hem `edit` ister. `401` yanıtının `WWW-Authenticate` header'ı da iki scope'u adlandırır. Token yine hesabın rolüyle sınırlıdır: viewer rolündeki bir hesap `edit` token'ıyla hiçbir şeyi değiştiremez. Metadata belgeleri `/.well-known/oauth-authorization-server` ve `/.well-known/oauth-protected-resource` adreslerindedir.

OAuth akışını çalıştıramayan ve yalnız sabit bir `Authorization` header'ı gönderebilen bir istemci, hesabın bir API token'ını kullanır (`r3 <sunucu> token create`). `edit` token'ı hem `read` hem `edit` kapsamını, `read` token'ı yalnız `read` kapsamını verir. Hesabın rolü bu durumda da sınırdır. Silinen bir token anında `401` alır.

Token'lar ve dinamik olarak kaydedilen client'lar SQLite (`r3v3rs3.db`) veritabanında kalıcı olarak saklanır. Sunucunun yeniden başlaması veya güncellenmesi yetkilendirilmiş oturumları korur; client'lar 30 günlük refresh token'larını kullanarak kullanıcıya yeniden giriş sormadan access token'larını arka planda sessizce yeniler. Refresh token'lar RFC 6819 ve OAuth 2.1 gereği tek kullanımlık rotasyon ve tekrar kullanım tespiti uygular: daha önce rotasyona uğramış bir refresh token tekrar sunulduğunda ele geçirilmiş token ailesi derhal sonlandırılır. Aktif oturumlar WebUI Ayarlar sayfasındaki "Aktif MCP Oturumları" kartından veya `GET /api/mcp/sessions` ve `DELETE /api/mcp/sessions/{id}` endpoint'leri üzerinden incelenebilir ve sonlandırılabilir (revoke).

Bağlanma yolu client'a göre değişir. MCP authorization akışını konuşan bir AI host, örneğin Claude Code veya Cursor, ekstra bir programa ihtiyaç duymaz: bu sunucuyu bir HTTP MCP sunucusu olarak ekler, örneğin `claude mcp add --transport http --scope user r3v3rs3 https://panel.example.com/api/mcp` komutuyla, ve `/mcp` komutunun `Authenticate` adımı aynı authorization akışını tarayıcıda çalıştırır. Aşağıdaki komut satırı client'ı diğer durumlar içindir: script'ler, cron job'ları ve tarayıcı oturumu olmayan sunucular; tek komut bir kez giriş yapar ve token'larını sonraki her çağrı için saklar.

Depo, `r3` (`r3v3rs3-cli` paketi) içinde bir komut satırı client'ı ile gelir. Kurulum script'i en yeni release'in derlenmiş binary'sini GitHub'dan indirir, sha256 özetini doğrular ve kurar:

```sh
# Linux ve macOS:
curl -fsSL https://raw.githubusercontent.com/KilimcininKorOglu/r3v3rs3/main/install-cli.sh | sh
```

```powershell
# Windows (PowerShell):
irm https://raw.githubusercontent.com/KilimcininKorOglu/r3v3rs3/main/install-cli.ps1 | iex
```

Windows'ta binary `%LOCALAPPDATA%\Programs\r3` içine düşer; onu `PATH`'e eklersiniz. Script Linux ve macOS'ta `/usr/local/bin`'e kurar, `INSTALL_DIR` bu dizini değiştirir. Belirli bir release `--version vX.Y.Z` ile kurulur; her platformun arşivi ayrıca [releases sayfasında](https://github.com/KilimcininKorOglu/r3v3rs3/releases) durur.

Sonra:

```sh
r3 http://127.0.0.1:46492 login
r3 http://127.0.0.1:46492 platform get-app-info
```

`login`, onay sayfasını tarayıcıda açar ve token'ları yapılandırma dizinine kaydeder. Bir script, access token'ı doğrudan `--token` ile veya `R3V3RS3_TOKEN` ortam değişkeniyle verebilir. `update` komutu, çalışan binary'nin yerine daha yeni bir release kurar ve sha256 özetini release'in checksum'larıyla doğrular.

### Tam kaynak yönetimi

`r3`, ters proxy ve PaaS platformunun tüm bileşenlerini doğrudan yönetmek için birinci sınıf alt komutlar sunar:

```sh
# Portlar
r3 <server> port list
r3 <server> port get <id>
r3 <server> port create <spec.json|yaml|->
r3 <server> port update <id> <spec.json|yaml|->
r3 <server> port delete <id>
r3 <server> port reset <id>

# Proxy'ler
r3 <server> proxy list
r3 <server> proxy get <id>
r3 <server> proxy create <proxy.json|yaml|->
r3 <server> proxy update <id> <proxy.json|yaml|->
r3 <server> proxy get-status <id>
r3 <server> proxy purge-cache <id> [--path /static]
r3 <server> proxy delete <id>

# Sertifikalar ve ACME
r3 <server> cert list
r3 <server> cert get <id>
r3 <server> cert upload --cert server.crt --key server.key
r3 <server> cert check-coverage
r3 <server> cert delete <id>
r3 <server> cert create-self-signed --san app.example.com [--ca <ca-id>] [--kind client]
r3 <server> cert download <id> [--output cert.tar.gz]
r3 <server> cert delete-many <id> <id>
r3 <server> cert verify-cname --domain app.example.com [--expected-target panel.example.com]
r3 <server> acme list
r3 <server> acme get <id>
r3 <server> acme create <acme.json|yaml|->
r3 <server> acme update <id> <acme.json|yaml|->
r3 <server> acme request <id>
r3 <server> acme delete <id>

# Erişim Listeleri (ACL) ve DNS Sağlayıcıları
r3 <server> acl list
r3 <server> acl get <id>
r3 <server> acl create <acl.json|yaml|->
r3 <server> acl update <id> <acl.json|yaml|->
r3 <server> acl delete <id>
r3 <server> dns list
r3 <server> dns get <id>
r3 <server> dns create <dns.json|yaml|->
r3 <server> dns update <id> <dns.json|yaml|->
r3 <server> dns delete <id>

# Dağıtılan Uygulamalar
r3 <server> app list
r3 <server> app get <id>
r3 <server> app create <app.json|yaml|->
r3 <server> app update <id> <app.json|yaml|->
r3 <server> app deploy <id>
r3 <server> app rollback-deployment <id>
r3 <server> app start <id>
r3 <server> app stop <id>
r3 <server> app restart <id>
r3 <server> app get-logs <id> [--tail 100]
r3 <server> app get-log-size <id>
r3 <server> app vacuum-logs <id>
r3 <server> app get-env <id>
r3 <server> app update-env <id> KEY=VAL [KEY2=VAL2...]
r3 <server> app delete <id>
r3 <server> app check-update <id>
r3 <server> app list-deployments <id>
r3 <server> app get-deployment <deployment-id>
r3 <server> app approve-deployment <deployment-id>
r3 <server> app reject-deployment <deployment-id>
r3 <server> app get-metrics <id> [--hours 6]
r3 <server> app get-services <id>
r3 <server> app get-topology <id>
r3 <server> app upgrade-catalog <id>
printf '%s' "$TOKEN" | r3 <server> app update-git-token <id>
r3 <server> app delete-git-token <id>
r3 <server> app create-webhook-secret <id>
r3 <server> app delete-webhook-secret <id>
r3 <server> app install-hook <id>
r3 <server> app files list <id> --path /data
r3 <server> app files download <id> --path /data/app.db [--output app.db]
r3 <server> app files upload <id> --path /data/config.yml --file ./config.yml
r3 <server> app files delete <id> --path /data/old.log
r3 <server> app volumes list-backups <id> <volume>
r3 <server> app volumes create-backup <id> <volume>
r3 <server> app volumes restore-backup <id> <volume> <backup-id>
r3 <server> app volumes delete-backup <id> <volume> <backup-id>
r3 <server> app volumes download-backup <id> <volume> <backup-id> [--output data.tar.gz]
r3 <server> app volumes export-archive <id> <volume> [--output data-export.tar.gz]
r3 <server> app volumes import-archive <id> <volume> --file ./data.tar.gz

# Yönetilen Veritabanları
r3 <server> db list
r3 <server> db get-app <id>
r3 <server> db create --name my-db --engine postgres [--version postgres:18-alpine]
r3 <server> db clone <id> --name my-db-clone
r3 <server> db create-replica <id> --name my-db-replica
r3 <server> db get-app-logs <id> [--tail 100]
r3 <server> db get-credentials <id>
r3 <server> db list-tenants <id>
r3 <server> db reset-password <id> [--tenant <name>]
r3 <server> db delete-tenant <id> <tenant>
r3 <server> db delete <id>
r3 <server> db list-replicas <id>
r3 <server> db refresh-engines
r3 <server> db list-engines

# Yedekler, Otomatik Doğrulama, Belirli Bir Ana Geri Yükleme (PITR) ve WAL
r3 <server> backup get-config
r3 <server> backup list <database-id>
r3 <server> backup create <database-id>
r3 <server> backup test <database-id> <backup-id>
r3 <server> backup restore <database-id> <backup-id>
r3 <server> backup get-pitr-range <database-id>
r3 <server> backup restore-pitr <database-id> <timestamp_ms> [--new-name <name>] [--host-port <port>]
r3 <server> backup archive-wal <database-id>
r3 <server> backup list-wal-segments <database-id>
r3 <server> backup delete <database-id> <backup-id>
r3 <server> backup list-all
r3 <server> backup get-overview
r3 <server> backup run-now <database-id>
r3 <server> backup list-s3 [--database <name>]
r3 <server> backup download-s3 --key <key> [--output db.sql.gz]
r3 <server> backup download <database-id> <backup-id> [--output db.sql.gz]
r3 <server> backup restore-s3 <key> [--target-db <id> | --new-name <name> [--engine <engine>]]

# Çoklu Düğüm Agent Hedefleri
r3 <server> target list
r3 <server> target get <id>
r3 <server> target create --name worker-1 --host 10.0.0.5 --port 46493
r3 <server> target drain <id>
r3 <server> target undrain <id>
r3 <server> target upgrade <id>
r3 <server> target upgrade-all
r3 <server> target create-token <id>
r3 <server> target delete <id>

# Projeler ve Ortamlar
r3 <server> project list
r3 <server> project get <id>
r3 <server> project create --name my-project [--description "Ana servisler"]
r3 <server> project sync-environment-env <project-id> --from staging --to production
r3 <server> project delete <id>
r3 <server> project update <id> --name my-project [--description "Core apps"]
r3 <server> project create-environment <project-id> --name staging
r3 <server> project get-environment <project-id> <env-id>
r3 <server> project update-environment <project-id> <env-id> --name stage
r3 <server> project delete-environment <project-id> <env-id>
r3 <server> project get-environment-env <project-id> <env-id>
r3 <server> project update-environment-env <project-id> <env-id> <vars.json|yaml|->
r3 <server> project diff-environments <project-id> <source-env-id> <target-env-id>
r3 <server> env-group list
r3 <server> env-group get <id>
r3 <server> env-group create <group.json|yaml|->
r3 <server> env-group update <id> <group.json|yaml|->
r3 <server> env-group delete <id>

# Uygulama Kataloğu (App Store)
r3 <server> store get
r3 <server> store refresh
r3 <server> store install-app <store-app-id> --name my-service [--domain service.example.com]

# Trafik Telemetrisi ve İstek İnceleme
r3 <server> traffic connections list [--proxy <id>]
r3 <server> traffic connections terminate <connection-id>
r3 <server> traffic get-priority-stats
r3 <server> traffic get-geo
r3 <server> traffic replay-request --entry <log-entry-id>
r3 <server> traffic get [--resource-id <proxy-id>] [--status 502] [--search /api] [--country DE] [--limit 50]
r3 <server> traffic get-entry <log-entry-id>
r3 <server> traffic stream [--resource-id <proxy-id>] [--status 502] [--search /api] [--country DE]
r3 <server> app stream-logs <id> [--tail 100] [--service web]
r3 <server> app open-terminal <id> [--shell /bin/bash]
r3 <server> traffic replay-request <request.json|yaml|->
r3 <server> security get-system-anomalies
r3 <server> security get-anomaly-rules
r3 <server> security update-anomaly-rules <rules.json|yaml|->
r3 <server> security dismiss-anomalies
r3 <server> security analyze-headers <rules.json|yaml|->
r3 <server> proxy preview <request.json|yaml|->
r3 <server> port get-status <port-id>
r3 <server> port list-network-interfaces
r3 <server> cdn get-status
r3 <server> cdn refresh-ranges
r3 <server> discovery list-status
r3 <server> get-logs server [--since 1700000000] [--until 1700003600] [--limit 100]
r3 <server> get-dashboard
r3 <server> audit list-log [--username admin] [--action create_proxy] [--resource-id <id>] [--search text] [--limit 50]
r3 <server> grpc check-health <request.json|yaml|->
r3 <server> grpc invoke <request.json|yaml|->
r3 <server> grpc reflect <request.json|yaml|->
r3 <server> proxy invoke-grpc <proxy-id> <request.json|yaml|->
r3 <server> proxy reflect-grpc <proxy-id> [<request.json|yaml|->]
r3 <server> backup update-config <config.json|yaml|->
r3 <server> platform list-mcp-sessions
r3 <server> platform revoke-mcp-session <session-id>
r3 <server> platform get-grafana-dashboard [--output r3v3rs3-grafana.json]

# Güvenlik, Yasaklı & Karantinalı IP'ler
r3 <server> security list-banned-ips
r3 <server> security unban-ip <ip>
r3 <server> security list-quarantined-ips
r3 <server> security quarantine-ip <ip> [--reason "Tarayıcı sondası"] [--duration-seconds 3600]
r3 <server> security unquarantine-ip <ip>
r3 <server> security get-quarantine-config
r3 <server> security update-quarantine-config [--enabled true] [--max-violations 5] [--window-seconds 60] [--ban-duration-seconds 3600] [--block-status-codes 404,403] [--scanner-patterns "/.env,/.git"] [--allowed-ips "198.51.100.0/24"]

# Webhook'lar ve Zamanlanmış Görevler (Cron)
r3 <server> webhook list
r3 <server> webhook get <id>
r3 <server> webhook create <webhook.json|yaml|->
r3 <server> webhook update <id> <webhook.json|yaml|->
r3 <server> webhook test <id>
r3 <server> webhook delete <id>
r3 <server> cron list <app-id>
r3 <server> cron create <app-id> --cron "0 * * * *" --command "php artisan schedule:run" [--name hourly]
r3 <server> cron update <app-id> <job-id> [--cron "*/15 * * * *"] [--command "php artisan queue:work"] [--name queue]
r3 <server> cron run <app-id> <job-id>
r3 <server> cron delete <app-id> <job-id>

# Konteyner Registry'leri
r3 <server> registry list
r3 <server> registry create --server ghcr.io --username user --password token
r3 <server> registry update <id> [--server ghcr.io] [--username user] [--password token]
r3 <server> registry delete <id>

# Git Sağlayıcı Bağlantıları ve Depolar
r3 <server> git list
r3 <server> git get <id>
r3 <server> git create --name my-gitlab --provider gitlab --client-id <id> [--client-secret <secret>]
r3 <server> git list-repositories <id> [--search query] [--page 1]
r3 <server> git list-branches <id> --repo owner/repo
r3 <server> git delete <id>
r3 <server> git update <id> <connection.json|yaml|->
r3 <server> git authorize <id> --redirect-uri https://panel.example.com/oauth/git/callback
r3 <server> git create-github-app --name team-github --redirect-uri https://panel.example.com/oauth/git/callback [--organization my-team] [--url https://github.example.com]
r3 <server> git install-github-app <id>

# Kullanıcı Hesapları ve Yetki Token'ları
r3 <server> account list
r3 <server> account create --username operator --password <pass> [--role editor]
r3 <server> account update <username> [--password <pass>] [--role admin]
r3 <server> account delete <username>
r3 <server> token list
r3 <server> token create --name ci-deploy [--scope edit]
r3 <server> token delete <id>

# Platform Durumu ve Bakım
r3 <server> platform get-app-info
r3 <server> platform get-workers
r3 <server> platform get-metrics
r3 <server> platform run-prune
r3 <server> platform vacuum-logs [--threshold-mb 50]
r3 <server> platform upgrade-server
r3 <server> platform get-upgrade-status
r3 <server> platform rotate-key
```

`r3 app open-terminal` komutları standart girdisinden de alır, örneğin `echo 'df -h' | r3 <server> app open-terminal <id>`. Girdi bittiğinde istemci kabuğa Ctrl-D gönderir ve kabuk kapanana kadar çıktıyı yazdırır.

### Deklaratif tanımlar (`apply -f`)

CI/CD pipeline'ları ve betikli yönetim için `r3 apply -f <dosya>`, portları, proxy'leri ve uygulamaları tek hamlede ve idempotent biçimde tanımlamak üzere YAML veya JSON manifest uygular:

```sh
# Dosyadan uygula
r3 http://127.0.0.1:46492 apply -f infrastructure.yaml

# Standart girdiden uygula
cat infrastructure.yaml | r3 http://127.0.0.1:46492 apply -f -
```

Örnek bir YAML manifesti:

```yaml
ports:
  - id: http
    port:
      listen: /ip4/0.0.0.0/tcp/80
      active: true

proxies:
  - id: web
    name: "Web frontend"
    protocol: http
    ports: ["http"]
    routes:
      - host: "example.com"
        servers:
          - url: "http://127.0.0.1:3000"

apps:
  - spec:
      name: "my-app"
      domains: ["app.example.com"]
      target:
        type: image
        image: "nginx:alpine"
        port: 80
```
