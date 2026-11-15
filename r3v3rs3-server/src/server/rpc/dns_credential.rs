use super::RpcMethod;
use crate::{audit::AuditRecord, server::state::ServerState};
use r3v3rs3_api::{
    acme::{DnsCredentialInfo, DnsCredentialRequest, DnsProvider},
    audit::AuditAction,
    error::Error,
    id::ShortId,
};

pub struct GetDnsCredentialList;

#[async_trait::async_trait]
impl RpcMethod for GetDnsCredentialList {
    type Output = Vec<DnsCredentialInfo>;

    async fn call(self, state: &mut ServerState) -> Result<Self::Output, Error> {
        credentials(state).await
    }
}

pub struct GetDnsCredential {
    pub id: ShortId,
}

#[async_trait::async_trait]
impl RpcMethod for GetDnsCredential {
    type Output = DnsCredentialInfo;

    async fn call(self, state: &mut ServerState) -> Result<Self::Output, Error> {
        find(state, self.id).await
    }
}

/// The credential with the id, or the not-found error that names the id.
async fn find(state: &ServerState, id: ShortId) -> Result<DnsCredentialInfo, Error> {
    credentials(state)
        .await?
        .into_iter()
        .find(|credential| credential.id == id)
        .ok_or_else(|| Error::DnsCredentialNotFound {
            name: id.to_string(),
        })
}

async fn credentials(state: &ServerState) -> Result<Vec<DnsCredentialInfo>, Error> {
    state.storage.load_dns_credentials().await
}

/// The stored provider of the credential, for an update that keeps the secrets.
async fn stored_provider(state: &ServerState, id: ShortId) -> Result<DnsProvider, Error> {
    state
        .storage
        .dns_provider_of(id)
        .await?
        .ok_or_else(|| Error::DnsCredentialNotFound {
            name: id.to_string(),
        })
}

pub struct AddDnsCredential {
    pub request: DnsCredentialRequest,
}

#[async_trait::async_trait]
impl RpcMethod for AddDnsCredential {
    type Output = DnsCredentialInfo;
    const MUTATES: bool = true;

    async fn call(self, state: &mut ServerState) -> Result<Self::Output, Error> {
        let provider = provider_of(&self.request)?;
        let name = self.request.name.trim().to_string();
        if credentials(state)
            .await?
            .iter()
            .any(|known| known.name == name)
        {
            return Err(Error::DnsCredentialNameTaken { name });
        }
        let id = state.generate_id();
        state
            .storage
            .save_dns_credential(id, &name, &provider)
            .await?;
        Ok(DnsCredentialInfo {
            id,
            name,
            provider: provider.name().to_string(),
        })
    }

    fn audit(&self) -> Option<AuditRecord> {
        Some(AuditRecord::new(AuditAction::AddDnsCredential).summary(self.request.name.clone()))
    }
}

pub struct UpdateDnsCredential {
    pub id: ShortId,
    pub request: DnsCredentialRequest,
}

#[async_trait::async_trait]
impl RpcMethod for UpdateDnsCredential {
    type Output = ();
    const MUTATES: bool = true;

    async fn call(self, state: &mut ServerState) -> Result<Self::Output, Error> {
        find(state, self.id).await?;
        let name = self.request.name.trim().to_string();
        if credentials(state)
            .await?
            .iter()
            .any(|other| other.id != self.id && other.name == name)
        {
            return Err(Error::DnsCredentialNameTaken { name });
        }
        let provider = match self.request.dns_provider {
            // An update without credentials keeps the stored ones, because the panel never
            // shows a secret again.
            None => stored_provider(state, self.id).await?,
            Some(provider) => {
                if !provider.has_credentials() {
                    return Err(Error::AcmeDnsProviderRequired);
                }
                provider.validate()?;
                provider
            }
        };
        state
            .storage
            .save_dns_credential(self.id, &name, &provider)
            .await
    }

    fn audit(&self) -> Option<AuditRecord> {
        Some(AuditRecord::new(AuditAction::UpdateDnsCredential).id(self.id))
    }
}

pub struct DeleteDnsCredential {
    pub id: ShortId,
}

#[async_trait::async_trait]
impl RpcMethod for DeleteDnsCredential {
    type Output = ();
    const MUTATES: bool = true;

    async fn call(self, state: &mut ServerState) -> Result<Self::Output, Error> {
        let known = find(state, self.id).await?;
        if state
            .acmes
            .entries()
            .any(|acme| acme.acme.dns_credential == Some(self.id))
        {
            return Err(Error::DnsCredentialInUse { name: known.name });
        }
        state.storage.delete_dns_credential(self.id).await
    }

    fn audit(&self) -> Option<AuditRecord> {
        Some(AuditRecord::new(AuditAction::DeleteDnsCredential).id(self.id))
    }
}

/// The provider of a create request. A create needs the credentials.
fn provider_of(request: &DnsCredentialRequest) -> Result<DnsProvider, Error> {
    let provider = request
        .dns_provider
        .as_ref()
        .ok_or(Error::AcmeDnsProviderRequired)?;
    if !provider.has_credentials() {
        return Err(Error::AcmeDnsProviderRequired);
    }
    provider.validate()?;
    Ok(provider.clone())
}
