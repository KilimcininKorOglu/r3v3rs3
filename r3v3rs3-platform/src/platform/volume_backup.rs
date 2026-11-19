//! Persistent volume backups and snapshots: tar.gz archives of volume mounts, local and S3 storage,
//! snapshot listing, on-demand restoration, and direct archive export/import.
//!
//! A short-lived helper container of BusyBox mounts the volume and packs or unpacks it with
//! `tar`, and the archive moves between the panel and the helper in bounded pieces. The image
//! of the app needs no shell and no `tar`, and an archive of any size passes without being
//! held in memory.

use super::Platform;
use super::transfer::{DiskRoom, Scratch, fetch_to_file, push_file};
use crate::clock::unix_ms;
use crate::runtime::ContainerRuntime;
use anyhow::{Context as _, bail};
use r3v3rs3_api::container::{
    ContainerName, ContainerSpec, ResourceLimits, RestartPolicy, VolumeMount,
};
use r3v3rs3_api::error::Error;
use r3v3rs3_api::id::ShortId;
use r3v3rs3_api::platform::{AppEntry, VolumeBackupEntry};
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;

/// The image of the helper container: BusyBox carries `tar`, `gzip` and `sleep`. The mirror
/// of Google keeps every pull of a backup below the pull limit of Docker Hub.
const HELPER_IMAGE: &str = "mirror.gcr.io/library/busybox:1.38.0";
/// Where the helper mounts the volume.
const HELPER_MOUNT: &str = "/volume";
/// The archive inside the helper.
const HELPER_ARCHIVE: &str = "/tmp/volume.tar.gz";
/// How long the helper waits for its commands before it ends by itself: the limit of one
/// container command, so a helper that the panel lost does not run forever.
const HELPER_LIFETIME: &str = "21600";

/// Packs the volume into the archive. The entries start with `./`, which a restore uses to
/// tell them from the archives of the Docker archive API that older releases stored.
const PACK: &str = "tar --numeric-owner -czf /tmp/volume.tar.gz -C /volume .";
/// Unpacks the archive into the volume. An archive of an older release holds the mount
/// directory itself as its first entry, which is dropped, so its files land in the volume
/// and not in a directory of the same name inside it.
const UNPACK_BACKUP: &str = "first=$(tar -tzf /tmp/volume.tar.gz | head -n 1) || exit 1; \
     case \"$first\" in ./*) strip=0;; *) strip=1;; esac; \
     tar --numeric-owner --strip-components=$strip -xzf /tmp/volume.tar.gz -C /volume";
