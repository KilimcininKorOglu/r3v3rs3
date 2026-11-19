//! Point-in-time recovery of Redis from the archived append-only file on a Docker Engine.
//! The required behavior comes from the Redis documentation, "Redis persistence": with
//! `aof-timestamp-enabled`, `redis-check-aof --truncate-to-timestamp` cuts the AOF at a
//! second, and a server that loads the cut AOF holds the writes before it. An existing database
//! must keep its data when its next deployment turns the AOF on.
//!
//! The tests need a Docker Engine (`DOCKER_HOST`, else `unix:///var/run/docker.sock`).
//! `make test-runtime-docker` runs them.

use super::Platform;
use super::tests::{TempDir, platform_with, request};
use anyhow::{Context as _, ensure};
use r3v3rs3_api::container::{
    ContainerName, ContainerSpec, EnvVar, ResourceLimits, RestartPolicy, VolumeMount,
};
use r3v3rs3_api::id::ShortId;
use r3v3rs3_api::platform::{
    AppEntry, AppSource, DeploymentTrigger, PitrRestoreRequest, PlatformConfig,
};
use std::collections::BTreeMap;
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::mpsc;

const PASSWORD: &str = "s3cret0123";

const IMAGE: &str = "mirror.gcr.io/library/redis:8-alpine";

fn catalog() {
    let catalog: r3v3rs3_api::platform::EnginesCatalog =
        serde_json::from_str(include_str!("../../../r3v3rs3/data/engines.json")).unwrap();
    r3v3rs3_api::platform::install_engines(catalog);
}

async fn docker_platform() -> anyhow::Result<(Arc<Platform>, TempDir)> {
    let (command, _) = mpsc::channel(1);
    let (mut platform, _fake, dir) = platform_with(&PlatformConfig::default(), command).await?;
    let host = std::env::var("DOCKER_HOST").unwrap_or("unix:///var/run/docker.sock".into());
    platform.local = Arc::new(super::docker_runtime(&host)?);
    platform.timing.health_timeout = Duration::from_secs(120);
    Ok((Arc::new(platform), dir))
}

fn data_volume(name: &str) -> anyhow::Result<VolumeMount> {
    Ok(VolumeMount {
        volume: format!("{name}-data").parse()?,
        target: "/data".parse()?,
        read_only: false,
    })
}

/// Adds a Redis database app on the volume `<name>-data` with a host port, as
/// `create_database` makes it, without deploying it.
async fn add_db(platform: &Arc<Platform>, name: &str) -> anyhow::Result<AppEntry> {
    let mut req = request(name);
    req.spec.source = AppSource::Image {
        image: IMAGE.parse()?,
    };
    req.spec.port = 6379;
    req.spec.host_port = Some(platform.allocate_host_port().await?);
    req.spec.database = Some("redis".into());
    req.spec.volumes = vec![data_volume(name)?];
    let app = platform.add_app(req, 1).await?;
    let env = super::database_env::initial_db_env_entries(
        &r3v3rs3_api::platform::engine_info("redis").context("the catalog")?,
        PASSWORD,
    )?;
    platform.set_env(app.id, env).await?;
    Ok(app)
}

async fn deploy(platform: &Arc<Platform>, id: ShortId) -> anyhow::Result<()> {
    platform
        .deploy(id, "alice", DeploymentTrigger::Manual)
        .await?;
    let deadline = tokio::time::Instant::now() + Duration::from_secs(240);
    loop {
        let latest = platform.deployments(id).await?.into_iter().next();
        let serving = platform.store.running_deployment(id).await?;
        if !platform.busy_apps().contains(&id) && latest.as_ref().map(|d| d.id) == serving {
            return Ok(());
        }
        ensure!(
            tokio::time::Instant::now() < deadline,
            "{id} does not serve: {latest:?}"
        );
        tokio::time::sleep(Duration::from_millis(500)).await;
    }
}

