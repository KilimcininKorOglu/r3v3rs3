//! SQLite-backed configuration and state storage for r3v3rs3.
//!
//! Stores server configuration, listening ports, reverse proxies, access lists,
//! ACME configs, sealed DNS credentials, accounts, certificate metadata, and sent notifications
//! in a single unified SQLite database (`r3v3rs3.db`).

use super::{account, storage::Storage};
use anyhow::Context as _;
use crate::audit::AuditStore;
use crate::cdn::CdnRanges;
use crate::certs::{Cert, acme::AcmeEntry};
use crate::dns_credential::StoredDnsCredential;
use crate::keys::crypto::ClusterKeys;
use crate::keys::key_file::{load_keys, write_new_key_file};
use crate::platform::{DATABASE_FILE, KEY_FILE};
use indexmap::map::IndexMap;
use r3v3rs3_api::{
    access_list::{AccessList, AccessListEntry},
    acme::{DnsCredentialInfo, DnsProvider},
    app::AppConfig,
    auth::{Account, LoginRequest, LoginResponse, Role},
    cert::CertKind,
    error::Error,
    id::ShortId,
    port::{Port, PortEntry},
    proxy::{Proxy, ProxyEntry},
    store::StoreCatalog,
};
use serde::de::DeserializeOwned;
use serde_derive::{Deserialize, Serialize};
use sqlx::{
    ConnectOptions as _, Row, SqlitePool,
    sqlite::{SqliteConnectOptions, SqliteJournalMode, SqliteSynchronous},
};
use std::{
    collections::{HashMap, HashSet},
    path::{Path, PathBuf},
    sync::Arc,
};
use tokio::fs;
use tracing::info;

const SCHEMA: &[&str] = &[
    "CREATE TABLE IF NOT EXISTS server_config (
        id         INTEGER PRIMARY KEY CHECK (id = 1),
        data       TEXT NOT NULL,
        updated_at INTEGER NOT NULL
    )",
    "CREATE TABLE IF NOT EXISTS ports (
        id          TEXT PRIMARY KEY,
        listen_addr TEXT NOT NULL,
        name        TEXT NOT NULL,
        data        TEXT NOT NULL,
        updated_at  INTEGER NOT NULL
    )",
    "CREATE TABLE IF NOT EXISTS proxies (
        id         TEXT PRIMARY KEY,
        name       TEXT NOT NULL,
        data       TEXT NOT NULL,
        updated_at INTEGER NOT NULL
    )",
    "CREATE INDEX IF NOT EXISTS proxies_name ON proxies (name)",
    "CREATE TABLE IF NOT EXISTS access_lists (
        id         TEXT PRIMARY KEY,
        name       TEXT NOT NULL,
        data       TEXT NOT NULL,
        updated_at INTEGER NOT NULL
    )",
    "CREATE TABLE IF NOT EXISTS acme_configs (
        id         TEXT PRIMARY KEY,
        domain     TEXT NOT NULL,
        data       TEXT NOT NULL,
        updated_at INTEGER NOT NULL
    )",
    "CREATE TABLE IF NOT EXISTS dns_credentials (
        id          TEXT PRIMARY KEY,
        name        TEXT NOT NULL UNIQUE,
        provider    TEXT NOT NULL,
        sealed      TEXT NOT NULL,
        created_at  INTEGER NOT NULL
    )",
    "CREATE TABLE IF NOT EXISTS accounts (
        username   TEXT PRIMARY KEY,
        data       TEXT NOT NULL,
        updated_at INTEGER NOT NULL
    )",
    "CREATE TABLE IF NOT EXISTS certificates (
        id          TEXT PRIMARY KEY,
        kind        TEXT NOT NULL,
        san_domains TEXT NOT NULL,
        issuer      TEXT,
        not_before  INTEGER NOT NULL,
        not_after   INTEGER NOT NULL,
        cert_path   TEXT NOT NULL,
        key_path    TEXT,
        fingerprint TEXT NOT NULL UNIQUE,
        created_at  INTEGER NOT NULL
    )",
    "CREATE INDEX IF NOT EXISTS certificates_domains ON certificates (san_domains)",
    "CREATE TABLE IF NOT EXISTS sent_notifications (
        id      TEXT PRIMARY KEY,
        sent_at INTEGER NOT NULL
    )",
];

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(bound(
    serialize = "T: serde::Serialize",
    deserialize = "T: serde::de::DeserializeOwned"
))]
struct Versioned<T> {
    #[serde(default = "default_version")]
    pub version: String,
    #[serde(flatten, default)]
    pub data: T,
}

fn default_version() -> String {
    crate::config::build_info::PKG_VERSION.to_owned()
}

pub struct SqliteStorage {
    dir: PathBuf,
    db_path: PathBuf,
    pool: SqlitePool,
    secrets: tokio::sync::RwLock<Option<std::sync::Arc<ClusterKeys>>>,
}

impl SqliteStorage {
    pub async fn open(dir: &Path) -> anyhow::Result<Self> {
        fs::create_dir_all(dir).await?;
        ensure_key_files(dir).await?;

        let db_path = dir.join(DATABASE_FILE);
        migrate_legacy_platform_db(dir, &db_path).await?;

        let pool = connect_db(&db_path).await?;
        init_schema(&pool).await?;

        let storage = Self {
            dir: dir.to_owned(),
            db_path,
            pool,
            secrets: tokio::sync::RwLock::new(None),
        };

        if let Ok(keys) = storage.secret_keys().await {
            migrate_legacy_files(dir, &storage.pool, &keys).await?;
        }

        Ok(storage)
    }

    pub fn pool(&self) -> &SqlitePool {
        &self.pool
    }

