//! Security features: threat detection, auto-quarantine, and rate control.

pub mod ban;
pub mod login_attempts;
pub mod quarantine;
pub use quarantine::AutoQuarantineTracker;
