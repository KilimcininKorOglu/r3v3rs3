//! Moves files of any size between a container and the server in bounded pieces. The download
//! and upload calls of the runtime carry one file in memory and refuse a large one, while a
//! binary log, a dump or a base backup of a database can reach gigabytes. Every function here
//! keeps at most one piece in memory and works on every runtime that runs a command and writes
//! a file, the local Docker Engine and an agent alike.

use crate::runtime::ContainerRuntime;
use anyhow::{Context as _, ensure};
use r3v3rs3_api::container::ContainerName;
use sha2::{Digest, Sha256};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use tokio::io::{AsyncReadExt, AsyncWriteExt};

/// The bytes of one read from a container.
pub(super) const PIECE: u64 = 32 << 20;

/// The bytes of one write into a container, below the upload limit of the runtime.
pub(super) const UPLOAD_PIECE: usize = 8 << 20;

/// The two calls a transfer makes into a container: run a shell script and write one file.
#[async_trait::async_trait]
trait Shell: Send + Sync {
    async fn run(&self, script: String) -> anyhow::Result<Vec<u8>>;
    async fn write(&self, path: &str, bytes: Vec<u8>) -> anyhow::Result<()>;
}

/// A container of a runtime.
struct InContainer<'a> {
    runtime: &'a Arc<dyn ContainerRuntime>,
    container: &'a ContainerName,
}

#[async_trait::async_trait]
impl Shell for InContainer<'_> {
    async fn run(&self, script: String) -> anyhow::Result<Vec<u8>> {
        let command = vec!["sh".to_string(), "-c".to_string(), script];
        self.runtime
            .exec_capture(self.container, command, Vec::new())
            .await
    }

    async fn write(&self, path: &str, bytes: Vec<u8>) -> anyhow::Result<()> {
        self.runtime.upload_file(self.container, path, bytes).await
    }
}

/// A path inside a container goes into a single-quoted shell word.
fn quoted(path: &str) -> anyhow::Result<String> {
    ensure!(!path.contains('\''), "the path {path} holds a quote");
    Ok(format!("'{path}'"))
}

/// The size of a file inside the container.
async fn size_of(shell: &dyn Shell, path: &str) -> anyhow::Result<u64> {
    let out = shell.run(format!("wc -c < {}", quoted(path)?)).await?;
    let text = String::from_utf8_lossy(&out);
    text.trim()
        .parse()
        .with_context(|| format!("unexpected size of {path}: {text}"))
}

/// Copies the file at `path` inside the container from `offset` to its end into `dest`, and
/// returns the number of bytes copied.
pub(super) async fn fetch_to_file(
    runtime: &Arc<dyn ContainerRuntime>,
    container: &ContainerName,
    (path, offset): (&str, u64),
    dest: &Path,
) -> anyhow::Result<u64> {
    let shell = InContainer { runtime, container };
    fetch_in_pieces(&shell, (path, offset), dest, PIECE).await
}

async fn fetch_in_pieces(
    shell: &dyn Shell,
    (path, offset): (&str, u64),
    dest: &Path,
    piece: u64,
) -> anyhow::Result<u64> {
    let size = size_of(shell, path).await?;
    ensure!(
        offset <= size,
        "the file {path} holds {size} bytes, not {offset}"
    );
    let mut out = tokio::fs::File::create(dest).await?;
    let mut at = offset;
    while at < size {
        let count = piece.min(size - at);
        let script = format!("tail -c +{} {} | head -c {count}", at + 1, quoted(path)?);
        let bytes = shell.run(script).await?;
        ensure!(
            bytes.len() as u64 == count,
            "the file {path} gave {} bytes at {at}, not {count}",
            bytes.len()
        );
        out.write_all(&bytes).await?;
        at += count;
    }
    out.sync_all().await?;
    Ok(size - offset)
}

