//! BuildKit runner: builds images with BuildKit via `docker buildx build` or a remote BuildKit daemon,
//! supporting cache mounts, remote builders, and external cache import/export.

use super::process::run;
use anyhow::Context as _;
use r3v3rs3_api::container::ImageRef;
use r3v3rs3_api::platform::BuildKitConfig;
use std::os::unix::fs::PermissionsExt as _;
use std::path::{Path, PathBuf};
use std::time::Duration;
use tokio::process::Command;

/// The default timeout for BuildKit builds (30 minutes).
pub const BUILDKIT_TIMEOUT: Duration = Duration::from_secs(1800);

/// The directory of the docker CLI configuration under a work directory.
const DOCKER_CONFIG_DIR: &str = "docker";

/// The config directory that the `docker` CLI must receive, or `None` when its default works.
///
/// buildx writes its state into the config directory, `$HOME/.docker` by default. The service
/// user of the server has no writable home directory (`ProtectHome=true`), so buildx fails there.
/// The default stays when `DOCKER_CONFIG` is set or `$HOME/.docker/buildx` can be created,
/// because it holds the CLI plugins and the registry logins of the user. Otherwise the CLI gets
/// `docker` under `base`, created with mode 0700; the system plugin directories still apply.
pub async fn docker_config_dir(base: &Path) -> anyhow::Result<Option<PathBuf>> {
    if std::env::var_os("DOCKER_CONFIG").is_some() {
        return Ok(None);
    }
    let Some(home) = std::env::var_os("HOME") else {
        return private_dir(base.join(DOCKER_CONFIG_DIR)).await.map(Some);
    };
    let default = Path::new(&home).join(".docker/buildx");
    match tokio::fs::create_dir_all(&default).await {
        Ok(()) => Ok(None),
        Err(err) => {
            let dir = private_dir(base.join(DOCKER_CONFIG_DIR)).await?;
            tracing::info!(
                default = %default.display(),
                dir = %dir.display(),
                "the docker CLI uses its own config directory: {err}"
            );
            Ok(Some(dir))
        }
    }
}

async fn private_dir(dir: PathBuf) -> anyhow::Result<PathBuf> {
    tokio::fs::create_dir_all(&dir)
        .await
        .with_context(|| format!("failed to create {}", dir.display()))?;
    tokio::fs::set_permissions(&dir, std::fs::Permissions::from_mode(0o700))
        .await
        .with_context(|| format!("failed to restrict {}", dir.display()))?;
    Ok(dir)
}

pub struct BuildKitRunner {
    pub docker_host: Option<String>,
    pub docker_config: Option<PathBuf>,
}

impl BuildKitRunner {
    pub fn new(docker_host: Option<String>, docker_config: Option<PathBuf>) -> Self {
        Self {
            docker_host,
            docker_config,
        }
    }

    pub fn build_command(
        &self,
        context_dir: &Path,
        dockerfile_path: &Path,
        tag: &ImageRef,
        config: &BuildKitConfig,
    ) -> Command {
        let mut cmd = Command::new("docker");
        cmd.env_clear()
            .env("PATH", std::env::var_os("PATH").unwrap_or_default())
            .env("DOCKER_BUILDKIT", "1");
        if let Some(home) = std::env::var_os("HOME") {
            cmd.env("HOME", home);
        }
        if let Some(host) = &self.docker_host {
            cmd.env("DOCKER_HOST", host);
        }
        if let Some(dir) = &self.docker_config {
            cmd.env("DOCKER_CONFIG", dir);
        }
        if let Some(remote) = &config.remote_addr {
            cmd.env("BUILDKIT_HOST", remote);
        }

        cmd.args(["buildx", "build"]);
        cmd.arg("-t").arg(tag.as_str());
        cmd.arg("-f").arg(dockerfile_path);
        cmd.arg("--load");

        if let Some(from) = &config.cache_from {
            cmd.arg("--cache-from").arg(from);
        }
        if let Some(to) = &config.cache_to {
            cmd.arg("--cache-to").arg(to);
        }

        cmd.arg(context_dir);
        cmd.current_dir(context_dir);
        cmd
    }

    /// Builds the image and tags it with `tag`. The error carries the end of the buildx output.
    pub async fn build(
        &self,
        context_dir: &Path,
        dockerfile_path: &Path,
        tag: &ImageRef,
        config: &BuildKitConfig,
    ) -> anyhow::Result<()> {
        let cmd = self.build_command(context_dir, dockerfile_path, tag, config);
        run(cmd, BUILDKIT_TIMEOUT)
            .await
            .context("BuildKit build failed")?;
        Ok(())
    }
}

/// Writes the cache mounts of `config` into the `RUN` instructions of the Dockerfile.
pub async fn prepare_dockerfile(dockerfile: &Path, config: &BuildKitConfig) -> anyhow::Result<()> {
    if config.cache_mounts.is_empty() && config.custom_cache_paths.is_empty() {
        return Ok(());
    }
    let content = tokio::fs::read_to_string(dockerfile)
        .await
        .with_context(|| format!("failed to read {}", dockerfile.display()))?;
    let optimized = inject_cache_mounts(&content, config);
    tokio::fs::write(dockerfile, optimized)
        .await
        .with_context(|| format!("failed to write {}", dockerfile.display()))
}

