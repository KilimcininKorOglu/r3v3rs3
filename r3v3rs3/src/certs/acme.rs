use crate::{
    cdn::fetch::{build_client, HttpClient as FetchClient},
    certs::Cert,
    server::cert_list::CertList,
};
use anyhow::{anyhow, bail};
use backoff::{backoff::Backoff, ExponentialBackoffBuilder};
use bytes::Bytes;
use http_body_util::{BodyExt, Full};
use hyper::{
    header::{HeaderValue, USER_AGENT},
    Request,
};
use instant_acme::{
    Account, AccountCredentials, AuthorizationStatus, BodyWrapper, BytesResponse, ChallengeType,
    ExternalAccountKey, HttpClient, Identifier, NewAccount, NewOrder, Order, OrderStatus,
};
use r3v3rs3_api::acme::AcmeInfo;
use r3v3rs3_api::{
    acme::Acme,
    cert::{CertKind, CertMetadata},
    id::ShortId,
};
use r3v3rs3_api::{acme::AcmeRequest, error::Error};
use rcgen::{CertificateParams, DistinguishedName, KeyPair};
use serde_derive::{Deserialize, Serialize};
use std::{
    collections::HashMap,
    fmt,
    future::Future,
    pin::Pin,
    sync::Arc,
    time::{Duration, SystemTime},
};
use tracing::{error, info};

const HTTP_CHALLENGE_TIMEOUT: Duration = Duration::from_secs(180);
const ACME_USER_AGENT: &str = concat!("r3v3rs3/", env!("CARGO_PKG_VERSION"));

/// Adds the User-Agent header that RFC 8555 section 6.1 requires on every ACME request.
struct AcmeHttpClient<H> {
    inner: H,
}

impl<H: HttpClient> HttpClient for AcmeHttpClient<H> {
    fn request(
        &self,
        mut req: Request<BodyWrapper<Bytes>>,
    ) -> Pin<Box<dyn Future<Output = Result<BytesResponse, instant_acme::Error>> + Send>> {
        if req.uri().scheme_str() != Some("https") {
            return Box::pin(std::future::ready(Err(instant_acme::Error::Str(
                "ACME requests need an https URL",
            ))));
        }
        req.headers_mut()
            .insert(USER_AGENT, HeaderValue::from_static(ACME_USER_AGENT));
        self.inner.request(req)
    }
}

/// Sends ACME requests through the hyper client that the server uses for other outbound HTTPS.
struct HyperAcmeClient(FetchClient);

impl HttpClient for HyperAcmeClient {
    fn request(
        &self,
        req: Request<BodyWrapper<Bytes>>,
    ) -> Pin<Box<dyn Future<Output = Result<BytesResponse, instant_acme::Error>> + Send>> {
        let client = self.0.clone();
        Box::pin(async move {
            let (parts, body) = req.into_parts();
            let Ok(body) = body.collect().await;
            let req = Request::from_parts(parts, Full::new(body.to_bytes()));
            let rsp = client
                .request(req)
                .await
                .map_err(|err| instant_acme::Error::Other(Box::new(err)))?;
            Ok(BytesResponse::from(rsp))
        })
    }
}

async fn acme_http_client() -> anyhow::Result<Box<dyn HttpClient>> {
    Ok(Box::new(AcmeHttpClient {
        inner: HyperAcmeClient(build_client().await?),
    }))
}

#[derive(Clone, Serialize, Deserialize)]
pub struct AcmeEntry {
    pub id: ShortId,
    #[serde(flatten)]
    pub acme: Acme,
    pub account: Arc<AccountCredentials>,
}

impl fmt::Debug for AcmeEntry {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("AcmeEntry")
            .field("config", &self.acme.config)
            .field("identifiers", &self.acme.identifiers)
            .finish()
    }
}

