//! The connected agents of the master: one session for each agent target.

use super::link::Link;
use r3v3rs3_api::id::ShortId;
use std::collections::HashMap;
use std::sync::{Mutex, MutexGuard};
use tokio::task::AbortHandle;

struct Session {
    link: Link,
    version: String,
    last_seen_at: u64,
    /// Tells a session apart from a later session of the same target.
    generation: u64,
    task: AbortHandle,
}

/// The state of a connected agent.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AgentStatus {
    pub version: String,
    pub last_seen_at: u64,
}

#[derive(Default)]
pub struct AgentRegistry {
    sessions: Mutex<HashMap<ShortId, Session>>,
    generations: Mutex<u64>,
}

impl AgentRegistry {
    fn sessions(&self) -> MutexGuard<'_, HashMap<ShortId, Session>> {
        // The map stays valid when a holder panics.
        self.sessions
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
    }

    /// Adds the session of an agent and closes its previous session. Returns the generation of
    /// the new session.
    pub fn insert(
        &self,
        target: ShortId,
        link: Link,
        version: String,
        task: AbortHandle,
        now: u64,
    ) -> u64 {
        let generation = {
            let mut generations = self
                .generations
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner());
            *generations += 1;
            *generations
        };
        let session = Session {
            link,
            version,
            last_seen_at: now,
            generation,
            task,
        };
        if let Some(previous) = self.sessions().insert(target, session) {
            previous.task.abort();
        }
        generation
    }

    /// The link of a connected agent.
    pub fn link(&self, target: ShortId) -> Option<Link> {
        self.sessions()
            .get(&target)
            .map(|session| session.link.clone())
            .filter(|link| !link.is_closed())
    }

    pub fn status(&self, target: ShortId) -> Option<AgentStatus> {
        self.sessions()
            .get(&target)
            .filter(|session| !session.link.is_closed())
            .map(|session| AgentStatus {
                version: session.version.clone(),
                last_seen_at: session.last_seen_at,
            })
    }

    /// Records an answer of the agent in the session of `generation`.
    pub fn seen(&self, target: ShortId, generation: u64, now: u64) {
        if let Some(session) = self.sessions().get_mut(&target)
            && session.generation == generation
        {
            session.last_seen_at = now;
        }
    }

    /// Closes the session of `generation`. A later session of the same target stays. Returns
    /// whether the session of `generation` was still the session of the target.
    pub fn remove(&self, target: ShortId, generation: u64) -> bool {
        let mut sessions = self.sessions();
        if sessions
            .get(&target)
            .is_some_and(|session| session.generation == generation)
            && let Some(session) = sessions.remove(&target)
        {
            session.task.abort();
            return true;
        }
        false
    }

    /// Closes the session of a target, for example after its deletion.
    pub fn disconnect(&self, target: ShortId) {
        if let Some(session) = self.sessions().remove(&target) {
            session.task.abort();
        }
    }
}
