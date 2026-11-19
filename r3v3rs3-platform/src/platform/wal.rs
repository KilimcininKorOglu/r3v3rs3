//! Continuous archiving of the PostgreSQL WAL and point-in-time recovery from it.
//!
//! The server copies every closed WAL file into its archive volume. The platform pulls the
//! files into the backup directory, takes physical base backups with `pg_basebackup`, and
//! recovers a database by replaying the archived WAL on top of a base backup up to a target
//! time (PostgreSQL manual, "Continuous Archiving and Point-in-Time Recovery").

use super::Platform;
use super::backup::{backup_id, gzip, ungzip};
use super::transfer::{Scratch, fetch_to_file, gzip_file};
use crate::clock::unix_ms;
use crate::runtime::ContainerRuntime;
use anyhow::{Context as _, bail, ensure};
use r3v3rs3_api::container::{
    ContainerName, ContainerSpec, ResourceLimits, RestartPolicy, VolumeMount,
};
use r3v3rs3_api::error::Error;
use r3v3rs3_api::id::ShortId;
use r3v3rs3_api::platform::{
    AppEntry, AppSource, AppSpec, EngineInfo, PitrRange, PitrRestoreRequest, PitrRestoreResponse,
    WalSegment,
};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

/// The path of the archive volume inside a PostgreSQL container.
pub(super) const WAL_ARCHIVE_PATH: &str = "/wal-archive";

/// The start command of a PostgreSQL server that archives its WAL. A named volume belongs to
/// root when Docker creates it, so the shell gives the archive to `postgres` before the
/// entrypoint drops its rights. A file is copied under a temporary name and renamed, so the
/// pull loop never reads a half-written file. `archive_timeout` closes a segment at least
/// once a minute, which bounds the data that a recovery can lose.
pub(super) const POSTGRES_ARCHIVE_START: &str = "chown postgres:postgres /wal-archive && \
exec docker-entrypoint.sh postgres -c wal_level=replica -c archive_mode=on \
-c archive_timeout=60 \
-c 'archive_command=test ! -f /wal-archive/%f && cp %p /wal-archive/%f.tmp && mv /wal-archive/%f.tmp /wal-archive/%f'";

/// How often the platform pulls the closed WAL files of every database.
const PULL_ROUND: Duration = Duration::from_secs(60);

/// How long a recovery may replay the WAL before it counts as failed.
const RECOVERY_TIMEOUT: Duration = Duration::from_secs(600);

/// How long the database container may take to answer on the recovered data.
const READY_TIMEOUT: Duration = Duration::from_secs(120);

/// The directory of the physical base backups under the backup directory of a database.
const BASE_DIR: &str = "base";

/// The directory under the data volume that keeps the data of the server while a recovery
/// replaces it, so a failed recovery can put it back.
pub(super) const PREVIOUS_DIR: &str = ".pitr-previous";

/// The length of a WAL file name: timeline, log and segment, 8 hex digits each.
const WAL_NAME_LEN: usize = 24;

/// Whether the containers of the database archive their WAL. Only the local target runs the
/// pull loop, so a database on an agent leaves archiving off instead of filling its disk.
pub(super) fn archives(engine: Option<&str>, target: ShortId) -> bool {
    matches!(
        engine,
        Some("postgres" | "mysql" | "mariadb" | "mongodb" | "redis")
    ) && super::is_local(target)
}

fn wal_volume(app: ShortId) -> anyhow::Result<VolumeMount> {
    Ok(VolumeMount {
        volume: format!("r3v3rs3-wal-{app}").parse()?,
        target: WAL_ARCHIVE_PATH.parse()?,
        read_only: false,
    })
}

/// The volumes of a container of the app: the volumes of its spec, and the archive volume of
/// a PostgreSQL database that archives its WAL. MySQL and MariaDB keep their binary log in
/// the data directory, where the platform reads it.
pub(super) fn container_volumes(
    app: ShortId,
    spec: &AppSpec,
    archive: bool,
) -> anyhow::Result<Vec<VolumeMount>> {
    let mut volumes = spec.volumes.clone();
    if archive && spec.database.as_deref() == Some("postgres") {
        volumes.push(wal_volume(app)?);
    }
    Ok(volumes)
}

/// One base backup: a `pg_basebackup` tar archive of the data directory, or a dump of a MySQL
/// or MariaDB server with the binary log coordinates of its snapshot.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub(super) struct BaseBackup {
    pub id: String,
    /// Unix milliseconds after the backup ended; a recovery can stop at this time or later.
    pub created_at: u64,
    /// The log file that was current when the backup started; a recovery replays from it.
    pub start_wal: String,
    /// The position in `start_wal` where the replay of a binary log starts.
    #[serde(default)]
    pub start_position: Option<u64>,
    /// The data directory of the server, which the archive holds.
    pub data_directory: String,
    pub size: u64,
    pub sha256: String,
}

/// Where the replay from a new base backup starts.
pub(super) struct BaseStart {
    pub log: String,
    pub position: Option<u64>,
    pub data_directory: String,
}

/// Writes a file through a temporary file in the same directory, so a crash leaves either
/// the old file or the new one.
pub(super) async fn write_atomic(path: &Path, bytes: &[u8]) -> anyhow::Result<()> {
    use tokio::io::AsyncWriteExt as _;
    let name = path
        .file_name()
        .context("the file has no name")?
        .to_string_lossy();
    let temp = path.with_file_name(format!(".{name}.tmp"));
    let mut file = tokio::fs::File::create(&temp).await?;
    file.write_all(bytes).await?;
    file.sync_all().await?;
    drop(file);
    tokio::fs::rename(&temp, path).await?;
    Ok(())
}

/// The order of a WAL file: its timeline, log and segment. A timeline history file has none.
fn wal_order(name: &str) -> Option<&str> {
    let head = name.get(..WAL_NAME_LEN)?;
    head.chars().all(|c| c.is_ascii_hexdigit()).then_some(head)
}

/// The order of an archived log file: the timeline, log and segment of a WAL file, or the
/// number of a binary log file (`binlog.000003`), zero-padded so that the strings sort like
/// the numbers. A timeline history file has none.
pub(super) fn log_order(name: &str) -> Option<String> {
    if let Some(order) = wal_order(name) {
        return Some(order.to_string());
    }
    let (_, number) = name.rsplit_once('.')?;
    let number: u64 = number.parse().ok()?;
    Some(format!("{number:020}"))
}

/// A file name that the archive of the server can hold, and nothing that could leave the
/// archive directory.
pub(super) fn is_archive_name(name: &str) -> bool {
    !name.is_empty()
        && !name.ends_with(".tmp")
        && name.chars().all(|c| c.is_ascii_alphanumeric() || c == '.')
}

/// The archived files that a recovery from a base backup replays: every WAL file from the
/// segment that was current when the backup started, and every timeline history file.
pub(super) fn replay_files(segments: &[WalSegment], start_wal: &str) -> Vec<WalSegment> {
    let start = log_order(start_wal).unwrap_or_else(|| start_wal.to_string());
    let mut files: Vec<WalSegment> = segments
        .iter()
        .filter(|s| s.native)
        .filter(|s| log_order(&s.filename).is_none_or(|order| order >= start))
        .cloned()
        .collect();
    files.sort_by_key(|s| log_order(&s.filename));
    files
}

