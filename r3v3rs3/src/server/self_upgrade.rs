//! Installs a newer release of the server binary, in place, for the upgrade endpoint.
//!
//! The endpoint reads the latest GitHub release, picks the archive of this platform, verifies
//! its sha256 digest against the digest that the release API reports, and replaces the running
//! binary with a rename. The caller then hands the listeners to a new process over the usual
//! upgrade path.

use anyhow::{Context, bail};
use serde_derive::Deserialize;
use std::path::PathBuf;
use std::time::Duration;
use tokio::sync::{mpsc, watch};
use tracing::{info, warn};

use super::ServerCommand;

const RELEASES_API: &str = "https://api.github.com/repos/KilimcininKorOglu/r3v3rs3/releases/latest";
const USER_AGENT: &str = concat!("r3v3rs3-self-upgrade/", env!("CARGO_PKG_VERSION"));
/// The wait before the first check, so the startup of the server does not wait for GitHub.
const FIRST_CHECK_DELAY: Duration = Duration::from_secs(10);
/// The shortest check interval, so a mistyped setting cannot turn into a request loop.
const MIN_CHECK_INTERVAL: Duration = Duration::from_secs(60);

/// The newest release that this platform can run, read from the JSON of the releases API.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Release {
    /// The tag of the release, for example `v1.6.12`.
    pub tag: String,
    /// The name of the archive asset.
    pub file: String,
    /// The download URL of the archive.
    pub url: String,
    /// The sha256 digest that the API reports, without its `sha256:` prefix.
    pub digest: String,
}

#[derive(Deserialize)]
struct ReleaseJson {
    tag_name: String,
    assets: Vec<AssetJson>,
}

#[derive(Deserialize)]
struct AssetJson {
    name: String,
    browser_download_url: String,
    digest: Option<String>,
}

/// The release target that the running binary was built for. The platform detection matches
/// the installer: the architecture of the running binary, and the musl loader when it exists.
pub fn target() -> &'static str {
    let libc = if runs_on_musl() { "musl" } else { "gnu" };
    match std::env::consts::ARCH {
        "x86_64" => match libc {
            "musl" => "x86_64-unknown-linux-musl",
            _ => "x86_64-unknown-linux-gnu",
        },
        "aarch64" => match libc {
            "musl" => "aarch64-unknown-linux-musl",
            _ => "aarch64-unknown-linux-gnu",
        },
        _ => "unknown",
    }
}

/// The musl loader exists only on a musl system.
fn runs_on_musl() -> bool {
    std::fs::read_dir("/lib")
        .map(|entries| {
            entries
                .filter_map(|entry| entry.ok())
                .any(|entry| entry.file_name().to_string_lossy().starts_with("ld-musl-"))
        })
        .unwrap_or(false)
}

/// The newest release of this platform in the JSON of the releases API, when it is newer than
/// `current`. A release without the archive of this platform, or without a digest, is an error.
pub fn parse_release(json: &str, current: &str) -> anyhow::Result<Option<Release>> {
    parse_release_for(json, current, target())
}

/// [`parse_release`] for an explicit target, so a test does not depend on the host.
fn parse_release_for(json: &str, current: &str, target: &str) -> anyhow::Result<Option<Release>> {
    let release: ReleaseJson =
        serde_json::from_str(json).context("the release information is not valid JSON")?;
    if version_of(&release.tag_name) <= version_of(current) {
        return Ok(None);
    }
    let asset = release
        .assets
        .iter()
        .find(|asset| {
            asset.name == format!("r3-server-{target}.tar.xz")
                || asset.name == format!("r3v3rs3-{target}.tar.xz")
        })
        .context("the release has no archive for this platform")?;
    let digest = asset
        .digest
        .as_deref()
        .and_then(|digest| digest.strip_prefix("sha256:"))
        .context("the release asset carries no sha256 digest")?
        .to_string();
    Ok(Some(Release {
        tag: release.tag_name,
        file: asset.name.clone(),
        url: asset.browser_download_url.clone(),
        digest,
    }))
}

/// Compares the numeric parts of two versions and ignores a leading `v` and any suffix.
fn version_of(version: &str) -> Vec<u64> {
    version
        .trim_start_matches('v')
        .split(|c: char| !c.is_ascii_digit())
        .filter_map(|part| part.parse().ok())
        .collect()
}

