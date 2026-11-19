//! Continuous archiving of the MongoDB oplog and point-in-time recovery from it.
//!
//! A local MongoDB runs as a single-member replica set (`mongo.rs`), which writes every change
//! into `local.oplog.rs`. The platform copies the new entries into the backup directory once a
//! minute. A base backup is a `mongodump --oplog` archive with the newest oplog timestamp
//! before it started. A recovery restores the archive into a fresh server and replays the
//! oplog up to the target second with `mongorestore --oplogReplay --oplogLimit` (MongoDB
//! manual, "mongorestore").

use super::Platform;
use super::backup::{mongodb_uri, ungzip};
use super::binlog::{WORK_DIR, aside_script, tar_files};
use super::history::{Archive, timeline_for};
use super::wal::{
    BaseBackup, BaseStart, PREVIOUS_DIR, data_mount, pick_base, replay_files, rollback_script,
    store_native_segment,
};
use crate::runtime::ContainerRuntime;
use anyhow::{Context as _, ensure};
use r3v3rs3_api::container::ContainerName;
use r3v3rs3_api::error::Error;
use r3v3rs3_api::id::ShortId;
use r3v3rs3_api::platform::{
    AppEntry, PitrRange, PitrRestoreRequest, PitrRestoreResponse, WalSegment,
};
use std::sync::Arc;
use std::time::Duration;

/// The prefix of an archived oplog file. The suffix is the packed timestamp of its newest
/// entry, so the files sort in the order of the oplog.
const OPLOG_PREFIX: &str = "oplog.";

/// The namespaces whose entries stay out of the archive: the accounts belong to the platform,
/// which sets them after a recovery, so a replay must not bring back an old password.
const ACCOUNT_NAMESPACES: &str = "['admin.system.users', 'admin.system.roles']";

/// A mongosh function that packs an oplog timestamp into one number: the seconds in the upper
/// 32 bits and the ordinal in the lower ones, so the order of the numbers is the order of the
/// oplog.
const PACK: &str =
    "function pack(ts) { return ((BigInt(ts.t) << 32n) | BigInt(ts.i)).toString(); }";

/// The seconds and the ordinal of a packed oplog timestamp.
fn unpack(packed: u64) -> (u64, u64) {
    (packed >> 32, packed & 0xffff_ffff)
}

/// The packed timestamp in the name of an archived oplog file.
fn packed_of(name: &str) -> Option<u64> {
    name.strip_prefix(OPLOG_PREFIX)?.parse().ok()
}

fn oplog_name(packed: u64) -> String {
    format!("{OPLOG_PREFIX}{packed}")
}

/// The mongosh expression of a packed timestamp.
fn timestamp(packed: u64) -> String {
    let (t, i) = unpack(packed);
    format!("Timestamp({{t: {t}, i: {i}}})")
}

/// The `mongodump --query` of the entries after `since` up to `until`, without the account
/// entries.
fn dump_query(since: u64, until: u64) -> String {
    let (st, si) = unpack(since);
    let (ut, ui) = unpack(until);
    format!(
        "{{\"ts\":{{\"$gt\":{{\"$timestamp\":{{\"t\":{st},\"i\":{si}}}}},\"$lte\":{{\"$timestamp\":{{\"t\":{ut},\"i\":{ui}}}}}}},\"ns\":{{\"$nin\":[\"admin.system.users\",\"admin.system.roles\"]}}}}"
    )
}

/// The running container of a MongoDB database with its root connection string.
struct MongoSession {
    container: ContainerName,
    runtime: Arc<dyn ContainerRuntime>,
    uri: String,
}

impl MongoSession {
    /// Runs a shell command with `MONGO_URI` and `$1`… set, and returns its standard output.
    async fn shell(&self, script: &str, vars: &[(&str, String)]) -> anyhow::Result<Vec<u8>> {
        let command = vec!["sh".to_string(), "-c".to_string(), script.to_string()];
        let mut env = vec![format!("MONGO_URI={}", self.uri)];
        env.extend(vars.iter().map(|(key, value)| format!("{key}={value}")));
        self.runtime
            .exec_capture(&self.container, command, env)
            .await
    }