/// The base backup that a recovery to `target` starts from: the newest one that ended before
/// the target. PostgreSQL stops with a fatal error when the target lies past the archived
/// WAL, so a target later than the newest archived file is refused before anything changes.
pub(super) fn pick_base<'a>(
    bases: &'a [BaseBackup],
    segments: &[WalSegment],
    target: u64,
) -> Result<&'a BaseBackup, Error> {
    let latest = segments
        .iter()
        .filter(|s| s.native)
        .map(|s| s.end_time)
        .max();
    if latest.is_none_or(|latest| target > latest) {
        return Err(Error::PitrTargetOutOfRange {
            reason: format!("the archived WAL ends at {latest:?}, before the target time {target}"),
        });
    }
    bases
        .iter()
        .filter(|base| base.created_at <= target)
        .max_by_key(|base| base.created_at)
        .ok_or_else(|| Error::PitrTargetOutOfRange {
            reason: format!("no base backup ended before the target time {target}"),
        })
}

/// The base backups that the retention removes: all but the newest `keep`.
fn expired_bases(bases: &[BaseBackup], keep: usize) -> Vec<BaseBackup> {
    let mut sorted = bases.to_vec();
    sorted.sort_by_key(|base| std::cmp::Reverse(base.created_at));
    sorted.into_iter().skip(keep.max(1)).collect()
}

/// The archived WAL files that no kept base backup needs: the files before the start of the
/// oldest kept base. History files stay, since every later timeline needs them.
fn expired_wal(segments: &[WalSegment], oldest_start: &str) -> Vec<WalSegment> {
    let Some(start) = log_order(oldest_start) else {
        return Vec::new();
    };
    segments
        .iter()
        .filter(|s| log_order(&s.filename).is_some_and(|order| order < start))
        .cloned()
        .collect()
}

/// A time for `recovery_target_time`: UTC with milliseconds, which the timestamptz input of
/// PostgreSQL reads.
fn recovery_time(unix_ms: u64) -> anyhow::Result<String> {
    let nanos = i128::from(unix_ms) * 1_000_000;
    let time = time::OffsetDateTime::from_unix_timestamp_nanos(nanos)?;
    Ok(format!(
        "{:04}-{:02}-{:02} {:02}:{:02}:{:02}.{:03}+00",
        time.year(),
        u8::from(time.month()),
        time.day(),
        time.hour(),
        time.minute(),
        time.second(),
        time.millisecond()
    ))
}

/// The volume mount of the spec that holds the data directory of the server.
pub(super) fn data_mount<'a>(
    spec: &'a AppSpec,
    data_directory: &str,
) -> anyhow::Result<&'a VolumeMount> {
    spec.volumes
        .iter()
        .filter(|mount| data_directory.starts_with(mount.target.as_str()))
        .max_by_key(|mount| mount.target.as_str().len())
        .with_context(|| format!("no volume of the database holds {data_directory}"))
}

/// The shell script that replaces the data of the server with the base backup and the WAL to
/// replay, then replays it. It first moves the current data aside, so `rollback_script` can
/// put it back. The replay runs here, in a container without a restart policy: a server that
/// stops with FATAL makes the script fail at once, where the database container would be
/// started again and again by Docker.
fn recovery_script(mount: &str, data_directory: &str, target: &str) -> String {
    format!(
        "set -e; M='{mount}'; D='{data_directory}'; P=\"$M/{PREVIOUS_DIR}\"; \
rm -rf \"$P\"; mkdir \"$P\"; \
find \"$M\" -mindepth 1 -maxdepth 1 ! -name {PREVIOUS_DIR} -exec mv {{}} \"$P\"/ \\;; \
mkdir -p \"$D\"; tar -xzf /restore/base.tar.gz -C \"$D\"; \
tar -xf /restore/wal.tar -C {WAL_ARCHIVE_PATH}; \
touch \"$D/recovery.signal\"; \
printf '%s\\n' \"restore_command = 'cp {WAL_ARCHIVE_PATH}/%f %p'\" \
\"recovery_target_time = '{target}'\" \"recovery_target_action = 'promote'\" \
>> \"$D/postgresql.auto.conf\"; \
chown -R postgres:postgres \"$M\" {WAL_ARCHIVE_PATH}; chmod 700 \"$D\"; {REPLAY}"
    )
}

/// Starts the server on the prepared data, waits until it left recovery and stops it. The
/// log of the server goes to the output of the helper, so a FATAL line reaches the error.
const REPLAY: &str = "G=$(command -v gosu || command -v su-exec); L=/tmp/recovery.log; \
fail() { cat \"$L\"; exit 1; }; \
$G postgres pg_ctl -D \"$D\" -w -t 600 -l \"$L\" start || fail; i=0; \
until [ \"$($G postgres psql -tAc 'SELECT pg_is_in_recovery()')\" = f ]; do \
$G postgres pg_ctl -D \"$D\" status >/dev/null || fail; \
i=$((i+1)); [ $i -lt 600 ] || fail; sleep 1; done; \
$G postgres pg_ctl -D \"$D\" -w -t 120 stop";

/// The shell script that puts the data that `recovery_script` moved aside back in place.
pub(super) fn rollback_script(mount: &str) -> String {
    format!(
        "set -e; M='{mount}'; P=\"$M/{PREVIOUS_DIR}\"; [ -d \"$P\" ] || exit 0; \
find \"$M\" -mindepth 1 -maxdepth 1 ! -name {PREVIOUS_DIR} -exec rm -rf {{}} +; \
find \"$P\" -mindepth 1 -maxdepth 1 -exec mv {{}} \"$M\"/ \\;; rmdir \"$P\""
    )
}

/// A tar archive in memory with the given files.
fn tar_of(files: &[(String, Vec<u8>)]) -> anyhow::Result<Vec<u8>> {
    let mut builder = tar::Builder::new(Vec::new());
    for (name, bytes) in files {
        let mut header = tar::Header::new_gnu();
        header.set_size(bytes.len() as u64);
        header.set_mode(0o644);
        header.set_cksum();
        builder.append_data(&mut header, name, bytes.as_slice())?;
    }
    Ok(builder.into_inner()?)
}

/// The image of a database app.
fn database_image(app: &AppEntry) -> anyhow::Result<String> {
    match &app.spec.source {
        AppSource::Image { image } => Ok(image.to_string()),
        _ => bail!("the database {} does not run an image", app.name),
    }
}

/// The value of one SQL query, run with `psql` as the root user of the server.
async fn psql_value(
    runtime: &Arc<dyn ContainerRuntime>,
    container: &ContainerName,
    info: &EngineInfo,
    password: &str,
    sql: &str,
) -> anyhow::Result<String> {
    let command = [
        "psql",
        "-U",
        &info.user,
        "-d",
        &info.default_db,
        "-tAc",
        sql,
    ]
    .map(str::to_string)
    .to_vec();
    let env = vec![format!("PGPASSWORD={password}")];
    let out = runtime.exec_capture(container, command, env).await?;
    Ok(String::from_utf8_lossy(&out).trim().to_string())
}

/// The error of a database that runs without the archive volume: it was deployed before
/// archiving and starts archiving on its next deployment.
#[derive(Debug)]
struct NotArchiving;