impl AcmeEntry {
    pub async fn new(id: ShortId, req: AcmeRequest) -> Result<Self, Error> {
        let contact = req.contacts.iter().map(|c| c.as_str()).collect::<Vec<_>>();
        let external_account = req
            .eab
            .map(|eab| ExternalAccountKey::new(eab.key_id, &eab.hmac_key));
        let http = acme_http_client().await.map_err(|e| {
            error!("failed to create the ACME http client: {}", e);
            Error::AcmeAccountCreationFailed
        })?;
        let account = Account::builder_with_http(http)
            .create(
                &NewAccount {
                    contact: &contact,
                    terms_of_service_agreed: true,
                    only_return_existing: false,
                },
                req.server_url.clone(),
                external_account.as_ref(),
            )
            .await;

        let (_, account) = match account {
            Ok(account) => account,
            Err(e) => {
                error!("failed to create account: {}", e);
                return Err(Error::AcmeAccountCreationFailed);
            }
        };

        Ok(Self {
            id,
            acme: req.acme,
            account: Arc::new(account),
        })
    }

    pub async fn request(&self) -> anyhow::Result<AcmeOrder> {
        AcmeOrder::new(self).await
    }

    pub fn id(&self) -> ShortId {
        self.id
    }

    pub fn info(&self, certs: &CertList) -> AcmeInfo {
        AcmeInfo {
            id: self.id,
            config: self.acme.config.clone(),
            identifiers: self
                .acme
                .identifiers
                .iter()
                .map(|id| id.to_string())
                .collect(),
            challenge_type: self.acme.challenge_type.clone(),
            next_renewal: self
                .next_renewal(certs)
                .and_then(|t| t.duration_since(SystemTime::UNIX_EPOCH).ok())
                .map(|t| t.as_secs() as i64),
        }
    }

    pub fn last_issued(&self, certs: &CertList) -> Option<SystemTime> {
        certs
            .find_certs_by_acme(self.id)
            .iter()
            .map(|cert| {
                cert.metadata
                    .as_ref()
                    .map(|meta| meta.created_at)
                    .unwrap_or(SystemTime::UNIX_EPOCH)
            })
            .max()
    }

    pub fn next_renewal(&self, certs: &CertList) -> Option<SystemTime> {
        let last_issued = self.last_issued(certs)?;
        let renewal_days = self.acme.config.renewal_days;
        let next_renewal = last_issued + Duration::from_secs(60 * 60 * 24 * renewal_days);
        Some(next_renewal)
    }
}

#[derive(Clone, Serialize, Deserialize)]
pub struct AcmeAccount {
    #[serde(flatten)]
    pub acme: Acme,
    pub account: Arc<AccountCredentials>,
}

impl From<AcmeEntry> for (ShortId, AcmeAccount) {
    fn from(entry: AcmeEntry) -> Self {
        (
            entry.id,
            AcmeAccount {
                acme: entry.acme,
                account: entry.account,
            },
        )
    }
}

impl From<(ShortId, AcmeAccount)> for AcmeEntry {
    fn from((id, entry): (ShortId, AcmeAccount)) -> Self {
        Self {
            id,
            acme: entry.acme,
            account: entry.account,
        }
    }
}

pub struct AcmeOrder {
    pub id: ShortId,
    pub challenge_type: ChallengeType,
    pub identifiers: Vec<Identifier>,
    pub http_challenges: HashMap<String, String>,
    pub order: Order,
}

impl AcmeOrder {
    pub async fn new(entry: &AcmeEntry) -> anyhow::Result<Self> {
        info!("requesting certificate");

        // An entry stored before validation existed can still hold a name the challenge cannot validate.
        entry.acme.validate()?;
        let identifiers = entry
            .acme
            .identifiers
            .iter()
            .map(|id| Identifier::Dns(id.to_string()))
            .collect::<Vec<_>>();
        let account: AccountCredentials =
            serde_json::from_str(&serde_json::to_string(&entry.account)?)?;
        let challenge_type = match entry.acme.challenge_type.as_str() {
            "http-01" => ChallengeType::Http01,
            _ => bail!("challenge type is not supported"),
        };
        let account = Account::builder_with_http(acme_http_client().await?)
            .from_credentials(account)
            .await?;
        let mut order = account.new_order(&NewOrder::new(&identifiers)).await?;
        let http_challenges = collect_http_challenges(&mut order).await?;
        Ok(Self {
            id: entry.id,
            challenge_type,
            identifiers,
            http_challenges,
            order,
        })
    }

