//! The r3v3rs3 server. The modules live in the `r3v3rs3-*` crates, which compile in parallel;
//! this crate re-exports all of them for the `r3-server` binary and the integration tests.

pub use r3v3rs3_admin::*;