/// The answer of one `redis-cli` command in the container of the database.
async fn cli(platform: &Arc<Platform>, id: ShortId, args: &str) -> anyhow::Result<String> {
    let (_, container, runtime) = platform.database_container(id).await?;
    let command = vec!["sh".into(), "-c".into(), format!("redis-cli {args}")];
    let env = vec![format!("REDISCLI_AUTH={PASSWORD}")];
    let out = runtime.exec_capture(&container, command, env).await?;
    Ok(String::from_utf8_lossy(&out).trim().to_string())
}

/// The items of the list `t`, joined with commas.
async fn items(platform: &Arc<Platform>, id: ShortId) -> anyhow::Result<String> {
    Ok(cli(platform, id, "LRANGE t 0 -1")
        .await?
        .lines()
        .collect::<Vec<_>>()
        .join(","))
}

/// Appends item `i` to the list `t` and returns the server time of the write in Unix
/// milliseconds.
async fn push(platform: &Arc<Platform>, id: ShortId, i: u32) -> anyhow::Result<u64> {
    let length = cli(platform, id, &format!("RPUSH t {i}")).await?;
    ensure!(length.parse::<u32>().is_ok(), "RPUSH answered {length}");
    let time = cli(platform, id, "TIME").await?;
    let mut parts = time.lines();
    let secs: u64 = parts.next().context("TIME")?.parse()?;
    let micros: u64 = parts.next().context("TIME")?.parse()?;
    Ok(secs * 1000 + micros / 1000)
}

/// A running database with the base backup of its first archive call and then items 1 to 6
/// written 1.2 seconds apart; the target time is one second after item 3.
async fn six_items(platform: &Arc<Platform>, name: &str) -> anyhow::Result<(AppEntry, u64)> {
    let db = add_db(platform, name).await?;
    deploy(platform, db.id).await?;
    platform.archive_wal(db.id).await?;
    let mut third = 0;
    for i in 1..=6 {
        let at = push(platform, db.id, i).await?;
        if i == 3 {
            third = at;
        }
        tokio::time::sleep(Duration::from_millis(1200)).await;
    }
    Ok((db, third + 1000))
}

async fn branch_items(
    platform: &Arc<Platform>,
    id: ShortId,
    name: &str,
    target_time: u64,
) -> anyhow::Result<String> {
    let request = PitrRestoreRequest {
        target_time,
        new_name: Some(name.parse()?),
        host_port: None,
    };
    let response = platform.restore_pitr(id, request).await?;
    ensure!(response.branched, "{response:?}");
    items(platform, response.app.id).await
}

async fn recover_branch(platform: &Arc<Platform>, name: &str) -> anyhow::Result<()> {
    let (db, target_time) = six_items(platform, name).await?;
    let branch = branch_items(platform, db.id, &format!("{name}-b"), target_time).await?;
    ensure!(branch == "1,2,3", "the branch holds {branch}");
    let source = items(platform, db.id).await?;
    ensure!(source == "1,2,3,4,5,6", "the source holds {source}");
    Ok(())
}

async fn recover_in_place(platform: &Arc<Platform>, name: &str) -> anyhow::Result<()> {
    let (db, target_time) = six_items(platform, name).await?;
    let fifth_target = target_time + 2400;
    let request = PitrRestoreRequest {
        target_time,
        new_name: None,
        host_port: None,
    };
    let response = platform.restore_pitr(db.id, request).await?;
    ensure!(!response.branched, "{response:?}");
    let now = items(platform, db.id).await?;
    ensure!(now == "1,2,3", "the database holds {now}");
    // The recovered database takes writes, and its new history is archived again.
    let seventh = push(platform, db.id, 7).await?;
    tokio::time::sleep(Duration::from_millis(1200)).await;
    platform.archive_wal(db.id).await?;
    let old = branch_items(platform, db.id, &format!("{name}-o"), fifth_target).await?;
    ensure!(
        old == "1,2,3,4,5",
        "the branch of the old history holds {old}"
    );
    let new = branch_items(platform, db.id, &format!("{name}-n"), seventh + 1000).await?;
    ensure!(
        new == "1,2,3,7",
        "the branch of the new history holds {new}"
    );
    Ok(())
}

