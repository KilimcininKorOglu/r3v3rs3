//! Continuous archiving of the MySQL and MariaDB binary log and point-in-time recovery from it.
//!
//! The server writes its changes into the binary log in row format. The platform closes the
//! open log file once a minute when it grew, copies the closed files into the backup
//! directory and purges them on the server. A base backup is a dump with the binary log
//! coordinates of its snapshot. A recovery loads the dump into a fresh server and replays the
//! binary log from those coordinates up to the target time (MySQL manual, "Point-in-Time
//! Recovery Using Binary Log"; MariaDB "mariadb-binlog").

use super::Platform;
use super::backup::ungzip;
use super::history::{Archive, timeline_for};
use super::wal::{
    BaseBackup, BaseStart, PREVIOUS_DIR, data_mount, is_archive_name, pick_base, replay_files,
    rollback_script, store_native_segment,
};
use crate::runtime::ContainerRuntime;
use anyhow::{Context as _, bail, ensure};
use r3v3rs3_api::container::ContainerName;
use r3v3rs3_api::error::Error;
use r3v3rs3_api::id::ShortId;
use r3v3rs3_api::platform::{
    AppEntry, PitrRange, PitrRestoreRequest, PitrRestoreResponse, WalSegment,
};
use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use std::time::Duration;

/// The start arguments of a server that writes its binary log. The official images pass
/// arguments that start with `-` to the server. Row format records the changed rows, so a
/// replay does not depend on the time or state of the replaying server.
const BINLOG_ARGS: [&str; 3] = ["--log-bin=binlog", "--server-id=1", "--binlog-format=ROW"];

/// The directory inside the container that a recovery uploads its files to.
pub(super) const WORK_DIR: &str = "/tmp/r3v3rs3-pitr";

/// How long a fresh server may take to initialise and answer.
const READY_TIMEOUT: Duration = Duration::from_secs(180);

/// The open log file and its size after the last flush of each database, so the pull loop
/// closes a file only when the server wrote into it.
static FLUSHED: Mutex<Option<HashMap<ShortId, (String, u64)>>> = Mutex::new(None);

/// The start command of a MySQL or MariaDB server that writes its binary log.
pub(super) fn binlog_command() -> Vec<String> {
    BINLOG_ARGS.map(str::to_string).to_vec()
}

/// The tools of one engine.
struct Tools {
    client: &'static str,
    /// The decoder of the binary log with the flags that keep the replay out of the binary log
    /// of the target. MySQL also needs `--skip-gtids`: the GTIDs of the source make the target
    /// log empty transactions under them.
    decoder: &'static str,
    /// The dump flag that writes the binary log coordinates of the snapshot as a comment.
    coordinates: &'static str,
}

fn tools(engine: &str) -> anyhow::Result<Tools> {
    match engine {
        // The image ships `mysqlbinlog` only inside MySQL Shell, off the PATH.
        "mysql" => Ok(Tools {
            client: "mysql",
            decoder: "$(command -v mysqlbinlog || echo /usr/libexec/mysqlsh/mysqlbinlog) --disable-log-bin --skip-gtids",
            coordinates: "--source-data=2",
        }),
        "mariadb" => Ok(Tools {
            client: "mariadb",
            decoder: "mariadb-binlog --disable-log-bin",
            coordinates: "--master-data=2",
        }),
        other => bail!("the engine {other} keeps no binary log"),
    }
}

/// The dump of every user database with the coordinates of its snapshot. The `mysql` schema
/// with the accounts stays out, so a recovery cannot bring back an old password.
fn base_dump_script(engine: &str, tools: &Tools) -> String {
    let (client, dump, extra) = match engine {
        "mysql" => ("mysql", "mysqldump", " --set-gtid-purged=OFF"),
        _ => ("mariadb", "mariadb-dump", ""),
    };
    format!(
        "dbs=$({client} -u root -N -e \"SELECT schema_name FROM information_schema.schemata WHERE schema_name NOT IN ('mysql', 'sys', 'information_schema', 'performance_schema')\") || exit 1; \
if [ -z \"$dbs\" ]; then echo '-- no user databases'; else {dump} -u root --single-transaction --routines --events{extra} {coordinates} --databases $dbs; fi",
        coordinates = tools.coordinates
    )
}

