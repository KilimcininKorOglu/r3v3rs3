//! The standard tab bar of the WebUI: an underline in the primary color marks the active tab.

use crate::pages::Route;
use yew::prelude::*;
use yew_router::prelude::*;

/// Where a tab leads: a page of the router, or a view of the current page.
#[derive(Clone, PartialEq)]
pub enum TabTarget {
    Route(Route),
    Click(Callback<MouseEvent>),
}

#[derive(Clone, PartialEq)]
pub struct TabItem {
    pub label: AttrValue,
    pub target: TabTarget,
    pub active: bool,
    /// Shown after the label, for example a count or a close button.
    pub trailing: Option<Html>,
}

impl TabItem {
    pub fn route(label: impl Into<AttrValue>, route: Route, active: bool) -> Self {
        Self {
            label: label.into(),
            target: TabTarget::Route(route),
            active,
            trailing: None,
        }
    }

    pub fn click(label: impl Into<AttrValue>, onclick: Callback<MouseEvent>, active: bool) -> Self {
        Self {
            label: label.into(),
            target: TabTarget::Click(onclick),
            active,
            trailing: None,
        }
    }
}

const BAR: &str = "border-b border-neutral-300 dark:border-neutral-700";
const NAV: &str = "flex gap-6 overflow-x-auto -mb-px";
const TAB: &str = "inline-flex items-center gap-1.5 py-3 px-1 text-sm whitespace-nowrap border-b-2 transition-colors cursor-pointer";
const ACTIVE: &str = "border-primary-600 text-primary-600 dark:border-primary-400 dark:text-primary-400 font-semibold";
const INACTIVE: &str = "border-transparent text-neutral-500 hover:text-neutral-700 hover:border-neutral-300 dark:text-neutral-400 dark:hover:text-neutral-200 dark:hover:border-neutral-600";

/// The classes of one tab.
pub fn tab_class(active: bool) -> Classes {
    classes!(TAB, if active { ACTIVE } else { INACTIVE })
}

#[derive(Properties, PartialEq)]
pub struct TabsProps {
    pub tabs: Vec<TabItem>,
}

#[function_component(Tabs)]
pub fn tabs(props: &TabsProps) -> Html {
    html! {
        <div class={BAR}>
            <nav class={NAV}>
                { for props.tabs.iter().map(render_tab) }
            </nav>
        </div>
    }
}

fn render_tab(tab: &TabItem) -> Html {
    let class = tab_class(tab.active);
    let current = tab.active.then_some("page");
    let body = html! {
        <>
            { tab.label.clone() }
            if let Some(trailing) = &tab.trailing {
                { trailing.clone() }
            }
        </>
    };
    match &tab.target {
        TabTarget::Route(route) => html! {
            <Link<Route> to={route.clone()} classes={class}>{ body }</Link<Route>>
        },
        TabTarget::Click(onclick) => html! {
            <button type="button" {class} aria-current={current} onclick={onclick.clone()}>{ body }</button>
        },
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_active_tab_uses_the_primary_color() {
        let active = tab_class(true).to_string();
        assert!(active.contains("border-primary-600") && active.contains("text-primary-600"));
        assert!(active.contains("dark:text-primary-400"));
    }

    #[test]
    fn an_inactive_tab_has_no_primary_color() {
        let inactive = tab_class(false).to_string();
        assert!(!inactive.contains("primary"));
        assert!(inactive.contains("border-transparent"));
    }
}
