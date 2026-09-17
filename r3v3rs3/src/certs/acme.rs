use crate::{
    cdn::fetch::{build_client, HttpClient as FetchClient},
    certs::{
        alpn::TlsAlpnChallenge,
        dns::{self, TxtName},
        Cert,
    },
    server::cert_list::CertList,
};
use anyhow::{anyhow, bail};
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
use r3v3rs3_api::acme::{AcmeInfo, DnsProvider, DNS_01, HTTP_01, TLS_ALPN_01};
use r3v3rs3_api::app::AcmeExecConfig;
use r3v3rs3_api::{
    acme::Acme,
    cert::{CertKind, CertMetadata},
    id::ShortId,
};
use r3v3rs3_api::{acme::AcmeRequest, error::Error, subject_name::SubjectName};
use rcgen::{CertificateParams, DistinguishedName, KeyPair};
use serde_derive::{Deserialize, Serialize};
use std::{
    collections::HashMap,
    fmt,
    future::Future,
    net::SocketAddr,
    pin::Pin,
    sync::Arc,
    time::{Duration, Instant, SystemTime},
};
use tracing::{error, info};

/// Longest wait for the ACME server to validate the challenges.
const VALIDATION_TIMEOUT: Duration = Duration::from_secs(180);
/// First wait between two order refreshes.
const VALIDATION_MIN_INTERVAL: Duration = Duration::from_millis(500);
/// Longest wait between two order refreshes.
const VALIDATION_MAX_INTERVAL: Duration = Duration::from_secs(60);
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

    /// Starts an order for the domain names of the target. `dns_resolver` is the DNS server that
    /// the DNS-01 challenge asks, and `acme_exec` holds the programs of the exec DNS provider.
    pub async fn request(
        &self,
        target: &AcmeTarget,
        dns_resolver: Option<SocketAddr>,
        acme_exec: &AcmeExecConfig,
    ) -> anyhow::Result<AcmeOrder> {
        AcmeOrder::new(self, target, dns_resolver, acme_exec).await
    }

    pub fn id(&self) -> ShortId {
        self.id
    }

    /// The target of the domain names of this entry.
    pub fn target(&self) -> AcmeTarget {
        AcmeTarget::new(self.id, &self.acme.identifiers)
    }

    /// The settings of this entry with the domain names of the target. Fails when the challenge
    /// of the entry cannot validate a name.
    pub fn acme_for(&self, target: &AcmeTarget) -> Result<Acme, Error> {
        let identifiers = target
            .identifiers
            .iter()
            .map(|name| name.parse::<SubjectName>())
            .collect::<Result<Vec<_>, _>>()?;
        let acme = Acme {
            identifiers,
            ..self.acme.clone()
        };
        acme.validate()?;
        Ok(acme)
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
            dns_provider: self
                .acme
                .dns_provider
                .as_ref()
                .map(|provider| provider.name().to_string()),
            next_renewal: self
                .next_renewal(certs)
                .and_then(|t| t.duration_since(SystemTime::UNIX_EPOCH).ok())
                .map(|t| t.as_secs() as i64),
        }
    }

    pub fn next_renewal(&self, certs: &CertList) -> Option<SystemTime> {
        self.next_renewal_for(&self.target(), certs)
    }

    /// The renewal time of the newest certificate of the target, or `None` without a certificate.
    pub fn next_renewal_for(&self, target: &AcmeTarget, certs: &CertList) -> Option<SystemTime> {
        let last_issued = certs
            .find_certs_for_target(target)
            .iter()
            .map(|cert| {
                cert.metadata
                    .as_ref()
                    .map(|meta| meta.created_at)
                    .unwrap_or(SystemTime::UNIX_EPOCH)
            })
            .max()?;
        let renewal_days = self.acme.config.renewal_days;
        Some(last_issued + Duration::from_secs(60 * 60 * 24 * renewal_days))
    }
}

/// The domain names that one ACME entry orders together. The certificates of a target renew
/// together, apart from the other certificates of the entry.
#[derive(Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct AcmeTarget {
    pub acme_id: ShortId,
    /// Lowercase, sorted and without duplicates.
    pub identifiers: Vec<String>,
}

impl AcmeTarget {
    pub fn new<T: fmt::Display>(acme_id: ShortId, names: impl IntoIterator<Item = T>) -> Self {
        let mut identifiers = names
            .into_iter()
            .map(|name| name.to_string().to_ascii_lowercase())
            .collect::<Vec<_>>();
        identifiers.sort();
        identifiers.dedup();
        Self {
            acme_id,
            identifiers,
        }
    }

