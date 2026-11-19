//! Volume backups, restores, exports and imports on a Docker Engine. The app runs an image
//! without a shell and without `tar`, so the helper container does every step; a separate
//! BusyBox container of the test writes and reads the volume.

use super::Platform;
use super::bounded_runtime::BoundedRuntime;
use super::tests::{TempDir, platform_with, request};
use anyhow::ensure;
use r3v3rs3_api::container::{
    ContainerName, ContainerSpec, ResourceLimits, RestartPolicy, VolumeMount,
};
use r3v3rs3_api::id::ShortId;
use r3v3rs3_api::platform::{AppEntry, AppSource, DeploymentTrigger, PlatformConfig};
use std::collections::BTreeMap;
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::mpsc;

/// An image without a shell and without `tar`.
const APP_IMAGE: &str = "mirror.gcr.io/traefik/whoami:v1.10.3";
const CHECK_IMAGE: &str = "mirror.gcr.io/library/busybox:1.38.0";

async fn bounded_platform() -> anyhow::Result<(Arc<Platform>, Arc<BoundedRuntime>, TempDir)> {
    let (command, _) = mpsc::channel(1);
    let (mut platform, _fake, dir) = platform_with(&PlatformConfig::default(), command).await?;
    let host = std::env::var("DOCKER_HOST").unwrap_or("unix:///var/run/docker.sock".into());
    let bounded = Arc::new(BoundedRuntime::new(Arc::new(super::docker_runtime(&host)?)));
    platform.local = bounded.clone();
    platform.timing.health_timeout = Duration::from_secs(120);
    Ok((Arc::new(platform), bounded, dir))
}

/// A running app with the volume `<name>-vol` at `/data`.
async fn running_app(platform: &Arc<Platform>, name: &str) -> anyhow::Result<AppEntry> {
    let mut req = request(name);
    req.spec.source = AppSource::Image {
        image: APP_IMAGE.parse()?,
    };
    req.spec.port = 80;
    req.spec.volumes = vec![VolumeMount {
        volume: format!("{name}-vol").parse()?,
        target: "/data".parse()?,
        read_only: false,
    }];
    let app = platform.add_app(req, 1).await?;
    platform
        .deploy(app.id, "alice", DeploymentTrigger::Manual)
        .await?;
    wait_serving(platform, app.id).await?;
    Ok(app)
}

async fn wait_serving(platform: &Arc<Platform>, id: ShortId) -> anyhow::Result<()> {
    let deadline = tokio::time::Instant::now() + Duration::from_secs(240);
    loop {
        let latest = platform.deployments(id).await?.into_iter().next();
        let serving = platform.store.running_deployment(id).await?;
        if !platform.busy_apps().contains(&id) && latest.as_ref().map(|d| d.id) == serving {
            return Ok(());
        }
        ensure!(
            tokio::time::Instant::now() < deadline,
            "{id} does not serve: {latest:?}"
        );
        tokio::time::sleep(Duration::from_millis(500)).await;
    }
}

/// Runs `script` in a BusyBox container of the test with the volume at `/v`, and returns
/// its output.
async fn in_volume(platform: &Arc<Platform>, volume: &str, script: &str) -> anyhow::Result<String> {
    let runtime = platform.local.clone();
    let name: ContainerName = format!("r3v3rs3-volcheck-{:08x}", rand::random::<u32>()).parse()?;
    let spec = ContainerSpec {
        name: name.clone(),
        image: CHECK_IMAGE.parse()?,
        env: Vec::new(),
        labels: BTreeMap::new(),
        network: None,
        volumes: vec![VolumeMount {
            volume: volume.parse()?,
            target: "/v".parse()?,
            read_only: false,
        }],
        restart: RestartPolicy::No,
        limits: ResourceLimits::default(),
        publish: None,
        host_port: None,
        aliases: Vec::new(),
        extra_networks: Vec::new(),
        log_max_size: None,
        log_max_file: None,
        command: Some(vec!["sleep".into(), "600".into()]),
    };
    runtime.pull_image(&spec.image).await?;
    runtime.create_container(&spec).await?;
    let run = async {
        runtime.start_container(&name).await?;
        let command = vec!["sh".into(), "-c".into(), script.to_string()];
        runtime.exec_capture(&name, command, Vec::new()).await
    }
    .await;
    runtime.remove_container(&name).await?;
    Ok(String::from_utf8_lossy(&run?).trim().to_string())
}

