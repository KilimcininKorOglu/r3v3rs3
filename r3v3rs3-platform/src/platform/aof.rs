//! Continuous archiving of the Redis append-only file and point-in-time recovery from it.
//!
//! Redis 7 and later keep the AOF as a base file and an incremental (incr) file named by the
//! manifest in `appendonlydir`. With `aof-timestamp-enabled`, the server writes a `#TS:<unix
//! seconds>` line before the commands of a new second. The platform copies the new bytes of
//! the incr file into the backup directory once a minute. A base backup rewrites the AOF and
//! keeps the new base file with the manifest; the incr file of a recovery is rebuilt from the
//! archived bytes and cut at the target with `redis-check-aof --truncate-to-timestamp` (Redis
//! documentation, "Redis persistence").

use super::Platform;
use super::backup::redis_auth;
use super::history::{Archive, timeline_for};
use super::transfer::{Scratch, fetch_to_file, gunzip_file, push_file};
use super::wal::{
    BaseBackup, BaseStart, PREVIOUS_DIR, data_mount, pick_base, rollback_script,
    store_marker_segment, store_native_segment,
};
use crate::runtime::ContainerRuntime;
use anyhow::{Context as _, bail, ensure};
use r3v3rs3_api::container::ContainerName;
use r3v3rs3_api::error::Error;
use r3v3rs3_api::id::ShortId;
use r3v3rs3_api::platform::{
    AppEntry, PitrRange, PitrRestoreRequest, PitrRestoreResponse, WalSegment,
};
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

/// The start command of a Redis that archives its AOF. The ACL file part is the one of
/// `REDIS_START`. The server loads its AOF only when the manifest exists: with `appendonly yes`
/// and no AOF, Redis starts empty and ignores `dump.rdb` (measured on redis:8), and a server
/// killed while it switched to the AOF leaves the directory without a manifest. Every other
/// start runs without the AOF, and the deployment switches it on while the server runs, which
/// writes the AOF from the data in memory. Automatic rewrites stay off: a rewrite replaces the
/// incr file, and only a base backup of the platform may do that.
pub(super) const AOF_START: &str = "f=/data/users.acl; touch \"$f\" && \
if ! grep -q '^user default ' \"$f\"; then \
if [ -n \"$REDIS_PASSWORD\" ]; then \
echo \"user default on #$(printf %s \"$REDIS_PASSWORD\" | sha256sum | cut -d' ' -f1) ~* &* +@all\"; \
else echo 'user default on nopass ~* &* +@all'; fi >> \"$f\"; fi && \
chown redis:redis \"$f\" && a=no; \
if [ -s /data/appendonlydir/appendonly.aof.manifest ]; then a=yes; fi; \
exec docker-entrypoint.sh redis-server --aclfile \"$f\" --appendonly $a \
--aof-timestamp-enabled yes --auto-aof-rewrite-percentage 0";

/// The directory of the AOF inside the data volume.
const AOF_DIR: &str = "/data/appendonlydir";

const MANIFEST: &str = "/data/appendonlydir/appendonly.aof.manifest";

/// The hard link that keeps the incr file of a rewrite until its last bytes are archived.
const KEPT_INCR: &str = "/data/r3v3rs3-incr.kept";

/// The prefix of an archived AOF piece. The suffix packs the sequence of the incr file into the
/// upper bits and the end offset of the piece into the lower 40 bits, so the pieces sort in the
/// order of the AOF.
const AOF_PREFIX: &str = "aof.";

const OFFSET_BITS: u32 = 40;

/// The most bytes that one pull copies, so a long pause cannot load a huge piece into memory.
const PIECE_LIMIT: u64 = 64 * 1024 * 1024;

/// The incr file size above which the pull takes a new base backup, which starts a new and
/// empty incr file. With automatic rewrites off, nothing else bounds the file.
const INCR_LIMIT: u64 = 256 * 1024 * 1024;

/// How long a switch to the AOF or a fresh server may take.
const READY_TIMEOUT: Duration = Duration::from_secs(120);

