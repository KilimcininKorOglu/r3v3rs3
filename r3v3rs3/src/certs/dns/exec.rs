use super::{DnsClient, TxtName, TxtRecord};
use anyhow::{anyhow, bail, Context};
use async_trait::async_trait;
use r3v3rs3_api::{
    acme::{DnsProvider, LocalProvider},
    app::AcmeExecConfig,
    error::Error,
};
use std::{
    path::{Path, PathBuf},
    process::Stdio,
    time::Duration,
};
use tokio::process::Command;
use tracing::warn;

/// Longest part of the standard error that an error message holds, in characters.
const STDERR_LIMIT: usize = 512;

/// The canonical path of `program` when it is an absolute path to a program of `allowlist`.
/// Symbolic links are resolved on both sides, so a link cannot point outside the list.
pub fn allowed_program(program: &str, allowlist: &[PathBuf]) -> Option<PathBuf> {
    let path = Path::new(program.trim());
    if !path.is_absolute() {
        return None;
    }
    let canonical = std::fs::canonicalize(path).ok()?;
    allowlist
        .iter()
        .filter(|allowed| allowed.is_absolute())
        .filter_map(|allowed| std::fs::canonicalize(allowed).ok())
        .any(|allowed| allowed == canonical)
        .then_some(canonical)
}

/// Rejects an exec provider whose program is not in the allowlist.
pub fn check_exec_provider(
    provider: Option<&DnsProvider>,
    config: &AcmeExecConfig,
) -> Result<(), Error> {
    match provider {
        Some(DnsProvider::Local(LocalProvider::Exec { program }))
            if allowed_program(program, &config.programs).is_none() =>
        {
            Err(Error::AcmeExecProgramNotAllowed {
                program: program.clone(),
            })
        }
        _ => Ok(()),
    }
}

/// Runs `program add|remove <fqdn> <value>` for each TXT value, without a shell.
pub struct Exec {
    program: PathBuf,
    timeout: Duration,
}

impl Exec {
    pub fn new(program: &str, config: &AcmeExecConfig) -> anyhow::Result<Self> {
        let program = allowed_program(program, &config.programs).ok_or_else(|| {
            anyhow!("the DNS exec program {program} is not in the acme_exec programs")
        })?;
        Ok(Self {
            program,
            timeout: config.timeout,
        })
    }

    async fn run(&self, action: &str, fqdn: &str, value: &str) -> anyhow::Result<()> {
        let program = self.program.display();
        let child = Command::new(&self.program)
            .args([action, fqdn, value])
            .env_clear()
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::piped())
            .kill_on_drop(true)
            .spawn()
            .with_context(|| format!("failed to start {program}"))?;
        // Dropping the child at the timeout kills the program.
        let output = tokio::time::timeout(self.timeout, child.wait_with_output())
            .await
            .map_err(|_| anyhow!("{program} did not {action} {fqdn} in {:?}", self.timeout))??;
        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr);
            let stderr = stderr.trim().chars().take(STDERR_LIMIT).collect::<String>();
            bail!(
                "{program} failed to {action} {fqdn} with {}: {stderr}",
                output.status
            );
        }
        Ok(())
    }

    /// Runs `remove` for every value, and returns the first error.
    async fn remove_values(&self, fqdn: &str, values: &[String]) -> anyhow::Result<()> {
        let mut first_error = None;
        for value in values {
            if let Err(err) = self.run("remove", fqdn, value).await {
                warn!(fqdn, %err, "failed to remove TXT record");
                first_error.get_or_insert(err);
            }
        }
        first_error.map_or(Ok(()), Err)
    }
}

#[async_trait]
impl DnsClient for Exec {
    async fn add_txt(&self, name: &TxtName) -> anyhow::Result<TxtRecord> {
        for (index, value) in name.values.iter().enumerate() {
            if let Err(err) = self.run("add", &name.fqdn, value).await {
                // The program can have added the failed value before it failed.
                let _ = self.remove_values(&name.fqdn, &name.values[..=index]).await;
                return Err(err);
            }
        }
        Ok(TxtRecord::new(name, String::new()))
    }

    async fn remove_txt(&self, record: &TxtRecord) -> anyhow::Result<()> {
        self.remove_values(&record.fqdn, &record.values).await
    }
}
