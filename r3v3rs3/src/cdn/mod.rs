//! Known CDN edge IP ranges, used to decide which client IP headers can be trusted.

use arc_swap::{ArcSwap, Guard};
use ipnet::IpNet;
use once_cell::sync::Lazy;
use r3v3rs3_api::cdn::{CdnProvider, CdnProviderStatus, CdnRangesSource, CdnStatus};
use serde_derive::{Deserialize, Serialize};
use std::{
    collections::{BTreeMap, HashMap},
    net::IpAddr,
    sync::{Arc, Mutex},
};
use tracing::error;

pub mod fetch;

const EMBEDDED_SNAPSHOT: &str = include_str!("../../data/cdn-ranges.json");

static TABLE: Lazy<ArcSwap<CdnTable>> = Lazy::new(|| ArcSwap::from_pointee(embedded_table()));
static LAST_ERRORS: Mutex<Vec<String>> = Mutex::new(Vec::new());

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct CdnRanges {
    /// Unix time in seconds.
    pub updated_at: i64,
    pub providers: BTreeMap<CdnProvider, Vec<IpNet>>,
}

#[derive(Debug)]
pub struct CdnTable {
    ranges: CdnRanges,
    source: CdnRangesSource,
    exact: HashMap<IpAddr, CdnProvider>,
    prefixes: Vec<(IpNet, CdnProvider)>,
}

impl CdnTable {
    pub fn new(ranges: CdnRanges, source: CdnRangesSource) -> Self {
        let mut exact = HashMap::new();
        let mut prefixes = Vec::new();
        for (provider, nets) in &ranges.providers {
            for net in nets {
                if net.prefix_len() == net.max_prefix_len() {
                    exact.insert(net.addr(), *provider);
                } else {
                    prefixes.push((*net, *provider));
                }
            }
        }
        Self {
            ranges,
            source,
            exact,
            prefixes,
        }
    }

    pub fn empty() -> Self {
        Self::new(CdnRanges::default(), CdnRangesSource::Embedded)
    }

    /// Returns the provider that owns the address, if any.
    pub fn lookup(&self, ip: IpAddr) -> Option<CdnProvider> {
        let ip = ip.to_canonical();
        self.exact.get(&ip).copied().or_else(|| {
            self.prefixes
                .iter()
                .find(|(net, _)| net.contains(&ip))
                .map(|(_, provider)| *provider)
        })
    }

    pub fn ranges(&self) -> &CdnRanges {
        &self.ranges
    }

    pub fn status(&self, errors: Vec<String>) -> CdnStatus {
        status_of(&self.ranges, self.source, errors)
    }
}

pub fn status_of(ranges: &CdnRanges, source: CdnRangesSource, errors: Vec<String>) -> CdnStatus {
    CdnStatus {
        updated_at: ranges.updated_at,
        source,
        providers: ranges
            .providers
            .iter()
            .map(|(provider, nets)| CdnProviderStatus {
                provider: *provider,
                ranges: nets.len(),
            })
            .collect(),
        errors,
    }
}

fn embedded_table() -> CdnTable {
    match serde_json::from_str::<CdnRanges>(EMBEDDED_SNAPSHOT) {
        Ok(ranges) => CdnTable::new(ranges, CdnRangesSource::Embedded),
        Err(err) => {
            error!(%err, "failed to parse the embedded CDN IP ranges");
            CdnTable::empty()
        }
    }
}

/// Returns the active table.
pub fn table() -> Guard<Arc<CdnTable>> {
    TABLE.load()
}

/// Installs downloaded ranges when they are newer than the active table.
pub fn install(ranges: CdnRanges) -> bool {
    if ranges.updated_at <= TABLE.load().ranges.updated_at {
        return false;
    }
    TABLE.store(Arc::new(CdnTable::new(ranges, CdnRangesSource::Downloaded)));
    true
}

pub fn set_last_errors(errors: Vec<String>) {
    let mut last = LAST_ERRORS
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    *last = errors;
}

pub fn status() -> CdnStatus {
    let errors = LAST_ERRORS
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
        .clone();
    table().status(errors)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn nets(values: &[&str]) -> Vec<IpNet> {
        values.iter().map(|v| v.parse().unwrap()).collect()
    }

    #[test]
    fn embedded_snapshot_parses() {
        let ranges: CdnRanges = serde_json::from_str(EMBEDDED_SNAPSHOT).unwrap();
        for provider in CdnProvider::ALL {
            let count = ranges.providers.get(provider).map(Vec::len).unwrap_or(0);
            assert!(
                count > 0,
                "{provider} has no ranges in the embedded snapshot"
            );
        }
    }

    #[test]
    fn lookup_matches_prefixes_and_exact_addresses() {
        let ranges = CdnRanges {
            updated_at: 1,
            providers: BTreeMap::from([
                (
                    CdnProvider::Cloudflare,
                    nets(&["173.245.48.0/20", "2400:cb00::/32"]),
                ),
                (CdnProvider::Bunny, nets(&["89.187.188.227/32"])),
            ]),
        };
        let table = CdnTable::new(ranges, CdnRangesSource::Downloaded);

        assert_eq!(
            table.lookup("173.245.48.1".parse().unwrap()),
            Some(CdnProvider::Cloudflare)
        );
        assert_eq!(
            table.lookup("2400:cb00::1".parse().unwrap()),
            Some(CdnProvider::Cloudflare)
        );
        assert_eq!(
            table.lookup("::ffff:173.245.48.1".parse().unwrap()),
            Some(CdnProvider::Cloudflare)
        );
        assert_eq!(
            table.lookup("89.187.188.227".parse().unwrap()),
            Some(CdnProvider::Bunny)
        );
        assert_eq!(table.lookup("89.187.188.228".parse().unwrap()), None);
        assert_eq!(table.lookup("127.0.0.1".parse().unwrap()), None);
    }

    #[test]
    fn status_counts_ranges() {
        let ranges = CdnRanges {
            updated_at: 42,
            providers: BTreeMap::from([(CdnProvider::Keycdn, nets(&["68.70.192.0/20"]))]),
        };
        let status = status_of(&ranges, CdnRangesSource::Downloaded, vec![]);
        assert_eq!(status.updated_at, 42);
        assert_eq!(status.providers.len(), 1);
        assert_eq!(status.providers[0].ranges, 1);
    }
}
