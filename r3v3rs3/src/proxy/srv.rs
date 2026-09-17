//! DNS SRV lookups of the `http+srv` and `https+srv` server URLs of HTTP routes.
//!
//! The registry keeps the targets of each SRV name. A task per name resolves the name again when
//! the TTL of the answer expires, and asks the server to reload the proxies when the targets
//! changed.

use crate::certs::dns::build_resolver;
use crate::command::ServerCommand;
use hickory_proto::rr::rdata::SRV;
use hickory_proto::rr::RData;
use r3v3rs3_api::{proxy::Server, upstream::SrvStatus};
use std::{
    collections::{BTreeSet, HashMap},
    net::SocketAddr,
    sync::{Arc, Mutex, MutexGuard, PoisonError},
    time::{Duration, Instant},
};
use tokio::{sync::mpsc, task::JoinHandle};
use tracing::{debug, info, warn};

/// Shortest wait between two lookups of a name, also when the TTL is shorter or a lookup failed.
pub const MIN_TTL: Duration = Duration::from_secs(5);

/// One target of an SRV name. The order sorts the targets by host and port, so the same set of
/// records gives the same list.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
pub struct SrvTarget {
    pub host: String,
    pub port: u16,
    pub weight: u16,
}

#[derive(Debug)]
struct SrvEntry {
    /// The targets of the last successful lookup. A failed lookup keeps them.
    targets: Vec<SrvTarget>,
    /// The error of the last lookup. `None` when the last lookup succeeded.
    error: Option<String>,
    /// The time of the last successful lookup, in seconds since the Unix epoch.
    refreshed_at: Option<u64>,
    expires_at: Instant,
    task: Option<JoinHandle<()>>,
}

impl SrvEntry {
    fn new() -> Self {
        Self {
            targets: Vec::new(),
            error: None,
            refreshed_at: None,
            expires_at: Instant::now(),
            task: None,
        }
    }

    /// Stores the result of a lookup. Returns true when the targets changed.
    fn apply(&mut self, name: &str, result: Result<(Vec<SrvTarget>, Duration), String>) -> bool {
        match result {
            Ok((targets, ttl)) => {
                self.expires_at = Instant::now() + ttl.max(MIN_TTL);
                self.error = None;
                self.refreshed_at = Some(crate::clock::unix_ms() / 1000);
                let changed = targets != self.targets;
                if changed {
                    info!(name, targets = ?targets, "SRV targets changed");
                }
                self.targets = targets;
                changed
            }
            Err(err) => {
                warn!(name, %err, "SRV lookup failed");
                self.expires_at = Instant::now() + MIN_TTL;
                self.error = Some(err);
                false
            }
        }
    }

    fn status(&self, name: &str) -> SrvStatus {
        SrvStatus {
            name: name.to_string(),
            targets: self
                .targets
                .iter()
                .map(|target| format!("{}:{}", target.host, target.port))
                .collect(),
            error: self.error.clone(),
            refreshed_at: self.refreshed_at,
        }
    }

    fn abort(&mut self) {
        if let Some(task) = self.task.take() {
            task.abort();
        }
    }
}

#[derive(Debug, Default)]
struct Inner {
    nameserver: Option<SocketAddr>,
    names: HashMap<String, SrvEntry>,
}

/// The resolved targets of every SRV name that an active HTTP proxy uses.
#[derive(Debug, Default)]
pub struct SrvRegistry {
    inner: Arc<Mutex<Inner>>,
}

impl SrvRegistry {
    /// Resolves the new names, forgets the names that are not in `names` any more, and starts a
    /// refresh task for each new name. The task sends `ServerCommand::SrvUpdated` to `sender`
    /// when the targets of the name change.
    pub async fn sync(&self, names: BTreeSet<String>, sender: mpsc::Sender<ServerCommand>) {
        let (nameserver, new_names) = {
            let mut inner = lock(&self.inner);
            inner.names.retain(|name, entry| {
                let keep = names.contains(name);
                if !keep {
                    entry.abort();
                }
                keep
            });
            let new_names = names
                .into_iter()
                .filter(|name| !inner.names.contains_key(name))
                .collect::<Vec<_>>();
            (inner.nameserver, new_names)
        };
        for name in new_names {
            let result = resolve(nameserver, &name).await;
            let mut inner = lock(&self.inner);
            let entry = inner
                .names
                .entry(name.clone())
                .or_insert_with(SrvEntry::new);
            entry.apply(&name, result);
            entry.abort();
            entry.task = Some(spawn_refresh(self.inner.clone(), name, sender.clone()));
        }
    }

