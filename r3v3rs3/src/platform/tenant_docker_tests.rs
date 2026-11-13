//! The tenant scripts against the real database images on a Docker Engine: a tenant writes
//! into its own database only, a new password replaces the old one, a deleted tenant cannot
//! sign in, and a new root password reaches the server.
//!
//! The tests need a Docker Engine (`DOCKER_HOST`, else `unix:///var/run/docker.sock`).
//! `make test-runtime-docker` runs them. The images come from mirror.gcr.io, Google's cache
//! of Docker Hub, so the CI runners stay below the Docker Hub pull rate limit.

use super::tenant::{create_script, drop_script, root_password_script, run_script};
use crate::runtime::ContainerRuntime;
use anyhow::{Context as _, ensure};
use r3v3rs3_api::container::{ContainerName, ContainerSpec, EnvVar, ResourceLimits, RestartPolicy};
use r3v3rs3_api::platform::EngineInfo;
use std::collections::BTreeMap;
use std::sync::Arc;
use std::time::Duration;

const ROOT_PASSWORD: &str = "rootpw0123";

/// An account that signs in: its user, its password and the database that holds the user.
struct Login<'a> {
    user: &'a str,
    password: &'a str,
    auth: &'a str,
}

fn engine(key: &str) -> anyhow::Result<EngineInfo> {
    let catalog: r3v3rs3_api::platform::EnginesCatalog =
        serde_json::from_str(include_str!("../../data/engines.json"))?;
    catalog
        .engines
        .into_iter()
        .find(|info| info.engine == key)
        .with_context(|| format!("the catalog has no engine {key}"))
}

fn runtime() -> anyhow::Result<Arc<dyn ContainerRuntime>> {
    let host = std::env::var("DOCKER_HOST").unwrap_or("unix:///var/run/docker.sock".into());
    Ok(Arc::new(super::docker_runtime(&host)?))
}

/// The environment of the database container: the root password and, for MongoDB, the
/// root user.
fn container_env(info: &EngineInfo) -> anyhow::Result<Vec<EnvVar>> {
    let entries = crate::admin::databases::initial_db_env_entries(info, ROOT_PASSWORD)?;
    Ok(entries
        .into_iter()
        .map(|entry| EnvVar {
            key: entry.key,
            value: entry.value.unwrap_or_default(),
        })
        .collect())
}

fn spec(info: &EngineInfo, name: &ContainerName, image: &str) -> anyhow::Result<ContainerSpec> {
    Ok(ContainerSpec {
        name: name.clone(),
        image: image.parse()?,
        env: container_env(info)?,
        labels: BTreeMap::new(),
        network: None,
        volumes: Vec::new(),
        restart: RestartPolicy::No,
        limits: ResourceLimits::default(),
        publish: None,
        host_port: None,
        extra_hosts: Vec::new(),
        log_max_size: None,
        log_max_file: None,
        command: super::deploy::database_command(Some(&info.engine)),
    })
}

/// The shell command that writes into `database` as `login`. It fails when the server
/// refuses the account or the database. It connects to the network address of the
/// container, like an app through the published port: the PostgreSQL image trusts every
/// connection from `127.0.0.1` without a password.
fn write_command(info: &EngineInfo, login: &Login<'_>, database: &str) -> String {
    let Login {
        user,
        password,
        auth,
    } = login;
    let command = match info.engine.as_str() {
        "postgres" => format!(
            "psql -v ON_ERROR_STOP=1 postgresql://{user}:{password}@$H/{database} -c 'CREATE TABLE IF NOT EXISTS t (x int)'"
        ),
        "mysql" | "mariadb" => format!(
            "{} -h$H -u{user} -p{password} {database} -e 'CREATE TABLE IF NOT EXISTS t (x int)'",
            info.engine
        ),
        "mongodb" => format!(
            "mongosh \"mongodb://{user}:{password}@$H/{database}?authSource={auth}\" --quiet --eval 'db.t.insertOne({{x: 1}})'"
        ),
        _ => format!(
            "redis-cli -h $H --no-auth-warning --user {user} --pass {password} set {database}:k 1 | grep -qx OK"
        ),
    };
    // Docker writes the IPv4 address of the container into /etc/hosts; not every image has
    // `hostname`.
    format!(
        "H=$(awk '$1 ~ /^[0-9]+\\./ && $1 != \"127.0.0.1\" {{print $1; exit}}' /etc/hosts); {command}"
    )
}