/// A target after the base backup but before the first write of its incr file recovers the
/// data of the base backup alone.
async fn recover_an_idle_target(platform: &Arc<Platform>, name: &str) -> anyhow::Result<()> {
    let db = add_db(platform, name).await?;
    deploy(platform, db.id).await?;
    push(platform, db.id, 1).await?;
    platform.archive_wal(db.id).await?;
    tokio::time::sleep(Duration::from_millis(2500)).await;
    let target = push(platform, db.id, 2).await? - 1000;
    tokio::time::sleep(Duration::from_millis(1200)).await;
    let branch = branch_items(platform, db.id, &format!("{name}-i"), target).await?;
    ensure!(branch == "1", "the branch holds {branch}");
    Ok(())
}

/// A recovery that misses one archived AOF piece fails, names the piece and leaves the data
/// of the database as it was.
async fn refuse_a_missing_piece(platform: &Arc<Platform>, name: &str) -> anyhow::Result<()> {
    let (db, target_time) = six_items(platform, name).await?;
    let lost = platform.archive_wal(db.id).await?;
    let dir = platform.wal_dir(db.id).await?;
    tokio::fs::remove_file(dir.join(format!("{}.wal.gz", lost.id))).await?;
    let request = PitrRestoreRequest {
        target_time,
        new_name: None,
        host_port: None,
    };
    let err = platform.restore_pitr(db.id, request).await.unwrap_err();
    ensure!(
        format!("{err:#}").contains(&lost.filename),
        "the error does not name {}: {err:#}",
        lost.filename
    );
    let now = items(platform, db.id).await?;
    ensure!(now == "1,2,3,4,5,6", "the database holds {now}");
    Ok(())
}

/// A recovery whose AOF cannot be loaded, here a damaged piece, puts the data that it moved
/// aside back, and the database takes writes again.
async fn roll_back_a_damaged_piece(platform: &Arc<Platform>, name: &str) -> anyhow::Result<()> {
    let (db, target_time) = six_items(platform, name).await?;
    let damaged = platform.archive_wal(db.id).await?;
    let dir = platform.wal_dir(db.id).await?;
    let broken = super::backup::gzip(b"*3\r\n$3\r\nSET\r\nbroken").await?;
    tokio::fs::write(dir.join(format!("{}.wal.gz", damaged.id)), broken).await?;
    let request = PitrRestoreRequest {
        target_time,
        new_name: None,
        host_port: None,
    };
    let err = platform.restore_pitr(db.id, request).await.unwrap_err();
    ensure!(format!("{err:#}").contains("restore failed"), "{err:#}");
    let now = items(platform, db.id).await?;
    ensure!(now == "1,2,3,4,5,6", "the database holds {now}");
    push(platform, db.id, 7).await?;
    Ok(())
}

/// A rewrite that the platform did not make replaces the incr file, and the archive misses
/// what the old file held after the last pull. A target before the next pull is refused, and
/// a target after it recovers from the new base backup.
async fn refuse_a_target_in_a_gap(platform: &Arc<Platform>, name: &str) -> anyhow::Result<()> {
    let (db, target_time) = six_items(platform, name).await?;
    cli(platform, db.id, "BGREWRITEAOF").await?;
    tokio::time::sleep(Duration::from_secs(2)).await;
    platform.pull_wal(db.id).await?;
    let seventh = push(platform, db.id, 7).await?;
    tokio::time::sleep(Duration::from_millis(1200)).await;
    let request = PitrRestoreRequest {
        target_time,
        new_name: Some(format!("{name}-g").parse()?),
        host_port: None,
    };
    let refused = platform.restore_pitr(db.id, request).await;
    ensure!(
        refused.is_err(),
        "a target in the gap was served: {refused:?}"
    );
    let after = branch_items(platform, db.id, &format!("{name}-a"), seventh + 1000).await?;
    ensure!(
        after == "1,2,3,4,5,6,7",
        "the branch after the gap holds {after}"
    );
    Ok(())
}