    /// The target of an ACME certificate: its entry and its subject names.
    pub fn of_cert(cert: &Cert) -> Option<Self> {
        let metadata = cert.metadata.as_ref()?;
        Some(Self::new(metadata.acme_id, &cert.san))
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
    pub target: AcmeTarget,
    pub identifiers: Vec<Identifier>,
    /// Key authorizations of the HTTP-01 challenges, by token.
    pub http_challenges: HashMap<String, String>,
    /// Digests of the TLS-ALPN-01 challenges, by domain name.
    pub tls_alpn_challenges: Vec<TlsAlpnChallenge>,
    challenge_type: ChallengeType,
    dns: Option<DnsChallenge>,
    pub order: Order,
}

struct DnsChallenge {
    provider: DnsProvider,
    resolver: Option<SocketAddr>,
    names: Vec<TxtName>,
    acme_exec: AcmeExecConfig,
}

#[derive(Default)]
struct Challenges {
    http: HashMap<String, String>,
    /// `(domain, TXT value)` pairs of the DNS-01 challenges.
    dns: Vec<(String, String)>,
    tls_alpn: Vec<TlsAlpnChallenge>,
}

impl AcmeOrder {
    pub async fn new(
        entry: &AcmeEntry,
        target: &AcmeTarget,
        dns_resolver: Option<SocketAddr>,
        acme_exec: &AcmeExecConfig,
    ) -> anyhow::Result<Self> {
        info!("requesting certificate");

        // An entry stored before validation existed can still hold a name the challenge cannot validate.
        let acme = entry.acme_for(target)?;
        let identifiers = acme
            .identifiers
            .iter()
            .map(|id| Identifier::Dns(id.to_string()))
            .collect::<Vec<_>>();
        let account: AccountCredentials =
            serde_json::from_str(&serde_json::to_string(&entry.account)?)?;
        let challenge_type = match entry.acme.challenge_type.as_str() {
            HTTP_01 => ChallengeType::Http01,
            DNS_01 => ChallengeType::Dns01,
            TLS_ALPN_01 => ChallengeType::TlsAlpn01,
            other => bail!("the {other} challenge is not supported"),
        };
        let account = Account::builder_with_http(acme_http_client().await?)
            .from_credentials(account)
            .await?;
        let mut order = account.new_order(&NewOrder::new(&identifiers)).await?;
        let challenges = collect_challenges(&mut order, &challenge_type).await?;
        let dns = match (&challenge_type, &entry.acme.dns_provider) {
            (ChallengeType::Dns01, Some(provider)) => Some(DnsChallenge {
                provider: provider.clone(),
                resolver: dns_resolver,
                names: dns::txt_names(challenges.dns),
                acme_exec: acme_exec.clone(),
            }),
            _ => None,
        };
        Ok(Self {
            target: target.clone(),
            identifiers,
            http_challenges: challenges.http,
            tls_alpn_challenges: challenges.tls_alpn,
            challenge_type,
            dns,
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
        let Some(dns) = self.dns.take() else {
            return self.complete().await;
        };
        let client = dns::client(&dns.provider, &dns.acme_exec).await?;
        let task = async {
            dns::wait_for_propagation(
                dns.resolver,
                &dns.names,
                dns::PROPAGATION_TIMEOUT,
                dns::PROPAGATION_INTERVAL,
            )
            .await?;
            self.complete().await
        };
        dns::with_txt_records(client.as_ref(), &dns.names, task).await
    }

    /// Tells the ACME server that the challenges are ready, waits for the validation,
    /// and downloads the certificate.
    async fn complete(&mut self) -> anyhow::Result<Cert> {
        self.set_challenges_ready().await?;
        self.wait_until_ready().await?;

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
            acme_id: self.target.acme_id,
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

    async fn wait_until_ready(&mut self) -> anyhow::Result<()> {
        let deadline = Instant::now() + VALIDATION_TIMEOUT;
        let mut interval = VALIDATION_MIN_INTERVAL;
        loop {
            let state = self.order.refresh().await?;
            match state.status {
                OrderStatus::Ready => return Ok(()),
                OrderStatus::Invalid => bail!("order is invalid"),
                _ => (),
            }
            if Instant::now() + interval > deadline {
                bail!("order is timed-out");
            }
            tokio::time::sleep(interval).await;
            interval = (interval * 2).min(VALIDATION_MAX_INTERVAL);
        }
    }
}

/// Collects the responses of the pending challenges of `challenge_type`.
async fn collect_challenges(
    order: &mut Order,
    challenge_type: &ChallengeType,
) -> anyhow::Result<Challenges> {
    let mut challenges = Challenges::default();
    let mut authorizations = order.authorizations();
    while let Some(authz) = authorizations.next().await {
        let mut authz = authz?;
        match authz.status {
            AuthorizationStatus::Pending => {}
            AuthorizationStatus::Valid => continue,
            _ => bail!("authorization status is not valid"),
        }
        let challenge = authz
            .challenge(challenge_type.clone())
            .ok_or_else(|| anyhow!("the ACME server offers no {challenge_type:?} challenge"))?;
        let key_authorization = challenge.key_authorization();
        match challenge_type {
            ChallengeType::Dns01 => {
                let domain = dns_domain(challenge.identifier().identifier)?;
                challenges.dns.push((domain, key_authorization.dns_value()));
            }
            ChallengeType::TlsAlpn01 => {
                let digest = key_authorization.digest();
                let identifier = challenge.identifier().identifier;
                challenges
                    .tls_alpn
                    .push(tls_alpn_challenge(identifier, digest.as_ref())?);
            }
            _ => {
                challenges.http.insert(
                    challenge.token.clone(),
                    key_authorization.as_str().to_string(),
                );
            }
        }
    }
    Ok(challenges)
}

/// The domain of a DNS identifier, without the wildcard label.
fn dns_domain(identifier: &Identifier) -> anyhow::Result<String> {
    match identifier {
        Identifier::Dns(domain) => Ok(domain.clone()),
        other => bail!("the DNS-01 challenge cannot validate {other:?}"),
    }
}

/// The TLS-ALPN-01 challenge of a DNS identifier with the SHA-256 digest of its key authorization.
fn tls_alpn_challenge(identifier: &Identifier, digest: &[u8]) -> anyhow::Result<TlsAlpnChallenge> {
    let Identifier::Dns(domain) = identifier else {
        bail!("the TLS-ALPN-01 challenge cannot validate {identifier:?}");
    };
    Ok(TlsAlpnChallenge {
        domain: domain.clone(),
        digest: digest.try_into()?,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use hyper::{
        header::{HeaderMap, LOCATION},
        Response,
    };
    use r3v3rs3_api::acme::{AcmeConfig, KeyedProvider, TokenApi, TokenProvider};
    use sha2::{Digest, Sha256};
    use std::sync::Mutex;

    #[test]
    fn a_dns_provider_survives_the_acme_toml_round_trip() {
        let acme = Acme {
            config: AcmeConfig::default(),
            identifiers: vec!["*.example.com".parse().unwrap()],
            challenge_type: DNS_01.to_string(),
            dns_provider: Some(DnsProvider::Keyed(KeyedProvider::Route53 {
                access_key_id: "AKID".to_string(),
                secret_access_key: "secret".to_string(),
                api_url: None,
            })),
        };
        let text = toml_edit::ser::to_document(&acme).unwrap().to_string();
        assert_eq!(toml::from_str::<Acme>(&text).unwrap(), acme);

        let token = Acme {
            dns_provider: Some(DnsProvider::Token(TokenProvider {
                provider: TokenApi::DigitalOcean,
                api_token: "token".to_string(),
                api_url: None,
            })),
            ..acme
        };
        let text = toml_edit::ser::to_document(&token).unwrap().to_string();
        assert!(text.contains("provider = \"digitalocean\""), "{text}");
        assert_eq!(toml::from_str::<Acme>(&text).unwrap(), token);
    }

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
            {"type": "http-01", "url": "https://acme.example/chall/1", "token": "http-token", "status": "pending"},
            {"type": "dns-01", "url": "https://acme.example/chall/3", "token": "dns-token", "status": "pending"},
            {"type": "tls-alpn-01", "url": "https://acme.example/chall/4", "token": "alpn-token", "status": "pending"}
        ]
    }"#;

    /// Answers the requests of an order for `example.com`.
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

    async fn challenges_of(challenge_type: ChallengeType) -> Challenges {
        let identifiers = [Identifier::Dns("example.com".to_string())];
        let mut order = stored_account()
            .await
            .new_order(&NewOrder::new(&identifiers))
            .await
            .unwrap();
        collect_challenges(&mut order, &challenge_type)
            .await
            .unwrap()
    }

    #[tokio::test]
    async fn a_challenge_without_a_token_does_not_break_the_order() {
        let challenges = challenges_of(ChallengeType::Http01).await;
        assert_eq!(challenges.http.keys().collect::<Vec<_>>(), ["http-token"]);
        assert!(challenges.http["http-token"].starts_with("http-token."));
        assert!(challenges.dns.is_empty());
    }

    #[tokio::test]
    async fn a_dns_01_order_collects_one_txt_value_per_domain() {
        let challenges = challenges_of(ChallengeType::Dns01).await;
        assert!(challenges.http.is_empty());
        assert_eq!(challenges.dns.len(), 1);
        assert_eq!(challenges.dns[0].0, "example.com");
        // The TXT value is the unpadded base64url SHA-256 digest: 43 characters.
        assert_eq!(challenges.dns[0].1.len(), 43);
    }

    #[tokio::test]
    async fn a_tls_alpn_01_order_collects_the_digest_of_the_key_authorization() {
        // A key authorization is `<token>.<account key thumbprint>`.
        let http = challenges_of(ChallengeType::Http01).await;
        let thumbprint = http.http["http-token"].trim_start_matches("http-token.");
        let expected: [u8; 32] = Sha256::digest(format!("alpn-token.{thumbprint}")).into();

        let challenges = challenges_of(ChallengeType::TlsAlpn01).await;
        assert!(challenges.http.is_empty());
        assert!(challenges.dns.is_empty());
        assert_eq!(
            challenges.tls_alpn,
            vec![TlsAlpnChallenge {
                domain: "example.com".to_string(),
                digest: expected,
            }]
        );
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
