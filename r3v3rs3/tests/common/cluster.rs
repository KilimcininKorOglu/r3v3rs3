//! Servers that share their state in a cluster store.

use super::{call, wait_for_rpc};
use r3v3rs3::cluster;
use r3v3rs3::cluster::storage::KvStorage;
use r3v3rs3::config::new_appinfo;
use r3v3rs3::server::rpc::RpcMethod;
use r3v3rs3::server::{Server, ServerChannels};
use r3v3rs3_api::event::ServerEvent;
use std::path::Path;
use std::sync::Arc;
use tokio::sync::broadcast;
use tokio::task::JoinHandle;

/// A server that follows the cluster store and takes part in the leader election.
pub struct Node {
    pub channels: ServerChannels,
    task: JoinHandle<anyhow::Result<()>>,
}

impl Node {
    pub async fn start(storage: KvStorage) -> anyhow::Result<Self> {
        let storage = Arc::new(storage);
        let app_info = new_appinfo(Path::new("."), Path::new("."));
        let (server, channels) = Server::new_shared(app_info, storage.clone()).await;
        cluster::spawn_tasks(storage, channels.command.clone());
        let task = tokio::spawn(server.start());
        Ok(Self { channels, task })
    }

    pub async fn call<M: RpcMethod + 'static>(&mut self, method: M) -> anyhow::Result<M::Output> {
        Ok(call(&mut self.channels, method).await??)
    }

    pub async fn wait_for<M>(
        &mut self,
        method: impl Fn() -> M,
        done: impl Fn(&M::Output) -> bool,
    ) -> anyhow::Result<M::Output>
    where
        M: RpcMethod + 'static,
        M::Output: std::fmt::Debug,
    {
        wait_for_rpc(&mut self.channels, method, done).await
    }

    pub async fn stop(self) -> anyhow::Result<()> {
        stop_server(&self.channels.event, self.task).await
    }
}

pub async fn stop_server(
    event: &broadcast::Sender<ServerEvent>,
    task: JoinHandle<anyhow::Result<()>>,
) -> anyhow::Result<()> {
    event.send(ServerEvent::Shutdown)?;
    task.await?
}
