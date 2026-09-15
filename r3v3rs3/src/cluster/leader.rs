//! Holds the leader lock of the cluster. Only one node runs the tasks that must not run twice.

use super::storage::KvStorage;
use super::{answer_within, check_interval};
use crate::command::ServerCommand;
use crate::kv::Lease;
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::mpsc;
use tokio::task::JoinHandle;
use tracing::{error, info};

/// Starts the task that tries to hold the leader lock. The task ends when the server stops, and
/// it then gives the lock back.
pub fn spawn(storage: Arc<KvStorage>, command: mpsc::Sender<ServerCommand>) -> JoinHandle<()> {
    let candidate = Candidate {
        storage,
        command,
        leader: false,
    };
    tokio::spawn(candidate.run())
}

struct Candidate {
    storage: Arc<KvStorage>,
    command: mpsc::Sender<ServerCommand>,
    leader: bool,
}

impl Candidate {
    /// Checks the lease and the lock three times in each `lock_ttl`, so the lease does not end
    /// while the store answers. A store call waits one interval at most, so the node stops leading
    /// before its lease can end on a store that does not answer.
    fn interval(&self) -> Duration {
        check_interval(&self.storage)
    }

    async fn run(mut self) {
        while !self.command.is_closed() {
            let store = self.storage.store();
            match answer_within(self.interval(), store.grant_lease(self.ttl())).await {
                Ok(lease) => {
                    self.hold(&lease).await;
                    let store = self.storage.store();
                    let revoked = answer_within(self.interval(), store.revoke_lease(&lease)).await;
                    if let Err(err) = revoked {
                        error!("failed to revoke the leader lease: {err:#}");
                    }
                }
                Err(err) => error!("failed to get a leader lease: {err:#}"),
            }
            if !self.set_leader(false).await {
                break;
            }
            tokio::time::sleep(self.interval()).await;
        }
        info!("the server stopped, so the node leaves the leader election");
    }

    fn ttl(&self) -> Duration {
        self.storage.cluster_config().lock_ttl
    }

    /// Keeps the lease and tries to take the lock until the lease fails or the server stops.
    async fn hold(&mut self, lease: &Lease) {
        if let Err(err) = self.register(lease).await {
            error!("failed to register the node: {err:#}");
            return;
        }
        let mut ticks = tokio::time::interval(self.interval());
        loop {
            ticks.tick().await;
            if self.command.is_closed() {
                return;
            }
            match self.try_lock(lease).await {
                Ok(leader) if self.set_leader(leader).await => {}
                Ok(_) => return,
                Err(err) => {
                    error!("the leader lease failed: {err:#}");
                    return;
                }
            }
        }
    }

    /// Marks this node as present while its lease lives. The leader waits for the present nodes
    /// before the ACME server validates a challenge.
    async fn register(&self, lease: &Lease) -> anyhow::Result<()> {
        let node = &self.storage.cluster_config().node_name;
        let key = self.storage.layout().node(node);
        let lease = Some(lease.id.clone());
        let put = self.storage.put_unconditional(key, node.as_bytes(), lease);
        answer_within(self.interval(), put).await
    }

    async fn try_lock(&self, lease: &Lease) -> anyhow::Result<bool> {
        let store = self.storage.store();
        let key = self.storage.layout().leader();
        let node = &self.storage.cluster_config().node_name;
        let value = self.storage.encode(&key, node.as_bytes())?;
        let locked = async {
            store.keep_alive(lease).await?;
            store.try_lock(&key, &value, lease).await
        };
        answer_within(self.interval(), locked).await
    }

    /// Tells the server about a change of the leadership. False when the server stopped.
    async fn set_leader(&mut self, leader: bool) -> bool {
        if self.leader == leader {
            return !self.command.is_closed();
        }
        self.leader = leader;
        info!(leader, "the leadership of this node changed");
        let command = ServerCommand::SetLeader { leader };
        self.command.send(command).await.is_ok()
    }
}