/// A plain Redis on the volume of `name`, as the platform started every Redis before: it saves
/// `dump.rdb` and has no AOF.
fn plain_spec(container: &ContainerName, name: &str) -> anyhow::Result<ContainerSpec> {
    Ok(ContainerSpec {
        name: container.clone(),
        image: IMAGE.parse()?,
        env: Vec::<EnvVar>::new(),
        labels: BTreeMap::new(),
        network: None,
        volumes: vec![data_volume(name)?],
        restart: RestartPolicy::No,
        limits: ResourceLimits::default(),
        publish: None,
        host_port: None,
        aliases: Vec::new(),
        extra_networks: Vec::new(),
        log_max_size: None,
        log_max_file: None,
        command: None,
    })
}

/// Writes the key `old` with a plain Redis and saves it to `dump.rdb`.
async fn plain_with_a_key(platform: &Arc<Platform>, name: &str) -> anyhow::Result<()> {
    let runtime = platform.local.clone();
    let container: ContainerName = format!("{name}-plain").parse()?;
    let spec = plain_spec(&container, name)?;
    // A fresh Docker Engine, such as the one of CI, has no image yet.
    runtime.pull_image(&spec.image).await?;
    runtime.create_container(&spec).await?;
    runtime.start_container(&container).await?;
    let script = "until redis-cli PING | grep -q PONG; do sleep 0.2; done; \
redis-cli SET old 1 && redis-cli SAVE";
    let command = vec!["sh".into(), "-c".into(), script.to_string()];
    let saved = tokio::time::timeout(
        Duration::from_secs(60),
        runtime.exec_capture(&container, command, Vec::new()),
    )
    .await;
    runtime.remove_container(&container).await?;
    let out = String::from_utf8_lossy(&saved??).to_string();
    ensure!(out.contains("OK"), "the plain Redis did not save: {out}");
    Ok(())
}

/// The data of a Redis without the AOF stays when the next deployment turns the AOF on, and
/// stays again on the deployment after it, which loads the AOF.
async fn convert(platform: &Arc<Platform>, name: &str) -> anyhow::Result<()> {
    let db = add_db(platform, name).await?;
    plain_with_a_key(platform, name).await?;
    for round in 1..=2 {
        deploy(platform, db.id).await?;
        let old = cli(platform, db.id, "GET old").await?;
        ensure!(old == "1", "deployment {round} lost the key: {old:?}");
        let info = cli(platform, db.id, "INFO persistence").await?;
        ensure!(
            info.lines().any(|line| line.trim() == "aof_enabled:1"),
            "deployment {round} runs without the AOF"
        );
    }
    Ok(())
}

/// A backup of a Redis dataset over 64 MiB is taken: the single download of the runtime refuses
/// more than 64 MiB. Random data keeps the compression of the RDB file from shrinking it.
async fn back_up_a_large_dataset(platform: &Arc<Platform>, name: &str) -> anyhow::Result<()> {
    let db = add_db(platform, name).await?;
    deploy(platform, db.id).await?;
    let (_, container, runtime) = platform.database_container(db.id).await?;
    let fill = "for i in $(seq 1 13); do head -c 4194304 /dev/urandom | base64 -w 0 \
| redis-cli -x SET k$i > /dev/null || exit 1; done";
    let command = vec!["sh".into(), "-c".into(), fill.into()];
    let env = vec![format!("REDISCLI_AUTH={PASSWORD}")];
    runtime.exec_capture(&container, command, env).await?;
    let backup = platform.run_backup(db.id).await?;
    // The compressed backup alone holds more than the 16 MiB that an upload takes at once.
    ensure!(backup.size > 16 << 20, "{backup:?}");
    Ok(())
}

