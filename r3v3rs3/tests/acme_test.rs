use r3v3rs3::{
    accounts::Caller,
    command::ServerCommand,
    server::rpc::{acme::AddAcme, ErasedRpcMethod, RpcWrapper},
};
use r3v3rs3_api::{
    acme::{Acme, AcmeConfig, AcmeRequest, DnsProvider, LocalProvider, DNS_01, HTTP_01},
    error::Error,
    subject_name::SubjectName,
};

mod common;
use common::{with_server, TestStorage};

/// A request whose account creation fails, so a validation error is the only other result.
fn request(
    identifiers: Vec<SubjectName>,
    challenge_type: &str,
    dns_provider: Option<DnsProvider>,
) -> AcmeRequest {
    AcmeRequest {
        // Nothing listens here, so reaching account creation fails with a different error.
        server_url: "http://127.0.0.1:1/directory".to_string(),
        contacts: vec![],
        eab: None,
        acme: Acme {
            config: AcmeConfig::default(),
            identifiers,
            challenge_type: challenge_type.to_string(),
            dns_provider,
        },
    }
}

/// Sends the request to a server and returns the result of the call.
async fn add_acme(request: AcmeRequest) -> anyhow::Result<Result<(), Error>> {
    let storage = TestStorage::builder().build();
    let (sender, receiver) = tokio::sync::oneshot::channel();
    with_server(storage, |mut channels| async move {
        let arg = Box::new(RpcWrapper::new(AddAcme { request })) as Box<dyn ErasedRpcMethod>;
        channels
            .command
            .send(ServerCommand::CallMethod {
                id: 1,
                arg,
                caller: Caller::system(),
            })
            .await?;
        let callback = channels
            .callback
            .recv()
            .await
            .ok_or_else(|| anyhow::anyhow!("callback channel closed"))?;
        let _ = sender.send(callback.result.map(|_| ()));
        Ok(())
    })
    .await?;
    Ok(receiver.await?)
}

#[tokio::test]
async fn a_wildcard_request_with_http_01_is_rejected_before_the_account_is_created(
) -> anyhow::Result<()> {
    let identifiers = vec!["example.com".parse()?, "*.example.com".parse()?];
    let result = add_acme(request(identifiers, HTTP_01, None)).await?;
    assert!(matches!(
        result,
        Err(Error::AcmeWildcardNeedsDnsChallenge { .. })
    ));
    Ok(())
}

#[tokio::test]
async fn an_exec_program_outside_the_allowlist_is_rejected_before_the_account_is_created(
) -> anyhow::Result<()> {
    let provider = DnsProvider::Local(LocalProvider::Exec {
        program: "/bin/sh".to_string(),
    });
    let result = add_acme(request(
        vec!["example.com".parse()?],
        DNS_01,
        Some(provider),
    ))
    .await?;
    assert!(matches!(
        result,
        Err(Error::AcmeExecProgramNotAllowed { program }) if program == "/bin/sh"
    ));
    Ok(())
}
