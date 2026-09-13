use super::dropdown::Dropdown;
use crate::preferences::{set_theme, PreferencesStore};
use r3v3rs3_api::i18n::{Locale, Theme};
use yew::prelude::*;
use yewdux::prelude::*;

#[derive(Properties, PartialEq)]
pub struct Props {
    /// Classes of the button that opens the menu.
    pub class: Classes,
}

#[function_component(ThemeMenu)]
pub fn theme_menu(props: &Props) -> Html {
    let (preferences, dispatch) = use_store::<PreferencesStore>();
    let locale = preferences.locale;

    html! {
        <Dropdown class={props.class.clone()} menu_class="w-40" label={label(locale, preferences.theme)} button={icon(preferences.theme)}>
            { for Theme::ALL.into_iter().map(|theme| theme_option(locale, theme, preferences.theme, &dispatch)) }
        </Dropdown>
    }
}

fn theme_option(
    locale: Locale,
    theme: Theme,
    current: Theme,
    dispatch: &Dispatch<PreferencesStore>,
) -> Html {
    let dispatch = dispatch.clone();
    let onclick = Callback::from(move |_: MouseEvent| set_theme(&dispatch, theme));
    let active = theme == current;
    html! {
        <li role="none">
            <button type="button" role="menuitemradio" aria-checked={active.to_string()} {onclick} class={classes!("flex", "w-full", "items-center", "gap-2", "px-3", "py-2", "hover:bg-neutral-100", "dark:hover:bg-neutral-700", active.then_some("font-semibold"))}>
                { icon(theme) }
                { label(locale, theme) }
            </button>
        </li>
    }
}

fn label(locale: Locale, theme: Theme) -> &'static str {
    locale.t(match theme {
        Theme::System => "theme.system",
        Theme::Light => "theme.light",
        Theme::Dark => "theme.dark",
    })
}

/// Outline icons from ionicons (MIT License). They use the current text color.
fn icon(theme: Theme) -> Html {
    match theme {
        Theme::System => html! {
            <svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 512 512" class="w-5 h-5 shrink-0" aria-hidden="true" fill="none" stroke="currentColor" stroke-linecap="round" stroke-linejoin="round" stroke-width="32">
                <rect x="32" y="64" width="448" height="320" rx="32" ry="32" />
                <path d="M304 448l-8-64h-80l-8 64h96zM368 448H144" />
            </svg>
        },
        Theme::Light => html! {
            <svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 512 512" class="w-5 h-5 shrink-0" aria-hidden="true" fill="none" stroke="currentColor" stroke-linecap="round" stroke-miterlimit="10" stroke-width="32">
                <path d="M256 48v48M256 416v48M403.08 108.92l-33.94 33.94M142.86 369.14l-33.94 33.94M464 256h-48M96 256H48M403.08 403.08l-33.94-33.94M142.86 142.86l-33.94-33.94" />
                <circle cx="256" cy="256" r="80" />
            </svg>
        },
        Theme::Dark => html! {
            <svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 512 512" class="w-5 h-5 shrink-0" aria-hidden="true" fill="none" stroke="currentColor" stroke-linecap="round" stroke-linejoin="round" stroke-width="32">
                <path d="M160 136c0-30.62 4.51-61.61 16-88C99.57 81.27 48 159.32 48 248c0 119.29 96.71 216 216 216 88.68 0 166.73-51.57 200-128-26.39 11.49-57.38 16-88 16-119.29 0-216-96.71-216-216z" />
            </svg>
        },
    }
}
