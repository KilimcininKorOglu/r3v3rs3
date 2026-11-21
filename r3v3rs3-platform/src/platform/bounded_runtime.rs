//! A test runtime that passes every call to a real runtime and records each transfer that holds
//! more than one bounded piece in memory: a command output over the read piece, a file write
//! over the write piece, or a whole-directory archive in either direction. The Docker tests of
//! the archive and recovery paths run on it and fail on every recorded transfer, so a path that
//! still moves a whole backup through memory fails its test.

use crate::runtime::ContainerRuntime;
use crate::runtime::docker::ExecHandle;
use r3v3rs3_api::container::{
    AppName, ContainerInfo, ContainerName, ContainerSpec, ContainerStats, ContainerSummary,
    ImageInfo, ImageRef, NetworkName, ProjectName, ResourceLimits,
};
use r3v3rs3_api::git::RelPath;
use r3v3rs3_api::platform::{BuildKitConfig, FileEntry};
use std::sync::{Arc, Mutex};
use std::time::Duration;

/// The largest command output that one read of `transfer::fetch_to_file` asks for, plus room
/// for the frame headers of the exec stream.
const READ_PIECE: usize = 32 * 1024 * 1024 + 64 * 1024;

/// The largest file that one write of `transfer::push_file` sends.
const WRITE_PIECE: usize = 8 * 1024 * 1024;

pub(super) struct BoundedRuntime {
    inner: Arc<dyn ContainerRuntime>,
    violations: Mutex<Vec<String>>,
}

impl BoundedRuntime {
    pub(super) fn new(inner: Arc<dyn ContainerRuntime>) -> Self {
        Self {
            inner,
            violations: Mutex::new(Vec::new()),
        }
    }

    /// Every transfer that held more than one piece in memory.
    pub(super) fn violations(&self) -> Vec<String> {
        self.violations
            .lock()
            .map(|list| list.clone())
            .unwrap_or_else(|poisoned| poisoned.into_inner().clone())
    }

    fn record(&self, violation: String) {
        match self.violations.lock() {
            Ok(mut list) => list.push(violation),
            Err(poisoned) => poisoned.into_inner().push(violation),
        }
    }
}

#[async_trait::async_trait]
impl ContainerRuntime for BoundedRuntime {
    async fn version(&self) -> anyhow::Result<String> {
        self.inner.version().await
    }

    async fn pull_image(&self, image: &ImageRef) -> anyhow::Result<()> {
        self.inner.pull_image(image).await
    }

    async fn pull_image_with_auth(
        &self,
        image: &ImageRef,
        auth: Option<&r3v3rs3_api::platform::RegistryAuth>,
    ) -> anyhow::Result<()> {
        self.inner.pull_image_with_auth(image, auth).await
    }

    async fn container_log_info(
        &self,
        name: &ContainerName,
    ) -> anyhow::Result<Option<r3v3rs3_api::container::ContainerLogInfo>> {
        self.inner.container_log_info(name).await
    }

    async fn disk_space(&self) -> anyhow::Result<Option<r3v3rs3_api::dashboard::DiskSpace>> {
        self.inner.disk_space().await
    }

    async fn list_project_containers(
        &self,
        project: &ProjectName,
    ) -> anyhow::Result<Vec<ContainerSummary>> {
        self.inner.list_project_containers(project).await
    }

    async fn logs_stream(
        &self,
        name: &ContainerName,
        tail: u32,
    ) -> anyhow::Result<(bool, hyper::Response<hyper::body::Incoming>)> {
        self.inner.logs_stream(name, tail).await
    }

    async fn prune_platform(
        &self,
        policy: &r3v3rs3_api::platform::PrunePolicy,
        keep: &crate::runtime::PruneKeep,
    ) -> anyhow::Result<r3v3rs3_api::platform::PruneResult> {
        self.inner.prune_platform(policy, keep).await
    }

    async fn run_prune(
        &self,
        policy: &r3v3rs3_api::platform::PrunePolicy,
        keep: &crate::runtime::PruneKeep,
    ) -> anyhow::Result<r3v3rs3_api::platform::PruneResult> {
        self.inner.run_prune(policy, keep).await
    }

    async fn vacuum_all_logs(
        &self,
        threshold_bytes: Option<u64>,
    ) -> anyhow::Result<r3v3rs3_api::container::LogVacuumResult> {
        self.inner.vacuum_all_logs(threshold_bytes).await
    }

    async fn vacuum_container_logs(
        &self,
        name: &ContainerName,
    ) -> anyhow::Result<r3v3rs3_api::container::LogVacuumResult> {
        self.inner.vacuum_container_logs(name).await
    }

    async fn inspect_image(&self, image: &ImageRef) -> anyhow::Result<Option<ImageInfo>> {
        self.inner.inspect_image(image).await
    }

    async fn build_image(
        &self,
        context: Vec<u8>,
        dockerfile: &RelPath,
        tag: &ImageRef,
        buildkit: Option<&BuildKitConfig>,
    ) -> anyhow::Result<String> {
        self.inner
            .build_image(context, dockerfile, tag, buildkit)
            .await
    }

    async fn remove_image(&self, image: &ImageRef) -> anyhow::Result<()> {
        self.inner.remove_image(image).await
    }