/// The directory in the data volume that a recovery writes its files into while the server
/// still runs. The helper moves the whole volume aside first, so it reads them from there.
const RESTORE_NAME: &str = ".r3v3rs3-restore";

/// The file in the container that a base backup is packed into.
const BASE_REMOTE: &str = "/tmp/r3v3rs3-aof-base.tar";

fn pack(seq: u64, offset: u64) -> u64 {
    (seq << OFFSET_BITS) | offset
}

fn unpack(packed: u64) -> (u64, u64) {
    (packed >> OFFSET_BITS, packed & ((1 << OFFSET_BITS) - 1))
}

fn piece_name(seq: u64, offset: u64) -> String {
    format!("{AOF_PREFIX}{}", pack(seq, offset))
}

fn packed_of(name: &str) -> Option<u64> {
    name.strip_prefix(AOF_PREFIX)?.parse().ok()
}

/// The name and the sequence of the incr file of a manifest. A manifest with any other number
/// of incr files does not describe an AOF that the archive follows.
fn incr_of(manifest: &str) -> anyhow::Result<(String, u64)> {
    let incrs: Vec<(String, u64)> = manifest
        .lines()
        .filter_map(|line| {
            let words: Vec<&str> = line.split_whitespace().collect();
            match words.as_slice() {
                ["file", name, "seq", seq, "type", "i", ..] => {
                    Some(((*name).to_string(), seq.parse().ok()?))
                }
                _ => None,
            }
        })
        .collect();
    match incrs.as_slice() {
        [(name, seq)] if is_aof_name(name) => Ok((name.clone(), *seq)),
        _ => bail!("the AOF manifest names no single incr file: {manifest}"),
    }
}

/// A file name of the AOF directory, and nothing that could leave it.
fn is_aof_name(name: &str) -> bool {
    !name.is_empty()
        && !name.starts_with('.')
        && name
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '.' || c == '_' || c == '-')
}

/// The last second that a recovery to `target_ms` keeps: the AOF stamps whole seconds, and the
/// commands of the second of the target stay out, as with the other engines.
fn keep_until(target_ms: u64) -> u64 {
    (target_ms / 1000).saturating_sub(1)
}

/// The helper script of a recovery: it moves the data aside, puts the base file and the
/// manifest back, rebuilds the incr file from the archive and cuts it after second `until`. An
/// incr file whose first stamp lies after the target holds nothing to keep. With timestamps on,
/// Redis writes a stamp before the first command of an incr file, so a file that starts
/// otherwise is damaged and fails the recovery.
fn rebuild_script(mount: &str, until: u64) -> String {
    format!(
        "set -e; M='{mount}'; P=\"$M/{PREVIOUS_DIR}\"; rm -rf \"$P\"; mkdir \"$P\"; \
find \"$M\" -mindepth 1 -maxdepth 1 ! -name {PREVIOUS_DIR} -exec mv {{}} \"$P\"/ \\;; \
U=\"$P/{RESTORE_NAME}\"; D=\"$M/appendonlydir\"; mkdir \"$D\"; tar -xf \"$U/base.tar\" -C \"$D\"; \
i=$(awk '$6 == \"i\" {{print $2}}' \"$D/appendonly.aof.manifest\"); \
cp \"$U/incr.aof\" \"$D/$i\"; \
if [ -s \"$D/$i\" ] && ! head -c 4 \"$D/$i\" | grep -q '^#TS:'; then \
echo 'the archived AOF does not start with a timestamp' >&2; exit 1; fi; \
first=$(grep -a '^#TS:' \"$D/$i\" | head -1 | cut -d: -f2 | tr -d '\\r'); \
if [ -z \"$first\" ] || [ \"$first\" -gt {until} ]; then : > \"$D/$i\"; \
else (cd \"$D\" && redis-check-aof --truncate-to-timestamp {until} appendonly.aof.manifest); fi; \
chown -R redis:redis \"$M\""
    )
}

/// The running container of a Redis database with the sign-in of its default user.
struct RedisSession {
    container: ContainerName,
    runtime: Arc<dyn ContainerRuntime>,
    auth: Vec<String>,
}

