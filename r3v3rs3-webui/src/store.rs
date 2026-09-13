use r3v3rs3_api::{
    acme::AcmeInfo,
    cert::CertInfo,
    discovery::DiscoveryStatus,
    id::ShortId,
    port::{PortEntry, PortStatus},
    proxy::{ProxyEntry, ProxyStatus},
};
use std::collections::HashMap;
use yewdux::prelude::*;

#[derive(Default, Clone, PartialEq, Store)]
pub struct PortStore {
    pub entries: Vec<PortEntry>,
    pub statuses: HashMap<ShortId, PortStatus>,
    pub loaded: bool,
}

#[derive(Default, Clone, PartialEq, Store)]
pub struct ProxyStore {
    pub entries: Vec<ProxyEntry>,
    pub statuses: HashMap<ShortId, ProxyStatus>,
    pub loaded: bool,
}

#[derive(Default, Clone, PartialEq, Store)]
pub struct CertStore {
    pub entries: Vec<CertInfo>,
    pub loaded: bool,
}

#[derive(Default, Clone, PartialEq, Store)]
pub struct AcmeStore {
    pub entries: Vec<AcmeInfo>,
    pub loaded: bool,
}

#[derive(Default, Clone, PartialEq, Store)]
pub struct DiscoveryStore {
    pub entries: Vec<DiscoveryStatus>,
}
