//! The local control socket: `<config dir>/control.sock`, mode 0600.
//!
//! The socket lets the operator of the host release an address that the auto-quarantine or the
//! sign-in ban blocked, without a panel session, for example when the operator is the blocked
//! client. The permissions of the file are the only authorization: root and the service user
//! can connect. A client writes one line, `unblock <ip>`, and reads one line: `ok quarantine=<bool>
//! login_ban=<bool>`, where each value tells whether the address was blocked there, or
//! `error <message>`.

use super::AppState;
use crate::accounts::Caller;
use crate::audit::AuditRecord;
use crate::server::rpc::quarantine::UnquarantineIp;
use anyhow::Context as _;
use r3v3rs3_api::audit::AuditAction;
use r3v3rs3_api::auth::Role;
use std::net::IpAddr;
use std::os::unix::fs::PermissionsExt as _;
use std::path::{Path, PathBuf};
use tokio::io::{AsyncBufReadExt as _, AsyncReadExt as _, AsyncWriteExt as _, BufReader};
use tokio::net::{UnixListener, UnixStream};
use tracing::{error, info};

/// The file name of the socket in the config directory.
pub const SOCKET_FILE: &str = "control.sock";

/// The account name that the audit log shows for the changes of the socket.
pub const ACTOR: &str = "local-control";

/// The longest request line. `unblock` and an IPv6 address fit many times.
const MAX_LINE: usize = 256;

/// The path of the socket in the config directory.
pub fn socket_path(config_dir: &Path) -> PathBuf {
    config_dir.join(SOCKET_FILE)
}

/// Opens the socket and serves it in the background. A failure is logged and the server keeps
/// running, because the socket is a recovery aid and must not stop the proxy.
pub fn spawn(state: AppState, config_dir: &Path) {
    let path = socket_path(config_dir);
    match bind(&path) {
        Ok(listener) => {
            info!(path = %path.display(), "the local control socket listens");
            tokio::spawn(serve(listener, state));
        }
        Err(err) => {
            error!(path = %path.display(), "failed to open the local control socket: {err:#}")
        }
    }
}

/// Binds the socket with mode 0600. A file that a previous process left is replaced. The file
/// is never removed on exit, because during an upgrade the new process binds it while the old
/// one still runs.
fn bind(path: &Path) -> anyhow::Result<UnixListener> {
    match std::fs::remove_file(path) {
        Ok(()) => {}
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => {}
        Err(err) => return Err(err).context("failed to remove the old socket file"),
    }
    let listener = UnixListener::bind(path).context("failed to bind the socket")?;
    std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o600))
        .context("failed to restrict the socket to its owner")?;
    Ok(listener)
}

async fn serve(listener: UnixListener, state: AppState) {
    loop {
        match listener.accept().await {
            Ok((stream, _)) => {
                tokio::spawn(answer(stream, state.clone()));
            }
            Err(err) => {
                error!(%err, "the local control socket stopped accepting");
                return;
            }
        }
    }
}

/// Reads one request line and writes its answer.
async fn answer(stream: UnixStream, state: AppState) {
    let (read, mut write) = stream.into_split();
    let mut line = String::new();
    let reply = match BufReader::new(read.take(MAX_LINE as u64))
        .read_line(&mut line)
        .await
    {
        Ok(_) => handle(&state, line.trim()).await,
        Err(err) => format!("error {err}"),
    };
    if let Err(err) = write.write_all(format!("{reply}\n").as_bytes()).await {
        error!(%err, "failed to answer on the local control socket");
    }
}

/// The answer to one request line.
async fn handle(state: &AppState, request: &str) -> String {
    let Some(address) = request.strip_prefix("unblock ") else {
        return format!("error unknown request: {request}");
    };
    match address.trim().parse::<IpAddr>() {
        Ok(ip) => match unblock(state, ip).await {
            Ok((quarantine, login_ban)) => {
                format!("ok quarantine={quarantine} login_ban={login_ban}")
            }
            Err(err) => format!("error {err}"),
        },
        Err(_) => format!("error invalid IP address: {}", address.trim()),
    }
}

/// Releases the address from the auto-quarantine and the sign-in ban. Returns whether each one
/// blocked it.
async fn unblock(state: &AppState, ip: IpAddr) -> Result<(bool, bool), r3v3rs3_api::error::Error> {
    let caller = Caller {
        username: ACTOR.to_string(),
        role: Role::Admin,
        proxies: None,
        client: None,
    };
    let quarantine = *state.call(&caller, UnquarantineIp { ip }).await?;
    let login_ban = state.data.lock().await.ban_tracker.unban(&ip);
    if login_ban {
        let record = AuditRecord::new(AuditAction::UnbanIp).id(ip.to_string());
        state.record_audit(ACTOR, None, record).await;
    }
    info!(%ip, quarantine, login_ban, "the local control socket released an address");
    Ok((quarantine, login_ban))
}

/// Sends `unblock <ip>` to the socket of a running server and returns its answer without the
/// `ok ` prefix. An `error` answer becomes the error.
pub async fn request_unblock(config_dir: &Path, ip: IpAddr) -> anyhow::Result<String> {
    let path = socket_path(config_dir);
    let stream = UnixStream::connect(&path).await.with_context(|| {
        format!(
            "failed to connect to {}: is the server running, and is this its config directory? \
             Pass it with --config-dir or R3V3RS3_CONFIG_DIR, for example /etc/r3v3rs3",
            path.display()
        )
    })?;
    let (read, mut write) = stream.into_split();
    write
        .write_all(format!("unblock {ip}\n").as_bytes())
        .await?;
    let mut line = String::new();
    BufReader::new(read).read_line(&mut line).await?;
    let line = line.trim();
    match line.strip_prefix("ok ") {
        Some(result) => Ok(result.to_string()),
        None => anyhow::bail!(
            "the server refused the request: {}",
            line.strip_prefix("error ").unwrap_or(line)
        ),
    }
}