/// The binary log coordinates in the comment that `--source-data=2` (MySQL) or
/// `--master-data=2` (MariaDB) writes at the head of a dump.
fn dump_coordinates(dump: &str) -> Option<(String, u64)> {
    let line = dump
        .lines()
        .take(100)
        .find(|line| line.starts_with("-- CHANGE "))?;
    let file = line.split("_LOG_FILE='").nth(1)?.split('\'').next()?;
    let position = line.split("_LOG_POS=").nth(1)?;
    let position = position.trim_end_matches(';').trim().parse().ok()?;
    is_archive_name(file).then(|| (file.to_string(), position))
}

/// Whether a statement of a decoded binary log changes accounts. The platform owns the
/// accounts and sets them after a recovery; a replayed `CREATE USER` of an account that the
/// target already has stops the whole replay.
fn is_account_statement(statement: &str) -> bool {
    let words: Vec<String> = statement
        .split_whitespace()
        .take(3)
        .map(str::to_ascii_uppercase)
        .collect();
    let word = |i: usize| words.get(i).map_or("", String::as_str);
    match word(0) {
        "GRANT" | "REVOKE" => true,
        "CREATE" | "ALTER" | "DROP" | "RENAME" => matches!(word(1), "USER" | "ROLE"),
        "SET" => matches!(word(1), "PASSWORD" | "DEFAULT"),
        _ => false,
    }
}

/// A decoded binary log without its account statements. Every statement ends with the
/// delimiter `/*!*/;`; the comment, `SET` and `use` lines in front of a statement carry the
/// state of the session and stay.
fn without_account_statements(decoded: &str) -> String {
    let mut out = String::with_capacity(decoded.len());
    let mut block: Vec<&str> = Vec::new();
    for line in decoded.lines() {
        block.push(line);
        if line.trim_end().ends_with("/*!*/;") {
            push_block(&mut out, &block);
            block.clear();
        }
    }
    for line in block {
        out.push_str(line);
        out.push('\n');
    }
    out
}

fn is_session_line(line: &str) -> bool {
    line.starts_with('#') || line.starts_with("SET ") || line.starts_with("use ")
}

fn push_block(out: &mut String, block: &[&str]) {
    let statement: Vec<&str> = block
        .iter()
        .copied()
        .skip_while(|line| is_session_line(line))
        .collect();
    let drop = is_account_statement(&statement.join("\n"));
    for line in block {
        if !drop || is_session_line(line) {
            out.push_str(line);
            out.push('\n');
        }
    }
}

/// A time for `--stop-datetime`: whole seconds in UTC, rounded down. Binary log events carry
/// whole seconds and the decoder stops at the first event at or after the time, so a commit
/// in the second of the target is left out rather than a later one replayed.
fn stop_datetime(unix_ms: u64) -> anyhow::Result<String> {
    let seconds = i64::try_from(unix_ms / 1000)?;
    let time = time::OffsetDateTime::from_unix_timestamp(seconds)?;
    Ok(format!(
        "{:04}-{:02}-{:02} {:02}:{:02}:{:02}",
        time.year(),
        u8::from(time.month()),
        time.day(),
        time.hour(),
        time.minute(),
        time.second()
    ))
}

/// The running container of a MySQL or MariaDB database with its root password.
struct MySession {
    container: ContainerName,
    runtime: Arc<dyn ContainerRuntime>,
    engine: String,
    tools: Tools,
    password: String,
}

impl MySession {
    fn env(&self) -> Vec<String> {
        vec![format!("MYSQL_PWD={}", self.password), "TZ=UTC".to_string()]
    }

