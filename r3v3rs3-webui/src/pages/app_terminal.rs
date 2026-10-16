//! The web terminal into the running container of an app.

use super::Route;
use super::settings::fetch_json;
use crate::API_ENDPOINT;
use crate::auth::use_ensure_auth;
use crate::i18n::use_locale;
use crate::store::SessionStore;
use gloo_net::http::Request;
use r3v3rs3_api::i18n::Locale;
use r3v3rs3_api::id::ShortId;
use r3v3rs3_api::platform::{AppEntry, LOCAL_TARGET};
use wasm_bindgen::JsCast;
use wasm_bindgen::prelude::*;
use yew::prelude::*;
use yew_router::prelude::*;
use yewdux::prelude::*;

#[derive(Properties, PartialEq)]
pub struct Props {
    pub id: ShortId,
}

#[function_component(AppTerminal)]
pub fn app_terminal(props: &Props) -> Html {
    use_ensure_auth();
    let locale = use_locale();
    let (session, _) = use_store::<SessionStore>();
    let app = use_state(|| Option::<AppEntry>::None);
    let failed = use_state(|| Option::<String>::None);

    let signed_in = session.info.is_some();
    let app_id = props.id;
    {
        let app = app.clone();
        let failed = failed.clone();
        use_effect_with(signed_in, move |&signed_in| {
            if signed_in {
                let (app, failed) = (app.clone(), failed.clone());
                wasm_bindgen_futures::spawn_local(async move {
                    match fetch_json::<AppEntry>(
                        Locale::En,
                        Request::get(&format!("{API_ENDPOINT}/apps/{app_id}")),
                    )
                    .await
                    {
                        Ok(entry) => app.set(Some(entry)),
                        Err(message) => failed.set(Some(message)),
                    }
                });
            }
        });
    }

    let container_ref = use_node_ref();
    {
        let container_ref = container_ref.clone();
        use_effect_with((*app).clone(), move |app| {
            if let Some(app) = app {
                start_terminal(
                    app_id,
                    &container_ref,
                    app.target.to_string() == LOCAL_TARGET,
                );
            }
        });
    }

    if session.info.is_none() {
        return html!();
    }
    let title = match (*app).as_ref() {
        Some(app) => locale.tf("apps.terminal_title", &[("name", app.name.as_str())]),
        None => locale.t("apps.terminal").to_string(),
    };
    html! {
        <>
            <div class="flex items-center justify-between mb-3">
                <h1 class="text-xl font-semibold">{title}</h1>
                <Link<Route> to={Route::AppView { id: props.id }} classes="text-sm text-neutral-400 hover:text-neutral-200">
                    {locale.t("common.back")}
                </Link<Route>>
            </div>
            { status_view(locale, &failed, &app) }
            <div ref={container_ref} class="w-full h-[70vh] rounded bg-black" />
        </>
    }
}

/// The failure of the load, or the notice that a remote target has no terminal.
fn status_view(locale: Locale, failed: &Option<String>, app: &Option<AppEntry>) -> Html {
    if let Some(message) = failed.clone() {
        return html! { <p class="text-red-500">{message}</p> };
    }
    if let Some(app) = app
        && app.target.to_string() != LOCAL_TARGET
    {
        return html! { <p class="text-neutral-400">{locale.t("apps.terminal_unsupported")}</p> };
    }
    html! {}
}

