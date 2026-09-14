//! Answers a request with a redirect before r3v3rs3 sends it to an upstream server.

use crate::error::Error;
use crate::rewrite::PathRegex;
use serde_derive::{Deserialize, Serialize};
use std::fmt;
use utoipa::ToSchema;

/// Redirects the requests whose `host/path?query` matches the regex.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
pub struct RedirectRule {
    /// Matches the host without the port, the path and the query, e.g.
    /// `example.com/old/page?id=1`.
    #[schema(value_type = String, example = "^example\\.com/old/(.*)$")]
    pub regex: PathRegex,

    /// The `Location` of the redirect. `${1}` or `${name}` inserts a capture group.
    #[schema(example = "https://example.com/new/${1}")]
    pub target: String,

    #[serde(default)]
    #[schema(value_type = u16, example = 302)]
    pub status: RedirectStatus,
}

impl RedirectRule {
    /// Rejects an empty target and a target with a control character.
    pub fn validate(&self) -> Result<(), Error> {
        if self.target.is_empty() || self.target.chars().any(char::is_control) {
            return Err(Error::InvalidRedirectTarget {
                target: self.target.clone(),
            });
        }
        Ok(())
    }

    /// The `Location` for the `host/path?query` of a request, or `None` when the rule does not
    /// match.
    pub fn location(&self, subject: &str) -> Option<String> {
        self.regex.expand(subject, &self.target)
    }

    /// Parses one rule: `<status> <regex> <target>`, e.g.
    /// `301 ^example\.com/old$ https://example.com/new`.
    pub fn parse_line(line: &str) -> Result<Self, Error> {
        let line = line.trim();
        let invalid = || Error::InvalidRedirectRule { rule: line.into() };
        let mut parts = line.split_whitespace();
        let (Some(status), Some(regex), Some(target), None) =
            (parts.next(), parts.next(), parts.next(), parts.next())
        else {
            return Err(invalid());
        };
        let status = status.parse::<u16>().map_err(|_| invalid())?;
        let rule = Self {
            regex: regex.parse()?,
            target: target.into(),
            status: status.try_into()?,
        };
        rule.validate()?;
        Ok(rule)
    }
}

impl fmt::Display for RedirectRule {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{} {} {}", self.status.code(), self.regex, self.target)
    }
}

/// The status code of a redirect.
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(try_from = "u16", into = "u16")]
pub enum RedirectStatus {
    MovedPermanently,
    #[default]
    Found,
    TemporaryRedirect,
    PermanentRedirect,
}

impl RedirectStatus {
    pub const ALL: [Self; 4] = [
        Self::MovedPermanently,
        Self::Found,
        Self::TemporaryRedirect,
        Self::PermanentRedirect,
    ];

    pub fn code(self) -> u16 {
        match self {
            Self::MovedPermanently => 301,
            Self::Found => 302,
            Self::TemporaryRedirect => 307,
            Self::PermanentRedirect => 308,
        }
    }
}

impl From<RedirectStatus> for u16 {
    fn from(status: RedirectStatus) -> Self {
        status.code()
    }
}

impl TryFrom<u16> for RedirectStatus {
    type Error = Error;

    fn try_from(code: u16) -> Result<Self, Self::Error> {
        Self::ALL
            .into_iter()
            .find(|status| status.code() == code)
            .ok_or(Error::InvalidRedirectStatus { status: code })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn rules_parse_format_and_expand() {
        let line = r"301 ^example\.com/old/(.*)$ https://example.com/new/${1}";
        let rule = RedirectRule::parse_line(line).unwrap();
        assert_eq!(rule.status, RedirectStatus::MovedPermanently);
        assert_eq!(
            rule.location("example.com/old/a?b=1").as_deref(),
            Some("https://example.com/new/a?b=1")
        );
        assert_eq!(rule.location("example.com/other"), None);
        assert_eq!(rule.to_string(), line);
        assert_eq!(RedirectRule::parse_line(&rule.to_string()).unwrap(), rule);

        let found: RedirectRule =
            serde_json::from_value(json!({ "regex": "^a$", "target": "/b" })).unwrap();
        assert_eq!(found.status, RedirectStatus::Found);
        assert_eq!(
            serde_json::to_value(&found).unwrap(),
            json!({ "regex": "^a$", "target": "/b", "status": 302 })
        );
        let other = json!({ "regex": "^a$", "target": "/b", "status": 303 });
        assert!(serde_json::from_value::<RedirectRule>(other).is_err());
    }

    #[test]
    fn invalid_rules_are_rejected() {
        assert!(matches!(
            RedirectRule::parse_line("303 ^a$ /b"),
            Err(Error::InvalidRedirectStatus { status: 303 })
        ));
        for line in ["301 ^a$", "30x ^a$ /b", "301 ^a$ /b extra"] {
            assert!(
                matches!(
                    RedirectRule::parse_line(line),
                    Err(Error::InvalidRedirectRule { .. })
                ),
                "{line}"
            );
        }
        assert!(matches!(
            RedirectRule::parse_line("301 ( /b"),
            Err(Error::InvalidPathRegex { .. })
        ));
        let control = RedirectRule {
            regex: "^a$".parse().unwrap(),
            target: "/a\nb".into(),
            status: RedirectStatus::Found,
        };
        assert!(matches!(
            control.validate(),
            Err(Error::InvalidRedirectTarget { .. })
        ));
    }
}
