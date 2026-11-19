//! Point-in-time recovery of PostgreSQL on a Docker Engine: the server archives its WAL, the
//! platform pulls it and takes a base backup, and a recovery to a time between two commits
//! brings back exactly the commits before that time (PostgreSQL manual, "Continuous
//! Archiving and Point-in-Time Recovery", `recovery_target_time`).
//!
//! The tests need a Docker Engine (`DOCKER_HOST`, else `unix:///var/run/docker.sock`).
//! `make test-runtime-docker` runs them.

use super::Platform;
use super::tests::{TempDir, platform_with, request};
use anyhow::{Context as _, ensure};
use r3v3rs3_api::container::{ContainerSpec, EnvVar, ResourceLimits, RestartPolicy, VolumeMount};
use r3v3rs3_api::id::ShortId;
use r3v3rs3_api::platform::{
    AppEntry, AppSource, DeploymentTrigger, EnvEntry, PitrRestoreRequest, PlatformConfig,
};
use std::collections::BTreeMap;
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::mpsc;

const PASSWORD: &str = "s3cret0123";

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
    platform.timing.health_timeout = Duration::from_secs(60);
    Ok((Arc::new(platform), dir))
}

/// The image tag of the tests; PostgreSQL 18 mounts its volume above the data directory.
const PG17: &str = "17-alpine";

/// Adds a PostgreSQL database with its own data volume at the path of the catalog and
/// deploys it until it runs.
async fn running_postgres(
    platform: &Arc<Platform>,
    name: &str,
    tag: &str,
) -> anyhow::Result<AppEntry> {
    let info = r3v3rs3_api::platform::engine_info("postgres").context("the catalog")?;
    let mut req = request(name);
    req.spec.source = AppSource::Image {
        image: format!("mirror.gcr.io/library/postgres:{tag}").parse()?,
    };
    req.spec.port = 5432;
    req.spec.database = Some("postgres".into());
    req.spec.volumes = vec![VolumeMount {
        volume: format!("{name}-data").parse()?,
        target: info.data_path_for(&format!("postgres:{tag}")).parse()?,
        read_only: false,
    }];
    let app = platform.add_app(req, 1).await?;
    let password = EnvEntry {
        key: "POSTGRES_PASSWORD".parse()?,
        value: Some(PASSWORD.into()),
        secret: true,
    };
    platform.set_env(app.id, vec![password]).await?;
    platform
        .deploy(app.id, "alice", DeploymentTrigger::Manual)
        .await?;
    wait_running(platform, app.id).await?;
    Ok(app)
}

async fn wait_running(platform: &Arc<Platform>, id: ShortId) -> anyhow::Result<()> {
    let deadline = tokio::time::Instant::now() + Duration::from_secs(180);
    while platform.database_container(id).await.is_err() || platform.busy_apps().contains(&id) {
        ensure!(tokio::time::Instant::now() < deadline, "{id} did not run");
        tokio::time::sleep(Duration::from_millis(250)).await;
    }
    Ok(())
}

/// The answer of one query in the container of the database.
async fn sql(platform: &Arc<Platform>, id: ShortId, query: &str) -> anyhow::Result<String> {
    let (_, container, runtime) = platform.database_container(id).await?;
    let command = ["psql", "-U", "postgres", "-tAc", query]
        .map(str::to_string)
        .to_vec();
    let out = runtime
        .exec_capture(&container, command, vec![format!("PGPASSWORD={PASSWORD}")])
        .await?;
    Ok(String::from_utf8_lossy(&out).trim().to_string())
}

/// Commits six rows about a second apart and returns their commit times in unix ms.
async fn commit_six_rows(platform: &Arc<Platform>, id: ShortId) -> anyhow::Result<Vec<u64>> {
    let insert = "INSERT INTO t (i) SELECT coalesce(max(i), 0) + 1 FROM t";
    for _ in 1..=6 {
        sql(platform, id, insert).await?;
        tokio::time::sleep(Duration::from_millis(1200)).await;
    }
    let times = "SELECT (extract(epoch FROM at) * 1000)::bigint FROM t ORDER BY i";
    let commits: Vec<u64> = sql(platform, id, times)
        .await?
        .lines()
        .map(str::parse)
        .collect::<Result<_, _>>()?;
    ensure!(commits.len() == 6, "{commits:?}");
    Ok(commits)
}

