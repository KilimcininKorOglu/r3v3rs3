//! The back button of every page. It is the first item of the title row, so it stays in the same
//! place on every page.

use crate::components::button::{Button, ButtonKind, ButtonSize, button_class};
use crate::i18n::use_locale;
use crate::pages::Route;
use yew::prelude::*;
use yew_router::prelude::*;

#[derive(Properties, PartialEq)]
pub struct Props {
    /// The page that the button opens. `None` goes back in the history of the browser.
    #[prop_or_default]
    pub to: Option<Route>,
}

#[function_component(BackButton)]
pub fn back_button(props: &Props) -> Html {
    let locale = use_locale();
    let navigator = use_navigator();
    let content = html! {
        <>
            <img src="/assets/icons/arrow-back.svg" alt="" class="w-4 h-4 dark:invert" />
            {locale.t("common.back")}
        </>
    };
    match &props.to {
        Some(route) => html! {
            <Link<Route> to={route.clone()} classes={button_class(ButtonKind::Secondary, ButtonSize::Sm)}>
                { content }
            </Link<Route>>
        },
        None => {
            let onclick = Callback::from(move |_: MouseEvent| {
                if let Some(navigator) = &navigator {
                    navigator.back();
                }
            });
            html! {
                <Button kind={ButtonKind::Secondary} size={ButtonSize::Sm} {onclick}>
                    { content }
                </Button>
            }
        }
    }
}