    /// Runs a shell command in the container and returns its standard output.
    async fn shell(&self, script: String) -> anyhow::Result<Vec<u8>> {
        let command = vec!["sh".to_string(), "-c".to_string(), script];
        self.runtime
            .exec_capture(&self.container, command, self.env())
            .await
    }

    /// The rows of one query, tab-separated, without column names.
    async fn query(&self, sql: &str) -> anyhow::Result<String> {
        let command = [self.tools.client, "-u", "root", "-N", "-B", "-e", sql]
            .map(str::to_string)
            .to_vec();
        let out = self
            .runtime
            .exec_capture(&self.container, command, self.env())
            .await?;
        Ok(String::from_utf8_lossy(&out).trim().to_string())
    }

    /// The binary log files of the server with their sizes, oldest first.
    async fn logs(&self) -> anyhow::Result<Vec<(String, u64)>> {
        let rows = self.query("SHOW BINARY LOGS").await?;
        let mut logs = Vec::new();
        for row in rows.lines() {
            let mut columns = row.split('\t');
            let name = columns.next().unwrap_or_default();
            let size = columns.next().and_then(|size| size.parse().ok());
            let size = size.with_context(|| format!("unexpected binary log row: {row}"))?;
            ensure!(is_archive_name(name), "unexpected binary log name: {name}");
            logs.push((name.to_string(), size));
        }
        ensure!(!logs.is_empty(), "the server writes no binary log");
        Ok(logs)
    }

    async fn datadir(&self) -> anyhow::Result<String> {
        let dir = self.query("SELECT @@datadir").await?;
        Ok(dir.trim_end_matches('/').to_string())
    }
}

/// The open log file of `logs` and whether it changed since the last flush.
fn grew_since_flush(db_id: ShortId, logs: &[(String, u64)]) -> bool {
    let Some(open) = logs.last() else {
        return false;
    };
    let flushed = FLUSHED
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    flushed
        .as_ref()
        .and_then(|map| map.get(&db_id))
        .is_none_or(|last| last != open)
}

fn remember_flush(db_id: ShortId, open: Option<(String, u64)>) {
    let mut flushed = FLUSHED
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    let map = flushed.get_or_insert_with(HashMap::new);
    match open {
        Some(open) => map.insert(db_id, open),
        None => map.remove(&db_id),
    };
}

impl Platform {
    async fn my_session(&self, db_id: ShortId) -> anyhow::Result<MySession> {
        let (app, container, runtime) = self.database_container(db_id).await?;
        let info = r3v3rs3_api::platform::app_engine_info(&app.spec)
            .context("the engine is not in the database catalog")?;
        let password = self.database_password(db_id, &info).await?;
        let engine = app.spec.database.clone().unwrap_or_default();
        Ok(MySession {
            container,
            runtime,
            tools: tools(&engine)?,
            engine,
            password,
        })
    }

    /// Closes the open log file when it grew (or always with `force`), copies the closed files
    /// into the backup directory and purges them on the server once they are stored.
    pub(super) async fn pull_binlog_locked(
        &self,
        db_id: ShortId,
        force: bool,
    ) -> anyhow::Result<Vec<WalSegment>> {
        let session = self.my_session(db_id).await?;
        self.finish_history_moves(db_id).await?;
        self.drop_legacy_segments(db_id).await?;
        let first = self.first_needed_log(db_id).await?;
        let logs = flush_if_grew(&session, db_id, force).await?;
        let stored = self.native_segments(db_id).await?;
        let names = files_to_fetch(&logs, &stored, &first);
        let pulled = self.store_logs(db_id, &session, &names).await?;
        purge_closed(&session, &logs).await?;
        Ok(pulled)
    }

    /// The log file where the replay from the oldest base backup starts. A database without a
    /// base backup gets one first: a MySQL server keeps its binary log by default, often for
    /// weeks, and the files before the first base are never needed.
    async fn first_needed_log(&self, db_id: ShortId) -> anyhow::Result<String> {
        if let Some(base) = self.list_base_backups(db_id).await?.into_iter().next() {
            return Ok(base.start_wal);
        }
        Ok(self.take_binlog_base_locked(db_id).await?.start_wal)
    }

