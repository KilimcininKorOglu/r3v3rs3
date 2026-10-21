//! platform.key rotation: generates a new encryption key, sets it as primary,
//! and re-seals all stored secrets in SQLite tables with the new key.

use super::backup::{
    RSYNC_SECRET, RSYNC_SECRET_AAD, S3_SECRET, S3_SECRET_AAD, SFTP_SECRET, SFTP_SECRET_AAD,
};
use super::connections::connection_aad;
use super::hooks::webhook_secret_aad;
use super::{KEY_FILE, env_aad, git_token_aad};
use crate::keys::crypto::ClusterKeys;
use crate::keys::key_file::{new_key_file, parse_key_file};
use anyhow::Context;
use r3v3rs3_api::id::ShortId;
use r3v3rs3_api::platform::KeyRotationReport;
use sqlx::{Row, SqlitePool};
use std::path::Path;
use tokio::io::AsyncWriteExt;

struct ResealedCounts {
    app_env: u32,
    git_connections: u32,
    github_apps: u32,
    settings: u32,
    app_secrets: u32,
    registries: u32,
    env_groups: u32,
}

async fn reseal_all_tables(
    pool: &SqlitePool,
    keys: &std::sync::RwLock<ClusterKeys>,
) -> anyhow::Result<ResealedCounts> {
    Ok(ResealedCounts {
        app_env: reseal_app_env(pool, keys).await?,
        app_secrets: reseal_app_secrets(pool, keys).await?,
        git_connections: reseal_git_connections(pool, keys).await?,
        github_apps: reseal_github_apps(pool, keys).await?,
        settings: reseal_settings(pool, keys).await?,
        registries: reseal_registries(pool, keys).await?,
        env_groups: reseal_env_groups(pool, keys).await?,
    })
}

pub(super) async fn run_key_rotation(
    pool: &SqlitePool,
    config_dir: &Path,
    keys: &std::sync::RwLock<ClusterKeys>,
) -> anyhow::Result<KeyRotationReport> {
    let key_text = new_key_file()?;
    let new_primary = parse_key_file(&key_text)?;
    let new_key_id = hex::encode(new_primary.id());

    // Rotate the key in-memory: new_primary becomes primary, old primary moves to older
    {
        let mut guard = match keys.write() {
            Ok(guard) => guard,
            Err(poisoned) => poisoned.into_inner(),
        };
        guard.rotate(new_primary);
    }

    let counts = reseal_all_tables(pool, keys).await?;

    // Persist new key to disk, backing up previous key to platform.key.old
    write_key_files(config_dir, &key_text).await?;

    Ok(KeyRotationReport {
        new_key_id,
        resealed_app_env: counts.app_env,
        resealed_git_connections: counts.git_connections,
        resealed_github_apps: counts.github_apps,
        resealed_settings: counts.settings,
        resealed_app_secrets: counts.app_secrets,
        resealed_registries: counts.registries,
        resealed_env_groups: counts.env_groups,
    })
}

fn reseal_value(
    keys: &std::sync::RwLock<ClusterKeys>,
    aad: &str,
    sealed: &[u8],
) -> anyhow::Result<Vec<u8>> {
    let guard = match keys.read() {
        Ok(g) => g,
        Err(p) => p.into_inner(),
    };
    let plaintext = guard.open(aad, sealed)?;
    guard.seal(aad, &plaintext)
}

async fn reseal_app_env(
    pool: &SqlitePool,
    keys: &std::sync::RwLock<ClusterKeys>,
) -> anyhow::Result<u32> {
    let rows = sqlx::query("SELECT app_id, key, value FROM app_env")
        .fetch_all(pool)
        .await?;
    let mut count = 0;
    for row in rows {
        let app_id_str: String = row.try_get("app_id")?;
        let app_id: ShortId = app_id_str.parse()?;
        let key: String = row.try_get("key")?;
        let value: Vec<u8> = row.try_get("value")?;
        let aad = env_aad(app_id, &key);
        let resealed = reseal_value(keys, &aad, &value)?;
        sqlx::query("UPDATE app_env SET value = ? WHERE app_id = ? AND key = ?")
            .bind(resealed)
            .bind(app_id_str)
            .bind(key)
            .execute(pool)
            .await?;
        count += 1;
    }
    Ok(count)
}