    /// The printed answer of a mongosh script.
    async fn eval(&self, script: &str) -> anyhow::Result<String> {
        let out = self
            .shell(
                "mongosh \"$MONGO_URI\" --quiet --eval \"$MONGO_SCRIPT\"",
                &[("MONGO_SCRIPT", script.to_string())],
            )
            .await?;
        Ok(String::from_utf8_lossy(&out).trim().to_string())
    }

    async fn packed(&self, script: &str) -> anyhow::Result<Option<u64>> {
        let answer = self.eval(&format!("{PACK}; {script}")).await?;
        if answer == "none" {
            return Ok(None);
        }
        Ok(Some(answer.parse().with_context(|| {
            format!("unexpected oplog timestamp: {answer}")
        })?))
    }

    /// The newest entry of the oplog.
    async fn newest(&self) -> anyhow::Result<u64> {
        self.packed("const e = db.getSiblingDB('local').oplog.rs.find().sort({$natural: -1}).limit(1).toArray(); print(e.length ? pack(e[0].ts) : 'none');")
            .await?
            .context("the server writes no oplog")
    }

    /// The oldest entry that the capped oplog still holds.
    async fn oldest(&self) -> anyhow::Result<u64> {
        self.packed("const e = db.getSiblingDB('local').oplog.rs.find().sort({$natural: 1}).limit(1).toArray(); print(e.length ? pack(e[0].ts) : 'none');")
            .await?
            .context("the server writes no oplog")
    }

    /// The newest archived entry after `since`, or none.
    async fn newest_after(&self, since: u64) -> anyhow::Result<Option<u64>> {
        self.packed(&format!(
            "const e = db.getSiblingDB('local').oplog.rs.find({{ts: {{$gt: {}}}, ns: {{$nin: {ACCOUNT_NAMESPACES}}}}}).sort({{$natural: -1}}).limit(1).toArray(); print(e.length ? pack(e[0].ts) : 'none');",
            timestamp(since)
        ))
        .await
    }

    /// The oplog entries after `since` up to `until`, as BSON.
    async fn dump(&self, since: u64, until: u64) -> anyhow::Result<Vec<u8>> {
        self.shell(
            "mongodump --uri=\"$MONGO_URI\" -d local -c oplog.rs --query=\"$Q\" --out=- 2>/dev/null",
            &[("Q", dump_query(since, until))],
        )
        .await
    }
}

impl Platform {
    async fn mongo_session(&self, db_id: ShortId) -> anyhow::Result<MongoSession> {
        let (app, container, runtime) = self.database_container(db_id).await?;
        let info = r3v3rs3_api::platform::app_engine_info(&app.spec)
            .context("the engine is not in the database catalog")?;
        let password = self.database_password(db_id, &info).await?;
        Ok(MongoSession {
            container,
            runtime,
            uri: mongodb_uri(&info.user, &password)?,
        })
    }

    /// Copies the oplog entries after the newest archived one into the backup directory. A
    /// database without a base backup gets one first. When the capped oplog no longer holds
    /// the entry after the archive, the archive has a gap: a new base backup starts again.
    pub(super) async fn pull_oplog_locked(
        &self,
        db_id: ShortId,
    ) -> anyhow::Result<Vec<WalSegment>> {
        let session = self.mongo_session(db_id).await?;
        self.finish_history_moves(db_id).await?;
        self.drop_legacy_segments(db_id).await?;
        let since = self.archived_until(db_id).await?;
        if self.restarted_after_gap(&session, db_id, since).await? {
            return Ok(Vec::new());
        }
        let Some(until) = session.newest_after(since).await? else {
            return Ok(Vec::new());
        };
        let bson = session.dump(since, until).await?;
        Ok(vec![self.store_oplog(db_id, until, &bson).await?])
    }

    /// Takes a new base backup when the capped oplog no longer holds the entry after `since`:
    /// the archive has a gap, and a replay across it would miss changes.
    async fn restarted_after_gap(
        &self,
        session: &MongoSession,
        db_id: ShortId,
        since: u64,
    ) -> anyhow::Result<bool> {
        if session.oldest().await? <= since {
            return Ok(false);
        }
        tracing::error!(database = %db_id, "the oplog no longer holds the entries after the archive; a new base backup starts again");
        self.take_oplog_base_locked(db_id).await?;
        Ok(true)
    }

