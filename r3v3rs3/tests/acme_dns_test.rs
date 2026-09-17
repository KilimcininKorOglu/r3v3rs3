use axum::{
    Router,
    http::{HeaderMap, Method, StatusCode, Uri},
};
use base64::{Engine, engine::general_purpose::URL_SAFE_NO_PAD};
use r3v3rs3::certs::dns::{self, DnsClient, TxtName};
use r3v3rs3_api::{
    acme::{
        CloudProvider, DnsProvider, KeyedProvider, LocalProvider, OvhEndpoint, TokenApi,
        TokenProvider,
    },
    app::AcmeExecConfig,
    error::Error,
};
use ring::signature::{RSA_PKCS1_2048_8192_SHA256, UnparsedPublicKey};
use rsa::{
    pkcs1::EncodeRsaPublicKey,
    pkcs8::{EncodePrivateKey, LineEnding},
};
use serde_json::json;
use sha1::{Digest, Sha1};
use std::{
    collections::HashMap,
    path::{Path, PathBuf},
    sync::{Arc, Mutex},
    time::Duration,
};

mod common;
use common::serve_http_upstream;

/// A request that a mock provider API received.
#[derive(Debug, Clone)]
struct Call {
    method: String,
    uri: String,
    authorization: String,
    headers: HeaderMap,
    body: String,
}