async fn reseal_app_secrets(
    pool: &SqlitePool,
    keys: &std::sync::RwLock<ClusterKeys>,
) -> anyhow::Result<u32> {
    let rows = sqlx::query("SELECT app_id, name, value FROM app_secrets")
        .fetch_all(pool)
        .await?;
    let mut count = 0;
    for row in rows {
        let app_id_str: String = row.try_get("app_id")?;
        let app_id: ShortId = app_id_str.parse()?;
        let name: String = row.try_get("name")?;
        let value: Vec<u8> = row.try_get("value")?;
        let aad = if name == "git_token" {
            git_token_aad(app_id)
        } else {
            webhook_secret_aad(app_id)
        };
        let resealed = reseal_value(keys, &aad, &value)?;
        sqlx::query("UPDATE app_secrets SET value = ? WHERE app_id = ? AND name = ?")
            .bind(resealed)
            .bind(app_id_str)
            .bind(name)
            .execute(pool)
            .await?;
        count += 1;
    }
    Ok(count)
}

async fn reseal_one_git_connection(
    pool: &SqlitePool,
    keys: &std::sync::RwLock<ClusterKeys>,
    row: &sqlx::sqlite::SqliteRow,
) -> anyhow::Result<()> {
    let id_str: String = row.try_get("id")?;
    let id: ShortId = id_str.parse()?;
    let client_secret: Vec<u8> = row.try_get("client_secret")?;
    let client_secret_aad = connection_aad(id, "client_secret");
    let resealed_client_secret = reseal_value(keys, &client_secret_aad, &client_secret)?;

    let resealed_access_token =
        reseal_opt_token(keys, id, "access_token", row.try_get("access_token")?)?;
    let resealed_refresh_token =
        reseal_opt_token(keys, id, "refresh_token", row.try_get("refresh_token")?)?;

    sqlx::query(
        "UPDATE git_connections SET client_secret = ?, access_token = ?, refresh_token = ? WHERE id = ?",
    )
    .bind(resealed_client_secret)
    .bind(resealed_access_token)
    .bind(resealed_refresh_token)
    .bind(id_str)
    .execute(pool)
    .await?;
    Ok(())
}

async fn reseal_git_connections(
    pool: &SqlitePool,
    keys: &std::sync::RwLock<ClusterKeys>,
) -> anyhow::Result<u32> {
    let rows =
        sqlx::query("SELECT id, client_secret, access_token, refresh_token FROM git_connections")
            .fetch_all(pool)
            .await?;
    let mut count = 0;
    for row in &rows {
        reseal_one_git_connection(pool, keys, row).await?;
        count += 1;
    }
    Ok(count)
}

fn reseal_opt_token(
    keys: &std::sync::RwLock<ClusterKeys>,
    id: ShortId,
    field: &str,
    val: Option<Vec<u8>>,
) -> anyhow::Result<Option<Vec<u8>>> {
    let Some(sealed) = val else { return Ok(None) };
    let aad = connection_aad(id, field);
    Ok(Some(reseal_value(keys, &aad, &sealed)?))
}

async fn reseal_github_apps(
    pool: &SqlitePool,
    keys: &std::sync::RwLock<ClusterKeys>,
) -> anyhow::Result<u32> {
    let rows = sqlx::query("SELECT connection_id, private_key FROM github_apps")
        .fetch_all(pool)
        .await?;
    let mut count = 0;
    for row in rows {
        let id_str: String = row.try_get("connection_id")?;
        let id: ShortId = id_str.parse()?;
        let private_key: Vec<u8> = row.try_get("private_key")?;
        let aad = connection_aad(id, "private_key");
        let resealed = reseal_value(keys, &aad, &private_key)?;
        sqlx::query("UPDATE github_apps SET private_key = ? WHERE connection_id = ?")
            .bind(resealed)
            .bind(id_str)
            .execute(pool)
            .await?;
        count += 1;
    }
    Ok(count)
}

async fn reseal_settings(
    pool: &SqlitePool,
    keys: &std::sync::RwLock<ClusterKeys>,
) -> anyhow::Result<u32> {
    let targets = [
        (S3_SECRET, S3_SECRET_AAD),
        (SFTP_SECRET, SFTP_SECRET_AAD),
        (RSYNC_SECRET, RSYNC_SECRET_AAD),
    ];
    let mut count = 0;
    for (setting_key, aad) in targets {
        let row = sqlx::query("SELECT value FROM platform_settings WHERE key = ?")
            .bind(setting_key)
            .fetch_optional(pool)
            .await?;
        if let Some(row) = row {
            let hex_val: String = row.try_get("value")?;
            let sealed = hex::decode(&hex_val)?;
            let resealed = reseal_value(keys, aad, &sealed)?;
            sqlx::query("UPDATE platform_settings SET value = ? WHERE key = ?")
                .bind(hex::encode(resealed))
                .bind(setting_key)
                .execute(pool)
                .await?;
            count += 1;
        }
    }
    Ok(count)
}