    async fn store_oplog(
        &self,
        db_id: ShortId,
        until: u64,
        bson: &[u8],
    ) -> anyhow::Result<WalSegment> {
        let dir = self.wal_dir(db_id).await?;
        tokio::fs::create_dir_all(&dir).await?;
        let start = self.archive_end(db_id).await?;
        store_native_segment(&dir, db_id, oplog_name(until), bson, start).await
    }

    /// The packed timestamp up to which the archive holds the oplog: the newest archived file
    /// or the start of the newest base backup, whichever is later. A database without a base
    /// backup takes one now.
    async fn archived_until(&self, db_id: ShortId) -> anyhow::Result<u64> {
        let base = match self.list_base_backups(db_id).await?.pop() {
            Some(base) => base,
            None => self.take_oplog_base_locked(db_id).await?,
        };
        let start =
            packed_of(&base.start_wal).context("the base backup names no oplog timestamp")?;
        let stored = self.native_segments(db_id).await?;
        let newest = stored.iter().filter_map(|s| packed_of(&s.filename)).max();
        Ok(newest.map_or(start, |newest| newest.max(start)))
    }

    /// Takes a `mongodump --oplog` archive with the newest oplog timestamp before it.
    pub(super) async fn take_oplog_base_locked(
        &self,
        db_id: ShortId,
    ) -> anyhow::Result<BaseBackup> {
        let session = self.mongo_session(db_id).await?;
        let start = session.newest().await?;
        let archive = session
            .shell(
                "mongodump --uri=\"$MONGO_URI\" --archive --oplog 2>/dev/null",
                &[],
            )
            .await?;
        let start = BaseStart {
            log: oplog_name(start),
            position: None,
            data_directory: "/data/db".to_string(),
        };
        let base = self.store_base(db_id, start, &archive).await?;
        self.prune_base_backups(db_id).await?;
        Ok(base)
    }

    /// Writes a note into the oplog and pulls the archive, so it reaches now, and returns the
    /// newest stored file.
    pub(super) async fn archive_oplog(&self, db_id: ShortId) -> anyhow::Result<WalSegment> {
        let app = self.app(db_id).await?;
        let _lock = self.lock_app(&app)?;
        let session = self.mongo_session(db_id).await?;
        self.archived_until(db_id).await?;
        session
            .eval("print(db.adminCommand({appendOplogNote: 1, data: {r3v3rs3: 'archive'}}).ok)")
            .await?;
        self.pull_oplog_locked(db_id).await?;
        self.archived_file(db_id, None).await
    }

