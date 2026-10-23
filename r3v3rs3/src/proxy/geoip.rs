//! MaxMind MMDB GeoIP country lookup and automated database updater.

use arc_swap::ArcSwapOption;
use flate2::read::GzDecoder;
use maxminddb::{Reader, geoip2::Country};
use r3v3rs3_api::app::GeoIpConfig;
use r3v3rs3_api::geoip::GeoIpStatus;
use sha2::{Digest, Sha256};
use std::io::{Cursor, Read};
use std::net::IpAddr;
use std::path::Path;
use std::sync::{Arc, RwLock};
use tar::Archive;
use tracing::{info, warn};

/// In-memory metadata of the loaded GeoIP database.
#[derive(Debug, Default, Clone)]
pub struct GeoIpLoadedMeta {
    pub path: Option<String>,
    pub file_size_bytes: Option<u64>,
    pub last_updated_ms: Option<u64>,
    pub build_epoch: Option<u64>,
    pub database_type: Option<String>,
}

/// Thread-safe GeoIP resolver with lock-free atomic hot-reloading.
pub struct GeoIpResolver {
    reader: ArcSwapOption<Reader<Vec<u8>>>,
    meta: RwLock<GeoIpLoadedMeta>,
}

impl Default for GeoIpResolver {
    fn default() -> Self {
        Self::empty()
    }
}

impl std::fmt::Debug for GeoIpResolver {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("GeoIpResolver")
            .field("is_loaded", &self.is_loaded())
            .finish()
    }
}

impl GeoIpResolver {
    pub fn empty() -> Self {
        Self {
            reader: ArcSwapOption::from(None),
            meta: RwLock::new(GeoIpLoadedMeta::default()),
        }
    }

    pub fn is_loaded(&self) -> bool {
        self.reader.load().is_some()
    }

    /// Reloads the in-memory reader and updates loaded file metadata.
    pub fn reload(&self, reader: Reader<Vec<u8>>, meta: GeoIpLoadedMeta) {
        self.reader.store(Some(Arc::new(reader)));
        if let Ok(mut guard) = self.meta.write() {
            *guard = meta;
        }
    }

    /// Loads a GeoIP MMDB database from file path and stores it.
    pub fn load_file<P: AsRef<Path>>(&self, path: P) -> Result<(), anyhow::Error> {
        let path_ref = path.as_ref();
        let reader = Reader::open_readfile(path_ref)?;
        let file_meta = std::fs::metadata(path_ref).ok();
        let file_size_bytes = file_meta.as_ref().map(|m| m.len());
        let last_updated_ms = file_meta
            .and_then(|m| m.modified().ok())
            .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
            .map(|d| d.as_millis() as u64);
        let build_epoch = Some(reader.metadata().build_epoch);
        let database_type = Some(reader.metadata().database_type.clone());

        info!(path = %path_ref.display(), db_type = ?database_type, "loaded GeoIP database");
        self.reload(
            reader,
            GeoIpLoadedMeta {
                path: Some(path_ref.display().to_string()),
                file_size_bytes,
                last_updated_ms,
                build_epoch,
                database_type,
            },
        );
        Ok(())
    }

    pub fn from_file<P: AsRef<Path>>(path: P) -> Result<Self, anyhow::Error> {
        let resolver = Self::empty();
        resolver.load_file(path)?;
        Ok(resolver)
    }

    pub fn from_bytes(bytes: Vec<u8>) -> Result<Self, anyhow::Error> {
        let reader = Reader::from_source(bytes)?;
        let resolver = Self::empty();
        let meta = GeoIpLoadedMeta {
            path: None,
            file_size_bytes: None,
            last_updated_ms: Some(crate::clock::unix_ms()),
            build_epoch: Some(reader.metadata().build_epoch),
            database_type: Some(reader.metadata().database_type.clone()),
        };
        resolver.reload(reader, meta);
        Ok(resolver)
    }

    /// Tries loading from configured path, or falls back to standard filenames in config_dir.
    pub fn reload_or_fallback(&self, configured: Option<&Path>, config_dir: &Path) {
        if let Some(path) = configured {
            match self.load_file(path) {
                Ok(()) => return,
                Err(err) => {
                    warn!(%err, path = %path.display(), "failed to open configured GeoIP database")
                }
            }
        }
        for filename in ["GeoLite2-Country.mmdb", "geoip.mmdb", "GeoIP2-Country.mmdb"] {
            let path = config_dir.join(filename);
            if path.exists() {
                match self.load_file(&path) {
                    Ok(()) => return,
                    Err(err) => {
                        warn!(%err, path = %path.display(), "failed to open GeoIP database file")
                    }
                }
            }
        }
    }