/// Injects BuildKit cache mount syntax into `RUN` instructions of a Dockerfile.
pub fn inject_cache_mounts(dockerfile: &str, config: &BuildKitConfig) -> String {
    let mut mounts = Vec::new();
    for preset in &config.cache_mounts {
        for path in preset.paths() {
            let id = format!("{}_{}", preset.id(), sanitize_path_id(path));
            mounts.push(format!("--mount=type=cache,id={id},target={path}"));
        }
    }
    for custom in &config.custom_cache_paths {
        if !r3v3rs3_api::platform::is_valid_cache_path(custom) {
            continue;
        }
        let id = format!("custom_{}", sanitize_path_id(custom));
        mounts.push(format!("--mount=type=cache,id={id},target={custom}"));
    }
    if mounts.is_empty() {
        return dockerfile.to_string();
    }
    let mount_str = mounts.join(" ");

    let mut result = String::new();
    let has_syntax = dockerfile.lines().any(|l| l.starts_with("# syntax="));
    if !has_syntax {
        result.push_str("# syntax=docker/dockerfile:1\n");
    }

    for line in dockerfile.lines() {
        let trimmed = line.trim_start();
        if trimmed.starts_with("RUN ") && !trimmed.contains("--mount=type=cache") {
            let indent = &line[..line.len() - trimmed.len()];
            let run_cmd = &trimmed[4..];
            result.push_str(&format!("{indent}RUN {mount_str} {run_cmd}\n"));
        } else {
            result.push_str(line);
            result.push('\n');
        }
    }
    result
}

fn sanitize_path_id(path: &str) -> String {
    let mut out = String::new();
    let mut last_was_sep = true;
    for c in path.chars() {
        if c.is_alphanumeric() {
            out.push(c.to_ascii_lowercase());
            last_was_sep = false;
        } else if !last_was_sep {
            out.push('_');
            last_was_sep = true;
        }
    }
    if out.ends_with('_') {
        out.pop();
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use r3v3rs3_api::platform::CacheMountPreset;

    #[test]
    fn test_inject_cache_mounts_cargo() {
        let df = "FROM rust:1.80\nRUN cargo build --release\nCMD [\"./app\"]\n";
        let config = BuildKitConfig {
            enabled: true,
            remote_addr: None,
            cache_mounts: vec![CacheMountPreset::Cargo],
            custom_cache_paths: Vec::new(),
            cache_from: None,
            cache_to: None,
        };
        let injected = inject_cache_mounts(df, &config);
        assert!(injected.starts_with("# syntax=docker/dockerfile:1\n"));
        assert!(injected.contains(
            "--mount=type=cache,id=cargo_root_cargo_registry,target=/root/.cargo/registry"
        ));
        assert!(injected.contains("cargo build --release"));
    }

    #[test]
    fn test_inject_custom_cache_paths() {
        let df = "# syntax=docker/dockerfile:1\nRUN npm ci\n";
        let config = BuildKitConfig {
            enabled: true,
            remote_addr: None,
            cache_mounts: Vec::new(),
            custom_cache_paths: vec!["/root/.npm".to_string()],
            cache_from: None,
            cache_to: None,
        };
        let injected = inject_cache_mounts(df, &config);
        assert!(injected.contains("--mount=type=cache,id=custom_root_npm,target=/root/.npm"));
        assert_eq!(injected.matches("# syntax=").count(), 1);
    }

    #[test]
    fn test_inject_custom_cache_paths_rejects_injection() {
        let df = "RUN build\n";
        let config = BuildKitConfig {
            enabled: true,
            remote_addr: None,
            cache_mounts: Vec::new(),
            custom_cache_paths: vec![
                "/cache --mount=type=bind,source=/,target=/host_root".to_string(),
                "/cache,source=/".to_string(),
                "/cache\nRUN id".to_string(),
                "relative/path".to_string(),
            ],
            cache_from: None,
            cache_to: None,
        };
        let injected = inject_cache_mounts(df, &config);
        assert!(!injected.contains("bind"));
        assert!(!injected.contains("host_root"));
        assert!(!injected.contains("id"));
        assert_eq!(injected, "RUN build\n");
    }

    fn env_of(cmd: &Command, key: &str) -> Option<String> {
        cmd.as_std()
            .get_envs()
            .find(|(name, _)| *name == key)
            .and_then(|(_, value)| value)
            .map(|value| value.to_string_lossy().into_owned())
    }

    #[test]
    fn the_build_command_carries_the_docker_config_and_host() -> anyhow::Result<()> {
        let runner = BuildKitRunner::new(
            Some("unix:///var/run/docker.sock".into()),
            Some(PathBuf::from("/etc/r3v3rs3/docker")),
        );
        let tag: ImageRef = "r3v3rs3/abc-def:ghi-jkl".parse()?;
        let cmd = runner.build_command(
            Path::new("/tmp/ctx"),
            Path::new("/tmp/ctx/Dockerfile"),
            &tag,
            &BuildKitConfig {
                enabled: true,
                ..Default::default()
            },
        );
        assert_eq!(
            env_of(&cmd, "DOCKER_CONFIG").as_deref(),
            Some("/etc/r3v3rs3/docker")
        );
        assert_eq!(
            env_of(&cmd, "DOCKER_HOST").as_deref(),
            Some("unix:///var/run/docker.sock")
        );
        assert_eq!(env_of(&cmd, "BUILDKIT_HOST"), None);
        Ok(())
    }

    #[tokio::test]
    async fn the_own_config_directory_is_private() -> anyhow::Result<()> {
        let base = std::env::temp_dir().join(format!(
            "r3v3rs3-docker-config-{}",
            hex::encode(rand::random::<[u8; 8]>())
        ));
        let dir = private_dir(base.join(DOCKER_CONFIG_DIR)).await?;
        let mode = std::fs::metadata(&dir)?.permissions().mode() & 0o777;
        std::fs::remove_dir_all(&base)?;
        assert_eq!(dir, base.join("docker"));
        assert_eq!(mode, 0o700);
        Ok(())
    }
}