    pub async fn read_local_config(config_dir: &Path) -> anyhow::Result<AppConfig> {
        let db_path = config_dir.join(DATABASE_FILE);
        if db_path.exists() {
            let storage = Self::open(config_dir).await?;
            return Ok(storage.load_app_config().await);
        }
        let config_file = config_dir.join("config.toml");
        if config_file.exists() {
            return super::file::FileStorage::new(config_dir)
                .read_app_config()
                .await;
        }
        Ok(AppConfig::default())
    }

    async fn secret_keys(&self) -> anyhow::Result<std::sync::Arc<ClusterKeys>> {
        {
            let guard = self.secrets.read().await;
            if let Some(keys) = guard.as_ref() {
                return Ok(keys.clone());
            }
        }
        let mut guard = self.secrets.write().await;
        if let Some(keys) = guard.as_ref() {
            return Ok(keys.clone());
        }
        let key_path = self.dir.join(KEY_FILE);
        let mut paths = vec![key_path];
        let old_key_path = self.dir.join("r3v3rs3.key.old");
        if fs::try_exists(&old_key_path).await? {
            paths.push(old_key_path);
        }
        let loaded = std::sync::Arc::new(load_keys(&paths).await?);
        *guard = Some(loaded.clone());
        Ok(loaded)
    }

    /// Invalidates cached encryption keys so subsequent operations reload them.
    pub async fn invalidate_secrets(&self) {
        let mut guard = self.secrets.write().await;
        *guard = None;
    }
}

async fn ensure_key_files(dir: &Path) -> anyhow::Result<()> {
    ensure_primary_key(dir).await?;
    ensure_old_key(dir).await
}

async fn ensure_primary_key(dir: &Path) -> anyhow::Result<()> {
    let key_path = dir.join(KEY_FILE);
    let legacy_key = dir.join("platform.key");
    if fs::try_exists(&key_path).await? {
        return Ok(());
    }
    if fs::try_exists(&legacy_key).await? {
        fs::rename(&legacy_key, &key_path).await?;
        info!("migrated legacy platform.key to r3v3rs3.key");
    } else {
        write_new_key_file(&key_path).await?;
    }
    Ok(())
}

async fn ensure_old_key(dir: &Path) -> anyhow::Result<()> {
    let old_key = dir.join("r3v3rs3.key.old");
    let legacy_old = dir.join("platform.key.old");
    if !fs::try_exists(&old_key).await? && fs::try_exists(&legacy_old).await? {
        fs::rename(&legacy_old, &old_key).await?;
        info!("migrated legacy platform.key.old to r3v3rs3.key.old");
    }
    Ok(())
}

async fn migrate_legacy_platform_db(dir: &Path, db_path: &Path) -> anyhow::Result<()> {
    let legacy_db = dir.join("platform.db");
    if !fs::try_exists(db_path).await? && fs::try_exists(&legacy_db).await? {
        fs::rename(&legacy_db, db_path).await?;
        let wal = dir.join("platform.db-wal");
        let shm = dir.join("platform.db-shm");
        if fs::try_exists(&wal).await? {
            let _ = fs::rename(&wal, dir.join(format!("{DATABASE_FILE}-wal"))).await;
        }
        if fs::try_exists(&shm).await? {
            let _ = fs::rename(&shm, dir.join(format!("{DATABASE_FILE}-shm"))).await;
        }
        info!("migrated platform.db to r3v3rs3.db");
    }
    Ok(())
}

async fn connect_db(db_path: &Path) -> anyhow::Result<SqlitePool> {
    let options = SqliteConnectOptions::new()
        .filename(db_path)
        .create_if_missing(true)
        .journal_mode(SqliteJournalMode::Wal)
        .synchronous(SqliteSynchronous::Normal)
        .busy_timeout(std::time::Duration::from_secs(5))
        .log_statements(log::LevelFilter::Trace);
    Ok(SqlitePool::connect_with(options).await?)
}

async fn init_schema(pool: &SqlitePool) -> anyhow::Result<()> {
    for query in SCHEMA {
        sqlx::query(*query).execute(pool).await?;
    }
    Ok(())
}

async fn migrate_legacy_files(
    dir: &Path,
    pool: &SqlitePool,
    keys: &ClusterKeys,
) -> anyhow::Result<()> {
    migrate_config_file(dir, pool).await?;
    migrate_ports_file(dir, pool).await?;
    migrate_proxies_file(dir, pool).await?;
    migrate_access_lists_file(dir, pool).await?;
    migrate_acme_file(dir, pool).await?;
    migrate_dns_file(dir, pool, keys).await?;
    migrate_accounts_file(dir, pool).await?;
    migrate_notifications_file(dir, pool).await?;
    migrate_disk_certs(dir, pool).await?;
    Ok(())
}

async fn migrate_config_file(dir: &Path, pool: &SqlitePool) -> anyhow::Result<()> {
    let file = dir.join("config.toml");
    if !fs::try_exists(&file).await? {
        return Ok(());
    }
    let content = fs::read_to_string(&file).await?;
    let doc = toml::from_str::<Versioned<AppConfig>>(&content)
        .with_context(|| format!("failed to parse {}", file.display()))?;
    let json = serde_json::to_string(&doc.data)?;
    let now = crate::clock::unix_ms() as i64;
    sqlx::query("INSERT OR REPLACE INTO server_config (id, data, updated_at) VALUES (1, ?, ?)")
        .bind(json)
        .bind(now)
        .execute(pool)
        .await?;
    let _ = fs::rename(&file, dir.join("config.toml.migrated")).await;
    info!("migrated config.toml to database");
    Ok(())
}