/// Downloads the release JSON of the latest release.
pub async fn fetch_latest(
    client: &crate::cdn::fetch::HttpClient,
) -> anyhow::Result<hyper::body::Bytes> {
    let request = hyper::Request::builder()
        .method(hyper::Method::GET)
        .uri(RELEASES_API)
        .header("User-Agent", USER_AGENT)
        .header("Accept", "application/vnd.github+json")
        .body(http_body_util::Full::new(hyper::body::Bytes::new()))?;
    let response = client.request(request).await?;
    let status = response.status();
    let body = http_body_util::BodyExt::collect(response.into_body())
        .await?
        .to_bytes();
    if !status.is_success() {
        bail!("the releases API answered {status}");
    }
    Ok(body)
}

/// Checks the releases API on every interval and tells the server loop whether a newer
/// release exists. The server loop turns a change into the event that the panel banner
/// shows. The interval watcher carries the setting, so a settings change applies from the
/// next round on.
pub fn spawn_check_task(
    command: mpsc::Sender<ServerCommand>,
    mut interval: watch::Receiver<Duration>,
) {
    tokio::spawn(async move {
        tokio::time::sleep(FIRST_CHECK_DELAY).await;
        let mut last: Option<String> = None;
        loop {
            let delay = (*interval.borrow_and_update()).max(MIN_CHECK_INTERVAL);
            tokio::time::sleep(delay).await;
            match newest_release().await {
                Ok(tag) if tag != last => {
                    if command
                        .send(ServerCommand::SetUpdateAvailable { tag: tag.clone() })
                        .await
                        .is_err()
                    {
                        break;
                    }
                    last = tag;
                }
                Ok(_) => {}
                Err(err) => warn!(%err, "the update check failed"),
            }
        }
    });
}

/// The tag of the newest release, when it is newer than this build.
async fn newest_release() -> anyhow::Result<Option<String>> {
    let client = crate::cdn::fetch::build_client().await?;
    let json = fetch_latest(&client).await?;
    let release = parse_release(&String::from_utf8_lossy(&json), env!("CARGO_PKG_VERSION"))?;
    Ok(release.map(|release| release.tag))
}

/// Downloads the archive of the release. GitHub answers the asset URL with a redirect to its
/// CDN, and the shared client follows no redirects, so this loop walks them itself.
pub async fn download(
    client: &crate::cdn::fetch::HttpClient,
    url: &str,
) -> anyhow::Result<hyper::body::Bytes> {
    let mut next = url.to_string();
    for hop in 0..10 {
        let request = hyper::Request::builder()
            .method(hyper::Method::GET)
            .uri(&next)
            .header("User-Agent", USER_AGENT)
            .body(http_body_util::Full::new(hyper::body::Bytes::new()))?;
        let response = client.request(request).await?;
        let status = response.status();
        if !status.is_redirection() {
            let body = http_body_util::BodyExt::collect(response.into_body())
                .await?
                .to_bytes();
            if !status.is_success() {
                bail!("the archive download answered {status}");
            }
            return Ok(body);
        }
        let location = response
            .headers()
            .get(hyper::header::LOCATION)
            .and_then(|value| value.to_str().ok())
            .context("the redirect carries no location")?
            .to_string();
        tracing::debug!(hop, %location, "the archive download redirects");
        next = location;
    }
    bail!("the archive download redirected more than 10 times")
}

/// Verifies the digest of the archive and installs it at the path of the running binary.
/// Returns the version tag that the installation brought.
pub fn install(release: &Release, archive: &[u8]) -> anyhow::Result<()> {
    verify_archive_digest(release, archive)?;

    let exe = upgrade_exe_path()?;
    let dir = exe
        .parent()
        .context("the binary has no parent directory")?
        .to_path_buf();
    let (installed, extracted_digest) = stage_binary(release, archive, &dir)?;
    // A rename keeps the old inode open for the running process and puts the new binary at
    // the path that the upgrade spawns.
    std::fs::rename(&installed, &exe).context("failed to replace the running binary")?;
    // The release digest belongs to the compressed archive, so the binary at the path is
    // checked against the bytes that the extraction produced, not against that digest.
    verify_installed_matches(&extracted_digest, &exe)?;
    info!(tag = %release.tag, exe = %exe.display(), "installed the new release");
    Ok(())
}