/// A base backup and a recovery of a database larger than one transfer piece pass in bounded
/// pieces: a server with hundreds of megabytes must not be held in the memory of the panel,
/// which can be smaller than the database. Random data keeps the base larger than one piece
/// after compression too.
async fn recover_a_large_base(
    platform: &Arc<Platform>,
    bounded: &super::bounded_runtime::BoundedRuntime,
    name: &str,
) -> anyhow::Result<()> {
    let db = add_db(platform, name).await?;
    deploy(platform, db.id).await?;
    let (_, container, runtime) = platform.database_container(db.id).await?;
    let fill = "for i in $(seq 1 13); do head -c 4194304 /dev/urandom | base64 -w 0 \
| redis-cli -x SET k$i > /dev/null || exit 1; done";
    let command = vec!["sh".into(), "-c".into(), fill.into()];
    let env = vec![format!("REDISCLI_AUTH={PASSWORD}")];
    runtime.exec_capture(&container, command, env).await?;
    platform.archive_wal(db.id).await?;
    let mut third = 0;
    for i in 1..=6 {
        let at = push(platform, db.id, i).await?;
        if i == 3 {
            third = at;
        }
        tokio::time::sleep(Duration::from_millis(1200)).await;
    }
    let branch = branch_items(platform, db.id, &format!("{name}-b"), third + 1000).await?;
    ensure!(branch == "1,2,3", "the branch holds {branch}");
    let violations = bounded.violations();
    ensure!(
        violations.is_empty(),
        "whole transfers in memory: {violations:?}"
    );
    Ok(())
}

#[tokio::test]
#[ignore = "needs a Docker Engine; run with make test-runtime-docker"]
async fn redis_recovers_a_large_base_in_pieces_on_the_docker_engine() -> anyhow::Result<()> {
    catalog();
    let (command, _) = mpsc::channel(1);
    let (mut platform, _fake, _dir) = platform_with(&PlatformConfig::default(), command).await?;
    let host = std::env::var("DOCKER_HOST").unwrap_or("unix:///var/run/docker.sock".into());
    let bounded = Arc::new(super::bounded_runtime::BoundedRuntime::new(Arc::new(
        super::docker_runtime(&host)?,
    )));
    platform.local = bounded.clone();
    platform.timing.health_timeout = Duration::from_secs(120);
    let platform = Arc::new(platform);
    let run = recover_a_large_base(&platform, &bounded, &name()).await;
    run.and(remove_apps(&platform).await)
}

/// Every key of the database, sorted.
async fn sorted_keys(platform: &Arc<Platform>, id: ShortId) -> anyhow::Result<Vec<String>> {
    let out = cli(platform, id, "--scan").await?;
    let mut keys: Vec<String> = out.lines().map(str::to_string).collect();
    keys.sort_unstable();
    Ok(keys)
}

/// Writes the keys `a` and `b`, backs them up, then replaces them with the key `late`, and
/// returns the id of the backup.
async fn back_up_then_replace(platform: &Arc<Platform>, id: ShortId) -> anyhow::Result<String> {
    cli(platform, id, "SET a 1").await?;
    cli(platform, id, "SET b 2").await?;
    let backup = platform.run_backup(id).await?;
    cli(platform, id, "FLUSHALL").await?;
    cli(platform, id, "SET late 3").await?;
    Ok(backup.id)
}

/// A backup restores on a Redis that writes its AOF, and the archive keeps working after it.
/// The server loads the AOF when one exists, so a restore that only writes `dump.rdb` left the
/// database as it was before (measured: 0 keys after a restore of 2).
async fn restore_on_the_aof(platform: &Arc<Platform>, name: &str) -> anyhow::Result<()> {
    let db = add_db(platform, name).await?;
    deploy(platform, db.id).await?;
    let backup = back_up_then_replace(platform, db.id).await?;
    platform.restore_backup(db.id, &backup).await?;
    let keys = sorted_keys(platform, db.id).await?;
    ensure!(keys == ["a", "b"], "the database holds {keys:?}");
    let aof = cli(platform, db.id, "CONFIG GET appendonly").await?;
    ensure!(
        aof.ends_with("yes"),
        "the AOF is off after the restore: {aof}"
    );
    branch_after_restore(platform, db.id, name).await
}