async fn reseal_registries(
    pool: &SqlitePool,
    keys: &std::sync::RwLock<ClusterKeys>,
) -> anyhow::Result<u32> {
    let rows = sqlx::query("SELECT id, secret FROM registries")
        .fetch_all(pool)
        .await?;
    let mut count = 0;
    for row in rows {
        let id_str: String = row.try_get("id")?;
        let id: ShortId = id_str.parse()?;
        let secret: Vec<u8> = row.try_get("secret")?;
        let aad = super::registries::registry_aad(id);
        let resealed = reseal_value(keys, &aad, &secret)?;
        sqlx::query("UPDATE registries SET secret = ? WHERE id = ?")
            .bind(resealed)
            .bind(id_str)
            .execute(pool)
            .await?;
        count += 1;
    }
    Ok(count)
}

async fn reseal_env_groups(
    pool: &SqlitePool,
    keys: &std::sync::RwLock<ClusterKeys>,
) -> anyhow::Result<u32> {
    let rows = sqlx::query("SELECT group_id, key, value FROM env_group_vars")
        .fetch_all(pool)
        .await?;
    let mut count = 0;
    for row in rows {
        let group_id_str: String = row.try_get("group_id")?;
        let group_id: ShortId = group_id_str.parse()?;
        let key: String = row.try_get("key")?;
        let value: Vec<u8> = row.try_get("value")?;
        let aad = env_aad(group_id, &key);
        let resealed = reseal_value(keys, &aad, &value)?;
        sqlx::query("UPDATE env_group_vars SET value = ? WHERE group_id = ? AND key = ?")
            .bind(resealed)
            .bind(group_id_str)
            .bind(key)
            .execute(pool)
            .await?;
        count += 1;
    }
    Ok(count)
}

async fn write_key_files(config_dir: &Path, key_text: &str) -> anyhow::Result<()> {
    let key_path = config_dir.join(KEY_FILE);
    let old_key_path = config_dir.join("platform.key.old");
    if tokio::fs::try_exists(&key_path).await? {
        let current_text = tokio::fs::read_to_string(&key_path).await?;
        write_private_file(&old_key_path, &current_text).await?;
    }
    write_private_file(&key_path, key_text).await?;
    Ok(())
}