/// The owner, the mode and the checksum of the files of the volume, one line each.
const LISTING: &str = "cd /v && find . -type f | sort | while read f; do \
     echo \"$f $(stat -c '%u:%g %a' \"$f\") $(md5sum < \"$f\" | cut -d' ' -f1)\"; done";

/// Fills the volume with a file of random content larger than one transfer piece, and a file
/// with an owner and a mode of its own.
const FILL: &str = "mkdir -p /v/sub && head -c 41943040 /dev/urandom > /v/sub/big && \
     echo secret > /v/owned && chown 999:998 /v/owned && chmod 640 /v/owned";

/// A volume backup of the required behavior: the archive leaves the helper and comes back in
/// bounded pieces, and a restore brings every file back to its place, with its owner and its
/// mode, and not into a directory of the same name inside the volume.
async fn back_up_and_restore(
    platform: &Arc<Platform>,
    bounded: &BoundedRuntime,
    name: &str,
) -> anyhow::Result<()> {
    let app = running_app(platform, name).await?;
    let volume = format!("{name}-vol");
    in_volume(platform, &volume, FILL).await?;
    let before = in_volume(platform, &volume, LISTING).await?;
    let backup = platform.create_volume_backup(app.id, &volume).await?;
    in_volume(platform, &volume, "rm -rf /v/* && echo stray > /v/stray").await?;
    platform
        .restore_volume_backup(app.id, &volume, &backup.id)
        .await?;
    let after = in_volume(platform, &volume, LISTING).await?;
    let expected = format!("{before}\n./stray");
    ensure!(
        after
            .lines()
            .filter(|l| !l.starts_with("./stray"))
            .collect::<Vec<_>>()
            == before.lines().collect::<Vec<_>>(),
        "the volume holds\n{after}\ninstead of\n{expected}"
    );
    let violations = bounded.violations();
    ensure!(
        violations.is_empty(),
        "whole transfers in memory: {violations:?}"
    );
    Ok(())
}

/// A backup that an older release took with the Docker archive API holds the mount
/// directory itself as its first entry; its restore lands in the volume, not in a directory
/// `data` inside it.
async fn restore_an_old_backup(platform: &Arc<Platform>, name: &str) -> anyhow::Result<()> {
    let app = running_app(platform, name).await?;
    let volume = format!("{name}-vol");
    let backup = platform.create_volume_backup(app.id, &volume).await?;
    let dir = platform
        .backup_dir
        .join("volumes")
        .join(app.name.as_str())
        .join(&volume);
    let old = old_archive().await?;
    tokio::fs::write(dir.join(format!("{}.tar.gz", backup.id)), old).await?;
    platform
        .restore_volume_backup(app.id, &volume, &backup.id)
        .await?;
    let found = in_volume(platform, &volume, "find /v -type f | sort").await?;
    ensure!(found == "/v/sub/old", "the volume holds {found}");
    Ok(())
}

/// An archive of the layout that the Docker archive API gives for `/data`.
async fn old_archive() -> anyhow::Result<Vec<u8>> {
    let mut tar = tar::Builder::new(Vec::new());
    let mut header = tar::Header::new_gnu();
    header.set_entry_type(tar::EntryType::Directory);
    header.set_mode(0o755);
    header.set_size(0);
    tar.append_data(&mut header, "data/", std::io::empty())?;
    let mut header = tar::Header::new_gnu();
    header.set_mode(0o644);
    header.set_size(3);
    tar.append_data(&mut header, "data/sub/old", &b"old"[..])?;
    super::backup::gzip(&tar.into_inner()?).await
}

