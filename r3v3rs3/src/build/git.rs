//! Clones one branch or one commit of a repository with the `git` binary. Every operand is a
//! validated type, and the process reads no configuration of the host, so a repository cannot run
//! hooks or reach another protocol.

use super::Revision;
use anyhow::{Context as _, anyhow, bail};
use base64::Engine as _;
use base64::engine::general_purpose::STANDARD;
use r3v3rs3_api::git::{CommitSha, GitRef, RepoUrl};
use std::path::Path;
use std::process::Stdio;
use std::time::Duration;
use tokio::process::Command;

/// The longest time of a clone.
const CLONE_TIMEOUT: Duration = Duration::from_secs(600);

/// The user name that GitHub, GitLab and Gitea accept together with an access token.
const TOKEN_USER: &str = "x-access-token";

/// The most characters of the `git` error output that a failure message carries.
const MAX_ERROR_OUTPUT: usize = 2000;

/// Checks out the revision into `dest`, which must not exist, and returns the commit SHA.
pub async fn clone(
    repository: &RepoUrl,
    revision: Revision<'_>,
    token: Option<&str>,
    dest: &Path,
) -> anyhow::Result<String> {
    match revision {
        Revision::Branch(branch) => clone_branch(repository, branch, token, dest).await?,
        Revision::Commit(commit) => fetch_commit(repository, commit, token, dest).await?,
    }
    let sha = head(dest).await?;
    if let Revision::Commit(commit) = revision
        && sha != commit.as_str()
    {
        bail!("git checked out {sha} instead of {commit}");
    }
    Ok(sha)
}

async fn clone_branch(
    repository: &RepoUrl,
    branch: &GitRef,
    token: Option<&str>,
    dest: &Path,
) -> anyhow::Result<()> {
    let mut clone = git(token, dest.parent().context("the checkout has no parent")?);
    clone
        .args(["clone", "--depth", "1", "--single-branch", "--no-tags"])
        .args(["--branch", branch.as_str(), "--", repository.as_str()])
        .arg(dest);
    run(clone, CLONE_TIMEOUT).await?;
    Ok(())
}

/// Fetches only the commit. The server must allow a fetch by commit, as GitHub, GitLab and Gitea
/// do for a commit of a branch.
async fn fetch_commit(
    repository: &RepoUrl,
    commit: &CommitSha,
    token: Option<&str>,
    dest: &Path,
) -> anyhow::Result<()> {
    tokio::fs::create_dir_all(dest).await?;
    let mut init = git(None, dest);
    init.args(["init", "--quiet"]);
    run(init, CLONE_TIMEOUT).await?;
    let mut fetch = git(token, dest);
    fetch
        .args(["fetch", "--depth", "1", "--no-tags", "--"])
        .args([repository.as_str(), commit.as_str()]);
    run(fetch, CLONE_TIMEOUT).await?;
    let mut checkout = git(None, dest);
    checkout.args(["checkout", "--quiet", "--detach", "FETCH_HEAD"]);
    run(checkout, CLONE_TIMEOUT).await?;
    Ok(())
}

/// The commit SHA of the checkout.
async fn head(dest: &Path) -> anyhow::Result<String> {
    let mut rev_parse = git(None, dest);
    rev_parse.args(["rev-parse", "HEAD"]);
    let sha = run(rev_parse, CLONE_TIMEOUT).await?;
    let sha = sha.trim();
    if sha.len() < 40 || !sha.chars().all(|c| c.is_ascii_hexdigit()) {
        bail!("git returned an invalid commit: {sha}");
    }
    Ok(sha.to_string())
}