    /// Stores the given closed log files.
    async fn store_logs(
        &self,
        db_id: ShortId,
        session: &MySession,
        names: &[String],
    ) -> anyhow::Result<Vec<WalSegment>> {
        let stored = self.native_segments(db_id).await?;
        let dir = self.wal_dir(db_id).await?;
        tokio::fs::create_dir_all(&dir).await?;
        let datadir = session.datadir().await?;
        let mut start = stored.iter().map(|s| s.end_time).max().unwrap_or(0);
        let mut pulled = Vec::new();
        for name in names {
            let path = format!("{datadir}/{name}");
            let bytes = session
                .runtime
                .download_file(&session.container, &path)
                .await?;
            let segment = store_native_segment(&dir, db_id, name.clone(), &bytes, start).await?;
            start = segment.end_time;
            pulled.push(segment);
        }
        Ok(pulled)
    }

    /// Takes a dump with the binary log coordinates of its snapshot.
    pub(super) async fn take_binlog_base_locked(
        &self,
        db_id: ShortId,
    ) -> anyhow::Result<BaseBackup> {
        let session = self.my_session(db_id).await?;
        let script = base_dump_script(&session.engine, &session.tools);
        let dump = session.shell(script).await?;
        let text = String::from_utf8_lossy(&dump);
        let (log, position) = match dump_coordinates(&text) {
            Some(coordinates) => coordinates,
            // A server without user databases dumps nothing, so the open file is the start.
            None => self.open_log_end(&session).await?,
        };
        let start = BaseStart {
            log,
            position: Some(position),
            data_directory: session.datadir().await?,
        };
        let base = self.store_base(db_id, start, &dump).await?;
        self.prune_base_backups(db_id).await?;
        Ok(base)
    }

    /// The open log file and its size, where the next event goes.
    async fn open_log_end(&self, session: &MySession) -> anyhow::Result<(String, u64)> {
        session
            .logs()
            .await?
            .pop()
            .context("the server writes no binary log")
    }

    /// Closes the open log file, pulls the archive and returns the newest stored file. A
    /// database without a base backup gets one, so the archive can be replayed.
    pub(super) async fn archive_binlog(&self, db_id: ShortId) -> anyhow::Result<WalSegment> {
        let app = self.app(db_id).await?;
        let _lock = self.lock_app(&app)?;
        self.pull_binlog_locked(db_id, true).await?;
        let segment = self.archived_file(db_id, None).await?;
        if self.list_base_backups(db_id).await?.is_empty() {
            self.take_binlog_base_locked(db_id).await?;
        }
        Ok(segment)
    }

