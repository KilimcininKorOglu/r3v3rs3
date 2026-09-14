use axum::{
    http::{HeaderMap, Method, StatusCode, Uri},
    Router,
};
use r3v3rs3::certs::dns::{self, DnsClient, TxtName};
use r3v3rs3_api::acme::{DnsProvider, KeyedProvider, TokenApi, TokenProvider};
use serde_json::json;
use std::sync::{Arc, Mutex};

mod common;
use common::serve_http_upstream;

/// A request that a mock provider API received.
#[derive(Debug, Clone)]
struct Call {
    method: String,
    uri: String,
    authorization: String,
    body: String,
}

type Calls = Arc<Mutex<Vec<Call>>>;
type Respond = fn(&Call) -> (StatusCode, String);

/// Starts a mock provider API that records every request and answers with `respond`.
async fn start_mock(respond: Respond) -> anyhow::Result<(String, Calls)> {
    let calls = Calls::default();
    let recorded = calls.clone();
    let app = Router::new().fallback(
        move |method: Method, uri: Uri, headers: HeaderMap, body: String| {
            let calls = recorded.clone();
            async move {
                let call = Call {
                    method: method.to_string(),
                    uri: uri.to_string(),
                    authorization: headers
                        .get("authorization")
                        .and_then(|value| value.to_str().ok())
                        .unwrap_or_default()
                        .to_string(),
                    body,
                };
                let response = respond(&call);
                calls.lock().unwrap().push(call);
                response
            }
        },
    );
    let url = serve_http_upstream(app).await?;
    Ok((url.as_str().trim_end_matches('/').to_string(), calls))
}

fn challenge_name() -> TxtName {
    TxtName {
        fqdn: "_acme-challenge.app.example.test".to_string(),
        values: vec!["v1".to_string(), "v2".to_string()],
    }
}

fn requests(calls: &Calls) -> Vec<String> {
    calls
        .lock()
        .unwrap()
        .iter()
        .map(|call| format!("{} {}", call.method, call.uri))
        .collect()
}

fn body(calls: &Calls, index: usize) -> anyhow::Result<serde_json::Value> {
    Ok(serde_json::from_str(&calls.lock().unwrap()[index].body)?)
}

/// Starts a mock provider API with `respond`, and returns a client of the provider that
/// `provider` builds for the mock URL.
async fn mock_client(
    respond: Respond,
    provider: impl FnOnce(String) -> DnsProvider,
) -> anyhow::Result<(Box<dyn DnsClient>, Calls)> {
    let (url, calls) = start_mock(respond).await?;
    Ok((dns::client(&provider(url)).await?, calls))
}

/// Adds and removes the challenge name through a mock provider API.
async fn add_and_remove(
    respond: Respond,
    provider: impl FnOnce(String) -> DnsProvider,
) -> anyhow::Result<Calls> {
    let (client, calls) = mock_client(respond, provider).await?;
    let record = client.add_txt(&challenge_name()).await?;
    client.remove_txt(&record).await?;
    Ok(calls)
}

/// A token provider that uses the mock URL.
fn token_provider(provider: TokenApi, token: &str) -> impl FnOnce(String) -> DnsProvider + '_ {
    move |url| {
        DnsProvider::Token(TokenProvider {
            provider,
            api_token: token.to_string(),
            api_url: Some(url),
        })
    }
}

fn cloudflare_with(token: &str) -> impl FnOnce(String) -> DnsProvider + '_ {
    token_provider(TokenApi::Cloudflare, token)
}

fn ok(body: &str) -> (StatusCode, String) {
    (StatusCode::OK, body.to_string())
}

