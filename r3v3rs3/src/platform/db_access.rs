//! The accounts of a database: the root account and the tenants of a shared server. An
//! administrator reads them with their passwords, gives any of them a new password, and
//! removes a tenant that no app uses.

use super::Platform;
use super::tenant::{
    self, Credentials, TENANT_LOCK, create_script, drop_script, new_password, root_password_script,
    run_script, secret_name,
};
use anyhow::Context as _;
use r3v3rs3_api::error::Error;
use r3v3rs3_api::id::ShortId;
use r3v3rs3_api::platform::{
    AppEntry, DatabaseAccount, DatabaseCredentials, DatabasePasswordReset, DatabaseTenant,
    DeploymentTrigger, EngineInfo, EnvEntry,
};
use std::sync::Arc;
use tracing::error;

/// The host that the connection strings of the credentials name: the r3v3rs3 server itself.
const LOCAL_HOST: &str = "127.0.0.1";

/// A database app with its engine and its host port.
struct Database {
    app: AppEntry,
    info: EngineInfo,
    host_port: u16,
}

impl Database {
    fn account(&self, tenant: Option<&str>, credentials: Credentials) -> DatabaseAccount {
        let url = super::deploy::format_url(&self.info, LOCAL_HOST, self.host_port, &credentials);
        DatabaseAccount {
            tenant: tenant.map(String::from),
            user: credentials.user,
            password: credentials.password,
            database: credentials.database,
            key_prefix: credentials.key_prefix,
            url,
        }
    }
}

/// The apps that connect to a database as a tenant.
fn tenant_apps(apps: &[AppEntry], db_id: ShortId, name: &str) -> Vec<ShortId> {
    apps.iter()
        .filter(|app| app.spec.uses_database == Some(db_id))
        .filter(|app| {
            tenant::tenant_name(app.id, app.spec.project_id, app.spec.environment_id) == name
        })
        .map(|app| app.id)
        .collect()
}

/// The apps that connect to a database as its root account: every app of a database of a
/// project, and no app of a shared server.
fn root_apps(apps: &[AppEntry], db: &AppEntry) -> Vec<ShortId> {
    if tenant::is_shared(db) {
        return Vec::new();
    }
    apps.iter()
        .filter(|app| app.spec.uses_database == Some(db.id))
        .map(|app| app.id)
        .collect()
}

fn tenant_not_found(name: &str) -> anyhow::Error {
    Error::IdNotFound {
        id: name.to_string(),
    }
    .into()
}

impl Platform {
    async fn database(&self, db_id: ShortId) -> anyhow::Result<Database> {
        let app = self.app(db_id).await?;
        let (Some(info), Some(host_port)) = (
            r3v3rs3_api::platform::app_engine_info(&app.spec),
            app.spec.host_port,
        ) else {
            return Err(super::not_found(db_id));
        };
        Ok(Database {
            app,
            info,
            host_port,
        })
    }

    /// The root account and every tenant of a database, with their passwords.
    pub async fn database_accounts(&self, db_id: ShortId) -> anyhow::Result<DatabaseCredentials> {
        let db = self.database(db_id).await?;
        let root_password = self.database_password(db_id, &db.info).await?;
        let root = db.account(None, Credentials::root(&db.info, root_password));
        let mut tenants = Vec::new();
        for name in self.tenants(db_id).await? {
            let password = self
                .tenant_password(db_id, &name)
                .await?
                .with_context(|| format!("the password of the tenant {name} is missing"))?;
            let credentials = Credentials::tenant(&db.info, &name, password);
            tenants.push(db.account(Some(&name), credentials));
        }
        Ok(DatabaseCredentials { root, tenants })
    }

    /// The tenants of a database and the apps that connect as each of them.
    pub async fn database_tenants(&self, db_id: ShortId) -> anyhow::Result<Vec<DatabaseTenant>> {
        self.database(db_id).await?;
        let apps = self.store.apps().await?;
        Ok(self
            .tenants(db_id)
            .await?
            .into_iter()
            .map(|name| DatabaseTenant {
                apps: tenant_apps(&apps, db_id, &name),
                name,
            })
            .collect())
    }

