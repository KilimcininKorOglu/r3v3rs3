//! An agent in memory for the unit tests: its link runs over a pipe, and its requests run on a
//! fake runtime.

use super::client::VERSION;
use super::executor::Executor;
use super::link::{Link, serve};
use super::registry::AgentRegistry;
use crate::platform::fake::FakeRuntime;
use r3v3rs3_api::id::ShortId;
use std::sync::Arc;
use std::time::Duration;
use tokio::task::JoinHandle;
use tokio_util::compat::TokioAsyncReadCompatExt;

pub struct TestAgent {
    pub runtime: Arc<FakeRuntime>,
    link: Link,
    task: JoinHandle<()>,
}

impl TestAgent {
    /// Connects a new agent to `target` in `registry`.
    pub fn connect(registry: &AgentRegistry, target: ShortId) -> Self {
        let runtime = Arc::new(FakeRuntime::default());
        let executor = Arc::new(Executor::new(runtime.clone()));
        let (master_io, agent_io) = tokio::io::duplex(1 << 20);
        let task = tokio::spawn(async move {
            let _ = serve(agent_io.compat(), move |stream| {
                let executor = executor.clone();
                tokio::spawn(async move { executor.answer(stream).await });
            })
            .await;
        });
        let (link, link_task) = Link::spawn(master_io.compat());
        registry.insert(
            target,
            link.clone(),
            VERSION.to_string(),
            link_task.abort_handle(),
            1,
        );
        Self {
            runtime,
            link,
            task,
        }
    }

    /// Ends the connection of the agent and waits until the master sees it closed.
    pub async fn stop(&self) {
        self.task.abort();
        for _ in 0..200 {
            if self.link.is_closed() {
                return;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
        panic!("the link of the test agent did not close");
    }
}

impl Drop for TestAgent {
    fn drop(&mut self) {
        self.task.abort();
    }
}