impl std::fmt::Display for NotArchiving {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("the database writes no WAL archive; deploy it again to turn archiving on")
    }
}

/// Whether a failed pull of the loop is an expected state rather than a fault: another job
/// holds the database, or the database runs without the archive volume. Every other failure
/// stops the archive and must reach the log.
fn is_expected_pull_failure(err: &anyhow::Error) -> bool {
    err.downcast_ref::<NotArchiving>().is_some()
        || matches!(err.downcast_ref::<Error>(), Some(Error::AppBusy { .. }))
}

/// The names of the closed files in the archive volume of the container, sorted.
async fn archive_names(
    runtime: &Arc<dyn ContainerRuntime>,
    container: &ContainerName,
) -> anyhow::Result<Vec<String>> {
    let entries = runtime
        .list_dir(container, WAL_ARCHIVE_PATH)
        .await
        .context(NotArchiving)?;
    let mut names: Vec<String> = entries
        .into_iter()
        .filter(|entry| !entry.dir && is_archive_name(&entry.name))
        .map(|entry| entry.name)
        .collect();
    names.sort();
    Ok(names)
}

/// Runs a one-off container on the volumes of a database and waits for its exit.
async fn run_helper(
    runtime: &Arc<dyn ContainerRuntime>,
    spec: ContainerSpec,
    upload: Option<Vec<u8>>,
) -> anyhow::Result<()> {
    let name = spec.name.clone();
    let _ = runtime.remove_container(&name).await;
    runtime.create_container(&spec).await?;
    let result = async {
        if let Some(archive) = upload {
            runtime.upload_archive(&name, "/", archive).await?;
        }
        runtime.start_container(&name).await?;
        wait_exit(runtime, &name).await
    }
    .await;
    let _ = runtime.remove_container(&name).await;
    result
}

async fn wait_exit(
    runtime: &Arc<dyn ContainerRuntime>,
    name: &ContainerName,
) -> anyhow::Result<()> {
    let deadline = tokio::time::Instant::now() + RECOVERY_TIMEOUT;
    loop {
        let info = runtime
            .inspect_container(name)
            .await?
            .with_context(|| format!("the container {name} disappeared"))?;
        if !info.running {
            ensure!(
                info.exit_code == 0,
                "{name} exited with code {}: {}",
                info.exit_code,
                runtime.logs(name, 20).await.unwrap_or_default()
            );
            return Ok(());
        }
        ensure!(
            tokio::time::Instant::now() < deadline,
            "{name} did not finish in time"
        );
        tokio::time::sleep(Duration::from_millis(500)).await;
    }
}

/// The last fatal line of the log of a server that stopped during a recovery.
async fn fatal_line(runtime: &Arc<dyn ContainerRuntime>, name: &ContainerName) -> String {
    let log = runtime.logs(name, 50).await.unwrap_or_default();
    log.lines()
        .rev()
        .find(|line| line.contains("FATAL"))
        .unwrap_or("the server stopped without a FATAL line")
        .to_string()
}

/// What a recovery replaces the data of a database with.
struct Recovery<'a> {
    source: ShortId,
    base: &'a BaseBackup,
    files: &'a [WalSegment],
    target: String,
}

/// The base backups in the directory `dir`, oldest first.
pub(super) async fn bases_in(dir: &Path) -> anyhow::Result<Vec<BaseBackup>> {
    let mut bases = Vec::new();
    let mut reader = match tokio::fs::read_dir(dir).await {
        Ok(reader) => reader,
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => return Ok(bases),
        Err(err) => return Err(err.into()),
    };
    while let Some(file) = reader.next_entry().await? {
        let path = file.path();
        if path.extension().is_some_and(|ext| ext == "json") {
            let json = tokio::fs::read(&path).await?;
            bases.push(serde_json::from_slice::<BaseBackup>(&json)?);
        }
    }
    bases.sort_by_key(|base| base.created_at);
    Ok(bases)
}

impl Platform {
    pub(super) async fn base_dir(&self, db_id: ShortId) -> anyhow::Result<PathBuf> {
        Ok(self.backup_dir(db_id).await?.join(BASE_DIR))
    }

    /// The physical base backups of a database, oldest first.
    pub(super) async fn list_base_backups(
        &self,
        db_id: ShortId,
    ) -> anyhow::Result<Vec<BaseBackup>> {
        bases_in(&self.base_dir(db_id).await?).await
    }

    /// The archived files of the database that the engine wrote itself.
    pub(super) async fn native_segments(&self, db_id: ShortId) -> anyhow::Result<Vec<WalSegment>> {
        let mut segments = self.list_wal_segments(db_id).await?;
        segments.retain(|s| s.native);
        Ok(segments)
    }

    /// The running container of a PostgreSQL database with its root credentials.
    async fn pg_session(&self, db_id: ShortId) -> anyhow::Result<PgSession> {
        let (app, container, runtime) = self.database_container(db_id).await?;
        let info = r3v3rs3_api::platform::app_engine_info(&app.spec)
            .context("the engine is not in the database catalog")?;
        let password = self.database_password(db_id, &info).await?;
        Ok(PgSession {
            container,
            runtime,
            info,
            password,
        })
    }

    /// Takes a physical base backup of the running server with `pg_basebackup`. The archive
    /// holds the WAL that the backup needs, so the backup carries none (`-X none`).
    async fn take_base_backup_locked(&self, db_id: ShortId) -> anyhow::Result<BaseBackup> {
        let pg = self.pg_session(db_id).await?;
        let (start_wal, data_directory) = pg.base_start().await?;
        let archive = pg.basebackup().await?;
        let start = BaseStart {
            log: start_wal,
            position: None,
            data_directory,
        };
        let base = self.store_base(db_id, start, &archive).await?;
        self.prune_base_backups(db_id).await?;
        Ok(base)
    }

    /// Stores the archive of a base backup with its manifest.
    pub(super) async fn store_base(
        &self,
        db_id: ShortId,
        start: BaseStart,
        archive: &[u8],
    ) -> anyhow::Result<BaseBackup> {
        let compressed = gzip(archive).await?;
        let base = BaseBackup {
            id: backup_id(),
            created_at: unix_ms(),
            start_wal: start.log,
            start_position: start.position,
            data_directory: start.data_directory,
            size: compressed.len() as u64,
            sha256: hex::encode(Sha256::digest(&compressed)),
        };
        let dir = self.base_dir(db_id).await?;
        tokio::fs::create_dir_all(&dir).await?;
        write_atomic(&dir.join(format!("{}.tar.gz", base.id)), &compressed).await?;
        let manifest = serde_json::to_vec(&base)?;
        write_atomic(&dir.join(format!("{}.json", base.id)), &manifest).await?;
        Ok(base)
    }

    /// Stores the local file `src` as the archive of a base backup, compressed on disk, so a
    /// base of any size passes without being held in memory.
    pub(super) async fn store_base_file(
        &self,
        db_id: ShortId,
        start: BaseStart,
        src: &Path,
    ) -> anyhow::Result<BaseBackup> {
        let dir = self.base_dir(db_id).await?;
        tokio::fs::create_dir_all(&dir).await?;
        let id = backup_id();
        let (size, sha256) = gzip_file(src, &dir.join(format!("{id}.tar.gz"))).await?;
        let base = BaseBackup {
            id,
            created_at: unix_ms(),
            start_wal: start.log,
            start_position: start.position,
            data_directory: start.data_directory,
            size,
            sha256,
        };
        let manifest = serde_json::to_vec(&base)?;
        write_atomic(&dir.join(format!("{}.json", base.id)), &manifest).await?;
        Ok(base)
    }

