use crate::{API_ENDPOINT, pages::Route, store::SessionStore};
use gloo_net::http::Request;
use yew::prelude::*;
use yew_router::prelude::*;
use yewdux::prelude::*;

#[function_component(Logout)]
pub fn logout() -> Html {
    let (_, session) = use_store::<SessionStore>();
    wasm_bindgen_futures::spawn_local(async move {
        Request::get(&format!("{API_ENDPOINT}/logout"))
            .send()
            .await
            .unwrap();
        session.set(SessionStore::default());
    });

    html! {
        <Redirect<Route> to={Route::Login}/>
    }
}
