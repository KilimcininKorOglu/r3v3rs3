use super::state::ServerState;
use crate::accounts::{Caller, Permission};
use crate::audit::AuditRecord;
use r3v3rs3_api::error::Error;
use r3v3rs3_api::id::ShortId;
use std::any::Any;

pub mod access_lists;
pub mod accounts;
pub mod acme;
pub mod auth;
pub mod certs;
pub mod cluster;
pub mod config;
pub mod discovery;
pub mod ports;
pub mod proxies;

#[async_trait::async_trait]
pub trait RpcMethod: Any + Send + Sync {
    type Output: Any + Send + Sync;
    /// Whether the method changes the stored state. Such a method runs only when the storage can
    /// write.
    const MUTATES: bool = false;
    /// What the caller needs to run the method. A method that changes the stored state needs
    /// `Edit` unless it sets another permission.
    const PERMISSION: Permission = if Self::MUTATES {
        Permission::Edit
    } else {
        Permission::Read
    };
    async fn call(self, state: &mut ServerState) -> Result<Self::Output, Error>;

    /// The proxy that the method reads or changes. The method fails with `IdNotFound` for a caller
    /// that does not see the proxy.
    fn proxy_scope(&self) -> Option<ShortId> {
        None
    }

    /// The audit log entry of a successful call. A method that changes nothing has none.
    fn audit(&self) -> Option<AuditRecord> {
        None
    }

    /// Removes the parts of the output that the caller does not see.
    fn restrict(output: Self::Output, _caller: &Caller) -> Self::Output {
        output
    }
}

pub struct RpcWrapper<T: RpcMethod> {
    inner: Option<T>,
}

impl<T> RpcWrapper<T>
where
    T: RpcMethod,
{
    pub fn new(inner: T) -> Self {
        Self { inner: Some(inner) }
    }
}

#[async_trait::async_trait]
impl<T> ErasedRpcMethod for RpcWrapper<T>
where
    T: RpcMethod,
{
    async fn call(
        &mut self,
        state: &mut ServerState,
        caller: &Caller,
    ) -> Result<Box<dyn Any + Send + Sync>, Error> {
        let this = self.inner.take().ok_or(Error::FailedToInvokeRpc)?;
        <T as RpcMethod>::call(this, state)
            .await
            .map(|output| Box::new(T::restrict(output, caller)) as Box<dyn Any + Send + Sync>)
    }

    fn mutates(&self) -> bool {
        T::MUTATES
    }

    fn permission(&self) -> Permission {
        T::PERMISSION
    }

    fn proxy_scope(&self) -> Option<ShortId> {
        self.inner.as_ref().and_then(RpcMethod::proxy_scope)
    }

    fn audit(&self) -> Option<AuditRecord> {
        self.inner.as_ref().and_then(RpcMethod::audit)
    }
}

#[async_trait::async_trait]
pub trait ErasedRpcMethod: Any + Send + Sync {
    async fn call(
        &mut self,
        state: &mut ServerState,
        caller: &Caller,
    ) -> Result<Box<dyn Any + Send + Sync>, Error>;

    fn mutates(&self) -> bool;

    fn permission(&self) -> Permission;

    fn proxy_scope(&self) -> Option<ShortId>;

    fn audit(&self) -> Option<AuditRecord>;
}

pub struct RpcCallback {
    pub id: usize,
    pub result: Result<Box<dyn Any + Send + Sync>, Error>,
}
