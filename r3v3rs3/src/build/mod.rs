//! The build step of the deployment platform: a checkout of the source, and the build context
//! that the Docker Engine builds an image from.

pub mod git;

use anyhow::{Context as _, bail};
use r3v3rs3_api::git::{CommitSha, GitRef, RelPath, RepoUrl};
use std::path::Path;

/// The largest build context. The context is held in memory while it is sent to Docker.
pub const MAX_CONTEXT_BYTES: u64 = 512 << 20;

/// The version of a repository that a checkout holds.
#[derive(Debug, Clone, Copy)]
pub enum Revision<'a> {
    /// The newest commit of a branch or a tag.
    Branch(&'a GitRef),
    /// One commit, which a rollback repeats.
    Commit(&'a CommitSha),
}

impl Revision<'_> {
    /// The branch name or the commit SHA.
    pub fn as_str(&self) -> &str {
        match self {
            Self::Branch(branch) => branch.as_str(),
            Self::Commit(commit) => commit.as_str(),
        }
    }
}

/// Fetches the source of an app into a directory.
#[async_trait::async_trait]
pub trait SourceFetcher: Send + Sync {
    /// Checks out the revision into `dest`, which must not exist, and returns the commit SHA.
    async fn fetch(
        &self,
        repository: &RepoUrl,
        revision: Revision<'_>,
        token: Option<&str>,
        dest: &Path,
    ) -> anyhow::Result<String>;
}

/// The fetcher that runs the `git` binary.
pub struct GitFetcher;

#[async_trait::async_trait]
impl SourceFetcher for GitFetcher {
    async fn fetch(
        &self,
        repository: &RepoUrl,
        revision: Revision<'_>,
        token: Option<&str>,
        dest: &Path,
    ) -> anyhow::Result<String> {
        git::clone(repository, revision, token, dest).await
    }
}

/// Packs the directory `context` of the checkout into a tar archive without the `.git`
/// directory. Symbolic links stay links, so the archive holds no file from outside the checkout.
pub fn context_archive(checkout: &Path, context: &RelPath) -> anyhow::Result<Vec<u8>> {
    let root = checkout.canonicalize()?;
    let dir = context_dir(&root, context)?;
    let git_dir = root.join(".git");
    if git_dir.exists() {
        std::fs::remove_dir_all(&git_dir)?;
    }
    let size = tree_size(&dir, MAX_CONTEXT_BYTES)?;
    if size > MAX_CONTEXT_BYTES {
        bail!(
            "the build context is larger than {} MiB",
            MAX_CONTEXT_BYTES >> 20
        );
    }
    let mut builder = tar::Builder::new(Vec::new());
    builder.follow_symlinks(false);
    builder.append_dir_all(".", &dir)?;
    Ok(builder.into_inner()?)
}

/// The real directory of `context`, which must stay inside `root` after every link is resolved.
fn context_dir(root: &Path, context: &RelPath) -> anyhow::Result<std::path::PathBuf> {
    let dir = root
        .join(context.as_str())
        .canonicalize()
        .with_context(|| format!("the context {context} does not exist in the repository"))?;
    if !dir.starts_with(root) || !dir.is_dir() {
        bail!("the context {context} is not a directory of the repository");
    }
    Ok(dir)
}

/// The size of the files under `dir`, counted until it passes `limit`.
fn tree_size(dir: &Path, limit: u64) -> anyhow::Result<u64> {
    let mut total = 0;
    let mut pending = vec![dir.to_path_buf()];
    while let Some(dir) = pending.pop() {
        for entry in std::fs::read_dir(&dir)? {
            let entry = entry?;
            let metadata = entry.path().symlink_metadata()?;
            if metadata.is_dir() {
                pending.push(entry.path());
            } else {
                total += metadata.len();
            }
            if total > limit {
                return Ok(total);
            }
        }
    }
    Ok(total)
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

    fn checkout() -> anyhow::Result<TempDir> {
        let dir = std::env::temp_dir().join(format!(
            "r3v3rs3-context-{}",
            hex::encode(rand::random::<[u8; 8]>())
        ));
        std::fs::create_dir_all(dir.join(".git"))?;
        std::fs::create_dir_all(dir.join("app/src"))?;
        std::fs::write(dir.join(".git/config"), "secret")?;
        std::fs::write(dir.join("app/Dockerfile"), "FROM scratch\n")?;
        std::fs::write(dir.join("app/src/main.txt"), "hello")?;
        std::os::unix::fs::symlink("/etc/hostname", dir.join("app/host"))?;
        Ok(TempDir(dir))
    }

    fn entries(archive: &[u8]) -> anyhow::Result<Vec<String>> {
        let mut names = Vec::new();
        for entry in tar::Archive::new(archive).entries()? {
            let entry = entry?;
            let kind = entry.header().entry_type();
            let path = entry.path()?.to_string_lossy().into_owned();
            names.push(format!(
                "{path}:{}",
                if kind.is_symlink() { "link" } else { "file" }
            ));
        }
        names.sort();
        Ok(names)
    }

    #[test]
    fn the_context_holds_its_directory_without_git_and_keeps_links() -> anyhow::Result<()> {
        let dir = checkout()?;
        let archive = context_archive(&dir.0, &"app".parse()?)?;
        let names = entries(&archive)?;
        assert!(names.contains(&"Dockerfile:file".to_string()), "{names:?}");
        assert!(
            names.contains(&"src/main.txt:file".to_string()),
            "{names:?}"
        );
        assert!(names.contains(&"host:link".to_string()), "{names:?}");
        assert!(!dir.0.join(".git").exists());
        let root = context_archive(&dir.0, &".".parse()?)?;
        assert!(!entries(&root)?.iter().any(|name| name.contains(".git")));
        Ok(())
    }

    #[test]
    fn a_context_must_be_a_directory_inside_the_checkout() -> anyhow::Result<()> {
        let dir = checkout()?;
        std::os::unix::fs::symlink("/etc", dir.0.join("outside"))?;
        for context in ["outside", "missing", "app/Dockerfile"] {
            let result = context_archive(&dir.0, &context.parse()?);
            assert!(result.is_err(), "{context}");
        }
        Ok(())
    }
}