async fn migrate_ports_file(dir: &Path, pool: &SqlitePool) -> anyhow::Result<()> {
    let file = dir.join("ports.toml");
    if !fs::try_exists(&file).await? {
        return Ok(());
    }
    let content = fs::read_to_string(&file).await?;
    let table = toml::from_str::<Versioned<IndexMap<ShortId, Port>>>(&content)
        .with_context(|| format!("failed to parse {}", file.display()))?;
    let now = crate::clock::unix_ms() as i64;
    for (id, port) in table.data {
        let entry = PortEntry::from((id, port));
        let json = serde_json::to_string(&entry)?;
        sqlx::query(
            "INSERT OR REPLACE INTO ports (id, listen_addr, name, data, updated_at) VALUES (?, ?, ?, ?, ?)",
        )
        .bind(entry.id.to_string())
        .bind(entry.port.listen.to_string())
        .bind(&entry.port.name)
        .bind(json)
        .bind(now)
        .execute(pool)
        .await?;
    }
    let _ = fs::rename(&file, dir.join("ports.toml.migrated")).await;
    info!("migrated ports.toml to database");
    Ok(())
}

async fn migrate_proxies_file(dir: &Path, pool: &SqlitePool) -> anyhow::Result<()> {
    let file = dir.join("proxies.toml");
    if !fs::try_exists(&file).await? {
        return Ok(());
    }
    let content = fs::read_to_string(&file).await?;
    let table = toml::from_str::<Versioned<IndexMap<ShortId, Proxy>>>(&content)
        .with_context(|| format!("failed to parse {}", file.display()))?;
    let now = crate::clock::unix_ms() as i64;
    for (id, proxy) in table.data {
        let entry = ProxyEntry::from((id, proxy));
        let json = serde_json::to_string(&entry)?;
        sqlx::query(
            "INSERT OR REPLACE INTO proxies (id, name, data, updated_at) VALUES (?, ?, ?, ?)",
        )
        .bind(entry.id.to_string())
        .bind(&entry.proxy.name)
        .bind(json)
        .bind(now)
        .execute(pool)
        .await?;
    }
    let _ = fs::rename(&file, dir.join("proxies.toml.migrated")).await;
    info!("migrated proxies.toml to database");
    Ok(())
}

async fn migrate_access_lists_file(dir: &Path, pool: &SqlitePool) -> anyhow::Result<()> {
    let file = dir.join("access_lists.toml");
    if !fs::try_exists(&file).await? {
        return Ok(());
    }
    let content = fs::read_to_string(&file).await?;
    if let Ok(table) = toml::from_str::<Versioned<IndexMap<ShortId, AccessList>>>(&content) {
        let now = crate::clock::unix_ms() as i64;
        for (id, list) in table.data {
            let entry = AccessListEntry { id, list };
            let json = serde_json::to_string(&entry)?;
            sqlx::query(
                "INSERT OR REPLACE INTO access_lists (id, name, data, updated_at) VALUES (?, ?, ?, ?)",
            )
            .bind(entry.id.to_string())
            .bind(&entry.list.name)
            .bind(json)
            .bind(now)
            .execute(pool)
            .await?;
        }
        let _ = fs::rename(&file, dir.join("access_lists.toml.migrated")).await;
        info!("migrated access_lists.toml to database");
    }
    Ok(())
}

async fn migrate_acme_file(dir: &Path, pool: &SqlitePool) -> anyhow::Result<()> {
    let file = dir.join("acme.toml");
    if !fs::try_exists(&file).await? {
        return Ok(());
    }
    let content = fs::read_to_string(&file).await?;
    if let Ok(table) =
        toml::from_str::<Versioned<IndexMap<ShortId, crate::certs::acme::AcmeAccount>>>(&content)
    {
        let now = crate::clock::unix_ms() as i64;
        for (id, account) in table.data {
            let entry: AcmeEntry = (id, account).into();
            let json = serde_json::to_string(&entry)?;
            let domain = entry
                .acme
                .identifiers
                .first()
                .map(|s| s.to_string())
                .unwrap_or_default();
            sqlx::query(
                "INSERT OR REPLACE INTO acme_configs (id, domain, data, updated_at) VALUES (?, ?, ?, ?)",
            )
            .bind(id.to_string())
            .bind(domain)
            .bind(json)
            .bind(now)
            .execute(pool)
            .await?;
        }
        let _ = fs::rename(&file, dir.join("acme.toml.migrated")).await;
        info!("migrated acme.toml to database");
    }
    Ok(())
}

async fn migrate_dns_file(
    dir: &Path,
    pool: &SqlitePool,
    _keys: &ClusterKeys,
) -> anyhow::Result<()> {
    let file = dir.join("dns-credentials.toml");
    if !fs::try_exists(&file).await? {
        return Ok(());
    }
    let content = fs::read_to_string(&file).await?;
    if let Ok(table) = toml::from_str::<Versioned<IndexMap<ShortId, StoredDnsCredential>>>(&content)
    {
        let now = crate::clock::unix_ms() as i64;
        for (id, cred) in table.data {
            sqlx::query(
                "INSERT OR REPLACE INTO dns_credentials (id, name, provider, sealed, created_at) VALUES (?, ?, ?, ?, ?)",
            )
            .bind(id.to_string())
            .bind(&cred.name)
            .bind(&cred.provider)
            .bind(&cred.sealed)
            .bind(now)
            .execute(pool)
            .await?;
        }
        let _ = fs::rename(&file, dir.join("dns-credentials.toml.migrated")).await;
        info!("migrated dns-credentials.toml to database");
    }
    Ok(())
}

async fn migrate_accounts_file(dir: &Path, pool: &SqlitePool) -> anyhow::Result<()> {
    let file = dir.join("accounts.toml");
    if !fs::try_exists(&file).await? {
        return Ok(());
    }
    let content = fs::read_to_string(&file).await?;
    if let Ok(table) = toml::from_str::<Versioned<HashMap<String, Account>>>(&content) {
        let now = crate::clock::unix_ms() as i64;
        for (name, account) in table.data {
            let json = serde_json::to_string(&account)?;
            sqlx::query(
                "INSERT OR REPLACE INTO accounts (username, data, updated_at) VALUES (?, ?, ?)",
            )
            .bind(name)
            .bind(json)
            .bind(now)
            .execute(pool)
            .await?;
        }
        let _ = fs::rename(&file, dir.join("accounts.toml.migrated")).await;
        info!("migrated accounts.toml to database");
    }
    Ok(())
}

