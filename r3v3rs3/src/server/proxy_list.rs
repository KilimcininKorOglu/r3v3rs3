use indexmap::map::Entry;
use indexmap::IndexMap;
use r3v3rs3_api::discovery::DiscoveryProvider;
use r3v3rs3_api::error::Error;
use r3v3rs3_api::id::ShortId;
use r3v3rs3_api::multiaddr::Multiaddr;
use r3v3rs3_api::port::PortEntry;
use r3v3rs3_api::proxy::{Proxy, ProxyEntry, ProxyKind, ProxyState, ProxyStatus};

#[derive(Debug)]
pub struct ProxyContext {
    pub entry: ProxyEntry,
    pub status: ProxyStatus,
}

impl ProxyContext {
    fn new(entry: ProxyEntry) -> Self {
        let state = if entry.proxy.active && !entry.proxy.ports.is_empty() {
            ProxyState::Active
        } else {
            ProxyState::Inactive
        };
        Self {
            entry,
            status: ProxyStatus {
                state,
                upstreams: Vec::new(),
                srv: Vec::new(),
            },
        }
    }
}

#[derive(Debug, PartialEq, Eq)]
pub struct DiscoveredUpdate {
    /// Whether the proxies of the provider changed.
    pub changed: bool,
    /// The proxies that were not added.
    pub skipped: Vec<ShortId>,
}

/// Whether a port with this listening address can serve a proxy of this kind.
pub fn accepts(kind: &ProxyKind, listen: &Multiaddr) -> bool {
    match kind {
        ProxyKind::Http(_) => listen.is_http(),
        ProxyKind::Tcp(_) => !listen.is_udp() && !listen.is_http(),
        ProxyKind::Udp(_) => listen.is_udp() && !listen.is_http(),
    }
}

fn is_from(entry: &ProxyEntry, provider: DiscoveryProvider) -> bool {
    entry
        .source
        .as_ref()
        .is_some_and(|source| source.provider == provider)
}

#[derive(Debug, Default)]
pub struct ProxyList {
    entries: IndexMap<ShortId, ProxyContext>,
}

impl FromIterator<ProxyEntry> for ProxyList {
    fn from_iter<I: IntoIterator<Item = ProxyEntry>>(iter: I) -> Self {
        Self {
            entries: iter
                .into_iter()
                .map(|proxy| (proxy.id, ProxyContext::new(proxy)))
                .collect(),
        }
    }
}

impl ProxyList {
    pub fn get(&self, id: ShortId) -> Option<&ProxyContext> {
        self.entries.get(&id)
    }

    pub fn entries(&self) -> impl Iterator<Item = &ProxyEntry> {
        self.entries.values().map(|ctx| &ctx.entry)
    }

    pub fn contexts(&self) -> impl Iterator<Item = &ProxyContext> {
        self.entries.values()
    }

    pub fn set(&mut self, entry: ProxyEntry) -> bool {
        self.remove_deplicate_ports(&entry.proxy);
        match self.entries.entry(entry.id) {
            Entry::Occupied(mut e) => {
                if e.get().entry.proxy != entry.proxy {
                    e.insert(ProxyContext::new(entry));
                    true
                } else {
                    false
                }
            }
            Entry::Vacant(inner) => {
                inner.insert(ProxyContext::new(entry));
                true
            }
        }
    }

    pub fn delete(&mut self, id: ShortId) -> Result<(), Error> {
        if !self.entries.contains_key(&id) {
            Err(Error::IdNotFound { id: id.to_string() })
        } else {
            self.entries.swap_remove(&id);
            Ok(())
        }
    }

    /// Replaces the manual proxies and keeps the discovered proxies. Returns true when the list
    /// changed.
    pub fn replace_manual(&mut self, entries: Vec<ProxyEntry>) -> bool {
        let unchanged = self
            .entries()
            .filter(|entry| !entry.is_discovered())
            .eq(entries.iter());
        if unchanged {
            return false;
        }
        self.entries.retain(|_, ctx| ctx.entry.is_discovered());
        for entry in entries {
            self.entries.insert(entry.id, ProxyContext::new(entry));
        }
        true
    }

    pub fn remove_incompatible_ports(&mut self, ports: &[PortEntry]) -> bool {
        let mut changed = false;
        for ctx in self.entries.values_mut() {
            let len = ctx.entry.proxy.ports.len();
            ctx.entry.proxy.ports = ctx
                .entry
                .proxy
                .ports
                .drain(..)
                .filter(|port| {
                    ports
                        .iter()
                        .find(|p| p.id == *port)
                        .is_some_and(|port| accepts(&ctx.entry.proxy.kind, &port.port.listen))
                })
                .collect();
            changed |= len != ctx.entry.proxy.ports.len();
        }
        changed
    }

