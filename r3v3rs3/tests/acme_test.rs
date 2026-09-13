use r3v3rs3::{
    command::ServerCommand,
    server::rpc::{acme::AddAcme, ErasedRpcMethod, RpcWrapper},
};
use r3v3rs3_api::{
    acme::{Acme, AcmeConfig, AcmeRequest, HTTP_01},
    error::Error,
};

mod common;
use common::{with_server, TestStorage};

#[tokio::test]
async fn a_wildcard_request_with_http_01_is_rejected_before_the_account_is_created(
) -> anyhow::Result<()> {
    let request = AcmeRequest {
        // Nothing listens here, so reaching account creation fails with a different error.
        server_url: "http://127.0.0.1:1/directory".to_string(),
        contacts: vec![],
        eab: None,
        acme: Acme {
            config: AcmeConfig::default(),
            identifiers: vec!["example.com".parse()?, "*.example.com".parse()?],
            challenge_type: HTTP_01.to_string(),
            dns_provider: None,
        },
    };

    let storage = TestStorage::builder().build();
    with_server(storage, |mut channels| async move {
        let arg = Box::new(RpcWrapper::new(AddAcme { request })) as Box<dyn ErasedRpcMethod>;
        channels
            .command
            .send(ServerCommand::CallMethod { id: 1, arg })
            .await?;
        let callback = channels
            .callback
            .recv()
            .await
            .ok_or_else(|| anyhow::anyhow!("callback channel closed"))?;
        assert!(matches!(
            callback.result,
            Err(Error::AcmeWildcardNeedsDnsChallenge { .. })
        ));
        Ok(())
    })
    .await
}
