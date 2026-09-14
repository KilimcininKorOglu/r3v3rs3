//! Translations and display preferences shared by the WebUI and the pages that the server renders.

use crate::error::Error;
use serde_derive::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::HashMap;
use std::sync::OnceLock;

/// Name of the cookie that stores the language selected in the WebUI.
pub const LOCALE_COOKIE: &str = "r3v3rs3_lang";

/// Name of the cookie that stores the theme selected in the WebUI.
pub const THEME_COOKIE: &str = "r3v3rs3_theme";

type Table = HashMap<String, String>;

#[derive(Debug, Default, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Locale {
    #[default]
    En,
    Tr,
}

impl Locale {
    pub const ALL: [Locale; 2] = [Locale::En, Locale::Tr];

    pub fn code(self) -> &'static str {
        match self {
            Self::En => "en",
            Self::Tr => "tr",
        }
    }

    pub fn from_code(code: &str) -> Option<Self> {
        Self::ALL
            .into_iter()
            .find(|locale| locale.code().eq_ignore_ascii_case(code.trim()))
    }

    /// Reads the language from a `Cookie` header value. A missing cookie or an unknown language
    /// gives the default language.
    pub fn from_cookie_header(header: &str) -> Self {
        cookie_value(header, LOCALE_COOKIE)
            .and_then(Self::from_code)
            .unwrap_or_default()
    }

    /// Returns the text of the key. A key that this language does not define falls back to
    /// English. A key that English does not define returns the key itself, so the gap is visible.
    pub fn t(self, key: &str) -> &str {
        table(self)
            .get(key)
            .or_else(|| table(Self::En).get(key))
            .map_or(key, String::as_str)
    }

    /// Returns the text of the key with every `{name}` placeholder replaced by its value.
    pub fn tf(self, key: &str, params: &[(&str, &str)]) -> String {
        params
            .iter()
            .fold(self.t(key).to_string(), |text, (name, value)| {
                text.replace(&format!("{{{name}}}"), value)
            })
    }

    /// Returns the message of an API error. The `error.<variant>` key selects the text, and the
    /// fields of the variant fill its placeholders.
    pub fn error_message(self, error: &Error) -> String {
        let Ok(Value::Object(fields)) = serde_json::to_value(error) else {
            return error.to_string();
        };
        let code = fields
            .get("message")
            .and_then(Value::as_str)
            .unwrap_or_default();
        let values = fields
            .iter()
            .filter(|(name, _)| name.as_str() != "message")
            .map(|(name, value)| (name.as_str(), field_text(value)))
            .collect::<Vec<_>>();
        let params = values
            .iter()
            .map(|(name, value)| (*name, value.as_str()))
            .collect::<Vec<_>>();
        self.tf(&format!("error.{code}"), &params)
    }
}

#[derive(Debug, Default, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Theme {
    /// Follows the color scheme of the operating system.
    #[default]
    System,
    Light,
    Dark,
}

impl Theme {
    pub const ALL: [Theme; 3] = [Theme::System, Theme::Light, Theme::Dark];

    pub fn code(self) -> &'static str {
        match self {
            Self::System => "system",
            Self::Light => "light",
            Self::Dark => "dark",
        }
    }

    pub fn from_code(code: &str) -> Option<Self> {
        Self::ALL
            .into_iter()
            .find(|theme| theme.code().eq_ignore_ascii_case(code.trim()))
    }

    /// Reads the theme from a `Cookie` header value. A missing cookie or an unknown theme gives
    /// the system theme.
    pub fn from_cookie_header(header: &str) -> Self {
        cookie_value(header, THEME_COOKIE)
            .and_then(Self::from_code)
            .unwrap_or_default()
    }
}

/// Returns the value of the named cookie in a `Cookie` header value.
pub fn cookie_value<'a>(header: &'a str, name: &str) -> Option<&'a str> {
    header
        .split(';')
        .filter_map(|pair| pair.trim().split_once('='))
        .find(|(key, _)| *key == name)
        .map(|(_, value)| value.trim())
}

fn field_text(value: &Value) -> String {
    value
        .as_str()
        .map_or_else(|| value.to_string(), str::to_string)
}

fn table(locale: Locale) -> &'static Table {
    static TABLES: OnceLock<[Table; 2]> = OnceLock::new();
    let tables = TABLES.get_or_init(|| {
        [
            parse(include_str!("../locales/en.json")),
            parse(include_str!("../locales/tr.json")),
        ]
    });
    match locale {
        Locale::En => &tables[0],
        Locale::Tr => &tables[1],
    }
}