const ROWS: &str = "SELECT string_agg(i::text, ',' ORDER BY i) FROM t";

/// A running database with six rows committed about a second apart after its first archive
/// call, and the time between the commits of rows 3 and 4.
async fn six_rows(
    platform: &Arc<Platform>,
    name: &str,
    tag: &str,
) -> anyhow::Result<(AppEntry, u64)> {
    let db = running_postgres(platform, name, tag).await?;
    let table = "CREATE TABLE t (i int, at timestamptz DEFAULT clock_timestamp())";
    sql(platform, db.id, table).await?;
    // The first archive call takes the base backup that the recovery starts from.
    platform.archive_wal(db.id).await?;
    let commits = commit_six_rows(platform, db.id).await?;
    let (third, fourth) = (commits[2], commits[3]);
    Ok((db, third + (fourth - third) / 2))
}

/// Recovers a branch to the middle between rows 3 and 4.
async fn recover_between_two_rows(
    platform: &Arc<Platform>,
    name: &str,
    tag: &str,
) -> anyhow::Result<()> {
    let (db, target_time) = six_rows(platform, name, tag).await?;
    let request = PitrRestoreRequest {
        target_time,
        new_name: Some(format!("{name}-branch").parse()?),
        host_port: None,
    };
    let response = platform.restore_pitr(db.id, request).await?;
    ensure!(response.branched, "{response:?}");
    let branch = sql(platform, response.app.id, ROWS).await?;
    ensure!(branch == "1,2,3", "the branch holds the rows {branch}");
    // The source keeps every row.
    let source = sql(platform, db.id, ROWS).await?;
    ensure!(
        source == "1,2,3,4,5,6",
        "the source holds the rows {source}"
    );
    Ok(())
}

/// Recovers the database itself to the middle between rows 3 and 4.
async fn recover_in_place(platform: &Arc<Platform>, name: &str) -> anyhow::Result<()> {
    let (db, target_time) = six_rows(platform, name, PG17).await?;
    let fifth = row_time(platform, db.id, 5).await?;
    let request = PitrRestoreRequest {
        target_time,
        new_name: None,
        host_port: None,
    };
    let response = platform.restore_pitr(db.id, request).await?;
    ensure!(!response.branched, "{response:?}");
    let rows = sql(platform, db.id, ROWS).await?;
    ensure!(rows == "1,2,3", "the database holds the rows {rows}");
    // The promoted server takes new writes, and its new timeline is archived.
    sql(platform, db.id, "INSERT INTO t (i) VALUES (7)").await?;
    let seventh = row_time(platform, db.id, 7).await?;
    tokio::time::sleep(Duration::from_millis(1200)).await;
    platform.archive_wal(db.id).await?;
    // Both timelines stay reachable: a target before the recovery takes the old one.
    let old = branch_rows(platform, db.id, &format!("{name}-o"), fifth + 500).await?;
    ensure!(
        old == "1,2,3,4,5",
        "the branch of the old timeline holds {old}"
    );
    let new = branch_rows(platform, db.id, &format!("{name}-n"), seventh + 500).await?;
    ensure!(
        new == "1,2,3,7",
        "the branch of the new timeline holds {new}"
    );
    Ok(())
}

/// The commit time of row `i` in Unix milliseconds.
async fn row_time(platform: &Arc<Platform>, id: ShortId, i: u32) -> anyhow::Result<u64> {
    let query = "SELECT i || ' ' || (extract(epoch FROM at) * 1000)::bigint FROM t";
    let rows = sql(platform, id, query).await?;
    let time = rows
        .lines()
        .find_map(|row| row.strip_prefix(&format!("{i} ")))
        .with_context(|| format!("no row {i} in {rows:?}"))?;
    Ok(time.parse()?)
}

/// The rows of a new branch of `id` recovered to `target_time`.
async fn branch_rows(
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
    sql(platform, response.app.id, ROWS).await
}

