use r3v3rs3_api::error::Error;
use r3v3rs3_api::i18n::Locale;
use r3v3rs3_api::redirect::RedirectStatus;
use std::collections::HashMap;
use wasm_bindgen::{JsCast, UnwrapThrowExt};
use web_sys::{HtmlInputElement, HtmlSelectElement, HtmlTextAreaElement};
use yew::prelude::*;

pub const LABEL_CLASS: &str =
    "block mt-4 mb-2 text-sm font-medium text-neutral-900 dark:text-neutral-200";
pub const SECTION_CLASS: &str =
    "block mt-6 text-sm font-medium text-neutral-900 dark:text-neutral-200";
pub const INPUT_CLASS: &str = "bg-neutral-50 dark:text-neutral-200 dark:bg-neutral-800 dark:border-neutral-600 border border-neutral-300 text-neutral-900 text-sm rounded-lg focus:ring-blue-500 focus:border-blue-500 block w-full p-2.5";
pub const HINT_CLASS: &str = "mt-2 text-sm text-neutral-500 dark:text-neutral-400";
pub const ERROR_CLASS: &str = "mt-2 text-sm text-red-600 dark:text-red-500";
pub const TOGGLE_CLASS: &str = "shrink-0 w-9 h-5 bg-neutral-200 dark:bg-neutral-600 peer-focus:outline-none peer-focus:ring-4 peer-focus:ring-blue-300 rounded-full peer peer-checked:after:translate-x-full peer-checked:after:border-white after:content-[''] after:absolute after:top-[2px] after:left-[2px] after:bg-white after:border-neutral-300 after:border after:rounded-full after:h-4 after:w-4 after:transition-all peer-checked:bg-blue-600";

pub fn toggle(
    onchange: Callback<Event>,
    checked: bool,
    label: &'static str,
    margin: &'static str,
) -> Html {
    html! {
        <label class={classes!("relative", "inline-flex", "items-center", "cursor-pointer", margin)}>
            <input {onchange} type="checkbox" {checked} class="sr-only peer" />
            <div class={TOGGLE_CLASS}></div>
            <span class="ml-3 text-sm font-medium text-neutral-900 dark:text-neutral-200">{label}</span>
        </label>
    }
}

pub fn error_view(error: Option<&String>) -> Html {
    match error {
        Some(error) => html! { <p class={ERROR_CLASS}>{error.clone()}</p> },
        None => html! {},
    }
}

/// A labeled select element with an optional hint below it.
pub fn select_field(
    label: &str,
    onchange: Callback<Event>,
    options: Html,
    hint: Option<&str>,
) -> Html {
    html! {
        <>
            <label class={LABEL_CLASS}>{label.to_string()}</label>
            <select {onchange} class={INPUT_CLASS}>
                { options }
            </select>
            if let Some(hint) = hint {
                <p class={HINT_CLASS}>{hint.to_string()}</p>
            }
        </>
    }
}

/// Returns a change handler of a select element that stores the parsed value in the state.
pub fn select_setter<T: 'static>(
    state: &UseStateHandle<T>,
    parse: fn(&str) -> T,
) -> Callback<Event> {
    let state = state.clone();
    Callback::from(move |event: Event| {
        let target: HtmlSelectElement = event.target().unwrap_throw().dyn_into().unwrap_throw();
        state.set(parse(&target.value()));
    })
}

pub fn input_element(event: &Event) -> HtmlInputElement {
    event.target().unwrap_throw().dyn_into().unwrap_throw()
}

/// Returns a change handler of an input element that stores its value in the state.
pub fn text_setter(state: &UseStateHandle<String>) -> Callback<Event> {
    let state = state.clone();
    Callback::from(move |event: Event| state.set(text(&event)))
}

