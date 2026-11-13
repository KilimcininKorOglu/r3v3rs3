//! The parts of the pages that list resources and add or change one with a form.

use super::settings::{failure_box, fetch_json, format_network_error, send_request, success_box};
use crate::API_ENDPOINT;
use crate::components::action_button::{ActionIconButton, ActionVariant};
use crate::components::button::{Button, ButtonKind};
use crate::components::modal::{Modal, ModalSize};
use crate::dialog::{self, Icon};
use gloo_net::http::Request;
use r3v3rs3_api::i18n::Locale;
use serde::de::DeserializeOwned;
use std::future::Future;
use wasm_bindgen_futures::spawn_local;
use web_sys::HtmlInputElement;
use yew::prelude::*;

pub(super) const BUTTON_CLASS: &str = "inline-flex justify-center items-center text-neutral-500 bg-neutral-50 dark:text-neutral-200 dark:bg-neutral-800 border border-neutral-300 dark:border-neutral-600 focus:outline-none hover:bg-neutral-100 hover:dark:bg-neutral-900 focus:ring-4 focus:ring-neutral-200 dark:focus:ring-neutral-600 font-medium rounded-lg text-sm px-4 py-2";

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
            { form_buttons(locale, card.error, card.cancel, card.save_label, card.can_save) }
        </form>
    }
}

/// The add or edit form of a list page in a modal. The cancel button, Escape and a click on the
/// backdrop call `on_close`. The notice of the card shows at the top of the modal, so a failed
/// save is visible while the modal stays open.
pub fn form_modal(
    locale: Locale,
    card: FormCard,
    onsubmit: Callback<SubmitEvent>,
    on_close: Callback<()>,
) -> Html {
    let cancel = {
        let on_close = on_close.clone();
        Callback::from(move |event: MouseEvent| {
            event.prevent_default();
            on_close.emit(());
        })
    };
    html! {
        <Modal title={card.title} on_close={on_close} size={ModalSize::Lg}>
            <form {onsubmit}>
                { card.notice }
                { card.fields }
                { form_buttons(locale, card.error, Some(cancel), card.save_label, card.can_save) }
            </form>
        </Modal>
    }
}

fn form_buttons(
    locale: Locale,
    error: Option<String>,
    cancel: Option<Callback<MouseEvent>>,
    save_label: &'static str,
    can_save: bool,
) -> Html {
    html! {
        <>
            if let Some(error) = error {
                <p class="mt-2 text-sm text-red-600 dark:text-red-500">{error}</p>
            }
            <div class="flex flex-col-reverse gap-2 mt-4 sm:flex-row sm:items-center sm:justify-end">
                if let Some(onclick) = cancel {
                    <Button kind={ButtonKind::Secondary} {onclick}>{locale.t("common.cancel")}</Button>
                }
                <Button kind={ButtonKind::Primary} r#type="submit" disabled={!can_save}>{locale.t(save_label)}</Button>
            </div>
        </>
    }
}

/// The add button of a list page, shown below the list on the right.
pub fn add_button(locale: Locale, onclick: Callback<MouseEvent>) -> Html {
    html! {
        <div class="flex items-center justify-end my-4">
            <Button kind={ButtonKind::Primary} {onclick}>
                <AddIcon />
                {locale.t("common.add")}
            </Button>
        </div>
    }
}

/// The plus icon of the add buttons.
#[function_component(AddIcon)]
pub fn add_icon() -> Html {
    html! {
        <svg class="w-4 h-4" fill="none" stroke="currentColor" viewBox="0 0 24 24" aria-hidden="true">
            <path stroke-linecap="round" stroke-linejoin="round" stroke-width="2" d="M12 5v14m-7-7h14" />
        </svg>
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

/// Shows only a successful save, above the list of a page whose form opens in a modal.
pub fn saved_view<T>(notice: &Option<Notice<T>>, saved: impl Fn(&T) -> Html) -> Html {
    match notice {
        Some(Notice::Saved(value)) => success_box(saved(value)),
        _ => html! {},
    }
}

/// Shows only a failed save, inside the modal of the form, which stays open.
pub fn failed_view<T>(notice: &Option<Notice<T>>) -> Html {
    match notice {
        Some(Notice::Failed(message)) => failure_box(message),
        _ => html! {},
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
            <ActionIconButton
                variant={ActionVariant::Edit}
                title={locale.t("common.edit").to_string()}
                onclick={edit}
            />
            if let Some(onclick) = delete {
                <ActionIconButton
                    variant={ActionVariant::Delete}
                    title={locale.t("common.delete").to_string()}
                    {onclick}
                />
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

/// A form that opens in a modal and whether its modal is open.
pub type FormState<'a, F> = (&'a UseStateHandle<F>, &'a UseStateHandle<bool>);

/// The state of a form that opens in a modal: the form and whether the modal is open.
#[hook]
pub fn use_form_modal<F: Default + 'static>() -> (UseStateHandle<F>, UseStateHandle<bool>) {
    (use_state(F::default), use_state(bool::default))
}

/// Returns a callback that fills the form with `value` and opens its modal.
pub fn open_on_click<F: Clone + 'static>(
    form: &UseStateHandle<F>,
    open: &UseStateHandle<bool>,
    value: F,
) -> Callback<MouseEvent> {
    let (form, open) = (form.clone(), open.clone());
    Callback::from(move |event: MouseEvent| {
        event.prevent_default();
        form.set(value.clone());
        open.set(true);
    })
}

/// Returns a callback that closes the modal of the form and empties the form.
pub fn close_callback<F: Default + 'static>(
    form: &UseStateHandle<F>,
    open: &UseStateHandle<bool>,
) -> Callback<()> {
    let (form, open) = (form.clone(), open.clone());
    Callback::from(move |()| {
        open.set(false);
        form.set(F::default());
    })
}

/// Returns a callback that runs `first`, then closes the modal of the form.
pub fn then_close(first: Callback<()>, open: &UseStateHandle<bool>) -> Callback<()> {
    let open = open.clone();
    Callback::from(move |()| {
        first.emit(());
        open.set(false);
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
        .map_err(|err| format_network_error(locale, &err))?;
    send_request(locale, request).await.map(|_| ())
}

/// Returns a callback that reads the value at `path` of the admin API into the state.
pub fn reload_callback<T: DeserializeOwned + 'static>(
    locale: Locale,
    state: &UseStateHandle<Option<T>>,
    path: &'static str,
) -> Callback<()> {
    let state = state.clone();
    Callback::from(move |()| load(locale, state.clone(), path))
}

/// Reads the value at `path` of the admin API into the state. A failed read keeps the state and
/// shows its message.
pub fn load<T: DeserializeOwned + 'static>(
    locale: Locale,
    state: UseStateHandle<Option<T>>,
    path: &'static str,
) {
    spawn_local(async move {
        let request = Request::get(&format!("{API_ENDPOINT}{path}"));
        match fetch_json(locale, request).await {
            Ok(value) => state.set(Some(value)),
            Err(message) => dialog::message(locale, &message, Icon::Error).await,
        }
    });
}