/// The archive follows the restored data: a write after the restore reaches a branch.
async fn branch_after_restore(
    platform: &Arc<Platform>,
    id: ShortId,
    name: &str,
) -> anyhow::Result<()> {
    let db = platform.app(id).await?;
    cli(platform, db.id, "SET c 3").await?;
    tokio::time::sleep(Duration::from_millis(1200)).await;
    let request = PitrRestoreRequest {
        target_time: crate::clock::unix_ms(),
        new_name: Some(format!("{name}-b").parse()?),
        host_port: None,
    };
    let branch = platform.restore_pitr(db.id, request).await?;
    let count = cli(platform, branch.app.id, "DBSIZE").await?;
    ensure!(count == "3", "the branch holds {count} keys");
    Ok(())
}

/// A clone of a Redis that writes its AOF gets the data of the source and writes its own AOF
/// again: the restore sets the AOF aside for its restart, so a clone that kept it off would
/// have no log to archive.
async fn clone_on_the_aof(platform: &Arc<Platform>, name: &str) -> anyhow::Result<()> {
    let source = add_db(platform, name).await?;
    deploy(platform, source.id).await?;
    cli(platform, source.id, "SET a 1").await?;
    cli(platform, source.id, "SET b 2").await?;
    let dest = add_db(platform, &format!("{name}-c")).await?;
    deploy(platform, dest.id).await?;
    platform
        .restore_clone_data(source.id, dest.id, PASSWORD)
        .await?;
    wait_for_keys(platform, dest.id, &["a", "b"]).await?;
    wait_for_aof(platform, dest.id).await
}

/// Waits up to 90 s until the database writes its AOF. A clone restores in the background and
/// turns the AOF on after the data is back, so the keys can show up before the AOF does.
async fn wait_for_aof(platform: &Arc<Platform>, id: ShortId) -> anyhow::Result<()> {
    let deadline = tokio::time::Instant::now() + Duration::from_secs(90);
    loop {
        let aof = cli(platform, id, "CONFIG GET appendonly").await?;
        if aof.ends_with("yes") {
            return Ok(());
        }
        ensure!(
            tokio::time::Instant::now() < deadline,
            "the clone runs without the AOF: {aof}"
        );
        tokio::time::sleep(Duration::from_millis(500)).await;
    }
}

/// Waits up to 90 s until the database holds exactly `expected`.
async fn wait_for_keys(
    platform: &Arc<Platform>,
    id: ShortId,
    expected: &[&str],
) -> anyhow::Result<()> {
    let deadline = tokio::time::Instant::now() + Duration::from_secs(90);
    loop {
        if let Ok(keys) = sorted_keys(platform, id).await
            && keys == expected
        {
            return Ok(());
        }
        ensure!(
            tokio::time::Instant::now() < deadline,
            "the keys of {id} never became {expected:?}"
        );
        tokio::time::sleep(Duration::from_millis(500)).await;
    }
}

/// A snapshot from S3 restores into a Redis that writes its AOF, and the server writes its AOF
/// again: the step after the download is the same as a local restore.
async fn s3_restore_on_the_aof(platform: &Arc<Platform>, name: &str) -> anyhow::Result<()> {
    let db = add_db(platform, name).await?;
    deploy(platform, db.id).await?;
    let backup = back_up_then_replace(platform, db.id).await?;
    let compressed = platform.download_backup(db.id, &backup).await?;
    let tmp = std::env::temp_dir();
    let compressed = super::backup::DumpFile::from_bytes(&compressed, &tmp).await?;
    let dump = super::backup::DumpFile::gunzip(&compressed.path, &tmp).await?;
    platform
        .restore_s3_into_existing(db.id, dump, Some("redis"))
        .await?;
    let keys = sorted_keys(platform, db.id).await?;
    ensure!(keys == ["a", "b"], "the database holds {keys:?}");
    let aof = cli(platform, db.id, "CONFIG GET appendonly").await?;
    ensure!(
        aof.ends_with("yes"),
        "the AOF is off after the restore: {aof}"
    );
    Ok(())
}