    /// Tells the ACME server that every pending challenge is ready for validation.
    async fn set_challenges_ready(&mut self) -> anyhow::Result<()> {
        let mut authorizations = self.order.authorizations();
        while let Some(authz) = authorizations.next().await {
            let mut authz = authz?;
            if authz.status != AuthorizationStatus::Pending {
                continue;
            }
            let mut challenge = authz
                .challenge(self.challenge_type.clone())
                .ok_or_else(|| {
                    anyhow!(
                        "the ACME server offers no {:?} challenge",
                        self.challenge_type
                    )
                })?;
            challenge.set_ready().await?;
        }
        Ok(())
    }

    pub async fn start_challenge(&mut self) -> anyhow::Result<Cert> {
        self.set_challenges_ready().await?;

        let mut backoff = ExponentialBackoffBuilder::new()
            .with_max_elapsed_time(Some(HTTP_CHALLENGE_TIMEOUT))
            .build();
        loop {
            let state = self.order.refresh().await?;
            match state.status {
                OrderStatus::Ready => break,
                OrderStatus::Invalid => {
                    bail!("order is invalid");
                }
                _ => (),
            }
            if let Some(next) = backoff.next_backoff() {
                tokio::time::sleep(next).await;
            } else {
                bail!("order is timed-out");
            }
        }

        let san = self
            .identifiers
            .iter()
            .filter_map(|id| match id {
                Identifier::Dns(domain) => Some(domain.clone()),
                _ => None,
            })
            .collect::<Vec<_>>();

        let mut params = CertificateParams::new(san)?;
        params.distinguished_name = DistinguishedName::new();

        let keypair = KeyPair::generate()?;
        let request = params.serialize_request(&keypair)?;
        let csr = request.der();

        self.order.finalize_csr(csr).await?;
        let cert_chain_pem = loop {
            match self.order.certificate().await? {
                Some(cert_chain_pem) => break cert_chain_pem,
                None => tokio::time::sleep(Duration::from_secs(1)).await,
            }
        };

        let metadata = CertMetadata {
            acme_id: self.id,
            created_at: SystemTime::now(),
        };
        let metadata = serde_qs::to_string(&metadata).unwrap_or_default();
        let cert_chain_pem = format!("# {}\r\n\r\n{}", metadata, cert_chain_pem);

        let cert = Cert::new(
            CertKind::Server,
            cert_chain_pem.into_bytes(),
            Some(keypair.serialize_pem().into_bytes()),
        );

        Ok(cert?)
    }
}

/// Collects the key authorizations of the pending HTTP-01 challenges, by token.
async fn collect_http_challenges(order: &mut Order) -> anyhow::Result<HashMap<String, String>> {
    let mut challenges = HashMap::new();
    let mut authorizations = order.authorizations();
    while let Some(authz) = authorizations.next().await {
        let mut authz = authz?;
        match authz.status {
            AuthorizationStatus::Pending => {}
            AuthorizationStatus::Valid => continue,
            _ => bail!("authorization status is not valid"),
        }
        let challenge = authz
            .challenge(ChallengeType::Http01)
            .ok_or_else(|| anyhow!("no http01 challenge found"))?;
        challenges.insert(
            challenge.token.clone(),
            challenge.key_authorization().as_str().to_string(),
        );
    }
    Ok(challenges)
}

#[cfg(test)]
mod tests {
    use super::*;
    use hyper::{
        header::{HeaderMap, LOCATION},
        Response,
    };
    use std::sync::Mutex;

    #[derive(Default)]
    struct RecordingClient {
        headers: Arc<Mutex<Vec<HeaderMap>>>,
    }

    impl HttpClient for RecordingClient {
        fn request(
            &self,
            req: Request<BodyWrapper<Bytes>>,
        ) -> Pin<Box<dyn Future<Output = Result<BytesResponse, instant_acme::Error>> + Send>>
        {
            if let Ok(mut headers) = self.headers.lock() {
                headers.push(req.headers().clone());
            }
            Box::pin(std::future::ready(Ok(BytesResponse::from(Response::new(
                Full::new(Bytes::new()),
            )))))
        }
    }

