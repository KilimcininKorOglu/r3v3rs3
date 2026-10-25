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
    /// An account without a proxy list that changes the platform.
    PlatformEdit,
}

/// A page of one menu item, shown indented below it.
struct MenuSubItem {
    /// The translation key of the name.
    name: &'static str,
    /// The route of the page.
    route: Route,
}

struct MenuItem {
    /// The translation key of the name.
    name: &'static str,
    icon: &'static str,
    route: Route,
    access: Access,
    /// The pages below the item, indented in the sidebar.
    subs: &'static [MenuSubItem],
}

struct MenuGroup {
    /// The translation key of the heading.
    name: &'static str,
    items: &'static [MenuItem],
}

/// The sub pages of the certificates menu item.
const CERT_SUBS: &[MenuSubItem] = &[
    MenuSubItem {
        name: "certs.tab_server",
        route: Route::Certs,
    },
    MenuSubItem {
        name: "certs.tab_client",
        route: Route::ClientCerts,
    },
    MenuSubItem {
        name: "certs.tab_root",
        route: Route::RootCerts,
    },
    MenuSubItem {
        name: "certs.tab_acme",
        route: Route::AcmeCerts,
    },
    MenuSubItem {
        name: "certs.tab_dns_credentials",
        route: Route::DnsCredentials,
    },
    MenuSubItem {
        name: "certs.tab_ct_logs",
        route: Route::CtLogs,
    },
];

const NO_SUBS: &[MenuSubItem] = &[];

const GROUPS: &[MenuGroup] = &[
    MenuGroup {
        name: "nav.group_proxy",
        items: &[
            MenuItem {
                name: "nav.dashboard",
                icon: "/assets/icons/server.svg",
                route: Route::Home,
                access: Access::All,
                subs: NO_SUBS,
            },
            MenuItem {
                name: "nav.ports",
                icon: "/assets/icons/wifi.svg",
                route: Route::Ports,
                access: Access::All,
                subs: NO_SUBS,
            },
            MenuItem {
                name: "nav.proxies",
                icon: "/assets/icons/swap-horizontal.svg",
                route: Route::Proxies,
                access: Access::All,
                subs: NO_SUBS,
            },
            MenuItem {
                name: "nav.traffic",
                icon: "/assets/icons/document-text.svg",
                route: Route::Traffic,
                access: Access::All,
                subs: NO_SUBS,
            },
            MenuItem {
                name: "nav.access_lists",
                icon: "/assets/icons/shield-checkmark.svg",
                route: Route::AccessLists,
                access: Access::All,
                subs: NO_SUBS,
            },
            MenuItem {
                name: "nav.certificates",
                icon: "/assets/icons/ribbon.svg",
                route: Route::Certs,
                access: Access::All,
                subs: CERT_SUBS,
            },
        ],
    },
    MenuGroup {
        name: "nav.group_platform",
        items: &[
            MenuItem {
                name: "nav.projects",
                icon: "/assets/icons/folder.svg",
                route: Route::Projects,
                access: Access::Platform,
                subs: NO_SUBS,
            },
            MenuItem {
                name: "nav.apps",
                icon: "/assets/icons/cube.svg",
                route: Route::Apps,
                access: Access::Platform,
                subs: NO_SUBS,
            },
            MenuItem {
                name: "nav.app_store",
                icon: "/assets/icons/create.svg",
                route: Route::AppStore,
                access: Access::PlatformEdit,
                subs: NO_SUBS,
            },
            MenuItem {
                name: "nav.databases",
                icon: "/assets/icons/database.svg",
                route: Route::Databases,
                access: Access::PlatformEdit,
                subs: NO_SUBS,
            },
            MenuItem {
                name: "nav.backups",
                icon: "/assets/icons/cloud-upload.svg",
                route: Route::Backups,
                access: Access::PlatformEdit,
                subs: NO_SUBS,
            },
            MenuItem {
                name: "nav.targets",
                icon: "/assets/icons/server.svg",
                route: Route::Targets,
                access: Access::Platform,
                subs: NO_SUBS,
            },
            MenuItem {
                name: "nav.git_connections",
                icon: "/assets/icons/git-branch.svg",
                route: Route::GitConnections,
                access: Access::PlatformEdit,
                subs: NO_SUBS,
            },
            MenuItem {
                name: "nav.registries",
                icon: "/assets/icons/cube.svg",
                route: Route::Registries,
                access: Access::Platform,
                subs: NO_SUBS,
            },
            MenuItem {
                name: "nav.env_groups",
                icon: "/assets/icons/document-text.svg",
                route: Route::EnvGroups,
                access: Access::Platform,
                subs: NO_SUBS,
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
                subs: NO_SUBS,
            },
            MenuItem {
                name: "nav.audit",
                icon: "/assets/icons/document-text.svg",
                route: Route::Audit,
                access: Access::AdminOnly,
                subs: NO_SUBS,
            },
            MenuItem {
                name: "nav.webhooks",
                icon: "/assets/icons/swap-horizontal.svg",
                route: Route::Webhooks,
                access: Access::AdminOnly,
                subs: NO_SUBS,
            },
            MenuItem {
                name: "nav.settings",
                icon: "/assets/icons/settings.svg",
                route: Route::Settings,
                access: Access::AdminOnly,
                subs: NO_SUBS,
            },
        ],
    },
];

