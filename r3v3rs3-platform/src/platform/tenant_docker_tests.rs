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
        serde_json::from_str(include_str!("../../../r3v3rs3/data/engines.json"))?;
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
    let entries = super::database_env::initial_db_env_entries(info, ROOT_PASSWORD)?;
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
        aliases: Vec::new(),
        extra_networks: Vec::new(),
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
        // An empty user signs in as nobody: the default user without a password.
        _ if user.is_empty() => format!("redis-cli -h $H set {database}:k 1 | grep -qx OK"),
        _ => format!(
            "redis-cli -h $H --no-auth-warning --user {user} --pass {password} set {database}:k 1 | grep -qx OK"
        ),
    };
    format!("{HOST}; {command}")
}

/// Sets `H` to the IPv4 address of the container, which Docker writes into /etc/hosts; not
/// every image has `hostname`.
const HOST: &str =
    "H=$(awk '$1 ~ /^[0-9]+\\./ && $1 != \"127.0.0.1\" {print $1; exit}' /etc/hosts)";

/// A PostgreSQL tenant connects to no database but its own: PUBLIC may connect to the
/// `postgres` and `template1` databases by default.
async fn check_postgres_system_databases(
    runtime: &Arc<dyn ContainerRuntime>,
    container: &ContainerName,
    info: &EngineInfo,
) -> anyhow::Result<()> {
    if info.engine != "postgres" {
        return Ok(());
    }
    for database in ["postgres", "template1"] {
        let command = format!(
            "{HOST}; psql -v ON_ERROR_STOP=1 postgresql://p_one:pwone1@$H/{database} -c 'SELECT 1'"
        );
        let shell = vec!["sh".to_string(), "-c".to_string(), command];
        let connected = runtime
            .exec_capture(container, shell, Vec::new())
            .await
            .is_ok();
        ensure!(!connected, "a tenant connected to the {database} database");
    }
    Ok(())
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

/// The command that succeeds once the final server takes connections over TCP. The mongo image
/// runs its initialization on a temporary server at 127.0.0.1:27017 and shuts it down before it
/// starts the final one as PID 1, so the check also requires that PID 1 is `mongod`.
fn ready_command(info: &EngineInfo, root: &str) -> String {
    match info.engine.as_str() {
        "postgres" => "pg_isready -h 127.0.0.1 -U postgres".to_string(),
        "mysql" | "mariadb" => format!("{} -h127.0.0.1 -uroot -p{root} -e 'SELECT 1'", info.engine),
        "mongodb" => format!(
            "[ \"$(cat /proc/1/comm)\" = mongod ] && mongosh 'mongodb://root:{root}@127.0.0.1/admin' --quiet --eval 'db.runCommand({{ping: 1}})'"
        ),
        _ => format!("REDISCLI_AUTH={root} redis-cli ping | grep -qx PONG"),
    }
}

async fn wait_ready(
    runtime: &Arc<dyn ContainerRuntime>,
    container: &ContainerName,
    info: &EngineInfo,
    root: &str,
) -> anyhow::Result<()> {
    let command = vec![
        "sh".to_string(),
        "-c".to_string(),
        ready_command(info, root),
    ];
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

/// After a restart of the container, the tenant and the new root password still sign in, the
/// old root password does not, and Redis still refuses a client without an account.
async fn check_restart(
    runtime: &Arc<dyn ContainerRuntime>,
    container: &ContainerName,
    info: &EngineInfo,
) -> anyhow::Result<()> {
    runtime
        .stop_container(container, Duration::from_secs(10))
        .await?;
    runtime.start_container(container).await?;
    wait_ready(runtime, container, info, "rootpw9876").await?;
    let one = tenant("p_one", "pwone2");
    ensure!(
        can_write(runtime, container, info, &one, "p_one").await,
        "the {} tenant was lost on a restart",
        info.engine
    );
    let root = |password| Login {
        user: &info.user,
        password,
        auth: "admin",
    };
    ensure!(can_write(runtime, container, info, &root("rootpw9876"), "p_one").await);
    ensure!(!can_write(runtime, container, info, &root(ROOT_PASSWORD), "p_one").await);
    if info.engine == "redis" {
        ensure!(
            !can_write(runtime, container, info, &tenant("", ""), "p_one").await,
            "Redis opened its default user without a password after a restart"
        );
    }
    Ok(())
}

/// The query of `read_command` for the SQL engines: it fails when the table is missing.
const READ_ENV: &str = "Q=SELECT count(*) FROM t";

/// The shell command that reads what `write_command` wrote into `database` as `login`. It
/// fails when the data or the account is missing.
fn read_command(info: &EngineInfo, login: &Login<'_>, database: &str) -> String {
    let Login {
        user,
        password,
        auth,
    } = login;
    let command = match info.engine.as_str() {
        // The query comes from the fixed `Q` variable of `READ_ENV`.
        "postgres" => format!(
            "psql -v ON_ERROR_STOP=1 postgresql://{user}:{password}@$H/{database} -c \"$Q\""
        ),
        "mysql" | "mariadb" => format!(
            "{} -h$H -u{user} -p{password} {database} -e \"$Q\"",
            info.engine
        ),
        "mongodb" => format!(
            "mongosh \"mongodb://{user}:{password}@$H/{database}?authSource={auth}\" --quiet --eval 'if (db.t.countDocuments() < 1) {{ quit(1); }}'"
        ),
        _ => format!(
            "redis-cli -h $H --no-auth-warning --user {user} --pass {password} get {database}:k | grep -qx 1"
        ),
    };
    format!("{HOST}; {command}")
}

async fn can_read(
    runtime: &Arc<dyn ContainerRuntime>,
    container: &ContainerName,
    info: &EngineInfo,
    login: &Login<'_>,
    database: &str,
) -> bool {
    let command = vec![
        "sh".to_string(),
        "-c".to_string(),
        read_command(info, login, database),
    ];
    runtime
        .exec_capture(container, command, vec![READ_ENV.to_string()])
        .await
        .is_ok()
}

/// A backup of a shared server holds the data of its tenants: restored into a new server,
/// whose users the platform then sets from its stored passwords, the tenant reads its data
/// again with its own password.
async fn check_backup_round_trip(
    runtime: &Arc<dyn ContainerRuntime>,
    name: &ContainerName,
    info: &EngineInfo,
    image: &str,
) -> anyhow::Result<()> {
    let dump = super::backup::dump_for_test(runtime, name, info, "rootpw9876").await?;
    let copy: ContainerName = format!("{name}-restore").parse()?;
    runtime.create_container(&spec(info, &copy, image)?).await?;
    let result = restore_and_read(runtime, &copy, info, dump).await;
    let removed = runtime.remove_container(&copy).await;
    result?;
    removed
}

async fn restore_and_read(
    runtime: &Arc<dyn ContainerRuntime>,
    copy: &ContainerName,
    info: &EngineInfo,
    dump: Vec<u8>,
) -> anyhow::Result<()> {
    runtime.start_container(copy).await?;
    wait_ready(runtime, copy, info, ROOT_PASSWORD).await?;
    super::backup::restore_for_test(runtime, copy, info, ROOT_PASSWORD, dump).await?;
    wait_ready(runtime, copy, info, ROOT_PASSWORD).await?;
    run_script(
        runtime,
        copy,
        create_script(info, ROOT_PASSWORD, "p_one", "pwone2")?,
    )
    .await?;
    ensure!(
        can_read(runtime, copy, info, &tenant("p_one", "pwone2"), "p_one").await,
        "the {} backup did not bring back the data of the tenant",
        info.engine
    );
    Ok(())
}

/// A restore runs into the live server, where apps keep their connections open: it replaces
/// the data over an open tenant session and the tenant reads it again afterwards.
async fn check_in_place_restore(
    runtime: &Arc<dyn ContainerRuntime>,
    name: &ContainerName,
    info: &EngineInfo,
) -> anyhow::Result<()> {
    let dump = super::backup::dump_for_test(runtime, name, info, "rootpw9876").await?;
    // A session that stays connected to the tenant database during the restore. It runs in
    // an exec of its own that the test waits for: a client left in the background would be
    // adopted by the server process, which counts its exit as a crash of the server.
    let hold = format!("{HOST}; psql postgresql://p_one:pwone2@$H/p_one -c 'SELECT pg_sleep(60)'");
    let shell = vec!["sh".to_string(), "-c".to_string(), hold];
    let session = (info.engine == "postgres").then(|| {
        let (runtime, name) = (runtime.clone(), name.clone());
        tokio::spawn(async move { runtime.exec_capture(&name, shell, Vec::new()).await })
    });
    tokio::time::sleep(Duration::from_secs(1)).await;
    super::backup::restore_for_test(runtime, name, info, "rootpw9876", dump).await?;
    if let Some(session) = session {
        ensure!(
            session.await?.is_err(),
            "the restore left the open tenant session running"
        );
    }
    wait_ready(runtime, name, info, "rootpw9876").await?;
    run_script(
        runtime,
        name,
        create_script(info, "rootpw9876", "p_one", "pwone2")?,
    )
    .await?;
    ensure!(
        can_read(runtime, name, info, &tenant("p_one", "pwone2"), "p_one").await,
        "the {} restore into the live server lost the data of the tenant",
        info.engine
    );
    Ok(())
}

/// Starts the server and runs every check on it.
async fn run_checks(
    runtime: &Arc<dyn ContainerRuntime>,
    name: &ContainerName,
    info: &EngineInfo,
    image: &str,
) -> anyhow::Result<()> {
    runtime.start_container(name).await?;
    wait_ready(runtime, name, info, ROOT_PASSWORD).await?;
    check_isolation(runtime, name, info).await?;
    check_postgres_system_databases(runtime, name, info).await?;
    check_reset_and_drop(runtime, name, info).await?;
    check_root_password(runtime, name, info).await?;
    check_restart(runtime, name, info).await?;
    check_backup_round_trip(runtime, name, info, image).await?;
    check_in_place_restore(runtime, name, info).await
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
    let result = run_checks(&runtime, &name, &info, image).await;
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
