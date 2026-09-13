use crate::id::ShortId;
use serde_default::DefaultFromSerde;
use serde_derive::{Deserialize, Serialize};
use utoipa::ToSchema;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "snake_case")]
pub enum TlsState {
    Active,
    /// The TLS configuration is invalid, so the port does not accept TLS connections.
    Error,
}

/// Whether the port asks the client for a certificate.
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "snake_case")]
pub enum ClientAuthMode {
    /// The port does not ask for a client certificate.
    #[default]
    Off,
    /// The port accepts clients without a certificate. A certificate that the client sends
    /// must be valid.
    Optional,
    /// The port rejects clients without a valid certificate.
    Required,
}

impl ClientAuthMode {
    pub fn is_off(&self) -> bool {
        *self == Self::Off
    }
}

#[derive(Debug, Clone, DefaultFromSerde, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
pub struct TlsTermination {
    #[serde(default)]
    #[schema(example = json!(["*.example.com"]))]
    pub server_names: Vec<String>,

    #[serde(default, skip_serializing_if = "ClientAuthMode::is_off")]
    pub client_auth: ClientAuthMode,

    /// Root certificates that verify the client certificates.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    #[schema(value_type = Vec<String>)]
    pub client_ca_certs: Vec<ShortId>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn client_auth_fields_are_optional() {
        let tls: TlsTermination = serde_json::from_str(r#"{"server_names":["a.com"]}"#).unwrap();
        assert_eq!(tls.client_auth, ClientAuthMode::Off);
        assert!(tls.client_ca_certs.is_empty());
        assert_eq!(
            serde_json::to_string(&tls).unwrap(),
            r#"{"server_names":["a.com"]}"#
        );

        let tls: TlsTermination =
            serde_json::from_str(r#"{"client_auth":"required","client_ca_certs":["abc"]}"#)
                .unwrap();
        assert_eq!(tls.client_auth, ClientAuthMode::Required);
        assert_eq!(tls.client_ca_certs, vec!["abc".parse().unwrap()]);
    }
}