/// Writes the archive into `dir`, extracts the binary and stages it as `r3v3rs3.new`. Returns
/// the staged path and the sha256 digest of the extracted bytes.
fn stage_binary(
    release: &Release,
    archive: &[u8],
    dir: &std::path::Path,
) -> anyhow::Result<(PathBuf, String)> {
    let tmp = tempfile_dir(dir, &release.file);
    std::fs::write(&tmp, archive).context("failed to write the downloaded archive")?;
    let extracted = extract(&tmp, dir).context("failed to extract the archive")?;
    std::fs::remove_file(&tmp).ok();

    use sha2::Digest as _;
    let bytes = std::fs::read(&extracted).context("failed to read the extracted binary")?;
    let extracted_digest = hex::encode(sha2::Sha256::digest(&bytes));

    let installed = dir.join("r3v3rs3.new");
    std::fs::rename(&extracted, &installed).context("failed to stage the new binary")?;
    if let Some(staging) = extracted.parent() {
        std::fs::remove_dir(staging).ok();
    }
    set_executable_permissions(&installed)?;
    Ok((installed, extracted_digest))
}

/// Checks the sha256 digest of the downloaded archive against the digest of the release.
fn verify_archive_digest(release: &Release, archive: &[u8]) -> anyhow::Result<()> {
    use sha2::Digest as _;
    let digest = hex::encode(sha2::Sha256::digest(archive));
    if digest != release.digest {
        bail!("the sha256 digest of {} does not match", release.file);
    }
    Ok(())
}

/// Checks the binary at the install path against the bytes that the extraction produced, so a
/// replacement that races the rename cannot pass for an installation.
fn verify_installed_matches(extracted_digest: &str, exe: &std::path::Path) -> anyhow::Result<()> {
    use sha2::Digest as _;
    let bytes = std::fs::read(exe).context("failed to read the installed binary")?;
    let digest = hex::encode(sha2::Sha256::digest(&bytes));
    if digest != extracted_digest {
        bail!("the installed binary does not match the extracted binary");
    }
    Ok(())
}

/// Makes the staged binary executable.
#[cfg(unix)]
fn set_executable_permissions(installed: &std::path::Path) -> anyhow::Result<()> {
    use std::os::unix::fs::PermissionsExt as _;
    std::fs::set_permissions(installed, std::fs::Permissions::from_mode(0o755))
        .context("failed to set the permissions of the new binary")
}

#[cfg(not(unix))]
fn set_executable_permissions(_installed: &std::path::Path) -> anyhow::Result<()> {
    Ok(())
}

/// The path that the upgrade spawns: the path of the running binary without the marker of a
/// replaced file.
fn upgrade_exe_path() -> anyhow::Result<PathBuf> {
    let exe = std::env::current_exe().context("failed to locate the running binary")?;
    let path = exe.to_string_lossy();
    match path.strip_suffix(" (deleted)") {
        Some(path) => Ok(path.into()),
        None => Ok(exe),
    }
}

/// A temporary path in `dir` that carries the process id, so two upgrades never collide.
fn tempfile_dir(dir: &std::path::Path, name: &str) -> PathBuf {
    dir.join(format!(".{name}.{}", std::process::id()))
}

/// Extracts the single binary that the archive carries into a staging directory of `dir`.
/// The archive is extracted without a member name: GNU tar refuses to replace an existing
/// file of a named member, and the target of an upgrade always exists.
fn extract(archive: &std::path::Path, dir: &std::path::Path) -> anyhow::Result<PathBuf> {
    let staging = dir.join(format!(".upgrade-extract.{}", std::process::id()));
    std::fs::remove_dir_all(&staging).ok();
    std::fs::create_dir_all(&staging).context("failed to create the extraction directory")?;
    let status = std::process::Command::new("tar")
        .arg("-xJf")
        .arg(archive)
        .arg("-C")
        .arg(&staging)
        .status()
        .context("tar is not available")?;
    let extracted = if staging.join("r3-server").exists() {
        staging.join("r3-server")
    } else {
        staging.join("r3v3rs3")
    };
    if !status.success() {
        std::fs::remove_dir_all(&staging).ok();
        bail!("tar exited with {status}");
    }
    Ok(extracted)
}

