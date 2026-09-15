use super::RpcMethod;
use crate::{
    certs::{acme::AcmeEntry, dns},
    server::state::ServerState,
};
use r3v3rs3_api::{
    acme::{AcmeConfig, AcmeInfo, AcmeRequest},
    error::Error,
    id::ShortId,
};

pub struct GetAcmeList;

#[async_trait::async_trait]
impl RpcMethod for GetAcmeList {
    type Output = Vec<AcmeInfo>;

    async fn call(self, state: &mut ServerState) -> Result<Self::Output, Error> {
        Ok(state
            .acmes
            .entries()
            .map(|acme| acme.info(&state.certs))
            .collect())
    }
}

pub struct GetAcme {
    pub id: ShortId,
}

#[async_trait::async_trait]
impl RpcMethod for GetAcme {
    type Output = AcmeInfo;

    async fn call(self, state: &mut ServerState) -> Result<Self::Output, Error> {
        get_entry(state, self.id).map(|acme| acme.info(&state.certs))
    }
}

fn get_entry(state: &ServerState, id: ShortId) -> Result<&AcmeEntry, Error> {
    state
        .acmes
        .get(id)
        .ok_or_else(|| Error::IdNotFound { id: id.to_string() })
}

pub struct AddAcme {
    pub request: AcmeRequest,
}

#[async_trait::async_trait]
impl RpcMethod for AddAcme {
    type Output = ();
    const MUTATES: bool = true;

    async fn call(self, state: &mut ServerState) -> Result<Self::Output, Error> {
        self.request.acme.validate()?;
        dns::check_exec_provider(
            self.request.acme.dns_provider.as_ref(),
            &state.config().acme_exec,
        )?;
        let entry = AcmeEntry::new(state.generate_id(), self.request).await?;
        state.storage.save_acme(&entry).await?;
        state.acmes.add(entry)?;
        state.update_acmes().await;
        Ok(())
    }
}

pub struct UpdateAcme {
    pub id: ShortId,
    pub config: AcmeConfig,
}

#[async_trait::async_trait]
impl RpcMethod for UpdateAcme {
    type Output = ();
    const MUTATES: bool = true;

    async fn call(self, state: &mut ServerState) -> Result<Self::Output, Error> {
        let mut entry = get_entry(state, self.id)?.clone();
        entry.acme.config = self.config;
        state.storage.save_acme(&entry).await?;
        state.acmes.update(self.id, entry.acme.config)?;
        state.update_acmes().await;
        Ok(())
    }
}

pub struct DeleteAcme {
    pub id: ShortId,
}

#[async_trait::async_trait]
impl RpcMethod for DeleteAcme {
    type Output = ();
    const MUTATES: bool = true;

    async fn call(self, state: &mut ServerState) -> Result<Self::Output, Error> {
        get_entry(state, self.id)?;
        state.storage.delete_acme(self.id).await?;
        state.acmes.delete(self.id)?;
        state.update_acmes().await;
        Ok(())
    }
}