    /// Sets the DNS server of the lookups. Returns true when it changed: the names are forgotten,
    /// so the next `sync` resolves every name with the new server.
    pub fn set_nameserver(&self, addr: Option<SocketAddr>) -> bool {
        let mut inner = lock(&self.inner);
        if inner.nameserver == addr {
            return false;
        }
        inner.nameserver = addr;
        for entry in inner.names.values_mut() {
            entry.abort();
        }
        inner.names.clear();
        true
    }

    /// The lookup state of each name, in the order of `names`. A name that the registry does not
    /// know is skipped.
    pub fn snapshot(&self, names: &[String]) -> Vec<SrvStatus> {
        let inner = lock(&self.inner);
        names
            .iter()
            .filter_map(|name| Some(inner.names.get(name)?.status(name)))
            .collect()
    }

    /// Replaces each SRV server with one server per resolved target. A name without targets
    /// contributes no server.
    pub fn expand(&self, servers: &[Server]) -> Vec<Server> {
        let inner = lock(&self.inner);
        servers
            .iter()
            .flat_map(|server| match server.url.srv_name() {
                None => vec![server.clone()],
                Some(name) => inner
                    .names
                    .get(name)
                    .map(|entry| targets_to_servers(server, &entry.targets))
                    .unwrap_or_default(),
            })
            .collect()
    }
}

fn targets_to_servers(server: &Server, targets: &[SrvTarget]) -> Vec<Server> {
    targets
        .iter()
        .filter_map(|target| {
            let url = server.url.with_srv_target(&target.host, target.port)?;
            Some(Server {
                url,
                weight: target.weight,
            })
        })
        .collect()
}

fn lock(inner: &Mutex<Inner>) -> MutexGuard<'_, Inner> {
    inner.lock().unwrap_or_else(PoisonError::into_inner)
}

/// Resolves the name again each time its TTL expires, until the registry forgets the name.
fn spawn_refresh(
    inner: Arc<Mutex<Inner>>,
    name: String,
    sender: mpsc::Sender<ServerCommand>,
) -> JoinHandle<()> {
    tokio::spawn(async move {
        loop {
            let Some((nameserver, expires_at)) = next_lookup(&inner, &name) else {
                return;
            };
            tokio::time::sleep_until(expires_at.into()).await;
            let result = resolve(nameserver, &name).await;
            let changed = match lock(&inner).names.get_mut(&name) {
                Some(entry) => entry.apply(&name, result),
                None => return,
            };
            if changed {
                let update = ServerCommand::SrvUpdated { name: name.clone() };
                if sender.send(update).await.is_err() {
                    return;
                }
            }
        }
    })
}

/// The DNS server and the time of the next lookup of a name. `None` when the registry forgot
/// the name.
fn next_lookup(inner: &Mutex<Inner>, name: &str) -> Option<(Option<SocketAddr>, Instant)> {
    let inner = lock(inner);
    let entry = inner.names.get(name)?;
    Some((inner.nameserver, entry.expires_at))
}

/// Looks up the SRV records of the name and returns the targets with the TTL of the answer. A
/// new resolver for each lookup, so a cached answer does not hide a change.
async fn resolve(
    nameserver: Option<SocketAddr>,
    name: &str,
) -> Result<(Vec<SrvTarget>, Duration), String> {
    let resolver = build_resolver(nameserver).map_err(|err| err.to_string())?;
    let lookup = resolver
        .srv_lookup(format!("{name}."))
        .await
        .map_err(|err| err.to_string())?;
    let ttl = lookup.valid_until().saturating_duration_since(Instant::now());
    let targets = lowest_priority(lookup.answers().iter().filter_map(|record| match &record.data {
        RData::SRV(srv) => Some(srv),
        _ => None,
    }));
    debug!(name, targets = ?targets, ttl = ?ttl, "SRV lookup finished");
    Ok((targets, ttl))
}

