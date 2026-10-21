//! Disaster recovery: encrypted bundle creation, S3 replication, and full restoration.
//!
//! A disaster recovery bundle packages the complete platform state: server configuration,
//! ports, proxies, access lists, ACME certificates, user accounts, platform.db (SQLite database),
//! platform secrets key, and cluster sealing keys into an AES-256-GCM encrypted `.r3bundle`.

use anyhow::{Context as _, anyhow};
use aws_lc_rs::aead::{AES_256_GCM, Aad, LessSafeKey, NONCE_LEN, Nonce, UnboundKey};
use aws_lc_rs::rand::{SecureRandom, SystemRandom};
use flate2::Compression;
use flate2::read::GzDecoder;
use flate2::write::GzEncoder;
use r3v3rs3_api::platform::DisasterRecoveryBackupEntry;
use sha2::{Digest, Sha256};
use std::fs;
use std::path::{Path, PathBuf};
use std::time::SystemTime;
use tar::Header;

const DR_MAGIC: &[u8; 4] = b"R3DR";
const DR_VERSION: u8 = 1;
pub const SALT_LEN: usize = 16;
const AAD_BYTES: &[u8] = b"r3v3rs3-disaster-recovery";
pub const DISASTER_RECOVERY_DIR: &str = "disaster-recovery";

const ROOT_CONFIG_FILES: &[&str] = &[
    "config.toml",
    "ports.toml",
    "proxies.toml",
    "access_lists.toml",
    "acme.toml",
    "accounts.toml",
    "notifications.json",
    "cdn-ranges.json",
    "app-store.json",
    "platform.key",
    "platform.db",
    "platform.db-wal",
    "platform.db-shm",
];

const SUBDIRS: &[&str] = &["certs", "keys"];

/// Derives a 32-byte AES-256 key from a passphrase and salt using iterated SHA-256.
pub fn derive_dr_key(passphrase: &str, salt: &[u8; SALT_LEN]) -> [u8; 32] {
    let mut hasher = Sha256::new();
    hasher.update(b"R3DR-KDF-V1");
    hasher.update(salt);
    hasher.update(passphrase.as_bytes());
    let mut round = hasher.finalize();
    for i in 0..10_000u32 {
        let mut h = Sha256::new();
        h.update(round);
        h.update(i.to_le_bytes());
        h.update(salt);
        h.update(passphrase.as_bytes());
        round = h.finalize();
    }
    let mut key = [0u8; 32];
    key.copy_from_slice(&round);
    key
}

/// Encrypts raw bundle bytes with AES-256-GCM and prepends the magic header, version, salt, and nonce.
pub fn encrypt_dr_bundle(payload: &[u8], passphrase: &str) -> anyhow::Result<Vec<u8>> {
    let random = SystemRandom::new();
    let mut salt = [0u8; SALT_LEN];
    random
        .fill(&mut salt)
        .map_err(|_| anyhow!("failed to generate random salt"))?;
    let key_bytes = derive_dr_key(passphrase, &salt);
    let key =
        UnboundKey::new(&AES_256_GCM, &key_bytes).map_err(|_| anyhow!("invalid encryption key"))?;
    let safe_key = LessSafeKey::new(key);
    let mut nonce = [0u8; NONCE_LEN];
    random
        .fill(&mut nonce)
        .map_err(|_| anyhow!("failed to generate random nonce"))?;
    let mut body = payload.to_vec();
    safe_key
        .seal_in_place_append_tag(
            Nonce::assume_unique_for_key(nonce),
            Aad::from(AAD_BYTES),
            &mut body,
        )
        .map_err(|_| anyhow!("failed to encrypt disaster recovery bundle"))?;
    Ok([DR_MAGIC.as_slice(), &[DR_VERSION], &salt, &nonce, &body].concat())
}