async fn migrate_notifications_file(dir: &Path, pool: &SqlitePool) -> anyhow::Result<()> {
    let file = dir.join("notifications.json");
    if !fs::try_exists(&file).await? {
        return Ok(());
    }
    let bytes = fs::read(&file).await?;
    if let Ok(keys) = serde_json::from_slice::<HashSet<String>>(&bytes) {
        let now = crate::clock::unix_ms() as i64;
        for key in keys {
            sqlx::query("INSERT OR IGNORE INTO sent_notifications (id, sent_at) VALUES (?, ?)")
                .bind(key)
                .bind(now)
                .execute(pool)
                .await?;
        }
        let _ = fs::rename(&file, dir.join("notifications.json.migrated")).await;
        info!("migrated notifications.json to database");
    }
    Ok(())
}

async fn migrate_disk_certs(dir: &Path, pool: &SqlitePool) -> anyhow::Result<()> {
    let certs_dir = dir.join("certs");
    if !fs::try_exists(&certs_dir).await? {
        return Ok(());
    }
    for kind in [CertKind::Server, CertKind::Client, CertKind::Root] {
        migrate_kind_certs(dir, pool, &certs_dir, kind).await?;
    }
    Ok(())
}

async fn migrate_kind_certs(
    dir: &Path,
    pool: &SqlitePool,
    certs_dir: &Path,
    kind: CertKind,
) -> anyhow::Result<()> {
    let kind_dir = certs_dir.join(kind.to_string());
    if !fs::try_exists(&kind_dir).await? {
        return Ok(());
    }
    let walker = globwalk::GlobWalkerBuilder::from_patterns(&kind_dir, &["*/cert.pem"])
        .build()?
        .filter_map(Result::ok);
    for entry in walker {
        migrate_one_cert(dir, pool, entry.path(), kind).await?;
    }
    Ok(())
}

async fn migrate_one_cert(
    dir: &Path,
    pool: &SqlitePool,
    cert_path: &Path,
    kind: CertKind,
) -> anyhow::Result<()> {
    let key_path = cert_path.with_file_name("key.pem");
    let chain = fs::read(cert_path).await.unwrap_or_default();
    let key = if fs::try_exists(&key_path).await.unwrap_or(false) {
        Some(fs::read(&key_path).await.unwrap_or_default())
    } else {
        None
    };
    if let Ok(cert) = Cert::new(kind, chain, key) {
        insert_cert_row(dir, pool, &cert).await?;
    }
    Ok(())
}

async fn insert_cert_row(dir: &Path, pool: &SqlitePool, cert: &Cert) -> anyhow::Result<()> {
    let rel_cert = format!("certs/{}/{}/cert.pem", cert.kind, cert.id());
    let rel_key = if cert.pem_key.is_some() {
        Some(format!("certs/{}/{}/key.pem", cert.kind, cert.id()))
    } else {
        None
    };
    let san_domains = cert
        .san
        .iter()
        .map(|s| s.to_string())
        .collect::<Vec<_>>()
        .join(",");
    let not_before = cert.not_before.timestamp();
    let not_after = cert.not_after.timestamp();
    let now = crate::clock::unix_ms() as i64;
    sqlx::query(
        "INSERT OR REPLACE INTO certificates (id, kind, san_domains, issuer, not_before, not_after, cert_path, key_path, fingerprint, created_at)
         VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?)",
    )
    .bind(cert.id().to_string())
    .bind(cert.kind.to_string())
    .bind(san_domains)
    .bind(&cert.issuer)
    .bind(not_before)
    .bind(not_after)
    .bind(rel_cert)
    .bind(rel_key)
    .bind(&cert.fingerprint)
    .bind(now)
    .execute(pool)
    .await?;
    let _ = dir;
    Ok(())
}

#[async_trait::async_trait]
impl Storage for SqliteStorage {
    async fn ensure_writable(&self) -> Result<(), Error> {
        sqlx::query("SELECT 1")
            .execute(&self.pool)
            .await
            .map_err(|_| Error::FailedToSaveConfig)?;
        Ok(())
    }

    async fn invalidate_secrets(&self) {
        self.invalidate_secrets().await;
    }

    async fn save_app_config(&self, config: &AppConfig) -> Result<(), Error> {
        let json = serde_json::to_string(config).map_err(|_| Error::FailedToSaveConfig)?;
        let now = crate::clock::unix_ms() as i64;
        sqlx::query(
            "INSERT INTO server_config (id, data, updated_at) VALUES (1, ?, ?)
             ON CONFLICT(id) DO UPDATE SET data = excluded.data, updated_at = excluded.updated_at",
        )
        .bind(json)
        .bind(now)
        .execute(&self.pool)
        .await
        .map_err(|_| Error::FailedToSaveConfig)?;
        Ok(())
    }

    async fn load_app_config(&self) -> AppConfig {
        let row = sqlx::query("SELECT data FROM server_config WHERE id = 1")
            .fetch_optional(&self.pool)
            .await;
        match row {
            Ok(Some(r)) => {
                let data: String = r.get(0);
                serde_json::from_str(&data).unwrap_or_default()
            }
            _ => AppConfig::default(),
        }
    }

