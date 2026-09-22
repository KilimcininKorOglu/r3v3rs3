//! Confirmation and message dialogs. SweetAlert2 from `assets/vendor/sweetalert2` draws them
//! inside the page, so the WebUI never opens a native browser dialog.

use js_sys::{Object, Promise, Reflect};
use r3v3rs3_api::i18n::Locale;
use std::future::Future;
use wasm_bindgen::prelude::*;
use wasm_bindgen_futures::{JsFuture, spawn_local};

#[wasm_bindgen]
extern "C" {
    #[wasm_bindgen(js_namespace = Swal, js_name = fire, catch)]
    fn swal_fire(options: &Object) -> Result<Promise, JsValue>;
}

/// The icon of a message dialog.
#[derive(Clone, Copy)]
pub enum Icon {
    Success,
    Error,
}

impl Icon {
    fn name(self) -> &'static str {
        match self {
            Icon::Success => "success",
            Icon::Error => "error",
        }
    }
}

/// Asks the account to confirm `question` and runs `action` after a confirmation.
pub fn confirm_then(locale: Locale, question: String, action: impl Future<Output = ()> + 'static) {
    spawn_local(async move {
        if confirm(locale, &question).await {
            action.await;
        }
    });
}

/// Asks the account to confirm `question`. A dialog that fails counts as a refusal, and its
/// error goes to the console.
pub async fn confirm(locale: Locale, question: &str) -> bool {
    let entries = [
        ("text", JsValue::from_str(question)),
        ("icon", JsValue::from_str("warning")),
        ("showCancelButton", JsValue::TRUE),
        ("focusCancel", JsValue::TRUE),
        (
            "confirmButtonText",
            JsValue::from_str(locale.t("common.yes")),
        ),
        ("cancelButtonText", JsValue::from_str(locale.t("common.no"))),
    ];
    let confirmed = show(&entries)
        .await
        .and_then(|result| Reflect::get(&result, &JsValue::from_str("isConfirmed")))
        .and_then(|value| {
            value
                .as_bool()
                .ok_or_else(|| JsValue::from_str("the dialog result has no isConfirmed"))
        });
    match confirmed {
        Ok(confirmed) => confirmed,
        Err(err) => {
            report(&err);
            false
        }
    }
}

/// Shows `text` until the account closes the dialog.
pub async fn message(locale: Locale, text: &str, icon: Icon) {
    let entries = [
        ("text", JsValue::from_str(text)),
        ("icon", JsValue::from_str(icon.name())),
        (
            "confirmButtonText",
            JsValue::from_str(locale.t("common.ok")),
        ),
    ];
    if let Err(err) = show(&entries).await {
        report(&err);
    }
}

async fn show(entries: &[(&str, JsValue)]) -> Result<JsValue, JsValue> {
    let options = Object::new();
    for (key, value) in entries {
        Reflect::set(&options, &JsValue::from_str(key), value)?;
    }
    Reflect::set(&options, &JsValue::from_str("theme"), &theme().into())?;
    JsFuture::from(swal_fire(&options)?).await
}

/// The theme of the page, which `index.html` and `preferences.rs` set as the `dark` class.
fn theme() -> &'static str {
    let dark = web_sys::window()
        .and_then(|window| window.document())
        .and_then(|document| document.document_element())
        .is_some_and(|root| root.class_list().contains("dark"));
    if dark { "dark" } else { "light" }
}

fn report(err: &JsValue) {
    web_sys::console::error_2(&JsValue::from_str("dialog failed:"), err);
}
