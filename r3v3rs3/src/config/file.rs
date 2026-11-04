use r3v3rs3_api::app::AppConfig;
use std::path::{Path, PathBuf};
use tokio::fs;
use tracing::info;

/// Creates the directory of the file.
async fn create_parent(path: &Path) -> anyhow::Result<()> {
    let dir = path
        .parent()
        .ok_or_else(|| anyhow::anyhow!("the path has no directory: {}", path.display()))?;
    fs::create_dir_all(dir).await?;
    Ok(())
}

/// Writes to a temporary file, sets mode if specified, flushes and fsyncs to disk.
async fn write_tmp_file(
    options: &fs::OpenOptions,
    tmp_path: &Path,
    contents: &[u8],
    mode: Option<u32>,
) -> anyhow::Result<()> {
    use std::os::unix::fs::PermissionsExt;
    use tokio::io::AsyncWriteExt;

    let mut file = options.open(tmp_path).await?;
    if let Some(m) = mode {
        file.set_permissions(std::fs::Permissions::from_mode(m))
            .await?;
    }
    file.write_all(contents).await?;
    file.sync_all().await?;
    Ok(())
}

/// Atomically writes contents to `path` via a temporary file in the same directory, fsync, and rename.
async fn write_atomic_impl(path: &Path, contents: &[u8], mode: Option<u32>) -> anyhow::Result<()> {
    create_parent(path).await?;
    let parent = path.parent().unwrap_or(Path::new("."));
    let file_name = path.file_name().and_then(|n| n.to_str()).unwrap_or("file");
    let random_suffix: u64 = rand::random();
    let tmp_path = parent.join(format!(".{file_name}.{random_suffix}.tmp"));

    let mut options = fs::OpenOptions::new();
    options.write(true).create_new(true);
    if let Some(m) = mode {
        options.mode(m);
    }
    let res = write_tmp_file(&options, &tmp_path, contents, mode).await;
    if let Err(err) = res {
        let _ = fs::remove_file(&tmp_path).await;
        return Err(err);
    }
    if let Err(err) = fs::rename(&tmp_path, path).await {
        let _ = fs::remove_file(&tmp_path).await;
        return Err(err.into());
    }
    Ok(())
}

/// Writes a file that holds secrets, such as ACME account keys, so only the owner can read it.
pub(crate) async fn write_private(path: &Path, contents: impl AsRef<[u8]>) -> anyhow::Result<()> {
    write_atomic_impl(path, contents.as_ref(), Some(0o600)).await
}

/// Writes a configuration file atomically through a temporary file, fsync and rename.
#[allow(dead_code)]
pub(crate) async fn write_atomic(path: &Path, contents: impl AsRef<[u8]>) -> anyhow::Result<()> {
    write_atomic_impl(path, contents.as_ref(), None).await
}

/// Reads `config.toml` from the legacy configuration directory if it exists.
pub async fn read_legacy_app_config(config_dir: &Path) -> anyhow::Result<AppConfig> {
    let path = config_dir.join("config.toml");
    info!(?path, "load legacy config");
    let content = fs::read_to_string(&path).await?;
    Ok(toml::from_str(&content)?)
}

/// Legacy file storage wrapper retained for reading old `config.toml` during SQLite migration.
pub struct FileStorage {
    dir: PathBuf,
}

impl FileStorage {
    pub fn new(dir: &Path) -> Self {
        Self {
            dir: dir.to_path_buf(),
        }
    }

    /// Reads `config.toml` from the legacy configuration directory.
    pub async fn read_app_config(&self) -> anyhow::Result<AppConfig> {
        read_legacy_app_config(&self.dir).await
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::os::unix::fs::PermissionsExt;

    const DEFAULT_CONFIG: &str = include_str!("../../data/default-config.toml");

    #[tokio::test]
    async fn an_existing_readable_file_becomes_owner_only() -> anyhow::Result<()> {
        let path = std::env::temp_dir().join(format!("r3v3rs3-acme-{}.toml", std::process::id()));
        std::fs::write(&path, "")?;
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o644))?;

        write_private(&path, "version = \"1\"".to_string()).await?;

        let mode = std::fs::metadata(&path)?.permissions().mode() & 0o777;
        let contents = std::fs::read_to_string(&path)?;
        std::fs::remove_file(&path)?;
        assert_eq!(mode, 0o600);
        assert_eq!(contents, "version = \"1\"");
        Ok(())
    }

    #[tokio::test]
    async fn a_new_file_is_created_owner_only() -> anyhow::Result<()> {
        let path =
            std::env::temp_dir().join(format!("r3v3rs3-acme-new-{}.toml", std::process::id()));
        let _ = std::fs::remove_file(&path);

        write_private(&path, String::new()).await?;

        let mode = std::fs::metadata(&path)?.permissions().mode() & 0o777;
        std::fs::remove_file(&path)?;
        assert_eq!(mode, 0o600);
        Ok(())
    }

    #[tokio::test]
    async fn write_atomic_persists_contents_via_rename() -> anyhow::Result<()> {
        let path = std::env::temp_dir().join(format!("r3v3rs3-atomic-{}.toml", std::process::id()));
        write_atomic(&path, "atomic test content").await?;
        assert_eq!(std::fs::read_to_string(&path)?, "atomic test content");
        std::fs::remove_file(&path)?;
        Ok(())
    }

    #[test]
    fn default_config_template_holds_defaults() {
        let config: AppConfig = toml::from_str(DEFAULT_CONFIG).unwrap();
        assert_eq!(config, AppConfig::default());
    }

    #[tokio::test]
    async fn read_legacy_app_config_parses_valid_toml() -> anyhow::Result<()> {
        let dir =
            std::env::temp_dir().join(format!("r3v3rs3-legacy-config-{}", rand::random::<u64>()));
        tokio::fs::create_dir_all(&dir).await?;
        let config_path = dir.join("config.toml");
        tokio::fs::write(&config_path, "public_url = \"https://panel.example.com\"\n").await?;

        let storage = FileStorage::new(&dir);
        let loaded = storage.read_app_config().await?;
        assert_eq!(
            loaded.public_url.as_ref().map(|u| u.as_str()),
            Some("https://panel.example.com")
        );

        let direct = read_legacy_app_config(&dir).await?;
        assert_eq!(
            direct.public_url.as_ref().map(|u| u.as_str()),
            Some("https://panel.example.com")
        );

        let _ = tokio::fs::remove_dir_all(&dir).await;
        Ok(())
    }

    #[tokio::test]
    async fn read_legacy_app_config_fails_on_missing_file() {
        let dir =
            std::env::temp_dir().join(format!("r3v3rs3-missing-config-{}", rand::random::<u64>()));
        let res = read_legacy_app_config(&dir).await;
        assert!(res.is_err());
    }
}