/// Emits `entry` to `onchanged` when it differs from the last emitted entry. Returns the errors
/// of `entry`.
#[hook]
pub fn use_entry_errors<T>(
    entry: Result<T, HashMap<String, String>>,
    onchanged: Callback<Result<T, HashMap<String, String>>>,
) -> HashMap<String, String>
where
    T: Clone + PartialEq + 'static,
{
    let prev_entry = use_state::<Result<T, HashMap<String, String>>, _>(|| Err(Default::default()));
    if entry != *prev_entry {
        prev_entry.set(entry.clone());
        onchanged.emit(entry.clone());
    }
    entry.err().unwrap_or_default()
}

pub fn select_value(event: &Event) -> String {
    let select: HtmlSelectElement = event.target().unwrap_throw().dyn_into().unwrap_throw();
    select.value()
}

pub fn text(event: &Event) -> String {
    input_element(event).value()
}

pub fn text_area(event: &Event) -> String {
    let area: HtmlTextAreaElement = event.target().unwrap_throw().dyn_into().unwrap_throw();
    area.value()
}

pub fn checked(event: &Event) -> bool {
    input_element(event).checked()
}

/// Returns the option whose name is the value of the select element, or the default option.
pub fn select_option<T: Copy + Default>(event: &Event, options: &[T], name: fn(&T) -> &str) -> T {
    let value = select_value(event);
    options
        .iter()
        .find(|option| name(option) == value)
        .copied()
        .unwrap_or_default()
}

/// The status code of a select element whose options are only valid codes.
pub fn status_code(event: &Event) -> u16 {
    select_value(event).parse().unwrap_or_default()
}

pub fn redirect_status(event: &Event) -> RedirectStatus {
    RedirectStatus::try_from(status_code(event)).unwrap_or_default()
}

pub fn state_update<T, V>(state: &UseStateHandle<T>, update: fn(&mut T, V)) -> Callback<V>
where
    T: Clone + 'static,
    V: 'static,
{
    let state = state.clone();
    Callback::from(move |value: V| {
        let mut next = (*state).clone();
        update(&mut next, value);
        state.set(next);
    })
}

pub fn state_input<T, V>(
    state: &UseStateHandle<T>,
    read: fn(&Event) -> V,
    update: fn(&mut T, V),
) -> Callback<Event>
where
    T: Clone + 'static,
    V: 'static,
{
    state_update(state, update).reform(move |event: Event| read(&event))
}

/// Returns a handler that updates the item at the index of a list state.
pub fn item_update<T, V>(
    items: &UseStateHandle<Vec<T>>,
    index: usize,
    update: fn(&mut T, V),
) -> Callback<V>
where
    T: Clone + 'static,
    V: 'static,
{
    let items = items.clone();
    Callback::from(move |value: V| {
        let mut list = (*items).clone();
        if let Some(item) = list.get_mut(index) {
            update(item, value);
            items.set(list);
        }
    })
}

/// The buttons that add a new item after the item at the index and remove the item. The last
/// item cannot be removed.
pub fn list_buttons<T: Clone + 'static>(
    items: &UseStateHandle<Vec<T>>,
    index: usize,
    new_item: fn() -> T,
) -> Html {
    let add_onclick = {
        let items = items.clone();
        Callback::from(move |_: MouseEvent| {
            let mut list = (*items).clone();
            list.insert(index + 1, new_item());
            items.set(list);
        })
    };

    let remove_onclick = {
        let items = items.clone();
        Callback::from(move |_: MouseEvent| {
            if items.len() > 1 {
                let mut list = (*items).clone();
                list.remove(index);
                items.set(list);
            }
        })
    };

    html! {
        <div class="flex justify-end gap-2 mt-4 sm:ml-auto" role="group">
            <crate::components::button::Button kind={crate::components::button::ButtonKind::Secondary} size={crate::components::button::ButtonSize::Sm} onclick={add_onclick}>
                <img src="/assets/icons/add.svg" class="w-4 h-4 dark:invert" />
            </crate::components::button::Button>
            <crate::components::button::Button kind={crate::components::button::ButtonKind::Secondary} size={crate::components::button::ButtonSize::Sm} onclick={remove_onclick} disabled={items.len() <= 1}>
                <img src="/assets/icons/remove.svg" class="w-4 h-4 dark:invert" />
            </crate::components::button::Button>
        </div>
    }
}

