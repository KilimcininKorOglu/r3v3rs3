//! Encryption of the values that the cluster stores. A sealed value is the magic bytes, the key id,
//! the nonce, the AES-256-GCM ciphertext and its tag. The KV key of the value is the associated
//! data, so a sealed value does not open under another KV key.

use anyhow::{Context as _, anyhow};
use ring::aead::{AES_256_GCM, Aad, LessSafeKey, NONCE_LEN, Nonce, UnboundKey};
use ring::rand::{SecureRandom, SystemRandom};
use sha2::{Digest, Sha256};
use std::collections::HashSet;
use std::fmt;

pub const KEY_LEN: usize = 32;
pub const KEY_ID_LEN: usize = 8;
const MAGIC: &[u8; 4] = b"R3E1";

pub type KeyId = [u8; KEY_ID_LEN];

pub struct ClusterKey {
    id: KeyId,
    key: LessSafeKey,
}

impl ClusterKey {
    /// The id of the key is the start of its SHA-256 digest.
    pub fn new(bytes: &[u8]) -> anyhow::Result<Self> {
        if bytes.len() != KEY_LEN {
            anyhow::bail!("an encryption key has {KEY_LEN} bytes, not {}", bytes.len());
        }
        let digest = Sha256::digest(bytes);
        let mut id = [0; KEY_ID_LEN];
        id.copy_from_slice(&digest[..KEY_ID_LEN]);
        let key =
            UnboundKey::new(&AES_256_GCM, bytes).map_err(|_| anyhow!("invalid encryption key"))?;
        Ok(Self {
            id,
            key: LessSafeKey::new(key),
        })
    }

    pub fn id(&self) -> KeyId {
        self.id
    }
}

impl fmt::Debug for ClusterKey {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("ClusterKey")
            .field("id", &hex::encode(self.id))
            .finish()
    }
}

/// The encryption keys of the cluster. The first key seals. Every key opens.
#[derive(Debug)]
pub struct ClusterKeys {
    primary: ClusterKey,
    older: Vec<ClusterKey>,
    random: SystemRandom,
}

impl ClusterKeys {
    pub fn new(keys: Vec<ClusterKey>) -> anyhow::Result<Self> {
        let mut ids = HashSet::new();
        if let Some(key) = keys.iter().find(|key| !ids.insert(key.id)) {
            anyhow::bail!("the encryption key {} is listed twice", hex::encode(key.id));
        }
        let mut keys = keys.into_iter();
        let primary = keys
            .next()
            .context("the cluster needs at least one encryption key")?;
        Ok(Self {
            primary,
            older: keys.collect(),
            random: SystemRandom::new(),
        })
    }

    pub fn primary_id(&self) -> KeyId {
        self.primary.id
    }

    pub fn seal(&self, kv_key: &str, plaintext: &[u8]) -> anyhow::Result<Vec<u8>> {
        let mut nonce = [0; NONCE_LEN];
        self.random
            .fill(&mut nonce)
            .map_err(|_| anyhow!("no random bytes for the nonce"))?;
        let mut body = plaintext.to_vec();
        self.primary
            .key
            .seal_in_place_append_tag(
                Nonce::assume_unique_for_key(nonce),
                Aad::from(kv_key.as_bytes()),
                &mut body,
            )
            .map_err(|_| anyhow!("failed to encrypt the value of {kv_key}"))?;
        Ok([MAGIC.as_slice(), &self.primary.id, &nonce, &body].concat())
    }

    pub fn open(&self, kv_key: &str, sealed: &[u8]) -> anyhow::Result<Vec<u8>> {
        let (id, nonce, body) =
            split(sealed).with_context(|| format!("the value of {kv_key} is not encrypted"))?;
        let key = self.find(id).with_context(|| {
            format!(
                "the value of {kv_key} needs the unknown encryption key {}",
                hex::encode(id)
            )
        })?;
        let mut body = body.to_vec();
        let len = key
            .key
            .open_in_place(
                Nonce::assume_unique_for_key(nonce),
                Aad::from(kv_key.as_bytes()),
                &mut body,
            )
            .map_err(|_| anyhow!("the value of {kv_key} failed the authentication"))?
            .len();
        body.truncate(len);
        Ok(body)
    }

    fn find(&self, id: KeyId) -> Option<&ClusterKey> {
        std::iter::once(&self.primary)
            .chain(&self.older)
            .find(|key| key.id == id)
    }
}

/// The key id of a sealed value. `None` for a value that is not sealed.
pub fn sealed_key_id(value: &[u8]) -> Option<KeyId> {
    split(value).map(|(id, _, _)| id)
}

fn split(value: &[u8]) -> Option<(KeyId, [u8; NONCE_LEN], &[u8])> {
    let rest = value.strip_prefix(MAGIC.as_slice())?;
    let (id, rest) = rest.split_first_chunk::<KEY_ID_LEN>()?;
    let (nonce, body) = rest.split_first_chunk::<NONCE_LEN>()?;
    Some((*id, *nonce, body))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn keys(bytes: &[u8]) -> ClusterKeys {
        let keys = bytes
            .iter()
            .map(|byte| ClusterKey::new(&[*byte; KEY_LEN]).unwrap())
            .collect();
        ClusterKeys::new(keys).unwrap()
    }

    #[test]
    fn a_sealed_value_opens_only_under_its_kv_key() {
        let keys = keys(&[1]);
        let sealed = keys.seal("r3v3rs3/v1/state/config", b"config").unwrap();
        assert_eq!(sealed_key_id(&sealed), Some(keys.primary_id()));
        assert!(
            !sealed
                .windows(b"config".len())
                .any(|part| part == b"config")
        );
        assert_eq!(
            keys.open("r3v3rs3/v1/state/config", &sealed).unwrap(),
            b"config"
        );

        let moved = keys.open("r3v3rs3/v1/state/accounts/admin", &sealed);
        assert!(moved.is_err(), "{moved:?}");
        let mut changed = sealed.clone();
        if let Some(last) = changed.last_mut() {
            *last ^= 1;
        }
        assert!(keys.open("r3v3rs3/v1/state/config", &changed).is_err());
        assert_ne!(
            sealed,
            keys.seal("r3v3rs3/v1/state/config", b"config").unwrap()
        );
    }

    #[test]
    fn the_first_key_seals_and_every_key_opens() {
        let old = keys(&[1]);
        let sealed = old.seal("k", b"value").unwrap();

        let rotated = keys(&[2, 1]);
        assert_eq!(rotated.open("k", &sealed).unwrap(), b"value");
        let resealed = rotated.seal("k", b"value").unwrap();
        assert_eq!(sealed_key_id(&resealed), Some(keys(&[2]).primary_id()));

        let error = old.open("k", &resealed).unwrap_err().to_string();
        assert!(error.contains("unknown encryption key"), "{error}");
    }

    #[test]
    fn keys_need_32_bytes_and_unique_ids() {
        assert!(ClusterKey::new(&[1; 16]).is_err());
        let twice = vec![
            ClusterKey::new(&[1; KEY_LEN]).unwrap(),
            ClusterKey::new(&[1; KEY_LEN]).unwrap(),
        ];
        assert!(ClusterKeys::new(twice).is_err());
        assert!(ClusterKeys::new(Vec::new()).is_err());
    }

    #[test]
    fn a_plain_value_has_no_key_id() {
        assert_eq!(
            sealed_key_id(br#"{"listen":"/ip4/0.0.0.0/tcp/80/http"}"#),
            None
        );
        assert_eq!(sealed_key_id(b"R3E1short"), None);
        assert!(keys(&[1]).open("k", b"{}").is_err());
    }
}