impl RedisSession {
    async fn shell(&self, script: &str) -> anyhow::Result<Vec<u8>> {
        let command = vec!["sh".to_string(), "-c".to_string(), script.to_string()];
        self.runtime
            .exec_capture(&self.container, command, self.auth.clone())
            .await
    }

    /// Packs the AOF base file `base_file` with the manifest into the local tar file `dest`.
    /// The tar goes into a file in the container first and leaves it in bounded pieces, so a
    /// base of any size passes.
    async fn pack_base(&self, base_file: &str, dest: &std::path::Path) -> anyhow::Result<()> {
        let script =
            format!("tar -C {AOF_DIR} -cf {BASE_REMOTE} '{base_file}' appendonly.aof.manifest");
        let fetched = match self.shell(&script).await {
            Ok(_) => fetch_to_file(&self.runtime, &self.container, (BASE_REMOTE, 0), dest).await,
            Err(err) => Err(err),
        };
        self.runtime
            .remove_path(&self.container, BASE_REMOTE)
            .await?;
        fetched.map(|_| ())
    }

    async fn text(&self, script: &str) -> anyhow::Result<String> {
        Ok(String::from_utf8_lossy(&self.shell(script).await?)
            .trim()
            .to_string())
    }

    /// One command of `redis-cli`; an error reply fails.
    async fn cli(&self, args: &str) -> anyhow::Result<String> {
        let out = self.text(&format!("redis-cli {args}")).await?;
        ensure!(
            !out.starts_with("ERR") && !out.starts_with("NOAUTH") && !out.starts_with("WRONGPASS"),
            "redis-cli {args}: {out}"
        );
        Ok(out)
    }

    /// The value of one field of `INFO persistence`.
    async fn persistence(&self, field: &str) -> anyhow::Result<String> {
        let info = self.cli("INFO persistence").await?;
        info.lines()
            .find_map(|line| line.strip_prefix(&format!("{field}:")))
            .map(|value| value.trim().to_string())
            .with_context(|| format!("INFO persistence has no {field}"))
    }

    /// Waits until no rewrite runs or waits, and fails on a failed rewrite.
    async fn wait_rewrite(&self) -> anyhow::Result<()> {
        let deadline = tokio::time::Instant::now() + READY_TIMEOUT;
        loop {
            let running = self.persistence("aof_rewrite_in_progress").await? != "0"
                || self.persistence("aof_rewrite_scheduled").await? != "0";
            if !running {
                let status = self.persistence("aof_last_bgrewrite_status").await?;
                ensure!(status == "ok", "the AOF rewrite failed: {status}");
                return Ok(());
            }
            ensure!(
                tokio::time::Instant::now() < deadline,
                "the AOF rewrite did not end in {} seconds",
                READY_TIMEOUT.as_secs()
            );
            tokio::time::sleep(Duration::from_millis(500)).await;
        }
    }

    async fn manifest(&self) -> anyhow::Result<String> {
        self.text(&format!("cat {MANIFEST}")).await
    }

    async fn size(&self, path: &str) -> anyhow::Result<u64> {
        let size = self.text(&format!("wc -c < '{path}'")).await?;
        size.parse()
            .with_context(|| format!("unexpected size of {path}: {size}"))
    }

    /// The bytes of `path` from `offset` on, at most `PIECE_LIMIT` of them.
    async fn tail(&self, path: &str, offset: u64, size: u64) -> anyhow::Result<Vec<u8>> {
        let count = (size - offset).min(PIECE_LIMIT);
        self.shell(&format!(
            "tail -c +{} '{path}' | head -c {count}",
            offset + 1
        ))
        .await
    }
}

impl Platform {
    async fn redis_session(&self, db_id: ShortId) -> anyhow::Result<RedisSession> {
        let (_, container, runtime) = self.database_container(db_id).await?;
        self.redis_session_on(db_id, container, runtime).await
    }