    /// The recovery window over every timeline.
    pub(super) async fn oplog_pitr_range(&self, db_id: ShortId) -> anyhow::Result<PitrRange> {
        let (mut bases, mut segments) = (Vec::new(), Vec::new());
        for archive in self.archives(db_id).await? {
            bases.extend(archive.bases().await?);
            segments.extend(archive.segments().await?);
        }
        let earliest = bases.iter().map(|base| base.created_at).min();
        let latest = segments.iter().map(|s| s.end_time).max();
        let archiving = match self.mongo_session(db_id).await {
            Ok(session) => session
                .eval("print(db.hello().setName)")
                .await
                .is_ok_and(|set| set == super::mongo::REPLICA_SET),
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

    /// Recovers a MongoDB database to `request.target_time`, in place or into a new branch,
    /// with second precision.
    pub(super) async fn restore_oplog_pitr(
        self: &Arc<Self>,
        db_id: ShortId,
        app: &AppEntry,
        request: PitrRestoreRequest,
    ) -> anyhow::Result<PitrRestoreResponse> {
        let (base, files, upload) = self.oplog_plan(db_id, request.target_time).await?;
        let (target, branched) = self.recovery_target(db_id, app, &request).await?;
        self.recover_oplog_into(target.id, upload, request.target_time / 1000)
            .await?;
        self.reapply_tenants(target.id).await?;
        self.archive_oplog(target.id)
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

    /// The base backup, the oplog files and the upload of a recovery to `target`.
    async fn oplog_plan(
        &self,
        db_id: ShortId,
        target: u64,
    ) -> anyhow::Result<(BaseBackup, Vec<WalSegment>, Vec<(String, Vec<u8>)>)> {
        self.finish_history_moves(db_id).await?;
        if self.store.running_deployment(db_id).await?.is_some() {
            self.archive_oplog(db_id).await?;
        }
        let mut archives = self.archives(db_id).await?;
        let ends: Vec<u64> = archives.iter().filter_map(|a| a.end).collect();
        let archive = archives.swap_remove(timeline_for(&ends, target));
        let bases = archive.bases().await?;
        let segments = archive.segments().await?;
        let base = pick_base(&bases, &segments, target)?.clone();
        let files = replay_files(&segments, &base.start_wal);
        let upload = oplog_upload(&archive, &base, &files).await?;
        Ok((base, files, upload))
    }

    /// Replaces the data of the database with a fresh replica set that holds the base archive
    /// and the replayed oplog. A failed recovery puts the old data back.
    async fn recover_oplog_into(
        &self,
        db_id: ShortId,
        upload: Vec<(String, Vec<u8>)>,
        limit: u64,
    ) -> anyhow::Result<()> {
        let app = self.app(db_id).await?;
        let _lock = self.lock_app(&app)?;
        let (app, container, runtime) = self.database_container(db_id).await?;
        let mount = mount_of(&app)?;
        runtime
            .stop_container(&container, Duration::from_secs(60))
            .await?;
        match self
            .replay_into_fresh_set(&runtime, &app, &mount, upload, limit)
            .await
        {
            Ok(()) => self.finish_oplog_recovery(db_id, &mount).await,
            Err(err) => {
                self.roll_back_oplog(&runtime, &app, &container, &mount)
                    .await?;
                Err(Error::RestoreFailed {
                    reason: format!("{err:#}"),
                }
                .into())
            }
        }
    }

    /// Moves the archive into a history, while the lock keeps every pull of the new server
    /// out, and removes the data that the recovery kept aside.
    async fn finish_oplog_recovery(&self, db_id: ShortId, mount: &str) -> anyhow::Result<()> {
        self.start_history(db_id).await?;
        self.mongo_session(db_id)
            .await?
            .shell(&format!("rm -rf '{mount}/{PREVIOUS_DIR}' {WORK_DIR}"), &[])
            .await?;
        Ok(())
    }

    /// Moves the data aside, starts a fresh replica set on the empty data directory, restores
    /// the base archive without the `admin` database and replays the oplog up to `limit`.
    async fn replay_into_fresh_set(
        &self,
        runtime: &Arc<dyn ContainerRuntime>,
        app: &AppEntry,
        mount: &str,
        upload: Vec<(String, Vec<u8>)>,
        limit: u64,
    ) -> anyhow::Result<()> {
        self.run_db_helper(runtime, app, aside_script(mount), None)
            .await?;
        let session = self.ready_set(app).await?;
        let replay = upload.len() > 1;
        runtime
            .upload_archive(&session.container, "/tmp", tar_files(&upload)?)
            .await?;
        session
            .shell(
                &format!("mongorestore --uri=\"$MONGO_URI\" --archive={WORK_DIR}/base.archive --nsExclude='admin.*' --drop 2>&1"),
                &[],
            )
            .await
            .context("the base archive did not restore")?;
        if !replay {
            return Ok(());
        }
        session
            .shell(
                &format!("mkdir -p {WORK_DIR}/empty && mongorestore --uri=\"$MONGO_URI\" --oplogReplay --oplogFile={WORK_DIR}/oplog.bson --oplogLimit={limit} {WORK_DIR}/empty 2>&1"),
                &[],
            )
            .await
            .context("the oplog did not replay")?;
        Ok(())
    }

    /// Starts the database container and waits until its replica set takes writes.
    async fn ready_set(&self, app: &AppEntry) -> anyhow::Result<MongoSession> {
        let (_, container, runtime) = self.database_container(app.id).await?;
        runtime.start_container(&container).await?;
        self.ensure_replica_set(app.id, &*runtime, &container, app.spec.port)
            .await?;
        self.mongo_session(app.id).await
    }

    /// Puts the data that a failed recovery moved aside back and waits until it takes writes.
    async fn roll_back_oplog(
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
        self.ready_set(app)
            .await
            .context("the database did not answer on its old data")?;
        Ok(())
    }
}

/// The path of the data volume inside the container of a database.
fn mount_of(app: &AppEntry) -> anyhow::Result<String> {
    let info = r3v3rs3_api::platform::app_engine_info(&app.spec)
        .context("the engine is not in the database catalog")?;
    Ok(data_mount(&app.spec, &info.data_path)?
        .target
        .as_str()
        .to_string())
}

/// The base archive and the oplog files of a recovery as one BSON file, read before anything
/// stops, so a missing file fails the recovery while the database still runs on its data.
async fn oplog_upload(
    archive: &Archive,
    base: &BaseBackup,
    files: &[WalSegment],
) -> anyhow::Result<Vec<(String, Vec<u8>)>> {
    let dump = tokio::fs::read(archive.base.join(format!("{}.tar.gz", base.id))).await?;
    let mut upload = vec![("base.archive".to_string(), ungzip(&dump).await?)];
    if files.is_empty() {
        return Ok(upload);
    }
    // BSON documents follow each other without a separator, so the files join into one.
    let mut oplog = Vec::new();
    for segment in files {
        let path = archive.wal.join(format!("{}.wal.gz", segment.id));
        let compressed = tokio::fs::read(&path)
            .await
            .with_context(|| format!("the oplog file {} is missing", segment.filename))?;
        oplog.extend(ungzip(&compressed).await?);
    }
    ensure!(!oplog.is_empty(), "the archived oplog is empty");
    upload.push(("oplog.bson".to_string(), oplog));
    Ok(upload)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// An oplog timestamp is (seconds, ordinal); later entries sort after earlier ones, also
    /// within one second, and the archive replays its files in the order of their names
    /// (MongoDB manual, "BSON Types: Timestamps").
    #[test]
    fn oplog_files_replay_in_the_order_of_their_timestamps() {
        let pack = |t: u64, i: u64| (t << 32) | i;
        let segment = |packed: u64| WalSegment {
            id: packed.to_string(),
            app: "abc-def".parse().unwrap(),
            filename: oplog_name(packed),
            size: 1,
            sha256: String::new(),
            start_time: 0,
            end_time: 1,
            created_at: 1,
            native: true,
        };
        let segments = [
            segment(pack(1_795_000_100, 2)),
            segment(pack(1_795_000_100, 10)),
            segment(pack(1_795_000_099, 7)),
            segment(pack(1_794_999_000, 1)),
        ];
        let base = oplog_name(pack(1_795_000_000, 0));
        let names: Vec<String> = replay_files(&segments, &base)
            .into_iter()
            .map(|s| s.id)
            .collect();
        let expected: Vec<String> = [
            pack(1_795_000_099, 7),
            pack(1_795_000_100, 2),
            pack(1_795_000_100, 10),
        ]
        .map(|p| p.to_string())
        .to_vec();
        assert_eq!(names, expected);
    }

    /// The query of a pull leaves the account entries out and covers exactly the entries
    /// after the archive up to the newest one (MongoDB Extended JSON v2, `$timestamp`).
    #[test]
    fn the_pull_query_names_the_range_and_leaves_the_accounts_out() {
        let since = (1_795_000_000_u64 << 32) | 3;
        let until = (1_795_000_060_u64 << 32) | 1;
        let query: serde_json::Value = serde_json::from_str(&dump_query(since, until)).unwrap();
        assert_eq!(
            query["ts"]["$gt"]["$timestamp"],
            serde_json::json!({"t": 1_795_000_000_u64, "i": 3})
        );
        assert_eq!(
            query["ts"]["$lte"]["$timestamp"],
            serde_json::json!({"t": 1_795_000_060_u64, "i": 1})
        );
        assert_eq!(
            query["ns"]["$nin"],
            serde_json::json!(["admin.system.users", "admin.system.roles"])
        );
    }
}
