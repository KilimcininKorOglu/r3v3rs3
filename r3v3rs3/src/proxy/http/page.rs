//! Display preferences of the pages that r3v3rs3 renders itself: the error page and the sign-in
//! page.

use hyper::header::{HeaderMap, COOKIE};
use r3v3rs3_api::i18n::{Locale, Theme};

/// Language and theme that the WebUI stores in cookies. A browser sends these cookies only to the
/// host of the WebUI, so the pages of a proxy on another host use the defaults.
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub struct PagePreferences {
    pub locale: Locale,
    pub theme: Theme,
}

impl PagePreferences {
    /// Reads the preferences from every `Cookie` header of the request.
    pub fn from_headers(headers: &HeaderMap) -> Self {
        let cookies = headers
            .get_all(COOKIE)
            .iter()
            .filter_map(|value| value.to_str().ok())
            .collect::<Vec<_>>()
            .join("; ");
        Self {
            locale: Locale::from_cookie_header(&cookies),
            theme: Theme::from_cookie_header(&cookies),
        }
    }

    /// Returns the value of the `data-theme` attribute. The system theme sets no attribute, so
    /// the page follows `prefers-color-scheme`.
    pub fn theme_attribute(self) -> Option<&'static str> {
        match self.theme {
            Theme::System => None,
            theme => Some(theme.code()),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use hyper::header::HeaderValue;

    #[test]
    fn preferences_are_read_from_all_cookie_headers() {
        let mut headers = HeaderMap::new();
        headers.append(
            COOKIE,
            HeaderValue::from_static("session=abc; r3v3rs3_lang=tr"),
        );
        headers.append(COOKIE, HeaderValue::from_static("r3v3rs3_theme=dark"));
        let preferences = PagePreferences::from_headers(&headers);
        assert_eq!(
            preferences,
            PagePreferences {
                locale: Locale::Tr,
                theme: Theme::Dark,
            }
        );
        assert_eq!(preferences.theme_attribute(), Some("dark"));
    }

    #[test]
    fn missing_or_unknown_cookies_give_the_defaults() {
        assert_eq!(
            PagePreferences::from_headers(&HeaderMap::new()),
            PagePreferences::default()
        );
        let mut headers = HeaderMap::new();
        headers.insert(
            COOKIE,
            HeaderValue::from_static("r3v3rs3_lang=xx; r3v3rs3_theme=blue"),
        );
        let preferences = PagePreferences::from_headers(&headers);
        assert_eq!(preferences, PagePreferences::default());
        assert_eq!(preferences.locale, Locale::En);
        assert_eq!(preferences.theme_attribute(), None);
    }
}