/// An archive that an export gave imports back, also when it is larger than the old 512 MiB
/// limit of an import: a volume of any size that the disk holds moves.
async fn import_and_export(platform: &Arc<Platform>, name: &str) -> anyhow::Result<()> {
    let app = running_app(platform, name).await?;
    let volume = format!("{name}-vol");
    import_a_large_archive(platform, app.id, &volume).await?;
    in_volume(platform, &volume, "rm -f /v/zeros && echo kept > /v/kept").await?;
    let (mut file, _) = platform.export_volume_archive(app.id, &volume).await?;
    let mut exported = Vec::new();
    tokio::io::AsyncReadExt::read_to_end(&mut file, &mut exported).await?;
    in_volume(platform, &volume, "rm -rf /v/*").await?;
    platform
        .import_volume_archive(app.id, &volume, body(exported))
        .await?;
    let found = in_volume(platform, &volume, "cat /v/kept").await?;
    ensure!(found == "kept", "the volume holds {found}");
    Ok(())
}

/// Imports a file of 600 MiB, past the old limit, and checks its size in the volume.
async fn import_a_large_archive(
    platform: &Arc<Platform>,
    id: ShortId,
    volume: &str,
) -> anyhow::Result<()> {
    let archive = zeros_archive(600 * 1024 * 1024)?;
    platform
        .import_volume_archive(id, volume, body(archive))
        .await?;
    let found = in_volume(platform, volume, "wc -c < /v/zeros").await?;
    ensure!(
        found == "629145600",
        "the imported file holds {found} bytes"
    );
    Ok(())
}

/// A `.tar.gz` that holds the file `zeros` of `size` zero bytes, written in pieces.
fn zeros_archive(size: u64) -> anyhow::Result<Vec<u8>> {
    use std::io::Read as _;
    let encoder = flate2::write::GzEncoder::new(Vec::new(), flate2::Compression::fast());
    let mut tar = tar::Builder::new(encoder);
    let mut header = tar::Header::new_gnu();
    header.set_mode(0o644);
    header.set_size(size);
    tar.append_data(&mut header, "zeros", std::io::repeat(0).take(size))?;
    Ok(tar.into_inner()?.finish()?)
}

/// A request body of `content` in pieces of 64 KiB.
fn body(
    content: Vec<u8>,
) -> futures::stream::Iter<std::vec::IntoIter<Result<bytes::Bytes, std::io::Error>>> {
    let pieces: Vec<_> = content
        .chunks(64 * 1024)
        .map(|piece| Ok(bytes::Bytes::copy_from_slice(piece)))
        .collect();
    futures::stream::iter(pieces)
}

async fn remove_apps(platform: &Arc<Platform>) -> anyhow::Result<()> {
    for app in platform.apps().await? {
        platform.delete_app(app.id).await?;
    }
    Ok(())
}

fn unique(prefix: &str) -> String {
    format!("{prefix}{}", hex::encode(rand::random::<[u8; 3]>()))
}

#[tokio::test]
#[ignore = "needs a Docker Engine; run with make test-runtime-docker"]
async fn a_volume_backs_up_and_restores_in_pieces_on_the_docker_engine() -> anyhow::Result<()> {
    let (platform, bounded, _dir) = bounded_platform().await?;
    let run = back_up_and_restore(&platform, &bounded, &unique("vol")).await;
    run.and(remove_apps(&platform).await)
}

#[tokio::test]
#[ignore = "needs a Docker Engine; run with make test-runtime-docker"]
async fn an_old_volume_backup_restores_into_the_volume_on_the_docker_engine() -> anyhow::Result<()>
{
    let (platform, _bounded, _dir) = bounded_platform().await?;
    let run = restore_an_old_backup(&platform, &unique("vold")).await;
    run.and(remove_apps(&platform).await)
}

#[tokio::test]
#[ignore = "needs a Docker Engine; run with make test-runtime-docker"]
async fn a_volume_imports_and_exports_on_the_docker_engine() -> anyhow::Result<()> {
    let (platform, _bounded, _dir) = bounded_platform().await?;
    let run = import_and_export(&platform, &unique("vimp")).await;
    run.and(remove_apps(&platform).await)
}
