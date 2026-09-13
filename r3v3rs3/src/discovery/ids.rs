//! Ids of discovered proxies. The id comes from the provider and the key of the proxy, so a
//! proxy keeps its id across reads and restarts.

use r3v3rs3_api::discovery::DiscoveryProvider;
use r3v3rs3_api::id::ShortId;
use sha2::{Digest, Sha256};
use std::collections::HashSet;

/// The letters of the ids that the server generates.
const LETTERS: &[u8] = b"bcdfghjklmnpqrstvwxyz";

/// Returns the id of the proxy. When another resource uses the id, the next candidate is used.
pub fn discovered_id(provider: DiscoveryProvider, key: &str, taken: &HashSet<ShortId>) -> ShortId {
    let mut salt = 0_u64;
    loop {
        if let Some(id) = candidate(provider, key, salt).filter(|id| !taken.contains(id)) {
            return id;
        }
        salt += 1;
    }
}

fn candidate(provider: DiscoveryProvider, key: &str, salt: u64) -> Option<ShortId> {
    let digest = Sha256::digest(format!("{provider}\0{key}\0{salt}"));
    let letters = digest[..6]
        .iter()
        .map(|byte| char::from(LETTERS[usize::from(*byte) % LETTERS.len()]))
        .collect::<String>();
    format!("{}-{}", &letters[..3], &letters[3..]).parse().ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_id_is_stable_and_skips_taken_ids() {
        let taken = HashSet::new();
        let id = discovered_id(DiscoveryProvider::Docker, "compose/app/http.app", &taken);
        assert_eq!(
            id,
            discovered_id(DiscoveryProvider::Docker, "compose/app/http.app", &taken)
        );
        assert_eq!(id.to_string().len(), 7);
        assert_ne!(
            id,
            discovered_id(DiscoveryProvider::Consul, "compose/app/http.app", &taken)
        );

        let taken = HashSet::from([id]);
        let other = discovered_id(DiscoveryProvider::Docker, "compose/app/http.app", &taken);
        assert_ne!(other, id);
    }
}
