use crate::{i18n::use_locale, pages::Route, store::SessionStore};
use yew::prelude::*;
use yew_router::prelude::*;
use yewdux::prelude::*;

/// A banner on every page while the update check found a newer release. The link opens the
/// settings page, where the upgrade card installs it.
#[function_component(UpdateBanner)]
pub fn update_banner() -> Html {
    let locale = use_locale();
    let (session, _) = use_store::<SessionStore>();
    let Some(tag) = &session.update_available else {
        return html! {};
    };
    html! {
        <div class="mb-4 bg-blue-100 border border-blue-400 text-blue-700 dark:bg-blue-950 dark:border-blue-800 dark:text-blue-300 px-4 py-3 rounded" role="alert">
            {locale.tf("common.update_available", &[("version", tag)])}
            {" "}
            <Link<Route> to={Route::Settings} classes="underline font-medium">
                {locale.t("nav.settings")}
            </Link<Route>>
        </div>
    }
}
