use serde_derive::{Deserialize, Serialize};
use thiserror::Error;
use utoipa::ToSchema;

use crate::{id::ShortId, multiaddr::Multiaddr};

#[derive(Serialize, Deserialize, ToSchema)]
pub struct ErrorMessage {
    pub message: String,
    pub error: Option<Error>,
}

#[derive(Debug, Clone, Error, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "snake_case", tag = "message")]
#[non_exhaustive]
pub enum Error {
    #[error("invalid listening address: {addr}")]
    InvalidListeningAddress {
        #[schema(value_type = String)]
        addr: Multiaddr,
    },

    #[error("invalid server address: {addr}")]
    InvalidServerAddress {
        #[schema(value_type = String)]
        addr: Multiaddr,
    },

    #[error("invalid subject name: {name}")]
    InvalidSubjectName { name: String },

    #[error("invalid virtual host: {host}")]
    InvalidVirtualHost { host: String },

    #[error("invalid server url: {url}")]
    InvalidServerUrl { url: String },

    #[error("invalid CIDR block: {cidr}")]
    InvalidCidr { cidr: String },

    #[error("failed to refresh CDN IP ranges")]
    FailedToRefreshCdnRanges,

    #[error("invalid multiaddr: {addr}")]
    InvalidMultiaddr { addr: String },

    #[error("missing TLS termination config")]
    TlsTerminationConfigMissing,

    #[error("certificate is in use: {id}")]
    CertificateInUse { id: ShortId },

    #[error("client CA certificate must be a root certificate: {id}")]
    InvalidClientCaCert { id: ShortId },

    #[error("client authentication needs at least one root certificate")]
    ClientCaCertsMissing,

    #[error("upstream client certificate must be a client certificate with a private key: {id}")]
    InvalidClientCert { id: ShortId },

    #[error("failed to generate self-signed certificate")]
    FailedToGenerateSelfSignedCertificate,

    #[error("failed to read certificate")]
    FailedToReadCertificate,

    #[error("failed to read private key")]
    FailedToReadPrivateKey,

    #[error("invalid short id: {id}")]
    InvalidShortId { id: String },

    #[error("port id not found: {id}")]
    IdNotFound { id: String },

    #[error("port id already exists: {id}")]
    IdAlreadyExists { id: ShortId },

    #[error("proxy is managed by service discovery and cannot be changed: {id}")]
    ProxyReadOnly { id: ShortId },

    #[error("certificate is managed by service discovery and cannot be deleted: {id}")]
    CertificateReadOnly { id: ShortId },

    #[error("invalid service discovery settings: {reason}")]
    InvalidDiscoveryConfig { reason: String },

    #[error("acme account creation failed")]
    AcmeAccountCreationFailed,

    #[error("unsupported acme challenge: {challenge}")]
    AcmeUnsupportedChallenge { challenge: String },

    #[error("acme request needs at least one domain name")]
    AcmeIdentifiersMissing,

    #[error("invalid acme domain name: {identifier}")]
    AcmeInvalidIdentifier { identifier: String },

    #[error("wildcard domain name needs the dns-01 challenge: {identifier}")]
    AcmeWildcardNeedsDnsChallenge { identifier: String },

    #[error("dns-01 challenge needs a dns provider with credentials")]
    AcmeDnsProviderRequired,

    #[error("unauthorized")]
    Unauthorized,

    #[error("failed to create account")]
    FailedToCreateAccount,

    #[error("invalid login credentials")]
    InvalidLoginCredentials,

    #[error("too many login attempts")]
    TooManyLoginAttempts,

    #[error("invalid username: {username}")]
    InvalidUsername { username: String },

    #[error("password is required for user: {username}")]
    PasswordRequired { username: String },

    #[error("invalid token name: {name}")]
    InvalidTokenName { name: String },

    #[error("token is missing or shorter than 16 characters: {name}")]
    InvalidToken { name: String },

    #[error("invalid header name: {name}")]
    InvalidHeaderName { name: String },

    #[error("header cannot be changed: {name}")]
    ProtectedHeader { name: String },

    #[error("invalid header value: {value}")]
    InvalidHeaderValue { value: String },

    #[error("invalid header rule: {rule}")]
    InvalidHeaderRule { rule: String },

    #[error("invalid media type: {mime}")]
    InvalidMimeType { mime: String },

    #[error("compression algorithm is listed more than once: {algorithm}")]
    DuplicateCompressionAlgorithm { algorithm: String },

    #[error("maximum response size must be from 1 byte to the memory limit")]
    InvalidCacheSize,

    #[error("timeout must be greater than zero")]
    InvalidTimeout,

    #[error("health check path must start with / and only HTTP proxies use it: {path}")]
    InvalidHealthCheckPath { path: String },

    #[error("failed to hash password")]
    FailedToHashPassword,

    #[error("failed to fetch log")]
    FailedToFetchLog,

    #[error("failed to invoke rpc")]
    FailedToInvokeRpc,

    #[error("failed to list network interfaces")]
    FailedToListNetworkInterfaces,
}

impl Error {
    pub fn status_code(&self) -> u16 {
        match self {
            Self::IdNotFound { .. } => 404,
            Self::Unauthorized => 401,
            Self::ProxyReadOnly { .. } | Self::CertificateReadOnly { .. } => 403,
            Self::TooManyLoginAttempts => 429,
            Self::FailedToFetchLog | Self::FailedToInvokeRpc | Self::FailedToHashPassword => 500,
            _ => 400,
        }
    }
}
