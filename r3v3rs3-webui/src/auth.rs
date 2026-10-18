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

    /// An OAuth target on the server, like `/oauth/authorize?...`.
    pub fn is_oauth_redirect(&self) -> bool {
        self.redirect
            .as_deref()
            .is_some_and(|target| target.starts_with("/oauth/"))
    }
}

/// The outcome of a session check. A request that died without an answer is `Unreachable`: the
/// browser aborts the in-flight requests of a page that navigates away, and a failed check must
/// never end a session that the server may still hold.
pub enum SessionState {
    Active(SessionInfo),
    SignedOut,
    Unreachable,
}

/// Asks the server for the session. Only a 401 answer signs the client out; a transport error or
/// another status leaves the question open.
pub async fn check_session() -> SessionState {
    let res = match Request::get(&format!("{API_ENDPOINT}/session"))
        .send()
        .await
    {
        Ok(res) => res,
        Err(_) => return SessionState::Unreachable,
    };
    if res.status() == 401 {
        return SessionState::SignedOut;
    }
    if res.status() != 200 {
        return SessionState::Unreachable;
    }
    match res.json::<SessionInfo>().await {
        Ok(info) => SessionState::Active(info),
        Err(_) => SessionState::Unreachable,
    }
}

/// Sends a client without a session to the sign-in page, and keeps the account of the session in
/// the session store. The check runs once per mount, and an unreachable server changes nothing.
#[hook]
pub fn use_ensure_auth() {
    let Some(navigator) = use_navigator() else {
        return;
    };
    let (_, session) = use_store::<SessionStore>();

    let query = LoginQuery::new(use_route::<Route>().filter(|route| route != &Route::Login));

    use_effect_with((), move |_| {
        let navigator = navigator.clone();
        let session = session.clone();
        let query = query.clone();
        wasm_bindgen_futures::spawn_local(async move {
            match check_session().await {
                SessionState::Active(info) => {
                    if session.get().info.as_ref() != Some(&info) {
                        let (platform_enabled, update_available) = get_session_flags().await;
                        crate::format::set_display_timezone(&info.display_timezone);
                        session.set(SessionStore {
                            display_timezone: info.display_timezone.clone(),
                            info: Some(info),
                            platform_enabled,
                            update_available,
                        });
                    }
                }
                SessionState::SignedOut => {
                    if let Err(err) = navigator.replace_with_query(&Route::Login, &query) {
                        let message = format!("the sign-in redirect failed: {err}");
                        web_sys::console::error_1(&message.into());
                    }
                }
                SessionState::Unreachable => {}
            }
        });
        || {}
    });
}

/// The session flags of one app_info answer, so the sign-in needs one request for both: the
/// platform flag that the sidebar reads, and the newest release that the banner shows.
pub async fn get_session_flags() -> (bool, Option<String>) {
    let value = app_info_flag("platform_enabled").await;
    match value {
        Some(platform) => (
            platform.as_bool().unwrap_or(false),
            app_info_flag("update_available")
                .await
                .and_then(|value| value.as_str().map(str::to_owned)),
        ),
        None => (false, None),
    }
}

async fn app_info_flag(name: &str) -> Option<serde_json::Value> {
    let Ok(res) = Request::get(&format!("{API_ENDPOINT}/app_info"))
        .send()
        .await
    else {
        return None;
    };
    if res.status() != 200 {
        return None;
    }
    res.json::<serde_json::Value>()
        .await
        .ok()
        .and_then(|value| value.get(name).cloned())
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

    #[test]
    fn an_oauth_redirect_is_recognized() {
        let query = LoginQuery {
            redirect: Some("/oauth/authorize?client_id=mcp-cli".to_string()),
        };
        assert!(query.is_oauth_redirect());

        let internal = LoginQuery {
            redirect: Some("/apps".to_string()),
        };
        assert!(!internal.is_oauth_redirect());
    }
}