/// Decrypts an encrypted `.r3bundle` into its raw `.tar.gz` payload.
pub fn decrypt_dr_bundle(bundle: &[u8], passphrase: &str) -> anyhow::Result<Vec<u8>> {
    let min_len = 4 + 1 + SALT_LEN + NONCE_LEN + 16;
    if bundle.len() < min_len {
        anyhow::bail!("disaster recovery bundle is too short");
    }
    if &bundle[0..4] != DR_MAGIC {
        anyhow::bail!("invalid disaster recovery bundle magic bytes");
    }
    if bundle[4] != DR_VERSION {
        anyhow::bail!(
            "unsupported disaster recovery bundle version: {}",
            bundle[4]
        );
    }
    let mut salt = [0u8; SALT_LEN];
    salt.copy_from_slice(&bundle[5..5 + SALT_LEN]);
    let mut nonce = [0u8; NONCE_LEN];
    nonce.copy_from_slice(&bundle[5 + SALT_LEN..5 + SALT_LEN + NONCE_LEN]);
    let body_start = 5 + SALT_LEN + NONCE_LEN;
    let mut body = bundle[body_start..].to_vec();
    let key_bytes = derive_dr_key(passphrase, &salt);
    let key =
        UnboundKey::new(&AES_256_GCM, &key_bytes).map_err(|_| anyhow!("invalid decryption key"))?;
    let safe_key = LessSafeKey::new(key);
    let len = safe_key
        .open_in_place(
            Nonce::assume_unique_for_key(nonce),
            Aad::from(AAD_BYTES),
            &mut body,
        )
        .map_err(|_| anyhow!("invalid passphrase or corrupted disaster recovery bundle"))?
        .len();
    body.truncate(len);
    Ok(body)
}

/// Collects root files and subdirectories from `config_dir` for the bundle.
pub fn collect_bundle_files(config_dir: &Path) -> anyhow::Result<Vec<(String, Vec<u8>)>> {
    let mut files = Vec::new();
    collect_root_files(config_dir, &mut files);
    for subdir in SUBDIRS {
        let dir_path = config_dir.join(subdir);
        if dir_path.is_dir() {
            collect_dir_recursive(&dir_path, subdir, &mut files);
        }
    }
    Ok(files)
}

fn collect_root_files(config_dir: &Path, files: &mut Vec<(String, Vec<u8>)>) {
    for filename in ROOT_CONFIG_FILES {
        let path = config_dir.join(filename);
        if path.is_file()
            && let Ok(bytes) = fs::read(&path)
        {
            files.push(((*filename).to_string(), bytes));
        }
    }
}

fn collect_dir_recursive(dir: &Path, prefix: &str, files: &mut Vec<(String, Vec<u8>)>) {
    let Ok(entries) = fs::read_dir(dir) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        let name = entry.file_name().to_string_lossy().to_string();
        let rel_path = format!("{prefix}/{name}");
        if path.is_file() {
            if let Ok(bytes) = fs::read(&path) {
                files.push((rel_path, bytes));
            }
        } else if path.is_dir() {
            collect_dir_recursive(&path, &rel_path, files);
        }
    }
}

