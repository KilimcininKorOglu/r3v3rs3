//! Project and environment management for the deployment platform.

use super::{Platform, new_id, not_found};
use crate::platform::store::{StoredEnv, StoredProject, StoredProjectEnvironment};
use r3v3rs3_api::error::Error;
use r3v3rs3_api::id::ShortId;
use r3v3rs3_api::platform::{
    EnvEntry, EnvironmentEnvSpec, EnvironmentSpec, Project, ProjectEnvironment, ProjectSpec,
    ProjectSummary,
};
use std::collections::BTreeMap;

impl Platform {
    /// Lists all projects with their environment and app counts.
    pub async fn list_projects(&self) -> anyhow::Result<Vec<ProjectSummary>> {
        let mut projects = self.store.list_projects().await?;
        let apps = self.store.apps().await?;
        for p in &mut projects {
            p.apps_count = apps
                .iter()
                .filter(|a| a.spec.project_id == Some(p.id))
                .count();
        }
        Ok(projects)
    }

    /// Gets a project by ID with its environments summary.
    pub async fn project(&self, id: ShortId) -> anyhow::Result<Project> {
        let stored = self.store.project(id).await?.ok_or_else(|| not_found(id))?;
        let mut environments = self.store.list_environments(id).await?;
        let apps = self.store.apps().await?;
        for env in &mut environments {
            env.apps_count = apps
                .iter()
                .filter(|a| a.spec.environment_id == Some(env.id))
                .count();
        }
        Ok(Project {
            id: stored.id,
            name: stored.name,
            description: stored.description,
            environments,
            created_at: stored.created_at,
            updated_at: stored.updated_at,
        })
    }

    /// Creates a new project with optional default environments.
    pub async fn create_project(&self, spec: ProjectSpec, now: u64) -> anyhow::Result<Project> {
        spec.validate()?;
        if self.store.project_by_name(&spec.name).await?.is_some() {
            return Err(Error::InvalidProject {
                reason: format!("project with name '{}' already exists", spec.name),
            }
            .into());
        }
        let project_id = self.free_project_id().await?;
        let env_names = if spec.default_environments.is_empty() {
            vec!["production".to_string()]
        } else {
            spec.default_environments
        };
        let mut environments = Vec::with_capacity(env_names.len());
        for name in env_names {
            environments.push(StoredProjectEnvironment {
                id: new_id()?,
                project_id,
                name,
                description: None,
                created_at: now,
                updated_at: now,
            });
        }
        self.store
            .insert_project(
                &StoredProject {
                    id: project_id,
                    name: spec.name,
                    description: spec.description,
                    created_at: now,
                    updated_at: now,
                },
                &environments,
            )
            .await?;
        self.project(project_id).await
    }

    /// Updates a project's name and description.
    pub async fn update_project(
        &self,
        id: ShortId,
        spec: ProjectSpec,
        now: u64,
    ) -> anyhow::Result<Project> {
        spec.validate()?;
        let current = self.store.project(id).await?.ok_or_else(|| not_found(id))?;
        if spec.name != current.name && self.store.project_by_name(&spec.name).await?.is_some() {
            return Err(Error::InvalidProject {
                reason: format!("project with name '{}' already exists", spec.name),
            }
            .into());
        }
        self.store
            .update_project(&StoredProject {
                id,
                name: spec.name,
                description: spec.description,
                created_at: current.created_at,
                updated_at: now,
            })
            .await?;
        self.project(id).await
    }

    /// Deletes a project and its environments if no apps are linked.
    pub async fn delete_project(&self, id: ShortId) -> anyhow::Result<Project> {
        let project = self.project(id).await?;
        let apps = self.store.apps().await?;
        let in_use = apps.iter().any(|a| a.spec.project_id == Some(id));
        if in_use {
            return Err(Error::InvalidProject {
                reason: "cannot delete project currently linked to apps".into(),
            }
            .into());
        }
        self.store.delete_project(id).await?;
        Ok(project)
    }

