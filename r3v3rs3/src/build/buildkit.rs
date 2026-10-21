//! BuildKit runner: builds images with BuildKit via `docker buildx build` or a remote BuildKit daemon,
//! supporting cache mounts, remote builders, and external cache import/export.

use super::process::run;
use anyhow::Context as _;
use r3v3rs3_api::container::ImageRef;
use r3v3rs3_api::platform::BuildKitConfig;
use std::path::Path;
use std::time::Duration;
use tokio::process::Command;

/// The default timeout for BuildKit builds (30 minutes).
pub const BUILDKIT_TIMEOUT: Duration = Duration::from_secs(1800);

pub struct BuildKitRunner {
    pub docker_host: Option<String>,
}

impl BuildKitRunner {
    pub fn new(docker_host: Option<impl Into<String>>) -> Self {
        Self {
            docker_host: docker_host.map(Into::into),
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
        cmd.env("DOCKER_BUILDKIT", "1");
        if let Some(host) = &self.docker_host {
            cmd.env("DOCKER_HOST", host);
        }
        if let Some(remote) = &config.remote_addr {
            cmd.env("BUILDKIT_HOST", remote);
        }
        for key in ["PATH", "HOME", "DOCKER_CONFIG"] {
            if let Ok(val) = std::env::var(key) {
                cmd.env(key, val);
            }
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

    pub async fn build(
        &self,
        context_dir: &Path,
        dockerfile_path: &Path,
        tag: &ImageRef,
        config: &BuildKitConfig,
    ) -> anyhow::Result<String> {
        let cmd = self.build_command(context_dir, dockerfile_path, tag, config);
        run(cmd, BUILDKIT_TIMEOUT)
            .await
            .context("BuildKit build failed")?;
        Ok(tag.to_string())
    }
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
}
