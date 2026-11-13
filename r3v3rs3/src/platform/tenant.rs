//! The tenants of a shared database server.
//!
//! A database without a project is a shared server: every app may use it. Each project
//! environment that connects gets its own database and its own user on the server, so one
//! project cannot read the data of another. An app without a project gets a tenant of its own.
//! The password of a tenant is sealed in the `app_secrets` row `tenant:<name>` of the database.

use super::{Platform, secret_aad};
use crate::runtime::ContainerRuntime;
use anyhow::{Context as _, bail};
use r3v3rs3_api::container::ContainerName;
use r3v3rs3_api::id::ShortId;
use r3v3rs3_api::platform::{AppEntry, EngineInfo};
use std::sync::Arc;
use tracing::{error, warn};

/// The prefix of the `app_secrets` rows that hold the passwords of the tenants.
pub(crate) const TENANT_SECRET_PREFIX: &str = "tenant:";

/// The file inside the database container that a tenant script is uploaded to.
const SCRIPT_PATH: &str = "/tmp/r3v3rs3-tenant";

/// The longest tenant name; PostgreSQL cuts identifiers at 63 bytes.
const MAX_TENANT_NAME: usize = 63;

/// The command that writes the users of a Redis server into its ACL file, so they survive a
/// restart of the container.
const REDIS_SAVE: &str = "ACL SAVE\n";

/// The answer of `ACL SAVE` on a Redis that started without an ACL file: a container from
/// before the file, until its next deployment.
const REDIS_NO_ACL_FILE: &str = "not configured to use an ACL file";

/// One tenant script at a time, so two deployments of one project environment cannot create
/// its user with two passwords.
pub(super) static TENANT_LOCK: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());

/// The account that a connection to a database uses.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Credentials {
    pub user: String,
    pub password: String,
    /// The database of the connection. Empty for an engine without databases, such as Redis.
    pub database: String,
    /// Whether the connection string ends in the database name.
    pub database_in_url: bool,
    /// The prefix that the keys of the tenant must start with, for Redis.
    pub key_prefix: Option<String>,
}

impl Credentials {
    /// The root account of a database, which a database of a project gives to its apps. A
    /// Redis from before the password has no account to sign in with, so its connection names
    /// no user.
    pub fn root(info: &EngineInfo, password: String) -> Self {
        Self {
            user: if password.is_empty() {
                String::new()
            } else {
                info.user.clone()
            },
            password,
            database: info.default_db.clone(),
            database_in_url: info.url_database,
            key_prefix: None,
        }
    }

    /// The account of a tenant on a shared server.
    pub fn tenant(info: &EngineInfo, tenant: &str, password: String) -> Self {
        let redis = info.engine == "redis";
        Self {
            user: tenant.to_string(),
            password,
            database: if redis { String::new() } else { tenant.into() },
            database_in_url: !redis,
            key_prefix: redis.then(|| format!("{tenant}:")),
        }
    }
}

/// Whether a database is a shared server: a database without a project.
pub fn is_shared(db: &AppEntry) -> bool {
    db.spec.database.is_some() && db.spec.project_id.is_none()
}

/// The tenant of an app: its project environment, or the app itself without a project. The
/// name is built from ids, so renaming a project does not move its data.
pub fn tenant_name(app: ShortId, project: Option<ShortId>, environment: Option<ShortId>) -> String {
    let part = |id: ShortId| id.to_string().to_ascii_lowercase().replace('-', "_");
    match (project, environment) {
        (Some(project), Some(environment)) => {
            format!("p_{}_{}", part(project), part(environment))
        }
        (Some(project), None) => format!("p_{}", part(project)),
        _ => format!("a_{}", part(app)),
    }
}

/// Whether a text is a tenant name: a lowercase letter, then lowercase letters, digits and
/// underscores. Every script quotes the name, but only such a name ever reaches one.
pub fn is_tenant_name(name: &str) -> bool {
    name.len() <= MAX_TENANT_NAME
        && name.starts_with(|c: char| c.is_ascii_lowercase())
        && name
            .chars()
            .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '_')
}