    /// Takes a base backup of a database that archives its log.
    pub(super) async fn take_base_backup(&self, db_id: ShortId) -> anyhow::Result<BaseBackup> {
        let app = self.app(db_id).await?;
        let _lock = self.lock_app(&app)?;
        match app.spec.database.as_deref() {
            Some("mysql" | "mariadb") => self.take_binlog_base_locked(db_id).await,
            Some("mongodb") => self.take_oplog_base_locked(db_id).await,
            Some("redis") => self.take_aof_base_locked(db_id).await,
            _ => self.take_base_backup_locked(db_id).await,
        }
    }

    /// Keeps the newest base backups of the retention, and removes the archived WAL that only
    /// the removed backups needed.
    pub(super) async fn prune_base_backups(&self, db_id: ShortId) -> anyhow::Result<()> {
        self.remove_expired_bases(db_id).await?;
        let keep = self.backup_config().await?.keep as usize;
        self.prune_histories(db_id, keep).await?;
        let Some(oldest) = self.list_base_backups(db_id).await?.into_iter().next() else {
            return Ok(());
        };
        let wal = self.wal_dir(db_id).await?;
        for segment in expired_wal(&self.native_segments(db_id).await?, &oldest.start_wal) {
            remove_segment(&wal, &segment.id).await?;
        }
        Ok(())
    }

    /// Removes the base backups beyond the retention of the backup configuration.
    async fn remove_expired_bases(&self, db_id: ShortId) -> anyhow::Result<()> {
        let keep = self.backup_config().await?.keep as usize;
        let dir = self.base_dir(db_id).await?;
        for base in expired_bases(&self.list_base_backups(db_id).await?, keep) {
            tokio::fs::remove_file(dir.join(format!("{}.tar.gz", base.id))).await?;
            tokio::fs::remove_file(dir.join(format!("{}.json", base.id))).await?;
        }
        Ok(())
    }

    /// Removes the segments of the older archive, which held dumps instead of the WAL.
    pub(super) async fn drop_legacy_segments(&self, db_id: ShortId) -> anyhow::Result<()> {
        let dir = self.wal_dir(db_id).await?;
        let legacy: Vec<WalSegment> = self
            .list_wal_segments(db_id)
            .await?
            .into_iter()
            .filter(|s| !s.native)
            .collect();
        for segment in &legacy {
            remove_segment(&dir, &segment.id).await?;
        }
        if !legacy.is_empty() {
            tracing::info!(database = %db_id, count = legacy.len(), "removed the dump segments of the old WAL archive");
        }
        Ok(())
    }

    /// Copies the closed WAL files of the archive volume into the backup directory and removes
    /// them from the volume once they are stored.
    async fn pull_wal_locked(&self, db_id: ShortId) -> anyhow::Result<Vec<WalSegment>> {
        let (_, container, runtime) = self.database_container(db_id).await?;
        let names = archive_names(&runtime, &container).await?;
        self.drop_legacy_segments(db_id).await?;
        let dir = self.wal_dir(db_id).await?;
        tokio::fs::create_dir_all(&dir).await?;
        let mut start = self.archive_end(db_id).await?;
        let mut pulled = Vec::new();
        for name in names {
            let segment = pull_file(&runtime, &container, &dir, db_id, name, start).await?;
            start = segment.end_time;
            pulled.push(segment);
        }
        Ok(pulled)
    }

    /// The end time of the newest archived WAL file, or zero for an empty archive.
    pub(super) async fn archive_end(&self, db_id: ShortId) -> anyhow::Result<u64> {
        let segments = self.native_segments(db_id).await?;
        Ok(segments.iter().map(|s| s.end_time).max().unwrap_or(0))
    }

    /// Pulls the closed WAL files of one database, unless another job holds it.
    pub(super) async fn pull_wal(&self, db_id: ShortId) -> anyhow::Result<Vec<WalSegment>> {
        let app = self.app(db_id).await?;
        let _lock = self.lock_app(&app)?;
        match app.spec.database.as_deref() {
            Some("mysql" | "mariadb") => self.pull_binlog_locked(db_id, false).await,
            Some("mongodb") => self.pull_oplog_locked(db_id).await,
            Some("redis") => self.pull_aof_locked(db_id).await,
            _ => self.pull_wal_locked(db_id).await,
        }
    }

    /// Closes the current WAL file of the server, pulls the archive and returns the closed
    /// file. A database without a base backup gets one, so the archive can be replayed.
    pub(super) async fn archive_postgres_wal(&self, db_id: ShortId) -> anyhow::Result<WalSegment> {
        let app = self.app(db_id).await?;
        let _lock = self.lock_app(&app)?;
        let pg = self.pg_session(db_id).await?;
        archive_names(&pg.runtime, &pg.container).await?;
        let (closed, mark) = pg.switch_wal().await?;
        self.wait_archived(db_id, &pg, closed.as_deref()).await?;
        self.pull_wal_locked(db_id).await?;
        // The first base backup starts after the closed file, so its retention removes that
        // file; the segment is read before the backup.
        let segment = self.closed_segment(db_id, closed.as_deref(), mark).await?;
        self.ensure_base_locked(db_id).await?;
        Ok(segment)
    }

    /// The stored segment of the file that the switch closed, ending before its own commit.
    async fn closed_segment(
        &self,
        db_id: ShortId,
        closed: Option<&str>,
        mark: u64,
    ) -> anyhow::Result<WalSegment> {
        let segment = self.archived_file(db_id, closed).await?;
        if closed.is_none() {
            return Ok(segment);
        }
        self.end_segment_at(db_id, segment, mark).await
    }

    /// Sets the end of a segment to the time before the commit that closed it. A target after
    /// that commit would need WAL that the archive does not hold yet.
    async fn end_segment_at(
        &self,
        db_id: ShortId,
        mut segment: WalSegment,
        mark: u64,
    ) -> anyhow::Result<WalSegment> {
        segment.end_time = mark.min(segment.end_time);
        let dir = self.wal_dir(db_id).await?;
        let manifest = serde_json::to_vec(&segment)?;
        write_atomic(&dir.join(format!("{}.json", segment.id)), &manifest).await?;
        Ok(segment)
    }

    /// The stored segment of the WAL file `name`, or the newest stored file when the switch
    /// closed none.
    pub(super) async fn archived_file(
        &self,
        db_id: ShortId,
        name: Option<&str>,
    ) -> anyhow::Result<WalSegment> {
        let segments = self.native_segments(db_id).await?;
        let found = match name {
            Some(name) => segments.into_iter().rfind(|s| s.filename == name),
            None => segments
                .into_iter()
                .filter(|s| log_order(&s.filename).is_some())
                .max_by_key(|s| log_order(&s.filename)),
        };
        found.with_context(|| format!("the WAL file {name:?} is not in the archive"))
    }

