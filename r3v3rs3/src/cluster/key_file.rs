//! The files of the cluster encryption keys. A file holds one key in base64. Empty lines and lines
//! that start with `#` are ignored.

use super::crypto::{ClusterKey, ClusterKeys, KEY_LEN, KeyId};
use anyhow::{Context as _, anyhow};
use base64::prelude::{BASE64_STANDARD, Engine as _};
use ring::rand::{SecureRandom, SystemRandom};
use std::path::{Path, PathBuf};
use tokio::io::AsyncWriteExt;

pub fn parse_key_file(text: &str) -> anyhow::Result<ClusterKey> {
    let mut lines = text
        .lines()
        .map(str::trim)
        .filter(|line| !line.is_empty() && !line.starts_with('#'));
    let line = lines.next().context("the file has no key")?;
    if lines.next().is_some() {
        anyhow::bail!("the file has more than one key");
    }
    let bytes = BASE64_STANDARD
        .decode(line)
        .context("the key is not base64")?;
    ClusterKey::new(&bytes)
}

/// Reads the key files in their order. The first key seals.
pub async fn load_keys(paths: &[PathBuf]) -> anyhow::Result<ClusterKeys> {
    let mut keys = Vec::with_capacity(paths.len());
    for path in paths {
        let text = tokio::fs::read_to_string(path)
            .await
            .with_context(|| format!("failed to read the key file {}", path.display()))?;
        let key = parse_key_file(&text)
            .with_context(|| format!("invalid key file {}", path.display()))?;
        keys.push(key);
    }
    ClusterKeys::new(keys)
}

/// The text of a key file with a new random key.
pub fn new_key_file() -> anyhow::Result<String> {
    let mut key = [0; KEY_LEN];
    SystemRandom::new()
        .fill(&mut key)
        .map_err(|_| anyhow!("no random bytes for the key"))?;
    Ok(format!(
        "# r3v3rs3 cluster encryption key\n{}\n",
        BASE64_STANDARD.encode(key)
    ))
}

/// Writes a new key to a file that only the owner can read, and returns the key id. An existing
/// file is an error.
pub async fn write_new_key_file(path: &Path) -> anyhow::Result<KeyId> {
    let text = new_key_file()?;
    let id = parse_key_file(&text)?.id();
    let mut options = tokio::fs::OpenOptions::new();
    options.write(true).create_new(true);
    options.mode(0o600);
    let mut file = options
        .open(path)
        .await
        .with_context(|| format!("failed to create the key file {}", path.display()))?;
    file.write_all(text.as_bytes()).await?;
    file.flush().await?;
    Ok(id)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::os::unix::fs::PermissionsExt;

    #[test]
    fn a_key_file_has_one_base64_key_of_32_bytes() {
        let text = new_key_file().unwrap();
        let key = parse_key_file(&text).unwrap();
        assert_ne!(
            key.id(),
            parse_key_file(&new_key_file().unwrap()).unwrap().id()
        );

        let key_line = BASE64_STANDARD.encode([7; KEY_LEN]);
        let commented = format!("# production\n\n  {key_line}  \n");
        assert!(parse_key_file(&commented).is_ok());
        assert!(parse_key_file("# only a comment\n").is_err());
        assert!(parse_key_file(&format!("{key_line}\n{key_line}\n")).is_err());
        assert!(parse_key_file(&BASE64_STANDARD.encode([7; 16])).is_err());
        assert!(parse_key_file("not base64!").is_err());
    }

    #[tokio::test]
    async fn keygen_creates_an_owner_only_file_and_does_not_overwrite() -> anyhow::Result<()> {
        let path = std::env::temp_dir().join(format!(
            "r3v3rs3-cluster-key-{}",
            hex::encode(rand::random::<[u8; 8]>())
        ));
        let id = write_new_key_file(&path).await?;
        let keys = load_keys(std::slice::from_ref(&path)).await?;
        assert_eq!(keys.primary_id(), id);
        let mode = std::fs::metadata(&path)?.permissions().mode() & 0o777;
        assert_eq!(mode, 0o600);

        let again = write_new_key_file(&path).await;
        assert_eq!(
            load_keys(std::slice::from_ref(&path)).await?.primary_id(),
            id
        );
        std::fs::remove_file(&path)?;
        assert!(again.is_err());
        Ok(())
    }
}
