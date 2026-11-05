//! The build step of the deployment platform: a checkout of the source, and the build context
//! that the Docker Engine builds an image from.

pub mod buildkit;
pub mod compose;
pub mod git;
mod process;

use anyhow::{Context as _, bail};
use ignore::gitignore::{Gitignore, GitignoreBuilder};
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

/// The user name that GitHub, GitLab and Gitea accept together with an access token of an app.
const TOKEN_USER: &str = "x-access-token";

/// The user name and the password that clone a private repository over HTTPS.
#[derive(Clone, PartialEq, Eq)]
pub struct GitCredential {
    pub user: String,
    pub password: String,
}

impl GitCredential {
    /// The credential of the Git token of an app.
    pub fn token(token: String) -> Self {
        Self {
            user: TOKEN_USER.to_string(),
            password: token,
        }
    }
}

/// The credential stays out of debug output and logs.
impl std::fmt::Debug for GitCredential {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("GitCredential").finish_non_exhaustive()
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
        credential: Option<&GitCredential>,
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
        credential: Option<&GitCredential>,
        dest: &Path,
    ) -> anyhow::Result<String> {
        git::clone(repository, revision, credential, dest).await
    }
}

/// Packs the directory `context` of the checkout into a tar archive without the `.git`
/// directory and excluding patterns matching `.dockerignore`. Symbolic links stay links,
/// so the archive holds no file from outside the checkout.
pub fn context_archive(checkout: &Path, context: &RelPath) -> anyhow::Result<Vec<u8>> {
    let root = checkout.canonicalize()?;
    let dir = context_dir(&root, context)?;
    let git_dir = root.join(".git");
    if git_dir.exists() {
        std::fs::remove_dir_all(&git_dir)?;
    }
    let ignore = load_dockerignore(&dir, Some(&root));
    directory_archive_filtered(&dir, ignore.as_ref())
}

/// Packs a directory into a tar archive of at most [`MAX_CONTEXT_BYTES`] of files,
/// respecting any `.dockerignore` file in `dir`. Symbolic links stay links.
pub fn directory_archive(dir: &Path) -> anyhow::Result<Vec<u8>> {
    let ignore = load_dockerignore(dir, None);
    directory_archive_filtered(dir, ignore.as_ref())
}

/// Loads `.dockerignore` patterns from the directory or optionally from the checkout root.
pub fn load_dockerignore(dir: &Path, root: Option<&Path>) -> Option<Gitignore> {
    let dir_ignore = dir.join(".dockerignore");
    if dir_ignore.is_file() {
        let mut builder = GitignoreBuilder::new(dir);
        let _ = builder.add(dir_ignore);
        return builder.build().ok();
    }
    if let Some(root) = root {
        let root_ignore = root.join(".dockerignore");
        if root_ignore.is_file() {
            let mut builder = GitignoreBuilder::new(dir);
            let _ = builder.add(root_ignore);
            return builder.build().ok();
        }
    }
    None
}

fn directory_archive_filtered(dir: &Path, ignore: Option<&Gitignore>) -> anyhow::Result<Vec<u8>> {
    let size = tree_size(dir, MAX_CONTEXT_BYTES, ignore)?;
    if size > MAX_CONTEXT_BYTES {
        bail!(
            "the directory is larger than {} MiB",
            MAX_CONTEXT_BYTES >> 20
        );
    }
    let mut builder = tar::Builder::new(Vec::new());
    builder.follow_symlinks(false);
    append_filtered_dir_all(&mut builder, dir, dir, ignore)?;
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

/// Removes a directory. A missing directory is not an error.
pub async fn remove_dir(dir: &Path) -> std::io::Result<()> {
    match tokio::fs::remove_dir_all(dir).await {
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => Ok(()),
        other => other,
    }
}

/// Removes every entry of `dir` except `keep`.
pub async fn remove_other_dirs(dir: &Path, keep: &Path) -> anyhow::Result<()> {
    let mut entries = tokio::fs::read_dir(dir).await?;
    while let Some(entry) = entries.next_entry().await? {
        if entry.path() != keep {
            remove_dir(&entry.path()).await?;
        }
    }
    Ok(())
}

fn is_ignored(ignore: Option<&Gitignore>, base_dir: &Path, path: &Path, is_dir: bool) -> bool {
    let Some(ignore) = ignore else { return false };
    let Ok(rel) = path.strip_prefix(base_dir) else {
        return false;
    };
    ignore.matched_path_or_any_parents(rel, is_dir).is_ignore()
}

fn append_entry(
    builder: &mut tar::Builder<Vec<u8>>,
    base_dir: &Path,
    path: &Path,
    is_dir: bool,
    ignore: Option<&Gitignore>,
) -> anyhow::Result<()> {
    if is_ignored(ignore, base_dir, path, is_dir) {
        return Ok(());
    }
    let rel_path = path.strip_prefix(base_dir)?;
    if is_dir {
        builder.append_dir(rel_path, path)?;
        append_filtered_dir_all(builder, base_dir, path, ignore)?;
    } else {
        builder.append_path_with_name(path, rel_path)?;
    }
    Ok(())
}

fn append_filtered_dir_all(
    builder: &mut tar::Builder<Vec<u8>>,
    base_dir: &Path,
    current_dir: &Path,
    ignore: Option<&Gitignore>,
) -> anyhow::Result<()> {
    for entry in std::fs::read_dir(current_dir)? {
        let entry = entry?;
        let path = entry.path();
        let is_dir = path.symlink_metadata()?.is_dir();
        append_entry(builder, base_dir, &path, is_dir, ignore)?;
    }
    Ok(())
}

/// The size of the files under `dir`, counted until it passes `limit`.
fn tree_size(dir: &Path, limit: u64, ignore: Option<&Gitignore>) -> anyhow::Result<u64> {
    let mut total = 0;
    let mut pending = vec![dir.to_path_buf()];
    while let Some(current_dir) = pending.pop() {
        for entry in std::fs::read_dir(&current_dir)? {
            let entry = entry?;
            let path = entry.path();
            let metadata = path.symlink_metadata()?;
            let is_dir = metadata.is_dir();
            if is_ignored(ignore, dir, &path, is_dir) {
                continue;
            }
            if is_dir {
                pending.push(path);
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

    #[test]
    fn context_archive_respects_dockerignore() -> anyhow::Result<()> {
        let dir = checkout()?;
        std::fs::create_dir_all(dir.0.join("app/ignore_dir"))?;
        std::fs::write(dir.0.join("app/ignore_dir/file.txt"), "should be ignored")?;
        std::fs::write(dir.0.join("app/src/app.secret"), "supersecret")?;
        std::fs::write(dir.0.join("app/.env"), "KEY=VALUE")?;
        std::fs::write(
            dir.0.join("app/.dockerignore"),
            "*.secret\nignore_dir/\n.env\n",
        )?;

        let archive = context_archive(&dir.0, &"app".parse()?)?;
        let names = entries(&archive)?;
        assert!(names.contains(&"Dockerfile:file".to_string()), "{names:?}");
        assert!(
            names.contains(&"src/main.txt:file".to_string()),
            "{names:?}"
        );
        assert!(!names.iter().any(|n| n.contains("secret")), "{names:?}");
        assert!(!names.iter().any(|n| n.contains("ignore_dir")), "{names:?}");
        assert!(!names.iter().any(|n| n.contains(".env")), "{names:?}");
        Ok(())
    }
}
