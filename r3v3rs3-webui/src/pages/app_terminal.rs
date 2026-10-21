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
use std::cell::RefCell;
use std::rc::Rc;
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
    let active_shell = use_state(initial_shell);
    let custom_input = use_state(String::new);
    let copied = use_state(|| false);
    let session_ref = use_mut_ref(|| Option::<TerminalSession>::None);

    let signed_in = session.info.is_some();
    let app_id = props.id;
    use_app_fetch(app_id, signed_in, &app, &failed, &active_shell);

    let container_ref = use_node_ref();
    use_session_lifecycle(app_id, &app, &active_shell, &container_ref, &session_ref);

    if session.info.is_none() {
        return html!();
    }
    let title = terminal_title(locale, (*app).as_ref());
    let on_custom_submit = custom_submit_action(&custom_input, &active_shell);
    let on_copy = copy_action(&session_ref, &copied);
    let on_clear = clear_action(&session_ref);

    html! {
        <>
            <div class="flex items-center justify-between mb-3">
                <h1 class="text-xl font-semibold">{title}</h1>
                <Link<Route> to={Route::AppView { id: props.id }} classes="text-sm text-neutral-400 hover:text-neutral-200">
                    {locale.t("common.back")}
                </Link<Route>>
            </div>
            { status_view(locale, &failed, &app) }
            <div class="flex flex-wrap items-center justify-between gap-2 p-2 mb-2 bg-neutral-900 border border-neutral-800 rounded">
                { shell_selector_bar(locale, &active_shell, &app, &custom_input, on_custom_submit) }
                { session_actions_bar(locale, *copied, on_copy, on_clear) }
            </div>
            <div ref={container_ref} class="w-full h-[70vh] rounded bg-black overflow-hidden" />
        </>
    }
}

fn initial_shell() -> String {
    let window = web_sys::window();
    let loc = window.and_then(|w| w.location().search().ok());
    let query = loc.and_then(|s| {
        let params = web_sys::UrlSearchParams::new_with_str(&s).ok()?;
        params.get("shell")
    });
    query.unwrap_or_else(|| "sh".to_string())
}

fn terminal_title(locale: Locale, app: Option<&AppEntry>) -> String {
    match app {
        Some(app) => locale.tf("apps.terminal_title", &[("name", app.name.as_str())]),
        None => locale.t("apps.terminal").to_string(),
    }
}

#[hook]
fn use_app_fetch(
    app_id: ShortId,
    signed_in: bool,
    app: &UseStateHandle<Option<AppEntry>>,
    failed: &UseStateHandle<Option<String>>,
    active_shell: &UseStateHandle<String>,
) {
    let app = app.clone();
    let failed = failed.clone();
    let active_shell = active_shell.clone();
    use_effect_with(signed_in, move |&signed_in| {
        if signed_in {
            let (app, failed, active_shell) = (app.clone(), failed.clone(), active_shell.clone());
            wasm_bindgen_futures::spawn_local(async move {
                match fetch_json::<AppEntry>(
                    Locale::En,
                    Request::get(&format!("{API_ENDPOINT}/apps/{app_id}")),
                )
                .await
                {
                    Ok(entry) => {
                        apply_db_shell_default(&entry, &active_shell);
                        app.set(Some(entry));
                    }
                    Err(message) => failed.set(Some(message)),
                }
            });
        }
    });
}

fn apply_db_shell_default(entry: &AppEntry, active_shell: &UseStateHandle<String>) {
    if **active_shell == "sh"
        && let Some(ref db) = entry.spec.database
    {
        active_shell.set(db_tool_name(db).to_string());
    }
}

