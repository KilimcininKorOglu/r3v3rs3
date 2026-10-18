//! Configuration backup and restore models.
//!
//! A single file contains the entire r3v3rs3 configuration (settings, ports, proxies, access lists,
//! and ACME entries) with secrets masked and accounts excluded.

use crate::access_list::AccessListEntry;
use crate::acme::Acme;
use crate::app::AppConfig;
use crate::id::ShortId;
use crate::port::PortEntry;
use crate::proxy::ProxyEntry;
use serde_derive::{Deserialize, Serialize};
use std::collections::BTreeMap;
use utoipa::ToSchema;

/// An ACME configuration entry in a backup.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize, Serialize, ToSchema)]
pub struct AcmeBackupEntry {
    pub id: ShortId,
    #[serde(flatten)]
    pub acme: Acme,
}

/// A complete r3v3rs3 configuration backup.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize, Serialize, ToSchema)]
pub struct ConfigBackup {
    /// The r3v3rs3 package version when the backup was created.
    #[serde(default = "default_version")]
    pub version: String,
    /// Unix timestamp in seconds when the backup was created.
    #[serde(default)]
    pub created_at: u64,
    /// The application configuration with webhook tokens masked.
    pub config: AppConfig,
    /// The configured ports.
    #[serde(default)]
    pub ports: Vec<PortEntry>,
    /// The manual proxies with authentication credentials masked.
    #[serde(default)]
    pub proxies: Vec<ProxyEntry>,
    /// The access lists with password hashes and token digests masked.
    #[serde(default)]
    pub access_lists: Vec<AccessListEntry>,
    /// The ACME certificate management entries.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub acme: Vec<AcmeBackupEntry>,
    /// Raw storage files (e.g. config.toml, ports.toml) with accounts excluded and secrets masked.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub files: BTreeMap<String, String>,
}

fn default_version() -> String {
    env!("CARGO_PKG_VERSION").to_string()
}

impl ConfigBackup {
    /// A human-readable summary of the items in the backup.
    pub fn summary(&self) -> String {
        format!(
            "{} ports, {} proxies, {} access lists, {} acme entries",
            self.ports.len(),
            self.proxies.len(),
            self.access_lists.len(),
            self.acme.len()
        )
    }
}