    /// Closes and pulls the current WAL file of a running server, so the archive reaches now.
    async fn close_current_wal(&self, db_id: ShortId) -> anyhow::Result<()> {
        if self.store.running_deployment(db_id).await?.is_some() {
            self.archive_postgres_wal(db_id).await?;
        }
        Ok(())
    }

    /// Takes the first base backup of a database whose archive has none.
    async fn ensure_base_locked(&self, db_id: ShortId) -> anyhow::Result<()> {
        if self.list_base_backups(db_id).await?.is_empty() {
            self.take_base_backup_locked(db_id).await?;
        }
        Ok(())
    }

    /// Waits until the server archived `closed`, or the platform already pulled it.
    /// A switch that closed no file has nothing to wait for.
    async fn wait_archived(
        &self,
        db_id: ShortId,
        pg: &PgSession,
        closed: Option<&str>,
    ) -> anyhow::Result<()> {
        let Some(closed) = closed else {
            return Ok(());
        };
        let deadline = tokio::time::Instant::now() + Duration::from_secs(30);
        loop {
            let in_volume = archive_names(&pg.runtime, &pg.container).await?;
            let pulled = self.native_segments(db_id).await?;
            if in_volume.iter().any(|n| n == closed) || pulled.iter().any(|s| s.filename == closed)
            {
                return Ok(());
            }
            ensure!(
                tokio::time::Instant::now() < deadline,
                "the server did not archive the WAL file {closed}; read pg_stat_archiver"
            );
            tokio::time::sleep(Duration::from_millis(250)).await;
        }
    }

    /// The recovery window of a PostgreSQL database: from the end of its oldest base backup
    /// to the end of its newest archived WAL file.
    pub(super) async fn postgres_pitr_range(&self, db_id: ShortId) -> anyhow::Result<PitrRange> {
        let bases = self.list_base_backups(db_id).await?;
        let segments = self.native_segments(db_id).await?;
        let earliest = bases.first().map(|base| base.created_at);
        let latest = segments.iter().map(|s| s.end_time).max();
        let archiving = match self.database_container(db_id).await {
            Ok((_, container, runtime)) => archive_names(&runtime, &container).await.is_ok(),
            Err(_) => false,
        };
        Ok(PitrRange {
            earliest,
            latest,
            base_backups_count: bases.len(),
            wal_segments_count: segments.len(),
            active: matches!((earliest, latest), (Some(e), Some(l)) if l >= e),
            archiving,
        })
    }

    /// Recovers a PostgreSQL database to `request.target_time`, in place or into a new branch.
    pub(super) async fn restore_postgres_pitr(
        self: &Arc<Self>,
        db_id: ShortId,
        app: &AppEntry,
        request: PitrRestoreRequest,
    ) -> anyhow::Result<PitrRestoreResponse> {
        // The newest WAL may still sit in the server; closing it extends the archive to now.
        self.close_current_wal(db_id).await?;
        let bases = self.list_base_backups(db_id).await?;
        let segments = self.native_segments(db_id).await?;
        let base = pick_base(&bases, &segments, request.target_time)?.clone();
        let files = replay_files(&segments, &base.start_wal);
        let (target, branched) = self.recovery_target(db_id, app, &request).await?;
        let recovery = Recovery {
            source: db_id,
            base: &base,
            files: &files,
            target: recovery_time(request.target_time)?,
        };
        self.recover_into(target.id, &recovery).await?;
        self.reapply_tenants(target.id).await?;
        // The promoted server writes a new timeline, and a replay from an older base backup
        // cannot follow it without the history file of the timeline. A base backup of the new
        // timeline makes the writes after the recovery recoverable.
        self.take_base_backup(target.id)
            .await
            .context("the recovered database took no new base backup")?;
        Ok(PitrRestoreResponse {
            app: target,
            target_time: request.target_time,
            base_backup_id: base.id,
            wal_segments_replayed: files.len(),
            branched,
        })
    }

    /// The database that a recovery fills: a new branch when the request names one, else the
    /// source itself. The flag tells whether a branch was created.
    pub(super) async fn recovery_target(
        self: &Arc<Self>,
        db_id: ShortId,
        app: &AppEntry,
        request: &PitrRestoreRequest,
    ) -> anyhow::Result<(AppEntry, bool)> {
        let Some(name) = request.new_name.clone() else {
            return Ok((app.clone(), false));
        };
        let branch = self
            .postgres_branch(db_id, app, name, request.host_port)
            .await?;
        Ok((branch, true))
    }

    /// Creates and deploys the branch database that a recovery fills.
    async fn postgres_branch(
        self: &Arc<Self>,
        source_id: ShortId,
        source: &AppEntry,
        name: r3v3rs3_api::container::AppName,
        host_port: Option<u16>,
    ) -> anyhow::Result<AppEntry> {
        let info = r3v3rs3_api::platform::app_engine_info(&source.spec)
            .context("the engine is not in the database catalog")?;
        let branch = self.create_pitr_app(source, name, host_port).await?;
        self.copy_tenants(source_id, branch.id).await?;
        self.sync_pitr_password(source_id, branch.id, &info).await?;
        self.deploy(
            branch.id,
            "pitr",
            r3v3rs3_api::platform::DeploymentTrigger::Manual,
        )
        .await?;
        let deadline = tokio::time::Instant::now() + RECOVERY_TIMEOUT;
        // The recovery takes the lock of the branch, which the deployment holds until its
        // job ends, after the container already runs.
        loop {
            // The guard of the busy set is released before the next await.
            let busy = self.busy_apps().contains(&branch.id);
            if !busy && self.database_container(branch.id).await.is_ok() {
                break;
            }
            if !busy {
                self.fail_on_failed_deployment(&branch).await?;
            }
            ensure!(
                tokio::time::Instant::now() < deadline,
                "the branch database {} did not start",
                branch.name
            );
            tokio::time::sleep(Duration::from_secs(1)).await;
        }
        Ok(branch)
    }

    /// Fails with the reason of the newest deployment of `branch` when it failed, so a branch
    /// that cannot start reports why instead of waiting for the timeout.
    async fn fail_on_failed_deployment(&self, branch: &AppEntry) -> anyhow::Result<()> {
        let latest = self.deployments(branch.id).await?.into_iter().next();
        if let Some(deployment) = latest
            && deployment.status == r3v3rs3_api::platform::DeploymentStatus::Failed
        {
            bail!(
                "the branch database {} did not start: {}",
                branch.name,
                deployment.message.unwrap_or_default()
            );
        }
        Ok(())
    }

    /// Replaces the data of the database with the base backup, replays the WAL up to the
    /// target and waits for the server to promote. A failed recovery puts the old data back.
    async fn recover_into(&self, db_id: ShortId, recovery: &Recovery<'_>) -> anyhow::Result<()> {
        let app = self.app(db_id).await?;
        let _lock = self.lock_app(&app)?;
        let (app, container, runtime) = self.database_container(db_id).await?;
        let mount = data_mount(&app.spec, &recovery.base.data_directory)?
            .target
            .as_str()
            .to_string();
        let upload = self.recovery_upload(recovery).await?;
        runtime
            .stop_container(&container, Duration::from_secs(60))
            .await?;
        let script = recovery_script(&mount, &recovery.base.data_directory, &recovery.target);
        let replaced = self
            .run_db_helper(&runtime, &app, script, Some(upload))
            .await;
        let replayed = match replaced {
            Ok(()) => wait_ready(&runtime, &container).await,
            Err(err) => Err(err),
        };
        match replayed {
            Ok(()) => finish_recovery(&runtime, &container, &mount, recovery.files).await,
            Err(err) => {
                self.roll_back(&runtime, &app, &container, &mount).await?;
                Err(Error::RestoreFailed {
                    reason: format!("{err:#}"),
                }
                .into())
            }
        }
    }