/// Writes the local file `src` into the container at `remote`, replacing it in one rename. The
/// pieces gather in a file beside the target, so the target never holds a part of the new
/// file: a Redis that loaded `dump.rdb` left it empty at its shutdown when the file was
/// truncated and refilled in place (measured on redis:8).
pub(super) async fn push_file(
    runtime: &Arc<dyn ContainerRuntime>,
    container: &ContainerName,
    src: &Path,
    remote: &str,
) -> anyhow::Result<()> {
    let shell = InContainer { runtime, container };
    push_in_pieces(&shell, src, remote, UPLOAD_PIECE).await
}

async fn push_in_pieces(
    shell: &dyn Shell,
    src: &Path,
    remote: &str,
    piece: usize,
) -> anyhow::Result<()> {
    let target = quoted(remote)?;
    let gathered_path = format!("{remote}.new");
    let gathered = quoted(&gathered_path)?;
    shell.run(format!(": > {gathered}")).await?;
    append_pieces(shell, src, &gathered_path, piece).await?;
    shell.run(format!("mv -f {gathered} {target}")).await?;
    Ok(())
}

/// Appends the local file `src` to the container file `gathered`, one upload per piece.
async fn append_pieces(
    shell: &dyn Shell,
    src: &Path,
    gathered: &str,
    piece: usize,
) -> anyhow::Result<()> {
    let part_path = format!("{gathered}.part");
    let part = quoted(&part_path)?;
    let gathered = quoted(gathered)?;
    let mut input = tokio::fs::File::open(src).await?;
    let mut buffer = vec![0u8; piece];
    loop {
        let read = read_full(&mut input, &mut buffer).await?;
        if read == 0 {
            return Ok(());
        }
        shell.write(&part_path, buffer[..read].to_vec()).await?;
        shell
            .run(format!("cat {part} >> {gathered} && rm -f {part}"))
            .await?;
    }
}

/// Fills `buffer` from `input` and returns the bytes read, fewer only at the end of the file.
async fn read_full(input: &mut tokio::fs::File, buffer: &mut [u8]) -> anyhow::Result<usize> {
    let mut filled = 0;
    while filled < buffer.len() {
        let read = input.read(&mut buffer[filled..]).await?;
        if read == 0 {
            break;
        }
        filled += read;
    }
    Ok(filled)
}

/// Compresses `src` into `dest` with gzip and returns the size and the SHA-256 of the
/// compressed file. `dest` appears only complete: the data goes to a temporary file in the same
/// directory, which is synced and renamed over it.
pub(super) async fn gzip_file(src: &Path, dest: &Path) -> anyhow::Result<(u64, String)> {
    let (src, dest) = (src.to_path_buf(), dest.to_path_buf());
    tokio::task::spawn_blocking(move || gzip_file_blocking(&src, &dest)).await?
}

fn gzip_file_blocking(src: &Path, dest: &Path) -> anyhow::Result<(u64, String)> {
    let temp = temp_beside(dest)?;
    let written = (|| -> anyhow::Result<()> {
        let mut input = std::io::BufReader::new(std::fs::File::open(src)?);
        let file = std::fs::File::create(&temp)?;
        let mut encoder = flate2::write::GzEncoder::new(file, flate2::Compression::default());
        std::io::copy(&mut input, &mut encoder)?;
        encoder.finish()?.sync_all()?;
        Ok(())
    })();
    if let Err(err) = written {
        std::fs::remove_file(&temp).ok();
        return Err(err);
    }
    std::fs::rename(&temp, dest)?;
    sha256_of(dest)
}

/// The size and the SHA-256 of a local file, read in pieces.
fn sha256_of(path: &Path) -> anyhow::Result<(u64, String)> {
    use std::io::Read as _;
    let mut file = std::fs::File::open(path)?;
    let mut hasher = Sha256::new();
    let mut buffer = vec![0u8; 1 << 20];
    let mut size = 0u64;
    loop {
        let read = file.read(&mut buffer)?;
        if read == 0 {
            return Ok((size, hex::encode(hasher.finalize())));
        }
        hasher.update(&buffer[..read]);
        size += read as u64;
    }
}