    async fn redis_session_on(
        &self,
        db_id: ShortId,
        container: ContainerName,
        runtime: Arc<dyn ContainerRuntime>,
    ) -> anyhow::Result<RedisSession> {
        let app = self.app(db_id).await?;
        let info = r3v3rs3_api::platform::app_engine_info(&app.spec)
            .context("the engine is not in the database catalog")?;
        let password = self.database_password(db_id, &info).await?;
        Ok(RedisSession {
            container,
            runtime,
            auth: redis_auth(&password),
        })
    }

    /// Switches a running Redis to the AOF when it started without one, and waits until the
    /// rewrite that writes the AOF from memory ended and the manifest exists. The deployment
    /// calls it on its new container before that container serves.
    pub(super) async fn ensure_aof(
        &self,
        db_id: ShortId,
        runtime: &Arc<dyn ContainerRuntime>,
        container: &ContainerName,
    ) -> anyhow::Result<()> {
        let session = self
            .redis_session_on(db_id, container.clone(), runtime.clone())
            .await?;
        if session.cli("CONFIG GET appendonly").await?.ends_with("yes") {
            return Ok(());
        }
        session.cli("CONFIG SET appendonly yes").await?;
        session.wait_rewrite().await?;
        let manifest = session.manifest().await?;
        incr_of(&manifest)?;
        Ok(())
    }

    /// Copies the new bytes of the incr file into the backup directory. A database without a
    /// base backup gets one first. When the incr file is not the one the archive follows, a
    /// rewrite that the platform did not make replaced it: the archive has a gap, and a new
    /// timeline starts.
    pub(super) async fn pull_aof_locked(&self, db_id: ShortId) -> anyhow::Result<Vec<WalSegment>> {
        let (session, (seq, offset)) = self.aof_position(db_id).await?;
        let (incr, current) = incr_of(&session.manifest().await?)?;
        if current != seq {
            self.restart_after_rewrite(db_id).await?;
            return Ok(Vec::new());
        }
        let path = format!("{AOF_DIR}/{incr}");
        let size = session.size(&path).await?;
        let pulled = self
            .store_tail(&session, db_id, &path, (seq, offset), size)
            .await?;
        if size > INCR_LIMIT {
            self.take_aof_base_locked(db_id).await?;
        }
        Ok(pulled)
    }

    /// The session of the database and the incr file sequence and offset that the archive holds
    /// up to, after an interrupted history move and the old dump segments are dealt with.
    async fn aof_position(&self, db_id: ShortId) -> anyhow::Result<(RedisSession, (u64, u64))> {
        let session = self.redis_session(db_id).await?;
        self.finish_history_moves(db_id).await?;
        self.drop_legacy_segments(db_id).await?;
        let position = unpack(self.aof_archived_until(db_id).await?);
        Ok((session, position))
    }

    /// Starts a new timeline after a rewrite that the platform did not make: the old archive
    /// moves into a history that ends now, so a target inside the gap is refused.
    async fn restart_after_rewrite(&self, db_id: ShortId) -> anyhow::Result<()> {
        tracing::error!(database = %db_id, "the AOF was rewritten outside the archive; a new timeline starts");
        self.start_history(db_id).await?;
        self.take_aof_base_locked(db_id).await?;
        Ok(())
    }

    /// Stores the bytes of `path` after `offset` as one piece of incr file `seq`.
    async fn store_tail(
        &self,
        session: &RedisSession,
        db_id: ShortId,
        path: &str,
        (seq, offset): (u64, u64),
        size: u64,
    ) -> anyhow::Result<Vec<WalSegment>> {
        if size <= offset {
            return Ok(Vec::new());
        }
        let bytes = session.tail(path, offset, size).await?;
        let end = offset + bytes.len() as u64;
        let dir = self.wal_dir(db_id).await?;
        tokio::fs::create_dir_all(&dir).await?;
        let start = self.archive_end(db_id).await?;
        let piece = store_native_segment(&dir, db_id, piece_name(seq, end), &bytes, start).await?;
        Ok(vec![piece])
    }