/// A new password: 32 hex characters, like the root password of a database.
pub(crate) fn new_password() -> String {
    format!("{:032x}", rand::random::<u128>())
}

/// What the database container runs for a tenant: a script file and the command that reads
/// it. The passwords travel in the file and in the environment, never on the command line.
#[derive(Debug)]
pub struct TenantScript {
    pub path: String,
    pub content: String,
    pub command: Vec<String>,
    pub env: Vec<String>,
    /// The client reports a failed command only on its output, not in its exit code.
    pub errors_in_output: bool,
}

/// The script that creates the user and the database of a tenant, or sets the password of an
/// existing one. It runs again on every connection, so it repairs a dropped user.
pub(crate) fn create_script(
    info: &EngineInfo,
    root_password: &str,
    tenant: &str,
    password: &str,
) -> anyhow::Result<TenantScript> {
    check_script_values(tenant, password)?;
    let content = match info.engine.as_str() {
        "postgres" => postgres_create(tenant, password),
        "mysql" | "mariadb" => mysql_create(tenant, password),
        "mongodb" => mongodb_create(tenant, password),
        "redis" => redis_create(tenant, password),
        other => bail!("the engine {other} has no tenants"),
    };
    script(info, root_password, content)
}

/// The script that removes the user and the database of a tenant, with its data.
pub fn drop_script(
    info: &EngineInfo,
    root_password: &str,
    tenant: &str,
) -> anyhow::Result<TenantScript> {
    check_script_values(tenant, "")?;
    let content = match info.engine.as_str() {
        "postgres" => format!(
            "DROP DATABASE IF EXISTS \"{tenant}\" WITH (FORCE);\nDROP ROLE IF EXISTS \"{tenant}\";\n"
        ),
        "mysql" | "mariadb" => {
            format!("DROP DATABASE IF EXISTS `{tenant}`;\nDROP USER IF EXISTS '{tenant}'@'%';\n")
        }
        "mongodb" => format!(
            "const d = db.getSiblingDB('{tenant}');\nif (d.getUser('{tenant}')) {{ d.dropUser('{tenant}'); }}\nd.dropDatabase();\n"
        ),
        "redis" => format!("ACL DELUSER {tenant}\n{REDIS_SAVE}"),
        other => bail!("the engine {other} has no tenants"),
    };
    script(info, root_password, content)
}

/// The script that gives the root account of a database a new password. The data of the
/// server keeps the root password, so a new value of the password variable alone changes
/// nothing.
pub(crate) fn root_password_script(
    info: &EngineInfo,
    root_password: &str,
    password: &str,
) -> anyhow::Result<TenantScript> {
    check_script_values(&info.user, password)?;
    let user = &info.user;
    let content = match info.engine.as_str() {
        "postgres" => format!("ALTER ROLE \"{user}\" WITH PASSWORD '{password}';\n"),
        // The image creates the root user for `%` and for `localhost`.
        "mysql" | "mariadb" => format!(
            "ALTER USER IF EXISTS '{user}'@'%' IDENTIFIED BY '{password}';\n\
             ALTER USER IF EXISTS '{user}'@'localhost' IDENTIFIED BY '{password}';\n\
             FLUSH PRIVILEGES;\n"
        ),
        "mongodb" => {
            format!("db.getSiblingDB('admin').changeUserPassword('{user}', '{password}');\n")
        }
        // The ACL file keeps the new password of the default user across a restart.
        "redis" => format!("CONFIG SET requirepass {password}\n{REDIS_SAVE}"),
        other => bail!("the engine {other} has no root password"),
    };
    script(info, root_password, content)
}

/// A tenant name and a generated password hold only characters that need no quoting, which
/// keeps them out of reach of an injection into a script.
fn check_script_values(tenant: &str, password: &str) -> anyhow::Result<()> {
    if !is_tenant_name(tenant) {
        bail!("{tenant} is not a tenant name");
    }
    if !password.chars().all(|c| c.is_ascii_alphanumeric()) {
        bail!("a tenant password holds letters and digits only");
    }
    Ok(())
}

