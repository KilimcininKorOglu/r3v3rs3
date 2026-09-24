//! The GitHub App of a GitHub connection. The page posts a manifest to GitHub, GitHub creates the
//! GitHub App and sends the browser to its installation page, and after the installation GitHub
//! returns to this page, which connects the connection.

use super::accounts::{HINT_CLASS, LABEL_CLASS};
use super::settings::{fetch_json, send_request};
use crate::API_ENDPOINT;
use gloo_net::http::Request;
use r3v3rs3_api::git_connection::{GitConnectionEntry, GithubAppForm, GithubAppRequest};
use r3v3rs3_api::i18n::Locale;
use r3v3rs3_api::id::ShortId;
use wasm_bindgen::JsCast;
use yew::prelude::*;

/// The fields of a new GitHub connection. `address` and `organization` are the inputs of the
/// optional GitHub Enterprise Server address and organization.
pub(super) fn fields_view(locale: Locale, address: Html, organization: Html) -> Html {
    html! {
        <>
            <p class={HINT_CLASS}>{locale.t("git_connections.github_app_hint")}</p>
            <label class={LABEL_CLASS}>{locale.t("git_connections.address")}</label>
            { address }
            <p class={HINT_CLASS}>{locale.t("git_connections.github_address_hint")}</p>
            <label class={LABEL_CLASS}>{locale.t("git_connections.organization")}</label>
            { organization }
            <p class={HINT_CLASS}>{locale.t("git_connections.organization_hint")}</p>
        </>
    }
}

/// Starts the creation of the GitHub App and posts its manifest to GitHub. The browser leaves the
/// page when the post starts.
pub(super) async fn create(locale: Locale, request: GithubAppRequest) -> Result<(), String> {
    let request = Request::post(&format!("{API_ENDPOINT}/git/github_apps"))
        .json(&request)
        .map_err(|err| err.to_string())?;
    let form: GithubAppForm = send_request(locale, request)
        .await?
        .json()
        .await
        .map_err(|err| err.to_string())?;
    post_manifest(&form).map_err(|_| "the GitHub page did not open".to_string())
}

/// Posts the manifest with a form, because GitHub takes the manifest only from a form post.
fn post_manifest(form: &GithubAppForm) -> Result<(), wasm_bindgen::JsValue> {
    let document = web_sys::window()
        .and_then(|window| window.document())
        .ok_or("no document")?;
    let element = document
        .create_element("form")?
        .dyn_into::<web_sys::HtmlFormElement>()?;
    element.set_method("post");
    element.set_action(&form.url);
    element.set_hidden(true);
    let input = document
        .create_element("input")?
        .dyn_into::<web_sys::HtmlInputElement>()?;
    input.set_type("hidden");
    input.set_name("manifest");
    input.set_value(&form.manifest);
    element.append_child(&input)?;
    document.body().ok_or("no body")?.append_child(&element)?;
    element.submit()
}

/// The id of the connection whose GitHub App GitHub installed, from the `installed` query that
/// the setup address of the GitHub App holds.
pub(super) fn installed_id(query: &str) -> Option<ShortId> {
    query
        .trim_start_matches('?')
        .split('&')
        .find_map(|pair| pair.strip_prefix("installed="))
        // An empty text parses as the zero id, and a short id accepts any ASCII text, so only the
        // characters of a generated id pass into the API path.
        .filter(|id| !id.is_empty() && id.chars().all(|c| c.is_ascii_alphanumeric() || c == '-'))
        .and_then(|id| id.parse().ok())
}

/// Connects the connection to the installation of its GitHub App.
pub(super) async fn install(locale: Locale, id: ShortId) -> Result<GitConnectionEntry, String> {
    let path = format!("{API_ENDPOINT}/git/connections/{id}/installation");
    fetch_json(locale, Request::post(&path)).await
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_setup_address_names_the_installed_connection() {
        let id = installed_id("?installed=bcd-fgh&installation_id=77&setup_action=install");
        assert_eq!(id.map(|id| id.to_string()).as_deref(), Some("bcd-fgh"));
        assert!(installed_id("?installed=<script>").is_none());
        assert!(installed_id("?installed=../apps").is_none());
        assert!(installed_id("?connected=bcd-fgh").is_none());
        assert!(installed_id("").is_none());
        assert!(installed_id("?installed=").is_none());
    }
}
