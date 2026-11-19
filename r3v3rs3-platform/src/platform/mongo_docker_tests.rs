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
use r3v3rs3_api::platform::{
    AppEntry, AppSource, DeploymentTrigger, PitrRestoreRequest, PlatformConfig,
};
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
    add_db_on(platform, name, IMAGE).await
}

async fn add_db_on(platform: &Arc<Platform>, name: &str, image: &str) -> anyhow::Result<AppEntry> {
    let mut req = request(name);
    req.spec.source = AppSource::Image {
        image: image.parse()?,
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
async fn standalone_with_a_document(
    platform: &Arc<Platform>,
    name: &str,
    image: &str,
) -> anyhow::Result<()> {
    let runtime = platform.local.clone();
    let container: ContainerName = format!("{name}-standalone").parse()?;
    let spec = standalone_spec(&container, name, image)?;
    // A fresh Docker Engine, such as the one of CI, has no image yet.
    runtime.pull_image(&spec.image).await?;
    runtime.create_container(&spec).await?;
    runtime.start_container(&container).await?;
    let written = write_one(&*runtime, &container).await;
    runtime
        .stop_container(&container, Duration::from_secs(30))
        .await?;
    runtime.remove_container(&container).await?;
    written
}

/// A plain mongod on the volume of `name`, without a start command.
fn standalone_spec(
    container: &ContainerName,
    name: &str,
    image: &str,
) -> anyhow::Result<ContainerSpec> {
    Ok(ContainerSpec {
        name: container.clone(),
        image: image.parse()?,
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

/// A standalone MongoDB on `image` becomes a replica set on its next deployment, keeps its
/// documents, takes writes, and its archive covers the writes after the conversion.
async fn convert(platform: &Arc<Platform>, name: &str, image: &str) -> anyhow::Result<()> {
    let db = add_db_on(platform, name, image).await?;
    standalone_with_a_document(platform, name, image).await?;
    deploy(platform, db.id).await?;
    let set = eval(platform, db.id, SET_NAME).await?;
    ensure!(set == "rs0", "the database runs in the replica set {set:?}");
    let count = eval(platform, db.id, COUNT).await?;
    ensure!(count == "1", "the database holds {count} documents");
    archive_a_write(platform, db.id, name).await
}

/// Archives, writes a second document, and requires a branch to now to hold both.
async fn archive_a_write(platform: &Arc<Platform>, id: ShortId, name: &str) -> anyhow::Result<()> {
    platform.archive_wal(id).await?;
    let written = eval(platform, id, WRITE).await?;
    ensure!(written == "true", "the database took no write: {written}");
    tokio::time::sleep(Duration::from_millis(1200)).await;
    let request = PitrRestoreRequest {
        target_time: crate::clock::unix_ms(),
        new_name: Some(format!("{name}-b").parse()?),
        host_port: None,
    };
    let branch = platform.restore_pitr(id, request).await?;
    let count = eval(platform, branch.app.id, COUNT).await?;
    ensure!(
        count == "2",
        "the branch of the converted database holds {count} documents"
    );
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

const DOCS: &str =
    "print(db.getSiblingDB('app').t.find().sort({i: 1}).toArray().map(d => d.i).join(','))";

const INSERT: &str = "const t = db.getSiblingDB('app').t; \
const last = t.find().sort({i: -1}).limit(1).toArray(); \
print(t.insertOne({i: last.length ? last[0].i + 1 : 1, at: new Date()}).acknowledged)";

/// The users of the `admin` database other than root.
const USERS: &str = "print(db.getSiblingDB('admin').getUsers().users.map(u => u.user).filter(u => u !== 'root').join(','))";

/// The commit time of document `i` in Unix milliseconds.
async fn doc_time(platform: &Arc<Platform>, id: ShortId, i: u32) -> anyhow::Result<u64> {
    let script = format!("print(db.getSiblingDB('app').t.findOne({{i: {i}}}).at.getTime())");
    Ok(eval(platform, id, &script).await?.parse()?)
}

/// A running database with six documents written about 1.2 seconds apart after its first
/// archive call, and the target time one second after document 3. An account that exists at
/// the base backup is dropped after it, and another one is created before the target: the
/// platform owns the accounts, so a replay must neither stop on them nor bring one back.
async fn six_docs(platform: &Arc<Platform>, name: &str) -> anyhow::Result<(AppEntry, u64)> {
    let db = add_db(platform, name).await?;
    deploy(platform, db.id).await?;
    let user = |name: &str| {
        format!(
            "db.getSiblingDB('admin').createUser({{user: '{name}', pwd: 'x0123456789', roles: []}})"
        )
    };
    eval(platform, db.id, &user("old")).await?;
    // The first archive call takes the base backup that the recovery starts from.
    platform.archive_wal(db.id).await?;
    eval(platform, db.id, "db.getSiblingDB('admin').dropUser('old')").await?;
    eval(platform, db.id, &user("late")).await?;
    for _ in 1..=6 {
        eval(platform, db.id, INSERT).await?;
        tokio::time::sleep(Duration::from_millis(1200)).await;
    }
    let third = doc_time(platform, db.id, 3).await?;
    Ok((db, third + 1000))
}

/// The documents and the extra accounts of a new branch of `id` recovered to `target_time`.
async fn branch_docs(
    platform: &Arc<Platform>,
    id: ShortId,
    name: &str,
    target_time: u64,
) -> anyhow::Result<(String, String)> {
    let request = PitrRestoreRequest {
        target_time,
        new_name: Some(name.parse()?),
        host_port: None,
    };
    let response = platform.restore_pitr(id, request).await?;
    ensure!(response.branched, "{response:?}");
    let docs = eval(platform, response.app.id, DOCS).await?;
    let users = eval(platform, response.app.id, USERS).await?;
    Ok((docs, users))
}

async fn recover_branch(platform: &Arc<Platform>, name: &str) -> anyhow::Result<()> {
    let (db, target_time) = six_docs(platform, name).await?;
    let (docs, users) = branch_docs(platform, db.id, &format!("{name}-b"), target_time).await?;
    ensure!(docs == "1,2,3", "the branch holds the documents {docs}");
    ensure!(users.is_empty(), "the branch holds the accounts {users}");
    let source = eval(platform, db.id, DOCS).await?;
    ensure!(source == "1,2,3,4,5,6", "the source holds {source}");
    Ok(())
}

async fn recover_in_place(platform: &Arc<Platform>, name: &str) -> anyhow::Result<()> {
    let (db, target_time) = six_docs(platform, name).await?;
    let fifth = doc_time(platform, db.id, 5).await?;
    let request = PitrRestoreRequest {
        target_time,
        new_name: None,
        host_port: None,
    };
    let response = platform.restore_pitr(db.id, request).await?;
    ensure!(!response.branched, "{response:?}");
    let docs = eval(platform, db.id, DOCS).await?;
    ensure!(docs == "1,2,3", "the database holds the documents {docs}");
    // The recovered set takes writes, and its new history can be archived again.
    eval(platform, db.id, INSERT).await?;
    let fourth = doc_time(platform, db.id, 4).await?;
    tokio::time::sleep(Duration::from_millis(1200)).await;
    platform.archive_wal(db.id).await?;
    // The history before the recovery stays reachable.
    let (old, _) = branch_docs(platform, db.id, &format!("{name}-o"), fifth + 1000).await?;
    ensure!(
        old == "1,2,3,4,5",
        "the branch of the old history holds {old}"
    );
    let (new, _) = branch_docs(platform, db.id, &format!("{name}-n"), fourth + 1000).await?;
    ensure!(
        new == "1,2,3,4",
        "the branch of the new history holds {new}"
    );
    Ok(())
}

/// A recovery that misses one archived oplog file fails, names the file and leaves the data
/// of the database as it was.
async fn refuse_a_gap(platform: &Arc<Platform>, name: &str) -> anyhow::Result<()> {
    let (db, target_time) = six_docs(platform, name).await?;
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
    let docs = eval(platform, db.id, DOCS).await?;
    ensure!(docs == "1,2,3,4,5,6", "the database holds {docs}");
    Ok(())
}

/// A recovery whose replay fails half way, here on a damaged oplog file, puts the data that
/// it moved aside back, and the set takes writes again.
async fn roll_back_a_failed_replay(platform: &Arc<Platform>, name: &str) -> anyhow::Result<()> {
    let (db, target_time) = six_docs(platform, name).await?;
    let damaged = platform.archive_wal(db.id).await?;
    let dir = platform.wal_dir(db.id).await?;
    let broken = super::backup::gzip(b"not an oplog").await?;
    tokio::fs::write(dir.join(format!("{}.wal.gz", damaged.id)), broken).await?;
    let request = PitrRestoreRequest {
        target_time,
        new_name: None,
        host_port: None,
    };
    let err = platform.restore_pitr(db.id, request).await.unwrap_err();
    ensure!(format!("{err:#}").contains("restore failed"), "{err:#}");
    let docs = eval(platform, db.id, DOCS).await?;
    ensure!(docs == "1,2,3,4,5,6", "the database holds {docs}");
    let written = eval(platform, db.id, INSERT).await?;
    ensure!(written == "true", "the database took no write: {written}");
    Ok(())
}

/// Makes the archive look as if the capped oplog had dropped the entries after it: the base
/// backup starts at the first possible timestamp and no oplog file follows it.
async fn open_a_gap(platform: &Arc<Platform>, id: ShortId) -> anyhow::Result<()> {
    let wal = platform.wal_dir(id).await?;
    tokio::fs::remove_dir_all(&wal).await?;
    let dir = platform.base_dir(id).await?;
    for mut base in platform.list_base_backups(id).await? {
        base.start_wal = "oplog.1".into();
        let path = dir.join(format!("{}.json", base.id));
        tokio::fs::write(path, serde_json::to_vec(&base)?).await?;
    }
    Ok(())
}

/// After a gap in the archive, a target inside the gap is refused: no archived oplog leads
/// from a base backup to it. A target after the gap recovers from the new base.
async fn refuse_a_target_in_a_gap(platform: &Arc<Platform>, name: &str) -> anyhow::Result<()> {
    let (db, target_time) = six_docs(platform, name).await?;
    open_a_gap(platform, db.id).await?;
    // The next pull finds the gap.
    platform.pull_wal(db.id).await?;
    eval(platform, db.id, INSERT).await?;
    let seventh = doc_time(platform, db.id, 7).await?;
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
    let (docs, _) = branch_docs(platform, db.id, &format!("{name}-a"), seventh + 1000).await?;
    ensure!(
        docs == "1,2,3,4,5,6,7",
        "the branch after the gap holds {docs}"
    );
    Ok(())
}

/// An idle primary writes a "periodic noop" entry about every ten seconds (measured on
/// mongo:8). Those entries change no data, so pulls across them store no file: otherwise an
/// idle database fills its archive with one file a minute. A real write is stored again.
async fn store_no_idle_noops(platform: &Arc<Platform>, name: &str) -> anyhow::Result<()> {
    let db = add_db(platform, name).await?;
    deploy(platform, db.id).await?;
    platform.archive_wal(db.id).await?;
    let before = platform.list_wal_segments(db.id).await?.len();
    tokio::time::sleep(Duration::from_secs(25)).await;
    platform.pull_wal(db.id).await?;
    let idle = platform.list_wal_segments(db.id).await?.len();
    ensure!(
        idle == before,
        "an idle pull stored {} files",
        idle - before
    );
    eval(platform, db.id, INSERT).await?;
    platform.pull_wal(db.id).await?;
    let written = platform.list_wal_segments(db.id).await?.len();
    ensure!(
        written == before + 1,
        "a write stored {} files",
        written - before
    );
    Ok(())
}

#[tokio::test]
#[ignore = "needs a Docker Engine; run with make test-runtime-docker"]
async fn an_idle_mongodb_archives_no_noop_entries_on_the_docker_engine() -> anyhow::Result<()> {
    catalog();
    let (platform, _dir) = docker_platform().await?;
    let run = store_no_idle_noops(&platform, &name()).await;
    run.and(remove_apps(&platform).await)
}

#[tokio::test]
#[ignore = "needs a Docker Engine; run with make test-runtime-docker"]
async fn mongodb_refuses_a_target_in_an_oplog_gap_on_the_docker_engine() -> anyhow::Result<()> {
    catalog();
    let (platform, _dir) = docker_platform().await?;
    let run = refuse_a_target_in_a_gap(&platform, &name()).await;
    run.and(remove_apps(&platform).await)
}

#[tokio::test]
#[ignore = "needs a Docker Engine; run with make test-runtime-docker"]
async fn mongodb_recovers_a_branch_to_a_second_on_the_docker_engine() -> anyhow::Result<()> {
    catalog();
    let (platform, _dir) = docker_platform().await?;
    let run = recover_branch(&platform, &name()).await;
    run.and(remove_apps(&platform).await)
}

#[tokio::test]
#[ignore = "needs a Docker Engine; run with make test-runtime-docker"]
async fn mongodb_recovers_in_place_on_the_docker_engine() -> anyhow::Result<()> {
    catalog();
    let (platform, _dir) = docker_platform().await?;
    let run = recover_in_place(&platform, &name()).await;
    run.and(remove_apps(&platform).await)
}

#[tokio::test]
#[ignore = "needs a Docker Engine; run with make test-runtime-docker"]
async fn mongodb_keeps_its_data_when_an_oplog_file_is_missing_on_the_docker_engine()
-> anyhow::Result<()> {
    catalog();
    let (platform, _dir) = docker_platform().await?;
    let run = refuse_a_gap(&platform, &name()).await;
    run.and(remove_apps(&platform).await)
}

#[tokio::test]
#[ignore = "needs a Docker Engine; run with make test-runtime-docker"]
async fn mongodb_puts_its_data_back_when_the_replay_fails_on_the_docker_engine()
-> anyhow::Result<()> {
    catalog();
    let (platform, _dir) = docker_platform().await?;
    let run = roll_back_a_failed_replay(&platform, &name()).await;
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
    let run = convert(&platform, &name(), IMAGE).await;
    run.and(remove_apps(&platform).await)
}

/// The image of the production databases at the time of the release.
#[tokio::test]
#[ignore = "needs a Docker Engine; run with make test-runtime-docker"]
async fn a_standalone_mongodb_9_becomes_a_replica_set_on_the_docker_engine() -> anyhow::Result<()> {
    catalog();
    let (platform, _dir) = docker_platform().await?;
    let run = convert(&platform, &name(), "mirror.gcr.io/library/mongo:9").await;
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