/// Returns the parsed value, or records the error under `key` and returns the default value.
pub fn or_error<T: Default>(
    result: Result<T, String>,
    key: &str,
    errors: &mut HashMap<String, String>,
) -> T {
    result.unwrap_or_else(|err| {
        errors.insert(key.to_string(), err);
        T::default()
    })
}

/// Replaces an API error with its message in the selected language.
pub fn translated<T>(locale: Locale, result: Result<T, Error>) -> Result<T, String> {
    result.map_err(|err| locale.error_message(&err))
}

pub fn parse_size(locale: Locale, value: &str, name_key: &str) -> Result<u64, String> {
    let value = value.trim();
    if value.is_empty() {
        return Ok(0);
    }
    value
        .parse()
        .map_err(|_| locale.tf("http_form.whole_number", &[("name", locale.t(name_key))]))
}

pub fn parse_count(locale: Locale, value: &str, name_key: &str) -> Result<u32, String> {
    let value = value.trim();
    if value.is_empty() {
        return Ok(0);
    }
    value.parse().map_err(|_| {
        locale.tf(
            "http_form.count_range",
            &[("name", locale.t(name_key)), ("max", &u32::MAX.to_string())],
        )
    })
}

pub fn parse_percent(locale: Locale, value: &str, name_key: &str) -> Result<u8, String> {
    value
        .trim()
        .parse::<u8>()
        .ok()
        .filter(|percent| (1..=100).contains(percent))
        .ok_or_else(|| locale.tf("proxy_form.percent_range", &[("name", locale.t(name_key))]))
}

/// Parses one item per line and skips empty lines and lines that start with `#`. The translated
/// error message names the line.
pub fn parse_lines<T>(
    locale: Locale,
    text: &str,
    error_key: &str,
    parse: fn(&str) -> Result<T, Error>,
) -> Result<Vec<T>, String> {
    text.lines()
        .enumerate()
        .filter(|(_, line)| {
            let line = line.trim();
            !line.is_empty() && !line.starts_with('#')
        })
        .map(|(index, line)| {
            parse(line).map_err(|err| {
                locale.tf(
                    error_key,
                    &[
                        ("line", &(index + 1).to_string()),
                        ("error", &locale.error_message(&err)),
                    ],
                )
            })
        })
        .collect()
}

/// Parses a comma-separated list and skips empty items. Records the translated error of an invalid item under `key`.
pub fn parse_comma_list<T>(
    locale: Locale,
    text: &str,
    key: &str,
    errors: &mut HashMap<String, String>,
    parse: impl Fn(&str) -> Result<T, Error>,
) -> Vec<T> {
    let mut items = Vec::new();
    for item in text.split(',').map(str::trim).filter(|s| !s.is_empty()) {
        match parse(item) {
            Ok(item) => items.push(item),
            Err(err) => {
                errors.insert(key.to_string(), locale.error_message(&err));
            }
        }
    }
    items
}

/// Loads the list at `path` (relative to the API endpoint) once and keeps the items that `keep`
/// accepts. A failed load keeps the list empty and shows its error in a dialog.
#[hook]
pub fn use_loaded_list<T>(path: &'static str, keep: fn(&T) -> bool) -> UseStateHandle<Vec<T>>
where
    T: serde::de::DeserializeOwned + 'static,
{
    let locale = crate::i18n::use_locale();
    let items = use_state(Vec::<T>::new);
    use_effect_with((), {
        let items = items.clone();
        move |_| {
            wasm_bindgen_futures::spawn_local(async move {
                let request =
                    gloo_net::http::Request::get(&format!("{}{path}", crate::API_ENDPOINT));
                match crate::pages::settings::fetch_json::<Vec<T>>(locale, request).await {
                    Ok(list) => items.set(list.into_iter().filter(keep).collect()),
                    Err(err) => {
                        crate::dialog::message(locale, &err, crate::dialog::Icon::Error).await
                    }
                }
            });
        }
    });
    items
}
