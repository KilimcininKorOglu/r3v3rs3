use crate::error::Error;
use crate::policy::is_header_name;
use serde_derive::{Deserialize, Serialize};
use std::fmt;
use utoipa::ToSchema;

/// Headers that control the connection or the message framing. Rules cannot change them.
pub const PROTECTED_HEADERS: [&str; 9] = [
    "connection",
    "content-length",
    "host",
    "keep-alive",
    "proxy-connection",
    "te",
    "trailer",
    "transfer-encoding",
    "upgrade",
];

/// Header changes for proxied requests and responses.
#[derive(Debug, Default, Clone, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
pub struct HeaderRules {
    /// Changes to the request that r3v3rs3 sends to the upstream server.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub request: Vec<HeaderRule>,

    /// Changes to the upstream response that r3v3rs3 sends to the client.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub response: Vec<HeaderRule>,
}

impl HeaderRules {
    pub fn is_empty(&self) -> bool {
        self.request.is_empty() && self.response.is_empty()
    }

    pub fn validate(&self) -> Result<(), Error> {
        self.request
            .iter()
            .chain(&self.response)
            .try_for_each(HeaderRule::validate)
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
#[serde(tag = "action", rename_all = "snake_case")]
pub enum HeaderRule {
    /// Replaces every value of the header.
    Set { name: String, value: String },
    /// Adds a value and keeps the existing values.
    Append { name: String, value: String },
    /// Removes every value of the header.
    Remove { name: String },
}

impl HeaderRule {
    pub fn name(&self) -> &str {
        match self {
            Self::Set { name, .. } | Self::Append { name, .. } | Self::Remove { name } => name,
        }
    }

    pub fn validate(&self) -> Result<(), Error> {
        let name = self.name();
        if !is_header_name(name) {
            return Err(Error::InvalidHeaderName { name: name.into() });
        }
        if PROTECTED_HEADERS
            .iter()
            .any(|protected| protected.eq_ignore_ascii_case(name))
        {
            return Err(Error::ProtectedHeader { name: name.into() });
        }
        match self {
            Self::Set { value, .. } | Self::Append { value, .. } => {
                parse_header_template(value).map(|_| ())
            }
            Self::Remove { .. } => Ok(()),
        }
    }

    /// Parses one rule: `set Name: value`, `append Name: value` or `remove Name`.
    pub fn parse_line(line: &str) -> Result<Self, Error> {
        let line = line.trim();
        let invalid = || Error::InvalidHeaderRule { rule: line.into() };
        let (action, rest) = line.split_once(char::is_whitespace).ok_or_else(invalid)?;
        let action = action.to_ascii_lowercase();
        let rule = if action == "remove" {
            Self::Remove {
                name: rest.trim().into(),
            }
        } else {
            let (name, value) = rest.split_once(':').ok_or_else(invalid)?;
            let (name, value) = (name.trim().to_string(), value.trim().to_string());
            match action.as_str() {
                "set" => Self::Set { name, value },
                "append" => Self::Append { name, value },
                _ => return Err(invalid()),
            }
        };
        rule.validate()?;
        Ok(rule)
    }
}

impl fmt::Display for HeaderRule {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Set { name, value } => write!(f, "set {name}: {value}"),
            Self::Append { name, value } => write!(f, "append {name}: {value}"),
            Self::Remove { name } => write!(f, "remove {name}"),
        }
    }
}

/// Parses one rule per line. Empty lines and lines that start with `#` are skipped.
pub fn parse_header_rules(text: &str) -> Result<Vec<HeaderRule>, String> {
    text.lines()
        .enumerate()
        .filter(|(_, line)| {
            let line = line.trim();
            !line.is_empty() && !line.starts_with('#')
        })
        .map(|(index, line)| {
            HeaderRule::parse_line(line).map_err(|err| format!("Line {}: {err}", index + 1))
        })
        .collect()
}

pub fn format_header_rules(rules: &[HeaderRule]) -> String {
    rules
        .iter()
        .map(ToString::to_string)
        .collect::<Vec<_>>()
        .join("\n")
}

/// A value that a header rule can insert.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HeaderVariable {
    ClientIp,
    Host,
    Scheme,
    RequestId,
    Route,
    ClientCertSubject,
    ClientCertFingerprint,
}

impl HeaderVariable {
    pub const ALL: [Self; 7] = [
        Self::ClientIp,
        Self::Host,
        Self::Scheme,
        Self::RequestId,
        Self::Route,
        Self::ClientCertSubject,
        Self::ClientCertFingerprint,
    ];

