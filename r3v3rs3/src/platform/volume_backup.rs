//! Persistent volume backups and snapshots: tar.gz archives of volume mounts, local and S3 storage,
//! snapshot listing, on-demand restoration, and direct archive export/import.

use super::Platform;
use crate::clock::unix_ms;
use anyhow::bail;
use async_compression::tokio::bufread::{GzipDecoder, GzipEncoder};
use r3v3rs3_api::container::VolumeMount;
use r3v3rs3_api::error::Error;
use r3v3rs3_api::id::ShortId;
use r3v3rs3_api::platform::{AppEntry, VolumeBackupEntry};
use sha2::{Digest, Sha256};
use std::path::PathBuf;
use tokio::io::AsyncReadExt;

impl Platform {
    /// Directory that holds the snapshots and manifests of a volume.
    fn volume_backup_dir(&self, app_name: &str, volume: &str) -> PathBuf {
        self.backup_dir.join("volumes").join(app_name).join(volume)
    }

    /// Finds the volume mount in the app spec and confirms it exists.
    fn find_volume_mount(app: &AppEntry, volume: &str) -> anyhow::Result<VolumeMount> {
        app.spec
            .volumes
            .iter()
            .find(|m| m.volume.as_str() == volume)
            .cloned()
            .ok_or_else(|| {
                Error::InvalidFilePath {
                    path: volume.to_string(),
                }
                .into()
            })
    }

    /// Lists stored backups for a volume, sorted newest first.
    pub async fn list_volume_backups(
        &self,
        id: ShortId,
        volume: &str,
    ) -> anyhow::Result<Vec<VolumeBackupEntry>> {
        let app = self.app(id).await?;
        Self::find_volume_mount(&app, volume)?;
        let dir = self.volume_backup_dir(app.name.as_str(), volume);
        if !dir.exists() {
            return Ok(Vec::new());
        }
        let mut entries = Vec::new();
        let mut read_dir = tokio::fs::read_dir(&dir).await?;
        while let Some(item) = read_dir.next_entry().await? {
            let path = item.path();
            if path.extension().and_then(|e| e.to_str()) == Some("json")
                && let Ok(bytes) = tokio::fs::read(&path).await
                && let Ok(entry) = serde_json::from_slice::<VolumeBackupEntry>(&bytes)
            {
                entries.push(entry);
            }
        }
        entries.sort_by_key(|a| std::cmp::Reverse(a.created_at));
        Ok(entries)
    }

    /// Creates a compressed snapshot of the volume and saves it locally and to S3 when configured.
    pub async fn create_volume_backup(
        &self,
        id: ShortId,
        volume: &str,
    ) -> anyhow::Result<VolumeBackupEntry> {
        let _guard = self.track_job();
        let (app, name, runtime) = self.file_runtime(id).await?;
        let mount = Self::find_volume_mount(&app, volume)?;
        let tar_bytes = runtime
            .download_archive(&name, mount.target.as_str())
            .await?;
        let compressed = gzip_bytes(&tar_bytes).await?;
        let backup_id = generate_backup_id();
        let entry = VolumeBackupEntry {
            id: backup_id.clone(),
            app: id,
            volume: volume.to_string(),
            path: mount.target.as_str().to_string(),
            size: compressed.len() as u64,
            sha256: hex::encode(Sha256::digest(&compressed)),
            created_at: unix_ms() as i64,
        };

        let dir = self.volume_backup_dir(app.name.as_str(), volume);
        tokio::fs::create_dir_all(&dir).await?;
        tokio::fs::write(dir.join(format!("{backup_id}.tar.gz")), &compressed).await?;
        let manifest = serde_json::to_vec(&entry)?;
        tokio::fs::write(dir.join(format!("{backup_id}.json")), manifest).await?;

        if let Err(err) = self
            .upload_volume_to_s3(app.name.as_str(), volume, &entry, &compressed)
            .await
        {
            tracing::error!(
                err = format!("{err:#}"),
                app = app.name.as_str(),
                volume = volume,
                "failed to upload volume backup to S3"
            );
            let summary = format!(
                "Remote S3 volume backup upload failed for app '{}' volume '{}'",
                app.name, volume
            );
            let data = serde_json::json!({
                "backup_id": entry.id,
                "app_id": id.to_string(),
                "app_name": app.name.as_str(),
                "volume": volume,
                "target": "s3",
                "error": err.to_string(),
            });
            self.dispatch_webhook_event(
                r3v3rs3_api::webhook::WebhookEvent::BackupUploadFailed,
                summary,
                data,
            )
            .await;
        }
        Ok(entry)
    }

    /// Reads a compressed volume snapshot.
    pub async fn download_volume_backup(
        &self,
        id: ShortId,
        volume: &str,
        backup_id: &str,
    ) -> anyhow::Result<Vec<u8>> {
        validate_backup_id(backup_id)?;
        let app = self.app(id).await?;
        Self::find_volume_mount(&app, volume)?;
        let path = self
            .volume_backup_dir(app.name.as_str(), volume)
            .join(format!("{backup_id}.tar.gz"));
        if !path.exists() {
            bail!(Error::BackupNotFound {
                id: backup_id.to_string()
            });
        }
        Ok(tokio::fs::read(&path).await?)
    }

    /// Restores a volume snapshot into the container.
    pub async fn restore_volume_backup(
        &self,
        id: ShortId,
        volume: &str,
        backup_id: &str,
    ) -> anyhow::Result<()> {
        validate_backup_id(backup_id)?;
        let (app, name, runtime) = self.file_runtime(id).await?;
        let mount = Self::find_volume_mount(&app, volume)?;
        if mount.read_only {
            bail!(Error::VolumeReadOnly {
                path: mount.target.as_str().to_string()
            });
        }
        let compressed = self.download_volume_backup(id, volume, backup_id).await?;
        let tar_bytes = ungzip_bytes(&compressed).await?;
        runtime
            .upload_archive(&name, mount.target.as_str(), tar_bytes)
            .await
    }