/// Packs collected files into a gzipped tar archive.
pub fn pack_tar_gz(files: &[(String, Vec<u8>)]) -> anyhow::Result<Vec<u8>> {
    let mut buf = Vec::new();
    let mtime = SystemTime::now()
        .duration_since(SystemTime::UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs();
    {
        let enc = GzEncoder::new(&mut buf, Compression::default());
        let mut tar = tar::Builder::new(enc);
        for (name, content) in files {
            let mut header = Header::new_old();
            header.set_size(content.len() as _);
            header.set_mtime(mtime);
            header.set_mode(0o600);
            header.set_cksum();
            tar.append_data(&mut header, name, content.as_slice())?;
        }
        tar.into_inner()?.finish()?;
    }
    Ok(buf)
}

/// Unpacks a gzipped tar archive into `dest_dir` with path traversal protections.
pub fn unpack_tar_gz(archive_bytes: &[u8], dest_dir: &Path) -> anyhow::Result<usize> {
    let dec = GzDecoder::new(archive_bytes);
    let mut tar = tar::Archive::new(dec);
    let mut count = 0;
    for entry in tar.entries()? {
        let mut entry = entry?;
        let rel_path = entry.path()?.to_path_buf();
        let clean = sanitize_relative_path(&rel_path)?;
        let target = dest_dir.join(clean);
        if let Some(parent) = target.parent() {
            fs::create_dir_all(parent)?;
        }
        entry.unpack(&target)?;
        count += 1;
    }
    Ok(count)
}

fn sanitize_relative_path(path: &Path) -> anyhow::Result<PathBuf> {
    let mut clean = PathBuf::new();
    for component in path.components() {
        if let std::path::Component::Normal(c) = component {
            clean.push(c);
        }
    }
    if clean.as_os_str().is_empty() {
        anyhow::bail!("invalid archive path");
    }
    Ok(clean)
}

fn dr_backup_id() -> String {
    let now = SystemTime::now()
        .duration_since(SystemTime::UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs();
    let rand_suffix: String = (0..6)
        .map(|_| {
            let idx = (aws_lc_rs::rand::SystemRandom::new()
                .fill(&mut [0u8; 1])
                .map(|_| 0)
                .unwrap_or(0)) as usize;
            char::from(b'a' + (idx % 26) as u8)
        })
        .collect();
    format!("dr-{now}-{rand_suffix}")
}

/// Directory path under `config_dir` where disaster recovery backups are stored.
pub fn dr_dir(config_dir: &Path) -> PathBuf {
    config_dir.join(DISASTER_RECOVERY_DIR)
}

/// Creates an encrypted disaster recovery backup and optionally replicates it to S3.
pub async fn create_dr_backup(
    config_dir: &Path,
    passphrase: Option<&str>,
    upload_s3: bool,
    platform: Option<&crate::platform::Platform>,
) -> anyhow::Result<DisasterRecoveryBackupEntry> {
    let files = collect_bundle_files(config_dir)?;
    let files_count = files.len();
    let tar_gz = pack_tar_gz(&files)?;
    let pass = passphrase.unwrap_or("");
    let encrypted_bundle = encrypt_dr_bundle(&tar_gz, pass)?;
    let id = dr_backup_id();
    let sha256 = hex::encode(Sha256::digest(&encrypted_bundle));
    let size = encrypted_bundle.len() as u64;
    let now = SystemTime::now()
        .duration_since(SystemTime::UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs() as i64;

    let dir = dr_dir(config_dir);
    fs::create_dir_all(&dir)?;
    let bundle_path = dir.join(format!("{id}.r3bundle"));
    fs::write(&bundle_path, &encrypted_bundle)?;

    let s3_replicated = if upload_s3 && let Some(p) = platform {
        replicate_to_s3(p, &id, &encrypted_bundle).await
    } else {
        false
    };

    let entry = DisasterRecoveryBackupEntry {
        id: id.clone(),
        created_at: now,
        size,
        sha256,
        encrypted: true,
        s3_replicated,
        files_count,
    };

    let manifest_path = dir.join(format!("{id}.json"));
    if let Ok(manifest_json) = serde_json::to_string_pretty(&entry) {
        let _ = fs::write(manifest_path, manifest_json);
    }

    Ok(entry)
}

async fn replicate_to_s3(
    platform: &crate::platform::Platform,
    id: &str,
    bundle_bytes: &[u8],
) -> bool {
    let Ok(config) = platform.backup_config().await else {
        return false;
    };
    let Some(s3) = config.s3 else {
        return false;
    };
    let Ok(Some(secret)) = platform.s3_secret().await else {
        return false;
    };
    let s3 = r3v3rs3_api::platform::S3Config {
        secret_key: secret,
        ..s3
    };
    let Ok(http_client) = platform.http().await else {
        return false;
    };
    let key = format!("disaster-recovery/{id}.r3bundle");
    super::s3::upload_key(http_client, &s3, &key, bundle_bytes.to_vec())
        .await
        .is_ok()
}

/// Lists all disaster recovery backups in `config_dir`, newest first.
pub async fn list_dr_backups(
    config_dir: &Path,
) -> anyhow::Result<Vec<DisasterRecoveryBackupEntry>> {
    let dir = dr_dir(config_dir);
    if !dir.is_dir() {
        return Ok(Vec::new());
    }
    let mut list = Vec::new();
    let entries = fs::read_dir(dir)?;
    for entry in entries.flatten() {
        let path = entry.path();
        if path.extension().and_then(|e| e.to_str()) == Some("json")
            && let Ok(text) = fs::read_to_string(&path)
            && let Ok(parsed) = serde_json::from_str::<DisasterRecoveryBackupEntry>(&text)
        {
            list.push(parsed);
        }
    }
    list.sort_by_key(|a| std::cmp::Reverse(a.created_at));
    Ok(list)
}

/// Reads the raw encrypted `.r3bundle` of a backup.
pub async fn download_dr_backup(config_dir: &Path, id: &str) -> anyhow::Result<Vec<u8>> {
    let path = dr_dir(config_dir).join(format!("{id}.r3bundle"));
    fs::read(&path).with_context(|| format!("disaster recovery backup not found: {id}"))
}

/// Deletes a backup and its manifest.
pub async fn delete_dr_backup(config_dir: &Path, id: &str) -> anyhow::Result<()> {
    let dir = dr_dir(config_dir);
    let bundle = dir.join(format!("{id}.r3bundle"));
    let manifest = dir.join(format!("{id}.json"));
    let _ = fs::remove_file(bundle);
    let _ = fs::remove_file(manifest);
    Ok(())
}

/// Restores platform state from a local disaster recovery backup ID.
pub async fn restore_dr_backup(
    config_dir: &Path,
    id: &str,
    passphrase: Option<&str>,
) -> anyhow::Result<usize> {
    let bundle = download_dr_backup(config_dir, id).await?;
    restore_dr_bytes(config_dir, &bundle, passphrase).await
}

/// Restores platform state from raw `.r3bundle` bytes.
pub async fn restore_dr_bytes(
    config_dir: &Path,
    bytes: &[u8],
    passphrase: Option<&str>,
) -> anyhow::Result<usize> {
    let pass = passphrase.unwrap_or("");
    let tar_gz = decrypt_dr_bundle(bytes, pass)?;
    unpack_tar_gz(&tar_gz, config_dir)
}

/// CLI entry point for creating a disaster recovery backup.
pub async fn run_cli_backup(
    config_dir: &Path,
    output: Option<&Path>,
    passphrase: &str,
    _upload_s3: bool,
) -> anyhow::Result<(DisasterRecoveryBackupEntry, PathBuf)> {
    let files = collect_bundle_files(config_dir)?;
    let files_count = files.len();
    let tar_gz = pack_tar_gz(&files)?;
    let encrypted = encrypt_dr_bundle(&tar_gz, passphrase)?;
    let id = dr_backup_id();
    let sha256 = hex::encode(Sha256::digest(&encrypted));
    let size = encrypted.len() as u64;
    let now = SystemTime::now()
        .duration_since(SystemTime::UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs() as i64;

    let dest_path = match output {
        Some(p) => p.to_path_buf(),
        None => {
            let dir = dr_dir(config_dir);
            fs::create_dir_all(&dir)?;
            dir.join(format!("{id}.r3bundle"))
        }
    };

    fs::write(&dest_path, &encrypted)?;

    let entry = DisasterRecoveryBackupEntry {
        id,
        created_at: now,
        size,
        sha256,
        encrypted: true,
        s3_replicated: false,
        files_count,
    };

    Ok((entry, dest_path))
}

/// CLI entry point for restoring from a disaster recovery bundle.
pub async fn run_cli_restore(
    file_path: &Path,
    config_dir: &Path,
    passphrase: &str,
) -> anyhow::Result<usize> {
    let bytes = fs::read(file_path)
        .with_context(|| format!("failed to read bundle file: {}", file_path.display()))?;
    restore_dr_bytes(config_dir, &bytes, Some(passphrase)).await
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_dr_bundle_encrypt_decrypt_round_trip() {
        let plaintext = b"Hello r3v3rs3 disaster recovery test payload!";
        let passphrase = "my-secret-dr-passphrase";
        let encrypted = encrypt_dr_bundle(plaintext, passphrase).unwrap();
        assert_ne!(&encrypted[..], plaintext);
        assert_eq!(&encrypted[0..4], DR_MAGIC);

        // Correct passphrase
        let decrypted = decrypt_dr_bundle(&encrypted, passphrase).unwrap();
        assert_eq!(&decrypted, plaintext);

        // Wrong passphrase
        assert!(decrypt_dr_bundle(&encrypted, "wrong-passphrase").is_err());
    }

    #[test]
    fn test_dr_bundle_tar_gz_pack_unpack() {
        let temp_dir = std::env::temp_dir().join(format!("r3dr_test_{}", std::process::id()));
        let _ = fs::remove_dir_all(&temp_dir);
        fs::create_dir_all(&temp_dir).unwrap();

        let files = vec![
            ("config.toml".to_string(), b"key = 'value'".to_vec()),
            ("certs/site.pem".to_string(), b"CERT DATA".to_vec()),
        ];
        let archive = pack_tar_gz(&files).unwrap();
        let count = unpack_tar_gz(&archive, &temp_dir).unwrap();
        assert_eq!(count, 2);

        assert_eq!(
            fs::read_to_string(temp_dir.join("config.toml")).unwrap(),
            "key = 'value'"
        );
        assert_eq!(
            fs::read_to_string(temp_dir.join("certs/site.pem")).unwrap(),
            "CERT DATA"
        );
        let _ = fs::remove_dir_all(&temp_dir);
    }
}