    async fn prune_project_images(&self, project: &ProjectName, all: bool) -> anyhow::Result<()> {
        self.inner.prune_project_images(project, all).await
    }

    async fn ensure_network(&self, network: &NetworkName, app: &AppName) -> anyhow::Result<()> {
        self.inner.ensure_network(network, app).await
    }

    async fn remove_network(&self, network: &NetworkName) -> anyhow::Result<()> {
        self.inner.remove_network(network).await
    }

    async fn create_container(&self, spec: &ContainerSpec) -> anyhow::Result<String> {
        self.inner.create_container(spec).await
    }

    async fn start_container(&self, name: &ContainerName) -> anyhow::Result<()> {
        self.inner.start_container(name).await
    }

    async fn stop_container(&self, name: &ContainerName, timeout: Duration) -> anyhow::Result<()> {
        self.inner.stop_container(name, timeout).await
    }

    async fn remove_container(&self, name: &ContainerName) -> anyhow::Result<()> {
        self.inner.remove_container(name).await
    }

    async fn inspect_container(
        &self,
        name: &ContainerName,
    ) -> anyhow::Result<Option<ContainerInfo>> {
        self.inner.inspect_container(name).await
    }

    async fn list_containers(&self, app: &AppName) -> anyhow::Result<Vec<ContainerSummary>> {
        self.inner.list_containers(app).await
    }

    async fn update_container_limits(
        &self,
        name: &ContainerName,
        limits: &ResourceLimits,
    ) -> anyhow::Result<()> {
        self.inner.update_container_limits(name, limits).await
    }

    async fn logs(&self, name: &ContainerName, tail: u32) -> anyhow::Result<String> {
        self.inner.logs(name, tail).await
    }

    async fn exec(
        &self,
        name: &ContainerName,
        cols: u16,
        rows: u16,
        cmd: &[&str],
    ) -> anyhow::Result<ExecHandle> {
        self.inner.exec(name, cols, rows, cmd).await
    }

    async fn exec_capture(
        &self,
        name: &ContainerName,
        command: Vec<String>,
        env: Vec<String>,
    ) -> anyhow::Result<Vec<u8>> {
        let first = command.first().cloned().unwrap_or_default();
        let out = self.inner.exec_capture(name, command, env).await?;
        if out.len() > READ_PIECE {
            self.record(format!("{first} returned {} bytes", out.len()));
        }
        Ok(out)
    }

    async fn stats(&self, name: &ContainerName) -> anyhow::Result<ContainerStats> {
        self.inner.stats(name).await
    }

    async fn list_dir(&self, name: &ContainerName, path: &str) -> anyhow::Result<Vec<FileEntry>> {
        self.inner.list_dir(name, path).await
    }

    async fn download_file(&self, name: &ContainerName, path: &str) -> anyhow::Result<Vec<u8>> {
        let out = self.inner.download_file(name, path).await?;
        if out.len() > READ_PIECE {
            self.record(format!("download of {path}: {} bytes", out.len()));
        }
        Ok(out)
    }

    async fn upload_file(
        &self,
        name: &ContainerName,
        path: &str,
        bytes: Vec<u8>,
    ) -> anyhow::Result<()> {
        if bytes.len() > WRITE_PIECE {
            self.record(format!("upload of {path}: {} bytes", bytes.len()));
        }
        self.inner.upload_file(name, path, bytes).await
    }

    async fn remove_path(&self, name: &ContainerName, path: &str) -> anyhow::Result<()> {
        self.inner.remove_path(name, path).await
    }

    async fn download_archive(&self, name: &ContainerName, path: &str) -> anyhow::Result<Vec<u8>> {
        let out = self.inner.download_archive(name, path).await?;
        self.record(format!("archive download of {path}: {} bytes", out.len()));
        Ok(out)
    }

    async fn upload_archive(
        &self,
        name: &ContainerName,
        path: &str,
        archive_tar: Vec<u8>,
    ) -> anyhow::Result<()> {
        self.record(format!(
            "archive upload to {path}: {} bytes",
            archive_tar.len()
        ));
        self.inner.upload_archive(name, path, archive_tar).await
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::platform::fake::FakeRuntime;

    /// Source: the module contract, "a test runtime that passes every call to a real runtime".
    /// A call that reached the trait default instead would fail with "does not ...".
    #[tokio::test]
    async fn every_call_reaches_the_wrapped_runtime() -> anyhow::Result<()> {
        let bounded = BoundedRuntime::new(Arc::new(FakeRuntime::default()));
        let policy = r3v3rs3_api::platform::PrunePolicy::default();
        let keep = crate::runtime::PruneKeep::default();
        let pruned = bounded.run_prune(&policy, &keep).await?;
        assert_eq!(pruned.space_reclaimed_bytes, 100);
        let pruned = bounded.prune_platform(&policy, &keep).await?;
        assert_eq!(pruned.space_reclaimed_bytes, 10);
        assert!(bounded.disk_space().await?.is_some());
        let project: ProjectName = "r3v3rs3-abc".parse()?;
        assert!(bounded.list_project_containers(&project).await?.is_empty());
        let name: ContainerName = "r3v3rs3-abc-1".parse()?;
        assert!(bounded.container_log_info(&name).await?.is_none());
        bounded.vacuum_container_logs(&name).await?;
        Ok(())
    }
}
