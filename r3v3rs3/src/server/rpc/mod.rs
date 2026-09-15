use super::state::ServerState;
use r3v3rs3_api::error::Error;
use std::any::Any;

pub mod acme;
pub mod auth;
pub mod certs;
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
    async fn call(self, state: &mut ServerState) -> Result<Self::Output, Error>;
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
    async fn call(&mut self, state: &mut ServerState) -> Result<Box<dyn Any + Send + Sync>, Error> {
        let this = self.inner.take().ok_or(Error::FailedToInvokeRpc)?;
        <T as RpcMethod>::call(this, state)
            .await
            .map(|r| Box::new(r) as Box<dyn Any + Send + Sync>)
    }

    fn mutates(&self) -> bool {
        T::MUTATES
    }
}

#[async_trait::async_trait]
pub trait ErasedRpcMethod: Any + Send + Sync {
    async fn call(&mut self, state: &mut ServerState) -> Result<Box<dyn Any + Send + Sync>, Error>;

    fn mutates(&self) -> bool;
}

pub struct RpcCallback {
    pub id: usize,
    pub result: Result<Box<dyn Any + Send + Sync>, Error>,
}
