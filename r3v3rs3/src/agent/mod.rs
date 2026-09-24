//! The agent link of the deployment platform: a remote server runs `r3v3rs3 agent`, connects to
//! the agent port of the master over mTLS, and runs the containers of its target.

pub mod pki;
pub mod token;