fn postgres_create(tenant: &str, password: &str) -> String {
    format!(
        "SELECT 'CREATE ROLE \"{tenant}\" LOGIN' WHERE NOT EXISTS (SELECT FROM pg_roles WHERE rolname = '{tenant}')\\gexec\n\
         ALTER ROLE \"{tenant}\" WITH LOGIN PASSWORD '{password}';\n\
         SELECT 'CREATE DATABASE \"{tenant}\" OWNER \"{tenant}\"' WHERE NOT EXISTS (SELECT FROM pg_database WHERE datname = '{tenant}')\\gexec\n\
         REVOKE ALL ON DATABASE \"{tenant}\" FROM PUBLIC;\n"
    )
}

fn mysql_create(tenant: &str, password: &str) -> String {
    format!(
        "CREATE DATABASE IF NOT EXISTS `{tenant}`;\n\
         CREATE USER IF NOT EXISTS '{tenant}'@'%' IDENTIFIED BY '{password}';\n\
         ALTER USER '{tenant}'@'%' IDENTIFIED BY '{password}';\n\
         GRANT ALL PRIVILEGES ON `{tenant}`.* TO '{tenant}'@'%';\n\
         FLUSH PRIVILEGES;\n"
    )
}

fn mongodb_create(tenant: &str, password: &str) -> String {
    format!(
        "const d = db.getSiblingDB('{tenant}');\n\
         if (d.getUser('{tenant}')) {{\n  d.updateUser('{tenant}', {{ pwd: '{password}' }});\n}} else {{\n  \
         d.createUser({{ user: '{tenant}', pwd: '{password}', roles: [{{ role: 'dbOwner', db: '{tenant}' }}] }});\n}}\n"
    )
}

/// A Redis tenant reaches only the keys and channels under its prefix, and no command of
/// the administration of the server.
fn redis_create(tenant: &str, password: &str) -> String {
    format!(
        "ACL SETUSER {tenant} reset on >{password} ~{tenant}:* &{tenant}:* +@all -@admin -@dangerous\n{REDIS_SAVE}"
    )
}

/// The command and the environment that run a script with the root account of the engine.
fn script(info: &EngineInfo, root_password: &str, content: String) -> anyhow::Result<TenantScript> {
    let (path, shell, env) = match info.engine.as_str() {
        "postgres" => (
            format!("{SCRIPT_PATH}.sql"),
            format!(
                "psql -v ON_ERROR_STOP=1 -U {} -d {} -f {SCRIPT_PATH}.sql",
                info.user, info.default_db
            ),
            vec![format!("PGPASSWORD={root_password}")],
        ),
        "mysql" => (
            format!("{SCRIPT_PATH}.sql"),
            format!("mysql -u {} < {SCRIPT_PATH}.sql", info.user),
            vec![format!("MYSQL_PWD={root_password}")],
        ),
        "mariadb" => (
            format!("{SCRIPT_PATH}.sql"),
            format!("mariadb -u {} < {SCRIPT_PATH}.sql", info.user),
            vec![format!("MYSQL_PWD={root_password}")],
        ),
        "mongodb" => (
            format!("{SCRIPT_PATH}.js"),
            format!("mongosh \"$MONGO_URI\" --quiet --file {SCRIPT_PATH}.js"),
            vec![format!(
                "MONGO_URI={}",
                super::backup::mongodb_uri(&info.user, root_password)?
            )],
        ),
        // Redis answers an error of a command on its output and still exits with 0.
        "redis" => (
            format!("{SCRIPT_PATH}.redis"),
            format!("redis-cli < {SCRIPT_PATH}.redis"),
            super::backup::redis_auth(root_password),
        ),
        other => bail!("the engine {other} has no tenants"),
    };
    Ok(TenantScript {
        path,
        content,
        command: vec!["sh".into(), "-c".into(), shell],
        env,
        errors_in_output: info.engine == "redis",
    })
}