fn cloudflare(call: &Call) -> (StatusCode, String) {
    match (call.method.as_str(), call.uri.as_str()) {
        ("GET", "/zones?name=example.test") => ok(r#"{"result":[{"id":"zone-1"}]}"#),
        ("GET", _) => ok(r#"{"result":[]}"#),
        ("POST", _) if call.body.contains("\"v2\"") => ok(r#"{"result":{"id":"rec-2"}}"#),
        ("POST", _) => ok(r#"{"result":{"id":"rec-1"}}"#),
        _ => ok(r#"{"result":{}}"#),
    }
}

#[tokio::test]
async fn cloudflare_adds_each_value_and_deletes_it_by_id() -> anyhow::Result<()> {
    let calls = add_and_remove(cloudflare, cloudflare_with("cf-token")).await?;

    assert_eq!(
        requests(&calls),
        vec![
            "GET /zones?name=app.example.test",
            "GET /zones?name=example.test",
            "POST /zones/zone-1/dns_records",
            "POST /zones/zone-1/dns_records",
            "DELETE /zones/zone-1/dns_records/rec-1",
            "DELETE /zones/zone-1/dns_records/rec-2",
        ]
    );
    assert!(calls
        .lock()
        .unwrap()
        .iter()
        .all(|call| call.authorization == "Bearer cf-token"));
    assert_eq!(
        body(&calls, 2)?,
        json!({ "type": "TXT", "name": "_acme-challenge.app.example.test", "content": "v1", "ttl": 60 })
    );
    Ok(())
}

fn cloudflare_second_value_fails(call: &Call) -> (StatusCode, String) {
    if call.method == "POST" && call.body.contains("\"v2\"") {
        return (StatusCode::INTERNAL_SERVER_ERROR, "{}".to_string());
    }
    cloudflare(call)
}

#[tokio::test]
async fn a_failed_value_removes_the_values_added_before_it() -> anyhow::Result<()> {
    let (client, calls) =
        mock_client(cloudflare_second_value_fails, cloudflare_with("cf-token")).await?;

    assert!(client.add_txt(&challenge_name()).await.is_err());
    assert_eq!(
        requests(&calls).last().map(String::as_str),
        Some("DELETE /zones/zone-1/dns_records/rec-1")
    );
    Ok(())
}

fn unauthorized(_: &Call) -> (StatusCode, String) {
    (
        StatusCode::UNAUTHORIZED,
        r#"{"success":false,"errors":[{"code":10000,"message":"Authentication error"}]}"#
            .to_string(),
    )
}

#[tokio::test]
async fn a_provider_error_names_the_status_but_not_the_token() -> anyhow::Result<()> {
    let (client, _) = mock_client(unauthorized, cloudflare_with("cf-secret-token")).await?;

    let Err(err) = client.add_txt(&challenge_name()).await else {
        anyhow::bail!("add_txt succeeded");
    };
    let message = format!("{err:#}");
    assert!(message.contains("401"), "{message}");
    assert!(message.contains("Authentication error"), "{message}");
    assert!(!message.contains("cf-secret-token"), "{message}");
    Ok(())
}

fn digitalocean(call: &Call) -> (StatusCode, String) {
    match (call.method.as_str(), call.uri.as_str()) {
        ("GET", "/domains?per_page=200&page=1") => {
            ok(r#"{"domains":[{"name":"other.test"},{"name":"example.test"}]}"#)
        }
        ("POST", _) if call.body.contains("\"v2\"") => (
            StatusCode::CREATED,
            r#"{"domain_record":{"id":12}}"#.to_string(),
        ),
        ("POST", _) => (
            StatusCode::CREATED,
            r#"{"domain_record":{"id":11}}"#.to_string(),
        ),
        _ => (StatusCode::NO_CONTENT, String::new()),
    }
}

#[tokio::test]
async fn digitalocean_uses_the_name_relative_to_the_domain() -> anyhow::Result<()> {
    let calls = add_and_remove(
        digitalocean,
        token_provider(TokenApi::DigitalOcean, "do-token"),
    )
    .await?;

    assert_eq!(
        requests(&calls),
        vec![
            "GET /domains?per_page=200&page=1",
            "POST /domains/example.test/records",
            "POST /domains/example.test/records",
            "DELETE /domains/example.test/records/11",
            "DELETE /domains/example.test/records/12",
        ]
    );
    assert_eq!(
        body(&calls, 1)?,
        json!({ "type": "TXT", "name": "_acme-challenge.app", "data": "v1", "ttl": 60 })
    );
    assert_eq!(calls.lock().unwrap()[0].authorization, "Bearer do-token");
    Ok(())
}

fn hetzner(call: &Call) -> (StatusCode, String) {
    match (call.method.as_str(), call.uri.as_str()) {
        ("GET", "/zones?name=example.test") => ok(r#"{"zones":[{"id":42,"name":"example.test"}]}"#),
        ("GET", _) => ok(r#"{"zones":[]}"#),
        _ => (StatusCode::CREATED, r#"{"action":{"id":1}}"#.to_string()),
    }
}

#[tokio::test]
async fn hetzner_adds_and_removes_both_values_in_one_rrset() -> anyhow::Result<()> {
    let calls = add_and_remove(hetzner, token_provider(TokenApi::Hetzner, "hz-token")).await?;

    assert_eq!(
        requests(&calls),
        vec![
            "GET /zones?name=app.example.test",
            "GET /zones?name=example.test",
            "POST /zones/42/rrsets/_acme-challenge.app/TXT/actions/add_records",
            "POST /zones/42/rrsets/_acme-challenge.app/TXT/actions/remove_records",
        ]
    );
    let records = json!([{ "value": "\"v1\"" }, { "value": "\"v2\"" }]);
    assert_eq!(body(&calls, 2)?, json!({ "ttl": 60, "records": records }));
    assert_eq!(body(&calls, 3)?, json!({ "records": records }));
    assert_eq!(calls.lock().unwrap()[2].authorization, "Bearer hz-token");
    Ok(())
}

fn route53(call: &Call) -> (StatusCode, String) {
    if call.method == "GET" {
        return ok(concat!(
            r#"<?xml version="1.0" encoding="UTF-8"?>"#,
            "<ListHostedZonesResponse><HostedZones>",
            "<HostedZone><Id>/hostedzone/ZPRIVATE</Id><Name>example.test.</Name>",
            "<Config><PrivateZone>true</PrivateZone></Config></HostedZone>",
            "<HostedZone><Id>/hostedzone/ZPUBLIC</Id><Name>example.test.</Name>",
            "<Config><PrivateZone>false</PrivateZone></Config></HostedZone>",
            "</HostedZones><IsTruncated>false</IsTruncated><MaxItems>100</MaxItems>",
            "</ListHostedZonesResponse>"
        ));
    }
    ok("<ChangeResourceRecordSetsResponse><ChangeInfo><Id>/change/C1</Id><Status>PENDING</Status></ChangeInfo></ChangeResourceRecordSetsResponse>")
}

#[tokio::test]
async fn route53_upserts_and_deletes_one_record_set_in_the_public_zone() -> anyhow::Result<()> {
    let calls = add_and_remove(route53, |url| {
        DnsProvider::Keyed(KeyedProvider::Route53 {
            access_key_id: "AKIDTEST".to_string(),
            secret_access_key: "route53-secret".to_string(),
            api_url: Some(url),
        })
    })
    .await?;

    assert_eq!(
        requests(&calls),
        vec![
            "GET /2013-04-01/hostedzone",
            "POST /2013-04-01/hostedzone/ZPUBLIC/rrset/",
            "POST /2013-04-01/hostedzone/ZPUBLIC/rrset/",
        ]
    );
    let calls = calls.lock().unwrap();
    for call in calls.iter() {
        assert!(
            call.authorization
                .starts_with("AWS4-HMAC-SHA256 Credential=AKIDTEST/"),
            "{}",
            call.authorization
        );
        assert!(call.authorization.contains(
            "/us-east-1/route53/aws4_request, SignedHeaders=host;x-amz-date, Signature="
        ));
        assert!(!call.authorization.contains("route53-secret"));
    }
    let values = "<ResourceRecord><Value>\"v1\"</Value></ResourceRecord><ResourceRecord><Value>\"v2\"</Value></ResourceRecord>";
    assert!(calls[1].body.contains("<Action>UPSERT</Action>"));
    assert!(calls[1].body.contains(values));
    assert!(calls[2].body.contains("<Action>DELETE</Action>"));
    assert!(calls[2].body.contains(values));
    Ok(())
}