#[cfg(test)]
mod tests {
    use super::*;

    const TARGET: &str = "x86_64-unknown-linux-gnu";

    const JSON: &str = r#"{
      "tag_name": "v9.9.9",
      "assets": [
        {"name": "r3v3rs3-x86_64-unknown-linux-gnu.tar.xz",
         "browser_download_url": "https://example.test/gnu.tar.xz",
         "digest": "sha256:abc123"},
        {"name": "r3v3rs3-x86_64-pc-windows-msvc.zip",
         "browser_download_url": "https://example.test/win.zip",
         "digest": "sha256:def456"}
      ]
    }"#;

    #[test]
    fn a_newer_release_picks_the_asset_of_the_target() {
        let release = parse_release_for(JSON, "1.0.0", TARGET).unwrap().unwrap();
        assert_eq!(release.tag, "v9.9.9");
        assert_eq!(release.digest, "abc123");
        assert_eq!(release.url, "https://example.test/gnu.tar.xz");
    }

    #[test]
    fn the_running_version_is_not_an_upgrade() {
        assert!(parse_release_for(JSON, "9.9.9", TARGET).unwrap().is_none());
        assert!(parse_release_for(JSON, "v9.9.9", TARGET).unwrap().is_none());
        assert!(parse_release_for(JSON, "10.0.0", TARGET).unwrap().is_none());
    }

    #[test]
    fn a_release_without_the_platform_archive_is_an_error() {
        let json = JSON.replace("x86_64-unknown-linux-gnu.tar.xz", "other.tar.xz");
        assert!(parse_release_for(&json, "1.0.0", TARGET).is_err());
    }

    #[test]
    fn an_asset_without_a_digest_is_an_error() {
        let json = r#"{"tag_name": "v9.9.9", "assets": [
            {"name": "r3v3rs3-x86_64-unknown-linux-gnu.tar.xz",
             "browser_download_url": "https://example.test/gnu.tar.xz"}]}"#;
        assert!(parse_release_for(json, "1.0.0", TARGET).is_err());
    }

    #[test]
    fn the_target_names_the_running_platform() {
        assert!(target().starts_with(std::env::consts::ARCH));
        assert!(target().ends_with("gnu") || target().ends_with("musl"));
    }

    #[tokio::test]
    async fn the_download_follows_a_redirect() {
        use std::io::Write as _;
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let addr = listener.local_addr().unwrap();
        let base = format!("http://{addr}");
        std::thread::spawn(move || {
            let (mut stream, _) = listener.accept().unwrap();
            let mut buf = [0u8; 1024];
            std::io::Read::read(&mut stream, &mut buf).ok();
            let redirect = format!("HTTP/1.1 302 Found\r\nLocation: {base}/file\r\n\r\n");
            stream.write_all(redirect.as_bytes()).unwrap();
            let (mut stream, _) = listener.accept().unwrap();
            let mut buf = [0u8; 1024];
            std::io::Read::read(&mut stream, &mut buf).ok();
            stream
                .write_all(b"HTTP/1.1 200 OK\r\nContent-Length: 4\r\n\r\nbody")
                .unwrap();
        });
        let client = crate::cdn::fetch::build_client().await.unwrap();
        let body = download(&client, &format!("http://{addr}/asset"))
            .await
            .expect("download failed");
        assert_eq!(&body[..], b"body");
    }

    #[test]
    fn the_extract_replaces_an_existing_binary() {
        let dir = std::env::temp_dir().join(format!("r33-extract-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("r3v3rs3"), b"the old binary").unwrap();
        let staging = dir.join("archive-source");
        std::fs::create_dir_all(&staging).unwrap();
        std::fs::write(staging.join("r3v3rs3"), b"the new binary").unwrap();
        let status = std::process::Command::new("tar")
            .arg("-cJf")
            .arg(dir.join("archive.tar.xz"))
            .arg("-C")
            .arg(&staging)
            .arg("r3v3rs3")
            .status()
            .unwrap();
        assert!(status.success());
        let extracted = extract(&dir.join("archive.tar.xz"), &dir).unwrap();
        assert_eq!(std::fs::read(&extracted).unwrap(), b"the new binary");
        std::fs::remove_dir_all(&dir).ok();
    }
}