#[tokio::test]
#[ignore = "needs a Docker Engine; run with make test-runtime-docker"]
async fn a_redis_s3_restore_writes_its_aof_on_the_docker_engine() -> anyhow::Result<()> {
    catalog();
    let (platform, _dir) = docker_platform().await?;
    let run = s3_restore_on_the_aof(&platform, &name()).await;
    run.and(remove_apps(&platform).await)
}

#[tokio::test]
#[ignore = "needs a Docker Engine; run with make test-runtime-docker"]
async fn a_redis_clone_writes_its_aof_on_the_docker_engine() -> anyhow::Result<()> {
    catalog();
    let (platform, _dir) = docker_platform().await?;
    let run = clone_on_the_aof(&platform, &name()).await;
    run.and(remove_apps(&platform).await)
}

/// A command that prints nothing for longer than the 30 s response timeout of the Docker
/// client still runs to its end: a large dump written to a file stays silent for minutes.
async fn wait_out_a_silent_command(platform: &Arc<Platform>, name: &str) -> anyhow::Result<()> {
    let db = add_db(platform, name).await?;
    deploy(platform, db.id).await?;
    let (_, container, runtime) = platform.database_container(db.id).await?;
    let command = vec!["sh".into(), "-c".into(), "sleep 35; echo done".into()];
    let out = runtime
        .exec_capture(&container, command, Vec::new())
        .await?;
    ensure!(out == b"done\n", "the command returned {out:?}");
    Ok(())
}

#[tokio::test]
#[ignore = "needs a Docker Engine; run with make test-runtime-docker"]
async fn a_command_silent_past_the_response_timeout_finishes_on_the_docker_engine()
-> anyhow::Result<()> {
    catalog();
    let (platform, _dir) = docker_platform().await?;
    let run = wait_out_a_silent_command(&platform, &name()).await;
    run.and(remove_apps(&platform).await)
}

/// A command that runs past its limit is an error that returns at the limit: the caller holds the
/// lock of the database while it waits, so a hanging command must not keep it for ever.
async fn give_up_on_a_hanging_command(platform: &Arc<Platform>, name: &str) -> anyhow::Result<()> {
    let db = add_db(platform, name).await?;
    deploy(platform, db.id).await?;
    let (_, container, _) = platform.database_container(db.id).await?;
    let host = std::env::var("DOCKER_HOST").unwrap_or("unix:///var/run/docker.sock".into());
    let runtime = super::docker_runtime(&host)?;
    let command = vec!["sh".into(), "-c".into(), "sleep 60; echo done".into()];
    let started = tokio::time::Instant::now();
    let result = runtime
        .exec_capture_within(&container, command, Vec::new(), Duration::from_secs(3))
        .await;
    let took = started.elapsed();
    ensure!(result.is_err(), "the hanging command returned {result:?}");
    ensure!(
        took < Duration::from_secs(15),
        "the limit ended the wait after {took:?}"
    );
    // The command itself ends too: a dump that runs on after its caller gave up would write
    // while the next job already holds the lock.
    let list = "for p in /proc/[0-9]*; do tr '\\0' ' ' < $p/cmdline 2>/dev/null; echo; done";
    let check = vec!["sh".into(), "-c".into(), list.into()];
    let processes = runtime.exec_capture(&container, check, Vec::new()).await?;
    let processes = String::from_utf8_lossy(&processes);
    ensure!(
        !processes.lines().any(|line| line.starts_with("sleep 60")),
        "the command still runs after its limit:\n{processes}"
    );
    Ok(())
}

#[tokio::test]
#[ignore = "needs a Docker Engine; run with make test-runtime-docker"]
async fn a_command_past_its_limit_fails_on_the_docker_engine() -> anyhow::Result<()> {
    catalog();
    let (platform, _dir) = docker_platform().await?;
    let run = give_up_on_a_hanging_command(&platform, &name()).await;
    run.and(remove_apps(&platform).await)
}