    pub fn load_or_fallback(configured: Option<&Path>, config_dir: &Path) -> Self {
        let resolver = Self::empty();
        resolver.reload_or_fallback(configured, config_dir);
        resolver
    }

    /// Returns current GeoIP database status and metadata.
    pub fn status(&self, config_dir: &Path, config: &GeoIpConfig) -> GeoIpStatus {
        let meta = match self.meta.read() {
            Ok(guard) => guard.clone(),
            Err(poisoned) => poisoned.into_inner().clone(),
        };
        let target_path = config
            .database
            .as_deref()
            .map(|p| p.display().to_string())
            .or_else(|| meta.path.clone())
            .or_else(|| {
                Some(
                    config_dir
                        .join("GeoLite2-Country.mmdb")
                        .display()
                        .to_string(),
                )
            });

        GeoIpStatus {
            loaded: self.is_loaded(),
            database_path: target_path,
            file_size_bytes: meta.file_size_bytes,
            last_updated_ms: meta.last_updated_ms,
            build_epoch: meta.build_epoch,
            database_type: meta.database_type,
            auto_update: config.auto_update,
            update_interval_days: config.update_interval_days,
            license_key_configured: config.license_key.is_some() || config.license_key_set,
            account_id: config.account_id.clone(),
        }
    }

    /// Downloads the latest GeoLite2-Country database from MaxMind, verifies SHA256,
    /// unpacks tar.gz and hot-reloads it.
    pub async fn update_database(
        &self,
        config_dir: &Path,
        config: &GeoIpConfig,
    ) -> Result<GeoIpStatus, anyhow::Error> {
        let license_key = match &config.license_key {
            Some(key) if !key.trim().is_empty() => key.trim(),
            _ => anyhow::bail!("MaxMind license key is required for GeoIP database updates"),
        };

        let dest_path = config
            .database
            .clone()
            .unwrap_or_else(|| config_dir.join("GeoLite2-Country.mmdb"));

        let client = reqwest::Client::builder()
            .timeout(std::time::Duration::from_secs(60))
            .build()?;

        let tar_bytes = download_and_verify_archive(&client, license_key).await?;
        let mmdb_bytes = extract_mmdb_bytes(&tar_bytes)?;
        let reader = save_and_verify_mmdb(&dest_path, &mmdb_bytes)?;
        let file_meta = std::fs::metadata(&dest_path).ok();

        let meta = GeoIpLoadedMeta {
            path: Some(dest_path.display().to_string()),
            file_size_bytes: file_meta.as_ref().map(|m| m.len()),
            last_updated_ms: Some(crate::clock::unix_ms()),
            build_epoch: Some(reader.metadata().build_epoch),
            database_type: Some(reader.metadata().database_type.clone()),
        };

        self.reload(reader, meta);
        info!(path = %dest_path.display(), "successfully downloaded and hot-reloaded GeoIP database");
        Ok(self.status(config_dir, config))
    }

    /// Looks up the two-letter ISO country code (e.g. "TR", "US", "DE") for the IP address.
    pub fn lookup_country(&self, ip: IpAddr) -> Option<String> {
        let reader = self.reader.load();
        let reader_ref = reader.as_ref()?;
        let lookup = reader_ref.lookup(ip).ok()?;
        let country: Country = lookup.decode().ok()??;
        let iso = country
            .country
            .iso_code
            .or(country.registered_country.iso_code)?;
        Some(iso.to_ascii_uppercase())
    }
}

/// Spawns a background task that periodically checks if a GeoIP database update is due.
pub fn spawn_periodic_update_task(
    command_tx: tokio::sync::mpsc::Sender<crate::command::ServerCommand>,
) {
    tokio::spawn(async move {
        tokio::time::sleep(std::time::Duration::from_secs(30)).await;
        let mut interval = tokio::time::interval(std::time::Duration::from_secs(3600 * 12));
        loop {
            interval.tick().await;
            if command_tx
                .send(crate::command::ServerCommand::CheckGeoIpUpdate)
                .await
                .is_err()
            {
                break;
            }
        }
    });
}

async fn download_and_verify_archive(
    client: &reqwest::Client,
    license_key: &str,
) -> Result<Vec<u8>, anyhow::Error> {
    let tar_url = format!(
        "https://download.maxmind.com/app/geoip_download?edition_id=GeoLite2-Country&license_key={license_key}&suffix=tar.gz"
    );
    let sha_url = format!(
        "https://download.maxmind.com/app/geoip_download?edition_id=GeoLite2-Country&license_key={license_key}&suffix=tar.gz.sha256"
    );

    let tar_bytes = fetch_url_bytes(client, &tar_url).await?;
    if let Ok(sha_text) = fetch_url_text(client, &sha_url).await {
        verify_sha256(&sha_text, &tar_bytes)?;
    }
    Ok(tar_bytes)
}