    /// The tar archive that a recovery uploads: the base backup and the WAL files.
    async fn recovery_upload(&self, recovery: &Recovery<'_>) -> anyhow::Result<Vec<u8>> {
        let base_dir = self.base_dir(recovery.source).await?;
        let base = tokio::fs::read(base_dir.join(format!("{}.tar.gz", recovery.base.id))).await?;
        let wal_dir = self.wal_dir(recovery.source).await?;
        let mut wal = Vec::new();
        for segment in recovery.files {
            let path = wal_dir.join(format!("{}.wal.gz", segment.id));
            let compressed = tokio::fs::read(&path)
                .await
                .with_context(|| format!("the WAL file {} is missing", segment.filename))?;
            wal.push((segment.filename.clone(), ungzip(&compressed).await?));
        }
        tar_of(&[
            ("restore/base.tar.gz".to_string(), base),
            ("restore/wal.tar".to_string(), tar_of(&wal)?),
        ])
    }

    /// Runs a one-off container of the database image on the volumes of the database.
    pub(super) async fn run_db_helper(
        &self,
        runtime: &Arc<dyn ContainerRuntime>,
        app: &AppEntry,
        script: String,
        upload: Option<Vec<u8>>,
    ) -> anyhow::Result<()> {
        let spec = ContainerSpec {
            name: format!("r3v3rs3-pitr-{}", app.id).parse()?,
            image: database_image(app)?.parse()?,
            env: Vec::new(),
            labels: BTreeMap::new(),
            network: None,
            volumes: container_volumes(app.id, &app.spec, true)?,
            restart: RestartPolicy::No,
            limits: ResourceLimits::default(),
            publish: None,
            host_port: None,
            aliases: Vec::new(),
            extra_networks: Vec::new(),
            log_max_size: None,
            log_max_file: None,
            command: Some(vec!["sh".to_string(), "-c".to_string(), script]),
        };
        run_helper(runtime, spec, upload).await
    }

    /// Puts the data that a failed recovery moved aside back and starts the server again.
    async fn roll_back(
        &self,
        runtime: &Arc<dyn ContainerRuntime>,
        app: &AppEntry,
        container: &ContainerName,
        mount: &str,
    ) -> anyhow::Result<()> {
        runtime
            .stop_container(container, Duration::from_secs(60))
            .await?;
        self.run_db_helper(runtime, app, rollback_script(mount), None)
            .await
            .context("the old data of the database could not be put back")?;
        // The caller reports the failure only once the database answers again.
        wait_ready(runtime, container)
            .await
            .context("the database did not answer on its old data")
    }
}

/// Starts the database container on the recovered data and waits until it answers. The
/// wait is bounded: Docker restarts a server that stops, so the container can look running
/// while it never answers.
async fn wait_ready(
    runtime: &Arc<dyn ContainerRuntime>,
    container: &ContainerName,
) -> anyhow::Result<()> {
    runtime.start_container(container).await?;
    let deadline = tokio::time::Instant::now() + READY_TIMEOUT;
    let command: Vec<String> = ["psql", "-U", "postgres", "-tAc", "SELECT 1"]
        .map(str::to_string)
        .to_vec();
    loop {
        if let Ok(out) = runtime
            .exec_capture(container, command.clone(), Vec::new())
            .await
            && String::from_utf8_lossy(&out).trim() == "1"
        {
            return Ok(());
        }
        if tokio::time::Instant::now() >= deadline {
            bail!("{}", fatal_line(runtime, container).await);
        }
        tokio::time::sleep(Duration::from_secs(1)).await;
    }
}

/// Removes the data that the recovery kept aside and the replayed copies of the WAL, which
/// the platform already holds.
async fn finish_recovery(
    runtime: &Arc<dyn ContainerRuntime>,
    container: &ContainerName,
    mount: &str,
    files: &[WalSegment],
) -> anyhow::Result<()> {
    let mut paths = vec![format!("{mount}/{PREVIOUS_DIR}")];
    paths.extend(
        files
            .iter()
            .map(|s| format!("{WAL_ARCHIVE_PATH}/{}", s.filename)),
    );
    let mut command = vec!["rm".to_string(), "-rf".to_string()];
    command.extend(paths);
    runtime.exec_capture(container, command, Vec::new()).await?;
    Ok(())
}

/// The running container of a PostgreSQL database with its root credentials.
struct PgSession {
    container: ContainerName,
    runtime: Arc<dyn ContainerRuntime>,
    info: EngineInfo,
    password: String,
}

impl PgSession {
    async fn value(&self, sql: &str) -> anyhow::Result<String> {
        psql_value(
            &self.runtime,
            &self.container,
            &self.info,
            &self.password,
            sql,
        )
        .await
    }

    /// The current WAL file, where a recovery from the next backup starts, and the data
    /// directory of the server.
    async fn base_start(&self) -> anyhow::Result<(String, String)> {
        let sql = "SELECT pg_walfile_name(pg_current_wal_lsn()) || ' ' || current_setting('data_directory')";
        let row = self.value(sql).await?;
        let (start_wal, data_directory) = row
            .split_once(' ')
            .with_context(|| format!("unexpected answer of the server: {row}"))?;
        Ok((start_wal.to_string(), data_directory.to_string()))
    }

    /// Closes the current WAL file and returns its name. Without a write since the last switch,
    /// `pg_switch_wal()` closes nothing and `pg_walfile_name` of its answer names the open
    /// file, which is never archived; so the name is read before and after the switch.
    ///
    /// A recovery stops only on a commit record later than its target, and ends with FATAL
    /// when the WAL runs out first. So the switch first commits a transaction of its own: the
    /// closed file then holds a commit at or after the returned time, and every target up to
    /// that time can be reached.
    async fn switch_wal(&self) -> anyhow::Result<(Option<String>, u64)> {
        let mark = unix_ms();
        self.value("SELECT pg_current_xact_id()").await?;
        let current = "SELECT pg_walfile_name(pg_current_wal_insert_lsn())";
        let before = self.value(current).await?;
        self.value("SELECT pg_switch_wal()").await?;
        let after = self.value(current).await?;
        Ok(((before != after).then_some(before), mark))
    }

    /// The tar archive of the data directory that `pg_basebackup` writes to stdout.
    async fn basebackup(&self) -> anyhow::Result<Vec<u8>> {
        let user = self.info.user.as_str();
        let command = ["pg_basebackup", "-D", "-", "-Ft", "-X", "none", "-U", user]
            .map(str::to_string)
            .to_vec();
        let env = vec![format!("PGPASSWORD={}", self.password)];
        let archive = self
            .runtime
            .exec_capture(&self.container, command, env)
            .await?;
        ensure!(!archive.is_empty(), "pg_basebackup wrote no archive");
        Ok(archive)
    }
}

