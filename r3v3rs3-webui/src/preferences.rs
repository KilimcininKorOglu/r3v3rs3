//! Display preferences of the WebUI. The values are stored in cookies, so the head script of
//! `index.html` and the pages that the server renders read the same values.

use r3v3rs3_api::i18n::{Theme, THEME_COOKIE};
use wasm_bindgen::{JsCast, JsValue};
use web_sys::{Document, HtmlDocument};
use yewdux::prelude::*;

const COOKIE_ATTRIBUTES: &str = "Path=/; Max-Age=31536000; SameSite=Lax";
const DARK_QUERY: &str = "(prefers-color-scheme: dark)";
const LIGHT_THEME_COLOR: &str = "#ffffff";
const DARK_THEME_COLOR: &str = "#333333";

#[derive(Clone, PartialEq)]
pub struct PreferencesStore {
    pub theme: Theme,
}

impl Store for PreferencesStore {
    fn new(_cx: &yewdux::Context) -> Self {
        let cookies = html_document()
            .map(|document| document.cookie().map_err(report).unwrap_or_default())
            .unwrap_or_default();
        Self {
            theme: Theme::from_cookie_header(&cookies),
        }
    }

    fn should_notify(&self, old: &Self) -> bool {
        self != old
    }
}

/// Stores the theme in its cookie and applies it to the page.
pub fn set_theme(dispatch: &Dispatch<PreferencesStore>, theme: Theme) {
    if let Some(document) = html_document() {
        let cookie = format!("{THEME_COOKIE}={}; {COOKIE_ATTRIBUTES}", theme.code());
        if let Err(err) = document.set_cookie(&cookie) {
            report(err);
        }
    }
    apply_theme(theme);
    dispatch.reduce_mut(|preferences| preferences.theme = theme);
}

/// Sets the `dark` class on the root element and the matching `theme-color`. The head script of
/// `index.html` does the same before the WebUI loads and when the system color scheme changes.
fn apply_theme(theme: Theme) {
    let Some(window) = web_sys::window() else {
        return;
    };
    let dark = match theme {
        Theme::Light => false,
        Theme::Dark => true,
        Theme::System => window
            .match_media(DARK_QUERY)
            .map_err(report)
            .ok()
            .flatten()
            .is_some_and(|query| query.matches()),
    };
    let Some(document) = window.document() else {
        return;
    };
    if let Some(root) = document.document_element() {
        if let Err(err) = root.class_list().toggle_with_force("dark", dark) {
            report(err);
        }
    }
    set_theme_color(&document, dark);
}

fn set_theme_color(document: &Document, dark: bool) {
    let color = if dark {
        DARK_THEME_COLOR
    } else {
        LIGHT_THEME_COLOR
    };
    match document.query_selector("meta[name=theme-color]") {
        Ok(Some(meta)) => {
            if let Err(err) = meta.set_attribute("content", color) {
                report(err);
            }
        }
        Ok(None) => {}
        Err(err) => report(err),
    }
}

fn html_document() -> Option<HtmlDocument> {
    web_sys::window()?
        .document()?
        .dyn_into::<HtmlDocument>()
        .ok()
}

fn report(err: JsValue) {
    web_sys::console::error_1(&err);
}
