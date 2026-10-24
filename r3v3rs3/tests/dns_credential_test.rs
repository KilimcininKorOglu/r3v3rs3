use r3v3rs3::{
    accounts::Caller,
    command::ServerCommand,
    config::storage::Storage,
    server::rpc::{
        ErasedRpcMethod, RpcWrapper,
        dns_credential::{AddDnsCredential, DeleteDnsCredential, GetDnsCredentialList},
    },
};
use r3v3rs3_api::{
    acme::{
        Acme, AcmeConfig, DnsCredentialInfo, DnsCredentialRequest, DnsProvider, TokenApi,
        TokenProvider,
    },
    error::Error,
    id::ShortId,
};

mod common;
use common::{TestStorage, with_server};

/// A Cloudflare provider whose API is a test server.
fn provider(token: &str) -> DnsProvider {
    DnsProvider::Token(TokenProvider {
        provider: TokenApi::Cloudflare,
        api_token: token.to_string(),
        api_url: None,
    })
}

/// Sends one RPC method to a server and returns its typed result.
async fn call<O: Send + 'static>(
    storage: TestStorage,
    method: Box<dyn ErasedRpcMethod>,
) -> anyhow::Result<Result<O, Error>> {
    let (sender, receiver) = tokio::sync::oneshot::channel();
    with_server(storage, move |mut channels| async move {
        channels
            .command
            .send(ServerCommand::CallMethod {
                id: 1,
                arg: method,
                caller: Caller::system(),
            })
            .await?;
        let callback = channels
            .callback
            .recv()
            .await
            .ok_or_else(|| anyhow::anyhow!("callback channel closed"))?;
        // An error of the RPC method is the result that the test inspects, so it must not
        // end the server task here.
        let payload = match callback.result {
            Ok(any) => Ok(any
                .downcast::<O>()
                .map(|boxed| *boxed)
                .expect("unexpected output type")),
            Err(err) => Err(err),
        };
        let _ = sender.send(payload);
        Ok(())
    })
    .await?;
    Ok(receiver.await?)
}

/// The ACME entry that the storage starts with, as `dns-01` with a credential reference.
fn acme_entry(credential: ShortId) -> anyhow::Result<r3v3rs3::certs::acme::AcmeEntry> {
    use base64::Engine as _;
    use std::sync::Arc;
    let key = rcgen::KeyPair::generate()?;
    let account = serde_json::from_value(serde_json::json!({
        "id": "https://acme.example/acct/1",
        "key_pkcs8": base64::engine::general_purpose::URL_SAFE_NO_PAD
            .encode(key.serialize_der()),
        "directory": "https://acme.example/directory",
    }))?;
    Ok(r3v3rs3::certs::acme::AcmeEntry {
        id: "crd001".parse()?,
        acme: Acme {
            config: AcmeConfig::default(),
            identifiers: vec!["*.example.com".parse()?],
            challenge_type: r3v3rs3_api::acme::DNS_01.to_string(),
            dns_provider: None,
            dns_credential: Some(credential),
            dns_alias: None,
        },
        account: Arc::new(account),
    })
}

#[tokio::test]
async fn a_credential_is_created_without_its_secret_in_the_list() -> anyhow::Result<()> {
    let storage = TestStorage::builder().build();
    let created = call::<DnsCredentialInfo>(
        storage.clone(),
        Box::new(RpcWrapper::new(AddDnsCredential {
            request: DnsCredentialRequest {
                name: "Cloudflare - test".to_string(),
                dns_provider: Some(provider("token-one")),
            },
        })),
    )
    .await??;
    assert_eq!(created.name, "Cloudflare - test");
    assert_eq!(created.provider, "cloudflare");

    let list = call::<Vec<DnsCredentialInfo>>(
        storage.clone(),
        Box::new(RpcWrapper::new(GetDnsCredentialList)),
    )
    .await??;
    assert_eq!(list, vec![created]);

    let again = call::<DnsCredentialInfo>(
        storage,
        Box::new(RpcWrapper::new(AddDnsCredential {
            request: DnsCredentialRequest {
                name: " Cloudflare - test ".to_string(),
                dns_provider: Some(provider("token-two")),
            },
        })),
    )
    .await?;
    assert!(matches!(
        again,
        Err(Error::DnsCredentialNameTaken { name }) if name == "Cloudflare - test"
    ));
    Ok(())
}

#[tokio::test]
async fn a_credential_without_credentials_is_rejected() -> anyhow::Result<()> {
    let result = call::<DnsCredentialInfo>(
        TestStorage::builder().build(),
        Box::new(RpcWrapper::new(AddDnsCredential {
            request: DnsCredentialRequest {
                name: "empty".to_string(),
                dns_provider: Some(provider("")),
            },
        })),
    )
    .await?;
    assert!(matches!(result, Err(Error::AcmeDnsProviderRequired)));
    Ok(())
}

#[tokio::test]
async fn a_credential_of_an_acme_entry_is_not_deleted() -> anyhow::Result<()> {
    let storage = TestStorage::builder().build();
    let id: ShortId = "crd002".parse()?;
    storage
        .save_dns_credential(id, "in use", &provider("token"))
        .await?;
    storage.save_acme(&acme_entry(id)?).await?;

    let deleted = call::<()>(
        storage.clone(),
        Box::new(RpcWrapper::new(DeleteDnsCredential { id })),
    )
    .await?;
    assert!(matches!(
        deleted,
        Err(Error::DnsCredentialInUse { name }) if name == "in use"
    ));

    // After the ACME entry no longer holds the reference, the delete succeeds.
    let free: ShortId = "crd003".parse()?;
    storage
        .save_dns_credential(free, "free", &provider("token"))
        .await?;
    let deleted = call::<()>(
        storage.clone(),
        Box::new(RpcWrapper::new(DeleteDnsCredential { id: free })),
    )
    .await?;
    assert!(deleted.is_ok());
    let unknown = call::<()>(
        storage,
        Box::new(RpcWrapper::new(DeleteDnsCredential { id: free })),
    )
    .await?;
    assert!(matches!(
        unknown,
        Err(Error::DnsCredentialNotFound { name }) if name == free.to_string()
    ));
    Ok(())
}