/// Opens the WebSocket and attaches xterm to the container element. The terminal lives until
/// the page closes; the server ends the exec session when the socket closes.
fn start_terminal(id: ShortId, container_ref: &NodeRef, local: bool) {
    if !local {
        return;
    }
    let Some(window) = web_sys::window() else {
        return;
    };
    let Some(term) = construct(&window, "Terminal") else {
        terminal_error("the xterm bundle is not loaded");
        return;
    };
    let Some(fit) = construct(&window, "FitAddon") else {
        terminal_error("the xterm fit addon is not loaded");
        return;
    };
    let Some(element) = container_ref.get() else {
        return;
    };
    call(&term, "open", &[element.into()]);
    call(&term, "loadAddon", &[(&fit).into()]);
    call(&fit, "fit", &[]);

    let protocol = if window.location().protocol().unwrap_or_default() == "https:" {
        "wss"
    } else {
        "ws"
    };
    let Ok(host) = window.location().host() else {
        return;
    };
    let address = format!("{protocol}://{host}{API_ENDPOINT}/apps/{id}/terminal");
    let Ok(socket) = web_sys::WebSocket::new(&address) else {
        terminal_error("the terminal websocket failed to open");
        return;
    };
    socket.set_binary_type(web_sys::BinaryType::Arraybuffer);

    // The output of the container is binary, so it is written as it arrives.
    let writer = term.clone();
    let on_message = Closure::<dyn FnMut(web_sys::MessageEvent)>::wrap(Box::new(
        move |event: web_sys::MessageEvent| {
            if let Ok(data) = event.data().dyn_into::<js_sys::ArrayBuffer>() {
                call(&writer, "write", &[js_sys::Uint8Array::new(&data).into()]);
            }
        },
    ));
    socket.set_onmessage(Some(on_message.as_ref().unchecked_ref()));
    on_message.forget();

    // The input of the user goes to the container as binary.
    let socket_input = socket.clone();
    let writer_fallback = term.clone();
    let on_data = Closure::<dyn FnMut(String)>::wrap(Box::new(move |input: String| {
        let bytes = input.as_bytes();
        if socket_input.send_with_u8_array(bytes).is_err() {
            call(
                &writer_fallback,
                "writeln",
                &[JsValue::from_str("connection lost")],
            );
        }
    }));
    call(
        &term,
        "onData",
        &[on_data.as_ref().unchecked_ref::<js_sys::Function>().into()],
    );
    on_data.forget();

    // A resize of the window fits the terminal and reports its new size.
    let term_resize = term.clone();
    let fit_resize = fit.clone();
    let socket_resize = socket.clone();
    let on_resize = Closure::<dyn FnMut()>::wrap(Box::new(move || {
        call(&fit_resize, "fit", &[]);
        if let Some(size) = size_json(&term_resize) {
            let _ = socket_resize.send_with_str(&size);
        }
    }));
    window
        .add_event_listener_with_callback("resize", on_resize.as_ref().unchecked_ref())
        .ok();
    on_resize.forget();
}

/// The resize message that the admin endpoint understands.
fn size_json(term: &JsValue) -> Option<String> {
    let cols = field(term, "cols")?;
    let rows = field(term, "rows")?;
    Some(format!("{{\"cols\":{cols},\"rows\":{rows}}}"))
}

fn field(term: &JsValue, name: &str) -> Option<u16> {
    let value = js_sys::Reflect::get(term, &JsValue::from_str(name)).ok()?;
    let size = value.as_f64()?;
    if (0.0..=65535.0).contains(&size) {
        Some(size as u16)
    } else {
        None
    }
}

/// The constructor of a class that a vendor script defined on the window.
fn construct(window: &web_sys::Window, name: &str) -> Option<JsValue> {
    let constructor = js_sys::Reflect::get(window, &JsValue::from_str(name)).ok()?;
    if constructor.is_undefined() {
        return None;
    }
    js_sys::Reflect::construct(
        &constructor.dyn_into::<js_sys::Function>().ok()?,
        &js_sys::Array::new(),
    )
    .ok()
}

fn call(target: &JsValue, method: &str, args: &[JsValue]) {
    let Ok(method) = js_sys::Reflect::get(target, &JsValue::from_str(method)) else {
        return;
    };
    let Ok(method) = method.dyn_into::<js_sys::Function>() else {
        return;
    };
    let _ = method.apply(target, &js_sys::Array::from_iter(args.iter()));
}

fn terminal_error(message: &str) {
    web_sys::console::error_1(&JsValue::from_str(message));
}
