//! MongoDB as a single-member replica set on a Docker Engine. Only a replica set writes the
//! oplog that a point-in-time recovery replays (MongoDB manual, "Convert a Standalone mongod
//! to a Replica Set"), and an existing database must keep its data when its next deployment
//! turns it into one.
//!
//! The tests need a Docker Engine (`DOCKER_HOST`, else `unix:///var/run/docker.sock`).
//! `make test-runtime-docker` runs them.

use super::Platform;
use super::backup::mongodb_uri;
use super::tests::{TempDir, platform_with, request};
use anyhow::{Context as _, ensure};
use r3v3rs3_api::container::{
    ContainerName, ContainerSpec, EnvVar, ResourceLimits, RestartPolicy, VolumeMount,
};
use r3v3rs3_api::id::ShortId;
use r3v3rs3_api::platform::{AppEntry, AppSource, DeploymentTrigger, PlatformConfig};
use std::collections::BTreeMap;
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::mpsc;

const PASSWORD: &str = "s3cret0123";

const IMAGE: &str = "mirror.gcr.io/library/mongo:8";

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
        target: "/data/db".parse()?,
        read_only: false,
    })
}

fn root_env() -> anyhow::Result<Vec<EnvVar>> {
    let info = r3v3rs3_api::platform::engine_info("mongodb").context("the catalog")?;
    let entries = super::database_env::initial_db_env_entries(&info, PASSWORD)?;
    Ok(entries
        .into_iter()
        .map(|entry| EnvVar {
            key: entry.key,
            value: entry.value.unwrap_or_default(),
        })
        .collect())
}

/// Adds a MongoDB database app on the volume `<name>-data` with a host port, as
/// `create_database` makes it, without deploying it.
async fn add_db(platform: &Arc<Platform>, name: &str) -> anyhow::Result<AppEntry> {
    let mut req = request(name);
    req.spec.source = AppSource::Image {
        image: IMAGE.parse()?,
    };
    req.spec.port = 27017;
    req.spec.host_port = Some(platform.allocate_host_port().await?);
    req.spec.database = Some("mongodb".into());
    req.spec.volumes = vec![data_volume(name)?];
    let app = platform.add_app(req, 1).await?;
    let env = super::database_env::initial_db_env_entries(
        &r3v3rs3_api::platform::engine_info("mongodb").context("the catalog")?,
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
            !platform.busy_apps().contains(&id) || tokio::time::Instant::now() < deadline,
            "the deployment of {id} did not finish: {latest:?}"
        );
        ensure!(
            tokio::time::Instant::now() < deadline,
            "{id} does not serve: {latest:?}"
        );
        tokio::time::sleep(Duration::from_millis(500)).await;
    }
}

/// The printed answer of a mongosh script in the container of the database.
async fn eval(platform: &Arc<Platform>, id: ShortId, script: &str) -> anyhow::Result<String> {
    let (_, container, runtime) = platform.database_container(id).await?;
    let command = [
        "sh",
        "-c",
        "mongosh \"$MONGO_URI\" --quiet --eval \"$MONGO_SCRIPT\"",
    ]
    .map(str::to_string)
    .to_vec();
    let env = vec![
        format!("MONGO_URI={}", mongodb_uri("root", PASSWORD)?),
        format!("MONGO_SCRIPT={script}"),
    ];
    let out = runtime.exec_capture(&container, command, env).await?;
    Ok(String::from_utf8_lossy(&out).trim().to_string())
}

const COUNT: &str = "print(db.getSiblingDB('app').t.countDocuments())";

const SET_NAME: &str = "print(db.hello().setName)";

const WRITE: &str = "print(db.getSiblingDB('app').t.insertOne({i: 2}).acknowledged)";

/// Runs a standalone mongod on the volume of `name`, as the platform started every MongoDB
/// before, writes one document and removes the container again.
async fn standalone_with_a_document(platform: &Arc<Platform>, name: &str) -> anyhow::Result<()> {
    let runtime = platform.local.clone();
    let container: ContainerName = format!("{name}-standalone").parse()?;
    runtime
        .create_container(&standalone_spec(&container, name)?)
        .await?;
    runtime.start_container(&container).await?;
    let written = write_one(&*runtime, &container).await;
    runtime
        .stop_container(&container, Duration::from_secs(30))
        .await?;
    runtime.remove_container(&container).await?;
    written
}

