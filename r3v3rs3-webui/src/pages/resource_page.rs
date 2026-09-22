//! The parts of the pages that list resources and add or change one with a form.

use super::settings::{failure_box, send_request, success_box};
use crate::API_ENDPOINT;
use crate::components::data_list::{DANGER_LINK_CLASS, LINK_CLASS};
use crate::dialog::{self, Icon};
use gloo_net::http::Request;
use r3v3rs3_api::i18n::Locale;
use serde::de::DeserializeOwned;
use std::future::Future;
use wasm_bindgen_futures::spawn_local;
use web_sys::HtmlInputElement;
use yew::prelude::*;

const BUTTON_CLASS: &str = "inline-flex justify-center items-center text-neutral-500 bg-neutral-50 dark:text-neutral-200 dark:bg-neutral-800 border border-neutral-300 dark:border-neutral-600 focus:outline-none hover:bg-neutral-100 hover:dark:bg-neutral-900 focus:ring-4 focus:ring-neutral-200 dark:focus:ring-neutral-600 font-medium rounded-lg text-sm px-4 py-2";

/// The result of the last save.
#[derive(Clone, PartialEq)]
pub enum Notice<T> {
    Saved(T),
    Failed(String),
}

/// The frame of the form that adds or changes a resource.
pub struct FormCard {
    pub title: String,
    pub notice: Html,
    pub fields: Html,
    /// The error of the fields, shown below them.
    pub error: Option<String>,
    /// Ends the change of an existing resource. A new resource has no cancel button.
    pub cancel: Option<Callback<MouseEvent>>,
    /// The translation key of the save button.
    pub save_label: &'static str,
    pub can_save: bool,
}

pub fn form_card(locale: Locale, card: FormCard, onsubmit: Callback<SubmitEvent>) -> Html {
    html! {
        <form {onsubmit} class="mt-4 bg-white dark:bg-neutral-800 shadow-sm p-5 border border-neutral-300 dark:border-neutral-700 rounded-md">
            { card.notice }
            <h2 class="text-lg font-semibold text-neutral-900 dark:text-neutral-200">{card.title}</h2>
            { card.fields }
            if let Some(error) = card.error {
                <p class="mt-2 text-sm text-red-600 dark:text-red-500">{error}</p>
            }
            <div class="flex flex-col-reverse gap-2 mt-4 sm:flex-row sm:items-center sm:justify-end">
                if let Some(onclick) = card.cancel {
                    <button type="button" {onclick} class={BUTTON_CLASS}>{locale.t("common.cancel")}</button>
                }
                <button type="submit" disabled={!card.can_save} class={BUTTON_CLASS}>{locale.t(card.save_label)}</button>
            </div>
        </form>
    }
}

/// Shows the result of the last save. `saved` renders the message of a successful save.
pub fn notice_view<T>(notice: &Option<Notice<T>>, saved: impl Fn(&T) -> Html) -> Html {
    match notice {
        Some(Notice::Saved(value)) => success_box(saved(value)),
        Some(Notice::Failed(message)) => failure_box(message),
        None => html! {},
    }
}

/// The edit link of a row, and its delete link when the row has one.
pub fn row_actions(
    locale: Locale,
    edit: Callback<MouseEvent>,
    delete: Option<Callback<MouseEvent>>,
) -> Html {
    html! {
        <>
            <a class={LINK_CLASS} onclick={edit}>{locale.t("common.edit")}</a>
            if let Some(onclick) = delete {
                <a class={DANGER_LINK_CLASS} {onclick}>{locale.t("common.delete")}</a>
            }
        </>
    }
}

/// Returns a callback that writes the value of a text input to a field of the form.
pub fn text_input<F: Clone + 'static>(
    form: &UseStateHandle<F>,
    select: fn(&mut F) -> &mut String,
) -> Callback<InputEvent> {
    let form = form.clone();
    Callback::from(move |event: InputEvent| {
        let input: HtmlInputElement = event.target_unchecked_into();
        let mut updated = (*form).clone();
        *select(&mut updated) = input.value();
        form.set(updated);
    })
}

/// Returns a callback that sets the state to `value`.
pub fn set_on_click<T: Clone + 'static>(
    state: &UseStateHandle<T>,
    value: T,
) -> Callback<MouseEvent> {
    let state = state.clone();
    Callback::from(move |event: MouseEvent| {
        event.prevent_default();
        state.set(value.clone());
    })
}

/// Returns a callback that sends the change that `parse` returns, and gives the result of `save`
/// to `done`. An invalid form sends nothing.
pub fn submit_callback<C, T, Fut>(
    parse: impl Fn() -> Option<C> + 'static,
    save: impl Fn(C) -> Fut + 'static,
    done: Callback<Result<T, String>>,
) -> Callback<SubmitEvent>
where
    C: 'static,
    T: 'static,
    Fut: Future<Output = Result<T, String>> + 'static,
{
    Callback::from(move |event: SubmitEvent| {
        event.prevent_default();
        let Some(change) = parse() else {
            return;
        };
        let (saving, done) = (save(change), done.clone());
        spawn_local(async move { done.emit(saving.await) });
    })
}

/// Returns a callback that takes the result of a save. A successful save empties the form and
/// calls `saved`.
pub fn save_result<F: Default + 'static, T: 'static>(
    form: &UseStateHandle<F>,
    notice: &UseStateHandle<Option<Notice<T>>>,
    saved: Callback<()>,
) -> Callback<Result<T, String>> {
    let (form, notice) = (form.clone(), notice.clone());
    Callback::from(move |result| match result {
        Ok(value) => {
            form.set(F::default());
            notice.set(Some(Notice::Saved(value)));
            saved.emit(());
        }
        Err(message) => notice.set(Some(Notice::Failed(message))),
    })
}

/// Returns a callback that asks `question`, deletes the resource at `path` of the admin API and
/// calls `deleted`. A failed deletion shows its message.
pub fn delete_on_click(
    locale: Locale,
    question: String,
    path: String,
    deleted: Callback<()>,
) -> Callback<MouseEvent> {
    Callback::from(move |event: MouseEvent| {
        event.prevent_default();
        let (path, deleted) = (path.clone(), deleted.clone());
        dialog::confirm_then(locale, question.clone(), async move {
            match delete(locale, &path).await {
                Ok(()) => deleted.emit(()),
                Err(message) => dialog::message(locale, &message, Icon::Error).await,
            }
        });
    })
}

async fn delete(locale: Locale, path: &str) -> Result<(), String> {
    let request = Request::delete(&format!("{API_ENDPOINT}{path}"))
        .build()
        .map_err(|err| err.to_string())?;
    send_request(locale, request).await.map(|_| ())
}

/// Returns a callback that reads the value at `path` of the admin API into the state.
pub fn reload_callback<T: DeserializeOwned + 'static>(
    state: &UseStateHandle<Option<T>>,
    path: &'static str,
) -> Callback<()> {
    let state = state.clone();
    Callback::from(move |()| load(state.clone(), path))
}

/// Reads the value at `path` of the admin API into the state. A failed read keeps the state.
pub fn load<T: DeserializeOwned + 'static>(state: UseStateHandle<Option<T>>, path: &'static str) {
    spawn_local(async move {
        if let Ok(value) = get_json(path).await {
            state.set(Some(value));
        }
    });
}

pub async fn get_json<T: DeserializeOwned>(path: &str) -> Result<T, gloo_net::Error> {
    Request::get(&format!("{API_ENDPOINT}{path}"))
        .send()
        .await?
        .json()
        .await
}
