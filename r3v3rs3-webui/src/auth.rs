use crate::{API_ENDPOINT, pages::Route, store::SessionStore};
use gloo_net::http::Request;
use r3v3rs3_api::auth::SessionInfo;
use serde_derive::{Deserialize, Serialize};
use yew::prelude::*;
use yew_router::prelude::*;
use yewdux::prelude::*;

/// The query of the sign-in page. The page to open after the sign-in travels as its path,
/// because the query encoding cannot hold a route with parameters.
#[derive(Default, Clone, Serialize, Deserialize)]
pub struct LoginQuery {
    #[serde(default)]
    pub redirect: Option<String>,
}

impl LoginQuery {
    pub fn new(route: Option<Route>) -> Self {
        Self {
            redirect: route.map(|route| route.to_path()),
        }
    }

    /// The page to open after the sign-in. An unknown path opens the home page.
    pub fn route(&self) -> Route {
        self.redirect
            .as_deref()
            .and_then(Route::recognize)
            .filter(|route| !matches!(route, Route::Login | Route::NotFound))
            .unwrap_or(Route::Home)
    }
}

/// Sends a client without a session to the sign-in page, and keeps the account of the session in
/// the session store.
#[hook]
pub fn use_ensure_auth() {
    let navigator = use_navigator().unwrap();
    let (_, session) = use_store::<SessionStore>();

    let query = LoginQuery::new(use_route::<Route>().filter(|route| route != &Route::Login));

    wasm_bindgen_futures::spawn_local(async move {
        match get_session().await {
            Some(info) => {
                if session.get().info.as_ref() != Some(&info) {
                    session.set(SessionStore { info: Some(info) });
                }
            }
            None => {
                if let Err(err) = navigator.replace_with_query(&Route::Login, &query) {
                    let message = format!("the sign-in redirect failed: {err}");
                    web_sys::console::error_1(&message.into());
                }
            }
        }
    });
}

pub async fn test_token() -> bool {
    if let Ok(res) = Request::get(&format!("{API_ENDPOINT}/app_info"))
        .send()
        .await
    {
        res.status() == 200
    } else {
        false
    }
}

/// The account of the session. `None` without a valid session.
async fn get_session() -> Option<SessionInfo> {
    let res = Request::get(&format!("{API_ENDPOINT}/session"))
        .send()
        .await
        .ok()?;
    if res.status() != 200 {
        return None;
    }
    res.json().await.ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_sign_in_returns_to_a_page_with_parameters() {
        let id = "fzn-txd".parse().unwrap();
        let query = LoginQuery::new(Some(Route::AppDeployments { id }));
        assert_eq!(query.redirect.as_deref(), Some("/apps/fzn-txd/deployments"));
        assert_eq!(query.route(), Route::AppDeployments { id });
    }

    #[test]
    fn an_unknown_or_missing_redirect_opens_the_home_page() {
        for redirect in [
            None,
            Some("/nothing/here"),
            Some("/login"),
            Some("https://x.test/"),
        ] {
            let query = LoginQuery {
                redirect: redirect.map(str::to_string),
            };
            assert_eq!(query.route(), Route::Home, "{redirect:?}");
        }
    }
}