async fn can_write(
    runtime: &Arc<dyn ContainerRuntime>,
    container: &ContainerName,
    info: &EngineInfo,
    login: &Login<'_>,
    database: &str,
) -> bool {
    let command = vec![
        "sh".to_string(),
        "-c".to_string(),
        write_command(info, login, database),
    ];
    runtime
        .exec_capture(container, command, Vec::new())
        .await
        .is_ok()
}

/// The command that succeeds once the server takes connections over TCP, which the images
/// open only after their initialization.
fn ready_command(info: &EngineInfo) -> String {
    match info.engine.as_str() {
        "postgres" => "pg_isready -h 127.0.0.1 -U postgres".to_string(),
        "mysql" | "mariadb" => format!(
            "{} -h127.0.0.1 -uroot -p{ROOT_PASSWORD} -e 'SELECT 1'",
            info.engine
        ),
        "mongodb" => format!(
            "mongosh 'mongodb://root:{ROOT_PASSWORD}@127.0.0.1/admin' --quiet --eval 'db.runCommand({{ping: 1}})'"
        ),
        _ => format!("REDISCLI_AUTH={ROOT_PASSWORD} redis-cli ping | grep -qx PONG"),
    }
}

async fn wait_ready(
    runtime: &Arc<dyn ContainerRuntime>,
    container: &ContainerName,
    info: &EngineInfo,
) -> anyhow::Result<()> {
    let command = vec!["sh".to_string(), "-c".to_string(), ready_command(info)];
    for _ in 0..120 {
        if runtime
            .exec_capture(container, command.clone(), Vec::new())
            .await
            .is_ok()
        {
            return Ok(());
        }
        tokio::time::sleep(Duration::from_secs(1)).await;
    }
    anyhow::bail!("the {} server did not start", info.engine)
}

/// A tenant with its password, as a login of its own database.
fn tenant<'a>(name: &'a str, password: &'a str) -> Login<'a> {
    Login {
        user: name,
        password,
        auth: name,
    }
}

/// Two tenants reach their own databases and not each other's.
async fn check_isolation(
    runtime: &Arc<dyn ContainerRuntime>,
    container: &ContainerName,
    info: &EngineInfo,
) -> anyhow::Result<()> {
    for (name, password) in [("p_one", "pwone1"), ("p_two", "pwtwo1")] {
        run_script(
            runtime,
            container,
            create_script(info, ROOT_PASSWORD, name, password)?,
        )
        .await?;
    }
    let one = tenant("p_one", "pwone1");
    ensure!(can_write(runtime, container, info, &one, "p_one").await);
    ensure!(
        can_write(
            runtime,
            container,
            info,
            &tenant("p_two", "pwtwo1"),
            "p_two"
        )
        .await
    );
    ensure!(
        !can_write(runtime, container, info, &one, "p_two").await,
        "a tenant wrote into the database of another tenant"
    );
    ensure!(
        !can_write(
            runtime,
            container,
            info,
            &tenant("p_one", "wrong1"),
            "p_one"
        )
        .await
    );
    // A client without an account reaches no data either.
    ensure!(
        !can_write(
            runtime,
            container,
            info,
            &tenant("nobody", "nopass1"),
            "p_one"
        )
        .await,
        "an unknown account wrote into a tenant database"
    );
    Ok(())
}

