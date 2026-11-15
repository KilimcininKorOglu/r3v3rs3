//! The connected agents of the master: one session for each agent target.

use super::link::Link;
use r3v3rs3_api::id::ShortId;
use r3v3rs3_api::platform::AgentLinkQuality;
use std::collections::HashMap;
use std::sync::{Mutex, MutexGuard};
use tokio::task::AbortHandle;

const MAX_SAMPLES: usize = 15;

struct Session {
    link: Link,
    version: String,
    last_seen_at: u64,
    /// Tells a session apart from a later session of the same target.
    generation: u64,
    task: AbortHandle,
    samples: Vec<f64>,
    attempts: Vec<bool>,
}

fn calculate_jitter(samples: &[f64]) -> f64 {
    if samples.len() < 2 {
        return 0.0;
    }
    let mut sum = 0.0;
    for window in samples.windows(2) {
        sum += (window[1] - window[0]).abs();
    }
    (sum / (samples.len() - 1) as f64 * 10.0).round() / 10.0
}

fn calculate_loss(attempts: &[bool]) -> f64 {
    if attempts.is_empty() {
        return 0.0;
    }
    let failed = attempts.iter().filter(|&&ok| !ok).count();
    let loss = (failed as f64 / attempts.len() as f64) * 100.0;
    (loss * 10.0).round() / 10.0
}

fn determine_quality(latency: f64, jitter: f64, loss: f64) -> AgentLinkQuality {
    if loss > 25.0 || latency > 350.0 || jitter > 80.0 {
        AgentLinkQuality::Poor
    } else if loss > 10.0 || latency > 150.0 || jitter > 40.0 {
        AgentLinkQuality::Degraded
    } else if loss > 2.0 || latency > 50.0 || jitter > 15.0 {
        AgentLinkQuality::Good
    } else {
        AgentLinkQuality::Excellent
    }
}

impl Session {
    fn new(
        link: Link,
        version: String,
        last_seen_at: u64,
        generation: u64,
        task: AbortHandle,
    ) -> Self {
        Self {
            link,
            version,
            last_seen_at,
            generation,
            task,
            samples: Vec::new(),
            attempts: Vec::new(),
        }
    }

    fn record_ping(&mut self, latency_ms: f64, now: u64) {
        self.last_seen_at = now;
        let rounded = (latency_ms * 10.0).round() / 10.0;
        self.samples.push(rounded);
        if self.samples.len() > MAX_SAMPLES {
            self.samples.remove(0);
        }
        self.attempts.push(true);
        if self.attempts.len() > MAX_SAMPLES {
            self.attempts.remove(0);
        }
    }

    fn record_ping_failure(&mut self) {
        self.attempts.push(false);
        if self.attempts.len() > MAX_SAMPLES {
            self.attempts.remove(0);
        }
    }

    fn status(&self) -> AgentStatus {
        let latency_ms = self.samples.last().copied();
        let jitter_ms = if self.samples.len() >= 2 {
            Some(calculate_jitter(&self.samples))
        } else {
            Some(0.0)
        };
        let loss = calculate_loss(&self.attempts);
        let quality = match latency_ms {
            Some(lat) => determine_quality(lat, jitter_ms.unwrap_or(0.0), loss),
            None => AgentLinkQuality::Offline,
        };
        AgentStatus {
            version: self.version.clone(),
            last_seen_at: self.last_seen_at,
            latency_ms,
            jitter_ms,
            packet_loss_percent: Some(loss),
            quality,
            samples: self.samples.clone(),
        }
    }
}