#[hook]
fn use_session_lifecycle(
    app_id: ShortId,
    app: &UseStateHandle<Option<AppEntry>>,
    active_shell: &UseStateHandle<String>,
    container_ref: &NodeRef,
    session_ref: &Rc<RefCell<Option<TerminalSession>>>,
) {
    let container_ref = container_ref.clone();
    let session_ref = session_ref.clone();
    use_effect_with(
        ((**app).clone(), (**active_shell).clone()),
        move |(app, shell)| {
            if let Some(app) = app
                && app.target.to_string() == LOCAL_TARGET
            {
                reset_container_element(&container_ref);
                *session_ref.borrow_mut() = None;
                let new_session = start_terminal(app_id, &container_ref, shell);
                *session_ref.borrow_mut() = new_session;
            }
            let session_ref = session_ref.clone();
            let container_ref = container_ref.clone();
            move || {
                *session_ref.borrow_mut() = None;
                reset_container_element(&container_ref);
            }
        },
    );
}

fn reset_container_element(container_ref: &NodeRef) {
    if let Some(element) = container_ref.cast::<web_sys::Element>() {
        element.set_inner_html("");
    }
}

fn custom_submit_action(
    custom_input: &UseStateHandle<String>,
    active_shell: &UseStateHandle<String>,
) -> Callback<()> {
    let custom_input = custom_input.clone();
    let active_shell = active_shell.clone();
    Callback::from(move |()| {
        let cmd = custom_input.trim();
        if !cmd.is_empty() {
            active_shell.set(cmd.to_string());
        }
    })
}

fn copy_action(
    session_ref: &Rc<RefCell<Option<TerminalSession>>>,
    copied: &UseStateHandle<bool>,
) -> Callback<MouseEvent> {
    let session_ref = session_ref.clone();
    let copied = copied.clone();
    Callback::from(move |_| {
        if let Some(session) = session_ref.borrow().as_ref() {
            let text = session.copy_text();
            copy_to_clipboard(&text);
            copied.set(true);
            let copied = copied.clone();
            gloo_timers::callback::Timeout::new(2000, move || copied.set(false)).forget();
        }
    })
}