/// Copies one closed WAL file out of the archive volume, stores it and removes it from the
/// volume. The file leaves the volume only after it is stored.
async fn pull_file(
    runtime: &Arc<dyn ContainerRuntime>,
    container: &ContainerName,
    dir: &Path,
    db_id: ShortId,
    name: String,
    start: u64,
) -> anyhow::Result<WalSegment> {
    let path = format!("{WAL_ARCHIVE_PATH}/{name}");
    let segment =
        fetch_native_segment(runtime, container, (&path, dir), db_id, name, start).await?;
    runtime.remove_path(container, &path).await?;
    Ok(segment)
}

/// Copies the closed log file at `path` inside the container into the archive `dir` in
/// bounded pieces and records it. A log file can outgrow any single download.
pub(super) async fn fetch_native_segment(
    runtime: &Arc<dyn ContainerRuntime>,
    container: &ContainerName,
    (path, dir): (&str, &Path),
    db_id: ShortId,
    filename: String,
    start_time: u64,
) -> anyhow::Result<WalSegment> {
    let scratch = Scratch::new(dir).await?;
    let raw = scratch.path("log");
    let copied = fetch_to_file(runtime, container, (path, 0), &raw).await?;
    ensure!(copied > 0, "the archived WAL file {filename} is empty");
    let id = backup_id();
    let (size, sha256) = gzip_file(&raw, &dir.join(format!("{id}.wal.gz"))).await?;
    let now = unix_ms();
    let segment = WalSegment {
        id,
        app: db_id,
        filename,
        size,
        sha256,
        start_time,
        end_time: now,
        created_at: now,
        native: true,
    };
    write_atomic(
        &dir.join(format!("{}.json", segment.id)),
        &serde_json::to_vec(&segment)?,
    )
    .await?;
    Ok(segment)
}

pub(super) async fn remove_segment(dir: &Path, id: &str) -> anyhow::Result<()> {
    for name in [format!("{id}.wal.gz"), format!("{id}.json")] {
        match tokio::fs::remove_file(dir.join(name)).await {
            Ok(()) => {}
            Err(err) if err.kind() == std::io::ErrorKind::NotFound => {}
            Err(err) => return Err(err.into()),
        }
    }
    Ok(())
}

/// Stores one WAL file that the server archived, with its manifest.
pub(super) async fn store_native_segment(
    dir: &Path,
    db_id: ShortId,
    filename: String,
    bytes: &[u8],
    start_time: u64,
) -> anyhow::Result<WalSegment> {
    ensure!(
        !bytes.is_empty(),
        "the archived WAL file {filename} is empty"
    );
    write_segment(dir, db_id, filename, bytes, start_time).await
}

/// Stores an empty piece that marks how far the archive of an append-only log reaches when no
/// write followed the base backup. Only the Redis AOF archive uses it: an empty file of the
/// other engines is a damaged one.
pub(super) async fn store_marker_segment(
    dir: &Path,
    db_id: ShortId,
    filename: String,
    start_time: u64,
) -> anyhow::Result<WalSegment> {
    write_segment(dir, db_id, filename, &[], start_time).await
}

async fn write_segment(
    dir: &Path,
    db_id: ShortId,
    filename: String,
    bytes: &[u8],
    start_time: u64,
) -> anyhow::Result<WalSegment> {
    let compressed = gzip(bytes).await?;
    let now = unix_ms();
    let segment = WalSegment {
        id: backup_id(),
        app: db_id,
        filename,
        size: compressed.len() as u64,
        sha256: hex::encode(Sha256::digest(&compressed)),
        start_time,
        end_time: now,
        created_at: now,
        native: true,
    };
    write_atomic(&dir.join(format!("{}.wal.gz", segment.id)), &compressed).await?;
    write_atomic(
        &dir.join(format!("{}.json", segment.id)),
        &serde_json::to_vec(&segment)?,
    )
    .await?;
    Ok(segment)
}

/// Pulls the WAL of every local PostgreSQL database once a minute, and takes the first base
/// backup of a database whose archive has none.
pub fn spawn_puller(platform: &Arc<Platform>) {
    let weak = Arc::downgrade(platform);
    let cancel = platform.shutdown_token().clone();
    tokio::spawn(async move {
        loop {
            tokio::select! {
                _ = cancel.cancelled() => return,
                _ = tokio::time::sleep(PULL_ROUND) => {}
            }
            let Some(platform) = weak.upgrade() else {
                return;
            };
            if let Err(err) = platform.pull_round().await {
                tracing::error!(err = format!("{err:#}"), "the WAL pull failed");
            }
        }
    });
}

impl Platform {
    async fn pull_round(&self) -> anyhow::Result<()> {
        for app in self.apps().await? {
            if !archives(app.spec.database.as_deref(), app.target)
                || self.store.running_deployment(app.id).await?.is_none()
            {
                continue;
            }
            self.pull_one_database(&app).await;
        }
        Ok(())
    }