    /// Where the archive holds the AOF up to: the newest piece or the start of the newest base
    /// backup, whichever is later. A database without a base backup takes one now.
    async fn aof_archived_until(&self, db_id: ShortId) -> anyhow::Result<u64> {
        let base = match self.list_base_backups(db_id).await?.pop() {
            Some(base) => base,
            None => self.take_aof_base_locked(db_id).await?,
        };
        let start = packed_of(&base.start_wal).context("the base backup names no AOF offset")?;
        let stored = self.native_segments(db_id).await?;
        let newest = stored.iter().filter_map(|s| packed_of(&s.filename)).max();
        Ok(newest.map_or(start, |newest| newest.max(start)))
    }

    /// Rewrites the AOF and stores the new base file with the manifest. The old incr file stays
    /// linked through the rewrite, so its last bytes reach the archive before it goes.
    pub(super) async fn take_aof_base_locked(&self, db_id: ShortId) -> anyhow::Result<BaseBackup> {
        let session = self.redis_session(db_id).await?;
        let (old_incr, old_seq) = incr_of(&session.manifest().await?)?;
        session
            .text(&format!(
                "rm -f {KEPT_INCR}; ln '{AOF_DIR}/{old_incr}' {KEPT_INCR}"
            ))
            .await?;
        session.wait_rewrite().await?;
        session.cli("BGREWRITEAOF").await?;
        session.wait_rewrite().await?;
        self.archive_kept_incr(&session, db_id, old_seq).await?;
        self.store_aof_base(&session, db_id).await
    }

    /// Stores the base file and the manifest of the current AOF as a base backup.
    async fn store_aof_base(
        &self,
        session: &RedisSession,
        db_id: ShortId,
    ) -> anyhow::Result<BaseBackup> {
        let (_, seq) = incr_of(&session.manifest().await?)?;
        let base_file = session
            .text(&format!("awk '$6 == \"b\" {{print $2}}' {MANIFEST}"))
            .await?;
        ensure!(
            is_aof_name(&base_file),
            "unexpected AOF base file: {base_file}"
        );
        let scratch = Scratch::new(&self.base_dir(db_id).await?).await?;
        let archive = scratch.path("base.tar");
        session.pack_base(&base_file, &archive).await?;
        let start = BaseStart {
            log: piece_name(seq, 0),
            position: None,
            data_directory: "/data".to_string(),
        };
        let base = self.store_base_file(db_id, start, &archive).await?;
        self.prune_base_backups(db_id).await?;
        Ok(base)
    }

    /// Archives the bytes of the old incr file after the newest piece of its sequence, then
    /// removes the link. A database without an archive of that sequence needs none of them.
    async fn archive_kept_incr(
        &self,
        session: &RedisSession,
        db_id: ShortId,
        seq: u64,
    ) -> anyhow::Result<()> {
        let bases = self.list_base_backups(db_id).await?;
        let follows = bases
            .iter()
            .filter_map(|base| packed_of(&base.start_wal))
            .any(|start| unpack(start).0 == seq);
        if follows {
            let pieces = self.native_segments(db_id).await?;
            let offset = pieces
                .iter()
                .filter_map(|s| packed_of(&s.filename))
                .filter(|packed| unpack(*packed).0 == seq)
                .map(|packed| unpack(packed).1)
                .max()
                .unwrap_or(0);
            let size = session.size(KEPT_INCR).await?;
            let mut done = offset;
            while done < size {
                let stored = self
                    .store_tail(session, db_id, KEPT_INCR, (seq, done), size)
                    .await?;
                done = stored
                    .first()
                    .and_then(|piece| packed_of(&piece.filename))
                    .map_or(size, |packed| unpack(packed).1);
            }
        }
        session.text(&format!("rm -f {KEPT_INCR}")).await?;
        Ok(())
    }

    /// Pulls the AOF so the archive reaches now, and returns the newest stored piece.
    pub(super) async fn archive_aof(&self, db_id: ShortId) -> anyhow::Result<WalSegment> {
        let app = self.app(db_id).await?;
        let _lock = self.lock_app(&app)?;
        self.pull_aof_locked(db_id).await?;
        if let Ok(piece) = self.archived_file(db_id, None).await {
            return Ok(piece);
        }
        // No write followed the base backup: an empty piece at the start of the incr file marks
        // that the archive reaches now, and it adds nothing to the replayed file.
        let (seq, offset) = unpack(self.aof_archived_until(db_id).await?);
        let dir = self.wal_dir(db_id).await?;
        tokio::fs::create_dir_all(&dir).await?;
        let start = self.archive_end(db_id).await?;
        store_marker_segment(&dir, db_id, piece_name(seq, offset), start).await
    }