#[tokio::test]
#[ignore = "needs a Docker Engine; run with make test-runtime-docker"]
async fn redis_restores_a_backup_on_the_aof_on_the_docker_engine() -> anyhow::Result<()> {
    catalog();
    let (platform, _dir) = docker_platform().await?;
    let run = restore_on_the_aof(&platform, &name()).await;
    run.and(remove_apps(&platform).await)
}

#[tokio::test]
#[ignore = "needs a Docker Engine; run with make test-runtime-docker"]
async fn redis_backs_up_a_dataset_over_64_mib_on_the_docker_engine() -> anyhow::Result<()> {
    catalog();
    let (platform, _dir) = docker_platform().await?;
    let run = back_up_a_large_dataset(&platform, &name()).await;
    run.and(remove_apps(&platform).await)
}

async fn remove_apps(platform: &Arc<Platform>) -> anyhow::Result<()> {
    for app in platform.store.apps().await? {
        platform.delete_app(app.id).await?;
    }
    Ok(())
}

fn name() -> String {
    format!("rds{}", hex::encode(rand::random::<[u8; 3]>()))
}

#[tokio::test]
#[ignore = "needs a Docker Engine; run with make test-runtime-docker"]
async fn a_redis_without_the_aof_keeps_its_data_on_the_docker_engine() -> anyhow::Result<()> {
    catalog();
    let (platform, _dir) = docker_platform().await?;
    let run = convert(&platform, &name()).await;
    run.and(remove_apps(&platform).await)
}

#[tokio::test]
#[ignore = "needs a Docker Engine; run with make test-runtime-docker"]
async fn redis_recovers_a_branch_to_a_second_on_the_docker_engine() -> anyhow::Result<()> {
    catalog();
    let (platform, _dir) = docker_platform().await?;
    let run = recover_branch(&platform, &name()).await;
    run.and(remove_apps(&platform).await)
}

#[tokio::test]
#[ignore = "needs a Docker Engine; run with make test-runtime-docker"]
async fn redis_recovers_in_place_on_the_docker_engine() -> anyhow::Result<()> {
    catalog();
    let (platform, _dir) = docker_platform().await?;
    let run = recover_in_place(&platform, &name()).await;
    run.and(remove_apps(&platform).await)
}

#[tokio::test]
#[ignore = "needs a Docker Engine; run with make test-runtime-docker"]
async fn redis_recovers_a_target_before_the_first_write_on_the_docker_engine() -> anyhow::Result<()>
{
    catalog();
    let (platform, _dir) = docker_platform().await?;
    let run = recover_an_idle_target(&platform, &name()).await;
    run.and(remove_apps(&platform).await)
}

#[tokio::test]
#[ignore = "needs a Docker Engine; run with make test-runtime-docker"]
async fn redis_keeps_its_data_when_an_aof_piece_is_missing_on_the_docker_engine()
-> anyhow::Result<()> {
    catalog();
    let (platform, _dir) = docker_platform().await?;
    let run = refuse_a_missing_piece(&platform, &name()).await;
    run.and(remove_apps(&platform).await)
}

#[tokio::test]
#[ignore = "needs a Docker Engine; run with make test-runtime-docker"]
async fn redis_puts_its_data_back_when_the_aof_is_damaged_on_the_docker_engine()
-> anyhow::Result<()> {
    catalog();
    let (platform, _dir) = docker_platform().await?;
    let run = roll_back_a_damaged_piece(&platform, &name()).await;
    run.and(remove_apps(&platform).await)
}

#[tokio::test]
#[ignore = "needs a Docker Engine; run with make test-runtime-docker"]
async fn redis_refuses_a_target_in_an_aof_gap_on_the_docker_engine() -> anyhow::Result<()> {
    catalog();
    let (platform, _dir) = docker_platform().await?;
    let run = refuse_a_target_in_a_gap(&platform, &name()).await;
    run.and(remove_apps(&platform).await)
}
