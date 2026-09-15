use crate::certs::{acme::AcmeTarget, Cert};
use indexmap::IndexMap;
use log::warn;
use r3v3rs3_api::discovery::{DiscoveryProvider, DiscoverySource};
use r3v3rs3_api::subject_name::SubjectName;
use r3v3rs3_api::{cert::CertKind, error::Error, id::ShortId};
use std::cmp::Ordering;
use std::collections::{BTreeMap, HashMap, HashSet};
use std::sync::Arc;
use tokio_rustls::rustls::RootCertStore;
use x509_parser::time::ASN1Time;

#[derive(Debug)]
pub struct CertList {
    certs: IndexMap<ShortId, Arc<Cert>>,
    system_root_certs: RootCertStore,
    root_certs: RootCertStore,
}

impl CertList {
    pub async fn new<I: IntoIterator<Item = Arc<Cert>>>(iter: I) -> Self {
        let mut certs = iter
            .into_iter()
            .map(|cert| (cert.id(), cert))
            .collect::<IndexMap<_, _>>();
        sort_certs(&mut certs);

        let mut system_root_certs = RootCertStore::empty();
        if let Ok(result) =
            tokio::task::spawn_blocking(rustls_native_certs::load_native_certs).await
        {
            for cert in result.certs {
                if let Err(err) = system_root_certs.add(cert) {
                    warn!("failed to add native certs: {err}");
                }
            }
            for error in result.errors {
                warn!("failed to load native certs: {error}");
            }
        }

        let mut this = Self {
            certs,
            system_root_certs: system_root_certs.clone(),
            root_certs: RootCertStore::empty(),
        };
        this.update_root_certs();
        this
    }

    pub fn iter(&self) -> impl Iterator<Item = &Arc<Cert>> {
        self.certs.values()
    }

    pub fn root_certs(&self) -> &RootCertStore {
        &self.root_certs
    }

    /// The ACME certificates of the target, from the newest to the oldest.
    pub fn find_certs_for_target(&self, target: &AcmeTarget) -> Vec<&Arc<Cert>> {
        self.certs
            .values()
            .filter(|cert| AcmeTarget::of_cert(cert).as_ref() == Some(target))
            .collect()
    }

    /// Whether a valid server certificate with a private key has every name.
    pub fn covers(&self, names: &[String]) -> bool {
        let Ok(names) = names
            .iter()
            .map(|name| name.parse::<SubjectName>())
            .collect::<Result<Vec<_>, _>>()
        else {
            return false;
        };
        self.certs.values().any(|cert| {
            cert.kind == CertKind::Server
                && cert.key.is_some()
                && cert.is_valid()
                && names.iter().all(|name| cert.has_subject_name(name))
        })
    }

    /// The ACME certificates that expired before `now`. A target in `active` keeps its newest
    /// certificate. A target that no entry or proxy orders loses every expired certificate.
    pub fn expired_acme_certs(&self, active: &HashSet<AcmeTarget>, now: ASN1Time) -> Vec<ShortId> {
        let mut groups = BTreeMap::<AcmeTarget, Vec<&Arc<Cert>>>::new();
        for cert in self.certs.values() {
            if let Some(target) = AcmeTarget::of_cert(cert) {
                groups.entry(target).or_default().push(cert);
            }
        }
        groups
            .into_iter()
            .flat_map(|(target, certs)| {
                let mut expired = certs
                    .iter()
                    .filter(|cert| cert.not_after < now)
                    .map(|cert| cert.id)
                    .collect::<Vec<_>>();
                if active.contains(&target) && expired.len() == certs.len() {
                    expired.remove(0);
                }
                expired
            })
            .collect()
    }

    pub fn get(&self, id: ShortId) -> Option<&Arc<Cert>> {
        self.certs.get(&id)
    }

    pub fn add(&mut self, cert: Arc<Cert>) {
        self.certs.insert(cert.id(), cert.clone());
        sort_certs(&mut self.certs);
        if cert.kind == CertKind::Root {
            self.update_root_certs();
        }
    }

    pub fn delete(&mut self, id: ShortId) -> Result<(), Error> {
        if !self.certs.contains_key(&id) {
            Err(Error::IdNotFound { id: id.to_string() })
        } else {
            if let Some(cert) = self.certs.swap_remove(&id) {
                if cert.kind == CertKind::Root {
                    self.update_root_certs();
                }
            }
            Ok(())
        }
    }

    /// Replaces the certificates that the storage keeps and keeps the discovered certificates.
    /// Returns true when the list changed.
    pub fn replace_stored(&mut self, certs: Vec<Arc<Cert>>) -> bool {
        let fingerprints = |certs: &mut dyn Iterator<Item = &Arc<Cert>>| {
            certs
                .map(|cert| (cert.id, cert.fingerprint.clone()))
                .collect::<HashMap<_, _>>()
        };
        let current = fingerprints(&mut self.certs.values().filter(|cert| cert.source.is_none()));
        if current == fingerprints(&mut certs.iter()) {
            return false;
        }
        self.certs.retain(|_, cert| cert.source.is_some());
        self.certs
            .extend(certs.into_iter().map(|cert| (cert.id, cert)));
        sort_certs(&mut self.certs);
        self.update_root_certs();
        true
    }

