use hyper::header::{HeaderMap, HeaderName, HeaderValue};
use r3v3rs3_api::{
    error::Error,
    header_rules::{parse_header_template, HeaderRule, HeaderRules, HeaderVariable, TemplatePart},
};
use std::{borrow::Cow, net::IpAddr, str::FromStr};
use tracing::error;

/// Header rules with parsed names and value templates.
#[derive(Debug, Default)]
pub struct CompiledHeaderRules {
    request: Vec<CompiledRule>,
    response: Vec<CompiledRule>,
}

#[derive(Debug)]
struct CompiledRule {
    name: HeaderName,
    action: Action,
}

#[derive(Debug)]
enum Action {
    Set(Vec<TemplatePart>),
    Append(Vec<TemplatePart>),
    Remove,
}

/// Values of the template variables for one request.
#[derive(Debug, Clone)]
pub struct HeaderVariables {
    pub client_ip: IpAddr,
    pub host: String,
    pub scheme: &'static str,
    pub request_id: String,
    pub route: String,
    /// Empty when the client sent no certificate.
    pub client_cert_subject: String,
    /// Empty when the client sent no certificate.
    pub client_cert_fingerprint: String,
}

impl HeaderVariables {
    fn value(&self, variable: HeaderVariable) -> Cow<'_, str> {
        match variable {
            HeaderVariable::ClientIp => Cow::Owned(self.client_ip.to_string()),
            HeaderVariable::Host => Cow::Borrowed(&self.host),
            HeaderVariable::Scheme => Cow::Borrowed(self.scheme),
            HeaderVariable::RequestId => Cow::Borrowed(&self.request_id),
            HeaderVariable::Route => Cow::Borrowed(&self.route),
            HeaderVariable::ClientCertSubject => Cow::Borrowed(&self.client_cert_subject),
            HeaderVariable::ClientCertFingerprint => Cow::Borrowed(&self.client_cert_fingerprint),
        }
    }
}

/// Returns a random 32-character hex request ID.
pub fn new_request_id() -> String {
    format!("{:032x}", rand::random::<u128>())
}

impl CompiledHeaderRules {
    /// Compiles the rules. Invalid rules are logged and skipped. The server validates rules
    /// before it stores them, so this only happens with a hand-edited configuration file.
    pub fn new(rules: &HeaderRules) -> Self {
        Self {
            request: compile(&rules.request),
            response: compile(&rules.response),
        }
    }

    pub fn is_empty(&self) -> bool {
        self.request.is_empty() && self.response.is_empty()
    }

    pub fn apply_request(&self, headers: &mut HeaderMap, variables: &HeaderVariables) {
        apply(&self.request, headers, variables);
    }

    pub fn apply_response(&self, headers: &mut HeaderMap, variables: &HeaderVariables) {
        apply(&self.response, headers, variables);
    }
}

fn compile(rules: &[HeaderRule]) -> Vec<CompiledRule> {
    rules
        .iter()
        .filter_map(|rule| {
            compile_rule(rule)
                .map_err(|err| error!(%rule, %err, "ignoring invalid header rule"))
                .ok()
        })
        .collect()
}

fn compile_rule(rule: &HeaderRule) -> Result<CompiledRule, Error> {
    rule.validate()?;
    let name = HeaderName::from_str(rule.name()).map_err(|_| Error::InvalidHeaderName {
        name: rule.name().into(),
    })?;
    let action = match rule {
        HeaderRule::Set { value, .. } => Action::Set(parse_header_template(value)?),
        HeaderRule::Append { value, .. } => Action::Append(parse_header_template(value)?),
        HeaderRule::Remove { .. } => Action::Remove,
    };
    Ok(CompiledRule { name, action })
}

fn apply(rules: &[CompiledRule], headers: &mut HeaderMap, variables: &HeaderVariables) {
    for rule in rules {
        let (template, append) = match &rule.action {
            Action::Remove => {
                headers.remove(&rule.name);
                continue;
            }
            Action::Set(template) => (template, false),
            Action::Append(template) => (template, true),
        };
        let value = match HeaderValue::from_str(&render(template, variables)) {
            Ok(value) => value,
            Err(err) => {
                error!(name = %rule.name, %err, "header rule produced an invalid value");
                continue;
            }
        };
        if append {
            headers.append(rule.name.clone(), value);
        } else {
            headers.insert(rule.name.clone(), value);
        }
    }
}

fn render(template: &[TemplatePart], variables: &HeaderVariables) -> String {
    let mut value = String::new();
    for part in template {
        match part {
            TemplatePart::Literal(text) => value.push_str(text),
            TemplatePart::Variable(variable) => value.push_str(&variables.value(*variable)),
        }
    }
    value
}

#[cfg(test)]
mod tests {
    use super::*;
    use r3v3rs3_api::header_rules::parse_header_rules;

    fn variables() -> HeaderVariables {
        HeaderVariables {
            client_ip: "203.0.113.7".parse().unwrap(),
            host: "example.com".into(),
            scheme: "https",
            request_id: "abc".into(),
            route: "/api".into(),
            client_cert_subject: "CN=client".into(),
            client_cert_fingerprint: String::new(),
        }
    }

    #[test]
    fn client_certificate_variables() {
        let rules = CompiledHeaderRules::new(&HeaderRules {
            request: parse_header_rules(
                "set X-Client-Cert: {client_cert_subject}\nset X-Client-Cert-Fingerprint: {client_cert_fingerprint}",
            )
            .unwrap(),
            response: vec![],
        });
        let mut headers = HeaderMap::new();
        headers.insert("x-client-cert", HeaderValue::from_static("CN=spoofed"));
        headers.insert(
            "x-client-cert-fingerprint",
            HeaderValue::from_static("spoofed"),
        );

        rules.apply_request(&mut headers, &variables());
        assert_eq!(headers["x-client-cert"], "CN=client");
        assert_eq!(headers["x-client-cert-fingerprint"], "");
    }

    #[test]
    fn rules_set_append_and_remove_headers() {
        let rules = CompiledHeaderRules::new(&HeaderRules {
            request: parse_header_rules(
                "set X-Client: {client_ip}\nappend X-Tag: {scheme}://{host}{route}\nremove X-Secret\nset X-Id: {{{request_id}}}",
            )
            .unwrap(),
            response: vec![],
        });
        let mut headers = HeaderMap::new();
        headers.insert("x-client", HeaderValue::from_static("spoofed"));
        headers.insert("x-tag", HeaderValue::from_static("client"));
        headers.insert("x-secret", HeaderValue::from_static("1"));

        rules.apply_request(&mut headers, &variables());
        assert_eq!(headers["x-client"], "203.0.113.7");
        assert_eq!(
            headers.get_all("x-tag").iter().collect::<Vec<_>>(),
            ["client", "https://example.com/api"]
        );
        assert!(headers.get("x-secret").is_none());
        assert_eq!(headers["x-id"], "{abc}");

        let mut response = HeaderMap::new();
        rules.apply_response(&mut response, &variables());
        assert!(response.is_empty());
    }

    #[test]
    fn invalid_rules_are_skipped() {
        let rules = CompiledHeaderRules::new(&HeaderRules {
            request: vec![HeaderRule::Remove {
                name: "Host".into(),
            }],
            response: vec![HeaderRule::Set {
                name: "X-A".into(),
                value: "{nope}".into(),
            }],
        });
        assert!(rules.is_empty());
    }
}
