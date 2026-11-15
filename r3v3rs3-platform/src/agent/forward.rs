//! The loopback forwarders of the master. Each published port of a container on an agent gets a
//! listener on `127.0.0.1` of the master, and each connection to it travels through a tunnel
//! stream to the port on the agent host. The proxy and the health check reach an app on an agent
//! as they reach a local app.

use super::protocol::AgentRequest;
use super::registry::AgentRegistry;
use anyhow::anyhow;
use r3v3rs3_api::container::ContainerName;
use r3v3rs3_api::id::ShortId;
use std::collections::HashMap;
use std::sync::{Arc, Mutex, MutexGuard};
use tokio::net::{TcpListener, TcpStream};
use tokio::task::AbortHandle;
use tracing::debug;

/// A published port of a container on an agent.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct ForwardKey {
    pub target: ShortId,
    pub container: ContainerName,
    /// The port of the container.
    pub port: u16,
}

struct Forward {
    /// The port of the listener on `127.0.0.1`.
    port: u16,
    task: AbortHandle,
}

pub struct Forwarders {
    agents: Arc<AgentRegistry>,
    forwards: Mutex<HashMap<ForwardKey, Forward>>,
}

impl Forwarders {
    pub fn new(agents: Arc<AgentRegistry>) -> Self {
        Self {
            agents,
            forwards: Mutex::default(),
        }
    }

    fn forwards(&self) -> MutexGuard<'_, HashMap<ForwardKey, Forward>> {
        // The map stays valid when a holder panics.
        self.forwards
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
    }

    /// The port of the listener of `key`, which opens the listener at the first call.
    pub async fn port(&self, key: &ForwardKey) -> anyhow::Result<u16> {
        if let Some(port) = self.existing(key) {
            return Ok(port);
        }
        let listener = TcpListener::bind(("127.0.0.1", 0)).await?;
        let port = listener.local_addr()?.port();
        let task = tokio::spawn(accept(listener, key.clone(), self.agents.clone()));
        let mut forwards = self.forwards();
        // Another call can open a listener for the same key at the same time; the first stays.
        if let Some(forward) = forwards.get(key) {
            task.abort();
            return Ok(forward.port);
        }
        let task = task.abort_handle();
        forwards.insert(key.clone(), Forward { port, task });
        Ok(port)
    }

    /// The port of an open listener.
    pub fn existing(&self, key: &ForwardKey) -> Option<u16> {
        self.forwards().get(key).map(|forward| forward.port)
    }

    /// Closes the listeners of a removed container.
    pub fn close_container(&self, target: ShortId, container: &ContainerName) {
        self.forwards().retain(|key, forward| {
            let keep = key.target != target || key.container != *container;
            if !keep {
                forward.task.abort();
            }
            keep
        });
    }

    /// Closes the listeners of a deleted target.
    pub fn close_target(&self, target: ShortId) {
        self.forwards().retain(|key, forward| {
            let keep = key.target != target;
            if !keep {
                forward.task.abort();
            }
            keep
        });
    }
}

impl Drop for Forwarders {
    fn drop(&mut self) {
        for forward in self.forwards().values() {
            forward.task.abort();
        }
    }
}

async fn accept(listener: TcpListener, key: ForwardKey, agents: Arc<AgentRegistry>) {
    loop {
        match listener.accept().await {
            Ok((tcp, _)) => {
                tokio::spawn(forward(tcp, key.clone(), agents.clone()));
            }
            Err(err) => {
                debug!(%err, "a forwarder failed to accept a connection");
                // An error such as the limit of open files would repeat at once.
                tokio::time::sleep(std::time::Duration::from_millis(100)).await;
            }
        }
    }
}

/// Copies one connection through a tunnel. An offline agent or a stopped container closes the
/// connection, so the proxy answers 502.
async fn forward(mut tcp: TcpStream, key: ForwardKey, agents: Arc<AgentRegistry>) {
    let result = async {
        let link = agents
            .link(key.target)
            .ok_or_else(|| anyhow!("the agent of {} is not connected", key.target))?;
        let request = AgentRequest::Tunnel {
            container: key.container.clone(),
            port: key.port,
        };
        let mut stream = link.tunnel(&request).await?;
        tokio::io::copy_bidirectional(&mut tcp, &mut stream).await?;
        anyhow::Ok(())
    }
    .await;
    if let Err(err) = result {
        debug!(target = %key.target, container = %key.container, "a tunnel ended: {err:#}");
    }
}
