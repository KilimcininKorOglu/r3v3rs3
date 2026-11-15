//! The local control socket releases a quarantined address without a panel session.

use r3v3rs3::admin::control::{request_unblock, socket_path};
use r3v3rs3::admin::start_admin;
use r3v3rs3::config::new_appinfo;
use r3v3rs3::log::DatabaseLayer;
use r3v3rs3::server::rpc::quarantine::QuarantineIp;
use std::net::IpAddr;
use std::os::unix::fs::PermissionsExt as _;
use std::path::{Path, PathBuf};
use std::time::Duration;
use tokio::io::{AsyncBufReadExt as _, AsyncWriteExt as _, BufReader};
use tracing_subscriber::filter::LevelFilter;

use crate::common;
use common::{TestStorage, alloc_tcp_port, call, with_server};

const BLOCKED: &str = "203.0.113.90";

/// A short directory: the path of a Unix socket has a limit of about 100 bytes.
fn short_dir() -> PathBuf {
    PathBuf::from(format!("/tmp/r3c-{}", std::process::id()))
}

/// Waits until the admin API opened the socket.
async fn wait_for_socket(path: &Path) -> anyhow::Result<()> {
    for _ in 0..50 {
        if path.exists() {
            return Ok(());
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    anyhow::bail!("{} did not appear", path.display())
}

/// Sends a raw request line and returns the answer line.
async fn raw_request(path: &Path, line: &str) -> anyhow::Result<String> {
    let stream = tokio::net::UnixStream::connect(path).await?;
    let (read, mut write) = stream.into_split();
    write.write_all(format!("{line}\n").as_bytes()).await?;
    let mut answer = String::new();
    BufReader::new(read).read_line(&mut answer).await?;
    Ok(answer.trim().to_string())
}

/// Checks the answers of the socket for a quarantined address.
async fn check_unblock(dir: &Path) -> anyhow::Result<()> {
    let path = socket_path(dir);
    wait_for_socket(&path).await?;
    let mode = std::fs::metadata(&path)?.permissions().mode() & 0o777;
    assert_eq!(mode, 0o600, "only the owner may connect");
    let ip: IpAddr = BLOCKED.parse()?;
    assert_eq!(
        request_unblock(dir, ip).await?,
        "quarantine=true login_ban=false"
    );
    assert_eq!(
        request_unblock(dir, ip).await?,
        "quarantine=false login_ban=false",
        "the first request released the address"
    );
    let answer = raw_request(&path, "unblock example.com").await?;
    assert_eq!(answer, "error invalid IP address: example.com");
    Ok(())
}

#[tokio::test]
async fn the_control_socket_releases_a_quarantined_address() -> anyhow::Result<()> {
    let dir = short_dir();
    std::fs::create_dir_all(&dir)?;
    DatabaseLayer::new(&dir.join("log.db"), LevelFilter::INFO).await?;
    let addr = alloc_tcp_port().await?.socket_addr();
    let app_info = new_appinfo(&dir, &dir);
    let socket_dir = dir.clone();
    with_server(TestStorage::builder().build(), |mut channels| async move {
        let quarantine = QuarantineIp {
            ip: BLOCKED.parse()?,
            reason: "manual".into(),
            duration_seconds: 600,
        };
        call(&mut channels, quarantine).await??;
        tokio::spawn(start_admin(
            app_info,
            addr,
            channels.command,
            channels.callback,
            channels.event.clone(),
            channels.accounts.clone(),
        ));
        check_unblock(&socket_dir).await
    })
    .await?;
    std::fs::remove_dir_all(&dir)?;
    Ok(())
}