/// Decompresses the gzip file `src` into `dest`.
pub(super) async fn gunzip_file(src: &Path, dest: &Path) -> anyhow::Result<()> {
    let (src, dest) = (src.to_path_buf(), dest.to_path_buf());
    tokio::task::spawn_blocking(move || -> anyhow::Result<()> {
        let input = std::io::BufReader::new(std::fs::File::open(&src)?);
        let mut decoder = flate2::read::GzDecoder::new(input);
        let mut out = std::io::BufWriter::new(std::fs::File::create(&dest)?);
        std::io::copy(&mut decoder, &mut out)?;
        out.into_inner()?.sync_all()?;
        Ok(())
    })
    .await?
}

/// The first `limit` bytes of a local file as text, for the header of a dump.
pub(super) async fn head_text(path: &Path, limit: u64) -> anyhow::Result<String> {
    let mut bytes = Vec::new();
    tokio::fs::File::open(path)
        .await?
        .take(limit)
        .read_to_end(&mut bytes)
        .await?;
    Ok(String::from_utf8_lossy(&bytes).into_owned())
}

fn temp_beside(dest: &Path) -> anyhow::Result<PathBuf> {
    let name = dest
        .file_name()
        .context("the target has no file name")?
        .to_string_lossy();
    Ok(dest.with_file_name(format!(".{name}.{}.tmp", crate::clock::unix_ms())))
}

/// Fails when the file system of `dir` has less than `need` bytes free for the server.
pub(super) fn ensure_free_space(dir: &Path, need: u64) -> anyhow::Result<()> {
    let space = super::prune::get_disk_space(dir)
        .with_context(|| format!("the free space of {} cannot be read", dir.display()))?;
    ensure!(
        space.available_bytes >= need,
        "{} has {} bytes free, and the transfer needs at least {need}",
        dir.display(),
        space.available_bytes
    );
    Ok(())
}

/// A directory for the files of one transfer job, removed with the value.
pub(super) struct Scratch {
    dir: PathBuf,
}

impl Scratch {
    pub(super) async fn new(parent: &Path) -> anyhow::Result<Self> {
        static NEXT: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
        let n = NEXT.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        let dir = parent.join(format!(
            ".tmp-{}-{}-{n}",
            crate::clock::unix_ms(),
            std::process::id()
        ));
        tokio::fs::create_dir_all(&dir).await?;
        Ok(Self { dir })
    }

    pub(super) fn path(&self, name: &str) -> PathBuf {
        self.dir.join(name)
    }
}