    /// The recovery window over every timeline.
    pub(super) async fn aof_pitr_range(&self, db_id: ShortId) -> anyhow::Result<PitrRange> {
        let (mut bases, mut segments) = (Vec::new(), Vec::new());
        for archive in self.archives(db_id).await? {
            bases.extend(archive.bases().await?);
            segments.extend(archive.segments().await?);
        }
        let earliest = bases.iter().map(|base| base.created_at).min();
        let latest = segments.iter().map(|s| s.end_time).max();
        let archiving = match self.redis_session(db_id).await {
            Ok(session) => session
                .cli("CONFIG GET appendonly")
                .await
                .is_ok_and(|answer| answer.ends_with("yes")),
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

    /// Recovers a Redis database to `request.target_time`, in place or into a new branch, with
    /// second precision.
    pub(super) async fn restore_aof_pitr(
        self: &Arc<Self>,
        db_id: ShortId,
        app: &AppEntry,
        request: PitrRestoreRequest,
    ) -> anyhow::Result<PitrRestoreResponse> {
        let (base, files, upload) = self.aof_plan(db_id, request.target_time).await?;
        let (target, branched) = self.recovery_target(db_id, app, &request).await?;
        self.recover_aof_into(target.id, upload, keep_until(request.target_time))
            .await?;
        self.reapply_tenants(target.id).await?;
        self.archive_aof(target.id)
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

    async fn aof_plan(
        &self,
        db_id: ShortId,
        target: u64,
    ) -> anyhow::Result<(BaseBackup, Vec<WalSegment>, AofUpload)> {
        let archive = self.timeline_archive(db_id, target).await?;
        let bases = archive.bases().await?;
        let segments = archive.segments().await?;
        let base = pick_base(&bases, &segments, target)?.clone();
        let files = pieces_of(&segments, &base)?;
        let upload = aof_upload(&archive, &base, &files).await?;
        Ok((base, files, upload))
    }

    /// The archive of the timeline that holds `target`, after a running database archived its
    /// newest writes.
    async fn timeline_archive(&self, db_id: ShortId, target: u64) -> anyhow::Result<Archive> {
        self.finish_history_moves(db_id).await?;
        if self.store.running_deployment(db_id).await?.is_some() {
            self.archive_aof(db_id).await?;
        }
        let mut archives = self.archives(db_id).await?;
        let ends: Vec<u64> = archives.iter().filter_map(|a| a.end).collect();
        Ok(archives.swap_remove(timeline_for(&ends, target)))
    }

    /// Replaces the data of the database with the base file and the incr file cut at second
    /// `until`. A failed recovery puts the old data back.
    async fn recover_aof_into(
        &self,
        db_id: ShortId,
        upload: AofUpload,
        until: u64,
    ) -> anyhow::Result<()> {
        let app = self.app(db_id).await?;
        let _lock = self.lock_app(&app)?;
        let (app, container, runtime) = self.database_container(db_id).await?;
        let mount = mount_of(&app)?;
        let restore = format!("{mount}/{RESTORE_NAME}");
        push_upload(&runtime, &container, &upload, &restore).await?;
        runtime
            .stop_container(&container, Duration::from_secs(60))
            .await?;
        let rebuilt = self
            .run_db_helper(&runtime, &app, rebuild_script(&mount, until), None)
            .await;
        let started = match rebuilt {
            Ok(()) => self.ready_redis(&app).await,
            Err(err) => Err(err),
        };
        match started {
            Ok(()) => self.finish_aof_recovery(db_id, &mount).await,
            Err(err) => {
                self.roll_back_aof(&runtime, &app, &container, &mount)
                    .await?;
                Err(Error::RestoreFailed {
                    reason: format!("{err:#}"),
                }
                .into())
            }
        }
    }

    async fn finish_aof_recovery(&self, db_id: ShortId, mount: &str) -> anyhow::Result<()> {
        self.close_archive(db_id).await?;
        self.start_history(db_id).await?;
        self.redis_session(db_id)
            .await?
            .text(&format!("rm -rf '{mount}/{PREVIOUS_DIR}'"))
            .await?;
        Ok(())
    }

    /// Stores an empty piece after the newest one, so the old history reaches the stop of the
    /// database and not only its last pull: a target in the last second before the recovery
    /// is then in range. The piece adds nothing to a replayed file.
    async fn close_archive(&self, db_id: ShortId) -> anyhow::Result<()> {
        let newest = self
            .native_segments(db_id)
            .await?
            .into_iter()
            .max_by_key(|s| s.end_time);
        let Some(newest) = newest else {
            return Ok(());
        };
        let dir = self.wal_dir(db_id).await?;
        store_marker_segment(&dir, db_id, newest.filename, newest.end_time).await?;
        Ok(())
    }

    /// Starts the database container and waits until it loaded its data and answers.
    async fn ready_redis(&self, app: &AppEntry) -> anyhow::Result<()> {
        let (_, container, runtime) = self.database_container(app.id).await?;
        runtime.start_container(&container).await?;
        let deadline = tokio::time::Instant::now() + READY_TIMEOUT;
        loop {
            if let Ok(session) = self.redis_session(app.id).await
                && session
                    .persistence("loading")
                    .await
                    .is_ok_and(|loading| loading == "0")
            {
                return Ok(());
            }
            ensure!(
                tokio::time::Instant::now() < deadline,
                "the database did not answer in {} seconds",
                READY_TIMEOUT.as_secs()
            );
            tokio::time::sleep(Duration::from_secs(1)).await;
        }
    }

    async fn roll_back_aof(
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
        self.ready_redis(app)
            .await
            .context("the database did not answer on its old data")?;
        // The upload of the recovery came back with the old data.
        runtime
            .remove_path(container, &format!("{mount}/{RESTORE_NAME}"))
            .await
    }
}

/// The pieces of the incr file that a base backup started, in the order of the file. A base
/// backup starts a new incr file, so the pieces of other sequences belong to other bases.
fn pieces_of(segments: &[WalSegment], base: &BaseBackup) -> anyhow::Result<Vec<WalSegment>> {
    let start = packed_of(&base.start_wal).context("the base backup names no AOF offset")?;
    let (seq, _) = unpack(start);
    let mut pieces: Vec<(u64, WalSegment)> = segments
        .iter()
        .filter_map(|s| Some((packed_of(&s.filename)?, s.clone())))
        .filter(|(packed, _)| unpack(*packed).0 == seq)
        .collect();
    pieces.sort_by_key(|(packed, _)| *packed);
    Ok(pieces.into_iter().map(|(_, s)| s).collect())
}

fn mount_of(app: &AppEntry) -> anyhow::Result<String> {
    let info = r3v3rs3_api::platform::app_engine_info(&app.spec)
        .context("the engine is not in the database catalog")?;
    Ok(data_mount(&app.spec, &info.data_path)?
        .target
        .as_str()
        .to_string())
}

/// Writes the files of a recovery into `dir` of the running database container, one bounded
/// piece at a time. A failed write leaves no part of the upload behind.
async fn push_upload(
    runtime: &Arc<dyn ContainerRuntime>,
    container: &ContainerName,
    upload: &AofUpload,
    dir: &str,
) -> anyhow::Result<()> {
    let pushed = push_files(runtime, container, upload, dir).await;
    if let Err(err) = pushed {
        if let Err(cleanup) = runtime.remove_path(container, dir).await {
            return Err(err.context(format!("{dir} stays: {cleanup:#}")));
        }
        return Err(err);
    }
    Ok(())
}

async fn push_files(
    runtime: &Arc<dyn ContainerRuntime>,
    container: &ContainerName,
    upload: &AofUpload,
    dir: &str,
) -> anyhow::Result<()> {
    runtime.remove_path(container, dir).await?;
    let mkdir = vec!["mkdir".into(), "-p".into(), dir.to_string()];
    runtime.exec_capture(container, mkdir, Vec::new()).await?;
    for (name, path) in &upload.files {
        push_file(runtime, container, path, &format!("{dir}/{name}")).await?;
    }
    Ok(())
}

/// The base archive and the incr file of a recovery, decompressed on disk.
pub(super) struct AofUpload {
    _scratch: Scratch,
    files: [(&'static str, PathBuf); 2],
}

/// The base archive and the incr file of a recovery, decompressed on disk before anything
/// stops, so a missing piece fails the recovery while the database still runs on its data.
async fn aof_upload(
    archive: &Archive,
    base: &BaseBackup,
    pieces: &[WalSegment],
) -> anyhow::Result<AofUpload> {
    let scratch = Scratch::new(&archive.base).await?;
    let tar = scratch.path("base.tar");
    gunzip_file(&archive.base.join(format!("{}.tar.gz", base.id)), &tar).await?;
    let incr = scratch.path("incr.aof");
    let mut out = tokio::fs::File::create(&incr).await?;
    let part = scratch.path("part.aof");
    for piece in pieces {
        gunzip_file(&archive.wal.join(format!("{}.wal.gz", piece.id)), &part)
            .await
            .with_context(|| format!("the AOF piece {} is missing", piece.filename))?;
        let mut input = tokio::fs::File::open(&part).await?;
        tokio::io::copy(&mut input, &mut out).await?;
    }
    out.sync_all().await?;
    Ok(AofUpload {
        _scratch: scratch,
        files: [("base.tar", tar), ("incr.aof", incr)],
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The manifest that redis:8 writes after a rewrite (measured).
    const MANIFEST_AFTER_REWRITE: &str = "file appendonly.aof.2.base.rdb seq 2 type b\nfile appendonly.aof.2.incr.aof seq 2 type i startoffset 1\n";

    #[test]
    fn the_incr_file_of_a_manifest_is_read() {
        assert_eq!(
            incr_of(MANIFEST_AFTER_REWRITE).unwrap(),
            ("appendonly.aof.2.incr.aof".to_string(), 2)
        );
    }

    /// A manifest with history files or two incr files is not one the archive follows, and a
    /// file name that leaves the directory is refused.
    #[test]
    fn a_manifest_without_one_safe_incr_file_is_refused() {
        let two = format!("{MANIFEST_AFTER_REWRITE}file appendonly.aof.3.incr.aof seq 3 type i\n");
        assert!(incr_of(&two).is_err());
        assert!(incr_of("file appendonly.aof.2.base.rdb seq 2 type b\n").is_err());
        assert!(incr_of("file ../../etc/passwd seq 2 type i\n").is_err());
    }

    /// The pieces replay in the order of the incr file, and only those of the incr file that
    /// the base backup started.
    #[test]
    fn a_base_replays_only_the_pieces_of_its_incr_file_in_order() {
        let piece = |seq: u64, end: u64| WalSegment {
            id: format!("{seq}-{end}"),
            app: "abc-def".parse().unwrap(),
            filename: piece_name(seq, end),
            size: 1,
            sha256: String::new(),
            start_time: 0,
            end_time: 1,
            created_at: 1,
            native: true,
        };
        let base = BaseBackup {
            id: "b".into(),
            created_at: 1,
            start_wal: piece_name(3, 0),
            start_position: None,
            data_directory: "/data".into(),
            size: 1,
            sha256: String::new(),
        };
        let segments = [piece(3, 900), piece(2, 50), piece(3, 120), piece(4, 10)];
        let ids: Vec<String> = pieces_of(&segments, &base)
            .unwrap()
            .into_iter()
            .map(|s| s.id)
            .collect();
        assert_eq!(ids, ["3-120", "3-900"]);
    }

    /// The commands of the second of the target stay out: the cut keeps the seconds before it.
    #[test]
    fn the_cut_keeps_the_seconds_before_the_target() {
        assert_eq!(keep_until(1_795_063_254_999), 1_795_063_253);
        assert_eq!(keep_until(1_795_063_255_000), 1_795_063_254);
    }
}
