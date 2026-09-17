//! A response that a route sends itself instead of an upstream server, like a redirect host or a
//! 404 host.

use crate::error::Error;
use crate::redirect::{RedirectStatus, validate_target};
use serde_derive::{Deserialize, Serialize};
use utoipa::ToSchema;

/// The status codes of a fixed status response.
pub const FIXED_STATUSES: [u16; 10] = [200, 400, 403, 404, 410, 429, 451, 500, 502, 503];

/// The longest body of a fixed status response in bytes.
pub const MAX_FIXED_BODY_SIZE: usize = 4096;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
#[serde(tag = "type", rename_all = "snake_case", try_from = "RawFixedResponse")]
pub enum FixedResponse {
    /// Redirects every request of the route.
    Redirect(FixedRedirect),
    /// Answers every request of the route with a status code and a plain text body.
    Status(FixedStatus),
}

impl FixedResponse {
    /// Rejects an empty target, a target with a control character, another status code and a
    /// body longer than 4096 bytes.
    pub fn validate(&self) -> Result<(), Error> {
        match self {
            Self::Redirect(redirect) => validate_target(&redirect.target),
            Self::Status(status) => status.validate(),
        }
    }
}

#[derive(Deserialize)]
#[serde(rename_all = "snake_case")]
enum FixedKind {
    Redirect,
    Status,
}

/// The fields of every fixed response type with their types. A tagged enum reads its fields
/// without a type hint, and service discovery labels then give every value as text.
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct RawFixedResponse {
    #[serde(rename = "type")]
    kind: FixedKind,
    target: Option<String>,
    status: Option<u16>,
    preserve_path: Option<bool>,
    body: Option<String>,
}

impl TryFrom<RawFixedResponse> for FixedResponse {
    type Error = String;

    fn try_from(raw: RawFixedResponse) -> Result<Self, Self::Error> {
        match raw.kind {
            FixedKind::Redirect => raw.into_redirect().map(Self::Redirect),
            FixedKind::Status => raw.into_status().map(Self::Status),
        }
    }
}

impl RawFixedResponse {
    fn into_redirect(self) -> Result<FixedRedirect, String> {
        if self.body.is_some() {
            return Err("body is available only for a status response".into());
        }
        let status = self
            .status
            .map(RedirectStatus::try_from)
            .transpose()
            .map_err(|err| err.to_string())?;
        Ok(FixedRedirect {
            target: self.target.ok_or("a redirect needs target")?,
            status: status.unwrap_or_default(),
            preserve_path: self.preserve_path.unwrap_or_else(default_preserve_path),
        })
    }

    fn into_status(self) -> Result<FixedStatus, String> {
        if self.target.is_some() || self.preserve_path.is_some() {
            return Err("target and preserve_path are available only for a redirect".into());
        }
        Ok(FixedStatus {
            status: self.status.ok_or("a status response needs status")?,
            body: self.body.unwrap_or_default(),
        })
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
pub struct FixedRedirect {
    /// The `Location` of the redirect.
    #[schema(example = "https://example.com")]
    pub target: String,
    #[serde(default)]
    #[schema(value_type = u16, example = 301)]
    pub status: RedirectStatus,
    /// Appends the path and the query of the request to the target.
    #[serde(default = "default_preserve_path", skip_serializing_if = "is_true")]
    pub preserve_path: bool,
}

fn default_preserve_path() -> bool {
    true
}

fn is_true(value: &bool) -> bool {
    *value
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
pub struct FixedStatus {
    #[schema(example = 404)]
    pub status: u16,
    /// The plain text body of the response.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub body: String,
}

impl FixedStatus {
    fn validate(&self) -> Result<(), Error> {
        if !FIXED_STATUSES.contains(&self.status) {
            return Err(Error::InvalidFixedStatus {
                status: self.status,
            });
        }
        if self.body.len() > MAX_FIXED_BODY_SIZE {
            return Err(Error::FixedBodyTooLarge {
                max: MAX_FIXED_BODY_SIZE,
            });
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::proxy::{HttpProxy, ProxyKind, Route, Server};
    use serde_json::json;

    fn validate(route: Route) -> Result<(), Error> {
        let http = HttpProxy {
            routes: vec![route],
            ..Default::default()
        };
        ProxyKind::Http(Box::new(http)).validate_upstream()
    }

    #[test]
    fn fixed_responses_have_a_type_and_defaults() {
        let redirect: FixedResponse =
            serde_json::from_value(json!({ "type": "redirect", "target": "https://a.test" }))
                .unwrap();
        let expected = FixedRedirect {
            target: "https://a.test".into(),
            status: RedirectStatus::Found,
            preserve_path: true,
        };
        assert_eq!(redirect, FixedResponse::Redirect(expected));
        assert_eq!(
            serde_json::to_value(&redirect).unwrap(),
            json!({ "type": "redirect", "target": "https://a.test", "status": 302 })
        );
        let status: FixedResponse =
            serde_json::from_value(json!({ "type": "status", "status": 404 })).unwrap();
        let expected = FixedStatus {
            status: 404,
            body: String::new(),
        };
        assert_eq!(status, FixedResponse::Status(expected));

        let invalid = [
            json!({ "type": "redirect" }),
            json!({ "type": "redirect", "target": "/", "body": "x" }),
            json!({ "type": "redirect", "target": "/", "status": 404 }),
            json!({ "type": "status" }),
            json!({ "type": "status", "status": 404, "target": "/" }),
            json!({ "type": "status", "status": 404, "other": 1 }),
            json!({ "type": "proxy" }),
        ];
        for value in invalid {
            assert!(
                serde_json::from_value::<FixedResponse>(value.clone()).is_err(),
                "{value}"
            );
        }
    }

    #[test]
    fn a_route_cannot_have_servers_and_an_invalid_fixed_response() {
        let not_found = FixedResponse::Status(FixedStatus {
            status: 404,
            body: "gone".into(),
        });
        let fixed = Route {
            response: Some(not_found),
            ..Default::default()
        };
        assert!(validate(fixed.clone()).is_ok());
        let both = Route {
            servers: vec![Server::new("http://127.0.0.1:9000/".parse().unwrap())],
            ..fixed
        };
        assert!(matches!(validate(both), Err(Error::RouteResponseConflict)));
        // A discovered backend without endpoints keeps its route without servers.
        assert!(validate(Route::default()).is_ok());

        let teapot = FixedResponse::Status(FixedStatus {
            status: 418,
            body: String::new(),
        });
        assert!(matches!(
            teapot.validate(),
            Err(Error::InvalidFixedStatus { status: 418 })
        ));
        let large = FixedResponse::Status(FixedStatus {
            status: 404,
            body: "a".repeat(MAX_FIXED_BODY_SIZE + 1),
        });
        assert!(matches!(
            large.validate(),
            Err(Error::FixedBodyTooLarge { max: 4096 })
        ));
        let empty = FixedResponse::Redirect(FixedRedirect {
            target: String::new(),
            status: RedirectStatus::Found,
            preserve_path: true,
        });
        assert!(matches!(
            empty.validate(),
            Err(Error::InvalidRedirectTarget { .. })
        ));
    }
}