impl Drop for Scratch {
    fn drop(&mut self) {
        std::fs::remove_dir_all(&self.dir).ok();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;
    use std::sync::Mutex;

    /// The upload limit of the Docker runtime (`MAX_UPLOAD_SIZE`).
    const UPLOAD_LIMIT: usize = 16 * 1024 * 1024;

    /// A transfer that the disk cannot hold is refused before it starts, and one that it can
    /// hold passes; a directory whose space cannot be read is an error, not a pass.
    #[test]
    fn a_transfer_larger_than_the_free_space_is_refused() {
        let tmp = std::env::temp_dir();
        let err = ensure_free_space(&tmp, u64::MAX).unwrap_err();
        assert!(err.to_string().contains("bytes free"), "{err}");
        ensure_free_space(&tmp, 1).unwrap();
        assert!(ensure_free_space(Path::new("/no/such/r3v3rs3/dir"), 1).is_err());
    }

    /// A container whose files live in memory. It runs the few shell commands of a transfer,
    /// refuses an upload over the limit of the Docker runtime, and records the largest piece it
    /// handed out, so a test sees the memory bound.
    #[derive(Default)]
    struct Files {
        files: Mutex<HashMap<String, Vec<u8>>>,
        largest_read: Mutex<usize>,
        /// The number of uploads that succeed before every further one fails, when set.
        uploads_left: Mutex<Option<usize>>,
    }

    fn unquote(word: &str) -> String {
        word.trim_matches('\'').to_string()
    }

    impl Files {
        fn with(path: &str, bytes: Vec<u8>) -> Self {
            let files = Self::default();
            files.files.lock().unwrap().insert(path.into(), bytes);
            files
        }

        fn get(&self, path: &str) -> Option<Vec<u8>> {
            self.files.lock().unwrap().get(path).cloned()
        }

        fn read(&self, path: &str, from: &str, count: &str) -> anyhow::Result<Vec<u8>> {
            let file = self.get(&unquote(path)).context("no such file")?;
            let from = from.trim_start_matches('+').parse::<usize>()? - 1;
            let end = (from + count.parse::<usize>()?).min(file.len());
            let piece = file[from.min(end)..end].to_vec();
            let mut largest = self.largest_read.lock().unwrap();
            *largest = (*largest).max(piece.len());
            Ok(piece)
        }

        fn append(&self, part: &str, target: &str) -> anyhow::Result<Vec<u8>> {
            let mut files = self.files.lock().unwrap();
            let piece = files.remove(&unquote(part)).context("no part")?;
            files.entry(unquote(target)).or_default().extend(piece);
            Ok(Vec::new())
        }

        fn rename(&self, from: &str, to: &str) -> anyhow::Result<Vec<u8>> {
            let mut files = self.files.lock().unwrap();
            let file = files.remove(&unquote(from)).context("no such file")?;
            files.insert(unquote(to), file);
            Ok(Vec::new())
        }

        fn take_upload(&self) -> anyhow::Result<()> {
            let mut left = self.uploads_left.lock().unwrap();
            if let Some(n) = left.as_mut() {
                ensure!(*n > 0, "the connection broke");
                *n -= 1;
            }
            Ok(())
        }
    }

    #[async_trait::async_trait]
    impl Shell for Files {
        async fn run(&self, script: String) -> anyhow::Result<Vec<u8>> {
            let words: Vec<&str> = script.split_whitespace().collect();
            match words.as_slice() {
                ["wc", "-c", "<", path] => {
                    let file = self.get(&unquote(path)).context("no such file")?;
                    Ok(format!("{}\n", file.len()).into_bytes())
                }
                ["tail", "-c", from, path, "|", "head", "-c", count] => {
                    self.read(path, from, count)
                }
                [":", ">", path] => {
                    self.files.lock().unwrap().insert(unquote(path), Vec::new());
                    Ok(Vec::new())
                }
                ["cat", part, ">>", target, "&&", "rm", "-f", _] => self.append(part, target),
                ["mv", "-f", from, to] => self.rename(from, to),
                _ => anyhow::bail!("unexpected script {script}"),
            }
        }

        async fn write(&self, path: &str, bytes: Vec<u8>) -> anyhow::Result<()> {
            ensure!(bytes.len() <= UPLOAD_LIMIT, "the upload is too large");
            self.take_upload()?;
            self.files.lock().unwrap().insert(path.to_string(), bytes);
            Ok(())
        }
    }

    fn content(len: usize) -> Vec<u8> {
        (0..len).map(|i| (i * 7 % 251) as u8).collect()
    }

    async fn scratch() -> anyhow::Result<Scratch> {
        Scratch::new(&std::env::temp_dir()).await
    }

    /// A file of any length arrives byte for byte, and no read asks for more than one piece:
    /// at the piece size, one byte over it, below it, empty, and from an offset.
    #[tokio::test]
    async fn a_fetch_copies_every_byte_in_bounded_pieces() -> anyhow::Result<()> {
        let dir = scratch().await?;
        for (len, offset) in [(0, 0), (5, 0), (64, 0), (65, 0), (200, 0), (200, 130)] {
            let files = Files::with("/f", content(len));
            let dest = dir.path(&format!("out-{len}-{offset}"));
            let copied = fetch_in_pieces(&files, ("/f", offset as u64), &dest, 64).await?;
            assert_eq!(copied, (len - offset) as u64);
            assert_eq!(tokio::fs::read(&dest).await?, content(len)[offset..]);
            assert!(*files.largest_read.lock().unwrap() <= 64, "len {len}");
        }
        Ok(())
    }

    /// An offset past the end of the file is an error, not an empty copy.
    #[tokio::test]
    async fn a_fetch_past_the_end_fails() -> anyhow::Result<()> {
        let dir = scratch().await?;
        let files = Files::with("/f", content(10));
        let result = fetch_in_pieces(&files, ("/f", 11), &dir.path("out"), 64).await;
        assert!(result.is_err());
        Ok(())
    }

    /// A push replaces the remote file with the local one and leaves no part file behind.
    #[tokio::test]
    async fn a_push_writes_every_byte_and_replaces_the_target() -> anyhow::Result<()> {
        let dir = scratch().await?;
        for len in [0usize, 5, 64, 65, 300] {
            let files = Files::with("/t", b"old".to_vec());
            let src = dir.path(&format!("in-{len}"));
            tokio::fs::write(&src, content(len)).await?;
            push_in_pieces(&files, &src, "/t", 64).await?;
            assert_eq!(files.get("/t"), Some(content(len)), "len {len}");
            assert_eq!(files.get("/t.new"), None);
            assert_eq!(files.get("/t.new.part"), None);
        }
        Ok(())
    }

    /// A push that breaks after its first piece leaves the target as it was: a file the server
    /// reads must never hold a part of the new content (atomic replace).
    #[tokio::test]
    async fn a_broken_push_keeps_the_old_target() -> anyhow::Result<()> {
        let dir = scratch().await?;
        let files = Files::with("/t", b"old".to_vec());
        *files.uploads_left.lock().unwrap() = Some(1);
        let src = dir.path("in");
        tokio::fs::write(&src, content(200)).await?;
        let result = push_in_pieces(&files, &src, "/t", 64).await;
        assert!(result.is_err());
        assert_eq!(files.get("/t"), Some(b"old".to_vec()));
        Ok(())
    }

    /// A file over the upload limit of the runtime goes through with the default piece size,
    /// where a single upload is refused.
    #[tokio::test]
    async fn a_push_passes_a_file_over_the_upload_limit() -> anyhow::Result<()> {
        let dir = scratch().await?;
        let files = Files::default();
        let src = dir.path("big");
        let data = content(UPLOAD_LIMIT + 1);
        tokio::fs::write(&src, &data).await?;
        push_in_pieces(&files, &src, "/big", UPLOAD_PIECE).await?;
        assert_eq!(files.get("/big"), Some(data));
        Ok(())
    }

    /// A gzip round trip gives the input back, and the reported size and SHA-256 are those of
    /// the compressed file on disk, as the archive records them.
    #[tokio::test]
    async fn a_gzip_round_trip_keeps_the_bytes_and_reports_the_compressed_file()
    -> anyhow::Result<()> {
        let dir = scratch().await?;
        let (src, gz, back) = (dir.path("src"), dir.path("src.gz"), dir.path("back"));
        let data = content(300_000);
        tokio::fs::write(&src, &data).await?;
        let (size, sha) = gzip_file(&src, &gz).await?;
        let compressed = tokio::fs::read(&gz).await?;
        assert_eq!(size, compressed.len() as u64);
        assert_eq!(sha, hex::encode(Sha256::digest(&compressed)));
        gunzip_file(&gz, &back).await?;
        assert_eq!(tokio::fs::read(&back).await?, data);
        Ok(())
    }
}
