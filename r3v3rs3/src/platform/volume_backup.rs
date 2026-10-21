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

        self.upload_volume_to_s3(app.name.as_str(), volume, &entry, &compressed)
            .await;
        Ok(entry)
    }

    /// Reads a compressed volume snapshot.
    pub async fn download_volume_backup(
        &self,
        id: ShortId,
        volume: &str,
        backup_id: &str,
    ) -> anyhow::Result<Vec<u8>> {
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
    ) {
        let Ok(config) = self.backup_config().await else {
            return;
        };
        let Some(s3) = config.s3 else {
            return;
        };
        let Ok(Some(secret)) = self.s3_secret().await else {
            return;
        };
        let s3 = r3v3rs3_api::platform::S3Config {
            secret_key: secret,
            ..s3
        };
        let Ok(http_client) = self.http().await else {
            return;
        };
        let key = format!("volumes/{app_name}/{volume}/{}.tar.gz", entry.id);
        let _ = super::s3::upload_key(http_client, &s3, &key, compressed.to_vec()).await;
    }
}

pub(crate) async fn gzip_bytes(content: &[u8]) -> anyhow::Result<Vec<u8>> {
    let mut encoder = GzipEncoder::new(content);
    let mut compressed = Vec::new();
    encoder.read_to_end(&mut compressed).await?;
    Ok(compressed)
}

pub(crate) async fn ungzip_bytes(content: &[u8]) -> anyhow::Result<Vec<u8>> {
    let mut decoder = GzipDecoder::new(content);
    let mut plain = Vec::new();
    decoder.read_to_end(&mut plain).await?;
    Ok(plain)
}

fn generate_backup_id() -> String {
    format!("{:x}-{:08x}", unix_ms(), rand::random::<u32>())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn gzip_round_trip() {
        let input = b"volume snapshot test content 1234567890";
        let compressed = gzip_bytes(input).await.unwrap();
        assert!(!compressed.is_empty());
        let decompressed = ungzip_bytes(&compressed).await.unwrap();
        assert_eq!(&decompressed, input);
    }
}