/// The state of a connected agent.
#[derive(Debug, Clone, PartialEq)]
pub struct AgentStatus {
    pub version: String,
    pub last_seen_at: u64,
    pub latency_ms: Option<f64>,
    pub jitter_ms: Option<f64>,
    pub packet_loss_percent: Option<f64>,
    pub quality: AgentLinkQuality,
    pub samples: Vec<f64>,
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
    /// the new session, and whether it replaced a session, also one whose link already closed.
    /// A replaced session ends without a `remove` that returns true, so the caller reports the
    /// target online only when nothing was replaced.
    pub fn insert(
        &self,
        target: ShortId,
        link: Link,
        version: String,
        task: AbortHandle,
        now: u64,
    ) -> (u64, bool) {
        let generation = {
            let mut generations = self
                .generations
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner());
            *generations += 1;
            *generations
        };
        let session = Session::new(link, version, now, generation, task);
        let previous = self.sessions().insert(target, session);
        let replaced = previous.is_some();
        if let Some(previous) = previous {
            previous.task.abort();
        }
        (generation, replaced)
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
            .map(|session| session.status())
    }

    /// Records a successful ping of the agent in the session of `generation`.
    pub fn record_ping(&self, target: ShortId, generation: u64, latency_ms: f64, now: u64) {
        if let Some(session) = self.sessions().get_mut(&target)
            && session.generation == generation
        {
            session.record_ping(latency_ms, now);
        }
    }

    /// Records a failed ping of the agent in the session of `generation`.
    pub fn record_ping_failure(&self, target: ShortId, generation: u64) {
        if let Some(session) = self.sessions().get_mut(&target)
            && session.generation == generation
        {
            session.record_ping_failure();
        }
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

#[cfg(test)]
mod tests {
    use super::*;
    use tokio_util::compat::TokioAsyncReadCompatExt;

    /// A link over a pipe whose other end is dropped, so the link closes.
    async fn closed_link() -> (Link, AbortHandle) {
        let (master, agent) = tokio::io::duplex(64);
        drop(agent);
        let (link, task) = Link::spawn(master.compat());
        let handle = task.abort_handle();
        let _ = task.await;
        (link, handle)
    }

    #[tokio::test]
    async fn a_session_replaces_one_whose_link_closed_already() {
        let registry = AgentRegistry::default();
        let target: ShortId = "bcd-fgh".parse().unwrap();
        let (link, task) = closed_link().await;
        let (first, replaced) = registry.insert(target, link, "1".into(), task, 1);
        assert!(!replaced);
        // The first link closed, but its session still waits for its own removal.
        assert!(registry.status(target).is_none());
        let (link, task) = closed_link().await;
        let (second, replaced) = registry.insert(target, link, "1".into(), task, 2);
        assert!(replaced);
        assert!(!registry.remove(target, first));
        assert!(registry.remove(target, second));
    }

    #[test]
    fn jitter_and_packet_loss_calculation() {
        assert_eq!(calculate_jitter(&[]), 0.0);
        assert_eq!(calculate_jitter(&[10.0]), 0.0);
        // differences: |15-10| = 5, |12-15| = 3. avg: 4.0
        assert_eq!(calculate_jitter(&[10.0, 15.0, 12.0]), 4.0);

        assert_eq!(calculate_loss(&[]), 0.0);
        assert_eq!(calculate_loss(&[true, true]), 0.0);
        assert_eq!(calculate_loss(&[true, false]), 50.0);
    }

    #[test]
    fn link_quality_thresholds() {
        assert_eq!(
            determine_quality(15.0, 2.0, 0.0),
            AgentLinkQuality::Excellent
        );
        assert_eq!(determine_quality(60.0, 5.0, 0.0), AgentLinkQuality::Good);
        assert_eq!(
            determine_quality(160.0, 10.0, 0.0),
            AgentLinkQuality::Degraded
        );
        assert_eq!(determine_quality(400.0, 10.0, 0.0), AgentLinkQuality::Poor);
        assert_eq!(determine_quality(20.0, 2.0, 30.0), AgentLinkQuality::Poor);
    }

    #[tokio::test]
    async fn ping_records_update_status() {
        let (master, _agent) = tokio::io::duplex(64);
        let (link, task) = Link::spawn(master.compat());
        let registry = AgentRegistry::default();
        let target: ShortId = "bcd-fgh".parse().unwrap();
        let (generation, _) = registry.insert(target, link, "1.0".into(), task.abort_handle(), 100);

        registry.record_ping(target, generation, 25.4, 101);
        registry.record_ping(target, generation, 28.6, 102);

        let status = registry.status(target).expect("agent status");
        assert_eq!(status.version, "1.0");
        assert_eq!(status.last_seen_at, 102);
        assert_eq!(status.latency_ms, Some(28.6));
        assert_eq!(status.jitter_ms, Some(3.2));
        assert_eq!(status.packet_loss_percent, Some(0.0));
        assert_eq!(status.quality, AgentLinkQuality::Excellent);
        assert_eq!(status.samples, vec![25.4, 28.6]);
    }
}