fn clear_action(session_ref: &Rc<RefCell<Option<TerminalSession>>>) -> Callback<MouseEvent> {
    let session_ref = session_ref.clone();
    Callback::from(move |_| {
        if let Some(session) = session_ref.borrow().as_ref() {
            session.clear();
        }
    })
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

fn shell_selector_bar(
    locale: Locale,
    active_shell: &UseStateHandle<String>,
    app: &Option<AppEntry>,
    custom_input: &UseStateHandle<String>,
    on_submit: Callback<()>,
) -> Html {
    let shells = ["sh", "bash", "zsh"];
    let db_tool = app
        .as_ref()
        .and_then(|a| a.spec.database.as_deref())
        .map(db_tool_name);

    html! {
        <div class="flex flex-wrap items-center gap-1.5">
            <span class="text-xs text-neutral-400 font-medium mr-1">{format!("{}:", locale.t("apps.terminal_shell"))}</span>
            { for shells.into_iter().map(|s| render_shell_button(s, active_shell)) }
            if let Some(tool) = db_tool {
                { render_db_button(tool, active_shell) }
            }
            { render_custom_input(locale, custom_input, on_submit) }
        </div>
    }
}

fn render_shell_button(name: &'static str, active: &UseStateHandle<String>) -> Html {
    let is_active = **active == name;
    let classes = if is_active {
        "px-2.5 py-1 text-xs font-semibold rounded bg-blue-600 text-white shadow-sm"
    } else {
        "px-2.5 py-1 text-xs font-medium rounded bg-neutral-800 text-neutral-300 hover:bg-neutral-700 hover:text-white transition-colors cursor-pointer"
    };
    let on_click = {
        let active = active.clone();
        Callback::from(move |_| active.set(name.to_string()))
    };
    html! {
        <button type="button" class={classes} onclick={on_click}>
            {name}
        </button>
    }
}

fn render_db_button(tool: &'static str, active: &UseStateHandle<String>) -> Html {
    let is_active = **active == tool;
    let classes = if is_active {
        "px-2.5 py-1 text-xs font-semibold rounded bg-amber-600 text-white shadow-sm"
    } else {
        "px-2.5 py-1 text-xs font-medium rounded bg-amber-950/60 text-amber-300 border border-amber-800/50 hover:bg-amber-900/60 transition-colors cursor-pointer"
    };
    let on_click = {
        let active = active.clone();
        Callback::from(move |_| active.set(tool.to_string()))
    };
    html! {
        <button type="button" class={classes} onclick={on_click} title="Database CLI">
            {format!("{tool} (DB)")}
        </button>
    }
}

fn render_custom_input(
    locale: Locale,
    custom_input: &UseStateHandle<String>,
    on_submit: Callback<()>,
) -> Html {
    let oninput = {
        let custom_input = custom_input.clone();
        Callback::from(move |e: InputEvent| {
            if let Some(input) = e.target_dyn_into::<web_sys::HtmlInputElement>() {
                custom_input.set(input.value());
            }
        })
    };
    let onkeydown = {
        let on_submit = on_submit.clone();
        Callback::from(move |e: KeyboardEvent| {
            if e.key() == "Enter" {
                e.prevent_default();
                on_submit.emit(());
            }
        })
    };
    let onclick = Callback::from(move |_| on_submit.emit(()));

    html! {
        <div class="flex items-center gap-1 ml-2">
            <input
                type="text"
                class="px-2 py-0.5 text-xs bg-neutral-950 border border-neutral-700 rounded text-neutral-200 placeholder-neutral-500 focus:outline-none focus:border-blue-500 w-28 sm:w-36"
                placeholder={locale.t("apps.terminal_custom")}
                value={(**custom_input).clone()}
                {oninput}
                {onkeydown}
            />
            <button
                type="button"
                class="px-2 py-1 text-xs font-medium rounded bg-neutral-800 text-neutral-300 hover:bg-neutral-700 hover:text-white transition-colors cursor-pointer"
                {onclick}
            >
                {locale.t("apps.terminal_run")}
            </button>
        </div>
    }
}

fn session_actions_bar(
    locale: Locale,
    copied: bool,
    on_copy: Callback<MouseEvent>,
    on_clear: Callback<MouseEvent>,
) -> Html {
    let copy_classes = if copied {
        "px-2.5 py-1 text-xs font-semibold rounded bg-green-600 text-white transition-colors"
    } else {
        "px-2.5 py-1 text-xs font-medium rounded bg-neutral-800 text-neutral-300 hover:bg-neutral-700 hover:text-white transition-colors cursor-pointer"
    };
    let copy_text = if copied {
        locale.t("apps.terminal_copied")
    } else {
        locale.t("apps.terminal_copy")
    };
    html! {
        <div class="flex items-center gap-1.5 ml-auto">
            <button
                type="button"
                class="px-2.5 py-1 text-xs font-medium rounded bg-neutral-800 text-neutral-300 hover:bg-neutral-700 hover:text-white transition-colors cursor-pointer"
                onclick={on_clear}
            >
                {locale.t("apps.terminal_clear")}
            </button>
            <button
                type="button"
                class={copy_classes}
                onclick={on_copy}
            >
                {copy_text}
            </button>
        </div>
    }
}

fn db_tool_name(engine: &str) -> &'static str {
    match engine {
        "postgres" => "psql",
        "mariadb" => "mariadb",
        "mysql" => "mysql",
        "redis" => "redis-cli",
        "mongodb" => "mongosh",
        _ => "cli",
    }
}

struct TerminalSession {
    socket: web_sys::WebSocket,
    term: JsValue,
    resize_cb: Closure<dyn FnMut()>,
}

impl Drop for TerminalSession {
    fn drop(&mut self) {
        let _ = self.socket.close();
        if let Some(window) = web_sys::window() {
            let _ = window.remove_event_listener_with_callback(
                "resize",
                self.resize_cb.as_ref().unchecked_ref(),
            );
        }
        call(&self.term, "dispose", &[]);
    }
}