    /// Gives the root account (`tenant` is `None`) or a tenant a new password, and starts a
    /// new deployment of every app that connects with it.
    pub async fn reset_database_password(
        self: &Arc<Self>,
        db_id: ShortId,
        tenant: Option<&str>,
        username: &str,
    ) -> anyhow::Result<DatabasePasswordReset> {
        let db = self.database(db_id).await?;
        let apps = self.store.apps().await?;
        let (account, affected) = match tenant {
            Some(name) => {
                let credentials = self.reset_tenant_password(&db, name).await?;
                (
                    db.account(Some(name), credentials),
                    tenant_apps(&apps, db_id, name),
                )
            }
            None => {
                let credentials = self.reset_root_password(&db).await?;
                (db.account(None, credentials), root_apps(&apps, &db.app))
            }
        };
        let (redeployed, failed) = self.redeploy(&affected, username).await;
        Ok(DatabasePasswordReset {
            account,
            redeployed,
            failed,
        })
    }

    /// Sets the new password in the database first. When the store fails after that, the next
    /// deployment of an app sets the stored password again.
    async fn reset_tenant_password(
        &self,
        db: &Database,
        name: &str,
    ) -> anyhow::Result<Credentials> {
        let _guard = TENANT_LOCK.lock().await;
        if self.tenant_password(db.app.id, name).await?.is_none() {
            return Err(tenant_not_found(name));
        }
        let (_, container, runtime) = self.database_container(db.app.id).await?;
        let root = self.database_password(db.app.id, &db.info).await?;
        let password = new_password();
        let script = create_script(&db.info, &root, name, &password)?;
        run_script(&runtime, &container, script).await?;
        self.store_tenant_password(db.app.id, name, &password)
            .await?;
        Ok(Credentials::tenant(&db.info, name, password))
    }

    /// Stores the new root password first and puts the old one back when the database does
    /// not take the new one, so the stored password always opens the database.
    async fn reset_root_password(&self, db: &Database) -> anyhow::Result<Credentials> {
        if db.info.password_env.is_empty() {
            return Err(Error::InvalidContainerSpec {
                reason: format!("the engine {} has no root password", db.info.engine),
            }
            .into());
        }
        // Every tenant script signs in with the root password.
        let _guard = TENANT_LOCK.lock().await;
        let (_, container, runtime) = self.database_container(db.app.id).await?;
        let old = self.database_password(db.app.id, &db.info).await?;
        let password = new_password();
        let script = root_password_script(&db.info, &old, &password)?;
        self.set_root_password(db, &password).await?;
        if let Err(err) = run_script(&runtime, &container, script).await {
            if let Err(restore) = self.set_root_password(db, &old).await {
                error!(app = %db.app.id, "failed to restore the root password: {restore:#}");
            }
            return Err(err);
        }
        Ok(Credentials::root(&db.info, password))
    }

    /// Replaces the value of the password variable of the engine and keeps the others.
    async fn set_root_password(&self, db: &Database, password: &str) -> anyhow::Result<()> {
        let mut entries = self.env(db.app.id).await?;
        entries.retain(|entry| entry.key.as_str() != db.info.password_env);
        entries.push(EnvEntry {
            key: db.info.password_env.parse()?,
            value: Some(password.to_string()),
            secret: true,
        });
        self.set_env(db.app.id, entries).await
    }

    /// Removes the user and the database of a tenant with its data. A tenant that an app
    /// still connects as stays.
    pub async fn delete_database_tenant(&self, db_id: ShortId, name: &str) -> anyhow::Result<()> {
        let db = self.database(db_id).await?;
        let _guard = TENANT_LOCK.lock().await;
        if self.tenant_password(db_id, name).await?.is_none() {
            return Err(tenant_not_found(name));
        }
        let apps = self.store.apps().await?;
        if !tenant_apps(&apps, db_id, name).is_empty() {
            return Err(Error::DatabaseInUse {
                name: name.to_string(),
            }
            .into());
        }
        let (_, container, runtime) = self.database_container(db_id).await?;
        let root = self.database_password(db_id, &db.info).await?;
        run_script(&runtime, &container, drop_script(&db.info, &root, name)?).await?;
        self.store.delete_secret(db_id, &secret_name(name)).await
    }

    /// Starts a new deployment of each app. A failure goes to the log and to the list of
    /// failed apps; the others still deploy.
    async fn redeploy(
        self: &Arc<Self>,
        apps: &[ShortId],
        username: &str,
    ) -> (Vec<ShortId>, Vec<ShortId>) {
        let mut redeployed = Vec::new();
        let mut failed = Vec::new();
        for &app in apps {
            match self.deploy(app, username, DeploymentTrigger::Manual).await {
                Ok(_) => redeployed.push(app),
                Err(err) => {
                    error!(%app, "failed to redeploy after a password reset: {err:#}");
                    failed.push(app);
                }
            }
        }
        (redeployed, failed)
    }
}
