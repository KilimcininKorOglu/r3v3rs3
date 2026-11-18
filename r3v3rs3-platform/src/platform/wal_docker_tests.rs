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
use r3v3rs3_api::container::VolumeMount;
use r3v3rs3_api::id::ShortId;
use r3v3rs3_api::platform::{
    AppEntry, AppSource, DeploymentTrigger, EnvEntry, PitrRestoreRequest, PlatformConfig,
};
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

/// Adds a PostgreSQL database with its own data volume and deploys it until it runs.
async fn running_postgres(platform: &Arc<Platform>, name: &str) -> anyhow::Result<AppEntry> {
    let mut req = request(name);
    req.spec.source = AppSource::Image {
        image: "mirror.gcr.io/library/postgres:17-alpine".parse()?,
    };
    req.spec.port = 5432;
    req.spec.database = Some("postgres".into());
    req.spec.volumes = vec![VolumeMount {
        volume: format!("{name}-data").parse()?,
        target: "/var/lib/postgresql/data".parse()?,
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
async fn six_rows(platform: &Arc<Platform>, name: &str) -> anyhow::Result<(AppEntry, u64)> {
    let db = running_postgres(platform, name).await?;
    let table = "CREATE TABLE t (i int, at timestamptz DEFAULT clock_timestamp())";
    sql(platform, db.id, table).await?;
    // The first archive call takes the base backup that the recovery starts from.
    platform.archive_wal(db.id).await?;
    let commits = commit_six_rows(platform, db.id).await?;
    let (third, fourth) = (commits[2], commits[3]);
    Ok((db, third + (fourth - third) / 2))
}

/// Recovers a branch to the middle between rows 3 and 4.
async fn recover_between_two_rows(platform: &Arc<Platform>, name: &str) -> anyhow::Result<()> {
    let (db, target_time) = six_rows(platform, name).await?;
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
    let (db, target_time) = six_rows(platform, name).await?;
    let request = PitrRestoreRequest {
        target_time,
        new_name: None,
        host_port: None,
    };
    let response = platform.restore_pitr(db.id, request).await?;
    ensure!(!response.branched, "{response:?}");
    let rows = sql(platform, db.id, ROWS).await?;
    ensure!(rows == "1,2,3", "the database holds the rows {rows}");
    // The promoted server takes new writes.
    sql(platform, db.id, "INSERT INTO t (i) VALUES (7)").await?;
    Ok(())
}

/// A recovery that misses one archived WAL file fails, names the file and leaves the data of
/// the database as it was.
async fn refuse_a_gap_in_the_archive(platform: &Arc<Platform>, name: &str) -> anyhow::Result<()> {
    let (db, target_time) = six_rows(platform, name).await?;
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

async fn remove_apps(platform: &Arc<Platform>) -> anyhow::Result<()> {
    for app in platform.store.apps().await? {
        platform.delete_app(app.id).await?;
    }
    Ok(())
}

#[tokio::test]
#[ignore = "needs a Docker Engine; run with make test-runtime-docker"]
async fn postgres_recovers_to_a_second_on_the_docker_engine() -> anyhow::Result<()> {
    catalog();
    let (platform, _dir) = docker_platform().await?;
    let name = format!("pitr{}", hex::encode(rand::random::<[u8; 3]>()));
    let run = recover_between_two_rows(&platform, &name)
        .await
        .context("the recovery between two rows");
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
