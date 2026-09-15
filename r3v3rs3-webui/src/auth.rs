use crate::{pages::Route, store::SessionStore, API_ENDPOINT};
use gloo_net::http::Request;
use r3v3rs3_api::auth::SessionInfo;
use serde_derive::{Deserialize, Serialize};
use yew::prelude::*;
use yew_router::prelude::*;
use yewdux::prelude::*;

#[derive(Default, Clone, Serialize, Deserialize)]
pub struct LoginQuery {
    #[serde(default)]
    pub redirect: Option<Route>,
}

/// Sends a client without a session to the sign-in page, and keeps the account of the session in
/// the session store.
#[hook]
pub fn use_ensure_auth() {
    let navigator = use_navigator().unwrap();
    let (_, session) = use_store::<SessionStore>();

    let query = LoginQuery {
        redirect: use_route::<Route>().filter(|route| route != &Route::Login),
    };

    wasm_bindgen_futures::spawn_local(async move {
        match get_session().await {
            Some(info) => {
                if session.get().info.as_ref() != Some(&info) {
                    session.set(SessionStore { info: Some(info) });
                }
            }
            None => {
                let _ = navigator.replace_with_query(&Route::Login, &query);
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
