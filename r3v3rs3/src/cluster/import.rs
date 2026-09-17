//! Copies the files of a node into an empty cluster store.

use super::storage::KvStorage;
use crate::config::file::{FileState, FileStorage};
use crate::config::storage::Storage;
use std::path::Path;

#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub struct ImportReport {
    pub ports: usize,
    pub proxies: usize,
    pub access_lists: usize,
    pub certs: usize,
    pub acmes: usize,
    pub accounts: usize,
}

/// Imports the files of `config_dir` into the store that its `config.toml` names.
pub async fn run(config_dir: &Path) -> anyhow::Result<ImportReport> {
    let state = FileStorage::new(config_dir).read_state().await?;
    let storage = KvStorage::connect(state.config.clone()).await?;
    import(&storage, state).await
}

/// Writes the state, then the schema key. A store that has data is an error.
pub async fn import(storage: &KvStorage, state: FileState) -> anyhow::Result<ImportReport> {
    ensure_empty(storage).await?;
    storage.save_app_config(&state.config).await?;
    storage.save_ports(&state.ports).await?;
    storage.save_proxies(&state.proxies).await?;
    storage.save_access_lists(&state.access_lists).await?;
    let certs = state
        .certs
        .iter()
        .filter(|cert| cert.source.is_none())
        .collect::<Vec<_>>();
    import_entries(storage, &state, &certs).await?;
    storage.write_schema().await?;
    Ok(ImportReport {
        ports: state.ports.len(),
        proxies: state.proxies.len(),
        access_lists: state.access_lists.len(),
        certs: certs.len(),
        acmes: state.acmes.len(),
        accounts: state.accounts.len(),
    })
}

/// Writes the entries that the store keeps under one key each: the certificates, the ACME
/// accounts, the accounts and the CDN ranges.
async fn import_entries(
    storage: &KvStorage,
    state: &FileState,
    certs: &[&std::sync::Arc<crate::certs::Cert>],
) -> anyhow::Result<()> {
    for cert in certs {
        storage.save_cert(cert).await?;
    }
    for acme in &state.acmes {
        storage.save_acme(acme).await?;
    }
    for (name, account) in &state.accounts {
        storage.put_account(name, account).await?;
    }
    if let Some(ranges) = &state.cdn_ranges {
        storage.save_cdn_ranges(ranges).await?;
    }
    Ok(())
}

async fn ensure_empty(storage: &KvStorage) -> anyhow::Result<()> {
    let data = storage.layout().data();
    anyhow::ensure!(
        !storage.is_imported().await?,
        "the cluster store already has data below {data}"
    );
    let state = storage.store().list(&storage.layout().state()).await?;
    anyhow::ensure!(
        state.items.is_empty(),
        "the cluster store has {} keys below {data} from an import that did not finish. Delete them before the import.",
        state.items.len()
    );
    Ok(())
}