    const DIRECTORY: &str = r#"{"newNonce":"https://acme.example/nonce","newAccount":"https://acme.example/account","newOrder":"https://acme.example/order"}"#;
    const ORDER: &str = r#"{"status":"pending","authorizations":["https://acme.example/authz/1"],"finalize":"https://acme.example/order/1/finalize"}"#;
    /// Pebble and Let's Encrypt also offer `dns-persist-01`, which has no token.
    const AUTHORIZATION: &str = r#"{
        "status": "pending",
        "identifier": {"type": "dns", "value": "example.com"},
        "challenges": [
            {"type": "dns-persist-01", "url": "https://acme.example/chall/2", "status": "pending", "issuer-domain-names": ["acme.example"]},
            {"type": "http-01", "url": "https://acme.example/chall/1", "token": "http-token", "status": "pending"}
        ]
    }"#;

    /// Answers the requests of an HTTP-01 order for `example.com`.
    struct FakeAcmeServer;

    impl HttpClient for FakeAcmeServer {
        fn request(
            &self,
            req: Request<BodyWrapper<Bytes>>,
        ) -> Pin<Box<dyn Future<Output = Result<BytesResponse, instant_acme::Error>> + Send>>
        {
            let (status, body) = match req.uri().path() {
                "/directory" => (200, DIRECTORY),
                "/nonce" => (200, ""),
                "/order" => (201, ORDER),
                "/authz/1" => (200, AUTHORIZATION),
                _ => (404, r#"{"type":"urn:ietf:params:acme:error:malformed"}"#),
            };
            let rsp = Response::builder()
                .status(status)
                .header("Replay-Nonce", "nonce")
                .header(LOCATION, "https://acme.example/order/1")
                .body(Full::new(Bytes::from_static(body.as_bytes())))
                .unwrap();
            Box::pin(std::future::ready(Ok(BytesResponse::from(rsp))))
        }
    }

    /// Account credentials as instant-acme 0.7 stored them in `acme.toml`.
    const STORED_BY_0_7: &str = r#"
        [account]
        id = "https://acme.example/acct/1"
        key_pkcs8 = "MIGHAgEAMBMGByqGSM49AgEGCCqGSM49AwEHBG0wawIBAQQgJVWC_QzOTCS5vtsJp2IG-UDc8cdDfeoKtxSZxaznM-mhRANCAAQenCPoGgPFTdPJ7VLLKt56RxPlYT1wNXnHc54PEyBg3LxKaH0-sJkX0mL8LyPEdsfL_Oz4TxHkWLJGrXVtNhfH"
        directory = "https://acme.example/directory"
    "#;

    async fn stored_account() -> Account {
        #[derive(Deserialize)]
        struct Stored {
            account: AccountCredentials,
        }
        let stored: Stored = toml::from_str(STORED_BY_0_7).unwrap();
        Account::builder_with_http(Box::new(FakeAcmeServer))
            .from_credentials(stored.account)
            .await
            .unwrap()
    }

    #[tokio::test]
    async fn credentials_stored_by_instant_acme_0_7_still_load() {
        assert_eq!(stored_account().await.id(), "https://acme.example/acct/1");
    }

    #[tokio::test]
    async fn a_challenge_without_a_token_does_not_break_the_order() {
        let identifiers = [Identifier::Dns("example.com".to_string())];
        let mut order = stored_account()
            .await
            .new_order(&NewOrder::new(&identifiers))
            .await
            .unwrap();
        let challenges = collect_http_challenges(&mut order).await.unwrap();
        assert_eq!(challenges.keys().collect::<Vec<_>>(), ["http-token"]);
        assert!(challenges["http-token"].starts_with("http-token."));
    }

    /// Sends one GET through the wrapper and returns its outcome with the headers the inner client saw.
    async fn send(url: &str) -> (bool, Vec<HeaderMap>) {
        let inner = RecordingClient::default();
        let headers = inner.headers.clone();
        let client = AcmeHttpClient { inner };
        let req = Request::get(url).body(BodyWrapper::default()).unwrap();
        let ok = client.request(req).await.is_ok();
        let seen = headers.lock().unwrap().clone();
        (ok, seen)
    }

    #[tokio::test]
    async fn every_acme_request_carries_a_user_agent() {
        let (ok, headers) = send("https://acme.example/directory").await;
        assert!(ok);
        assert_eq!(headers.len(), 1);
        assert_eq!(headers[0][USER_AGENT], ACME_USER_AGENT);
    }

    #[tokio::test]
    async fn a_plain_http_url_is_refused_before_it_is_sent() {
        let (ok, headers) = send("http://acme.example/directory").await;
        assert!(!ok);
        assert!(headers.is_empty());
    }
}
