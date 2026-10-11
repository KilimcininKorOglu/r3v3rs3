#![recursion_limit = "1024"]

use auth::LoginQuery;
use components::cluster_status::ClusterBanner;
use components::footer::Footer;
use components::navbar::Navbar;
use components::sidebar::Sidebar;
use console_error_panic_hook::set_once as set_panic_hook;
use store::SessionStore;
use yew::prelude::*;
use yew_router::prelude::*;
use yewdux::prelude::*;

mod auth;
mod components;
mod dialog;
mod event;
mod format;
mod i18n;
mod pages;
mod preferences;
mod store;

const API_ENDPOINT: &str = "/api";

/// The result of the session check that runs before a page renders.
#[derive(Clone, Copy, PartialEq)]
enum Auth {
    Pending,
    SignedIn,
    SignedOut,
}

#[function_component(App)]
pub fn app() -> Html {
    event::use_event_subscriber();
    html! {
        <BrowserRouter>
            <Shell />
        </BrowserRouter>
    }
}

/// The layout and the auth gate. It lives inside the router, because the redirect of a
/// signed-out client needs the router context.
#[function_component(Shell)]
fn shell() -> Html {
    let navigator = use_navigator();
    let route = use_route::<pages::Route>();
    let auth = use_state(|| Auth::Pending);
    let (_, set_session) = use_store::<SessionStore>();
    {
        let auth = auth.clone();
        let set_session = set_session.clone();
        use_effect_with((), move |_| {
            wasm_bindgen_futures::spawn_local(async move {
                match auth::get_session().await {
                    Some(info) => {
                        // The sidebar hides the platform group while the platform is off,
                        // so the app information travels with the session.
                        let platform_enabled = auth::get_platform_enabled().await;
                        set_session.set(SessionStore {
                            info: Some(info),
                            platform_enabled,
                        });
                        auth.set(Auth::SignedIn);
                    }
                    None => auth.set(Auth::SignedOut),
                }
            });
            || {}
        });
    }
    use_signed_out_redirect(&auth, &set_session, &navigator, &route);
    // Only a signed-in client sees a page, and the sign-in page is the only page a
    // signed-out client sees. The server already refuses the shell of every other
    // route to a signed-out navigation; this covers the routes inside the SPA.
    if *auth != Auth::SignedIn && route.as_ref() != Some(&pages::Route::Login) {
        return html!();
    }
    html! {
        <div class="min-h-screen flex flex-col">
            <Navbar />
            <div class="w-full max-w-7xl mx-auto px-4 sm:px-6 py-4 flex-1 flex gap-6">
                <Sidebar />
                <main class="flex-1 min-w-0">
                    <ClusterBanner />
                    <Switch<pages::Route> render={pages::switch} />
                </main>
            </div>
            <Footer />
        </div>
    }
}

fn main() {
    set_panic_hook();
    yew::Renderer::<App>::new().render();
}

/// Sends a client that the session store says is signed out to the sign-in page. A sign-in
/// that just finished has changed the session behind the stale state, so the redirect waits
/// for a fresh answer first and trusts a new session.
#[hook]
fn use_signed_out_redirect(
    auth: &UseStateHandle<Auth>,
    set_session: &Dispatch<SessionStore>,
    navigator: &Option<Navigator>,
    route: &Option<pages::Route>,
) {
    let navigator = navigator.clone();
    let route = route.clone();
    let auth = auth.clone();
    let set_session = set_session.clone();
    use_effect_with((auth.clone(), route), move |(auth, route)| {
        if **auth == Auth::SignedOut
            && route.as_ref() != Some(&pages::Route::Login)
            && let Some(navigator) = navigator.as_ref()
        {
            let navigator = navigator.clone();
            let route = route.clone();
            let auth = auth.clone();
            let set_session = set_session.clone();
            wasm_bindgen_futures::spawn_local(async move {
                match auth::get_session().await {
                    Some(info) => {
                        let platform_enabled = auth::get_platform_enabled().await;
                        set_session.set(SessionStore {
                            info: Some(info),
                            platform_enabled,
                        });
                        auth.set(Auth::SignedIn);
                    }
                    None => {
                        navigator
                            .replace_with_query(&pages::Route::Login, &LoginQuery::new(route))
                            .ok();
                    }
                }
            });
        }
        || {}
    });
}