    /// Replaces the proxies of a discovery provider. A proxy is skipped when it is not from the
    /// provider, when its id is already used, or when it is a TCP proxy on a port of another TCP
    /// proxy.
    pub fn replace_discovered(
        &mut self,
        provider: DiscoveryProvider,
        entries: Vec<ProxyEntry>,
    ) -> DiscoveredUpdate {
        let previous = self.discovered_by(provider);
        self.entries.retain(|_, ctx| !is_from(&ctx.entry, provider));
        let mut skipped = Vec::new();
        for entry in entries {
            if !is_from(&entry, provider)
                || self.entries.contains_key(&entry.id)
                || self.uses_taken_tcp_port(&entry.proxy)
            {
                skipped.push(entry.id);
                continue;
            }
            self.entries.insert(entry.id, ProxyContext::new(entry));
        }
        DiscoveredUpdate {
            changed: previous != self.discovered_by(provider),
            skipped,
        }
    }

    fn discovered_by(&self, provider: DiscoveryProvider) -> Vec<ProxyEntry> {
        self.entries()
            .filter(|entry| is_from(entry, provider))
            .cloned()
            .collect()
    }

    fn uses_taken_tcp_port(&self, proxy: &Proxy) -> bool {
        let is_tcp = |proxy: &Proxy| matches!(proxy.kind, ProxyKind::Tcp(_));
        is_tcp(proxy)
            && self.entries().any(|entry| {
                is_tcp(&entry.proxy) && entry.proxy.ports.iter().any(|p| proxy.ports.contains(p))
            })
    }

    fn remove_deplicate_ports(&mut self, proxy: &Proxy) {
        if let ProxyKind::Tcp(_) = &proxy.kind {
            for ctx in self.entries.values_mut() {
                ctx.entry.proxy.ports = ctx
                    .entry
                    .proxy
                    .ports
                    .drain(..)
                    .filter(|port| !proxy.ports.contains(port))
                    .collect();
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use r3v3rs3_api::discovery::DiscoverySource;
    use r3v3rs3_api::proxy::TcpProxy;

    fn entry(id: &str, kind: ProxyKind, provider: Option<DiscoveryProvider>) -> ProxyEntry {
        ProxyEntry {
            id: id.parse().unwrap(),
            proxy: Proxy {
                ports: vec!["port".parse().unwrap()],
                kind,
                ..Default::default()
            },
            source: provider.map(|provider| DiscoverySource {
                provider,
                resource: id.into(),
            }),
        }
    }

    fn http() -> ProxyKind {
        ProxyKind::Http(Box::default())
    }

    fn tcp() -> ProxyKind {
        ProxyKind::Tcp(TcpProxy::default())
    }

    const DOCKER: Option<DiscoveryProvider> = Some(DiscoveryProvider::Docker);
    const CONSUL: Option<DiscoveryProvider> = Some(DiscoveryProvider::Consul);

    #[test]
    fn replace_discovered_changes_only_the_proxies_of_the_provider() {
        let mut list = ProxyList::from_iter([
            entry("manual", http(), None),
            entry("consul", http(), CONSUL),
        ]);

        let update = list.replace_discovered(
            DiscoveryProvider::Docker,
            vec![entry("web", http(), DOCKER)],
        );
        assert_eq!(
            update,
            DiscoveredUpdate {
                changed: true,
                skipped: vec![]
            }
        );
        let ids = |list: &ProxyList| list.entries().map(|e| e.id.to_string()).collect::<Vec<_>>();
        assert_eq!(ids(&list), ["manual", "consul", "web"]);

        let update = list.replace_discovered(
            DiscoveryProvider::Docker,
            vec![entry("web", http(), DOCKER)],
        );
        assert!(!update.changed);

        let update = list.replace_discovered(DiscoveryProvider::Docker, vec![]);
        assert!(update.changed);
        assert_eq!(ids(&list), ["manual", "consul"]);
    }

    #[test]
    fn replace_discovered_skips_conflicting_proxies() {
        let mut list = ProxyList::from_iter([entry("manual", tcp(), None)]);
        let update = list.replace_discovered(
            DiscoveryProvider::Docker,
            vec![
                entry("manual", http(), DOCKER),
                entry("stream", tcp(), DOCKER),
                entry("other", http(), CONSUL),
                entry("web", http(), DOCKER),
            ],
        );
        let skipped = update
            .skipped
            .iter()
            .map(ToString::to_string)
            .collect::<Vec<_>>();
        assert_eq!(skipped, ["manual", "stream", "other"]);
        assert!(update.changed);
        assert_eq!(list.entries().count(), 2);
        assert!(list
            .get("manual".parse().unwrap())
            .is_some_and(|ctx| !ctx.entry.is_discovered()));
    }
}
