//! MaxMind MMDB GeoIP country lookup.

use maxminddb::{Reader, geoip2::Country};
use std::net::IpAddr;
use std::path::Path;
use std::sync::Arc;
use tracing::{info, warn};

#[derive(Clone, Default)]
pub struct GeoIpResolver {
    reader: Option<Arc<Reader<Vec<u8>>>>,
}

impl std::fmt::Debug for GeoIpResolver {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("GeoIpResolver")
            .field("is_loaded", &self.is_loaded())
            .finish()
    }
}

impl GeoIpResolver {
    pub fn empty() -> Self {
        Self { reader: None }
    }

    pub fn is_loaded(&self) -> bool {
        self.reader.is_some()
    }

    pub fn from_file<P: AsRef<Path>>(path: P) -> Result<Self, anyhow::Error> {
        let path_ref = path.as_ref();
        let reader = Reader::open_readfile(path_ref)?;
        info!(path = %path_ref.display(), "loaded GeoIP database");
        Ok(Self {
            reader: Some(Arc::new(reader)),
        })
    }

    pub fn from_bytes(bytes: Vec<u8>) -> Result<Self, anyhow::Error> {
        let reader = Reader::from_source(bytes)?;
        Ok(Self {
            reader: Some(Arc::new(reader)),
        })
    }

    /// Tries loading from configured path, or falls back to `GeoLite2-Country.mmdb` or `geoip.mmdb` in config_dir.
    pub fn load_or_fallback(configured: Option<&Path>, config_dir: &Path) -> Self {
        if let Some(path) = configured {
            match Self::from_file(path) {
                Ok(resolver) => return resolver,
                Err(err) => {
                    warn!(%err, path = %path.display(), "failed to open configured GeoIP database")
                }
            }
        }
        for filename in ["GeoLite2-Country.mmdb", "geoip.mmdb", "GeoIP2-Country.mmdb"] {
            let path = config_dir.join(filename);
            if path.exists() {
                match Self::from_file(&path) {
                    Ok(resolver) => return resolver,
                    Err(err) => {
                        warn!(%err, path = %path.display(), "failed to open GeoIP database file")
                    }
                }
            }
        }
        Self::empty()
    }

    /// Looks up the two-letter ISO country code (e.g. "TR", "US", "DE") for the IP address.
    pub fn lookup_country(&self, ip: IpAddr) -> Option<String> {
        let reader = self.reader.as_ref()?;
        let lookup = reader.lookup(ip).ok()?;
        let country: Country = lookup.decode().ok()??;
        let iso = country
            .country
            .iso_code
            .or(country.registered_country.iso_code)?;
        Some(iso.to_ascii_uppercase())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn empty_resolver_returns_none() {
        let resolver = GeoIpResolver::empty();
        assert!(!resolver.is_loaded());
        assert_eq!(resolver.lookup_country("8.8.8.8".parse().unwrap()), None);
    }

    #[test]
    fn lookup_with_test_mmdb() {
        let path =
            Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/data/GeoIP2-Country-Test.mmdb");
        if !path.exists() {
            return;
        }
        let resolver = GeoIpResolver::from_file(&path).expect("load test mmdb");
        assert!(resolver.is_loaded());
        // In GeoIP2-Country-Test.mmdb:
        // 81.2.69.160 is in GB (United Kingdom)
        // 202.196.224.0/24 is in PH (Philippines)
        let gb_ip: IpAddr = "81.2.69.160".parse().unwrap();
        assert_eq!(resolver.lookup_country(gb_ip), Some("GB".to_string()));
    }
}
