use super::language_menu::LanguageMenu;
use super::theme_menu::ThemeMenu;
use crate::i18n::use_locale;
use crate::pages::Route;
use r3v3rs3_api::i18n::Locale;
use yew::prelude::*;
use yew_router::prelude::*;

struct MenuItem {
    /// The translation key of the name.
    name: &'static str,
    icon: &'static str,
    route: Route,
}

const ITEMS: &[MenuItem] = {
    &[
        MenuItem {
            name: "nav.ports",
            icon: "/assets/icons/wifi.svg",
            route: Route::Ports,
        },
        MenuItem {
            name: "nav.proxies",
            icon: "/assets/icons/swap-horizontal.svg",
            route: Route::Proxies,
        },
        MenuItem {
            name: "nav.certificates",
            icon: "/assets/icons/ribbon.svg",
            route: Route::Certs,
        },
        MenuItem {
            name: "nav.settings",
            icon: "/assets/icons/settings.svg",
            route: Route::Settings,
        },
    ]
};

const MENU_BUTTON_CLASS: [&str; 5] = [
    "px-3",
    "py-3",
    "flex",
    "items-center",
    "hover:bg-neutral-600",
];

#[function_component(Navbar)]
pub fn navbar() -> Html {
    let navigator = use_navigator().unwrap();
    let route = use_route::<Route>().unwrap();
    let menu_open = use_state(|| false);
    let locale = use_locale();

    let navigator_cloned = navigator.clone();
    let logout_onclick = Callback::from(move |e: MouseEvent| {
        e.prevent_default();
        if gloo_dialogs::confirm(locale.t("nav.logout_confirm")) {
            navigator_cloned.push(&Route::Logout);
        }
    });

    let navigator_cloned = navigator.clone();
    let logo_onclick = Callback::from(move |e: MouseEvent| {
        e.prevent_default();
        navigator_cloned.push(&Route::Home);
    });

    let menu_open_cloned = menu_open.clone();
    let menu_onclick = Callback::from(move |_: MouseEvent| {
        menu_open_cloned.set(!*menu_open_cloned);
    });

    if route == Route::Login {
        return html! {};
    }

    let root = route.root();
    let items = |vertical: bool| {
        root.as_ref()
            .map(|root| {
                ITEMS
                    .iter()
                    .map(|entry| {
                        let is_active = *root == entry.route;
                        menu_item(locale, entry, is_active, vertical, &navigator, &menu_open)
                    })
                    .collect::<Html>()
            })
            .unwrap_or_default()
    };

    html! {
        <nav class="w-full max-w-6xl mx-auto px-4 sm:px-6 pt-4">
            <div class="relative rounded-md text-neutral-100 bg-neutral-800 shadow-lg font-medium flex items-stretch">
                <span class="rounded-l-md flex items-center justify-center px-3 cursor-pointer bg-yellow-300" onclick={logo_onclick}>
                    <img src="/assets/logo.svg" class="object-center w-7 h-7" />
                </span>
                <div class="hidden md:flex">
                    { items(false) }
                </div>
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
                        { items(true) }
                        <button type="button" class="px-4 py-3 flex items-center gap-2 hover:bg-neutral-600" onclick={logout_onclick}>
                            <img src="/assets/icons/log-out.svg" class="w-5 h-5" />
                            {locale.t("nav.logout")}
                        </button>
                    </div>
                }
            </div>
        </nav>
    }
}

fn menu_item(
    locale: Locale,
    entry: &'static MenuItem,
    is_active: bool,
    vertical: bool,
    navigator: &Navigator,
    menu_open: &UseStateHandle<bool>,
) -> Html {
    let navigator = navigator.clone();
    let menu_open = menu_open.clone();
    let onclick = Callback::from(move |e: MouseEvent| {
        e.prevent_default();
        menu_open.set(false);
        navigator.push(&entry.route);
    });
    let layout = if vertical {
        classes!("gap-2")
    } else {
        classes!(
            "gap-2",
            "border-b-2",
            "border-neutral-800",
            is_active.then_some("border-b-neutral-100")
        )
    };
    html! {
        <button type="button" class={classes!("px-4", "py-3", "cursor-pointer", "hover:bg-neutral-600", "flex", "items-center", is_active.then_some("bg-neutral-700"), layout)} {onclick} aria-current={is_active.then_some("page")}>
            <img src={entry.icon} class="w-5 h-5" />
            <span>{locale.t(entry.name)}</span>
        </button>
    }
}
