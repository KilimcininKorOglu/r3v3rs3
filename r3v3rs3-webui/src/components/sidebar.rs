use crate::i18n::use_locale;
use crate::pages::Route;
use crate::store::SessionStore;
use r3v3rs3_api::i18n::Locale;
use yew::prelude::*;
use yew_router::prelude::*;
use yewdux::prelude::*;

/// The accounts that see a menu item.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Access {
    All,
    AdminOnly,
    /// An account without a proxy list.
    Platform,
}

struct MenuItem {
    /// The translation key of the name.
    name: &'static str,
    icon: &'static str,
    route: Route,
    access: Access,
}

struct MenuGroup {
    /// The translation key of the heading.
    name: &'static str,
    items: &'static [MenuItem],
}

const GROUPS: &[MenuGroup] = &[
    MenuGroup {
        name: "nav.group_proxy",
        items: &[
            MenuItem {
                name: "nav.ports",
                icon: "/assets/icons/wifi.svg",
                route: Route::Ports,
                access: Access::All,
            },
            MenuItem {
                name: "nav.proxies",
                icon: "/assets/icons/swap-horizontal.svg",
                route: Route::Proxies,
                access: Access::All,
            },
            MenuItem {
                name: "nav.access_lists",
                icon: "/assets/icons/shield-checkmark.svg",
                route: Route::AccessLists,
                access: Access::All,
            },
            MenuItem {
                name: "nav.certificates",
                icon: "/assets/icons/ribbon.svg",
                route: Route::Certs,
                access: Access::All,
            },
        ],
    },
    MenuGroup {
        name: "nav.group_platform",
        items: &[
            MenuItem {
                name: "nav.apps",
                icon: "/assets/icons/cube.svg",
                route: Route::Apps,
                access: Access::Platform,
            },
            MenuItem {
                name: "nav.targets",
                icon: "/assets/icons/server.svg",
                route: Route::Targets,
                access: Access::Platform,
            },
        ],
    },
    MenuGroup {
        name: "nav.group_admin",
        items: &[
            MenuItem {
                name: "nav.accounts",
                icon: "/assets/icons/person.svg",
                route: Route::Accounts,
                access: Access::AdminOnly,
            },
            MenuItem {
                name: "nav.audit",
                icon: "/assets/icons/document-text.svg",
                route: Route::Audit,
                access: Access::AdminOnly,
            },
            MenuItem {
                name: "nav.settings",
                icon: "/assets/icons/settings.svg",
                route: Route::Settings,
                access: Access::AdminOnly,
            },
        ],
    },
];

impl Access {
    fn allows(self, session: &SessionStore) -> bool {
        match self {
            Access::All => true,
            Access::AdminOnly => session.is_admin(),
            Access::Platform => session.can_read_platform(),
        }
    }
}

#[derive(Properties, PartialEq)]
pub struct MenuProps {
    /// Called after a click on an item, so that the mobile menu closes.
    #[prop_or_default]
    pub onselect: Callback<()>,
}

/// The menu items in their groups. Groups without an item that the account sees are left out.
#[function_component(Menu)]
pub fn menu(props: &MenuProps) -> Html {
    let locale = use_locale();
    let navigator = use_navigator().unwrap();
    let route = use_route::<Route>().unwrap();
    let (session, _) = use_store::<SessionStore>();
    let root = route.root();

    GROUPS
        .iter()
        .map(|group| {
            let items = group
                .items
                .iter()
                .filter(|item| item.access.allows(&session))
                .map(|item| {
                    let active = root.as_ref() == Some(&item.route);
                    menu_item(locale, item, active, &navigator, &props.onselect)
                })
                .collect::<Vec<_>>();
            if items.is_empty() {
                return html! {};
            }
            html! {
                <div class="py-2">
                    <div class="px-4 pb-1 text-xs font-semibold uppercase tracking-wide text-neutral-400">
                        {locale.t(group.name)}
                    </div>
                    { for items }
                </div>
            }
        })
        .collect()
}

fn menu_item(
    locale: Locale,
    item: &'static MenuItem,
    active: bool,
    navigator: &Navigator,
    onselect: &Callback<()>,
) -> Html {
    let navigator = navigator.clone();
    let onselect = onselect.clone();
    let onclick = Callback::from(move |e: MouseEvent| {
        e.prevent_default();
        onselect.emit(());
        navigator.push(&item.route);
    });
    html! {
        <button type="button" class={classes!("w-full", "px-4", "py-2", "flex", "items-center", "gap-3", "cursor-pointer", "border-l-4", "hover:bg-neutral-600", if active { "bg-neutral-700 border-neutral-100" } else { "border-transparent" })} {onclick} aria-current={active.then_some("page")}>
            <img src={item.icon} class="w-5 h-5" />
            <span>{locale.t(item.name)}</span>
        </button>
    }
}

/// The menu on the left side of a wide screen. A narrow screen opens the same menu from the
/// header.
#[function_component(Sidebar)]
pub fn sidebar() -> Html {
    let route = use_route::<Route>().unwrap();
    if route == Route::Login {
        return html! {};
    }
    html! {
        <aside class="hidden md:block w-56 shrink-0">
            <nav class="sticky top-4 rounded-md text-neutral-100 bg-neutral-800 shadow-lg font-medium py-1">
                <Menu />
            </nav>
        </aside>
    }
}
