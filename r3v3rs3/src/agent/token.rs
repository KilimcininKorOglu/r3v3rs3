//! The one-time enrollment token of an agent target: `<secret>.<ca_hash>`. The master stores only
//! the SHA-256 of the secret. The CA hash lets the agent recognize the master at its first
//! connection.

use anyhow::bail;
use rand::distr::{Alphanumeric, SampleString};
use sha2::{Digest, Sha256};
use std::fmt;
use std::str::FromStr;
use subtle::ConstantTimeEq;

/// The length of the secret: 43 alphanumeric characters carry more than 256 bits.
const SECRET_LENGTH: usize = 43;

/// The length of a SHA-256 in hex.
const HASH_LENGTH: usize = 64;

#[derive(Clone, PartialEq, Eq)]
pub struct EnrollmentToken {
    pub secret: String,
    pub ca_hash: String,
}

impl EnrollmentToken {
    /// A new token with a random secret for the CA with `ca_hash`.
    pub fn new(ca_hash: &str) -> Self {
        Self {
            secret: Alphanumeric.sample_string(&mut rand::rng(), SECRET_LENGTH),
            ca_hash: ca_hash.to_string(),
        }
    }

    /// The SHA-256 of the secret in lowercase hex, which the master stores.
    pub fn secret_hash(&self) -> String {
        secret_hash(&self.secret)
    }
}

/// The SHA-256 of an enrollment secret in lowercase hex.
pub fn secret_hash(secret: &str) -> String {
    hex::encode(Sha256::digest(secret.as_bytes()))
}

/// Whether `secret` has the stored hash. The comparison takes the same time for every secret.
pub fn secret_matches(secret: &str, stored_hash: &str) -> bool {
    secret_hash(secret)
        .as_bytes()
        .ct_eq(stored_hash.as_bytes())
        .into()
}

impl fmt::Display for EnrollmentToken {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}.{}", self.secret, self.ca_hash)
    }
}

// The secret stays out of every log line.
impl fmt::Debug for EnrollmentToken {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("EnrollmentToken")
            .field("ca_hash", &self.ca_hash)
            .finish_non_exhaustive()
    }
}

impl FromStr for EnrollmentToken {
    type Err = anyhow::Error;

    fn from_str(token: &str) -> Result<Self, Self::Err> {
        let Some((secret, ca_hash)) = token.trim().split_once('.') else {
            bail!("the enrollment token has no CA hash");
        };
        if secret.len() != SECRET_LENGTH || !secret.chars().all(|c| c.is_ascii_alphanumeric()) {
            bail!("the secret of the enrollment token is invalid");
        }
        let is_hex = ca_hash
            .chars()
            .all(|c| c.is_ascii_digit() || ('a'..='f').contains(&c));
        if ca_hash.len() != HASH_LENGTH || !is_hex {
            bail!("the CA hash of the enrollment token is invalid");
        }
        Ok(Self {
            secret: secret.to_string(),
            ca_hash: ca_hash.to_string(),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const CA_HASH: &str = "3f1c0e5b9a2d4c6e8f0a1b3c5d7e9f1a2b4c6d8e0f1a3b5c7d9e1f2a4b6c8d0e";

    #[test]
    fn a_token_survives_its_text_form() {
        let token = EnrollmentToken::new(CA_HASH);
        let parsed: EnrollmentToken = token.to_string().parse().unwrap();
        assert_eq!(parsed, token);
        assert_ne!(EnrollmentToken::new(CA_HASH).secret, token.secret);
    }

    #[test]
    fn only_the_secret_matches_its_hash() {
        let token = EnrollmentToken::new(CA_HASH);
        let stored = token.secret_hash();
        assert!(secret_matches(&token.secret, &stored));
        assert!(!secret_matches(
            &EnrollmentToken::new(CA_HASH).secret,
            &stored
        ));
        assert!(!secret_matches(&token.secret, ""));
    }

    #[test]
    fn a_malformed_token_is_refused() {
        let secret = "a".repeat(SECRET_LENGTH);
        for token in [
            String::new(),
            secret.clone(),
            format!("{secret}.{}", "g".repeat(HASH_LENGTH)),
            format!("{secret}.{}", CA_HASH.to_uppercase()),
            format!("{secret}.{}", &CA_HASH[1..]),
            format!("{}.{CA_HASH}", &secret[1..]),
            format!("{}-.{CA_HASH}", &secret[1..]),
        ] {
            assert!(token.parse::<EnrollmentToken>().is_err(), "{token}");
        }
    }

    #[test]
    fn the_debug_form_hides_the_secret() {
        let token = EnrollmentToken::new(CA_HASH);
        assert!(!format!("{token:?}").contains(&token.secret));
    }
}