    /// Replaces the certificates of a discovery provider. A certificate that is already in the
    /// list without this provider as its source is skipped. Returns true when the list changed.
    pub fn replace_discovered(
        &mut self,
        provider: DiscoveryProvider,
        certs: Vec<Arc<Cert>>,
    ) -> bool {
        let owned = |cert: &Cert| {
            cert.source
                .as_ref()
                .is_some_and(|source| source.provider == provider)
        };
        let current = self
            .certs
            .values()
            .filter(|cert| owned(cert))
            .map(|cert| (cert.id, cert.source.clone()))
            .collect::<HashMap<ShortId, Option<DiscoverySource>>>();
        let next = certs
            .into_iter()
            .filter(|cert| {
                self.certs
                    .get(&cert.id)
                    .is_none_or(|existing| owned(existing))
            })
            .map(|cert| (cert.id, cert))
            .collect::<IndexMap<_, _>>();
        let unchanged = current.len() == next.len()
            && next
                .values()
                .all(|cert| current.get(&cert.id) == Some(&cert.source));
        if unchanged {
            return false;
        }
        self.certs.retain(|_, cert| !owned(cert));
        self.certs.extend(next);
        sort_certs(&mut self.certs);
        self.update_root_certs();
        true
    }

    fn update_root_certs(&mut self) {
        let mut root_certs = self.system_root_certs.clone();
        for cert in self.certs.values() {
            if cert.kind == CertKind::Root {
                if let Ok(certs) = cert.certificates() {
                    for cert in certs {
                        if let Err(err) = root_certs.add(cert) {
                            warn!("failed to add root cert: {}", err);
                        }
                    }
                }
            }
        }
        self.root_certs = root_certs;
    }
}

/// Sorts the certificates from the newest to the oldest.
fn sort_certs(certs: &mut IndexMap<ShortId, Arc<Cert>>) {
    certs.sort_unstable_by(|_, v1, _, v2| v1.partial_cmp(v2).unwrap_or(Ordering::Equal));
}

#[cfg(test)]
mod tests {
    use super::*;
    use r3v3rs3_api::cert::CertMetadata;
    use std::time::SystemTime;

    fn names(names: &[&str]) -> Vec<SubjectName> {
        names.iter().map(|name| name.parse().unwrap()).collect()
    }

    fn server_cert(san: &[&str]) -> Cert {
        let ca = Cert::new_ca().unwrap();
        Cert::new_self_signed(&names(san), &ca).unwrap()
    }

    /// A certificate that the ACME entry `acme_id` ordered for the names.
    fn acme_cert(acme_id: &str, san: &[&str]) -> Arc<Cert> {
        let cert = server_cert(san);
        let metadata = CertMetadata {
            acme_id: acme_id.parse().unwrap(),
            created_at: SystemTime::now(),
        };
        let chain = String::from_utf8(cert.pem_chain.clone()).unwrap();
        let pem = format!(
            "# {}\r\n\r\n{chain}",
            serde_qs::to_string(&metadata).unwrap()
        );
        Arc::new(Cert::new(CertKind::Server, pem.into_bytes(), cert.pem_key.clone()).unwrap())
    }

    fn target(acme_id: &str, names: &[&str]) -> AcmeTarget {
        AcmeTarget::new(acme_id.parse().unwrap(), names)
    }

    #[tokio::test]
    async fn the_certificates_of_an_entry_are_grouped_by_their_names() {
        let manual = acme_cert("abc", &["www.example.com", "example.com"]);
        let discovered = acme_cert("abc", &["app.example.com"]);
        let other_entry = acme_cert("def", &["example.com", "www.example.com"]);
        let certs = CertList::new([manual.clone(), discovered.clone(), other_entry]).await;

        let found =
            certs.find_certs_for_target(&target("abc", &["EXAMPLE.com", "www.example.com"]));
        assert_eq!(found, [&manual]);
        let found = certs.find_certs_for_target(&target("abc", &["app.example.com"]));
        assert_eq!(found, [&discovered]);
    }

    #[tokio::test]
    async fn a_valid_server_certificate_with_every_name_covers_the_names() {
        let wildcard = Arc::new(server_cert(&["*.example.com"]));
        let certs = CertList::new([wildcard]).await;
        let covers = |names: &[&str]| {
            certs.covers(
                &names
                    .iter()
                    .map(|name| name.to_string())
                    .collect::<Vec<_>>(),
            )
        };
        assert!(covers(&["app.example.com", "www.example.com"]));
        assert!(!covers(&["app.example.com", "example.com"]));
        assert!(!covers(&["app.example.org"]));
    }

    #[tokio::test]
    async fn only_an_ordered_target_keeps_an_expired_certificate() {
        let manual = [
            acme_cert("abc", &["example.com"]),
            acme_cert("abc", &["example.com"]),
        ];
        let removed_label = acme_cert("abc", &["app.example.com"]);
        let certs = CertList::new(manual.iter().cloned().chain([removed_label.clone()])).await;
        let active = HashSet::from([target("abc", &["example.com"])]);
        // rcgen certificates expire in 4096, before the end of 9999.
        let later = ASN1Time::from_timestamp(253_402_300_799).unwrap();

        let expired = certs.expired_acme_certs(&active, later);
        assert_eq!(expired.len(), 2);
        assert!(expired.contains(&removed_label.id));
        assert_eq!(
            manual
                .iter()
                .filter(|cert| expired.contains(&cert.id))
                .count(),
            1
        );
        assert!(certs
            .expired_acme_certs(&active, ASN1Time::now())
            .is_empty());
    }
}
