use serde_derive::{Deserialize, Serialize};
use std::fmt;
use utoipa::ToSchema;

/// A CDN or load balancer whose edge IP ranges are known.
#[derive(
    Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize, ToSchema,
)]
#[serde(rename_all = "snake_case")]
pub enum CdnProvider {
    Cloudflare,
    Fastly,
    Cloudfront,
    Bunny,
    Gcore,
    Keycdn,
    Imperva,
    GoogleCloud,
}

impl CdnProvider {
    pub const ALL: &'static [CdnProvider] = &[
        CdnProvider::Cloudflare,
        CdnProvider::Fastly,
        CdnProvider::Cloudfront,
        CdnProvider::Bunny,
        CdnProvider::Gcore,
        CdnProvider::Keycdn,
        CdnProvider::Imperva,
        CdnProvider::GoogleCloud,
    ];
}

impl fmt::Display for CdnProvider {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let name = match self {
            CdnProvider::Cloudflare => "Cloudflare",
            CdnProvider::Fastly => "Fastly",
            CdnProvider::Cloudfront => "Amazon CloudFront",
            CdnProvider::Bunny => "Bunny CDN",
            CdnProvider::Gcore => "Gcore",
            CdnProvider::Keycdn => "KeyCDN",
            CdnProvider::Imperva => "Imperva",
            CdnProvider::GoogleCloud => "Google Cloud Load Balancing",
        };
        f.write_str(name)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "snake_case")]
pub enum CdnRangesSource {
    /// The snapshot compiled into the binary.
    Embedded,
    /// A list downloaded from the providers.
    Downloaded,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
pub struct CdnProviderStatus {
    pub provider: CdnProvider,
    pub ranges: usize,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
pub struct CdnStatus {
    /// Unix time in seconds of the last successful update.
    pub updated_at: i64,
    pub source: CdnRangesSource,
    pub providers: Vec<CdnProviderStatus>,
    /// Errors of the last refresh attempt.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub errors: Vec<String>,
}
