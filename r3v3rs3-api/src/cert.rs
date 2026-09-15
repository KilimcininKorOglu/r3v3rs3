use crate::id::ShortId;
use crate::subject_name::SubjectName;
use serde_default::DefaultFromSerde;
use serde_derive::{Deserialize, Serialize};
use std::fmt;
use std::time::{Duration, SystemTime, UNIX_EPOCH};
use utoipa::{IntoParams, ToSchema};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "snake_case")]
pub enum CertKind {
    Server,
    Client,
    Root,
}

impl fmt::Display for CertKind {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "{}",
            match self {
                CertKind::Server => "server",
                CertKind::Client => "client",
                CertKind::Root => "root",
            }
        )
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
pub struct CertInfo {
    #[schema(example = "a13e1ecc080e42cfcdd5")]
    pub id: ShortId,
    pub kind: CertKind,
    #[schema(example = "a13e1ecc080e42cfcdd5b77fec8450c777554aa7269c029b242a7c548d0d73da")]
    pub fingerprint: String,
    #[schema(example = "CN=r3v3rs3 self signed cert")]
    pub issuer: String,
    pub root_cert: Option<String>,
    #[schema(value_type = [String], example = json!(["localhost"]))]
    pub san: Vec<SubjectName>,
    #[schema(example = "67090118400")]
    pub not_after: i64,
    #[schema(example = "157766400")]
    pub not_before: i64,
    pub is_ca: bool,
    pub has_private_key: bool,
    pub metadata: Option<CertMetadata>,
    /// The service discovery provider that read the certificate, for example from a Kubernetes
    /// TLS secret. A discovered certificate is read-only and is not saved.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub source: Option<crate::discovery::DiscoverySource>,
}

/// How close a certificate is to its expiry.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ExpiryState {
    Valid,
    /// The certificate expires within the warning time.
    Expiring,
    Expired,
}

/// The expiry state of a certificate at the Unix time `now`.
pub fn expiry_state(not_after: i64, now: i64, warning: Duration) -> ExpiryState {
    let warning = i64::try_from(warning.as_secs()).unwrap_or(i64::MAX);
    if not_after <= now {
        ExpiryState::Expired
    } else if not_after <= now.saturating_add(warning) {
        ExpiryState::Expiring
    } else {
        ExpiryState::Valid
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
pub struct SelfSignedCertRequest {
    #[schema(value_type = [String], example = json!(["localhost"]))]
    pub san: Vec<SubjectName>,
    #[schema(example = "f9cf7e3faa1aca7e6086")]
    pub ca_cert: Option<ShortId>,
    #[serde(default)]
    pub kind: SelfSignedCertKind,
}

/// The kind of a self-signed certificate. A client certificate has the `clientAuth` extended key usage.
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "snake_case")]
pub enum SelfSignedCertKind {
    #[default]
    Server,
    Client,
}

impl From<SelfSignedCertKind> for CertKind {
    fn from(kind: SelfSignedCertKind) -> Self {
        match kind {
            SelfSignedCertKind::Server => CertKind::Server,
            SelfSignedCertKind::Client => CertKind::Client,
        }
    }
}

#[derive(DefaultFromSerde, Clone, Serialize, Deserialize, IntoParams)]
#[into_params(parameter_in = Query)]
pub struct UploadQuery {
    #[serde(default = "default_cert_kind")]
    pub kind: CertKind,
}

fn default_cert_kind() -> CertKind {
    CertKind::Server
}

/// The most certificates that one request deletes.
pub const MAX_DELETE_CERTS: usize = 200;

/// The certificates to delete.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
pub struct DeleteCertsRequest {
    #[schema(example = json!(["a13e1ecc080e42cfcdd5"]))]
    pub ids: Vec<ShortId>,
}

/// What happened to one certificate of a delete request.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "snake_case")]
pub enum DeleteCertStatus {
    Deleted,
    /// A port, a proxy or a discovery provider uses the certificate.
    InUse,
    /// Service discovery manages the certificate.
    ReadOnly,
    NotFound,
    /// The storage did not delete the certificate. The server log names the cause.
    Failed,
}

/// The result of one certificate of a delete request, in the order of the request.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
pub struct DeleteCertResult {
    pub id: ShortId,
    pub status: DeleteCertStatus,
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize, Serialize, ToSchema)]
pub struct CertMetadata {
    pub acme_id: ShortId,
    #[serde(
        serialize_with = "serialize_created_at",
        deserialize_with = "deserialize_created_at"
    )]
    #[schema(value_type = u64)]
    pub created_at: SystemTime,
}

fn serialize_created_at<S>(time: &SystemTime, serializer: S) -> Result<S::Ok, S::Error>
where
    S: serde::Serializer,
{
    let timestamp = time
        .duration_since(UNIX_EPOCH)
        .map_err(|_| serde::ser::Error::custom("invalid timestamp"))?;
    serializer.serialize_u64(timestamp.as_secs())
}

fn deserialize_created_at<'de, D>(deserializer: D) -> Result<SystemTime, D::Error>
where
    D: serde::Deserializer<'de>,
{
    use serde::Deserialize;
    let timestamp = u64::deserialize(deserializer)?;
    Ok(UNIX_EPOCH + Duration::from_secs(timestamp))
}

#[derive(ToSchema)]
pub struct CertPostBody {
    #[schema(format = Binary)]
    pub chain: String,
    #[schema(format = Binary)]
    pub key: String,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_certificate_expires_at_its_not_after_time_and_warns_before_it() {
        let day = Duration::from_secs(24 * 60 * 60);
        let now = 1_000_000;
        let one_day = 24 * 60 * 60;
        assert_eq!(expiry_state(now - 1, now, day), ExpiryState::Expired);
        assert_eq!(expiry_state(now, now, day), ExpiryState::Expired);
        assert_eq!(expiry_state(now + 1, now, day), ExpiryState::Expiring);
        assert_eq!(expiry_state(now + one_day, now, day), ExpiryState::Expiring);
        assert_eq!(
            expiry_state(now + one_day + 1, now, day),
            ExpiryState::Valid
        );
        // No warning time marks no certificate as expiring.
        assert_eq!(
            expiry_state(now + 1, now, Duration::ZERO),
            ExpiryState::Valid
        );
        assert_eq!(
            expiry_state(i64::MAX, now, Duration::MAX),
            ExpiryState::Expiring
        );
    }
}