async fn write_private_file(path: &Path, content: &str) -> anyhow::Result<()> {
    let mut options = tokio::fs::OpenOptions::new();
    options.write(true).create(true).truncate(true);
    options.mode(0o600);
    let mut file = options
        .open(path)
        .await
        .with_context(|| format!("failed to open {}", path.display()))?;
    file.write_all(content.as_bytes()).await?;
    file.flush().await?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::platform::Platform;
    use crate::platform::tests::platform;
    use r3v3rs3_api::platform::{
        AppRequest, AppSource, AppSpec, BackupConfig, EnvEntry, LOCAL_TARGET,
    };

    fn test_spec(image: &str, port: u16) -> anyhow::Result<AppSpec> {
        Ok(AppSpec {
            source: AppSource::Image {
                image: image.parse()?,
            },
            port,
            domains: Vec::new(),
            ports: Vec::new(),
            health_check_path: None,
            volumes: Vec::new(),
            restart: Default::default(),
            limits: Default::default(),
            host_port: None,
            database: None,
            uses_database: None,
            replica_of: None,
            auto_deploy: false,
            require_approval: false,
            deploy_window: None,
            tag_trigger: None,
            store_app_id: None,
            targets: Vec::new(),
            load_balancing: None,
            alert_rules: None,
            sticky: None,
            git_config: None,
            env_groups: Vec::new(),
            preview: None,
            preview_of: None,
            buildkit: None,
            bot_blocker: false,
        })
    }

    async fn populate_test_app_secrets(
        platform: &Platform,
        app_id: ShortId,
    ) -> anyhow::Result<String> {
        platform
            .set_env(
                app_id,
                vec![EnvEntry {
                    key: "DB_PASSWORD".parse()?,
                    value: Some("super_secret_123".into()),
                    secret: true,
                }],
            )
            .await?;

        platform.set_git_token(app_id, "ghp_test_token_xyz").await?;
        platform.ensure_webhook_secret(app_id).await
    }

    async fn populate_test_app_and_settings(
        platform: &Platform,
    ) -> anyhow::Result<(ShortId, String)> {
        let app = platform
            .add_app(
                AppRequest {
                    name: "test-app".parse()?,
                    target: LOCAL_TARGET.parse()?,
                    spec: test_spec("nginx:alpine", 80)?,
                },
                1,
            )
            .await?;

        let webhook_sec = populate_test_app_secrets(platform, app.id).await?;

        let s3 = Some(r3v3rs3_api::platform::S3Config {
            endpoint: "https://s3.example.com/b".into(),
            region: "us-east-1".into(),
            prefix: "db".into(),
            access_key: "AKIA".into(),
            secret_key: "secret_s3_key".into(),
        });
        platform
            .set_backup_config(BackupConfig {
                s3,
                ..Default::default()
            })
            .await?;

        platform
            .add_registry(r3v3rs3_api::platform::RegistryRequest {
                server: "ghcr.io".into(),
                username: "octocat".into(),
                password: Some("ghp_secret".into()),
            })
            .await?;

        Ok((app.id, webhook_sec))
    }

    async fn verify_rotated_secrets(
        platform: &Platform,
        app_id: ShortId,
        expected_webhook: &str,
    ) -> anyhow::Result<()> {
        let stored_env = platform.store.env(app_id).await?;
        assert_eq!(stored_env.len(), 1);
        assert_eq!(stored_env[0].key, "DB_PASSWORD");
        assert_eq!(
            platform.open_value(app_id, &stored_env[0])?,
            "super_secret_123"
        );

        let git_tok = platform.git_token(app_id).await?;
        assert_eq!(git_tok.as_deref(), Some("ghp_test_token_xyz"));

        let opened_sec = platform.ensure_webhook_secret(app_id).await?;
        assert_eq!(opened_sec, expected_webhook);

        let opened_s3 = platform.s3_secret().await?;
        assert_eq!(opened_s3.as_deref(), Some("secret_s3_key"));

        let auth = platform
            .registry_auth_for_image("ghcr.io/octocat/app:v1")
            .await?;
        assert_eq!(auth.map(|a| a.password), Some("ghp_secret".to_string()));
        Ok(())
    }

    async fn verify_fresh_restart(dir: &std::path::Path, app_id: ShortId) -> anyhow::Result<()> {
        let (command, _) = tokio::sync::mpsc::channel(1);
        let config = r3v3rs3_api::app::AppConfig {
            platform: r3v3rs3_api::platform::PlatformConfig {
                enabled: true,
                ..Default::default()
            },
            ..Default::default()
        };
        let fresh = Platform::open(&config, dir, command, None).await?;
        let fresh_stored = fresh.store.env(app_id).await?;
        assert_eq!(
            fresh.open_value(app_id, &fresh_stored[0])?,
            "super_secret_123"
        );
        let fresh_s3 = fresh.s3_secret().await?;
        assert_eq!(fresh_s3.as_deref(), Some("secret_s3_key"));

        let fresh_auth = fresh
            .registry_auth_for_image("ghcr.io/octocat/app:v1")
            .await?;
        assert_eq!(
            fresh_auth.map(|a| a.password),
            Some("ghp_secret".to_string())
        );
        Ok(())
    }

    #[tokio::test]
    async fn key_rotation_reseals_all_secrets_and_persists_old_key() -> anyhow::Result<()> {
        let (platform, dir) = platform().await?;
        let (app_id, webhook_sec) = populate_test_app_and_settings(&platform).await?;

        let report = platform.rotate_platform_key().await?;
        assert_eq!(report.resealed_app_env, 1);
        assert_eq!(report.resealed_app_secrets, 2);
        assert_eq!(report.resealed_settings, 1);
        assert_eq!(report.resealed_registries, 1);
        assert_eq!(report.resealed_env_groups, 0);

        verify_rotated_secrets(&platform, app_id, &webhook_sec).await?;

        assert!(tokio::fs::try_exists(dir.0.join(KEY_FILE)).await?);
        assert!(tokio::fs::try_exists(dir.0.join("platform.key.old")).await?);

        verify_fresh_restart(&dir.0, app_id).await?;
        Ok(())
    }
}