impl Access {
    fn allows(self, session: &SessionStore) -> bool {
        match self {
            Access::All => true,
            Access::AdminOnly => session.is_admin(),
            Access::Platform => session.can_see_platform(),
            Access::PlatformEdit => session.can_see_platform() && session.can_edit(),
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
/// A sub item of the active item is indented below it.
#[function_component(Menu)]
pub fn menu(props: &MenuProps) -> Html {
    let locale = use_locale();
    let Some(navigator) = use_navigator() else {
        return html! {};
    };
    let Some(route) = use_route::<Route>() else {
        return html! {};
    };
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
                    menu_item(locale, item, active, &route, &navigator, &props.onselect)
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
    route: &Route,
    navigator: &Navigator,
    onselect: &Callback<()>,
) -> Html {
    let nav = navigator.clone();
    let select = onselect.clone();
    let onclick = Callback::from(move |e: MouseEvent| {
        e.prevent_default();
        select.emit(());
        nav.push(&item.route);
    });
    let subs = active.then(|| {
        item.subs
            .iter()
            .map(|sub| menu_sub_item(locale, sub, route, navigator, onselect))
            .collect::<Vec<_>>()
    });
    html! {
        <>
            <button type="button" class={classes!("w-full", "px-4", "py-2", "flex", "items-center", "gap-3", "cursor-pointer", "border-l-4", "hover:bg-neutral-600", if active { "bg-neutral-700 border-neutral-100" } else { "border-transparent" })} {onclick} aria-current={active.then_some("page")}>
                <img src={item.icon} class="w-5 h-5" />
                <span>{locale.t(item.name)}</span>
            </button>
            { for subs.unwrap_or_default() }
        </>
    }
}

fn menu_sub_item(
    locale: Locale,
    sub: &'static MenuSubItem,
    route: &Route,
    navigator: &Navigator,
    onselect: &Callback<()>,
) -> Html {
    let navigator = navigator.clone();
    let onselect = onselect.clone();
    let target = sub.route.clone();
    let active = route == &target;
    let onclick = Callback::from(move |e: MouseEvent| {
        e.prevent_default();
        onselect.emit(());
        navigator.push(&target);
    });
    html! {
        <button type="button" class={classes!("w-full", "pl-11", "pr-4", "py-1.5", "flex", "items-center", "cursor-pointer", "text-sm", "text-left", "hover:bg-neutral-600", if active { "bg-neutral-700 text-neutral-100" } else { "text-neutral-300" })} {onclick} aria-current={active.then_some("page")}>
            {locale.t(sub.name)}
        </button>
    }
}

/// The menu on the left side of a wide screen. A narrow screen opens the same menu from the
/// header.
#[function_component(Sidebar)]
pub fn sidebar() -> Html {
    let Some(route) = use_route::<Route>() else {
        return html! {};
    };
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
