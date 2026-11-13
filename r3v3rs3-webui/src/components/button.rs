//! The standard buttons of the WebUI. Every button that is not a row action icon takes its
//! colors from here, so a kind of action has the same color on every page.

use yew::prelude::*;

/// The kind of an action, which sets the color of its button.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum ButtonKind {
    /// Save, create, add, apply and test.
    Primary,
    /// Cancel, back, refresh, filters and toolbars.
    Secondary,
    /// Delete and other actions that destroy data.
    Danger,
    /// Deploy, run, take a backup and start a restore.
    Success,
    /// Stop, maintenance and drain.
    Warning,
}

#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
pub enum ButtonSize {
    Sm,
    #[default]
    Md,
}

const BASE: &str = "inline-flex items-center justify-center gap-1.5 font-medium rounded-lg transition-colors focus:outline-none focus:ring-4 disabled:opacity-50 disabled:cursor-not-allowed cursor-pointer whitespace-nowrap";

const SM: &str = "text-xs px-3 py-1.5";
const MD: &str = "text-sm px-4 py-2";

const PRIMARY: &str = "text-white bg-primary-600 hover:bg-primary-700 focus:ring-primary-300 dark:bg-primary-600 dark:hover:bg-primary-700 dark:focus:ring-primary-800";
const SECONDARY: &str = "text-neutral-700 bg-white border border-neutral-300 hover:bg-neutral-100 focus:ring-neutral-200 dark:text-neutral-200 dark:bg-neutral-800 dark:border-neutral-600 dark:hover:bg-neutral-700 dark:focus:ring-neutral-700";
const DANGER: &str = "text-white bg-red-600 hover:bg-red-700 focus:ring-red-300 dark:bg-red-600 dark:hover:bg-red-700 dark:focus:ring-red-900";
const SUCCESS: &str = "text-white bg-emerald-600 hover:bg-emerald-700 focus:ring-emerald-300 dark:bg-emerald-600 dark:hover:bg-emerald-700 dark:focus:ring-emerald-900";
const WARNING: &str = "text-white bg-orange-500 hover:bg-orange-600 focus:ring-orange-300 dark:bg-orange-500 dark:hover:bg-orange-600 dark:focus:ring-orange-900";

fn kind_class(kind: ButtonKind) -> &'static str {
    match kind {
        ButtonKind::Primary => PRIMARY,
        ButtonKind::Secondary => SECONDARY,
        ButtonKind::Danger => DANGER,
        ButtonKind::Success => SUCCESS,
        ButtonKind::Warning => WARNING,
    }
}

/// The classes of a button of `kind`. Links that look like buttons (`<a>`, `Link<Route>`) use
/// the same classes.
pub fn button_class(kind: ButtonKind, size: ButtonSize) -> Classes {
    let size = match size {
        ButtonSize::Sm => SM,
        ButtonSize::Md => MD,
    };
    classes!(BASE, size, kind_class(kind))
}

#[derive(Properties, PartialEq)]
pub struct ButtonProps {
    pub kind: ButtonKind,
    #[prop_or_default]
    pub size: ButtonSize,
    /// `button` unless set; a form's save button passes `submit`.
    #[prop_or("button")]
    pub r#type: &'static str,
    #[prop_or_default]
    pub disabled: bool,
    #[prop_or_default]
    pub onclick: Option<Callback<MouseEvent>>,
    #[prop_or_default]
    pub title: Option<AttrValue>,
    #[prop_or_default]
    pub class: Classes,
    #[prop_or_default]
    pub children: Html,
}

#[function_component(Button)]
pub fn button(props: &ButtonProps) -> Html {
    let class = classes!(button_class(props.kind, props.size), props.class.clone());
    html! {
        <button
            type={props.r#type}
            {class}
            disabled={props.disabled}
            title={props.title.clone()}
            onclick={props.onclick.clone()}
        >
            { props.children.clone() }
        </button>
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const KINDS: [ButtonKind; 5] = [
        ButtonKind::Primary,
        ButtonKind::Secondary,
        ButtonKind::Danger,
        ButtonKind::Success,
        ButtonKind::Warning,
    ];

    #[test]
    fn every_kind_has_its_own_color() {
        let colors: std::collections::HashSet<&str> =
            KINDS.iter().map(|k| kind_class(*k)).collect();
        assert_eq!(colors.len(), KINDS.len());
    }

    #[test]
    fn every_kind_has_a_dark_variant() {
        for kind in KINDS {
            assert!(
                kind_class(kind).contains("dark:"),
                "{kind:?} has no dark classes"
            );
        }
    }

    #[test]
    fn primary_uses_the_theme_token() {
        assert!(PRIMARY.contains("bg-primary-600"));
    }

    #[test]
    fn the_size_sets_the_padding() {
        let sm = button_class(ButtonKind::Primary, ButtonSize::Sm).to_string();
        let md = button_class(ButtonKind::Primary, ButtonSize::Md).to_string();
        assert!(sm.contains("px-3") && sm.contains("text-xs"));
        assert!(md.contains("px-4") && md.contains("text-sm"));
    }
}
