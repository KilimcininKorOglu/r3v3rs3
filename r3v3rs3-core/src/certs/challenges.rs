//! The ACME challenges that a server answers while an order runs.

use super::alpn::TlsAlpnChallenge;
use sha2::{Digest, Sha256};
use std::collections::HashMap;

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ServedChallenges {
    /// Key authorizations of the HTTP-01 challenges, by token.
    pub http: HashMap<String, String>,
    pub tls_alpn: Vec<TlsAlpnChallenge>,
}

impl ServedChallenges {
    pub fn is_empty(&self) -> bool {
        self.http.is_empty() && self.tls_alpn.is_empty()
    }

    /// The SHA-256 digest of the challenges in hex. The order of the challenges does not change
    /// it, so two nodes with the same challenges have the same digest.
    pub fn digest(&self) -> String {
        let mut http = self.http.iter().collect::<Vec<_>>();
        http.sort();
        let mut tls_alpn = self
            .tls_alpn
            .iter()
            .map(|challenge| (challenge.domain.to_ascii_lowercase(), challenge.digest))
            .collect::<Vec<_>>();
        tls_alpn.sort();

        let mut hasher = Sha256::new();
        for (token, authorization) in http {
            update(&mut hasher, token.as_bytes());
            update(&mut hasher, authorization.as_bytes());
        }
        // A marker between the parts, so an HTTP-01 entry cannot read as a TLS-ALPN-01 entry.
        update(&mut hasher, b"tls-alpn");
        for (domain, digest) in tls_alpn {
            update(&mut hasher, domain.as_bytes());
            update(&mut hasher, &digest);
        }
        hex::encode(hasher.finalize())
    }
}

/// Adds the length before the bytes, so the boundaries of the values are part of the digest.
fn update(hasher: &mut Sha256, bytes: &[u8]) {
    hasher.update((bytes.len() as u64).to_be_bytes());
    hasher.update(bytes);
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tls_alpn(domain: &str, byte: u8) -> TlsAlpnChallenge {
        TlsAlpnChallenge {
            domain: domain.to_string(),
            digest: [byte; 32],
        }
    }

    #[test]
    fn the_digest_depends_on_the_challenges_and_not_their_order() {
        let first = ServedChallenges {
            http: HashMap::from([("a".into(), "a.key".into()), ("b".into(), "b.key".into())]),
            tls_alpn: vec![tls_alpn("one.example", 1), tls_alpn("two.example", 2)],
        };
        let mut reordered = first.clone();
        reordered.tls_alpn.reverse();
        assert_eq!(first.digest(), reordered.digest());

        let mut changed = first.clone();
        changed.http.insert("b".into(), "other.key".into());
        assert_ne!(first.digest(), changed.digest());

        // The boundary between a token and its key authorization is part of the digest.
        let joined = ServedChallenges {
            http: HashMap::from([("ab".into(), "c".into())]),
            ..Default::default()
        };
        let split = ServedChallenges {
            http: HashMap::from([("a".into(), "bc".into())]),
            ..Default::default()
        };
        assert_ne!(joined.digest(), split.digest());
        assert!(ServedChallenges::default().is_empty());
        assert!(!first.is_empty());
    }
}