    async fn pull_one_database(&self, app: &AppEntry) {
        match self.pull_wal(app.id).await {
            Ok(_) => {}
            Err(err) if is_expected_pull_failure(&err) => {
                tracing::debug!(database = %app.name, err = format!("{err:#}"), "no WAL pulled");
                return;
            }
            Err(err) => {
                tracing::error!(database = %app.name, err = format!("{err:#}"), "the WAL pull failed");
                return;
            }
        }
        let has_base = self
            .list_base_backups(app.id)
            .await
            .is_ok_and(|bases| !bases.is_empty());
        if !has_base && let Err(err) = self.take_base_backup(app.id).await {
            tracing::error!(database = %app.name, err = format!("{err:#}"), "the base backup failed");
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn segment(filename: &str, end_time: u64) -> WalSegment {
        WalSegment {
            id: filename.to_string(),
            app: "abc-def".parse().unwrap(),
            filename: filename.to_string(),
            size: 1,
            sha256: String::new(),
            start_time: 0,
            end_time,
            created_at: end_time,
            native: true,
        }
    }

    fn base(id: &str, created_at: u64, start_wal: &str) -> BaseBackup {
        BaseBackup {
            id: id.to_string(),
            created_at,
            start_wal: start_wal.to_string(),
            start_position: None,
            data_directory: "/var/lib/postgresql/data".to_string(),
            size: 1,
            sha256: String::new(),
        }
    }

    /// PostgreSQL stops with "recovery ended before configured recovery target was reached"
    /// when the target lies past the archived WAL, so the platform refuses it first.
    #[test]
    fn a_target_after_the_archived_wal_is_refused() {
        let bases = [base("a", 1_000, "000000010000000000000002")];
        let segments = [segment("000000010000000000000002", 5_000)];
        let err = pick_base(&bases, &segments, 5_001).unwrap_err();
        assert!(matches!(err, Error::PitrTargetOutOfRange { .. }), "{err}");
        let err = pick_base(&bases, &[], 2_000).unwrap_err();
        assert!(matches!(err, Error::PitrTargetOutOfRange { .. }), "{err}");
    }

    /// A recovery starts from a backup that ended before the target; PostgreSQL cannot stop
    /// before the end of its base backup.
    #[test]
    fn the_newest_base_before_the_target_is_chosen() {
        let bases = [
            base("old", 1_000, "000000010000000000000002"),
            base("new", 3_000, "000000010000000000000005"),
        ];
        let segments = [segment("000000010000000000000006", 9_000)];
        assert_eq!(pick_base(&bases, &segments, 2_000).unwrap().id, "old");
        assert_eq!(pick_base(&bases, &segments, 3_000).unwrap().id, "new");
        let err = pick_base(&bases, &segments, 999).unwrap_err();
        assert!(matches!(err, Error::PitrTargetOutOfRange { .. }), "{err}");
    }

    /// Recovery reads the WAL from the segment that was current when the backup started, and
    /// the history files of the timelines; older files are not needed.
    #[test]
    fn the_replay_starts_at_the_segment_of_the_base_backup() {
        let segments = [
            segment("000000010000000000000003", 1),
            segment("000000010000000000000004", 2),
            segment("000000010000000000000004.00000028.backup", 2),
            segment("00000002.history", 3),
            segment("000000020000000000000005", 4),
        ];
        // The files land in one directory, so only the set matters.
        let names: std::collections::BTreeSet<String> =
            replay_files(&segments, "000000010000000000000004")
                .into_iter()
                .map(|s| s.filename)
                .collect();
        let expected = [
            "000000010000000000000004",
            "000000010000000000000004.00000028.backup",
            "000000020000000000000005",
            "00000002.history",
        ]
        .map(str::to_string);
        assert_eq!(names, expected.into_iter().collect());
    }

    /// A dump segment of the old archive is not a log and is never replayed.
    #[test]
    fn a_dump_segment_is_never_replayed() {
        let mut dump = segment("000000010000000000000009", 1);
        dump.native = false;
        assert!(replay_files(&[dump], "000000010000000000000001").is_empty());
    }

    /// The retention keeps the newest backups, and the WAL that the oldest kept backup needs.
    #[test]
    fn the_retention_keeps_the_wal_of_the_oldest_kept_base() {
        let bases = [
            base("a", 1, "000000010000000000000002"),
            base("b", 2, "000000010000000000000005"),
            base("c", 3, "000000010000000000000008"),
        ];
        let expired: Vec<String> = expired_bases(&bases, 2).into_iter().map(|b| b.id).collect();
        assert_eq!(expired, ["a"]);
        let segments = [
            segment("000000010000000000000004", 1),
            segment("000000010000000000000005", 2),
            segment("00000002.history", 3),
        ];
        let gone: Vec<String> = expired_wal(&segments, "000000010000000000000005")
            .into_iter()
            .map(|s| s.filename)
            .collect();
        assert_eq!(gone, ["000000010000000000000004"]);
    }

    /// The timestamptz input of PostgreSQL reads an ISO date with an explicit UTC offset.
    /// `date -u -r 1795000123` gives 2026-11-18 11:08:43.
    #[test]
    fn the_recovery_time_is_utc_with_milliseconds() {
        assert_eq!(
            recovery_time(1_795_000_123_456).unwrap(),
            "2026-11-18 11:08:43.456+00"
        );
    }

    /// Writes a segment of the old archive, which held a dump, under the id `old`.
    async fn write_dump_segment(dir: &Path, db_id: ShortId) -> anyhow::Result<()> {
        tokio::fs::create_dir_all(dir).await?;
        let mut dump = segment("old", 1);
        dump.app = db_id;
        dump.filename = "000000010000000000000001".into();
        dump.native = false;
        write_atomic(&dir.join("old.wal.gz"), b"dump").await?;
        write_atomic(&dir.join("old.json"), &serde_json::to_vec(&dump)?).await
    }

    /// The old archive stored dumps as segments; they cannot be replayed as a log and are
    /// removed on the first pull, while a file that the server archived stays.
    #[tokio::test]
    async fn the_dump_segments_of_the_old_archive_are_removed() -> anyhow::Result<()> {
        let (command, _) = tokio::sync::mpsc::channel(1);
        let config = r3v3rs3_api::platform::PlatformConfig::default();
        let (platform, _runtime, _dir) =
            super::super::tests::platform_with(&config, command).await?;
        let db = platform
            .add_app(super::super::tests::request("shop-db"), 1)
            .await?;
        let dir = platform.wal_dir(db.id).await?;
        write_dump_segment(&dir, db.id).await?;
        let kept =
            store_native_segment(&dir, db.id, "000000010000000000000002".into(), b"wal", 2).await?;

        platform.drop_legacy_segments(db.id).await?;

        let left = platform.list_wal_segments(db.id).await?;
        assert_eq!(left, vec![kept]);
        assert!(!dir.join("old.wal.gz").exists());
        Ok(())
    }

    /// A file name from the container could name a path outside the archive.
    #[test]
    fn only_plain_wal_names_leave_the_archive() {
        assert!(is_archive_name("000000010000000000000004"));
        assert!(is_archive_name("00000002.history"));
        assert!(!is_archive_name("000000010000000000000004.tmp"));
        assert!(!is_archive_name("../../etc/passwd"));
        assert!(!is_archive_name("a b"));
        assert!(!is_archive_name(""));
    }

    /// A pull that stops the archive must reach the log: on production a binary log file over
    /// the download limit failed every round, and the loop wrote it at debug level only. A busy
    /// database and a database without the archive volume are expected states.
    #[test]
    fn only_a_busy_or_unarchived_database_is_an_expected_pull_failure() {
        let busy: anyhow::Error = Error::AppBusy {
            name: "shop-db".into(),
        }
        .into();
        let unarchived = anyhow::anyhow!("no such directory").context(NotArchiving);
        let too_large = anyhow::anyhow!("length limit exceeded");
        let wrapped_busy = anyhow::Error::from(Error::AppBusy {
            name: "shop-db".into(),
        })
        .context("pull");
        assert!(is_expected_pull_failure(&busy));
        assert!(is_expected_pull_failure(&unarchived));
        assert!(is_expected_pull_failure(&wrapped_busy));
        assert!(!is_expected_pull_failure(&too_large));
    }

    /// The archive volume belongs to the database and is added only where archiving runs.
    #[test]
    fn the_archive_volume_is_added_only_for_an_archiving_database() {
        let spec: AppSpec = serde_json::from_value(serde_json::json!({
            "source": {"type": "image", "image": "postgres:17-alpine"},
            "port": 5432,
            "database": "postgres",
            "volumes": [{"volume": "shop-db-data", "target": "/var/lib/postgresql/data"}],
        }))
        .unwrap();
        let app: ShortId = "abc-def".parse().unwrap();
        assert_eq!(container_volumes(app, &spec, false).unwrap(), spec.volumes);
        let volumes = container_volumes(app, &spec, true).unwrap();
        assert_eq!(volumes.len(), 2);
        assert_eq!(volumes[1].volume.as_str(), "r3v3rs3-wal-abc-def");
        assert_eq!(volumes[1].target.as_str(), WAL_ARCHIVE_PATH);
        // MySQL keeps its binary log in the data directory and gets no archive volume.
        let mut mysql = spec.clone();
        mysql.database = Some("mysql".into());
        assert_eq!(container_volumes(app, &mysql, true).unwrap(), mysql.volumes);
    }
}
