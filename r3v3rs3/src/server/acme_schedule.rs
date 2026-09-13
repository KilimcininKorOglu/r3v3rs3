use r3v3rs3_api::id::ShortId;
use std::{
    collections::{HashMap, HashSet},
    time::{Duration, Instant, SystemTime},
};

/// Time to wait before an entry whose order failed is ordered again.
pub const RETRY_AFTER_FAILURE: Duration = Duration::from_secs(60 * 60);

/// Decides when an ACME entry gets a new order, so each entry has at most one order at a time.
#[derive(Debug, Default)]
pub struct AcmeSchedule {
    in_progress: HashSet<ShortId>,
    retry_after: HashMap<ShortId, Instant>,
}

impl AcmeSchedule {
    pub fn is_due(&self, id: ShortId, next_renewal: Option<SystemTime>, now: Instant) -> bool {
        if self.in_progress.contains(&id) {
            return false;
        }
        if self.retry_after.get(&id).is_some_and(|at| now < *at) {
            return false;
        }
        next_renewal.is_none_or(|next| next.elapsed().is_ok())
    }

    pub fn start(&mut self, id: ShortId) {
        self.in_progress.insert(id);
    }

    pub fn finish(&mut self, id: ShortId, succeeded: bool, now: Instant) {
        self.in_progress.remove(&id);
        if succeeded {
            self.retry_after.remove(&id);
        } else {
            self.retry_after.insert(id, now + RETRY_AFTER_FAILURE);
        }
    }

    pub fn is_idle(&self) -> bool {
        self.in_progress.is_empty()
    }
}

#[cfg(test)]
mod test {
    use super::*;

    fn id() -> ShortId {
        "abc".parse().unwrap()
    }

    #[test]
    fn an_entry_with_a_running_order_is_not_due() {
        let mut schedule = AcmeSchedule::default();
        let now = Instant::now();
        assert!(schedule.is_due(id(), None, now));

        schedule.start(id());
        assert!(!schedule.is_due(id(), None, now));
        assert!(!schedule.is_idle());

        schedule.finish(id(), true, now);
        assert!(schedule.is_due(id(), None, now));
        assert!(schedule.is_idle());
    }

    #[test]
    fn a_failed_order_waits_before_the_next_order() {
        let mut schedule = AcmeSchedule::default();
        let now = Instant::now();
        schedule.start(id());
        schedule.finish(id(), false, now);

        assert!(!schedule.is_due(id(), None, now));
        assert!(schedule.is_due(id(), None, now + RETRY_AFTER_FAILURE));
    }

    #[test]
    fn an_entry_is_due_only_after_its_renewal_time() {
        let schedule = AcmeSchedule::default();
        let now = Instant::now();
        let hour = Duration::from_secs(3600);
        assert!(!schedule.is_due(id(), Some(SystemTime::now() + hour), now));
        assert!(schedule.is_due(id(), Some(SystemTime::now() - hour), now));
    }
}