    async fn save_ports(&self, entries: &[PortEntry]) -> Result<(), Error> {
        let mut tx = self
            .pool
            .begin()
            .await
            .map_err(|_| Error::FailedToSaveConfig)?;
        sqlx::query("DELETE FROM ports")
            .execute(&mut *tx)
            .await
            .map_err(|_| Error::FailedToSaveConfig)?;
        let now = crate::clock::unix_ms() as i64;
        for entry in entries {
            let json = serde_json::to_string(entry).map_err(|_| Error::FailedToSaveConfig)?;
            sqlx::query(
                "INSERT INTO ports (id, listen_addr, name, data, updated_at) VALUES (?, ?, ?, ?, ?)",
            )
            .bind(entry.id.to_string())
            .bind(entry.port.listen.to_string())
            .bind(&entry.port.name)
            .bind(json)
            .bind(now)
            .execute(&mut *tx)
            .await
            .map_err(|_| Error::FailedToSaveConfig)?;
        }
        tx.commit().await.map_err(|_| Error::FailedToSaveConfig)?;
        Ok(())
    }

    async fn load_ports(&self) -> Vec<PortEntry> {
        load_json_list(&self.pool, "SELECT data FROM ports ORDER BY rowid ASC").await
    }

    async fn save_proxies(&self, proxies: &[ProxyEntry]) -> Result<(), Error> {
        let mut tx = self
            .pool
            .begin()
            .await
            .map_err(|_| Error::FailedToSaveConfig)?;
        sqlx::query("DELETE FROM proxies")
            .execute(&mut *tx)
            .await
            .map_err(|_| Error::FailedToSaveConfig)?;
        let now = crate::clock::unix_ms() as i64;
        for entry in proxies {
            let json = serde_json::to_string(entry).map_err(|_| Error::FailedToSaveConfig)?;
            sqlx::query("INSERT INTO proxies (id, name, data, updated_at) VALUES (?, ?, ?, ?)")
                .bind(entry.id.to_string())
                .bind(&entry.proxy.name)
                .bind(json)
                .bind(now)
                .execute(&mut *tx)
                .await
                .map_err(|_| Error::FailedToSaveConfig)?;
        }
        tx.commit().await.map_err(|_| Error::FailedToSaveConfig)?;
        Ok(())
    }

    async fn load_proxies(&self) -> Vec<ProxyEntry> {
        load_json_list(&self.pool, "SELECT data FROM proxies ORDER BY rowid ASC").await
    }

    async fn save_access_lists(&self, lists: &[AccessListEntry]) -> Result<(), Error> {
        let mut tx = self
            .pool
            .begin()
            .await
            .map_err(|_| Error::FailedToSaveConfig)?;
        sqlx::query("DELETE FROM access_lists")
            .execute(&mut *tx)
            .await
            .map_err(|_| Error::FailedToSaveConfig)?;
        let now = crate::clock::unix_ms() as i64;
        for entry in lists {
            let json = serde_json::to_string(entry).map_err(|_| Error::FailedToSaveConfig)?;
            sqlx::query(
                "INSERT INTO access_lists (id, name, data, updated_at) VALUES (?, ?, ?, ?)",
            )
            .bind(entry.id.to_string())
            .bind(&entry.list.name)
            .bind(json)
            .bind(now)
            .execute(&mut *tx)
            .await
            .map_err(|_| Error::FailedToSaveConfig)?;
        }
        tx.commit().await.map_err(|_| Error::FailedToSaveConfig)?;
        Ok(())
    }

    async fn load_access_lists(&self) -> Vec<AccessListEntry> {
        load_json_list(
            &self.pool,
            "SELECT data FROM access_lists ORDER BY rowid ASC",
        )
        .await
    }

    async fn save_cert(&self, cert: &Cert) -> Result<(), Error> {
        let cert_dir = self
            .dir
            .join("certs")
            .join(cert.kind.to_string())
            .join(cert.id().to_string());
        fs::create_dir_all(&cert_dir)
            .await
            .map_err(|_| Error::FailedToSaveConfig)?;
        fs::write(cert_dir.join("cert.pem"), &cert.pem_chain)
            .await
            .map_err(|_| Error::FailedToSaveConfig)?;
        if let Some(key) = &cert.pem_key {
            let key_str = String::from_utf8_lossy(key).into_owned();
            super::file::write_private(&cert_dir.join("key.pem"), key_str)
                .await
                .map_err(|_| Error::FailedToSaveConfig)?;
        }
        insert_cert_row(&self.dir, &self.pool, cert)
            .await
            .map_err(|_| Error::FailedToSaveConfig)?;
        Ok(())
    }

    async fn delete_cert(&self, id: ShortId) -> Result<(), Error> {
        let row = sqlx::query("SELECT kind, cert_path FROM certificates WHERE id = ?")
            .bind(id.to_string())
            .fetch_optional(&self.pool)
            .await
            .map_err(|_| Error::FailedToSaveConfig)?;
        if let Some(r) = row {
            let kind: String = r.get(0);
            let cert_dir = self.dir.join("certs").join(kind).join(id.to_string());
            let _ = fs::remove_dir_all(&cert_dir).await;
        }
        sqlx::query("DELETE FROM certificates WHERE id = ?")
            .bind(id.to_string())
            .execute(&self.pool)
            .await
            .map_err(|_| Error::FailedToSaveConfig)?;
        Ok(())
    }

    async fn load_certs(&self) -> Vec<Arc<Cert>> {
        let rows = sqlx::query(
            "SELECT id, kind, cert_path, key_path FROM certificates ORDER BY created_at ASC",
        )
        .fetch_all(&self.pool)
        .await;
        let mut certs = Vec::new();
        if let Ok(rows) = rows {
            for row in rows {
                let kind_str: String = row.get(1);
                let cert_path_str: String = row.get(2);
                let key_path_opt: Option<String> = row.get(3);
                let kind = match kind_str.as_str() {
                    "client" => CertKind::Client,
                    "root" => CertKind::Root,
                    _ => CertKind::Server,
                };
                let cert_file = self.dir.join(&cert_path_str);
                let chain = fs::read(&cert_file).await.unwrap_or_default();
                let key = match key_path_opt {
                    Some(k) => fs::read(self.dir.join(&k)).await.ok(),
                    None => None,
                };
                if let Ok(cert) = Cert::new(kind, chain, key) {
                    certs.push(Arc::new(cert));
                }
            }
        }
        certs
    }

