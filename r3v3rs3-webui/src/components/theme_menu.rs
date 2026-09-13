use crate::preferences::{set_theme, PreferencesStore};
use r3v3rs3_api::i18n::Theme;
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
    let open = use_state(|| false);

    let toggle_onclick = {
        let open = open.clone();
        Callback::from(move |_: MouseEvent| open.set(!*open))
    };
    let close_onclick = {
        let open = open.clone();
        Callback::from(move |_: MouseEvent| open.set(false))
    };

    html! {
        <div class="relative flex items-stretch">
            <button type="button" onclick={toggle_onclick} class={props.class.clone()} title={label(preferences.theme)} aria-label={label(preferences.theme)} aria-haspopup="menu" aria-expanded={open.to_string()}>
                { icon(preferences.theme) }
            </button>
            if *open {
                <div class="fixed inset-0 z-10" onclick={close_onclick}></div>
                <ul role="menu" class="absolute right-0 top-full z-20 mt-1 w-40 py-1 rounded-md shadow-lg border border-neutral-300 dark:border-neutral-700 bg-white dark:bg-neutral-800 text-sm text-neutral-700 dark:text-neutral-200">
                    { for Theme::ALL.into_iter().map(|theme| theme_option(theme, preferences.theme, &dispatch, &open)) }
                </ul>
            }
        </div>
    }
}

fn theme_option(
    theme: Theme,
    current: Theme,
    dispatch: &Dispatch<PreferencesStore>,
    open: &UseStateHandle<bool>,
) -> Html {
    let dispatch = dispatch.clone();
    let open = open.clone();
    let onclick = Callback::from(move |_: MouseEvent| {
        set_theme(&dispatch, theme);
        open.set(false);
    });
    let active = theme == current;
    html! {
        <li role="none">
            <button type="button" role="menuitemradio" aria-checked={active.to_string()} {onclick} class={classes!("flex", "w-full", "items-center", "gap-2", "px-3", "py-2", "hover:bg-neutral-100", "dark:hover:bg-neutral-700", active.then_some("font-semibold"))}>
                { icon(theme) }
                { label(theme) }
            </button>
        </li>
    }
}

fn label(theme: Theme) -> &'static str {
    match theme {
        Theme::System => "System",
        Theme::Light => "Light",
        Theme::Dark => "Dark",
    }
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
