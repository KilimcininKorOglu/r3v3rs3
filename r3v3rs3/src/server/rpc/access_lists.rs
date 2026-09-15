use super::RpcMethod;
use crate::audit::AuditRecord;
use crate::server::credentials::{mask_policy, restore_policy, seal_blocking, seal_policy};
use crate::server::state::ServerState;
use r3v3rs3_api::access_list::{AccessList, AccessListEntry};
use r3v3rs3_api::audit::AuditAction;
use r3v3rs3_api::error::Error;
use r3v3rs3_api::id::ShortId;

pub struct GetAccessLists;

#[async_trait::async_trait]
impl RpcMethod for GetAccessLists {
    type Output = Vec<AccessListEntry>;

    async fn call(self, state: &mut ServerState) -> Result<Self::Output, Error> {
        Ok(state.access_lists.iter().cloned().map(masked).collect())
    }
}

pub struct AddAccessList {
    pub list: AccessList,
}

#[async_trait::async_trait]
impl RpcMethod for AddAccessList {
    type Output = ();
    const MUTATES: bool = true;

    async fn call(self, state: &mut ServerState) -> Result<Self::Output, Error> {
        let list = sealed(self.list).await?;
        let mut lists = state.access_lists.clone();
        lists.push((state.generate_id(), list).into());
        state.commit_access_lists(lists).await
    }

    /// The server adds the generated id to the entry.
    fn audit(&self) -> Option<AuditRecord> {
        Some(AuditRecord::new(AuditAction::AddAccessList).summary(self.list.name.clone()))
    }
}

pub struct UpdateAccessList {
    pub entry: AccessListEntry,
}

#[async_trait::async_trait]
impl RpcMethod for UpdateAccessList {
    type Output = ();
    const MUTATES: bool = true;

    /// A user or a token without a new secret keeps its current hash.
    async fn call(self, state: &mut ServerState) -> Result<Self::Output, Error> {
        let mut lists = state.access_lists.clone();
        let current = lists
            .iter_mut()
            .find(|current| current.id == self.entry.id)
            .ok_or_else(|| not_found(self.entry.id))?;
        let mut list = self.entry.list;
        restore_policy(&mut list.auth, &mut current.list.auth);
        current.list = sealed(list).await?;
        state.commit_access_lists(lists).await
    }

    fn audit(&self) -> Option<AuditRecord> {
        let record = AuditRecord::new(AuditAction::UpdateAccessList).id(self.entry.id);
        Some(record.summary(self.entry.list.name.clone()))
    }
}

pub struct DeleteAccessList {
    pub id: ShortId,
}

#[async_trait::async_trait]
impl RpcMethod for DeleteAccessList {
    type Output = ();
    const MUTATES: bool = true;

    async fn call(self, state: &mut ServerState) -> Result<Self::Output, Error> {
        let in_use = state
            .proxies
            .entries()
            .any(|entry| entry.proxy.kind.access_lists().contains(&self.id));
        if in_use {
            return Err(Error::AccessListInUse { id: self.id });
        }
        let mut lists = state.access_lists.clone();
        lists.retain(|list| list.id != self.id);
        if lists.len() == state.access_lists.len() {
            return Err(not_found(self.id));
        }
        state.commit_access_lists(lists).await
    }

    fn audit(&self) -> Option<AuditRecord> {
        Some(AuditRecord::new(AuditAction::DeleteAccessList).id(self.id))
    }
}

/// Checks the name, and replaces the plain text secrets of the list with hashes.
async fn sealed(list: AccessList) -> Result<AccessList, Error> {
    if list.name.trim().is_empty() {
        return Err(Error::AccessListNameRequired);
    }
    seal_blocking(list, |list| seal_policy(&mut list.auth)).await
}

/// The entry without the password hashes and the token digests.
fn masked(mut entry: AccessListEntry) -> AccessListEntry {
    mask_policy(&mut entry.list.auth);
    entry
}

fn not_found(id: ShortId) -> Error {
    Error::IdNotFound { id: id.to_string() }
}