    async fn save_acme(&self, acme: &AcmeEntry) -> Result<(), Error> {
        let json = serde_json::to_string(acme).map_err(|_| Error::FailedToSaveConfig)?;
        let domain = acme
            .acme
            .identifiers
            .first()
            .map(|s| s.to_string())
            .unwrap_or_default();
        let now = crate::clock::unix_ms() as i64;
        sqlx::query(
            "INSERT INTO acme_configs (id, domain, data, updated_at) VALUES (?, ?, ?, ?)
             ON CONFLICT(id) DO UPDATE SET domain = excluded.domain, data = excluded.data, updated_at = excluded.updated_at",
        )
        .bind(acme.id.to_string())
        .bind(domain)
        .bind(json)
        .bind(now)
        .execute(&self.pool)
        .await
        .map_err(|_| Error::FailedToSaveConfig)?;
        Ok(())
    }

    async fn delete_acme(&self, id: ShortId) -> Result<(), Error> {
        sqlx::query("DELETE FROM acme_configs WHERE id = ?")
            .bind(id.to_string())
            .execute(&self.pool)
            .await
            .map_err(|_| Error::FailedToSaveConfig)?;
        Ok(())
    }

    async fn load_acmes(&self) -> Vec<AcmeEntry> {
        load_json_list(
            &self.pool,
            "SELECT data FROM acme_configs ORDER BY rowid ASC",
        )
        .await
    }

    async fn save_dns_credential(
        &self,
        id: ShortId,
        name: &str,
        provider: &DnsProvider,
    ) -> Result<(), Error> {
        let keys = self
            .secret_keys()
            .await
            .map_err(|_| Error::FailedToSaveConfig)?;
        let stored = StoredDnsCredential::seal(id, name, provider, &keys)
            .map_err(|_| Error::FailedToSaveConfig)?;
        let now = crate::clock::unix_ms() as i64;
        sqlx::query(
            "INSERT INTO dns_credentials (id, name, provider, sealed, created_at) VALUES (?, ?, ?, ?, ?)
             ON CONFLICT(id) DO UPDATE SET name = excluded.name, provider = excluded.provider, sealed = excluded.sealed",
        )
        .bind(id.to_string())
        .bind(name)
        .bind(&stored.provider)
        .bind(&stored.sealed)
        .bind(now)
        .execute(&self.pool)
        .await
        .map_err(|_| Error::FailedToSaveConfig)?;
        Ok(())
    }

    async fn delete_dns_credential(&self, id: ShortId) -> Result<(), Error> {
        sqlx::query("DELETE FROM dns_credentials WHERE id = ?")
            .bind(id.to_string())
            .execute(&self.pool)
            .await
            .map_err(|_| Error::FailedToSaveConfig)?;
        Ok(())
    }

    async fn load_dns_credentials(&self) -> Result<Vec<DnsCredentialInfo>, Error> {
        let rows = sqlx::query("SELECT id, name, provider FROM dns_credentials ORDER BY name ASC")
            .fetch_all(&self.pool)
            .await
            .map_err(|_| Error::FailedToLoadDnsCredentials)?;
        let mut list = Vec::new();
        for r in rows {
            let id_str: String = r.get(0);
            let name: String = r.get(1);
            let provider: String = r.get(2);
            if let Ok(id) = id_str.parse() {
                list.push(DnsCredentialInfo { id, name, provider });
            }
        }
        Ok(list)
    }

    async fn dns_provider_of(&self, id: ShortId) -> Result<Option<DnsProvider>, Error> {
        let row = sqlx::query("SELECT name, provider, sealed FROM dns_credentials WHERE id = ?")
            .bind(id.to_string())
            .fetch_optional(&self.pool)
            .await
            .map_err(|_| Error::FailedToLoadDnsCredentials)?;
        let Some(r) = row else { return Ok(None) };
        let name: String = r.get(0);
        let provider_name: String = r.get(1);
        let sealed: String = r.get(2);
        let stored = StoredDnsCredential {
            name,
            provider: provider_name,
            sealed,
        };
        let keys = self
            .secret_keys()
            .await
            .map_err(|_| Error::FailedToLoadDnsCredentials)?;
        let provider = stored
            .open(id, &keys)
            .map_err(|_| Error::FailedToLoadDnsCredentials)?;
        Ok(Some(provider))
    }

    async fn add_account(
        &self,
        name: &str,
        password: &str,
        totp: bool,
        role: Role,
    ) -> Result<Account, Error> {
        let mut accounts = self.load_accounts().await?;
        let existing = accounts.get(name);
        let mut account =
            account::new_account(password, totp).map_err(|_| Error::FailedToCreateAccount)?;
        account.role = role;
        if let Some(prev) = existing {
            account.credentials_changed_at =
                account.credentials_changed_at.max(prev.credentials_changed_at);
        }
        accounts.insert(name.to_string(), account.clone());
        self.save_accounts(&accounts).await?;
        Ok(account)
    }

    async fn verify_account(&self, request: LoginRequest) -> Result<LoginResponse, Error> {
        let accounts = self.load_accounts().await?;
        account::verify(accounts.get(&request.username), request)
    }