/// A plain mongod on the volume of `name`, without a start command.
fn standalone_spec(container: &ContainerName, name: &str) -> anyhow::Result<ContainerSpec> {
    Ok(ContainerSpec {
        name: container.clone(),
        image: IMAGE.parse()?,
        env: root_env()?,
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

/// Writes one document as soon as the final server takes it; the entrypoint runs a temporary
/// server without the root user first.
async fn write_one(
    runtime: &dyn crate::runtime::ContainerRuntime,
    container: &ContainerName,
) -> anyhow::Result<()> {
    let insert =
        "db.getSiblingDB('app').t.insertOne({i: 1}, {writeConcern: {j: true}}).acknowledged";
    let command = vec![
        "sh".to_string(),
        "-c".to_string(),
        format!("mongosh \"$MONGO_URI\" --quiet --eval \"print({insert})\""),
    ];
    let env = vec![format!("MONGO_URI={}", mongodb_uri("root", PASSWORD)?)];
    let deadline = tokio::time::Instant::now() + Duration::from_secs(120);
    loop {
        let out = runtime
            .exec_capture(container, command.clone(), env.clone())
            .await;
        if out
            .as_ref()
            .is_ok_and(|out| String::from_utf8_lossy(out).trim() == "true")
        {
            return Ok(());
        }
        ensure!(
            tokio::time::Instant::now() < deadline,
            "the standalone server took no write: {out:?}"
        );
        tokio::time::sleep(Duration::from_secs(1)).await;
    }
}

async fn convert(platform: &Arc<Platform>, name: &str) -> anyhow::Result<()> {
    let db = add_db(platform, name).await?;
    standalone_with_a_document(platform, name).await?;
    deploy(platform, db.id).await?;
    let set = eval(platform, db.id, SET_NAME).await?;
    ensure!(set == "rs0", "the database runs in the replica set {set:?}");
    let count = eval(platform, db.id, COUNT).await?;
    ensure!(count == "1", "the database holds {count} documents");
    let written = eval(platform, db.id, WRITE).await?;
    ensure!(written == "true", "the database took no write: {written}");
    Ok(())
}

async fn redeploy(platform: &Arc<Platform>, name: &str) -> anyhow::Result<()> {
    let db = add_db(platform, name).await?;
    deploy(platform, db.id).await?;
    let written = eval(platform, db.id, WRITE).await?;
    ensure!(
        written == "true",
        "the first deployment took no write: {written}"
    );
    deploy(platform, db.id).await?;
    ensure!(
        platform.deployments(db.id).await?.len() == 2,
        "the database has not two deployments"
    );
    let count = eval(platform, db.id, COUNT).await?;
    ensure!(count == "1", "the database holds {count} documents");
    let written = eval(platform, db.id, WRITE).await?;
    ensure!(
        written == "true",
        "the second deployment took no write: {written}"
    );
    Ok(())
}

/// A backup of the replica set restores to the documents it held, and the restored set takes
/// writes again: the dump and the restore run against the member, not the standalone server
/// they ran against before.
async fn backup_and_restore(platform: &Arc<Platform>, name: &str) -> anyhow::Result<()> {
    let db = add_db(platform, name).await?;
    deploy(platform, db.id).await?;
    eval(platform, db.id, WRITE).await?;
    let backup = platform.run_backup(db.id).await?;
    eval(platform, db.id, WRITE).await?;
    platform.restore_backup(db.id, &backup.id).await?;
    let count = eval(platform, db.id, COUNT).await?;
    ensure!(
        count == "1",
        "the restored database holds {count} documents"
    );
    let written = eval(platform, db.id, WRITE).await?;
    ensure!(
        written == "true",
        "the restored database took no write: {written}"
    );
    Ok(())
}

#[tokio::test]
#[ignore = "needs a Docker Engine; run with make test-runtime-docker"]
async fn a_mongodb_replica_set_restores_a_backup_on_the_docker_engine() -> anyhow::Result<()> {
    catalog();
    let (platform, _dir) = docker_platform().await?;
    let run = backup_and_restore(&platform, &name()).await;
    run.and(remove_apps(&platform).await)
}

async fn remove_apps(platform: &Arc<Platform>) -> anyhow::Result<()> {
    for app in platform.store.apps().await? {
        platform.delete_app(app.id).await?;
    }
    Ok(())
}

fn name() -> String {
    format!("mgo{}", hex::encode(rand::random::<[u8; 3]>()))
}

#[tokio::test]
#[ignore = "needs a Docker Engine; run with make test-runtime-docker"]
async fn a_standalone_mongodb_becomes_a_replica_set_on_the_docker_engine() -> anyhow::Result<()> {
    catalog();
    let (platform, _dir) = docker_platform().await?;
    let run = convert(&platform, &name()).await;
    run.and(remove_apps(&platform).await)
}

#[tokio::test]
#[ignore = "needs a Docker Engine; run with make test-runtime-docker"]
async fn a_mongodb_replica_set_serves_after_a_redeploy_on_the_docker_engine() -> anyhow::Result<()>
{
    catalog();
    let (platform, _dir) = docker_platform().await?;
    let run = redeploy(&platform, &name()).await;
    run.and(remove_apps(&platform).await)
}
