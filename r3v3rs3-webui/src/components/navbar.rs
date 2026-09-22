use super::language_menu::LanguageMenu;
use super::sidebar::Menu;
use super::theme_menu::ThemeMenu;
use crate::dialog;
use crate::i18n::use_locale;
use crate::pages::Route;
use yew::prelude::*;
use yew_router::prelude::*;

const MENU_BUTTON_CLASS: [&str; 5] = [
    "px-3",
    "py-3",
    "flex",
    "items-center",
    "hover:bg-neutral-600",
];

/// The header: the logo, the language, the theme and the session. The navigation lives in the
/// sidebar, and a narrow screen opens it from the menu button of the header.
#[function_component(Navbar)]
pub fn navbar() -> Html {
    let navigator = use_navigator().unwrap();
    let route = use_route::<Route>().unwrap();
    let menu_open = use_state(|| false);
    let locale = use_locale();

    let navigator_cloned = navigator.clone();
    let logout_onclick = Callback::from(move |e: MouseEvent| {
        e.prevent_default();
        let navigator = navigator_cloned.clone();
        dialog::confirm_then(locale, locale.t("nav.logout_confirm").into(), async move {
            navigator.push(&Route::Logout);
        });
    });

    let logo_onclick = Callback::from(move |e: MouseEvent| {
        e.prevent_default();
        navigator.push(&Route::Home);
    });

    let menu_open_cloned = menu_open.clone();
    let menu_onclick = Callback::from(move |_: MouseEvent| {
        menu_open_cloned.set(!*menu_open_cloned);
    });

    let menu_open_cloned = menu_open.clone();
    let menu_onselect = Callback::from(move |()| menu_open_cloned.set(false));

    if route == Route::Login {
        return html! {};
    }

    html! {
        <nav class="w-full max-w-7xl mx-auto px-4 sm:px-6 pt-4">
            <div class="relative rounded-md text-neutral-100 bg-neutral-800 shadow-lg font-medium flex items-stretch">
                <span class="rounded-l-md flex items-center justify-center px-3 cursor-pointer bg-yellow-300" onclick={logo_onclick}>
                    <img src="/assets/logo.svg" class="object-center w-7 h-7" />
                </span>
                <span class="hidden sm:flex items-center px-4 font-semibold">{"r3v3rs3"}</span>
                <div class="flex ml-auto">
                    <LanguageMenu class={classes!(MENU_BUTTON_CLASS.to_vec())} />
                    <ThemeMenu class={classes!(MENU_BUTTON_CLASS.to_vec())} />
                    <button type="button" class="hidden md:flex rounded-r-md px-4 py-3 cursor-pointer hover:bg-neutral-600 items-center" onclick={logout_onclick.clone()}>
                        <img src="/assets/icons/log-out.svg" class="w-5 h-5" />
                        <span class="ml-2">{locale.t("nav.logout")}</span>
                    </button>
                    <button type="button" class="md:hidden rounded-r-md px-4 py-3 flex items-center hover:bg-neutral-600" onclick={menu_onclick} aria-label={locale.t("nav.menu")} aria-expanded={menu_open.to_string()}>
                        <svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 512 512" class="w-6 h-6" aria-hidden="true" fill="none" stroke="currentColor" stroke-linecap="round" stroke-miterlimit="10" stroke-width="32">
                            <path d="M80 160h352M80 256h352M80 352h352" />
                        </svg>
                    </button>
                </div>
                if *menu_open {
                    <div class="md:hidden absolute left-0 right-0 top-full z-30 mt-1 py-1 flex flex-col rounded-md bg-neutral-800 shadow-lg">
                        <Menu onselect={menu_onselect} />
                        <button type="button" class="px-4 py-3 flex items-center gap-3 hover:bg-neutral-600 border-t border-neutral-700" onclick={logout_onclick}>
                            <img src="/assets/icons/log-out.svg" class="w-5 h-5" />
                            {locale.t("nav.logout")}
                        </button>
                    </div>
                }
            </div>
        </nav>
    }
}
