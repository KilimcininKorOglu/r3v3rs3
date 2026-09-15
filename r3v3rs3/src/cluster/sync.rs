//! Follows the changes of the cluster store and sends them to the server.

use super::layout::StateKind;
use super::storage::KvStorage;
use crate::command::ServerCommand;
use crate::kv::{KvList, KvWatcher, WatchBatch};
use r3v3rs3_api::cluster::{ClusterState, ClusterStatus};
use std::collections::BTreeSet;
use std::sync::Arc;
use std::time::{Duration, Instant};
use tokio::sync::mpsc;
use tokio::task::JoinHandle;
use tracing::{error, info};

/// The wait after a failed read of the store, and the interval of the checks while the store
/// fails.
const RETRY_DELAY: Duration = Duration::from_secs(1);

/// Starts the task that applies the changes of the store to the server. The task ends when the
/// server stops.
pub fn spawn(storage: Arc<KvStorage>, command: mpsc::Sender<ServerCommand>) -> JoinHandle<()> {
    let status = ClusterStatus {
        state: ClusterState::Syncing,
        node_name: storage.cluster_config().node_name.clone(),
        ..Default::default()
    };
    let follower = Follower {
        storage,
        command,
        status,
        failing_since: None,
    };
    tokio::spawn(follower.run())
}

struct Follower {
    storage: Arc<KvStorage>,
    command: mpsc::Sender<ServerCommand>,
    status: ClusterStatus,
    /// The time of the first failure since the last successful read.
    failing_since: Option<Instant>,
}

impl Follower {
    async fn run(mut self) {
        if !self.publish().await {
            return;
        }
        let Some(from) = self.first_list().await else {
            return;
        };
        // The server loaded the state before this list, so it reads every part once again.
        if !self.changed(StateKind::ALL.to_vec(), from.revision).await {
            return;
        }
        let mut watcher = self
            .storage
            .store()
            .watch(&self.storage.layout().state(), &from);
        while self.step(watcher.as_mut()).await {}
        info!("the server stopped, so the cluster sync stops");
    }

    /// Lists the state until the store answers. `None` when the server stopped.
    async fn first_list(&mut self) -> Option<KvList> {
        loop {
            let prefix = self.storage.layout().state();
            match self.storage.store().list(&prefix).await {
                Ok(list) => return Some(list),
                Err(err) if self.failed(&err).await => tokio::time::sleep(RETRY_DELAY).await,
                Err(_) => return None,
            }
        }
    }

    /// Waits for the next changes. While the store fails, a check of the store ends the wait
    /// after [`RETRY_DELAY`], so the node sees that the store is back without a change. False
    /// when the server stopped.
    async fn step(&mut self, watcher: &mut dyn KvWatcher) -> bool {
        let next = if self.failing_since.is_some() {
            tokio::time::timeout(RETRY_DELAY, watcher.next()).await
        } else {
            Ok(watcher.next().await)
        };
        match next {
            Ok(Ok(batch)) => {
                let (kinds, revision) = self.kinds(batch);
                self.changed(kinds, revision).await
            }
            Ok(Err(err)) => {
                let running = self.failed(&err).await;
                tokio::time::sleep(RETRY_DELAY).await;
                running
            }
            Err(_) => self.check_store().await,
        }
    }

    async fn check_store(&mut self) -> bool {
        match self
            .storage
            .store()
            .get(&self.storage.layout().schema())
            .await
        {
            Ok(_) => {
                let revision = self.status.revision;
                self.changed(Vec::new(), revision).await
            }
            Err(err) => self.failed(&err).await,
        }
    }

    fn kinds(&self, batch: WatchBatch) -> (Vec<StateKind>, u64) {
        let layout = self.storage.layout();
        match batch {
            WatchBatch::Events { events, revision } => {
                let kinds = events
                    .iter()
                    .filter_map(|event| layout.kind(event.key()))
                    .collect::<BTreeSet<_>>();
                (kinds.into_iter().collect(), revision)
            }
            WatchBatch::Resync(list) => (StateKind::ALL.to_vec(), list.revision),
        }
    }

    /// Sends the changed parts to the server and marks the node as synced. After a failure the
    /// server reads every part again, because the node can miss changes while the store fails.
    async fn changed(&mut self, kinds: Vec<StateKind>, revision: u64) -> bool {
        let kinds = match self.failing_since.take() {
            Some(_) => StateKind::ALL.to_vec(),
            None => kinds,
        };
        self.storage.set_healthy(true);
        let recovered = self.status.state != ClusterState::Synced || self.status.error.is_some();
        self.status.state = ClusterState::Synced;
        self.status.revision = revision;
        self.status.error = None;
        if kinds.is_empty() && !recovered {
            return true;
        }
        if !kinds.is_empty() {
            let command = ServerCommand::ClusterChanged { kinds };
            if self.command.send(command).await.is_err() {
                return false;
            }
        }
        self.publish().await
    }

    /// Records a failure. A failure that lasts `lock_ttl` marks the node as degraded, and the
    /// storage rejects changes until the store is back.
    async fn failed(&mut self, err: &anyhow::Error) -> bool {
        error!("the cluster store failed: {err:#}");
        let since = *self.failing_since.get_or_insert_with(Instant::now);
        if since.elapsed() >= self.storage.cluster_config().lock_ttl {
            self.storage.set_healthy(false);
            self.status.state = ClusterState::Degraded;
        }
        self.status.error = Some(format!("{err:#}"));
        self.publish().await
    }

    async fn publish(&self) -> bool {
        let status = self.status.clone();
        let command = ServerCommand::SetClusterStatus { status };
        self.command.send(command).await.is_ok()
    }
}