/// Uploads the script, runs it and removes it again, also after a failure.
pub(super) async fn run_script(
    runtime: &Arc<dyn ContainerRuntime>,
    container: &ContainerName,
    script: TenantScript,
) -> anyhow::Result<()> {
    runtime
        .upload_file(container, &script.path, script.content.into_bytes())
        .await?;
    let outcome = runtime
        .exec_capture(container, script.command, script.env)
        .await;
    if let Err(err) = runtime.remove_path(container, &script.path).await {
        error!(%container, "failed to remove the tenant script: {err:#}");
    }
    let output = outcome?;
    if script.errors_in_output {
        check_redis_output(container, &String::from_utf8_lossy(&output))?;
    }
    Ok(())
}

/// Fails on an error line of `redis-cli`. A Redis that started without an ACL file cannot
/// save its users; the users still work until the container restarts, and the next
/// deployment of the database starts it with the file.
fn check_redis_output(container: &ContainerName, text: &str) -> anyhow::Result<()> {
    let mut errors = text.lines().filter(|line| line.starts_with("ERR"));
    if errors.clone().any(|line| !line.contains(REDIS_NO_ACL_FILE)) {
        bail!("the tenant script failed: {}", text.trim());
    }
    if errors.next().is_some() {
        warn!(
            %container,
            "the Redis server has no ACL file and loses its users on a restart; deploy the database again"
        );
    }
    Ok(())
}

pub(super) fn secret_name(tenant: &str) -> String {
    format!("{TENANT_SECRET_PREFIX}{tenant}")
}

impl Platform {
    /// The account of a tenant on a shared database. The first call creates the user and the
    /// database with a new password; every call sets them again, so a server that lost its
    /// users, like Redis after a restart, gets them back.
    pub(crate) async fn ensure_tenant(
        &self,
        db_id: ShortId,
        tenant: &str,
    ) -> anyhow::Result<Credentials> {
        let _guard = TENANT_LOCK.lock().await;
        let (db, container, runtime) = self.database_container(db_id).await?;
        let info = r3v3rs3_api::platform::app_engine_info(&db.spec)
            .context("the engine is not in the database catalog")?;
        let root = self.database_password(db_id, &info).await?;
        let password = match self.tenant_password(db_id, tenant).await? {
            Some(password) => password,
            None => new_password(),
        };
        run_script(
            &runtime,
            &container,
            create_script(&info, &root, tenant, &password)?,
        )
        .await?;
        self.store_tenant_password(db_id, tenant, &password).await?;
        Ok(Credentials::tenant(&info, tenant, password))
    }

    /// The names of the tenants of a database.
    pub async fn tenants(&self, db_id: ShortId) -> anyhow::Result<Vec<String>> {
        let names = self.store.secret_names(db_id, TENANT_SECRET_PREFIX).await?;
        Ok(names
            .into_iter()
            .filter_map(|name| name.strip_prefix(TENANT_SECRET_PREFIX).map(String::from))
            .collect())
    }

    /// The stored password of a tenant.
    pub(crate) async fn tenant_password(
        &self,
        db_id: ShortId,
        tenant: &str,
    ) -> anyhow::Result<Option<String>> {
        let name = secret_name(tenant);
        let Some(sealed) = self.store.secret(db_id, &name).await? else {
            return Ok(None);
        };
        let value = self.keys().open(&secret_aad(db_id, &name), &sealed)?;
        Ok(Some(
            String::from_utf8(value).context("a tenant password is not UTF-8")?,
        ))
    }

    pub(super) async fn store_tenant_password(
        &self,
        db_id: ShortId,
        tenant: &str,
        password: &str,
    ) -> anyhow::Result<()> {
        let name = secret_name(tenant);
        let sealed = self
            .keys()
            .seal(&secret_aad(db_id, &name), password.as_bytes())?;
        self.store.set_secret(db_id, &name, &sealed).await
    }

