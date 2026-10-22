//! Project and environment management for the deployment platform.

use super::{Platform, new_id, not_found};
use crate::platform::store::{StoredEnv, StoredProject, StoredProjectEnvironment};
use r3v3rs3_api::error::Error;
use r3v3rs3_api::id::ShortId;
use r3v3rs3_api::platform::{
    EnvDiffEntry, EnvDiffStatus, EnvEntry, EnvironmentDiffReport, EnvironmentEnvSpec,
    EnvironmentSpec, EnvironmentSyncMode, EnvironmentSyncReport, EnvironmentSyncRequest, Project,
    ProjectEnvironment, ProjectSpec, ProjectSummary,
};
use std::collections::{BTreeMap, BTreeSet};

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

    async fn validate_project_environments(
        &self,
        project_id: ShortId,
        source_id: ShortId,
        target_id: ShortId,
    ) -> anyhow::Result<(ProjectEnvironment, ProjectEnvironment)> {
        let source_env = self.environment(source_id).await?;
        let target_env = self.environment(target_id).await?;
        if source_env.project_id != project_id || target_env.project_id != project_id {
            return Err(Error::InvalidEnvironment {
                reason: "environments must belong to the specified project".to_string(),
            }
            .into());
        }
        Ok((source_env, target_env))
    }

    /// Compares environment variables and secrets between source and target environments.
    pub async fn diff_environments(
        &self,
        project_id: ShortId,
        source_id: ShortId,
        target_id: ShortId,
    ) -> anyhow::Result<EnvironmentDiffReport> {
        if source_id == target_id {
            return Err(Error::InvalidEnvironment {
                reason: "source and target environments must be different".to_string(),
            }
            .into());
        }
        let (source_env, target_env) = self
            .validate_project_environments(project_id, source_id, target_id)
            .await?;

        let src_vars = self.store.environment_vars(source_id).await?;
        let tgt_vars = self.store.environment_vars(target_id).await?;
        let src_map: BTreeMap<String, StoredEnv> =
            src_vars.into_iter().map(|v| (v.key.clone(), v)).collect();
        let tgt_map: BTreeMap<String, StoredEnv> =
            tgt_vars.into_iter().map(|v| (v.key.clone(), v)).collect();

        self.build_diff_report(project_id, &source_env, &target_env, &src_map, &tgt_map)
    }

    fn build_diff_report(
        &self,
        project_id: ShortId,
        source_env: &ProjectEnvironment,
        target_env: &ProjectEnvironment,
        src_map: &BTreeMap<String, StoredEnv>,
        tgt_map: &BTreeMap<String, StoredEnv>,
    ) -> anyhow::Result<EnvironmentDiffReport> {
        let all_keys: BTreeSet<String> = src_map.keys().chain(tgt_map.keys()).cloned().collect();
        let mut entries = Vec::with_capacity(all_keys.len());
        let (mut missing_in_target, mut missing_in_source, mut different_values, mut identical) =
            (0, 0, 0, 0);

        for key_str in &all_keys {
            let entry = self.compare_single_diff_key(
                source_env.id,
                target_env.id,
                key_str,
                src_map,
                tgt_map,
            )?;
            match entry.status {
                EnvDiffStatus::Identical => identical += 1,
                EnvDiffStatus::Different => different_values += 1,
                EnvDiffStatus::MissingInTarget => missing_in_target += 1,
                EnvDiffStatus::MissingInSource => missing_in_source += 1,
            }
            entries.push(entry);
        }

        Ok(EnvironmentDiffReport {
            project_id,
            source_env_id: source_env.id,
            source_env_name: source_env.name.clone(),
            target_env_id: target_env.id,
            target_env_name: target_env.name.clone(),
            entries,
            total_keys: all_keys.len(),
            missing_in_target,
            missing_in_source,
            different_values,
            identical,
        })
    }

    fn compare_single_diff_key(
        &self,
        src_id: ShortId,
        tgt_id: ShortId,
        key_str: &str,
        src_map: &BTreeMap<String, StoredEnv>,
        tgt_map: &BTreeMap<String, StoredEnv>,
    ) -> anyhow::Result<EnvDiffEntry> {
        let key = key_str.parse()?;
        let src_var = src_map.get(key_str);
        let tgt_var = tgt_map.get(key_str);

        match (src_var, tgt_var) {
            (Some(s), None) => Ok(EnvDiffEntry {
                key,
                status: EnvDiffStatus::MissingInTarget,
                source_present: true,
                target_present: false,
                source_secret: s.secret,
                target_secret: false,
                source_value: (!s.secret)
                    .then(|| self.open_value(src_id, s))
                    .transpose()?,
                target_value: None,
            }),
            (None, Some(t)) => Ok(EnvDiffEntry {
                key,
                status: EnvDiffStatus::MissingInSource,
                source_present: false,
                target_present: true,
                source_secret: false,
                target_secret: t.secret,
                source_value: None,
                target_value: (!t.secret)
                    .then(|| self.open_value(tgt_id, t))
                    .transpose()?,
            }),
            (Some(s), Some(t)) => self.compare_existing_diff_entry(src_id, tgt_id, key, s, t),
            (None, None) => anyhow::bail!("unreachable key in diff"),
        }
    }

    fn compare_existing_diff_entry(
        &self,
        src_id: ShortId,
        tgt_id: ShortId,
        key: r3v3rs3_api::container::EnvKey,
        s: &StoredEnv,
        t: &StoredEnv,
    ) -> anyhow::Result<EnvDiffEntry> {
        let s_val = self.open_value(src_id, s)?;
        let t_val = self.open_value(tgt_id, t)?;
        let is_same = s.secret == t.secret && s_val == t_val;
        let status = if is_same {
            EnvDiffStatus::Identical
        } else {
            EnvDiffStatus::Different
        };

        Ok(EnvDiffEntry {
            key,
            status,
            source_present: true,
            target_present: true,
            source_secret: s.secret,
            target_secret: t.secret,
            source_value: (!s.secret).then_some(s_val),
            target_value: (!t.secret).then_some(t_val),
        })
    }

    fn execute_sync_mode(
        &self,
        req: &EnvironmentSyncRequest,
        src_map: &BTreeMap<String, StoredEnv>,
        mut tgt_map: BTreeMap<String, StoredEnv>,
    ) -> anyhow::Result<(
        Vec<r3v3rs3_api::container::EnvKey>,
        Vec<r3v3rs3_api::container::EnvKey>,
        Vec<StoredEnv>,
    )> {
        let (added, updated) = match req.mode {
            EnvironmentSyncMode::SyncMissing => {
                self.apply_sync_missing(req, src_map, &mut tgt_map)?
            }
            EnvironmentSyncMode::CloneAll => self.apply_clone_all(req, src_map, &mut tgt_map)?,
            EnvironmentSyncMode::SelectedKeys => {
                self.apply_sync_selected(req, src_map, &mut tgt_map)?
            }
        };
        let final_vars = tgt_map.into_values().collect();
        Ok((added, updated, final_vars))
    }

    /// Synchronizes environment variables and secrets from source to target environment.
    pub async fn sync_environment_env(
        &self,
        project_id: ShortId,
        request: EnvironmentSyncRequest,
    ) -> anyhow::Result<EnvironmentSyncReport> {
        request.validate()?;
        let (_, target_env) = self
            .validate_project_environments(project_id, request.source_env_id, request.target_env_id)
            .await?;

        let src_vars = self.store.environment_vars(request.source_env_id).await?;
        let tgt_vars = self.store.environment_vars(request.target_env_id).await?;
        let src_map: BTreeMap<String, StoredEnv> =
            src_vars.into_iter().map(|v| (v.key.clone(), v)).collect();
        let tgt_map: BTreeMap<String, StoredEnv> =
            tgt_vars.into_iter().map(|v| (v.key.clone(), v)).collect();

        let (added_keys, updated_keys, final_vars) =
            self.execute_sync_mode(&request, &src_map, tgt_map)?;

        self.store
            .replace_environment_vars(request.target_env_id, &final_vars)
            .await?;

        let synced_count = added_keys.len() + updated_keys.len();
        Ok(EnvironmentSyncReport {
            project_id,
            source_env_id: request.source_env_id,
            target_env_id: request.target_env_id,
            target_env_name: target_env.name,
            synced_count,
            added_keys,
            updated_keys,
            total_variables: final_vars.len(),
        })
    }

    fn reseal_var_for_target(
        &self,
        src_id: ShortId,
        tgt_id: ShortId,
        var: &StoredEnv,
    ) -> anyhow::Result<StoredEnv> {
        let raw_val = self.open_value(src_id, var)?;
        let aad = super::env_aad(tgt_id, &var.key);
        let sealed = self.keys().seal(&aad, raw_val.as_bytes())?;
        Ok(StoredEnv {
            key: var.key.clone(),
            sealed,
            secret: var.secret,
        })
    }

    fn apply_sync_missing(
        &self,
        req: &EnvironmentSyncRequest,
        src_map: &BTreeMap<String, StoredEnv>,
        tgt_map: &mut BTreeMap<String, StoredEnv>,
    ) -> anyhow::Result<(
        Vec<r3v3rs3_api::container::EnvKey>,
        Vec<r3v3rs3_api::container::EnvKey>,
    )> {
        let mut added = Vec::new();
        for (key, src_var) in src_map {
            if !tgt_map.contains_key(key) {
                let re_sealed =
                    self.reseal_var_for_target(req.source_env_id, req.target_env_id, src_var)?;
                tgt_map.insert(key.clone(), re_sealed);
                added.push(key.parse()?);
            }
        }
        Ok((added, Vec::new()))
    }

    fn apply_clone_all(
        &self,
        req: &EnvironmentSyncRequest,
        src_map: &BTreeMap<String, StoredEnv>,
        tgt_map: &mut BTreeMap<String, StoredEnv>,
    ) -> anyhow::Result<(
        Vec<r3v3rs3_api::container::EnvKey>,
        Vec<r3v3rs3_api::container::EnvKey>,
    )> {
        let mut added = Vec::new();
        let mut updated = Vec::new();
        let old_keys: BTreeSet<String> = tgt_map.keys().cloned().collect();
        tgt_map.clear();

        for (key, src_var) in src_map {
            let re_sealed =
                self.reseal_var_for_target(req.source_env_id, req.target_env_id, src_var)?;
            if old_keys.contains(key) {
                updated.push(key.parse()?);
            } else {
                added.push(key.parse()?);
            }
            tgt_map.insert(key.clone(), re_sealed);
        }
        Ok((added, updated))
    }

    fn apply_sync_selected(
        &self,
        req: &EnvironmentSyncRequest,
        src_map: &BTreeMap<String, StoredEnv>,
        tgt_map: &mut BTreeMap<String, StoredEnv>,
    ) -> anyhow::Result<(
        Vec<r3v3rs3_api::container::EnvKey>,
        Vec<r3v3rs3_api::container::EnvKey>,
    )> {
        let mut added = Vec::new();
        let mut updated = Vec::new();

        for key_obj in &req.keys {
            let key_str = key_obj.as_str();
            if let Some(src_var) = src_map.get(key_str) {
                let re_sealed =
                    self.reseal_var_for_target(req.source_env_id, req.target_env_id, src_var)?;
                if tgt_map.contains_key(key_str) {
                    updated.push(key_obj.clone());
                } else {
                    added.push(key_obj.clone());
                }
                tgt_map.insert(key_str.to_string(), re_sealed);
            }
        }
        Ok((added, updated))
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
