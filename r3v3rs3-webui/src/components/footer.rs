use crate::{i18n::use_locale, pages::Route, API_ENDPOINT};
use gloo_net::http::Request;
use r3v3rs3_api::i18n::Locale;
use serde_derive::Deserialize;
use yew::prelude::*;
use yew_router::prelude::*;

const REPOSITORY_URL: &str = "https://github.com/KilimcininKorOglu/r3v3rs3";
const DOCS_URL_EN: &str = "https://kilimcininkoroglu.github.io/r3v3rs3/";
const DOCS_URL_TR: &str = "https://kilimcininkoroglu.github.io/r3v3rs3/tr/";
const LINK_CLASS: &str = "hover:underline hover:text-neutral-900 dark:hover:text-neutral-200";

#[derive(Deserialize)]
struct AppVersion {
    version: String,
}

/// Shows the server version and the project links. The version comes from `/api/app_info`,
/// which requires a session, so the footer stays empty on the login page.
#[function_component(Footer)]
pub fn footer() -> Html {
    let locale = use_locale();
    let is_login = use_route::<Route>() == Some(Route::Login);
    let version = use_state(|| Option::<String>::None);

    let version_cloned = version.clone();
    use_effect_with(is_login, move |is_login| {
        version_cloned.set(None);
        if !*is_login {
            wasm_bindgen_futures::spawn_local(async move {
                if let Ok(info) = get_app_version().await {
                    version_cloned.set(Some(info.version));
                }
            });
        }
    });

    let Some(version) = (*version).clone().filter(|_| !is_login) else {
        return html! {};
    };

    html! {
        <footer class="w-full max-w-6xl mx-auto px-4 sm:px-6 pt-2 pb-6 flex flex-wrap items-center justify-center gap-x-3 gap-y-1 text-sm text-neutral-500 dark:text-neutral-400">
            <span>{format!("r3v3rs3 v{version}")}</span>
            <span aria-hidden="true">{"·"}</span>
            <a href={docs_url(locale)} target="_blank" rel="noopener noreferrer" class={LINK_CLASS}>{locale.t("footer.documentation")}</a>
            <span aria-hidden="true">{"·"}</span>
            <a href={REPOSITORY_URL} target="_blank" rel="noopener noreferrer" class={LINK_CLASS}>{"GitHub"}</a>
        </footer>
    }
}

fn docs_url(locale: Locale) -> &'static str {
    match locale {
        Locale::En => DOCS_URL_EN,
        Locale::Tr => DOCS_URL_TR,
    }
}

async fn get_app_version() -> Result<AppVersion, gloo_net::Error> {
    Request::get(&format!("{API_ENDPOINT}/app_info"))
        .send()
        .await?
        .json()
        .await
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn docs_url_follows_locale() {
        assert_eq!(
            docs_url(Locale::En),
            "https://kilimcininkoroglu.github.io/r3v3rs3/"
        );
        assert_eq!(
            docs_url(Locale::Tr),
            "https://kilimcininkoroglu.github.io/r3v3rs3/tr/"
        );
    }
}