/// A recovery that misses one archived WAL file fails, names the file and leaves the data of
/// the database as it was.
async fn refuse_a_gap_in_the_archive(platform: &Arc<Platform>, name: &str) -> anyhow::Result<()> {
    let (db, target_time) = six_rows(platform, name, PG17).await?;
    // The rows sit in the current WAL file; closing it puts them into the archive.
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
    let rows = sql(platform, db.id, ROWS).await?;
    ensure!(rows == "1,2,3,4,5,6", "the database holds the rows {rows}");
    Ok(())
}

/// A target in an idle period after the last commit lies inside the archive: the recovery
/// keeps every commit and the server answers afterwards. PostgreSQL stops a recovery only on
/// a commit record, and ends with FATAL when the WAL runs out before the target.
async fn recover_into_an_idle_period(platform: &Arc<Platform>, name: &str) -> anyhow::Result<()> {
    let db = running_postgres(platform, name, PG17).await?;
    let table = "CREATE TABLE t (i int, at timestamptz DEFAULT clock_timestamp())";
    sql(platform, db.id, table).await?;
    platform.archive_wal(db.id).await?;
    let commits = commit_six_rows(platform, db.id).await?;
    tokio::time::sleep(Duration::from_secs(3)).await;
    let request = PitrRestoreRequest {
        target_time: commits[5] + 1000,
        new_name: None,
        host_port: None,
    };
    platform.restore_pitr(db.id, request).await?;
    let rows = sql(platform, db.id, ROWS).await?;
    ensure!(rows == "1,2,3,4,5,6", "the database holds the rows {rows}");
    Ok(())
}

/// A recovery whose replay fails half way, here on a damaged WAL file, puts the data that
/// it moved aside back, and the server answers and takes writes again.
async fn roll_back_a_failed_replay(platform: &Arc<Platform>, name: &str) -> anyhow::Result<()> {
    let (db, target_time) = six_rows(platform, name, PG17).await?;
    let damaged = platform.archive_wal(db.id).await?;
    let dir = platform.wal_dir(db.id).await?;
    let broken = super::backup::gzip(b"not a WAL file").await?;
    tokio::fs::write(dir.join(format!("{}.wal.gz", damaged.id)), broken).await?;
    let request = PitrRestoreRequest {
        target_time,
        new_name: None,
        host_port: None,
    };
    let err = platform.restore_pitr(db.id, request).await.unwrap_err();
    ensure!(format!("{err:#}").contains("FATAL"), "{err:#}");
    let rows = sql(platform, db.id, ROWS).await?;
    ensure!(rows == "1,2,3,4,5,6", "the database holds the rows {rows}");
    sql(platform, db.id, "INSERT INTO t (i) VALUES (7)").await?;
    Ok(())
}