impl TerminalSession {
    fn clear(&self) {
        call(&self.term, "clear", &[]);
    }

    fn copy_text(&self) -> String {
        call(&self.term, "selectAll", &[]);
        let text = js_sys::Reflect::get(&self.term, &JsValue::from_str("getSelection"))
            .ok()
            .and_then(|f| f.dyn_into::<js_sys::Function>().ok())
            .and_then(|f| f.call0(&self.term).ok())
            .and_then(|v| v.as_string())
            .unwrap_or_default();
        call(&self.term, "clearSelection", &[]);
        text
    }
}

fn copy_to_clipboard(text: &str) {
    let Some(window) = web_sys::window() else {
        return;
    };
    let Ok(nav) = js_sys::Reflect::get(&window, &JsValue::from_str("navigator")) else {
        return;
    };
    let Ok(cb) = js_sys::Reflect::get(&nav, &JsValue::from_str("clipboard")) else {
        return;
    };
    let Ok(write) = js_sys::Reflect::get(&cb, &JsValue::from_str("writeText")) else {
        return;
    };
    if let Ok(func) = write.dyn_into::<js_sys::Function>() {
        let _ = func.call1(&cb, &JsValue::from_str(text));
    }
}

fn build_socket_address(window: &web_sys::Window, id: ShortId, shell: &str) -> Option<String> {
    let protocol = if window.location().protocol().unwrap_or_default() == "https:" {
        "wss"
    } else {
        "ws"
    };
    let host = window.location().host().ok()?;
    let shell_query = if shell.is_empty() || shell == "sh" {
        String::new()
    } else {
        format!("?shell={}", js_sys::encode_uri_component(shell))
    };
    Some(format!(
        "{protocol}://{host}{API_ENDPOINT}/apps/{id}/terminal{shell_query}"
    ))
}

fn attach_terminal_io(socket: &web_sys::WebSocket, term: &JsValue) {
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
        term,
        "onData",
        &[on_data.as_ref().unchecked_ref::<js_sys::Function>().into()],
    );
    on_data.forget();
}

fn attach_resize_listener(
    window: &web_sys::Window,
    socket: &web_sys::WebSocket,
    term: &JsValue,
    fit: &JsValue,
) -> Closure<dyn FnMut()> {
    let term_resize = term.clone();
    let fit_resize = fit.clone();
    let socket_resize = socket.clone();
    let on_resize = Closure::<dyn FnMut()>::wrap(Box::new(move || {
        call(&fit_resize, "fit", &[]);
        if let Some(size) = size_json(&term_resize) {
            let _ = socket_resize.send_with_str(&size);
        }
    }));
    let _ = window.add_event_listener_with_callback("resize", on_resize.as_ref().unchecked_ref());
    on_resize
}

/// Opens the WebSocket and attaches xterm to the container element. The terminal lives until
/// the session drops; the server ends the exec session when the socket closes.
fn start_terminal(id: ShortId, container_ref: &NodeRef, shell: &str) -> Option<TerminalSession> {
    let window = web_sys::window()?;
    let term = construct(&window, "Terminal")?;
    let fit = construct(&window, "FitAddon")?;
    let element = container_ref.get()?;
    call(&term, "open", &[element.into()]);
    call(&term, "loadAddon", &[(&fit).into()]);
    call(&fit, "fit", &[]);

    let address = build_socket_address(&window, id, shell)?;
    let socket = web_sys::WebSocket::new(&address).ok()?;
    socket.set_binary_type(web_sys::BinaryType::Arraybuffer);

    attach_terminal_io(&socket, &term);
    let resize_cb = attach_resize_listener(&window, &socket, &term, &fit);

    Some(TerminalSession {
        socket,
        term,
        resize_cb,
    })
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
