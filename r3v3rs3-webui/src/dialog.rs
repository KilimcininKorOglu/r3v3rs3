//! Confirmation and message dialogs. SweetAlert2 from `assets/vendor/sweetalert2` draws them
//! inside the page, so the WebUI never opens a native browser dialog.

use crate::components::button::{ButtonKind, ButtonSize, button_class};
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
    let confirmed = show(&entries, ButtonKind::Danger)
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
    if let Err(err) = show(&entries, ButtonKind::Primary).await {
        report(&err);
    }
}

/// Shows `html` content in a dialog until the account closes it.
pub async fn html(locale: Locale, title: &str, html: &str) {
    let entries = [
        ("title", JsValue::from_str(title)),
        ("html", JsValue::from_str(html)),
        (
            "confirmButtonText",
            JsValue::from_str(locale.t("common.ok")),
        ),
    ];
    if let Err(err) = show(&entries, ButtonKind::Primary).await {
        report(&err);
    }
}

/// Asks the account for a text input.
pub async fn prompt(locale: Locale, text: &str, placeholder: &str) -> Option<String> {
    let entries = [
        ("title", JsValue::from_str(text)),
        ("input", JsValue::from_str("text")),
        ("inputPlaceholder", JsValue::from_str(placeholder)),
        ("showCancelButton", JsValue::TRUE),
        (
            "confirmButtonText",
            JsValue::from_str(locale.t("common.ok")),
        ),
        (
            "cancelButtonText",
            JsValue::from_str(locale.t("common.cancel")),
        ),
    ];
    let result = show(&entries, ButtonKind::Primary).await.ok()?;
    let confirmed = Reflect::get(&result, &JsValue::from_str("isConfirmed"))
        .ok()?
        .as_bool()?;
    if confirmed {
        Reflect::get(&result, &JsValue::from_str("value"))
            .ok()?
            .as_string()
    } else {
        None
    }
}

async fn show(entries: &[(&str, JsValue)], confirm_kind: ButtonKind) -> Result<JsValue, JsValue> {
    let options = Object::new();
    for (key, value) in entries {
        Reflect::set(&options, &JsValue::from_str(key), value)?;
    }
    Reflect::set(&options, &JsValue::from_str("theme"), &theme().into())?;
    Reflect::set(
        &options,
        &JsValue::from_str("buttonsStyling"),
        &JsValue::FALSE,
    )?;
    Reflect::set(
        &options,
        &JsValue::from_str("customClass"),
        &button_classes(confirm_kind)?.into(),
    )?;
    JsFuture::from(swal_fire(&options)?).await
}

/// The classes of the dialog buttons: the shared button colors, so a dialog matches the page.
fn button_classes(confirm_kind: ButtonKind) -> Result<Object, JsValue> {
    let classes = Object::new();
    let class_of = |kind| {
        let mut class = button_class(kind, ButtonSize::Md);
        class.push("mx-1");
        JsValue::from_str(&class.to_string())
    };
    Reflect::set(
        &classes,
        &JsValue::from_str("confirmButton"),
        &class_of(confirm_kind),
    )?;
    Reflect::set(
        &classes,
        &JsValue::from_str("cancelButton"),
        &class_of(ButtonKind::Secondary),
    )?;
    Ok(classes)
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