    /// Sets the users of every tenant again after a deployment of a shared database, because a
    /// new container of an engine that keeps its users in memory, like Redis, starts without
    /// them. A failure only reaches the log: the next deployment of the app repeats it.
    pub(super) async fn restore_tenants(&self, db_id: ShortId) {
        let tenants = match self.tenants(db_id).await {
            Ok(tenants) => tenants,
            Err(err) => {
                error!(app = %db_id, "failed to list the tenants: {err:#}");
                return;
            }
        };
        for tenant in tenants {
            if let Err(err) = self.ensure_tenant(db_id, &tenant).await {
                error!(app = %db_id, %tenant, "failed to restore the tenant: {err:#}");
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn info(engine: &str) -> EngineInfo {
        let catalog: r3v3rs3_api::platform::EnginesCatalog =
            serde_json::from_str(include_str!("../../data/engines.json")).unwrap();
        catalog
            .engines
            .into_iter()
            .find(|info| info.engine == engine)
            .unwrap()
    }

    fn id(text: &str) -> ShortId {
        text.parse().unwrap()
    }

    #[test]
    fn a_project_environment_shares_one_tenant_and_an_app_without_a_project_has_its_own() {
        let project = Some(id("prj-abc"));
        let environment = Some(id("env-xyz"));
        // The web and the worker of one project environment see the same data.
        assert_eq!(
            tenant_name(id("web-one"), project, environment),
            tenant_name(id("wrk-two"), project, environment)
        );
        assert_eq!(
            tenant_name(id("web-one"), project, environment),
            "p_prj_abc_env_xyz"
        );
        assert_ne!(
            tenant_name(id("web-one"), project, Some(id("env-stg"))),
            tenant_name(id("web-one"), project, environment)
        );
        assert_eq!(tenant_name(id("app-one"), None, None), "a_app_one");
        assert!(is_tenant_name(&tenant_name(id("app-one"), None, None)));
    }

    #[test]
    fn a_name_that_could_break_out_of_a_script_is_not_a_tenant() {
        for name in ["", "1abc", "a-b", "a'b", "a\"b", "a b", "A", "a;drop"] {
            assert!(!is_tenant_name(name), "{name}");
        }
        assert!(!is_tenant_name(&"a".repeat(64)));
        let info = info("postgres");
        assert!(create_script(&info, "root", "a'b", "pw").is_err());
        assert!(create_script(&info, "root", "tenant", "pw'; DROP").is_err());
    }

    #[test]
    fn every_engine_creates_a_tenant_without_a_password_on_the_command_line() {
        for engine in ["postgres", "mysql", "mariadb", "mongodb", "redis"] {
            let info = info(engine);
            let script = create_script(&info, "rootpw123", "p_shop_prod", "tenantpw456").unwrap();
            let command = script.command.join(" ");
            assert!(!command.contains("tenantpw456"), "{engine}: {command}");
            assert!(!command.contains("rootpw123"), "{engine}: {command}");
            assert!(script.content.contains("tenantpw456"), "{engine}");
            assert!(script.content.contains("p_shop_prod"), "{engine}");
            assert!(command.contains(&script.path), "{engine}: {command}");
        }
    }

    #[test]
    fn a_postgres_tenant_owns_its_database_and_other_roles_cannot_connect() {
        let script = create_script(&info("postgres"), "root", "p_shop", "pw1").unwrap();
        assert!(
            script
                .content
                .contains("CREATE DATABASE \"p_shop\" OWNER \"p_shop\"")
        );
        assert!(
            script
                .content
                .contains("REVOKE ALL ON DATABASE \"p_shop\" FROM PUBLIC")
        );
        assert!(
            script
                .content
                .contains("ALTER ROLE \"p_shop\" WITH LOGIN PASSWORD 'pw1'")
        );
        assert_eq!(script.env, ["PGPASSWORD=root"]);
    }

    #[test]
    fn a_mysql_tenant_gets_rights_on_its_own_database_only() {
        let script = create_script(&info("mysql"), "root", "p_shop", "pw1").unwrap();
        assert!(
            script
                .content
                .contains("GRANT ALL PRIVILEGES ON `p_shop`.* TO 'p_shop'@'%'")
        );
        assert!(!script.content.contains("*.*"));
        assert_eq!(script.env, ["MYSQL_PWD=root"]);
        let maria = create_script(&info("mariadb"), "root", "p_shop", "pw1").unwrap();
        assert!(maria.command.join(" ").contains("mariadb -u root"));
    }

    #[test]
    fn a_redis_tenant_reaches_only_the_keys_under_its_prefix() {
        let script = create_script(&info("redis"), "", "p_shop", "pw1").unwrap();
        assert_eq!(
            script.content,
            "ACL SETUSER p_shop reset on >pw1 ~p_shop:* &p_shop:* +@all -@admin -@dangerous\nACL SAVE\n"
        );
        let credentials = Credentials::tenant(&info("redis"), "p_shop", "pw1".into());
        assert_eq!(credentials.key_prefix.as_deref(), Some("p_shop:"));
        assert!(credentials.database.is_empty());
    }

    #[test]
    fn a_tenant_connects_to_its_own_database() {
        let credentials = Credentials::tenant(&info("mysql"), "p_shop", "pw1".into());
        assert_eq!(credentials.user, "p_shop");
        assert_eq!(credentials.database, "p_shop");
        assert!(credentials.database_in_url);
        let root = Credentials::root(&info("mysql"), "pw0".into());
        assert_eq!(root.user, "root");
        assert!(!root.database_in_url);
    }

    #[test]
    fn a_root_password_changes_in_the_database_and_never_on_the_command_line() {
        let pg = root_password_script(&info("postgres"), "old123", "new456").unwrap();
        assert_eq!(
            pg.content,
            "ALTER ROLE \"postgres\" WITH PASSWORD 'new456';\n"
        );
        // The script signs in with the old password.
        assert_eq!(pg.env, ["PGPASSWORD=old123"]);
        let my = root_password_script(&info("mysql"), "old123", "new456").unwrap();
        assert!(my.content.contains("ALTER USER IF EXISTS 'root'@'%'"));
        assert!(
            my.content
                .contains("ALTER USER IF EXISTS 'root'@'localhost'")
        );
        let mongo = root_password_script(&info("mongodb"), "old123", "new456").unwrap();
        assert!(
            mongo
                .content
                .contains("changeUserPassword('root', 'new456')")
        );
        for script in [pg, my, mongo] {
            assert!(!script.command.join(" ").contains("new456"));
        }
        let redis = root_password_script(&info("redis"), "old123", "new456").unwrap();
        assert_eq!(redis.content, "CONFIG SET requirepass new456\nACL SAVE\n");
        assert_eq!(redis.env, ["REDISCLI_AUTH=old123"]);
        // A Redis from before the password signs in without one.
        let open = root_password_script(&info("redis"), "", "new456").unwrap();
        assert!(open.env.is_empty());
        assert!(root_password_script(&info("postgres"), "old", "x'; DROP").is_err());
    }

    #[test]
    fn dropping_a_tenant_removes_its_user_and_its_database() {
        let pg = drop_script(&info("postgres"), "root", "p_shop").unwrap();
        assert!(
            pg.content
                .contains("DROP DATABASE IF EXISTS \"p_shop\" WITH (FORCE)")
        );
        assert!(pg.content.contains("DROP ROLE IF EXISTS \"p_shop\""));
        let my = drop_script(&info("mysql"), "root", "p_shop").unwrap();
        assert!(my.content.contains("DROP USER IF EXISTS 'p_shop'@'%'"));
        let redis = drop_script(&info("redis"), "root", "p_shop").unwrap();
        assert_eq!(redis.content, "ACL DELUSER p_shop\nACL SAVE\n");
        assert!(drop_script(&info("postgres"), "root", "x'y").is_err());
    }

    #[test]
    fn a_redis_without_an_acl_file_keeps_its_users_until_a_restart() {
        let container: ContainerName = "r3v3rs3-redis".parse().unwrap();
        assert!(check_redis_output(&container, "OK\nOK\n").is_ok());
        // A container from before the ACL file sets the user but cannot save it.
        let unsaved = "OK\nERR This Redis instance is not configured to use an ACL file.\n";
        assert!(check_redis_output(&container, unsaved).is_ok());
        let refused = "ERR Error in ACL SETUSER modifier\nOK\n";
        assert!(check_redis_output(&container, refused).is_err());
        assert!(check_redis_output(&container, &format!("{refused}{unsaved}")).is_err());
    }
}
