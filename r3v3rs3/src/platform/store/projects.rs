//! Projects and environments stored in the SQLite database.

use super::{PlatformStore, StoredEnv, id_of, sql_int, time_of};
use r3v3rs3_api::id::ShortId;
use r3v3rs3_api::platform::{ProjectEnvironmentSummary, ProjectSummary};
use sqlx::Row;
use sqlx::sqlite::SqliteRow;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StoredProject {
    pub id: ShortId,
    pub name: String,
    pub description: Option<String>,
    pub created_at: u64,
    pub updated_at: u64,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StoredProjectEnvironment {
    pub id: ShortId,
    pub project_id: ShortId,
    pub name: String,
    pub description: Option<String>,
    pub created_at: u64,
    pub updated_at: u64,
}

fn project_of(row: &SqliteRow) -> anyhow::Result<StoredProject> {
    Ok(StoredProject {
        id: id_of(row, "id")?,
        name: row.try_get("name")?,
        description: row.try_get("description")?,
        created_at: time_of(row, "created_at")?,
        updated_at: time_of(row, "updated_at")?,
    })
}

fn environment_of(row: &SqliteRow) -> anyhow::Result<StoredProjectEnvironment> {
    Ok(StoredProjectEnvironment {
        id: id_of(row, "id")?,
        project_id: id_of(row, "project_id")?,
        name: row.try_get("name")?,
        description: row.try_get("description")?,
        created_at: time_of(row, "created_at")?,
        updated_at: time_of(row, "updated_at")?,
    })
}

impl PlatformStore {
    /// Lists all projects with their environment counts.
    pub async fn list_projects(&self) -> anyhow::Result<Vec<ProjectSummary>> {
        let rows = sqlx::query(
            "SELECT p.id, p.name, p.description, p.created_at, p.updated_at,
                    COUNT(DISTINCT e.id) as environments_count
             FROM projects p
             LEFT JOIN project_environments e ON p.id = e.project_id
             GROUP BY p.id
             ORDER BY p.name ASC",
        )
        .fetch_all(&self.pool)
        .await?;
        rows.iter()
            .map(|row| {
                let env_count: i64 = row.try_get("environments_count")?;
                Ok(ProjectSummary {
                    id: id_of(row, "id")?,
                    name: row.try_get("name")?,
                    description: row.try_get("description")?,
                    environments_count: env_count as usize,
                    apps_count: 0,
                    created_at: time_of(row, "created_at")?,
                    updated_at: time_of(row, "updated_at")?,
                })
            })
            .collect()
    }

    /// Retrieves a project by ID.
    pub async fn project(&self, id: ShortId) -> anyhow::Result<Option<StoredProject>> {
        let row = sqlx::query(
            "SELECT id, name, description, created_at, updated_at
             FROM projects
             WHERE id = ?",
        )
        .bind(id.to_string())
        .fetch_optional(&self.pool)
        .await?;
        row.as_ref().map(project_of).transpose()
    }

    /// Retrieves a project by name.
    pub async fn project_by_name(&self, name: &str) -> anyhow::Result<Option<StoredProject>> {
        let row = sqlx::query(
            "SELECT id, name, description, created_at, updated_at
             FROM projects
             WHERE name = ?",
        )
        .bind(name)
        .fetch_optional(&self.pool)
        .await?;
        row.as_ref().map(project_of).transpose()
    }

    /// Inserts a new project and optional initial environments.
    pub async fn insert_project(
        &self,
        project: &StoredProject,
        environments: &[StoredProjectEnvironment],
    ) -> anyhow::Result<()> {
        let mut tx = self.pool.begin().await?;
        sqlx::query(
            "INSERT INTO projects (id, name, description, created_at, updated_at)
             VALUES (?, ?, ?, ?, ?)",
        )
        .bind(project.id.to_string())
        .bind(&project.name)
        .bind(&project.description)
        .bind(sql_int(project.created_at))
        .bind(sql_int(project.updated_at))
        .execute(&mut *tx)
        .await?;

        for env in environments {
            sqlx::query(
                "INSERT INTO project_environments (id, project_id, name, description, created_at, updated_at)
                 VALUES (?, ?, ?, ?, ?, ?)",
            )
            .bind(env.id.to_string())
            .bind(env.project_id.to_string())
            .bind(&env.name)
            .bind(&env.description)
            .bind(sql_int(env.created_at))
            .bind(sql_int(env.updated_at))
            .execute(&mut *tx)
            .await?;
        }

        tx.commit().await?;
        Ok(())
    }

    /// Updates a project's name and description.
    pub async fn update_project(&self, project: &StoredProject) -> anyhow::Result<bool> {
        let res = sqlx::query(
            "UPDATE projects
             SET name = ?, description = ?, updated_at = ?
             WHERE id = ?",
        )
        .bind(&project.name)
        .bind(&project.description)
        .bind(sql_int(project.updated_at))
        .bind(project.id.to_string())
        .execute(&self.pool)
        .await?;
        Ok(res.rows_affected() > 0)
    }

    /// Deletes a project and cascades environments.
    pub async fn delete_project(&self, id: ShortId) -> anyhow::Result<bool> {
        let res = sqlx::query("DELETE FROM projects WHERE id = ?")
            .bind(id.to_string())
            .execute(&self.pool)
            .await?;
        Ok(res.rows_affected() > 0)
    }

    /// Lists all environments of a project with variable counts.
    pub async fn list_environments(
        &self,
        project_id: ShortId,
    ) -> anyhow::Result<Vec<ProjectEnvironmentSummary>> {
        let rows = sqlx::query(
            "SELECT e.id, e.project_id, e.name, e.description, e.created_at, e.updated_at,
                    COUNT(v.key) as variable_count
             FROM project_environments e
             LEFT JOIN project_env_vars v ON e.id = v.environment_id
             WHERE e.project_id = ?
             GROUP BY e.id
             ORDER BY e.name ASC",
        )
        .bind(project_id.to_string())
        .fetch_all(&self.pool)
        .await?;
        rows.iter()
            .map(|row| {
                let count: i64 = row.try_get("variable_count")?;
                Ok(ProjectEnvironmentSummary {
                    id: id_of(row, "id")?,
                    project_id: id_of(row, "project_id")?,
                    name: row.try_get("name")?,
                    description: row.try_get("description")?,
                    variable_count: count as usize,
                    apps_count: 0,
                    created_at: time_of(row, "created_at")?,
                    updated_at: time_of(row, "updated_at")?,
                })
            })
            .collect()
    }

    /// Retrieves an environment by ID.
    pub async fn environment(
        &self,
        id: ShortId,
    ) -> anyhow::Result<Option<StoredProjectEnvironment>> {
        let row = sqlx::query(
            "SELECT id, project_id, name, description, created_at, updated_at
             FROM project_environments
             WHERE id = ?",
        )
        .bind(id.to_string())
        .fetch_optional(&self.pool)
        .await?;
        row.as_ref().map(environment_of).transpose()
    }

    /// Retrieves an environment by name within a project.
    pub async fn environment_by_name(
        &self,
        project_id: ShortId,
        name: &str,
    ) -> anyhow::Result<Option<StoredProjectEnvironment>> {
        let row = sqlx::query(
            "SELECT id, project_id, name, description, created_at, updated_at
             FROM project_environments
             WHERE project_id = ? AND name = ?",
        )
        .bind(project_id.to_string())
        .bind(name)
        .fetch_optional(&self.pool)
        .await?;
        row.as_ref().map(environment_of).transpose()
    }

    /// Inserts a new environment into a project.
    pub async fn insert_environment(&self, env: &StoredProjectEnvironment) -> anyhow::Result<()> {
        sqlx::query(
            "INSERT INTO project_environments (id, project_id, name, description, created_at, updated_at)
             VALUES (?, ?, ?, ?, ?, ?)",
        )
        .bind(env.id.to_string())
        .bind(env.project_id.to_string())
        .bind(&env.name)
        .bind(&env.description)
        .bind(sql_int(env.created_at))
        .bind(sql_int(env.updated_at))
        .execute(&self.pool)
        .await?;
        Ok(())
    }

    /// Updates an environment's name and description.
    pub async fn update_environment(&self, env: &StoredProjectEnvironment) -> anyhow::Result<bool> {
        let res = sqlx::query(
            "UPDATE project_environments
             SET name = ?, description = ?, updated_at = ?
             WHERE id = ?",
        )
        .bind(&env.name)
        .bind(&env.description)
        .bind(sql_int(env.updated_at))
        .bind(env.id.to_string())
        .execute(&self.pool)
        .await?;
        Ok(res.rows_affected() > 0)
    }

    /// Deletes an environment.
    pub async fn delete_environment(&self, id: ShortId) -> anyhow::Result<bool> {
        let res = sqlx::query("DELETE FROM project_environments WHERE id = ?")
            .bind(id.to_string())
            .execute(&self.pool)
            .await?;
        Ok(res.rows_affected() > 0)
    }

    /// Retrieves the environment variables of an environment.
    pub async fn environment_vars(&self, env_id: ShortId) -> anyhow::Result<Vec<StoredEnv>> {
        let rows = sqlx::query(
            "SELECT key, value, secret
             FROM project_env_vars
             WHERE environment_id = ?
             ORDER BY key ASC",
        )
        .bind(env_id.to_string())
        .fetch_all(&self.pool)
        .await?;
        rows.iter()
            .map(|row| {
                Ok(StoredEnv {
                    key: row.try_get("key")?,
                    sealed: row.try_get("value")?,
                    secret: row.try_get("secret")?,
                })
            })
            .collect()
    }

    /// Replaces the environment variables of an environment.
    pub async fn replace_environment_vars(
        &self,
        env_id: ShortId,
        vars: &[StoredEnv],
    ) -> anyhow::Result<()> {
        let mut tx = self.pool.begin().await?;
        sqlx::query("DELETE FROM project_env_vars WHERE environment_id = ?")
            .bind(env_id.to_string())
            .execute(&mut *tx)
            .await?;

        for var in vars {
            sqlx::query(
                "INSERT INTO project_env_vars (environment_id, key, value, secret)
                 VALUES (?, ?, ?, ?)",
            )
            .bind(env_id.to_string())
            .bind(&var.key)
            .bind(&var.sealed)
            .bind(var.secret)
            .execute(&mut *tx)
            .await?;
        }

        tx.commit().await?;
        Ok(())
    }
}
