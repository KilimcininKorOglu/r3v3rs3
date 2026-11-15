//! The systemd `sd_notify` messages of the service, through the `NOTIFY_SOCKET` that systemd
//! gives the process. Outside systemd, where the variable is unset, every call is a no-op.
//!
//! The service runs as `Type=notify`: it answers `READY=1` when its listeners are bound, and a
//! zero-downtime upgrade hands the main process over with `MAINPID`, so systemd keeps tracking
//! the new process when the old one drains and exits.

use std::os::unix::net::UnixDatagram;
use std::path::PathBuf;

/// Sends one `sd_notify` state to systemd. A failure stays quiet, because a message that
/// systemd misses must not take the server down.
pub fn notify(state: &str) {
    let Some(socket) = std::env::var_os("NOTIFY_SOCKET") else {
        return;
    };
    let mut path = PathBuf::from(socket);
    // An abstract socket name starts with '@' and binds without a file.
    if path
        .file_name()
        .is_some_and(|name| name.to_string_lossy().starts_with('@'))
        && let Some(name) = path.file_name()
    {
        let mut raw = PathBuf::from("\0");
        raw.as_mut_os_string().push(name);
        path = raw;
    }
    match UnixDatagram::unbound().and_then(|datagram| datagram.send_to(state.as_bytes(), &path)) {
        Ok(_) => {}
        Err(err) => tracing::debug!(%err, state, "the sd_notify message failed"),
    }
}

/// Tells systemd that the startup is over and the service serves.
pub fn ready() {
    notify("READY=1");
}

/// Tells systemd that `pid` is the new main process, after a zero-downtime upgrade.
pub fn hand_over_main_pid(pid: u32) {
    notify(&format!("MAINPID={pid}"));
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn without_a_socket_the_calls_are_no_ops() {
        // The test process runs outside systemd, so NOTIFY_SOCKET is unset. A panic or an
        // error here would mean that the calls cannot happen in a plain process.
        ready();
        hand_over_main_pid(1);
    }
}