    async fn load_accounts(&self) -> Result<HashMap<String, Account>, Error> {
        let rows = sqlx::query("SELECT username, data FROM accounts")
            .fetch_all(&self.pool)
            .await
            .map_err(|_| Error::FailedToLoadAccounts)?;
        let mut map = HashMap::new();
        for r in rows {
            let username: String = r.get(0);
            let data: String = r.get(1);
            if let Ok(acc) = serde_json::from_str::<Account>(&data) {
                map.insert(username, acc);
            }
        }
        Ok(map)
    }

    async fn save_accounts(&self, accounts: &HashMap<String, Account>) -> Result<(), Error> {
        let mut tx = self
            .pool
            .begin()
            .await
            .map_err(|_| Error::FailedToSaveConfig)?;
        sqlx::query("DELETE FROM accounts")
            .execute(&mut *tx)
            .await
            .map_err(|_| Error::FailedToSaveConfig)?;
        let now = crate::clock::unix_ms() as i64;
        for (name, acc) in accounts {
            let json = serde_json::to_string(acc).map_err(|_| Error::FailedToSaveConfig)?;
            sqlx::query("INSERT INTO accounts (username, data, updated_at) VALUES (?, ?, ?)")
                .bind(name)
                .bind(json)
                .bind(now)
                .execute(&mut *tx)
                .await
                .map_err(|_| Error::FailedToSaveConfig)?;
        }
        tx.commit().await.map_err(|_| Error::FailedToSaveConfig)?;
        Ok(())
    }

    async fn save_cdn_ranges(&self, ranges: &CdnRanges) -> Result<(), Error> {
        let path = self.dir.join("cdn-ranges.json");
        let bytes = serde_json::to_vec(ranges).map_err(|_| Error::FailedToSaveConfig)?;
        fs::write(&path, bytes)
            .await
            .map_err(|_| Error::FailedToSaveConfig)?;
        Ok(())
    }

    async fn load_cdn_ranges(&self) -> Option<CdnRanges> {
        let path = self.dir.join("cdn-ranges.json");
        let bytes = fs::read(&path).await.ok()?;
        serde_json::from_slice(&bytes).ok()
    }

    async fn save_app_store(&self, catalog: &StoreCatalog) -> Result<(), Error> {
        let path = self.dir.join("app-store.json");
        let bytes = serde_json::to_vec(catalog).map_err(|_| Error::FailedToSaveConfig)?;
        fs::write(&path, bytes)
            .await
            .map_err(|_| Error::FailedToSaveConfig)?;
        Ok(())
    }

    async fn load_app_store(&self) -> Option<StoreCatalog> {
        let path = self.dir.join("app-store.json");
        let bytes = fs::read(&path).await.ok()?;
        serde_json::from_slice(&bytes).ok()
    }

    async fn save_engines(
        &self,
        catalog: &r3v3rs3_api::platform::EnginesCatalog,
    ) -> Result<(), Error> {
        let path = self.dir.join("engines.json");
        let bytes = serde_json::to_vec(catalog).map_err(|_| Error::FailedToSaveConfig)?;
        fs::write(&path, bytes)
            .await
            .map_err(|_| Error::FailedToSaveConfig)?;
        Ok(())
    }

    async fn load_engines(&self) -> Option<r3v3rs3_api::platform::EnginesCatalog> {
        let path = self.dir.join("engines.json");
        let bytes = fs::read(&path).await.ok()?;
        serde_json::from_slice(&bytes).ok()
    }

    async fn save_sent_notifications(&self, keys: &HashSet<String>) -> Result<(), Error> {
        let mut tx = self
            .pool
            .begin()
            .await
            .map_err(|_| Error::FailedToSaveConfig)?;
        let now = crate::clock::unix_ms() as i64;
        for key in keys {
            sqlx::query("INSERT OR IGNORE INTO sent_notifications (id, sent_at) VALUES (?, ?)")
                .bind(key)
                .bind(now)
                .execute(&mut *tx)
                .await
                .map_err(|_| Error::FailedToSaveConfig)?;
        }
        tx.commit().await.map_err(|_| Error::FailedToSaveConfig)?;
        Ok(())
    }

    async fn load_sent_notifications(&self) -> HashSet<String> {
        let rows = sqlx::query("SELECT id FROM sent_notifications")
            .fetch_all(&self.pool)
            .await;
        match rows {
            Ok(r) => r.into_iter().map(|row| row.get(0)).collect(),
            Err(_) => HashSet::new(),
        }
    }

    fn audit_store(self: Arc<Self>) -> Option<Arc<dyn AuditStore>> {
        Some(Arc::new(crate::audit::SqliteAuditStore::new(
            self.db_path.clone(),
        )))
    }
}

