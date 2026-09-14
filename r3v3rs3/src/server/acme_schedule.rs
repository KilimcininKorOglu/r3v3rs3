use crate::certs::acme::AcmeTarget;
use std::{
    collections::{HashMap, HashSet},
    time::{Duration, Instant, SystemTime},
};

/// Time to wait before a target whose order failed is ordered again.
pub const RETRY_AFTER_FAILURE: Duration = Duration::from_secs(60 * 60);

/// Decides when an ACME target gets a new order, so each target has at most one order at a time.
#[derive(Debug, Default)]
pub struct AcmeSchedule {
    in_progress: HashSet<AcmeTarget>,
    retry_after: HashMap<AcmeTarget, Instant>,
}

impl AcmeSchedule {
    pub fn is_due(
        &self,
        target: &AcmeTarget,
        next_renewal: Option<SystemTime>,
        now: Instant,
    ) -> bool {
        if self.in_progress.contains(target) {
            return false;
        }
        if self.retry_after.get(target).is_some_and(|at| now < *at) {
            return false;
        }
        next_renewal.is_none_or(|next| next.elapsed().is_ok())
    }

    pub fn start(&mut self, target: AcmeTarget) {
        self.in_progress.insert(target);
    }

    pub fn finish(&mut self, target: &AcmeTarget, succeeded: bool, now: Instant) {
        self.in_progress.remove(target);
        if succeeded {
            self.retry_after.remove(target);
        } else {
            self.retry_after
                .insert(target.clone(), now + RETRY_AFTER_FAILURE);
        }
    }

    pub fn is_idle(&self) -> bool {
        self.in_progress.is_empty()
    }
}

#[cfg(test)]
mod test {
    use super::*;

    fn target() -> AcmeTarget {
        AcmeTarget::new("abc".parse().unwrap(), ["example.com"])
    }

    #[test]
    fn a_target_with_a_running_order_is_not_due() {
        let mut schedule = AcmeSchedule::default();
        let now = Instant::now();
        assert!(schedule.is_due(&target(), None, now));

        schedule.start(target());
        assert!(!schedule.is_due(&target(), None, now));
        assert!(!schedule.is_idle());

        // Another target of the same entry has its own order.
        let other = AcmeTarget::new("abc".parse().unwrap(), ["app.example.com"]);
        assert!(schedule.is_due(&other, None, now));

        schedule.finish(&target(), true, now);
        assert!(schedule.is_due(&target(), None, now));
        assert!(schedule.is_idle());
    }

    #[test]
    fn a_failed_order_waits_before_the_next_order() {
        let mut schedule = AcmeSchedule::default();
        let now = Instant::now();
        schedule.start(target());
        schedule.finish(&target(), false, now);

        assert!(!schedule.is_due(&target(), None, now));
        assert!(schedule.is_due(&target(), None, now + RETRY_AFTER_FAILURE));
    }

    #[test]
    fn a_target_is_due_only_after_its_renewal_time() {
        let schedule = AcmeSchedule::default();
        let now = Instant::now();
        let hour = Duration::from_secs(3600);
        assert!(!schedule.is_due(&target(), Some(SystemTime::now() + hour), now));
        assert!(schedule.is_due(&target(), Some(SystemTime::now() - hour), now));
    }
}