/// The targets of the records with the lowest priority value, sorted by host and port. A weight
/// of `0` becomes `1`, because a server with weight `0` gets no traffic in r3v3rs3. A record
/// whose target is `.` names no server.
fn lowest_priority<'a>(records: impl Iterator<Item = &'a SRV>) -> Vec<SrvTarget> {
    let records = records.collect::<Vec<_>>();
    let Some(priority) = records.iter().map(|record| record.priority).min() else {
        return Vec::new();
    };
    let mut targets = records
        .iter()
        .filter(|record| record.priority == priority)
        .filter_map(|record| {
            let host = record.target.to_utf8();
            let host = host.trim_end_matches('.');
            (!host.is_empty()).then(|| SrvTarget {
                host: host.to_string(),
                port: record.port,
                weight: record.weight.max(1),
            })
        })
        .collect::<Vec<_>>();
    targets.sort();
    targets.dedup();
    targets
}

#[cfg(test)]
mod tests {
    use super::*;
    use hickory_proto::rr::Name;
    use std::str::FromStr;

    fn record(priority: u16, weight: u16, port: u16, target: &str) -> SRV {
        SRV::new(priority, weight, port, Name::from_str(target).unwrap())
    }

    #[test]
    fn lowest_priority_keeps_one_priority_and_sorts_the_targets() {
        let records = [
            record(20, 5, 9000, "backup.example.com."),
            record(10, 0, 8080, "b.example.com."),
            record(10, 3, 8080, "a.example.com."),
            record(10, 3, 8080, "a.example.com."),
            record(10, 1, 1, "."),
        ];
        let targets = lowest_priority(records.iter());
        assert_eq!(
            targets,
            [
                SrvTarget {
                    host: "a.example.com".into(),
                    port: 8080,
                    weight: 3,
                },
                SrvTarget {
                    host: "b.example.com".into(),
                    port: 8080,
                    weight: 1,
                },
            ]
        );
        assert!(lowest_priority([].iter()).is_empty());
    }

    #[test]
    fn expand_replaces_an_srv_server_with_its_targets() {
        let registry = SrvRegistry::default();
        let name = "_http._tcp.app.example.com";
        {
            let mut inner = lock(&registry.inner);
            let mut entry = SrvEntry::new();
            entry.targets = vec![SrvTarget {
                host: "10.0.0.1".into(),
                port: 8080,
                weight: 2,
            }];
            inner.names.insert(name.into(), entry);
        }
        let servers = [
            Server::new("http://static.example.com/".parse().unwrap()),
            Server::new(format!("http+srv://{name}/api").parse().unwrap()),
            Server::new(
                "http+srv://_http._tcp.unknown.example.com/"
                    .parse()
                    .unwrap(),
            ),
        ];
        let expanded = registry.expand(&servers);
        let urls = expanded
            .iter()
            .map(|server| (server.url.to_string(), server.weight))
            .collect::<Vec<_>>();
        assert_eq!(
            urls,
            [
                ("http://static.example.com/".to_string(), 1),
                ("http://10.0.0.1:8080/api".to_string(), 2),
            ]
        );
    }

    #[test]
    fn a_failed_lookup_keeps_the_targets_and_a_changed_answer_reports_it() {
        let mut entry = SrvEntry::new();
        let target = SrvTarget {
            host: "a".into(),
            port: 1,
            weight: 1,
        };
        assert!(entry.apply("n", Ok((vec![target.clone()], Duration::from_secs(1)))));
        assert!(entry.expires_at >= Instant::now() + Duration::from_secs(4));
        assert!(!entry.apply("n", Ok((vec![target.clone()], Duration::from_secs(60)))));
        assert!(entry.refreshed_at.is_some());
        assert!(!entry.apply("n", Err("timed out".into())));
        assert_eq!(entry.targets, [target]);
        let status = entry.status("n");
        assert_eq!(status.targets, ["a:1"]);
        assert_eq!(status.error.as_deref(), Some("timed out"));
        assert!(status.refreshed_at.is_some());
    }
}