/// Unpacks an archive that a user imports: its entries land relative to the volume.
const UNPACK_IMPORT: &str = "tar --numeric-owner -xzf /tmp/volume.tar.gz -C /volume";

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
        let (app, _, runtime) = self.file_runtime(id).await?;
        let mount = Self::find_volume_mount(&app, volume)?;
        let dir = self.volume_backup_dir(app.name.as_str(), volume);
        tokio::fs::create_dir_all(&dir).await?;
        let scratch = Scratch::new(&dir).await?;
        let packed = scratch.path("volume.tar.gz");
        pack_volume(&runtime, &app, &mount, &packed).await?;
        let backup_id = generate_backup_id();
        let archive = dir.join(format!("{backup_id}.tar.gz"));
        // The archive appears only complete, and before its manifest.
        tokio::fs::rename(&packed, &archive).await?;
        let (size, sha256) = super::transfer::sha256_file(&archive).await?;
        let entry = VolumeBackupEntry {
            id: backup_id.clone(),
            app: id,
            volume: volume.to_string(),
            path: mount.target.as_str().to_string(),
            size,
            sha256,
            created_at: unix_ms() as i64,
        };
        let manifest = serde_json::to_vec(&entry)?;
        tokio::fs::write(dir.join(format!("{backup_id}.json")), manifest).await?;
        self.report_volume_upload(&app, volume, &entry, &archive)
            .await;
        Ok(entry)
    }

    /// Uploads a new snapshot to S3 and reports a failure as a webhook event.
    async fn report_volume_upload(
        &self,
        app: &AppEntry,
        volume: &str,
        entry: &VolumeBackupEntry,
        archive: &Path,
    ) {
        let Err(err) = self
            .upload_volume_to_s3(app.name.as_str(), volume, entry, archive)
            .await
        else {
            return;
        };
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
            "app_id": app.id.to_string(),
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

    /// The path of a stored volume snapshot.
    async fn volume_backup_path(
        &self,
        id: ShortId,
        volume: &str,
        backup_id: &str,
    ) -> anyhow::Result<PathBuf> {
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
        Ok(path)
    }

    /// A compressed volume snapshot, opened for reading, and its size.
    pub async fn open_volume_backup(
        &self,
        id: ShortId,
        volume: &str,
        backup_id: &str,
    ) -> anyhow::Result<(tokio::fs::File, u64)> {
        let path = self.volume_backup_path(id, volume, backup_id).await?;
        open_sized(&path).await
    }

    /// Restores a volume snapshot into the volume.
    pub async fn restore_volume_backup(
        &self,
        id: ShortId,
        volume: &str,
        backup_id: &str,
    ) -> anyhow::Result<()> {
        let path = self.volume_backup_path(id, volume, backup_id).await?;
        let (app, _, runtime) = self.file_runtime(id).await?;
        let mount = writable_mount(&app, volume)?;
        unpack_volume(&runtime, &app, &mount, (&path, UNPACK_BACKUP)).await
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

    /// Exports a volume as a `.tar.gz` archive, opened for reading, and its size. The file is
    /// removed at once; the open handle keeps its content until the download ends.
    pub async fn export_volume_archive(
        &self,
        id: ShortId,
        volume: &str,
    ) -> anyhow::Result<(tokio::fs::File, u64)> {
        let (app, _, runtime) = self.file_runtime(id).await?;
        let mount = Self::find_volume_mount(&app, volume)?;
        let scratch = self.volume_scratch(&app, volume).await?;
        let packed = scratch.path("volume.tar.gz");
        pack_volume(&runtime, &app, &mount, &packed).await?;
        open_sized(&packed).await
    }

    /// Imports a `.tar.gz` or `.tar` archive into the volume. The archive is outside input:
    /// the body and its decompressed content are both bounded while they stream to disk.
    pub async fn import_volume_archive<S, E>(
        &self,
        id: ShortId,
        volume: &str,
        body: S,
    ) -> anyhow::Result<u64>
    where
        S: futures::Stream<Item = Result<bytes::Bytes, E>> + Unpin,
        E: std::fmt::Display,
    {
        let (app, _, runtime) = self.file_runtime(id).await?;
        let mount = writable_mount(&app, volume)?;
        let scratch = self.volume_scratch(&app, volume).await?;
        let received = scratch.path("import");
        let mut room = DiskRoom::new(&self.volume_backup_dir(app.name.as_str(), volume))?;
        let size = receive(body, &received, &mut room).await?;
        let packed = scratch.path("volume.tar.gz");
        gzip_within(&received, &packed, &mut room).await?;
        unpack_volume(&runtime, &app, &mount, (&packed, UNPACK_IMPORT)).await?;
        Ok(size)
    }

    /// A directory for the files of one export or import, beside the snapshots of the volume.
    async fn volume_scratch(&self, app: &AppEntry, volume: &str) -> anyhow::Result<Scratch> {
        let dir = self.volume_backup_dir(app.name.as_str(), volume);
        tokio::fs::create_dir_all(&dir).await?;
        Scratch::new(&dir).await
    }

    async fn upload_volume_to_s3(
        &self,
        app_name: &str,
        volume: &str,
        entry: &VolumeBackupEntry,
        archive: &Path,
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
        super::s3::upload_file(http_client, &s3, &key, archive).await
    }
}

/// The mount of `volume` for a restore, which writes into it.
fn writable_mount(app: &AppEntry, volume: &str) -> anyhow::Result<VolumeMount> {
    let mount = Platform::find_volume_mount(app, volume)?;
    if mount.read_only {
        bail!(Error::VolumeReadOnly {
            path: mount.target.as_str().to_string()
        });
    }
    Ok(mount)
}

/// A local file opened for reading, and its size.
async fn open_sized(path: &Path) -> anyhow::Result<(tokio::fs::File, u64)> {
    let file = tokio::fs::File::open(path).await?;
    let size = file.metadata().await?.len();
    Ok((file, size))
}

/// Packs the volume of `mount` into the local file `dest`.
async fn pack_volume(
    runtime: &Arc<dyn ContainerRuntime>,
    app: &AppEntry,
    mount: &VolumeMount,
    dest: &Path,
) -> anyhow::Result<()> {
    let helper = start_helper(runtime, app, mount, true).await?;
    let packed = async {
        run_in(runtime, &helper, PACK).await?;
        fetch_to_file(runtime, &helper, (HELPER_ARCHIVE, 0), dest).await
    }
    .await;
    finish_helper(runtime, &helper, packed.map(|_| ())).await
}

/// Unpacks the local archive `src` into the volume of `mount` with `script`.
async fn unpack_volume(
    runtime: &Arc<dyn ContainerRuntime>,
    app: &AppEntry,
    mount: &VolumeMount,
    (src, script): (&Path, &str),
) -> anyhow::Result<()> {
    let helper = start_helper(runtime, app, mount, false).await?;
    let unpacked = async {
        push_file(runtime, &helper, src, HELPER_ARCHIVE).await?;
        run_in(runtime, &helper, script).await
    }
    .await;
    finish_helper(runtime, &helper, unpacked).await
}

/// Starts a helper container that mounts the volume of `mount` at `HELPER_MOUNT` and waits
/// for its commands.
async fn start_helper(
    runtime: &Arc<dyn ContainerRuntime>,
    app: &AppEntry,
    mount: &VolumeMount,
    read_only: bool,
) -> anyhow::Result<ContainerName> {
    let spec = helper_spec(app, mount, read_only)?;
    runtime.pull_image(&spec.image).await?;
    runtime.create_container(&spec).await?;
    let started = runtime.start_container(&spec.name).await;
    if started.is_err() {
        finish_helper(runtime, &spec.name, started).await?;
    }
    Ok(spec.name)
}

/// The spec of a helper container: a name of its own, so two jobs of one app never meet.
fn helper_spec(
    app: &AppEntry,
    mount: &VolumeMount,
    read_only: bool,
) -> anyhow::Result<ContainerSpec> {
    let volume = VolumeMount {
        volume: mount.volume.clone(),
        target: HELPER_MOUNT.parse()?,
        read_only,
    };
    Ok(ContainerSpec {
        name: format!("r3v3rs3-volume-{}-{:08x}", app.id, rand::random::<u32>()).parse()?,
        image: HELPER_IMAGE.parse()?,
        env: Vec::new(),
        labels: BTreeMap::new(),
        network: None,
        volumes: vec![volume],
        restart: RestartPolicy::No,
        limits: ResourceLimits::default(),
        publish: None,
        host_port: None,
        aliases: Vec::new(),
        extra_networks: Vec::new(),
        log_max_size: None,
        log_max_file: None,
        command: Some(vec!["sleep".to_string(), HELPER_LIFETIME.to_string()]),
    })
}

/// Runs a shell script in the helper.
async fn run_in(
    runtime: &Arc<dyn ContainerRuntime>,
    helper: &ContainerName,
    script: &str,
) -> anyhow::Result<()> {
    let command = vec!["sh".to_string(), "-c".to_string(), script.to_string()];
    runtime
        .exec_capture(helper, command, Vec::new())
        .await
        .map(|_| ())
}

/// Removes the helper and returns the outcome of its job; a helper that stays is an error
/// of its own once the job succeeded.
async fn finish_helper(
    runtime: &Arc<dyn ContainerRuntime>,
    helper: &ContainerName,
    outcome: anyhow::Result<()>,
) -> anyhow::Result<()> {
    let removed = runtime
        .remove_container(helper)
        .await
        .with_context(|| format!("the helper container {helper} could not be removed"));
    outcome?;
    removed
}

/// Writes a request body into `dest` within the room of the disk.
async fn receive<S, E>(mut body: S, dest: &Path, room: &mut DiskRoom) -> anyhow::Result<u64>
where
    S: futures::Stream<Item = Result<bytes::Bytes, E>> + Unpin,
    E: std::fmt::Display,
{
    use futures::StreamExt as _;
    use tokio::io::AsyncWriteExt as _;
    let mut out = tokio::fs::File::create(dest).await?;
    let mut size = 0u64;
    while let Some(piece) = body.next().await {
        let piece = piece.map_err(|err| anyhow::anyhow!("the archive upload failed: {err}"))?;
        room.take(piece.len() as u64)?;
        size += piece.len() as u64;
        out.write_all(&piece).await?;
    }
    out.sync_all().await?;
    Ok(size)
}

/// Writes the archive `src`, a `.tar` or a `.tar.gz`, as a `.tar.gz` into `dest`. An import
/// is outside input: a gzip whose decompressed content does not fit the room of the disk is
/// refused before it reaches the volume, which stops a decompression bomb.
async fn gzip_within(src: &Path, dest: &Path, room: &mut DiskRoom) -> anyhow::Result<()> {
    let size = tokio::fs::metadata(src).await?.len();
    let head = super::transfer::head_bytes(src, 2).await?;
    if !head.starts_with(&[0x1f, 0x8b]) {
        room.take(size)?;
        return super::transfer::gzip_file(src, dest).await.map(|_| ());
    }
    count_gunzipped(src, room).await?;
    room.take(size)?;
    tokio::fs::copy(src, dest).await?;
    Ok(())
}

/// Counts the decompressed content of the gzip file `src` against `room`, writing nothing.
async fn count_gunzipped(src: &Path, room: &mut DiskRoom) -> anyhow::Result<()> {
    use tokio::io::AsyncReadExt as _;
    let input = tokio::io::BufReader::new(tokio::fs::File::open(src).await?);
    let mut reader = async_compression::tokio::bufread::GzipDecoder::new(input);
    let mut buf = vec![0u8; 1024 * 1024];
    loop {
        let n = reader.read(&mut buf).await?;
        if n == 0 {
            return Ok(());
        }
        room.take(n as u64)?;
    }
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

    /// An import is outside input: a gzip whose decompressed content does not fit the room of
    /// the disk must be refused before it reaches the volume. The reserve here leaves room for
    /// 8 MiB, and the bomb holds 64 MiB of zeros in a few KiB.
    #[tokio::test]
    async fn an_import_bomb_is_refused_at_the_room_of_the_disk() -> anyhow::Result<()> {
        use std::io::Write as _;
        let dir = Scratch::new(&std::env::temp_dir()).await?;
        let mut encoder = flate2::write::GzEncoder::new(Vec::new(), flate2::Compression::fast());
        encoder.write_all(&vec![0u8; 64 * 1024 * 1024])?;
        let src = dir.path("bomb.tar.gz");
        tokio::fs::write(&src, encoder.finish()?).await?;
        let free = super::super::prune::get_disk_space(&std::env::temp_dir())
            .context("the free space of the temp dir")?
            .available_bytes;
        let mut room = DiskRoom::with_reserve(&std::env::temp_dir(), free - 8 * 1024 * 1024)?;
        let err = gzip_within(&src, &dir.path("out.tar.gz"), &mut room)
            .await
            .unwrap_err();
        assert!(err.to_string().contains("would keep less than"), "{err}");
        assert!(!tokio::fs::try_exists(dir.path("out.tar.gz")).await?);
        Ok(())
    }

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
}