    pub fn name(self) -> &'static str {
        match self {
            Self::ClientIp => "client_ip",
            Self::Host => "host",
            Self::Scheme => "scheme",
            Self::RequestId => "request_id",
            Self::Route => "route",
            Self::ClientCertSubject => "client_cert_subject",
            Self::ClientCertFingerprint => "client_cert_fingerprint",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TemplatePart {
    Literal(String),
    Variable(HeaderVariable),
}

/// Parses a header value with `{variable}` placeholders. `{{` and `}}` write a literal brace.
pub fn parse_header_template(value: &str) -> Result<Vec<TemplatePart>, Error> {
    if value.chars().any(|c| c.is_control() && c != '\t') {
        return Err(invalid_header_value(value));
    }
    let mut parts = Vec::new();
    let mut literal = String::new();
    let mut rest = value;
    while let Some(index) = rest.find(['{', '}']) {
        let (text, tail) = rest.split_at(index);
        literal.push_str(text);
        rest = parse_brace(value, tail, &mut parts, &mut literal)?;
    }
    literal.push_str(rest);
    push_literal(&mut parts, literal);
    Ok(parts)
}

fn invalid_header_value(value: &str) -> Error {
    Error::InvalidHeaderValue {
        value: value.into(),
    }
}

/// Reads one brace of `tail`. An escaped brace goes to `literal`, and a placeholder becomes a
/// variable part. Returns the rest of the template after the brace.
fn parse_brace<'a>(
    value: &str,
    tail: &'a str,
    parts: &mut Vec<TemplatePart>,
    literal: &mut String,
) -> Result<&'a str, Error> {
    if tail.starts_with("{{") || tail.starts_with("}}") {
        literal.push_str(&tail[..1]);
        return Ok(&tail[2..]);
    }
    let (variable, after) = tail
        .strip_prefix('{')
        .and_then(|inner| inner.split_once('}'))
        .and_then(|(name, after)| Some((variable_named(name)?, after)))
        .ok_or_else(|| invalid_header_value(value))?;
    push_literal(parts, std::mem::take(literal));
    parts.push(TemplatePart::Variable(variable));
    Ok(after)
}

/// Adds the text as a literal part. An empty text adds no part.
fn push_literal(parts: &mut Vec<TemplatePart>, literal: String) {
    if !literal.is_empty() {
        parts.push(TemplatePart::Literal(literal));
    }
}

fn variable_named(name: &str) -> Option<HeaderVariable> {
    HeaderVariable::ALL
        .into_iter()
        .find(|variable| variable.name() == name)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rules_are_parsed_and_formatted() {
        let text = "set X-Client: {client_ip}\n\n# comment\nappend Vary: Accept\nREMOVE Server";
        let rules = parse_header_rules(text).unwrap();
        assert_eq!(
            rules,
            vec![
                HeaderRule::Set {
                    name: "X-Client".into(),
                    value: "{client_ip}".into()
                },
                HeaderRule::Append {
                    name: "Vary".into(),
                    value: "Accept".into()
                },
                HeaderRule::Remove {
                    name: "Server".into()
                },
            ]
        );
        assert_eq!(
            format_header_rules(&rules),
            "set X-Client: {client_ip}\nappend Vary: Accept\nremove Server"
        );
        assert_eq!(parse_header_rules(&format_header_rules(&rules)), Ok(rules));
    }

    #[test]
    fn invalid_rules_report_the_line() {
        assert_eq!(
            parse_header_rules("set A: b\nreplace X: y"),
            Err("Line 2: invalid header rule: replace X: y".into())
        );
        assert!(parse_header_rules("set X-Missing-Colon").is_err());
        assert!(parse_header_rules("remove").is_err());
        assert!(parse_header_rules("set Bad Name: x").is_err());
        assert!(parse_header_rules("remove Content-Length").is_err());
        assert!(parse_header_rules("set X-Id: {unknown}").is_err());
    }

    #[test]
    fn template_parts() {
        assert_eq!(
            parse_header_template("{scheme}://{host}{{x}}").unwrap(),
            vec![
                TemplatePart::Variable(HeaderVariable::Scheme),
                TemplatePart::Literal("://".into()),
                TemplatePart::Variable(HeaderVariable::Host),
                TemplatePart::Literal("{x}".into()),
            ]
        );
        assert_eq!(
            parse_header_template("{client_cert_subject}/{client_cert_fingerprint}").unwrap(),
            vec![
                TemplatePart::Variable(HeaderVariable::ClientCertSubject),
                TemplatePart::Literal("/".into()),
                TemplatePart::Variable(HeaderVariable::ClientCertFingerprint),
            ]
        );
        assert!(parse_header_template("").unwrap().is_empty());
        for value in ["{host", "host}", "{}", "a\nb"] {
            assert!(parse_header_template(value).is_err(), "{value}");
        }
    }

    #[test]
    fn serde_uses_action_tag() {
        let rules: HeaderRules = serde_json::from_str(
            r#"{"request":[{"action":"set","name":"X-A","value":"1"}],"response":[{"action":"remove","name":"Server"}]}"#,
        )
        .unwrap();
        assert_eq!(rules.request[0].to_string(), "set X-A: 1");
        assert_eq!(rules.response[0].to_string(), "remove Server");
        assert!(HeaderRules::default().is_empty());
        assert!(rules.validate().is_ok());
    }
}
