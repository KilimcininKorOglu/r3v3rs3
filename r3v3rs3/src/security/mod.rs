//! Security features: threat detection, auto-quarantine, and rate control.

pub mod quarantine;
pub use quarantine::AutoQuarantineTracker;
