//! The agent link of the deployment platform: a remote server runs `r3v3rs3 agent`, connects to
//! the agent port of the master over mTLS, and runs the containers of its target.

pub mod client;
pub mod compose;
pub mod executor;
pub mod forward;
pub mod frame;
pub mod link;
pub mod listener;
pub mod pki;
pub mod protocol;
pub mod registry;
pub mod remote;
pub mod token;

#[cfg(test)]
pub(crate) mod testing;
#[cfg(test)]
mod tests;