async fn load_json_list<T: DeserializeOwned>(pool: &SqlitePool, query: &'static str) -> Vec<T> {
    let rows = sqlx::query(query).fetch_all(pool).await;
    match rows {
        Ok(rows) => rows
            .into_iter()
            .filter_map(|r| {
                let data: String = r.get(0);
                serde_json::from_str(&data).ok()
            })
            .collect(),
        Err(_) => Vec::new(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use r3v3rs3_api::auth::LoginMethod;

    fn temp_test_dir(name: &str) -> PathBuf {
        let rand_hex = hex::encode(rand::random::<[u8; 8]>());
        std::env::temp_dir().join(format!("r3v3rs3-sqlite-{name}-{rand_hex}"))
    }

    async fn check_config_and_ports(storage: &SqliteStorage) -> anyhow::Result<()> {
        let cfg = AppConfig {
            public_url: Some("https://example.com".parse()?),
            ..Default::default()
        };
        storage.save_app_config(&cfg).await?;
        let loaded_cfg = storage.load_app_config().await;
        assert_eq!(loaded_cfg.public_url, cfg.public_url);

        let port_entry = PortEntry::from((
            "p1".parse()?,
            Port {
                active: true,
                name: "http".into(),
                listen: "/ip4/0.0.0.0/tcp/80".parse()?,
                opts: Default::default(),
            },
        ));
        storage
            .save_ports(std::slice::from_ref(&port_entry))
            .await?;
        let ports = storage.load_ports().await;
        assert_eq!(ports.len(), 1);
        assert_eq!(ports[0].id, port_entry.id);
        Ok(())
    }

    async fn check_proxies_and_accounts(storage: &SqliteStorage) -> anyhow::Result<()> {
        let proxy_entry = ProxyEntry::from((
            "px1".parse()?,
            Proxy {
                active: true,
                name: "web-app".into(),
                ports: vec![],
                kind: r3v3rs3_api::proxy::ProxyKind::Tcp(Default::default()),
            },
        ));
        storage
            .save_proxies(std::slice::from_ref(&proxy_entry))
            .await?;
        let proxies = storage.load_proxies().await;
        assert_eq!(proxies.len(), 1);
        assert_eq!(proxies[0].proxy.name, "web-app");

        storage
            .add_account("admin", "super-secret-pass", false, Role::Admin)
            .await?;
        let accounts = storage.load_accounts().await?;
        assert!(accounts.contains_key("admin"));
        let prev_changed = accounts["admin"].credentials_changed_at;
        assert!(prev_changed > 0);

        let login_res = storage
            .verify_account(LoginRequest {
                username: "admin".into(),
                method: LoginMethod::Password {
                    password: "super-secret-pass".into(),
                },
                insecure: false,
            })
            .await?;
        assert!(matches!(login_res, LoginResponse::Success));

        storage
            .add_account("admin", "new-secret-pass", false, Role::Admin)
            .await?;
        let updated_accounts = storage.load_accounts().await?;
        assert!(updated_accounts["admin"].credentials_changed_at >= prev_changed);

        let old_login = storage
            .verify_account(LoginRequest {
                username: "admin".into(),
                method: LoginMethod::Password {
                    password: "super-secret-pass".into(),
                },
                insecure: false,
            })
            .await;
        assert!(matches!(old_login, Err(Error::InvalidLoginCredentials)));

        let new_login = storage
            .verify_account(LoginRequest {
                username: "admin".into(),
                method: LoginMethod::Password {
                    password: "new-secret-pass".into(),
                },
                insecure: false,
            })
            .await?;
        assert!(matches!(new_login, LoginResponse::Success));
        Ok(())
    }

    async fn check_dns_and_notifications(storage: &SqliteStorage) -> anyhow::Result<()> {
        let cred_id: ShortId = "dns1".parse()?;
        let provider = DnsProvider::Token(r3v3rs3_api::acme::TokenProvider {
            provider: r3v3rs3_api::acme::TokenApi::Cloudflare,
            api_token: "mock-cf-token".into(),
            api_url: None,
        });
        storage
            .save_dns_credential(cred_id, "mock", &provider)
            .await?;
        let creds = storage.load_dns_credentials().await?;
        assert_eq!(creds.len(), 1);
        assert_eq!(creds[0].name, "mock");
        let opened_provider = storage.dns_provider_of(cred_id).await?;
        assert!(opened_provider.is_some());

        let notifs = HashSet::from(["alert-1".to_string(), "alert-2".to_string()]);
        storage.save_sent_notifications(&notifs).await?;
        let loaded_notifs = storage.load_sent_notifications().await;
        assert!(loaded_notifs.contains("alert-1"));
        assert!(loaded_notifs.contains("alert-2"));
        Ok(())
    }

    #[tokio::test]
    async fn test_sqlite_storage_lifecycle() -> anyhow::Result<()> {
        let dir = temp_test_dir("lifecycle");
        let storage = SqliteStorage::open(&dir).await?;
        check_config_and_ports(&storage).await?;
        check_proxies_and_accounts(&storage).await?;
        check_dns_and_notifications(&storage).await?;
        let _ = fs::remove_dir_all(&dir).await;
        Ok(())
    }

    async fn prepare_legacy_files(dir: &Path) -> anyhow::Result<()> {
        let config_toml = "version = \"2.4.3\"\n[telemetry]\n";
        fs::write(dir.join("config.toml"), config_toml).await?;

        let legacy_key = dir.join("platform.key");
        write_new_key_file(&legacy_key).await?;

        let account = crate::config::account::new_account("admin-pass-123", false)?;
        let accounts = HashMap::from([("operator".to_string(), account)]);
        let doc = toml::to_string(&Versioned {
            version: "2.4.3".into(),
            data: accounts,
        })?;
        super::super::file::write_private(&dir.join("accounts.toml"), doc).await?;
        Ok(())
    }

    async fn verify_migrated_state(dir: &Path, storage: &SqliteStorage) -> anyhow::Result<()> {
        assert!(fs::try_exists(dir.join("r3v3rs3.key")).await?);
        assert!(!fs::try_exists(dir.join("platform.key")).await?);
        assert!(fs::try_exists(dir.join("config.toml.migrated")).await?);
        assert!(fs::try_exists(dir.join("accounts.toml.migrated")).await?);

        let loaded_accs = storage.load_accounts().await?;
        assert!(loaded_accs.contains_key("operator"));
        let resp = storage
            .verify_account(LoginRequest {
                username: "operator".into(),
                method: LoginMethod::Password {
                    password: "admin-pass-123".into(),
                },
                insecure: false,
            })
            .await?;
        assert!(matches!(resp, LoginResponse::Success));
        Ok(())
    }

    #[tokio::test]
    async fn test_sqlite_migration_from_files() -> anyhow::Result<()> {
        let dir = temp_test_dir("migrate");
        fs::create_dir_all(&dir).await?;
        prepare_legacy_files(&dir).await?;
        let storage = SqliteStorage::open(&dir).await?;
        verify_migrated_state(&dir, &storage).await?;
        let _ = fs::remove_dir_all(&dir).await;
        Ok(())
    }
}
