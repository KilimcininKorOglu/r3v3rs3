use super::dropdown::Dropdown;
use crate::preferences::{PreferencesStore, set_locale};
use r3v3rs3_api::i18n::Locale;
use yew::prelude::*;
use yewdux::prelude::*;

#[derive(Properties, PartialEq)]
pub struct Props {
    /// Classes of the button that opens the menu.
    pub class: Classes,
}

/// A menu that shows each language only as the flag of its country.
#[function_component(LanguageMenu)]
pub fn language_menu(props: &Props) -> Html {
    let (preferences, dispatch) = use_store::<PreferencesStore>();
    let current = preferences.locale;

    html! {
        <Dropdown class={props.class.clone()} label={native_name(current)} button={flag(current)}>
            { for Locale::ALL.into_iter().map(|locale| language_option(locale, current, &dispatch)) }
        </Dropdown>
    }
}

fn language_option(locale: Locale, current: Locale, dispatch: &Dispatch<PreferencesStore>) -> Html {
    let dispatch = dispatch.clone();
    let onclick = Callback::from(move |_: MouseEvent| set_locale(&dispatch, locale));
    let active = locale == current;
    html! {
        <li role="none">
            <button type="button" role="menuitemradio" aria-checked={active.to_string()} aria-label={native_name(locale)} title={native_name(locale)} lang={locale.code()} {onclick} class={classes!("flex", "w-full", "items-center", "justify-center", "px-4", "py-2", "hover:bg-neutral-100", "dark:hover:bg-neutral-700", active.then_some("bg-neutral-100 dark:bg-neutral-700"))}>
                { flag(locale) }
            </button>
        </li>
    }
}

/// The name of the language in the language itself.
fn native_name(locale: Locale) -> &'static str {
    match locale {
        Locale::En => "English",
        Locale::Tr => "Türkçe",
    }
}

fn flag(locale: Locale) -> Html {
    let src = match locale {
        Locale::En => "/assets/flags/gb.svg",
        Locale::Tr => "/assets/flags/tr.svg",
    };
    html! {
        <img {src} alt="" class="w-6 h-4 shrink-0 rounded-sm object-cover shadow-sm" />
    }
}