/// The tests parse both files, so an invalid file fails the test suite before a release.
/// An empty table makes every text show its key.
fn parse(source: &str) -> Table {
    serde_json::from_str(source).unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::*;
    use regex::Regex;
    use std::collections::BTreeSet;

    fn source(locale: Locale) -> &'static str {
        match locale {
            Locale::En => include_str!("../locales/en.json"),
            Locale::Tr => include_str!("../locales/tr.json"),
        }
    }

    fn placeholders(text: &str) -> BTreeSet<String> {
        let pattern = Regex::new(r"\{([a-z_]+)\}").unwrap();
        pattern
            .captures_iter(text)
            .map(|capture| capture[1].to_string())
            .collect()
    }

    fn all_errors() -> Vec<Error> {
        let addr = "/ip4/127.0.0.1/tcp/80".parse().unwrap();
        let text = || "value".to_string();
        vec![
            Error::InvalidListeningAddress { addr },
            Error::InvalidServerAddress {
                addr: "/ip4/127.0.0.1/tcp/80".parse().unwrap(),
            },
            Error::InvalidSubjectName { name: text() },
            Error::InvalidVirtualHost { host: text() },
            Error::InvalidServerUrl { url: text() },
            Error::InvalidCidr { cidr: text() },
            Error::FailedToRefreshCdnRanges,
            Error::InvalidMultiaddr { addr: text() },
            Error::TlsTerminationConfigMissing,
            Error::ProxyProtocolNotSupported,
            Error::ProxyProtocolTrustedMissing,
            Error::CertificateInUse {
                id: "abc".parse().unwrap(),
            },
            Error::InvalidClientCaCert {
                id: "abc".parse().unwrap(),
            },
            Error::ClientCaCertsMissing,
            Error::InvalidClientCert {
                id: "abc".parse().unwrap(),
            },
            Error::FailedToGenerateSelfSignedCertificate,
            Error::FailedToReadCertificate,
            Error::FailedToReadPrivateKey,
            Error::InvalidShortId { id: text() },
            Error::IdNotFound { id: text() },
            Error::IdAlreadyExists {
                id: "abc".parse().unwrap(),
            },
            Error::ProxyReadOnly {
                id: "abc".parse().unwrap(),
            },
            Error::CertificateReadOnly {
                id: "abc".parse().unwrap(),
            },
            Error::InvalidDiscoveryConfig { reason: text() },
            Error::AcmeAccountCreationFailed,
            Error::AcmeUnsupportedChallenge { challenge: text() },
            Error::AcmeIdentifiersMissing,
            Error::AcmeInvalidIdentifier { identifier: text() },
            Error::AcmeWildcardNeedsDnsChallenge { identifier: text() },
            Error::AcmeDnsProviderRequired,
            Error::Unauthorized,
            Error::FailedToCreateAccount,
            Error::InvalidLoginCredentials,
            Error::TooManyLoginAttempts,
            Error::InvalidUsername { username: text() },
            Error::PasswordRequired { username: text() },
            Error::InvalidTokenName { name: text() },
            Error::InvalidToken { name: text() },
            Error::InvalidHeaderName { name: text() },
            Error::ProtectedHeader { name: text() },
            Error::InvalidHeaderValue { value: text() },
            Error::InvalidHeaderRule { rule: text() },
            Error::InvalidMimeType { mime: text() },
            Error::DuplicateCompressionAlgorithm { algorithm: text() },
            Error::InvalidCacheSize,
            Error::InvalidTimeout,
            Error::InvalidHealthCheckPath { path: text() },
            Error::AllServersDrained,
            Error::InvalidCircuitBreaker,
            Error::InvalidRetryAttempts,
            Error::InvalidStickyCookieName { name: text() },
            Error::InvalidPathRegex { pattern: text() },
            Error::InvalidPathPrefix { prefix: text() },
            Error::InvalidRedirectRule { rule: text() },
            Error::InvalidRedirectStatus { status: 303 },
            Error::InvalidRedirectTarget { target: text() },
            Error::InvalidMirrorPercent { percent: 0 },
            Error::FailedToHashPassword,
            Error::FailedToFetchLog,
            Error::FailedToInvokeRpc,
            Error::FailedToListNetworkInterfaces,
        ]
    }

    #[test]
    fn locale_files_parse() {
        for locale in Locale::ALL {
            let parsed: Table = serde_json::from_str(source(locale)).unwrap();
            assert!(!parsed.is_empty());
            assert_eq!(table(locale), &parsed);
        }
    }

    #[test]
    fn locales_define_the_same_keys_and_placeholders() {
        let english = table(Locale::En);
        for locale in Locale::ALL {
            let other = table(locale);
            let english_keys = english.keys().collect::<BTreeSet<_>>();
            let other_keys = other.keys().collect::<BTreeSet<_>>();
            assert_eq!(english_keys, other_keys, "keys of {}", locale.code());
            for (key, text) in english {
                assert_eq!(
                    placeholders(text),
                    placeholders(&other[key]),
                    "placeholders of {key} in {}",
                    locale.code()
                );
            }
        }
    }

    #[test]
    fn every_error_has_a_message() {
        for locale in Locale::ALL {
            for error in all_errors() {
                let message = locale.error_message(&error);
                assert!(!message.starts_with("error."), "{message}");
                assert!(!message.contains('{'), "{message}");
            }
        }
        assert_eq!(
            Locale::Tr.error_message(&Error::InvalidUsername {
                username: "bob".into()
            }),
            "Geçersiz kullanıcı adı: bob"
        );
    }

    #[test]
    fn text_falls_back_to_english_and_then_to_the_key() {
        assert_eq!(Locale::Tr.t("login.title"), "Giriş Yap");
        assert_eq!(Locale::En.t("login.title"), "Sign In");
        assert_eq!(Locale::Tr.t("missing.key"), "missing.key");
        assert_eq!(
            Locale::En.tf("error.id_not_found", &[("id", "p1")]),
            "ID not found: p1"
        );
    }

    #[test]
    fn preferences_are_read_from_the_cookie_header() {
        let header = "r3v3rs3_session=abc; r3v3rs3_lang=tr; r3v3rs3_theme=dark";
        assert_eq!(Locale::from_cookie_header(header), Locale::Tr);
        assert_eq!(Theme::from_cookie_header(header), Theme::Dark);
        assert_eq!(Locale::from_cookie_header("r3v3rs3_lang=xx"), Locale::En);
        assert_eq!(Theme::from_cookie_header(""), Theme::System);
        assert_eq!(cookie_value("a=1; b = 2", "b"), None);
        assert_eq!(cookie_value("a=1;b=2", "b"), Some("2"));
    }
}