/// A `git` command that reads no global or system configuration, asks no questions, runs no
/// hooks and speaks only HTTPS. The token travels in an environment variable, so it does not
/// appear in the process list.
fn git(token: Option<&str>, dir: &Path) -> Command {
    let mut command = Command::new("git");
    command
        .env_clear()
        .env("PATH", std::env::var_os("PATH").unwrap_or_default())
        .env("HOME", dir)
        .env("GIT_CONFIG_NOSYSTEM", "1")
        .env("GIT_CONFIG_GLOBAL", "/dev/null")
        .env("GIT_TERMINAL_PROMPT", "0")
        .args([
            "-c",
            "protocol.allow=never",
            "-c",
            "protocol.https.allow=always",
        ])
        .args(["-c", "core.hooksPath=/dev/null", "-c", "credential.helper="])
        .current_dir(dir)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .kill_on_drop(true);
    if let Some(token) = token {
        let basic = STANDARD.encode(format!("{TOKEN_USER}:{token}"));
        command
            .env("GIT_CONFIG_COUNT", "1")
            .env("GIT_CONFIG_KEY_0", "http.extraHeader")
            .env(
                "GIT_CONFIG_VALUE_0",
                format!("Authorization: Basic {basic}"),
            );
    }
    command
}

/// Runs the command and returns its standard output. A failure carries the error output.
async fn run(mut command: Command, timeout: Duration) -> anyhow::Result<String> {
    let child = command.spawn().map_err(|err| match err.kind() {
        std::io::ErrorKind::NotFound => {
            anyhow!("the git binary is missing; run the -platform image of r3v3rs3")
        }
        _ => anyhow!("failed to run git: {err}"),
    })?;
    let output = tokio::time::timeout(timeout, child.wait_with_output())
        .await
        .map_err(|_| anyhow!("git did not finish in {} seconds", timeout.as_secs()))??;
    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        let stderr = stderr.trim();
        let start = stderr.len().saturating_sub(MAX_ERROR_OUTPUT);
        let tail = stderr.get(start..).unwrap_or(stderr);
        bail!("git failed with {}: {tail}", output.status);
    }
    Ok(String::from_utf8_lossy(&output.stdout).into_owned())
}

#[cfg(test)]
mod tests {
    use super::*;

    struct TempDir(std::path::PathBuf);

    impl Drop for TempDir {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    fn temp_dir() -> TempDir {
        let dir = std::env::temp_dir().join(format!(
            "r3v3rs3-git-{}",
            hex::encode(rand::random::<[u8; 8]>())
        ));
        std::fs::create_dir_all(&dir).unwrap();
        TempDir(dir)
    }

    #[tokio::test]
    async fn the_token_reaches_git_only_through_the_environment() -> anyhow::Result<()> {
        let dir = temp_dir();
        let command = git(Some("s3cr3t"), &dir.0);
        let std = command.as_std();
        let args = std
            .get_args()
            .map(|arg| arg.to_string_lossy().into_owned())
            .collect::<Vec<_>>();
        assert!(!args.iter().any(|arg| arg.contains("s3cr3t")));
        let header = std
            .get_envs()
            .find(|(key, _)| *key == "GIT_CONFIG_VALUE_0")
            .and_then(|(_, value)| value)
            .context("header")?;
        let expected = STANDARD.encode("x-access-token:s3cr3t");
        assert_eq!(
            header.to_string_lossy(),
            format!("Authorization: Basic {expected}")
        );
        Ok(())
    }

    #[tokio::test]
    async fn a_failed_clone_reports_the_git_error() -> anyhow::Result<()> {
        let dir = temp_dir();
        let repository: RepoUrl = "https://127.0.0.1:9/owner/app.git".parse()?;
        let branch = "main".parse()?;
        let commit = "0123456789abcdef0123456789abcdef01234567".parse()?;
        let revisions = [Revision::Branch(&branch), Revision::Commit(&commit)];
        for (index, revision) in revisions.into_iter().enumerate() {
            let dest = dir.0.join(index.to_string());
            let err = clone(&repository, revision, None, &dest).await.unwrap_err();
            let message = format!("{err:#}");
            assert!(message.contains("git failed"), "{message}");
        }
        Ok(())
    }
}