/// A new password of a tenant replaces the old one, and a dropped tenant cannot sign in.
async fn check_reset_and_drop(
    runtime: &Arc<dyn ContainerRuntime>,
    container: &ContainerName,
    info: &EngineInfo,
) -> anyhow::Result<()> {
    run_script(
        runtime,
        container,
        create_script(info, ROOT_PASSWORD, "p_one", "pwone2")?,
    )
    .await?;
    ensure!(
        !can_write(
            runtime,
            container,
            info,
            &tenant("p_one", "pwone1"),
            "p_one"
        )
        .await
    );
    ensure!(
        can_write(
            runtime,
            container,
            info,
            &tenant("p_one", "pwone2"),
            "p_one"
        )
        .await
    );
    run_script(
        runtime,
        container,
        drop_script(info, ROOT_PASSWORD, "p_two")?,
    )
    .await?;
    ensure!(
        !can_write(
            runtime,
            container,
            info,
            &tenant("p_two", "pwtwo1"),
            "p_two"
        )
        .await
    );
    Ok(())
}

/// A new root password signs in, and the old one does not.
async fn check_root_password(
    runtime: &Arc<dyn ContainerRuntime>,
    container: &ContainerName,
    info: &EngineInfo,
) -> anyhow::Result<()> {
    let script = root_password_script(info, ROOT_PASSWORD, "rootpw9876")?;
    run_script(runtime, container, script).await?;
    let root = |password| Login {
        user: &info.user,
        password,
        auth: "admin",
    };
    ensure!(can_write(runtime, container, info, &root("rootpw9876"), "p_one").await);
    ensure!(!can_write(runtime, container, info, &root(ROOT_PASSWORD), "p_one").await);
    // The backup signs in with the new password.
    let dump = super::backup::dump_for_test(runtime, container, info, "rootpw9876").await?;
    ensure!(!dump.is_empty(), "the {} dump is empty", info.engine);
    Ok(())
}

/// Starts the server and runs every check on it.
async fn run_checks(
    runtime: &Arc<dyn ContainerRuntime>,
    name: &ContainerName,
    info: &EngineInfo,
) -> anyhow::Result<()> {
    runtime.start_container(name).await?;
    wait_ready(runtime, name, info).await?;
    check_isolation(runtime, name, info).await?;
    check_reset_and_drop(runtime, name, info).await?;
    check_root_password(runtime, name, info).await
}

/// Runs every check on a fresh container of the image and removes it again.
async fn check_engine(key: &str, image: &str) -> anyhow::Result<()> {
    let info = engine(key)?;
    let runtime = runtime()?;
    let name: ContainerName =
        format!("r3v3rs3-tenant-{key}-{:08x}", rand::random::<u32>()).parse()?;
    runtime.pull_image(&image.parse()?).await?;
    runtime
        .create_container(&spec(&info, &name, image)?)
        .await?;
    let result = run_checks(&runtime, &name, &info).await;
    let removed = runtime.remove_container(&name).await;
    result?;
    removed
}

#[tokio::test]
#[ignore = "needs a Docker Engine; run with make test-runtime-docker"]
async fn postgres_tenants_are_isolated_on_the_docker_engine() -> anyhow::Result<()> {
    check_engine("postgres", "mirror.gcr.io/library/postgres:18-alpine").await
}

#[tokio::test]
#[ignore = "needs a Docker Engine; run with make test-runtime-docker"]
async fn mysql_tenants_are_isolated_on_the_docker_engine() -> anyhow::Result<()> {
    check_engine("mysql", "mirror.gcr.io/library/mysql:9").await
}

#[tokio::test]
#[ignore = "needs a Docker Engine; run with make test-runtime-docker"]
async fn mariadb_tenants_are_isolated_on_the_docker_engine() -> anyhow::Result<()> {
    check_engine("mariadb", "mirror.gcr.io/library/mariadb:12-noble").await
}

#[tokio::test]
#[ignore = "needs a Docker Engine; run with make test-runtime-docker"]
async fn mongodb_tenants_are_isolated_on_the_docker_engine() -> anyhow::Result<()> {
    check_engine("mongodb", "mirror.gcr.io/library/mongo:8").await
}

#[tokio::test]
#[ignore = "needs a Docker Engine; run with make test-runtime-docker"]
async fn redis_tenants_are_isolated_on_the_docker_engine() -> anyhow::Result<()> {
    check_engine("redis", "mirror.gcr.io/library/redis:8-alpine").await
}