    /// Deletes a stored volume snapshot.
    pub async fn delete_volume_backup(
        &self,
        id: ShortId,
        volume: &str,
        backup_id: &str,
    ) -> anyhow::Result<()> {
        validate_backup_id(backup_id)?;
        let app = self.app(id).await?;
        Self::find_volume_mount(&app, volume)?;
        let dir = self.volume_backup_dir(app.name.as_str(), volume);
        let archive = dir.join(format!("{backup_id}.tar.gz"));
        let manifest = dir.join(format!("{backup_id}.json"));
        let _ = tokio::fs::remove_file(archive).await;
        let _ = tokio::fs::remove_file(manifest).await;
        Ok(())
    }

    /// Exports a volume as a `.tar.gz` archive directly from the running container.
    pub async fn export_volume_archive(
        &self,
        id: ShortId,
        volume: &str,
    ) -> anyhow::Result<Vec<u8>> {
        let (app, name, runtime) = self.file_runtime(id).await?;
        let mount = Self::find_volume_mount(&app, volume)?;
        let tar_bytes = runtime
            .download_archive(&name, mount.target.as_str())
            .await?;
        gzip_bytes(&tar_bytes).await
    }

    /// Imports and unpacks a `.tar.gz` or `.tar` archive into the volume.
    pub async fn import_volume_archive(
        &self,
        id: ShortId,
        volume: &str,
        bytes: Vec<u8>,
    ) -> anyhow::Result<()> {
        let (app, name, runtime) = self.file_runtime(id).await?;
        let mount = Self::find_volume_mount(&app, volume)?;
        if mount.read_only {
            bail!(Error::VolumeReadOnly {
                path: mount.target.as_str().to_string()
            });
        }
        let tar_bytes = if bytes.starts_with(&[0x1f, 0x8b]) {
            ungzip_bytes(&bytes).await?
        } else {
            bytes
        };
        runtime
            .upload_archive(&name, mount.target.as_str(), tar_bytes)
            .await
    }

    async fn upload_volume_to_s3(
        &self,
        app_name: &str,
        volume: &str,
        entry: &VolumeBackupEntry,
        compressed: &[u8],
    ) -> anyhow::Result<()> {
        let config = self.backup_config().await?;
        let Some(s3) = config.s3 else {
            return Ok(());
        };
        let Some(secret) = self.s3_secret().await? else {
            bail!("the S3 target has no secret key stored");
        };
        let s3 = r3v3rs3_api::platform::S3Config {
            secret_key: secret,
            ..s3
        };
        let http_client = self.http().await?;
        let key = format!("volumes/{app_name}/{volume}/{}.tar.gz", entry.id);
        super::s3::upload_key(http_client, &s3, &key, compressed.to_vec()).await
    }
}

pub const MAX_VOLUME_ARCHIVE_UNCOMPRESSED_BYTES: u64 = 512 * 1024 * 1024;

pub(crate) async fn gzip_bytes(content: &[u8]) -> anyhow::Result<Vec<u8>> {
    let mut encoder = GzipEncoder::new(content);
    let mut compressed = Vec::new();
    encoder.read_to_end(&mut compressed).await?;
    Ok(compressed)
}

pub(crate) async fn ungzip_bytes(content: &[u8]) -> anyhow::Result<Vec<u8>> {
    ungzip_bytes_limited(content, MAX_VOLUME_ARCHIVE_UNCOMPRESSED_BYTES).await
}

pub(crate) async fn ungzip_bytes_limited(
    content: &[u8],
    max_bytes: u64,
) -> anyhow::Result<Vec<u8>> {
    use tokio::io::AsyncReadExt;
    let decoder = GzipDecoder::new(content);
    let mut reader = decoder.take(max_bytes + 1);
    let mut plain = Vec::new();
    reader.read_to_end(&mut plain).await?;
    if plain.len() as u64 > max_bytes {
        anyhow::bail!(
            "decompressed volume archive exceeds maximum allowed size of {max_bytes} bytes"
        );
    }
    Ok(plain)
}

pub fn validate_backup_id(backup_id: &str) -> anyhow::Result<()> {
    if !backup_id.is_empty()
        && backup_id
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_')
    {
        Ok(())
    } else {
        bail!(Error::InvalidFilePath {
            path: backup_id.to_string(),
        })
    }
}

fn generate_backup_id() -> String {
    format!("{:x}-{:08x}", unix_ms(), rand::random::<u32>())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_volume_backup_id_validation() {
        assert!(validate_backup_id("backup-123_abc").is_ok());
        assert!(validate_backup_id("12345678-abcdef01").is_ok());
        assert!(validate_backup_id("../traverse").is_err());
        assert!(validate_backup_id("foo/bar").is_err());
        assert!(validate_backup_id("foo\\bar").is_err());
        assert!(validate_backup_id("").is_err());
        assert!(validate_backup_id("backup.tar.gz").is_err());
    }

    #[tokio::test]
    async fn gzip_round_trip() {
        let input = b"volume snapshot test content 1234567890";
        let compressed = gzip_bytes(input).await.unwrap();
        assert!(!compressed.is_empty());
        let decompressed = ungzip_bytes(&compressed).await.unwrap();
        assert_eq!(&decompressed, input);

        let err = ungzip_bytes_limited(&compressed, 10).await.unwrap_err();
        assert!(err.to_string().contains("exceeds maximum allowed size"), "{err}");
    }
}