async fn fetch_url_bytes(client: &reqwest::Client, url: &str) -> Result<Vec<u8>, anyhow::Error> {
    let res = client.get(url).send().await?;
    if !res.status().is_success() {
        anyhow::bail!("download failed with HTTP {}", res.status());
    }
    Ok(res.bytes().await?.to_vec())
}

async fn fetch_url_text(client: &reqwest::Client, url: &str) -> Result<String, anyhow::Error> {
    let res = client.get(url).send().await?;
    if !res.status().is_success() {
        anyhow::bail!("download failed with HTTP {}", res.status());
    }
    Ok(res.text().await?)
}

fn verify_sha256(expected_text: &str, data: &[u8]) -> Result<(), anyhow::Error> {
    let mut hasher = Sha256::new();
    hasher.update(data);
    let calculated = hex::encode(hasher.finalize());
    let expected = expected_text.split_whitespace().next().unwrap_or("").trim();
    if !expected.is_empty() && !calculated.eq_ignore_ascii_case(expected) {
        anyhow::bail!("SHA256 checksum mismatch: expected {expected}, got {calculated}");
    }
    Ok(())
}

fn extract_mmdb_bytes(bytes: &[u8]) -> Result<Vec<u8>, anyhow::Error> {
    let gz = GzDecoder::new(Cursor::new(bytes));
    let mut archive = Archive::new(gz);
    for entry_result in archive.entries()? {
        let mut entry = entry_result?;
        let path = entry.path()?;
        if path.extension().and_then(|s| s.to_str()) == Some("mmdb") {
            let mut buf = Vec::new();
            entry.read_to_end(&mut buf)?;
            return Ok(buf);
        }
    }
    anyhow::bail!("no .mmdb file found in archive")
}

fn save_and_verify_mmdb(dest_path: &Path, bytes: &[u8]) -> Result<Reader<Vec<u8>>, anyhow::Error> {
    let reader = Reader::from_source(bytes.to_vec())?;
    let tmp_path = dest_path.with_extension("tmp");
    if let Some(parent) = dest_path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    std::fs::write(&tmp_path, bytes)?;
    std::fs::rename(&tmp_path, dest_path)?;
    Ok(reader)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn empty_resolver_returns_none() {
        let resolver = GeoIpResolver::empty();
        assert!(!resolver.is_loaded());
        assert_eq!(resolver.lookup_country("8.8.8.8".parse().unwrap()), None);
    }

    #[test]
    fn lookup_with_test_mmdb() {
        let path =
            Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/data/GeoIP2-Country-Test.mmdb");
        if !path.exists() {
            return;
        }
        let resolver = GeoIpResolver::from_file(&path).expect("load test mmdb");
        assert!(resolver.is_loaded());
        let gb_ip: IpAddr = "81.2.69.160".parse().unwrap();
        assert_eq!(resolver.lookup_country(gb_ip), Some("GB".to_string()));

        let config = GeoIpConfig::default();
        let status = resolver.status(Path::new("/tmp"), &config);
        assert!(status.loaded);
        assert_eq!(status.database_type, Some("GeoIP2-Country".to_string()));
    }

    #[test]
    fn sha256_verification_and_archive_extraction() {
        use flate2::Compression;
        use flate2::write::GzEncoder;

        let mmdb_path =
            Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/data/GeoIP2-Country-Test.mmdb");
        if !mmdb_path.exists() {
            return;
        }
        let raw_bytes = std::fs::read(&mmdb_path).unwrap();

        let mut tar_bytes = Vec::new();
        {
            let gz = GzEncoder::new(&mut tar_bytes, Compression::default());
            let mut tar = tar::Builder::new(gz);
            let mut header = tar::Header::new_gnu();
            header
                .set_path("GeoIP2-Country_test/GeoIP2-Country.mmdb")
                .unwrap();
            header.set_size(raw_bytes.len() as u64);
            header.set_cksum();
            tar.append(&header, &raw_bytes[..]).unwrap();
            tar.finish().unwrap();
        }

        let mut hasher = Sha256::new();
        hasher.update(&tar_bytes);
        let expected_hash = hex::encode(hasher.finalize());

        assert!(verify_sha256(&expected_hash, &tar_bytes).is_ok());
        assert!(verify_sha256("0000000000000000", &tar_bytes).is_err());

        let extracted = extract_mmdb_bytes(&tar_bytes).unwrap();
        assert_eq!(extracted.len(), raw_bytes.len());
    }
}