    /// Creates an environment under a project.
    pub async fn create_environment(
        &self,
        project_id: ShortId,
        spec: EnvironmentSpec,
        now: u64,
    ) -> anyhow::Result<ProjectEnvironment> {
        spec.validate()?;
        let _ = self
            .store
            .project(project_id)
            .await?
            .ok_or_else(|| not_found(project_id))?;
        if self
            .store
            .environment_by_name(project_id, &spec.name)
            .await?
            .is_some()
        {
            return Err(Error::InvalidEnvironment {
                reason: format!(
                    "environment with name '{}' already exists in this project",
                    spec.name
                ),
            }
            .into());
        }
        let env_id = new_id()?;
        self.store
            .insert_environment(&StoredProjectEnvironment {
                id: env_id,
                project_id,
                name: spec.name,
                description: spec.description,
                created_at: now,
                updated_at: now,
            })
            .await?;
        self.environment(env_id).await
    }

    /// Gets an environment by ID including its shared variables.
    pub async fn environment(&self, id: ShortId) -> anyhow::Result<ProjectEnvironment> {
        let stored = self
            .store
            .environment(id)
            .await?
            .ok_or_else(|| not_found(id))?;
        let stored_vars = self.store.environment_vars(id).await?;
        let variables = self.unseal_env_vars(id, stored_vars)?;
        let apps = self.store.apps().await?;
        let apps_count = apps
            .iter()
            .filter(|a| a.spec.environment_id == Some(id))
            .count();
        Ok(ProjectEnvironment {
            id: stored.id,
            project_id: stored.project_id,
            name: stored.name,
            description: stored.description,
            variables,
            apps_count,
            created_at: stored.created_at,
            updated_at: stored.updated_at,
        })
    }

    fn unseal_env_vars(
        &self,
        id: ShortId,
        stored_vars: Vec<StoredEnv>,
    ) -> anyhow::Result<Vec<EnvEntry>> {
        stored_vars
            .into_iter()
            .map(|var| {
                let value = if var.secret {
                    None
                } else {
                    Some(self.open_value(id, &var)?)
                };
                Ok(EnvEntry {
                    key: var.key.parse()?,
                    value,
                    secret: var.secret,
                })
            })
            .collect()
    }

    /// Updates an environment's name and description.
    pub async fn update_environment(
        &self,
        id: ShortId,
        spec: EnvironmentSpec,
        now: u64,
    ) -> anyhow::Result<ProjectEnvironment> {
        spec.validate()?;
        let current = self
            .store
            .environment(id)
            .await?
            .ok_or_else(|| not_found(id))?;
        if spec.name != current.name
            && self
                .store
                .environment_by_name(current.project_id, &spec.name)
                .await?
                .is_some()
        {
            return Err(Error::InvalidEnvironment {
                reason: format!(
                    "environment with name '{}' already exists in this project",
                    spec.name
                ),
            }
            .into());
        }
        self.store
            .update_environment(&StoredProjectEnvironment {
                id,
                project_id: current.project_id,
                name: spec.name,
                description: spec.description,
                created_at: current.created_at,
                updated_at: now,
            })
            .await?;
        self.environment(id).await
    }

    /// Deletes an environment if no apps are linked.
    pub async fn delete_environment(&self, id: ShortId) -> anyhow::Result<ProjectEnvironment> {
        let env = self.environment(id).await?;
        let apps = self.store.apps().await?;
        let in_use = apps.iter().any(|a| a.spec.environment_id == Some(id));
        if in_use {
            return Err(Error::InvalidEnvironment {
                reason: "cannot delete environment currently linked to apps".into(),
            }
            .into());
        }
        self.store.delete_environment(id).await?;
        Ok(env)
    }

    /// Updates environment variables of a project environment.
    pub async fn set_environment_env(
        &self,
        env_id: ShortId,
        spec: EnvironmentEnvSpec,
    ) -> anyhow::Result<ProjectEnvironment> {
        spec.validate()?;
        let _ = self.environment(env_id).await?;
        let current_vars = self
            .store
            .environment_vars(env_id)
            .await?
            .into_iter()
            .map(|var| (var.key.clone(), var))
            .collect::<BTreeMap<_, _>>();
        let mut vars = Vec::with_capacity(spec.variables.len());
        for entry in spec.variables {
            vars.push(self.stored_env(env_id, entry, &current_vars)?);
        }
        self.store.replace_environment_vars(env_id, &vars).await?;
        self.environment(env_id).await
    }

    async fn free_project_id(&self) -> anyhow::Result<ShortId> {
        for _ in 0..16 {
            let id = new_id()?;
            if self.store.project(id).await?.is_none() {
                return Ok(id);
            }
        }
        anyhow::bail!("failed to allocate a free project id");
    }
}
