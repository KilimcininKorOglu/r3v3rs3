//! Runs a helper binary of the build step, `git` or `docker`, and collects its output.

use anyhow::{anyhow, bail};
use std::process::Stdio;
use std::time::Duration;
use tokio::process::Command;

/// The most characters of the error output that a failure message carries.
const MAX_ERROR_OUTPUT: usize = 2000;

/// Runs the command and returns its standard output. A failure carries the end of the error
/// output, where the tools print the cause.
pub async fn run(mut command: Command, timeout: Duration) -> anyhow::Result<String> {
    let program = command
        .as_std()
        .get_program()
        .to_string_lossy()
        .into_owned();
    command
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .kill_on_drop(true);
    let child = command.spawn().map_err(|err| match err.kind() {
        std::io::ErrorKind::NotFound => {
            anyhow!("the {program} binary is missing; run the -platform image of r3v3rs3")
        }
        _ => anyhow!("failed to run {program}: {err}"),
    })?;
    let output = tokio::time::timeout(timeout, child.wait_with_output())
        .await
        .map_err(|_| anyhow!("{program} did not finish in {} seconds", timeout.as_secs()))??;
    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        let stderr = stderr.trim();
        let start = stderr.len().saturating_sub(MAX_ERROR_OUTPUT);
        let tail = stderr.get(start..).unwrap_or(stderr);
        bail!("{program} failed with {}: {tail}", output.status);
    }
    Ok(String::from_utf8_lossy(&output.stdout).into_owned())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn a_failure_carries_the_error_output() {
        let mut command = Command::new("sh");
        command.args(["-c", "echo out; echo broken >&2; exit 3"]);
        let err = run(command, Duration::from_secs(10)).await.unwrap_err();
        let message = format!("{err:#}");
        assert!(message.starts_with("sh failed with"), "{message}");
        assert!(message.ends_with(": broken"), "{message}");
    }

    #[tokio::test]
    async fn a_missing_binary_names_the_platform_image() {
        let command = Command::new("r3v3rs3-missing-binary");
        let err = run(command, Duration::from_secs(10)).await.unwrap_err();
        assert!(err.to_string().contains("-platform image"), "{err}");
    }
}