impl Call {
    fn header(&self, name: &str) -> &str {
        self.headers
            .get(name)
            .and_then(|value| value.to_str().ok())
            .unwrap_or_default()
    }
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
                    headers,
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
    let client = dns::client(&provider(url), &AcmeExecConfig::default()).await?;
    Ok((client, calls))
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
    assert!(
        calls
            .lock()
            .unwrap()
            .iter()
            .all(|call| call.authorization == "Bearer cf-token")
    );
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

fn linode(call: &Call) -> (StatusCode, String) {
    match (call.method.as_str(), call.uri.as_str()) {
        ("GET", "/domains?page=1&page_size=500") => {
            ok(r#"{"data":[{"id":7,"domain":"other.test"}],"page":1,"pages":2}"#)
        }
        ("GET", "/domains?page=2&page_size=500") => {
            ok(r#"{"data":[{"id":8,"domain":"example.test"}],"page":2,"pages":2}"#)
        }
        ("POST", _) if call.body.contains("\"v2\"") => ok(r#"{"id":22}"#),
        ("POST", _) => ok(r#"{"id":21}"#),
        _ => ok("{}"),
    }
}

#[tokio::test]
async fn linode_reads_every_domain_page_and_uses_its_lowest_ttl() -> anyhow::Result<()> {
    let calls = add_and_remove(linode, token_provider(TokenApi::Linode, "ln-token")).await?;

    assert_eq!(
        requests(&calls),
        vec![
            "GET /domains?page=1&page_size=500",
            "GET /domains?page=2&page_size=500",
            "POST /domains/8/records",
            "POST /domains/8/records",
            "DELETE /domains/8/records/21",
            "DELETE /domains/8/records/22",
        ]
    );
    assert_eq!(
        body(&calls, 2)?,
        json!({ "type": "TXT", "name": "_acme-challenge.app.example.test", "target": "v1", "ttl_sec": 300 })
    );
    assert_eq!(calls.lock().unwrap()[2].authorization, "Bearer ln-token");
    Ok(())
}

fn vultr(call: &Call) -> (StatusCode, String) {
    match (call.method.as_str(), call.uri.as_str()) {
        ("GET", "/domains?per_page=100") => ok(
            r#"{"domains":[{"domain":"other.test"}],"meta":{"total":2,"links":{"next":"b2Zmc2V0PTE=","prev":""}}}"#,
        ),
        ("GET", "/domains?per_page=100&cursor=b2Zmc2V0PTE%3D") => ok(
            r#"{"domains":[{"domain":"example.test"}],"meta":{"total":2,"links":{"next":"","prev":""}}}"#,
        ),
        ("POST", _) if call.body.contains("v2") => (
            StatusCode::CREATED,
            r#"{"record":{"id":"rec-b"}}"#.to_string(),
        ),
        ("POST", _) => (
            StatusCode::CREATED,
            r#"{"record":{"id":"rec-a"}}"#.to_string(),
        ),
        _ => (StatusCode::NO_CONTENT, String::new()),
    }
}

#[tokio::test]
async fn vultr_follows_the_cursor_and_quotes_the_txt_data() -> anyhow::Result<()> {
    let calls = add_and_remove(vultr, token_provider(TokenApi::Vultr, "vu-token")).await?;

    assert_eq!(
        requests(&calls),
        vec![
            "GET /domains?per_page=100",
            "GET /domains?per_page=100&cursor=b2Zmc2V0PTE%3D",
            "POST /domains/example.test/records",
            "POST /domains/example.test/records",
            "DELETE /domains/example.test/records/rec-a",
            "DELETE /domains/example.test/records/rec-b",
        ]
    );
    assert_eq!(
        body(&calls, 2)?,
        json!({ "type": "TXT", "name": "_acme-challenge.app", "data": "\"v1\"", "ttl": 60 })
    );
    assert_eq!(calls.lock().unwrap()[2].authorization, "Bearer vu-token");
    Ok(())
}

fn gandi(call: &Call) -> (StatusCode, String) {
    match (call.method.as_str(), call.uri.as_str()) {
        ("GET", "/domains?per_page=100&page=1") => {
            ok(r#"[{"fqdn":"other.test"},{"fqdn":"example.test"}]"#)
        }
        // Another tool keeps its own value in the record set, and v1 is already there.
        ("GET", "/domains/example.test/records/_acme-challenge.app/TXT") => ok(
            r#"{"rrset_name":"_acme-challenge.app","rrset_type":"TXT","rrset_ttl":300,"rrset_values":["\"other\"","\"v1\""]}"#,
        ),
        _ => (
            StatusCode::CREATED,
            r#"{"message":"DNS Record Created"}"#.to_string(),
        ),
    }
}

#[tokio::test]
async fn gandi_merges_the_values_and_keeps_the_values_of_other_tools() -> anyhow::Result<()> {
    let calls = add_and_remove(gandi, token_provider(TokenApi::Gandi, "gd-token")).await?;

    let record = "/domains/example.test/records/_acme-challenge.app/TXT";
    assert_eq!(
        requests(&calls),
        vec![
            "GET /domains?per_page=100&page=1".to_string(),
            format!("GET {record}"),
            format!("PUT {record}"),
            format!("GET {record}"),
            format!("PUT {record}"),
        ]
    );
    assert_eq!(
        body(&calls, 2)?,
        json!({ "rrset_ttl": 300, "rrset_values": ["\"other\"", "\"v1\"", "\"v2\""] })
    );
    assert_eq!(
        body(&calls, 4)?,
        json!({ "rrset_ttl": 300, "rrset_values": ["\"other\""] })
    );
    assert!(
        calls
            .lock()
            .unwrap()
            .iter()
            .all(|call| call.authorization == "Bearer gd-token")
    );
    Ok(())
}

fn desec_zone(call: &Call) -> Option<(StatusCode, String)> {
    (call.uri == "/domains/?owns_qname=_acme-challenge.app.example.test")
        .then(|| ok(r#"[{"name":"example.test","minimum_ttl":3600}]"#))
}

/// A deSEC API where the record set of the challenge name does not exist.
fn desec_missing(call: &Call) -> (StatusCode, String) {
    desec_zone(call).unwrap_or_else(|| match call.method.as_str() {
        "GET" => (
            StatusCode::NOT_FOUND,
            r#"{"detail":"Not found."}"#.to_string(),
        ),
        _ => ok("{}"),
    })
}

/// A deSEC API where the record set holds only the challenge values.
fn desec_challenge_only(call: &Call) -> (StatusCode, String) {
    desec_zone(call).unwrap_or_else(|| match call.method.as_str() {
        "GET" => {
            ok(r#"{"subname":"_acme-challenge.app","type":"TXT","records":["\"v1\"","\"v2\""]}"#)
        }
        _ => (StatusCode::NO_CONTENT, String::new()),
    })
}

fn desec_with(url: String) -> DnsProvider {
    token_provider(TokenApi::Desec, "ds-token")(url)
}

#[tokio::test]
async fn desec_creates_a_missing_rrset_and_deletes_it_when_only_its_values_remain()
-> anyhow::Result<()> {
    let rrset = "/domains/example.test/rrsets/_acme-challenge.app/TXT/";

    let (client, calls) = mock_client(desec_missing, desec_with).await?;
    let record = client.add_txt(&challenge_name()).await?;
    // The record set is gone, so there is nothing to remove.
    client.remove_txt(&record).await?;
    assert_eq!(
        requests(&calls),
        vec![
            "GET /domains/?owns_qname=_acme-challenge.app.example.test".to_string(),
            format!("GET {rrset}"),
            format!("PUT {rrset}"),
            format!("GET {rrset}"),
        ]
    );
    assert_eq!(
        body(&calls, 2)?,
        json!({ "subname": "_acme-challenge.app", "type": "TXT", "ttl": 3600, "records": ["\"v1\"", "\"v2\""] })
    );
    assert_eq!(calls.lock().unwrap()[0].authorization, "Token ds-token");

    let (client, calls) = mock_client(desec_challenge_only, desec_with).await?;
    client.remove_txt(&record).await?;
    assert_eq!(
        requests(&calls),
        vec![format!("GET {rrset}"), format!("DELETE {rrset}")]
    );
    Ok(())
}

const AZURE_ZONES: &str =
    "/subscriptions/sub-1/providers/Microsoft.Network/dnszones?api-version=2018-05-01";
const AZURE_RECORD: &str = "/subscriptions/sub-1/resourceGroups/rg2/providers/Microsoft.Network/dnszones/example.test/TXT/_acme-challenge.app?api-version=2018-05-01";

fn azure_zone(group: &str, name: &str) -> serde_json::Value {
    json!({
        "id": format!("/subscriptions/sub-1/resourceGroups/{group}/providers/Microsoft.Network/dnszones/{name}"),
        "name": name,
    })
}

fn azure(call: &Call) -> (StatusCode, String) {
    let second_page = format!("{AZURE_ZONES}&$skipToken=2");
    match (call.method.as_str(), call.uri.as_str()) {
        ("POST", "/tenant-1/oauth2/v2.0/token") => {
            ok(r#"{"token_type":"Bearer","expires_in":3599,"access_token":"az-access"}"#)
        }
        ("GET", AZURE_ZONES) => {
            // The next link is a full URL, as in the Azure API.
            let next = format!("http://{}{second_page}", call.header("host"));
            ok(
                &json!({ "value": [azure_zone("rg1", "other.test")], "nextLink": next })
                    .to_string(),
            )
        }
        ("GET", uri) if uri == second_page => {
            ok(&json!({ "value": [azure_zone("rg2", "example.test")] }).to_string())
        }
        // Another tool keeps its own value in the record set, and v1 is already there.
        ("GET", AZURE_RECORD) => {
            ok(r#"{"properties":{"TTL":60,"TXTRecords":[{"value":["other"]},{"value":["v1"]}]}}"#)
        }
        _ => ok("{}"),
    }
}

#[tokio::test]
async fn azure_reuses_its_token_follows_the_next_link_and_keeps_other_values() -> anyhow::Result<()>
{
    let (base, calls) = start_mock(azure).await?;
    let provider = DnsProvider::Cloud(CloudProvider::Azure {
        tenant_id: "tenant-1".to_string(),
        client_id: "client-1".to_string(),
        client_secret: "az-secret".to_string(),
        subscription_id: "sub-1".to_string(),
        api_url: Some(base.clone()),
        auth_url: Some(base.clone()),
    });
    let client = dns::client(&provider, &AcmeExecConfig::default()).await?;
    let record = client.add_txt(&challenge_name()).await?;
    client.remove_txt(&record).await?;

    assert_eq!(
        requests(&calls),
        vec![
            "POST /tenant-1/oauth2/v2.0/token".to_string(),
            format!("GET {AZURE_ZONES}"),
            format!("GET {AZURE_ZONES}&$skipToken=2"),
            format!("GET {AZURE_RECORD}"),
            format!("PUT {AZURE_RECORD}"),
            format!("GET {AZURE_RECORD}"),
            format!("PUT {AZURE_RECORD}"),
        ]
    );
    assert_eq!(
        body(&calls, 4)?,
        json!({ "properties": { "TTL": 60, "TXTRecords": [
            { "value": ["other"] }, { "value": ["v1"] }, { "value": ["v2"] }
        ] } })
    );
    assert_eq!(
        body(&calls, 6)?,
        json!({ "properties": { "TTL": 60, "TXTRecords": [{ "value": ["other"] }] } })
    );
    let calls = calls.lock().unwrap();
    let form: HashMap<String, String> = url::form_urlencoded::parse(calls[0].body.as_bytes())
        .into_owned()
        .collect();
    let expected = [
        ("grant_type", "client_credentials".to_string()),
        ("client_id", "client-1".to_string()),
        ("client_secret", "az-secret".to_string()),
        ("scope", format!("{base}/.default")),
    ]
    .map(|(key, value)| (key.to_string(), value));
    assert_eq!(form, HashMap::from(expected));
    assert!(
        calls[1..]
            .iter()
            .all(|call| call.authorization == "Bearer az-access")
    );
    Ok(())
}

const GOOGLE_RRSET: &str =
    "/projects/key-project/managedZones/example-zone/rrsets/_acme-challenge.app.example.test./TXT";

/// Answers the token and the managed zone requests of a mock Google Cloud DNS API.
fn google_zones(call: &Call) -> Option<(StatusCode, String)> {
    match (call.method.as_str(), call.uri.as_str()) {
        ("POST", "/token") => Some(ok(
            r#"{"access_token":"gc-access","expires_in":3599,"token_type":"Bearer"}"#,
        )),
        // A private zone with the same DNS name comes first and is skipped.
        ("GET", "/projects/key-project/managedZones?dnsName=example.test.") => Some(ok(concat!(
            r#"{"managedZones":[{"name":"private-zone","dnsName":"example.test.","visibility":"private"},"#,
            r#"{"name":"example-zone","dnsName":"example.test.","visibility":"public"}]}"#
        ))),
        ("GET", uri) if uri.contains("/managedZones?") => Some(ok(r#"{"managedZones":[]}"#)),
        _ => None,
    }
}

/// A mock Google Cloud DNS API whose TXT record set holds `rrdatas`, or does not exist.
fn google_with(call: &Call, rrdatas: Option<&str>) -> (StatusCode, String) {
    google_zones(call).unwrap_or_else(|| match (call.method.as_str(), rrdatas) {
        ("GET", Some(rrdatas)) => ok(&format!(
            r#"{{"name":"_acme-challenge.app.example.test.","type":"TXT","ttl":60,"rrdatas":{rrdatas}}}"#
        )),
        ("GET", None) => (
            StatusCode::NOT_FOUND,
            r#"{"error":{"code":404,"message":"Not found"}}"#.to_string(),
        ),
        _ => ok("{}"),
    })
}

fn google_missing(call: &Call) -> (StatusCode, String) {
    google_with(call, None)
}

fn google_with_other(call: &Call) -> (StatusCode, String) {
    google_with(call, Some(r#"["\"other\"","\"v1\"","\"v2\""]"#))
}

fn google_challenge_only(call: &Call) -> (StatusCode, String) {
    google_with(call, Some(r#"["\"v1\"","\"v2\""]"#))
}

/// Checks the form and the signed JWT of a Google token request with the public key of `key`.
fn verify_google_assertion(
    call: &Call,
    key: &rsa::RsaPrivateKey,
    audience: &str,
) -> anyhow::Result<()> {
    let form: HashMap<String, String> = url::form_urlencoded::parse(call.body.as_bytes())
        .into_owned()
        .collect();
    assert_eq!(
        form["grant_type"],
        "urn:ietf:params:oauth:grant-type:jwt-bearer"
    );
    let (message, signature) = form["assertion"]
        .rsplit_once('.')
        .ok_or_else(|| anyhow::anyhow!("the assertion has no signature"))?;
    let (header, claims) = message
        .split_once('.')
        .ok_or_else(|| anyhow::anyhow!("the assertion has no claims"))?;
    let decode = |part: &str| -> anyhow::Result<serde_json::Value> {
        Ok(serde_json::from_slice(&URL_SAFE_NO_PAD.decode(part)?)?)
    };
    assert_eq!(decode(header)?, json!({ "alg": "RS256", "typ": "JWT" }));
    let claims = decode(claims)?;
    assert_eq!(claims["iss"], "dns@key-project.iam.gserviceaccount.com");
    assert_eq!(
        claims["scope"],
        "https://www.googleapis.com/auth/ndev.clouddns.readwrite"
    );
    assert_eq!(claims["aud"], audience);
    let issued = claims["iat"].as_i64().unwrap_or_default();
    assert_eq!(claims["exp"].as_i64(), Some(issued + 3600));
    let public_key = key.to_public_key().to_pkcs1_der()?;
    UnparsedPublicKey::new(&RSA_PKCS1_2048_8192_SHA256, public_key.as_bytes())
        .verify(message.as_bytes(), &URL_SAFE_NO_PAD.decode(signature)?)
        .map_err(|_| anyhow::anyhow!("the assertion signature is invalid"))
}

#[tokio::test]
async fn google_cloud_signs_its_token_request_skips_private_zones_and_keeps_other_values()
-> anyhow::Result<()> {
    // The test creates its own key, so no key file is stored in the repository.
    let key = rsa::RsaPrivateKey::new(&mut rand::thread_rng(), 2048)?;
    let key_file = json!({
        "type": "service_account",
        "project_id": "key-project",
        "client_email": "dns@key-project.iam.gserviceaccount.com",
        "private_key": key.to_pkcs8_pem(LineEnding::LF)?.as_str(),
    })
    .to_string();
    let provider = |project_id: &str, url: String| {
        DnsProvider::Cloud(CloudProvider::GoogleCloud {
            service_account_key: key_file.clone(),
            project_id: project_id.to_string(),
            api_url: Some(url.clone()),
            auth_url: Some(url),
        })
    };

    let (base, calls) = start_mock(google_missing).await?;
    let client = dns::client(&provider("", base.clone()), &AcmeExecConfig::default()).await?;
    let record = client.add_txt(&challenge_name()).await?;
    assert_eq!(
        requests(&calls),
        vec![
            "POST /token".to_string(),
            "GET /projects/key-project/managedZones?dnsName=app.example.test.".to_string(),
            "GET /projects/key-project/managedZones?dnsName=example.test.".to_string(),
            format!("GET {GOOGLE_RRSET}"),
            "POST /projects/key-project/managedZones/example-zone/rrsets".to_string(),
        ]
    );
    let rrset = |rrdatas: serde_json::Value| json!({ "name": "_acme-challenge.app.example.test.", "type": "TXT", "ttl": 60, "rrdatas": rrdatas });
    assert_eq!(body(&calls, 4)?, rrset(json!(["\"v1\"", "\"v2\""])));
    {
        let calls = calls.lock().unwrap();
        verify_google_assertion(&calls[0], &key, &format!("{base}/token"))?;
        assert!(
            calls[1..]
                .iter()
                .all(|call| call.authorization == "Bearer gc-access")
        );
    }

    let (client, calls) = mock_client(google_with_other, |url| provider("", url)).await?;
    client.remove_txt(&record).await?;
    assert_eq!(
        requests(&calls),
        vec![
            "POST /token".to_string(),
            format!("GET {GOOGLE_RRSET}"),
            format!("PATCH {GOOGLE_RRSET}"),
        ]
    );
    assert_eq!(body(&calls, 2)?, rrset(json!(["\"other\""])));

    let (client, calls) = mock_client(google_challenge_only, |url| provider("", url)).await?;
    client.remove_txt(&record).await?;
    assert_eq!(
        requests(&calls),
        vec![
            "POST /token".to_string(),
            format!("GET {GOOGLE_RRSET}"),
            format!("DELETE {GOOGLE_RRSET}"),
        ]
    );

    // A project ID replaces the project of the key.
    let (client, calls) = mock_client(google_missing, |url| provider("dns-project", url)).await?;
    assert!(client.add_txt(&challenge_name()).await.is_err());
    assert_eq!(
        requests(&calls).get(1).map(String::as_str),
        Some("GET /projects/dns-project/managedZones?dnsName=app.example.test.")
    );
    Ok(())
}

fn webhook_with(url: String) -> DnsProvider {
    DnsProvider::Local(LocalProvider::Webhook {
        url: format!("{url}/acme"),
        token: "wh-token".to_string(),
    })
}

fn webhook(_: &Call) -> (StatusCode, String) {
    (StatusCode::NO_CONTENT, String::new())
}

#[tokio::test]
async fn a_webhook_receives_the_action_the_name_and_the_values() -> anyhow::Result<()> {
    let calls = add_and_remove(webhook, webhook_with).await?;

    assert_eq!(requests(&calls), vec!["POST /acme", "POST /acme"]);
    let expected = |action: &str| json!({ "action": action, "fqdn": "_acme-challenge.app.example.test", "values": ["v1", "v2"] });
    assert_eq!(body(&calls, 0)?, expected("add"));
    assert_eq!(body(&calls, 1)?, expected("remove"));
    assert!(
        calls
            .lock()
            .unwrap()
            .iter()
            .all(|call| call.authorization == "Bearer wh-token")
    );
    Ok(())
}

fn webhook_add_fails(call: &Call) -> (StatusCode, String) {
    if call.body.contains("\"add\"") {
        return (StatusCode::BAD_GATEWAY, "upstream DNS failed".to_string());
    }
    webhook(call)
}

#[tokio::test]
async fn a_failed_webhook_add_sends_a_remove_and_a_plain_http_host_is_refused() -> anyhow::Result<()>
{
    let (client, calls) = mock_client(webhook_add_fails, webhook_with).await?;
    let Err(err) = client.add_txt(&challenge_name()).await else {
        anyhow::bail!("add_txt succeeded");
    };
    assert!(format!("{err:#}").contains("502"), "{err:#}");
    assert_eq!(body(&calls, 1)?["action"], "remove");

    let remote = DnsProvider::Local(LocalProvider::Webhook {
        url: "http://dns-hook.example.test/acme".to_string(),
        token: String::new(),
    });
    assert!(
        dns::client(&remote, &AcmeExecConfig::default())
            .await
            .is_err()
    );
    Ok(())
}

/// Writes an executable shell script under the test target directory, and returns its path
/// and the path of the log that `$log` names in `body`.
fn exec_script(name: &str, body: &str) -> anyhow::Result<(PathBuf, PathBuf)> {
    use std::os::unix::fs::PermissionsExt;

    let dir = Path::new(env!("CARGO_TARGET_TMPDIR")).join("acme-exec");
    std::fs::create_dir_all(&dir)?;
    let log = dir.join(format!("{name}.log"));
    if log.exists() {
        std::fs::remove_file(&log)?;
    }
    let script = dir.join(format!("{name}.sh"));
    std::fs::write(
        &script,
        format!("#!/bin/sh\nlog='{}'\n{body}\n", log.display()),
    )?;
    std::fs::set_permissions(&script, std::fs::Permissions::from_mode(0o755))?;
    Ok((script, log))
}

fn exec_provider(program: &Path) -> DnsProvider {
    DnsProvider::Local(LocalProvider::Exec {
        program: program.display().to_string(),
    })
}

fn allowlist(programs: &[&Path], timeout: Duration) -> AcmeExecConfig {
    AcmeExecConfig {
        programs: programs
            .iter()
            .map(|program| program.to_path_buf())
            .collect(),
        timeout,
    }
}

fn log_lines(log: &Path) -> anyhow::Result<Vec<String>> {
    Ok(std::fs::read_to_string(log)?
        .lines()
        .map(str::to_string)
        .collect())
}

#[tokio::test]
async fn an_exec_program_gets_each_value_as_an_argument_without_a_shell_or_environment()
-> anyhow::Result<()> {
    let (script, log) = exec_script(
        "arguments",
        r#"echo "$# $1 $2 [$3] ${HOME-unset}" >> "$log""#,
    )?;
    let config = allowlist(&[&script], Duration::from_secs(10));
    let client = dns::client(&exec_provider(&script), &config).await?;
    let name = TxtName {
        fqdn: "_acme-challenge.app.example.test".to_string(),
        values: vec!["$(id)".to_string(), "a; b".to_string()],
    };
    let record = client.add_txt(&name).await?;
    client.remove_txt(&record).await?;

    let fqdn = "_acme-challenge.app.example.test";
    assert_eq!(
        log_lines(&log)?,
        vec![
            format!("3 add {fqdn} [$(id)] unset"),
            format!("3 add {fqdn} [a; b] unset"),
            format!("3 remove {fqdn} [$(id)] unset"),
            format!("3 remove {fqdn} [a; b] unset"),
        ]
    );
    Ok(())
}

#[tokio::test]
async fn an_exec_program_outside_the_allowlist_is_refused() -> anyhow::Result<()> {
    let (allowed, _) = exec_script("allowed", "exit 0")?;
    let (other, _) = exec_script("other", "exit 0")?;
    let config = allowlist(&[&allowed], Duration::from_secs(10));

    let link = allowed.with_file_name("allowed-link.sh");
    if link.symlink_metadata().is_ok() {
        std::fs::remove_file(&link)?;
    }
    std::os::unix::fs::symlink(&allowed, &link)?;
    assert!(dns::client(&exec_provider(&link), &config).await.is_ok());

    for program in [other.display().to_string(), "allowed.sh".to_string()] {
        let provider = DnsProvider::Local(LocalProvider::Exec {
            program: program.clone(),
        });
        assert!(dns::client(&provider, &config).await.is_err(), "{program}");
        assert!(matches!(
            dns::check_exec_provider(Some(&provider), &config),
            Err(Error::AcmeExecProgramNotAllowed { program: refused }) if refused == program
        ));
    }
    assert!(dns::check_exec_provider(Some(&exec_provider(&allowed)), &config).is_ok());
    Ok(())
}

#[tokio::test]
async fn a_failing_exec_program_removes_the_added_values_and_reports_its_stderr()
-> anyhow::Result<()> {
    let (script, log) = exec_script(
        "failing",
        r#"echo "$1 $3" >> "$log"
if [ "$1" = add ] && [ "$3" = v2 ]; then echo "zone is locked" >&2; exit 3; fi"#,
    )?;
    let config = allowlist(&[&script], Duration::from_secs(10));
    let client = dns::client(&exec_provider(&script), &config).await?;

    let Err(err) = client.add_txt(&challenge_name()).await else {
        anyhow::bail!("add_txt succeeded");
    };
    let message = format!("{err:#}");
    assert!(message.contains("zone is locked"), "{message}");
    assert!(message.contains('3'), "{message}");
    assert_eq!(
        log_lines(&log)?,
        vec!["add v1", "add v2", "remove v1", "remove v2"]
    );
    Ok(())
}

#[tokio::test]
async fn an_exec_program_is_stopped_when_the_timeout_expires() -> anyhow::Result<()> {
    let (script, _) = exec_script("slow", "exec /bin/sleep 5")?;
    let config = allowlist(&[&script], Duration::from_millis(200));
    let client = dns::client(&exec_provider(&script), &config).await?;

    let started = std::time::Instant::now();
    let Err(err) = client.add_txt(&challenge_name()).await else {
        anyhow::bail!("add_txt succeeded");
    };
    assert!(format!("{err:#}").contains("did not add"), "{err:#}");
    assert!(started.elapsed() < Duration::from_secs(3));
    Ok(())
}

fn porkbun_with(url: String) -> DnsProvider {
    DnsProvider::Keyed(KeyedProvider::Porkbun {
        api_key: "pk-key".to_string(),
        secret_api_key: "pk-secret".to_string(),
        api_url: Some(url),
    })
}

fn porkbun(call: &Call) -> (StatusCode, String) {
    let create = call.uri.starts_with("/dns/create/");
    match call.uri.as_str() {
        "/domain/listAll" => ok(
            r#"{"status":"SUCCESS","domains":[{"domain":"other.test"},{"domain":"example.test"}]}"#,
        ),
        _ if create && call.body.contains("\"v2\"") => ok(r#"{"status":"SUCCESS","id":102}"#),
        _ if create => ok(r#"{"status":"SUCCESS","id":101}"#),
        _ => ok(r#"{"status":"SUCCESS"}"#),
    }
}

#[tokio::test]
async fn porkbun_sends_the_keys_in_every_body() -> anyhow::Result<()> {
    let calls = add_and_remove(porkbun, porkbun_with).await?;

    assert_eq!(
        requests(&calls),
        vec![
            "POST /domain/listAll",
            "POST /dns/create/example.test",
            "POST /dns/create/example.test",
            "POST /dns/delete/example.test/101",
            "POST /dns/delete/example.test/102",
        ]
    );
    let keys = json!({ "apikey": "pk-key", "secretapikey": "pk-secret" });
    assert_eq!(
        body(&calls, 0)?,
        json!({ "start": 0, "apikey": keys["apikey"], "secretapikey": keys["secretapikey"] })
    );
    assert_eq!(
        body(&calls, 1)?,
        json!({ "name": "_acme-challenge.app", "type": "TXT", "content": "v1", "apikey": "pk-key", "secretapikey": "pk-secret" })
    );
    assert_eq!(body(&calls, 3)?, keys);
    Ok(())
}

fn porkbun_error(_: &Call) -> (StatusCode, String) {
    ok(r#"{"status":"ERROR","message":"Invalid API key."}"#)
}

#[tokio::test]
async fn a_porkbun_error_status_fails_without_the_keys() -> anyhow::Result<()> {
    let (client, _) = mock_client(porkbun_error, porkbun_with).await?;

    let Err(err) = client.add_txt(&challenge_name()).await else {
        anyhow::bail!("add_txt succeeded");
    };
    let message = format!("{err:#}");
    assert!(message.contains("Invalid API key."), "{message}");
    assert!(!message.contains("pk-secret"), "{message}");
    Ok(())
}

/// The clock of the mock OVH API.
const OVH_TIME: i64 = 1_700_000_000;

fn ovh(call: &Call) -> (StatusCode, String) {
    match (call.method.as_str(), call.uri.as_str()) {
        ("GET", "/auth/time") => ok(&OVH_TIME.to_string()),
        ("GET", "/domain/zone") => ok(r#"["other.test","example.test"]"#),
        ("POST", "/domain/zone/example.test/record") if call.body.contains("\"v2\"") => {
            ok(r#"{"id":502}"#)
        }
        ("POST", "/domain/zone/example.test/record") => ok(r#"{"id":501}"#),
        _ => ok("null"),
    }
}

#[tokio::test]
async fn ovh_signs_each_request_with_the_ovh_clock_and_refreshes_the_zone() -> anyhow::Result<()> {
    let (url, calls) = start_mock(ovh).await?;
    let provider = DnsProvider::Keyed(KeyedProvider::Ovh {
        endpoint: OvhEndpoint::OvhEu,
        application_key: "app-key".to_string(),
        application_secret: "app-secret".to_string(),
        consumer_key: "consumer-key".to_string(),
        api_url: Some(url.clone()),
    });
    let client = dns::client(&provider, &AcmeExecConfig::default()).await?;
    let record = client.add_txt(&challenge_name()).await?;
    client.remove_txt(&record).await?;

    assert_eq!(
        requests(&calls),
        vec![
            "GET /auth/time",
            "GET /domain/zone",
            "POST /domain/zone/example.test/record",
            "POST /domain/zone/example.test/record",
            "POST /domain/zone/example.test/refresh",
            "DELETE /domain/zone/example.test/record/501",
            "DELETE /domain/zone/example.test/record/502",
            "POST /domain/zone/example.test/refresh",
        ]
    );
    assert_eq!(
        body(&calls, 2)?,
        json!({ "fieldType": "TXT", "subDomain": "_acme-challenge.app", "target": "v1", "ttl": 60 })
    );
    let calls = calls.lock().unwrap();
    assert_eq!(calls[0].header("x-ovh-signature"), "");
    for call in &calls[1..] {
        assert_eq!(call.header("x-ovh-application"), "app-key");
        assert_eq!(call.header("x-ovh-consumer"), "consumer-key");
        let timestamp = call.header("x-ovh-timestamp");
        // The timestamp follows the clock of the API, not the local clock.
        let seconds: i64 = timestamp.parse()?;
        assert!((OVH_TIME..OVH_TIME + 60).contains(&seconds), "{timestamp}");
        let signed = format!(
            "app-secret+consumer-key+{}+{url}{}+{}+{timestamp}",
            call.method, call.uri, call.body
        );
        let expected = format!("$1${}", hex::encode(Sha1::digest(signed)));
        assert_eq!(call.header("x-ovh-signature"), expected);
    }
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
    ok(
        "<ChangeResourceRecordSetsResponse><ChangeInfo><Id>/change/C1</Id><Status>PENDING</Status></ChangeInfo></ChangeResourceRecordSetsResponse>",
    )
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