    /// The recovery window: from the end of the oldest base backup to the newest pulled file.
    pub(super) async fn binlog_pitr_range(&self, db_id: ShortId) -> anyhow::Result<PitrRange> {
        let (mut bases, mut segments) = (Vec::new(), Vec::new());
        for archive in self.archives(db_id).await? {
            bases.extend(archive.bases().await?);
            segments.extend(archive.segments().await?);
        }
        let earliest = bases.iter().map(|base| base.created_at).min();
        let latest = segments.iter().map(|s| s.end_time).max();
        let archiving = match self.my_session(db_id).await {
            Ok(session) => session
                .query("SELECT @@log_bin")
                .await
                .is_ok_and(|on| on == "1"),
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

    /// Recovers a MySQL or MariaDB database to `request.target_time`, in place or into a new
    /// branch, with second precision.
    pub(super) async fn restore_binlog_pitr(
        self: &Arc<Self>,
        db_id: ShortId,
        app: &AppEntry,
        request: PitrRestoreRequest,
    ) -> anyhow::Result<PitrRestoreResponse> {
        let (base, files, upload) = self.binlog_plan(db_id, request.target_time).await?;
        let stop = stop_datetime(request.target_time)?;
        let (target, branched) = self.recovery_target(db_id, app, &request).await?;
        self.recover_binlog_into(target.id, &base, upload, &stop)
            .await?;
        self.reapply_tenants(target.id).await?;
        self.start_new_history(target.id).await?;
        Ok(PitrRestoreResponse {
            app: target,
            target_time: request.target_time,
            base_backup_id: base.id,
            wal_segments_replayed: files.len(),
            branched,
        })
    }

    /// The base backup, the log files and the upload of a recovery to `target`. The open log
    /// file of a running server is closed first, so the archive reaches now.
    async fn binlog_plan(
        &self,
        db_id: ShortId,
        target: u64,
    ) -> anyhow::Result<(BaseBackup, Vec<WalSegment>, Vec<(String, Vec<u8>)>)> {
        self.finish_history_moves(db_id).await?;
        if self.store.running_deployment(db_id).await?.is_some() {
            self.archive_binlog(db_id).await?;
        }
        let mut archives = self.archives(db_id).await?;
        let ends: Vec<u64> = archives.iter().filter_map(|a| a.end).collect();
        let archive = archives.swap_remove(timeline_for(&ends, target));
        let bases = archive.bases().await?;
        let segments = archive.segments().await?;
        let base = pick_base(&bases, &segments, target)?.clone();
        let files = replay_files(&segments, &base.start_wal);
        let upload = binlog_upload(&archive, &base, &files).await?;
        Ok((base, files, upload))
    }
}

/// The dump and the log files that a recovery uploads, read before anything stops, so a
/// missing file fails the recovery while the database still runs on its data.
async fn binlog_upload(
    archive: &Archive,
    base: &BaseBackup,
    files: &[WalSegment],
) -> anyhow::Result<Vec<(String, Vec<u8>)>> {
    let dump = tokio::fs::read(archive.base.join(format!("{}.tar.gz", base.id))).await?;
    let mut upload = vec![("base.sql".to_string(), ungzip(&dump).await?)];
    for segment in files {
        let path = archive.wal.join(format!("{}.wal.gz", segment.id));
        let compressed = tokio::fs::read(&path)
            .await
            .with_context(|| format!("the binary log {} is missing", segment.filename))?;
        upload.push((segment.filename.clone(), ungzip(&compressed).await?));
    }
    Ok(upload)
}

impl Platform {
    /// Replaces the data of the database with a fresh server that holds the base dump and the
    /// replayed binary log. A failed recovery puts the old data back.
    async fn recover_binlog_into(
        &self,
        db_id: ShortId,
        base: &BaseBackup,
        upload: Vec<(String, Vec<u8>)>,
        stop: &str,
    ) -> anyhow::Result<()> {
        let app = self.app(db_id).await?;
        let _lock = self.lock_app(&app)?;
        let (app, container, runtime) = self.database_container(db_id).await?;
        let info = r3v3rs3_api::platform::app_engine_info(&app.spec)
            .context("the engine is not in the database catalog")?;
        let mount = data_mount(&app.spec, &info.data_path)?
            .target
            .as_str()
            .to_string();
        runtime
            .stop_container(&container, Duration::from_secs(60))
            .await?;
        let replayed = self
            .replay_into_fresh_server(&runtime, &app, &mount, (base, upload, stop))
            .await;
        match replayed {
            Ok(()) => {
                // The archive moves while the lock keeps every pull of the new server out.
                self.start_history(db_id).await?;
                self.finish_binlog_recovery(db_id, &mount).await
            }
            Err(err) => {
                self.roll_back_binlog(&runtime, &app, &container, &mount)
                    .await?;
                Err(Error::RestoreFailed {
                    reason: format!("{err:#}"),
                }
                .into())
            }
        }
    }

    /// Moves the data aside, starts a fresh server on the empty data directory, loads the
    /// dump and replays the binary log without its account statements.
    async fn replay_into_fresh_server(
        &self,
        runtime: &Arc<dyn ContainerRuntime>,
        app: &AppEntry,
        mount: &str,
        (base, upload, stop): (&BaseBackup, Vec<(String, Vec<u8>)>, &str),
    ) -> anyhow::Result<()> {
        self.run_db_helper(runtime, app, aside_script(mount), None)
            .await?;
        let session = self.ready_session(app.id).await?;
        let position = base.start_position.unwrap_or(4);
        let names: Vec<String> = upload
            .iter()
            .skip(1)
            .map(|(name, _)| name.clone())
            .collect();
        let archive = tar_files(&upload)?;
        runtime
            .upload_archive(&session.container, "/tmp", archive)
            .await?;
        session
            .shell(format!(
                "{{ echo 'SET SESSION sql_log_bin=0;'; cat {WORK_DIR}/base.sql; }} | {} -u root",
                session.tools.client
            ))
            .await
            .context("the base dump did not load")?;
        if names.is_empty() {
            return Ok(());
        }
        let files: Vec<String> = names.iter().map(|n| format!("{WORK_DIR}/{n}")).collect();
        let decoded = session
            .shell(format!(
                "{} --start-position={position} --stop-datetime='{stop}' {}",
                session.tools.decoder,
                files.join(" ")
            ))
            .await
            .context("the binary log could not be decoded")?;
        let replay = without_account_statements(&String::from_utf8_lossy(&decoded));
        runtime
            .upload_file(
                &session.container,
                &format!("{WORK_DIR}/replay.sql"),
                replay.into_bytes(),
            )
            .await?;
        session
            .shell(format!(
                "{} -u root < {WORK_DIR}/replay.sql",
                session.tools.client
            ))
            .await
            .context("the binary log did not replay")?;
        Ok(())
    }

    /// Starts the database container and waits until the final server answers. On an empty
    /// data directory the entrypoint first runs a temporary server without networking for its
    /// setup and stops it again; a load into that server would be cut off.
    async fn ready_session(&self, db_id: ShortId) -> anyhow::Result<MySession> {
        let (_, container, runtime) = self.database_container(db_id).await?;
        runtime.start_container(&container).await?;
        let deadline = tokio::time::Instant::now() + READY_TIMEOUT;
        loop {
            if let Ok(session) = self.my_session(db_id).await
                && session
                    .query("SELECT @@skip_networking")
                    .await
                    .is_ok_and(|off| off == "0")
            {
                return Ok(session);
            }
            ensure!(
                tokio::time::Instant::now() < deadline,
                "the database did not answer in {} seconds",
                READY_TIMEOUT.as_secs()
            );
            tokio::time::sleep(Duration::from_secs(1)).await;
        }
    }

    /// Removes the data that the recovery kept aside and the uploaded files.
    async fn finish_binlog_recovery(&self, db_id: ShortId, mount: &str) -> anyhow::Result<()> {
        let session = self.my_session(db_id).await?;
        session
            .shell(format!("rm -rf '{mount}/{PREVIOUS_DIR}' {WORK_DIR}"))
            .await?;
        Ok(())
    }

    /// Puts the data that a failed recovery moved aside back and waits until it answers.
    async fn roll_back_binlog(
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
        self.ready_session(app.id)
            .await
            .context("the database did not answer on its old data")?;
        Ok(())
    }

    /// The recovered server starts its binary log again at its first file, and the replay is
    /// not in it. The recovery moved the old archive into a history, so a new base backup
    /// starts the new one.
    async fn start_new_history(&self, db_id: ShortId) -> anyhow::Result<()> {
        remember_flush(db_id, None);
        self.archive_binlog(db_id)
            .await
            .context("the recovered database took no new base backup")?;
        Ok(())
    }
}

/// The closed log files that the archive needs and does not hold yet: the open file (the last
/// one) is still written, and a file before `first` precedes every base backup.
fn files_to_fetch(logs: &[(String, u64)], stored: &[WalSegment], first: &str) -> Vec<String> {
    let first = super::wal::log_order(first);
    let closed = logs.split_last().map_or(&[][..], |(_, closed)| closed);
    closed
        .iter()
        .map(|(name, _)| name)
        .filter(|name| super::wal::log_order(name) >= first)
        .filter(|name| !stored.iter().any(|s| &&s.filename == name))
        .cloned()
        .collect()
}

/// Lists the log files of the server, after closing the open one when it grew since the last
/// flush (or always with `force`).
async fn flush_if_grew(
    session: &MySession,
    db_id: ShortId,
    force: bool,
) -> anyhow::Result<Vec<(String, u64)>> {
    let logs = session.logs().await?;
    if !force && !grew_since_flush(db_id, &logs) {
        return Ok(logs);
    }
    session.query("FLUSH BINARY LOGS").await?;
    let logs = session.logs().await?;
    remember_flush(db_id, logs.last().cloned());
    Ok(logs)
}

/// Purges the closed log files on the server. The server owns its index of the files, so they
/// go only through `PURGE BINARY LOGS`, never by a removal of the file.
async fn purge_closed(session: &MySession, logs: &[(String, u64)]) -> anyhow::Result<()> {
    let (Some((open, _)), true) = (logs.last(), logs.len() > 1) else {
        return Ok(());
    };
    session
        .query(&format!("PURGE BINARY LOGS TO '{open}'"))
        .await?;
    Ok(())
}

/// The shell script that moves the data of the server aside, leaving an empty data directory
/// that the entrypoint of the image initialises again.
pub(super) fn aside_script(mount: &str) -> String {
    format!(
        "set -e; M='{mount}'; P=\"$M/{PREVIOUS_DIR}\"; rm -rf \"$P\"; mkdir \"$P\"; \
find \"$M\" -mindepth 1 -maxdepth 1 ! -name {PREVIOUS_DIR} -exec mv {{}} \"$P\"/ \\;"
    )
}

/// A tar archive of the work directory with the given files.
pub(super) fn tar_files(files: &[(String, Vec<u8>)]) -> anyhow::Result<Vec<u8>> {
    let mut builder = tar::Builder::new(Vec::new());
    for (name, bytes) in files {
        let mut header = tar::Header::new_gnu();
        header.set_size(bytes.len() as u64);
        header.set_mode(0o644);
        header.set_cksum();
        let path = format!("{}/{name}", WORK_DIR.trim_start_matches("/tmp/"));
        builder.append_data(&mut header, path, bytes.as_slice())?;
    }
    Ok(builder.into_inner()?)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The comments that the dump tools write, as measured on mysql:9 and mariadb:11.8.
    #[test]
    fn the_coordinates_of_both_dump_tools_are_read() {
        let mysql = "-- MySQL dump 10.13\n--\n-- CHANGE REPLICATION SOURCE TO SOURCE_LOG_FILE='binlog.000003', SOURCE_LOG_POS=1153;\n";
        assert_eq!(
            dump_coordinates(mysql),
            Some(("binlog.000003".to_string(), 1153))
        );
        let mariadb = "-- CHANGE MASTER TO MASTER_LOG_FILE='binlog.000002', MASTER_LOG_POS=939;\n";
        assert_eq!(
            dump_coordinates(mariadb),
            Some(("binlog.000002".to_string(), 939))
        );
        assert_eq!(dump_coordinates("-- no user databases\n"), None);
        // A file name from the dump must not reach a shell or a path outside the data dir.
        let hostile = "-- CHANGE MASTER TO MASTER_LOG_FILE='../x;rm', MASTER_LOG_POS=4;\n";
        assert_eq!(dump_coordinates(hostile), None);
    }

    /// A replayed `CREATE USER` of an account that the target has stops the replay (ERROR
    /// 1396, measured), so account statements leave the replay; data statements stay.
    #[test]
    fn account_statements_leave_the_replay() {
        let decoded = "# at 4\nSET TIMESTAMP=1/*!*/;\nCREATE USER 'tenant2'@'%' IDENTIFIED WITH 'caching_sha2_password' AS 'x'\n/*!*/;\n# at 9\nGRANT ALL PRIVILEGES ON `app`.* TO 'tenant2'@'%'\n/*!*/;\nuse `app`/*!*/;\nCREATE TABLE u (i int)\n/*!*/;\nBINLOG '\nAAAA\n'/*!*/;\n";
        let replay = without_account_statements(decoded);
        assert!(!replay.contains("CREATE USER"), "{replay}");
        assert!(!replay.contains("GRANT"), "{replay}");
        assert!(replay.contains("CREATE TABLE u"), "{replay}");
        assert!(replay.contains("BINLOG '"), "{replay}");
        assert!(replay.contains("SET TIMESTAMP=1"), "{replay}");
        for statement in [
            "ALTER USER 'a'@'%' IDENTIFIED BY 'b'",
            "drop role r",
            "SET PASSWORD FOR a = 'b'",
            "REVOKE ALL ON *.* FROM a",
        ] {
            assert!(is_account_statement(statement), "{statement}");
        }
        assert!(!is_account_statement("CREATE TABLE users (i int)"));
        assert!(!is_account_statement("INSERT INTO grants VALUES (1)"));
    }

    /// The `mysql` schema holds every account with its password hash; a base dump that carried
    /// it would bring back old passwords over the stored ones. The dump also needs the binary
    /// log coordinates of its snapshot, with the flag that the tool of the engine reads.
    #[test]
    fn the_base_dump_leaves_the_accounts_out_and_records_its_coordinates() {
        for (engine, tool, flag) in [
            ("mysql", "mysqldump", "--source-data=2"),
            ("mariadb", "mariadb-dump", "--master-data=2"),
        ] {
            let script = base_dump_script(engine, &tools(engine).unwrap());
            assert!(script.contains("NOT IN ('mysql', 'sys'"), "{script}");
            assert!(!script.contains("--all-databases"), "{script}");
            assert!(script.contains(tool) && script.contains(flag), "{script}");
            assert!(script.contains("--single-transaction"), "{script}");
        }
    }

    /// A default mysql:9 writes its binary log from the start (`@@log_bin = 1`, measured) and
    /// keeps weeks of files. The first pull after an upgrade must fetch none of the files before
    /// the first base backup, nor the open file, nor a file it already holds.
    #[test]
    fn only_the_closed_files_from_the_first_base_are_fetched() {
        let logs: Vec<(String, u64)> = [
            "binlog.000001",
            "binlog.000002",
            "binlog.000003",
            "binlog.000004",
            "binlog.000005",
        ]
        .map(|name| (name.to_string(), 100))
        .to_vec();
        let stored = WalSegment {
            id: "a".into(),
            app: "abc-def".parse().unwrap(),
            filename: "binlog.000003".into(),
            size: 1,
            sha256: String::new(),
            start_time: 0,
            end_time: 1,
            created_at: 1,
            native: true,
        };
        let names = files_to_fetch(&logs, &[stored], "binlog.000003");
        assert_eq!(names, ["binlog.000004"]);
    }

    /// `date -u -r 1795000123` gives 2026-11-18 11:08:43; the milliseconds are dropped.
    #[test]
    fn the_stop_time_is_the_utc_second_rounded_down() {
        assert_eq!(
            stop_datetime(1_795_000_123_999).unwrap(),
            "2026-11-18 11:08:43"
        );
    }

    /// Binary log files sort by their number, not by the text: `binlog.1000000` comes after
    /// `binlog.999999`.
    #[test]
    fn binary_logs_replay_in_the_order_of_their_number() {
        let segment = |name: &str| WalSegment {
            id: name.to_string(),
            app: "abc-def".parse().unwrap(),
            filename: name.to_string(),
            size: 1,
            sha256: String::new(),
            start_time: 0,
            end_time: 1,
            created_at: 1,
            native: true,
        };
        let segments = [
            segment("binlog.1000000"),
            segment("binlog.999998"),
            segment("binlog.999999"),
        ];
        let names: Vec<String> = replay_files(&segments, "binlog.999999")
            .into_iter()
            .map(|s| s.filename)
            .collect();
        assert_eq!(names, ["binlog.999999", "binlog.1000000"]);
    }
}