/// The container `<name>-plain` of `image` on the volume `<name>-data` at `mount`, without a
/// start command.
fn plain_spec(
    name: &str,
    (image, mount): (&str, &str),
    env: Vec<EnvVar>,
) -> anyhow::Result<ContainerSpec> {
    Ok(ContainerSpec {
        name: format!("{name}-plain").parse()?,
        image: image.parse()?,
        env,
        labels: BTreeMap::new(),
        network: None,
        volumes: vec![VolumeMount {
            volume: format!("{name}-data").parse()?,
            target: mount.parse()?,
            read_only: false,
        }],
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

/// Runs `image` without a start command on the volume `<name>-data` at `mount`, as the platform
/// started every database before it archived the log, runs `script` in it until it succeeds,
/// and stops and removes the container again.
pub(super) async fn plain_run(
    platform: &Arc<Platform>,
    name: &str,
    (image, mount): (&str, &str),
    env: Vec<EnvVar>,
    script: &str,
) -> anyhow::Result<()> {
    let runtime = platform.local.clone();
    let spec = plain_spec(name, (image, mount), env)?;
    let container = spec.name.clone();
    // A fresh Docker Engine, such as the one of CI, has no image yet.
    runtime.pull_image(&spec.image).await?;
    runtime.create_container(&spec).await?;
    runtime.start_container(&container).await?;
    let command = vec![
        "sh".into(),
        "-c".into(),
        format!("until {script}; do sleep 1; done"),
    ];
    let ran = tokio::time::timeout(
        Duration::from_secs(180),
        runtime.exec_capture(&container, command, Vec::new()),
    )
    .await;
    let stopped = runtime
        .stop_container(&container, Duration::from_secs(60))
        .await;
    runtime.remove_container(&container).await?;
    ran.context("the plain database did not take the script in time")??;
    stopped
}

/// Writes row 1 with a plain PostgreSQL on the volume of `name`. The temporary server of the
/// first start listens on no TCP port, so the script waits for the final one.
async fn seed_plain_postgres(
    platform: &Arc<Platform>,
    name: &str,
    tag: &str,
) -> anyhow::Result<()> {
    let info = r3v3rs3_api::platform::engine_info("postgres").context("the catalog")?;
    let image = format!("mirror.gcr.io/library/postgres:{tag}");
    let mount = info.data_path_for(&format!("postgres:{tag}"));
    let env = ["POSTGRES_PASSWORD", "PGPASSWORD"]
        .into_iter()
        .map(|key| {
            Ok(EnvVar {
                key: key.parse()?,
                value: PASSWORD.into(),
            })
        })
        .collect::<anyhow::Result<Vec<_>>>()?;
    let script = "psql -h 127.0.0.1 -U postgres -c \
'CREATE TABLE t AS SELECT 1 AS i, clock_timestamp() AS at'";
    plain_run(platform, name, (&image, mount), env, script).await
}

/// A PostgreSQL that ran without the WAL archive keeps its data when the next deployment turns
/// the archive on, and the archive then covers the writes after it.
async fn convert(platform: &Arc<Platform>, name: &str, tag: &str) -> anyhow::Result<()> {
    seed_plain_postgres(platform, name, tag).await?;
    let db = running_postgres(platform, name, tag).await?;
    let rows = sql(platform, db.id, ROWS).await?;
    ensure!(rows == "1", "the converted database holds {rows:?}");
    platform.archive_wal(db.id).await?;
    sql(
        platform,
        db.id,
        "INSERT INTO t VALUES (2, clock_timestamp())",
    )
    .await?;
    tokio::time::sleep(Duration::from_millis(1200)).await;
    let request = PitrRestoreRequest {
        target_time: crate::clock::unix_ms(),
        new_name: Some(format!("{name}-b").parse()?),
        host_port: None,
    };
    let branch = platform.restore_pitr(db.id, request).await?;
    let rows = sql(platform, branch.app.id, ROWS).await?;
    ensure!(
        rows == "1,2",
        "the branch of the converted database holds {rows:?}"
    );
    Ok(())
}

#[tokio::test]
#[ignore = "needs a Docker Engine; run with make test-runtime-docker"]
async fn a_postgres_without_the_archive_keeps_its_data_on_the_docker_engine() -> anyhow::Result<()>
{
    catalog();
    let (platform, _dir) = docker_platform().await?;
    let name = format!("pitr{}", hex::encode(rand::random::<[u8; 3]>()));
    let run = convert(&platform, &name, PG17).await;
    run.and(remove_apps(&platform).await)
}

/// PostgreSQL 18, the image of the production databases at the time of the release, keeps its
/// data below the mount point of its volume.
#[tokio::test]
#[ignore = "needs a Docker Engine; run with make test-runtime-docker"]
async fn a_postgres_18_without_the_archive_keeps_its_data_on_the_docker_engine()
-> anyhow::Result<()> {
    catalog();
    let (platform, _dir) = docker_platform().await?;
    let name = format!("pitr{}", hex::encode(rand::random::<[u8; 3]>()));
    let run = convert(&platform, &name, "18-alpine").await;
    run.and(remove_apps(&platform).await)
}

async fn remove_apps(platform: &Arc<Platform>) -> anyhow::Result<()> {
    for app in platform.store.apps().await? {
        platform.delete_app(app.id).await?;
    }
    Ok(())
}

/// A platform whose runtime records every transfer that holds more than one piece in memory.
async fn bounded_platform() -> anyhow::Result<(
    Arc<Platform>,
    Arc<super::bounded_runtime::BoundedRuntime>,
    TempDir,
)> {
    let (command, _) = mpsc::channel(1);
    let (mut platform, _fake, dir) = platform_with(&PlatformConfig::default(), command).await?;
    let host = std::env::var("DOCKER_HOST").unwrap_or("unix:///var/run/docker.sock".into());
    let bounded = Arc::new(super::bounded_runtime::BoundedRuntime::new(Arc::new(
        super::docker_runtime(&host)?,
    )));
    platform.local = bounded.clone();
    platform.timing.health_timeout = Duration::from_secs(60);
    Ok((Arc::new(platform), bounded, dir))
}

/// A base backup and a recovery of a database larger than one transfer piece pass in bounded
/// pieces: a server with hundreds of megabytes must not be held in the memory of the panel,
/// which can be smaller than the database. The filler is random text, so the compressed base
/// stays larger than one piece too.
async fn recover_a_large_base(
    platform: &Arc<Platform>,
    bounded: &super::bounded_runtime::BoundedRuntime,
    name: &str,
) -> anyhow::Result<()> {
    let db = running_postgres(platform, name, PG17).await?;
    let filler = "CREATE TABLE f AS SELECT md5(random()::text) || md5(random()::text) AS x \
                  FROM generate_series(1, 800000)";
    sql(platform, db.id, filler).await?;
    let table = "CREATE TABLE t (i int, at timestamptz DEFAULT clock_timestamp())";
    sql(platform, db.id, table).await?;
    platform.archive_wal(db.id).await?;
    let commits = commit_six_rows(platform, db.id).await?;
    let request = PitrRestoreRequest {
        target_time: commits[2] + (commits[3] - commits[2]) / 2,
        new_name: Some(format!("{name}-branch").parse()?),
        host_port: None,
    };
    let response = platform.restore_pitr(db.id, request).await?;
    let branch = sql(platform, response.app.id, ROWS).await?;
    ensure!(branch == "1,2,3", "the branch holds the rows {branch}");
    let filled = sql(platform, response.app.id, "SELECT count(*) FROM f").await?;
    ensure!(filled == "800000", "the branch holds {filled} filler rows");
    let violations = bounded.violations();
    ensure!(
        violations.is_empty(),
        "whole transfers in memory: {violations:?}"
    );
    Ok(())
}

/// A logical backup larger than one transfer piece is taken and restored in bounded pieces:
/// the dump of `pg_dumpall` goes to standard output, and a server of hundreds of megabytes must
/// not pass through the memory of the panel. The restore brings back every row that the
/// backup saw.
async fn back_up_and_restore_a_large_dump(
    platform: &Arc<Platform>,
    bounded: &super::bounded_runtime::BoundedRuntime,
    name: &str,
) -> anyhow::Result<()> {
    let db = running_postgres(platform, name, PG17).await?;
    let filler = "CREATE TABLE f AS SELECT md5(random()::text) || md5(random()::text) AS x \
                  FROM generate_series(1, 800000)";
    sql(platform, db.id, filler).await?;
    let backup = platform.run_backup(db.id).await?;
    sql(platform, db.id, "DROP TABLE f").await?;
    platform.restore_backup(db.id, &backup.id).await?;
    let filled = sql(platform, db.id, "SELECT count(*) FROM f").await?;
    ensure!(filled == "800000", "the restore holds {filled} filler rows");
    let violations = bounded.violations();
    ensure!(
        violations.is_empty(),
        "whole transfers in memory: {violations:?}"
    );
    Ok(())
}

#[tokio::test]
#[ignore = "needs a Docker Engine; run with make test-runtime-docker"]
async fn postgres_backs_up_and_restores_a_large_dump_in_pieces_on_the_docker_engine()
-> anyhow::Result<()> {
    catalog();
    let (platform, bounded, _dir) = bounded_platform().await?;
    let name = format!("dump{}", hex::encode(rand::random::<[u8; 3]>()));
    let run = back_up_and_restore_a_large_dump(&platform, &bounded, &name).await;
    run.and(remove_apps(&platform).await)
}

#[tokio::test]
#[ignore = "needs a Docker Engine; run with make test-runtime-docker"]
async fn postgres_recovers_a_large_base_in_pieces_on_the_docker_engine() -> anyhow::Result<()> {
    catalog();
    let (platform, bounded, _dir) = bounded_platform().await?;
    let name = format!("pitr{}", hex::encode(rand::random::<[u8; 3]>()));
    let run = recover_a_large_base(&platform, &bounded, &name).await;
    run.and(remove_apps(&platform).await)
}

#[tokio::test]
#[ignore = "needs a Docker Engine; run with make test-runtime-docker"]
async fn postgres_recovers_to_a_second_on_the_docker_engine() -> anyhow::Result<()> {
    catalog();
    let (platform, _dir) = docker_platform().await?;
    let name = format!("pitr{}", hex::encode(rand::random::<[u8; 3]>()));
    let run = recover_between_two_rows(&platform, &name, PG17)
        .await
        .context("the recovery between two rows");
    let cleanup = remove_apps(&platform).await;
    run.and(cleanup)
}

/// PostgreSQL 18 keeps its data directory below the mount point of its volume, so the base
/// backup goes into a subdirectory of the volume.
#[tokio::test]
#[ignore = "needs a Docker Engine; run with make test-runtime-docker"]
async fn postgres_18_recovers_to_a_second_on_the_docker_engine() -> anyhow::Result<()> {
    catalog();
    let (platform, _dir) = docker_platform().await?;
    let name = format!("pitr{}", hex::encode(rand::random::<[u8; 3]>()));
    let run = recover_between_two_rows(&platform, &name, "18-alpine")
        .await
        .context("the recovery between two rows on PostgreSQL 18");
    let cleanup = remove_apps(&platform).await;
    run.and(cleanup)
}

#[tokio::test]
#[ignore = "needs a Docker Engine; run with make test-runtime-docker"]
async fn postgres_recovers_in_place_on_the_docker_engine() -> anyhow::Result<()> {
    catalog();
    let (platform, _dir) = docker_platform().await?;
    let name = format!("pitr{}", hex::encode(rand::random::<[u8; 3]>()));
    let run = recover_in_place(&platform, &name)
        .await
        .context("the recovery in place");
    let cleanup = remove_apps(&platform).await;
    run.and(cleanup)
}

#[tokio::test]
#[ignore = "needs a Docker Engine; run with make test-runtime-docker"]
async fn postgres_keeps_its_data_when_a_wal_file_is_missing_on_the_docker_engine()
-> anyhow::Result<()> {
    catalog();
    let (platform, _dir) = docker_platform().await?;
    let name = format!("pitr{}", hex::encode(rand::random::<[u8; 3]>()));
    let run = refuse_a_gap_in_the_archive(&platform, &name)
        .await
        .context("the recovery with a missing WAL file");
    let cleanup = remove_apps(&platform).await;
    run.and(cleanup)
}

#[tokio::test]
#[ignore = "needs a Docker Engine; run with make test-runtime-docker"]
async fn postgres_puts_its_data_back_when_the_replay_fails_on_the_docker_engine()
-> anyhow::Result<()> {
    catalog();
    let (platform, _dir) = docker_platform().await?;
    let name = format!("pitr{}", hex::encode(rand::random::<[u8; 3]>()));
    let run = roll_back_a_failed_replay(&platform, &name)
        .await
        .context("the recovery with a damaged WAL file");
    let cleanup = remove_apps(&platform).await;
    run.and(cleanup)
}

#[tokio::test]
#[ignore = "needs a Docker Engine; run with make test-runtime-docker"]
async fn postgres_recovers_into_an_idle_period_on_the_docker_engine() -> anyhow::Result<()> {
    catalog();
    let (platform, _dir) = docker_platform().await?;
    let name = format!("pitr{}", hex::encode(rand::random::<[u8; 3]>()));
    let run = recover_into_an_idle_period(&platform, &name)
        .await
        .context("the recovery into an idle period");
    let cleanup = remove_apps(&platform).await;
    run.and(cleanup)
}
