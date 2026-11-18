//! Point-in-time recovery of MySQL and MariaDB on a Docker Engine: the server writes its
//! binary log, the platform pulls it and takes a dump with its coordinates, and a recovery to
//! a time between two commits brings back exactly the commits before that time (MySQL
//! manual, "Point-in-Time Recovery Using Binary Log"; MariaDB, "mariadb-binlog").
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

const ROWS: &str = "SELECT group_concat(i ORDER BY i) FROM app.t";

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

/// Adds a database of `engine` (`mysql` or `mariadb`) on `image` and deploys it until it runs.
async fn running_db(
    platform: &Arc<Platform>,
    name: &str,
    engine: &str,
    image: &str,
) -> anyhow::Result<AppEntry> {
    let info = r3v3rs3_api::platform::engine_info(engine).context("the catalog")?;
    let mut req = request(name);
    req.spec.source = AppSource::Image {
        image: format!("mirror.gcr.io/library/{image}").parse()?,
    };
    req.spec.port = 3306;
    req.spec.database = Some(engine.into());
    req.spec.volumes = vec![VolumeMount {
        volume: format!("{name}-data").parse()?,
        target: info.data_path.parse()?,
        read_only: false,
    }];
    let app = platform.add_app(req, 1).await?;
    let password = EnvEntry {
        key: info.password_env.parse()?,
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
    let deadline = tokio::time::Instant::now() + Duration::from_secs(240);
    loop {
        let busy = platform.busy_apps().contains(&id);
        if !busy
            && sql_on(platform, id, "SELECT @@skip_networking")
                .await
                .is_ok_and(|off| off == "0")
        {
            return Ok(());
        }
        ensure!(tokio::time::Instant::now() < deadline, "{id} did not run");
        tokio::time::sleep(Duration::from_millis(500)).await;
    }
}

/// The answer of one query in the container of the database.
async fn sql_on(platform: &Arc<Platform>, id: ShortId, query: &str) -> anyhow::Result<String> {
    let (app, container, runtime) = platform.database_container(id).await?;
    let client = if app.spec.database.as_deref() == Some("mariadb") {
        "mariadb"
    } else {
        "mysql"
    };
    let command = [client, "-u", "root", "-N", "-B", "-e", query]
        .map(str::to_string)
        .to_vec();
    let out = runtime
        .exec_capture(&container, command, vec![format!("MYSQL_PWD={PASSWORD}")])
        .await?;
    Ok(String::from_utf8_lossy(&out).trim().to_string())
}

/// A running database with six rows committed about 1.2 seconds apart after its first archive
/// call. An account that exists at the base backup is dropped after it: the fresh server of a
/// recovery has no such account, so a replayed `DROP USER` would stop the replay. The target
/// lies one second after row 3, a full second away from rows 3 and 4 after the rounding of the
/// decoder.
async fn six_rows(
    platform: &Arc<Platform>,
    name: &str,
    engine: &str,
    image: &str,
) -> anyhow::Result<(AppEntry, u64)> {
    let db = running_db(platform, name, engine, image).await?;
    let setup = "CREATE DATABASE app; \
CREATE TABLE app.t (i int primary key, at datetime(3) DEFAULT now(3)); \
CREATE USER 'old'@'%' IDENTIFIED BY 'x0123456789'";
    sql_on(platform, db.id, setup).await?;
    // The first archive call takes the base backup that the recovery starts from.
    platform.archive_wal(db.id).await?;
    sql_on(platform, db.id, "DROP USER 'old'@'%'").await?;
    let insert = "INSERT INTO app.t (i) SELECT coalesce(max(i), 0) + 1 FROM app.t";
    for _ in 1..=6 {
        sql_on(platform, db.id, insert).await?;
        tokio::time::sleep(Duration::from_millis(1200)).await;
    }
    let third = "SELECT ROUND(UNIX_TIMESTAMP(at) * 1000) FROM app.t WHERE i = 3";
    let third: u64 = sql_on(platform, db.id, third).await?.parse()?;
    Ok((db, third + 1000))
}

async fn recover_branch(
    platform: &Arc<Platform>,
    name: &str,
    engine: &str,
    image: &str,
) -> anyhow::Result<()> {
    let (db, target_time) = six_rows(platform, name, engine, image).await?;
    let request = PitrRestoreRequest {
        target_time,
        new_name: Some(format!("{name}-branch").parse()?),
        host_port: None,
    };
    let response = platform.restore_pitr(db.id, request).await?;
    ensure!(response.branched, "{response:?}");
    let branch = sql_on(platform, response.app.id, ROWS).await?;
    ensure!(branch == "1,2,3", "the branch holds the rows {branch}");
    let source = sql_on(platform, db.id, ROWS).await?;
    ensure!(
        source == "1,2,3,4,5,6",
        "the source holds the rows {source}"
    );
    Ok(())
}

async fn recover_in_place(
    platform: &Arc<Platform>,
    name: &str,
    engine: &str,
    image: &str,
) -> anyhow::Result<()> {
    let (db, target_time) = six_rows(platform, name, engine, image).await?;
    let request = PitrRestoreRequest {
        target_time,
        new_name: None,
        host_port: None,
    };
    let response = platform.restore_pitr(db.id, request).await?;
    ensure!(!response.branched, "{response:?}");
    let rows = sql_on(platform, db.id, ROWS).await?;
    ensure!(rows == "1,2,3", "the database holds the rows {rows}");
    // The recovered server takes writes, and its new history can be archived again.
    sql_on(platform, db.id, "INSERT INTO app.t (i) VALUES (7)").await?;
    platform.archive_wal(db.id).await?;
    Ok(())
}

/// A recovery that misses one archived log file fails, names the file and leaves the data of
/// the database as it was.
async fn refuse_a_gap(
    platform: &Arc<Platform>,
    name: &str,
    engine: &str,
    image: &str,
) -> anyhow::Result<()> {
    let (db, target_time) = six_rows(platform, name, engine, image).await?;
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
    let rows = sql_on(platform, db.id, ROWS).await?;
    ensure!(rows == "1,2,3,4,5,6", "the database holds the rows {rows}");
    Ok(())
}

async fn remove_apps(platform: &Arc<Platform>) -> anyhow::Result<()> {
    for app in platform.store.apps().await? {
        platform.delete_app(app.id).await?;
    }
    Ok(())
}

fn name() -> String {
    format!("bin{}", hex::encode(rand::random::<[u8; 3]>()))
}

#[tokio::test]
#[ignore = "needs a Docker Engine; run with make test-runtime-docker"]
async fn mysql_recovers_a_branch_to_a_second_on_the_docker_engine() -> anyhow::Result<()> {
    catalog();
    let (platform, _dir) = docker_platform().await?;
    let run = recover_branch(&platform, &name(), "mysql", "mysql:9").await;
    run.and(remove_apps(&platform).await)
}

#[tokio::test]
#[ignore = "needs a Docker Engine; run with make test-runtime-docker"]
async fn mariadb_recovers_a_branch_to_a_second_on_the_docker_engine() -> anyhow::Result<()> {
    catalog();
    let (platform, _dir) = docker_platform().await?;
    let run = recover_branch(&platform, &name(), "mariadb", "mariadb:11.8-noble").await;
    run.and(remove_apps(&platform).await)
}

#[tokio::test]
#[ignore = "needs a Docker Engine; run with make test-runtime-docker"]
async fn mysql_recovers_in_place_on_the_docker_engine() -> anyhow::Result<()> {
    catalog();
    let (platform, _dir) = docker_platform().await?;
    let run = recover_in_place(&platform, &name(), "mysql", "mysql:9").await;
    run.and(remove_apps(&platform).await)
}

#[tokio::test]
#[ignore = "needs a Docker Engine; run with make test-runtime-docker"]
async fn mariadb_recovers_in_place_on_the_docker_engine() -> anyhow::Result<()> {
    catalog();
    let (platform, _dir) = docker_platform().await?;
    let run = recover_in_place(&platform, &name(), "mariadb", "mariadb:11.8-noble").await;
    run.and(remove_apps(&platform).await)
}

#[tokio::test]
#[ignore = "needs a Docker Engine; run with make test-runtime-docker"]
async fn mysql_keeps_its_data_when_a_binlog_is_missing_on_the_docker_engine() -> anyhow::Result<()>
{
    catalog();
    let (platform, _dir) = docker_platform().await?;
    let run = refuse_a_gap(&platform, &name(), "mysql", "mysql:9").await;
    run.and(remove_apps(&platform).await)
}

#[tokio::test]
#[ignore = "needs a Docker Engine; run with make test-runtime-docker"]
async fn mariadb_keeps_its_data_when_a_binlog_is_missing_on_the_docker_engine() -> anyhow::Result<()>
{
    catalog();
    let (platform, _dir) = docker_platform().await?;
    let run = refuse_a_gap(&platform, &name(), "mariadb", "mariadb:11.8-noble").await;
    run.and(remove_apps(&platform).await)
}
