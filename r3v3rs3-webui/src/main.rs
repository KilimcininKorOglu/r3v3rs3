#![recursion_limit = "1024"]

use components::cluster_status::ClusterBanner;
use components::footer::Footer;
use components::navbar::Navbar;
use components::sidebar::Sidebar;
use console_error_panic_hook::set_once as set_panic_hook;
use yew::prelude::*;
use yew_router::prelude::*;

mod auth;
mod components;
mod event;
mod format;
mod i18n;
mod pages;
mod preferences;
mod store;

const API_ENDPOINT: &str = "/api";

#[function_component(App)]
pub fn app() -> Html {
    event::use_event_subscriber();
    html! {
        <>
        <BrowserRouter>
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
        </BrowserRouter>
        </>
    }
}

fn main() {
    set_panic_hook();
    yew::Renderer::<App>::new().render();
}
