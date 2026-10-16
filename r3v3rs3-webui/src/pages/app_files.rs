//! The files of an app: the volumes of its running container.

use super::Route;
use super::settings::fetch_json;
use crate::API_ENDPOINT;
use crate::auth::use_ensure_auth;
use crate::dialog;
use crate::i18n::use_locale;
use crate::store::SessionStore;
use gloo_net::http::Request;
use r3v3rs3_api::i18n::Locale;
use r3v3rs3_api::id::ShortId;
use r3v3rs3_api::platform::{AppEntry, FileEntry};
use web_sys::HtmlInputElement;
use yew::prelude::*;
use yew_router::prelude::*;
use yewdux::prelude::*;

/// The largest file that the editor opens as text.
const MAX_EDIT_SIZE: usize = 1024 * 1024;

/// The file that the editor holds.
#[derive(Clone)]
struct Editing {
    path: String,
    text: String,
}

#[derive(Properties, PartialEq)]
pub struct Props {
    pub id: ShortId,
}

#[function_component(AppFiles)]
pub fn app_files(props: &Props) -> Html {
    use_ensure_auth();
    let locale = use_locale();
    let (session, _) = use_store::<SessionStore>();
    let app = use_state(|| Option::<AppEntry>::None);
    let dir = use_state(String::new);
    let entries = use_state(Vec::<FileEntry>::new);
    let notice = use_state(|| Option::<String>::None);
    let editing = use_state(|| Option::<Editing>::None);
    let failed = use_state(|| Option::<String>::None);

    let signed_in = session.info.is_some();
    let app_id = props.id;
    {
        let (app, failed) = (app.clone(), failed.clone());
        use_effect_with(signed_in, move |&signed_in| {
            if signed_in {
                load_app(app_id, &app, &failed);
            }
        });
    }

    // The first volume of the app is the one that the page opens.
    {
        let app = (*app).clone();
        let (dir, entries, notice) = (dir.clone(), entries.clone(), notice.clone());
        use_effect_with(app, move |app| {
            let Some(mount) = app.as_ref().and_then(|app| app.spec.volumes.first()) else {
                return;
            };
            if dir.is_empty() {
                open_dir(app_id, mount.target.as_str(), &dir, &entries, &notice);
            }
        });
    }

    if session.info.is_none() {
        return html!();
    }
    let Some(app) = (*app).clone() else {
        return html! {
            { failed.as_deref().map(failed_view).unwrap_or_default() }
        };
    };
    let title = locale.tf("apps.files_title", &[("name", app.name.as_str())]);
    let writable = writable_at(&app, &dir);
    let navigate = {
        let app_id = props.id;
        let dir = dir.clone();
        let entries = entries.clone();
        let notice = notice.clone();
        Callback::from(move |path: String| open_dir(app_id, &path, &dir, &entries, &notice))
    };
    let edit = {
        let editing = editing.clone();
        let notice = notice.clone();
        Callback::from(move |path: String| open_editor(locale, app_id, &path, &editing, &notice))
    };
    let remove = {
        let dir = dir.clone();
        let entries = entries.clone();
        let notice = notice.clone();
        Callback::from(move |path: String| {
            confirm_delete(locale, app_id, path, &dir, &entries, &notice)
        })
    };
    let uploaded = {
        let app_id = props.id;
        let dir = dir.clone();
        let entries = entries.clone();
        let notice = notice.clone();
        Callback::from(move |event: Event| {
            upload(event, app_id, &dir, &entries, &notice);
        })
    };
    let actions = RowActions {
        navigate,
        edit,
        remove,
    };
    html! {
        <>
            <div class="flex items-center justify-between mb-3">
                <h1 class="text-xl font-semibold">{title}</h1>
                <Link<Route> to={Route::AppView { id: props.id }} classes="text-sm text-neutral-400 hover:text-neutral-200">
                    {locale.t("common.back")}
                </Link<Route>>
            </div>
            { notice.as_deref().map(failed_view).unwrap_or_default() }
            if app.spec.volumes.is_empty() {
                <p class="text-neutral-400">{locale.t("apps.files_no_volumes")}</p>
            } else {
                { browser_view(locale, app_id, &app, writable, &dir, &entries, &notice, uploaded, &actions) }
            }
            { editor_view(locale, app_id, &editing, &dir, &entries, &notice) }
        </>
    }
}

/// Reads the app of the page once the session is up.
fn load_app(
    app_id: ShortId,
    app: &UseStateHandle<Option<AppEntry>>,
    failed: &UseStateHandle<Option<String>>,
) {
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

/// The volume selector, the path bar and the table of the current directory.
#[allow(clippy::too_many_arguments)]
fn browser_view(
    locale: Locale,
    app_id: ShortId,
    app: &AppEntry,
    writable: bool,
    dir: &UseStateHandle<String>,
    entries: &UseStateHandle<Vec<FileEntry>>,
    notice: &UseStateHandle<Option<String>>,
    uploaded: Callback<Event>,
    actions: &RowActions,
) -> Html {
    html! {
        <>
            <div class="flex items-center gap-3 mb-3">
                <select
                    class="rounded bg-neutral-800 text-neutral-100 border border-neutral-700 px-2 py-1 text-sm"
                    onchange={{
                        let dir = dir.clone();
                        let entries = entries.clone();
                        let notice = notice.clone();
                        Callback::from(move |event: Event| {
                            let select: web_sys::HtmlSelectElement = event.target_unchecked_into();
                            open_dir(app_id, &select.value(), &dir, &entries, &notice);
                        })
                    }}
                >
                    { for app.spec.volumes.iter().map(|mount| {
                        let value = mount.target.as_str().to_string();
                        let selected = value == **dir;
                        html! { <option {value} {selected}>{mount.volume.to_string()}</option> }
                    }) }
                </select>
                <span class="font-mono text-sm text-neutral-300">{(**dir).clone()}</span>
                <button type="button" class="text-sm text-neutral-400 hover:text-neutral-200"
                    onclick={{
                        let dir = dir.clone();
                        let entries = entries.clone();
                        let notice = notice.clone();
                        Callback::from(move |_| {
                            open_dir(app_id, &parent_of(&dir), &dir, &entries, &notice);
                        })
                    }}
                >
                    {locale.t("apps.files_parent")}
                </button>
                if writable {
                    <label class="text-sm px-3 py-1.5 rounded bg-neutral-100 text-neutral-900 font-medium hover:bg-white cursor-pointer">
                        {locale.t("common.upload")}
                        <input type="file" class="hidden" onchange={uploaded} />
                    </label>
                }
            </div>
            <table class="w-full text-sm">
                <tbody>
                    { for entries.iter().cloned().map(|entry| {
                        let path = child_path(dir, &entry.name);
                        entry_row(locale, app_id, writable, path, entry, actions)
                    }) }
                </tbody>
            </table>
            if entries.is_empty() {
                <p class="text-neutral-400 mt-2">{locale.t("apps.files_empty")}</p>
            }
        </>
    }
}

/// The notice of a failed operation.
fn failed_view(message: &str) -> Html {
    html! { <p class="text-red-500">{message.to_string()}</p> }
}

/// Whether `dir` sits inside a volume that a write may change.
fn writable_at(app: &AppEntry, dir: &str) -> bool {
    app.spec
        .volumes
        .iter()
        .any(|mount| !mount.read_only && dir.starts_with(mount.target.as_str()))
}

/// The directory above `dir`.
fn parent_of(dir: &str) -> String {
    match dir.trim_end_matches('/').rsplit_once('/') {
        Some((parent, _)) if !parent.is_empty() => parent.to_string(),
        _ => "/".to_string(),
    }
}

/// The path of an entry inside `dir`.
fn child_path(dir: &str, name: &str) -> String {
    format!("{}/{name}", dir.trim_end_matches('/'))
}

/// Reads the entries of `path` and moves the table there.
fn open_dir(
    app_id: ShortId,
    path: &str,
    dir: &UseStateHandle<String>,
    entries: &UseStateHandle<Vec<FileEntry>>,
    notice: &UseStateHandle<Option<String>>,
) {
    let entries = entries.clone();
    let notice = notice.clone();
    dir.set(path.to_string());
    let path = path.to_string();
    wasm_bindgen_futures::spawn_local(async move {
        match fetch_json::<Vec<FileEntry>>(
            Locale::En,
            Request::get(&format!("{API_ENDPOINT}/apps/{app_id}/files"))
                .query([("path", path.as_str())]),
        )
        .await
        {
            Ok(list) => {
                entries.set(list);
                notice.set(None);
            }
            Err(message) => notice.set(Some(message)),
        }
    });
}

/// The actions of one row: open a directory, open the editor, delete a file.
struct RowActions {
    navigate: Callback<String>,
    edit: Callback<String>,
    remove: Callback<String>,
}

/// The row of one entry: its name, its size and its actions.
fn entry_row(
    locale: Locale,
    app_id: ShortId,
    writable: bool,
    path: String,
    entry: FileEntry,
    actions: &RowActions,
) -> Html {
    let (name, size) = if entry.dir {
        (
            format!("{}/", entry.name),
            locale.t("apps.files_dir").to_string(),
        )
    } else {
        (entry.name.clone(), entry.size.to_string())
    };
    let actions = if entry.dir {
        html! {
            <button type="button" class="text-neutral-400 hover:text-neutral-200" onclick={{
                let navigate = actions.navigate.clone();
                let path = path.clone();
                Callback::from(move |_| navigate.emit(path.clone()))
            }}>
                {locale.t("common.view")}
            </button>
        }
    } else {
        html! {
            <>
                <button type="button" class="text-neutral-400 hover:text-neutral-200 mr-3" onclick={{
                    let path = path.clone();
                    Callback::from(move |_| {
                        if let Some(window) = web_sys::window() {
                            let _ = window.location().assign(&download_url(app_id, &path));
                        }
                    })
                }}>
                    {locale.t("certs.download")}
                </button>
                if writable {
                    <button type="button" class="text-neutral-400 hover:text-neutral-200 mr-3" onclick={{
                        let edit = actions.edit.clone();
                        let path = path.clone();
                        Callback::from(move |_| edit.emit(path.clone()))
                    }}>
                        {locale.t("common.edit")}
                    </button>
                    <button type="button" class="text-red-400 hover:text-red-300" onclick={{
                        let remove = actions.remove.clone();
                        let path = path.clone();
                        Callback::from(move |_| remove.emit(path.clone()))
                    }}>
                        {locale.t("common.delete")}
                    </button>
                }
            </>
        }
    };
    html! {
        <tr class="border-b border-neutral-700">
            <td class="py-1.5 font-mono">{name}</td>
            <td class="py-1.5 text-neutral-500 text-right pr-6">{size}</td>
            <td class="py-1.5 text-right">{actions}</td>
        </tr>
    }
}

/// Downloads the file and opens the editor with its text.
fn open_editor(
    locale: Locale,
    app_id: ShortId,
    path: &str,
    editing: &UseStateHandle<Option<Editing>>,
    notice: &UseStateHandle<Option<String>>,
) {
    let editing = editing.clone();
    let notice = notice.clone();
    let path = path.to_string();
    wasm_bindgen_futures::spawn_local(async move {
        let loaded = read_text(locale, app_id, &path).await;
        match loaded {
            Ok(text) => editing.set(Some(Editing { path, text })),
            Err(message) => notice.set(Some(message)),
        }
    });
}

/// The file as text, or the reason why it does not open in the editor.
async fn read_text(locale: Locale, app_id: ShortId, path: &str) -> Result<String, String> {
    let response = Request::get(&download_url(app_id, path))
        .send()
        .await
        .map_err(|err| err.to_string())?;
    if !response.ok() {
        return Err(response.status_text());
    }
    let bytes = response.binary().await.map_err(|err| err.to_string())?;
    if bytes.len() > MAX_EDIT_SIZE {
        return Err(locale.t("apps.files_too_large").to_string());
    }
    String::from_utf8(bytes).map_err(|_| locale.t("apps.files_binary").to_string())
}

/// The editor of one file: its text in a textarea, saved back over the endpoint.
fn editor_view(
    locale: Locale,
    app_id: ShortId,
    editing: &UseStateHandle<Option<Editing>>,
    dir: &UseStateHandle<String>,
    entries: &UseStateHandle<Vec<FileEntry>>,
    notice: &UseStateHandle<Option<String>>,
) -> Html {
    let Some(file) = (**editing).clone() else {
        return html!();
    };
    let path = file.path.clone();
    html! {
        <div class="fixed inset-0 bg-black/60 flex items-center justify-center p-4">
            <div class="bg-neutral-800 rounded p-4 w-full max-w-3xl">
                <p class="font-mono text-sm mb-2">{path.clone()}</p>
                <textarea
                    class="w-full h-[50vh] font-mono text-sm rounded bg-neutral-900 text-neutral-100 p-2"
                    value={file.text.clone()}
                    oninput={{
                        let editing = editing.clone();
                        Callback::from(move |event: InputEvent| {
                            let area: web_sys::HtmlTextAreaElement = event.target_unchecked_into();
                            if let Some(file) = (*editing).clone() {
                                editing.set(Some(Editing { path: file.path, text: area.value() }));
                            }
                        })
                    }}
                />
                <div class="flex justify-end gap-3 mt-3">
                    <button type="button" class="px-3 py-1.5 rounded bg-neutral-700 text-neutral-100" onclick={{
                        let editing = editing.clone();
                        Callback::from(move |_| editing.set(None))
                    }}>
                        {locale.t("common.cancel")}
                    </button>
                    <button type="button" class="px-3 py-1.5 rounded bg-neutral-100 text-neutral-900 font-medium" onclick={{
                        let editing = editing.clone();
                        let dir = dir.clone();
                        let entries = entries.clone();
                        let notice = notice.clone();
                        Callback::from(move |_| {
                            let Some(file) = (*editing).clone() else {
                                return;
                            };
                            editing.set(None);
                            save_file(app_id, &file, &dir, &entries, &notice);
                        })
                    }}>
                        {locale.t("apps.files_save")}
                    </button>
                </div>
            </div>
        </div>
    }
}

/// Writes the text of the editor back into the file and reads the directory again.
fn save_file(
    app_id: ShortId,
    file: &Editing,
    dir: &UseStateHandle<String>,
    entries: &UseStateHandle<Vec<FileEntry>>,
    notice: &UseStateHandle<Option<String>>,
) {
    let request = match Request::put(&format!("{API_ENDPOINT}/apps/{app_id}/files"))
        .header("content-type", "application/octet-stream")
        .query([("path", file.path.as_str())])
        .body(js_sys::Uint8Array::from(file.text.as_bytes()))
    {
        Ok(request) => request,
        Err(err) => {
            notice.set(Some(err.to_string()));
            return;
        }
    };
    let current = dir.to_string();
    let dir = dir.clone();
    let entries = entries.clone();
    let notice = notice.clone();
    wasm_bindgen_futures::spawn_local(async move {
        if let Err(message) = send_ok(request).await {
            notice.set(Some(message));
            return;
        }
        open_dir(app_id, &current, &dir, &entries, &notice);
    });
}

/// Asks for a confirmation and deletes the file, then reads the directory again.
fn confirm_delete(
    locale: Locale,
    app_id: ShortId,
    path: String,
    dir: &UseStateHandle<String>,
    entries: &UseStateHandle<Vec<FileEntry>>,
    notice: &UseStateHandle<Option<String>>,
) {
    let question = locale.tf("apps.files_delete_confirm", &[("path", path.as_str())]);
    let request = match Request::delete(&format!("{API_ENDPOINT}/apps/{app_id}/files"))
        .query([("path", path.as_str())])
        .build()
    {
        Ok(request) => request,
        Err(err) => {
            notice.set(Some(err.to_string()));
            return;
        }
    };
    let after = {
        let dir = dir.clone();
        let entries = entries.clone();
        let notice = notice.clone();
        let current = dir.to_string();
        move || open_dir(app_id, &current, &dir, &entries, &notice)
    };
    let notice = notice.clone();
    dialog::confirm_then(locale, question, async move {
        if let Err(message) = send_ok(request).await {
            notice.set(Some(message));
            return;
        }
        after();
    });
}

/// Sends `request` and accepts only a 2xx response.
async fn send_ok(request: Request) -> Result<(), String> {
    let response = request.send().await.map_err(|err| err.to_string())?;
    if response.ok() {
        Ok(())
    } else {
        Err(response.status_text())
    }
}

/// Uploads the file of the input into the current directory.
fn upload(
    event: Event,
    app_id: ShortId,
    dir: &UseStateHandle<String>,
    entries: &UseStateHandle<Vec<FileEntry>>,
    notice: &UseStateHandle<Option<String>>,
) {
    let input: HtmlInputElement = event.target_unchecked_into();
    let Some(file) = input.files().and_then(|files| files.get(0)) else {
        return;
    };
    let name = file.name();
    let target = child_path(dir, &name);
    let entries = entries.clone();
    let notice = notice.clone();
    let current = dir.to_string();
    let dir = dir.clone();
    wasm_bindgen_futures::spawn_local(async move {
        let sent = async {
            let buffer = file
                .array_buffer()
                .await
                .map_err(|err| err.as_string().unwrap_or_default())?;
            let request = Request::put(&format!("{API_ENDPOINT}/apps/{app_id}/files"))
                .header("content-type", "application/octet-stream")
                .query([("path", target.as_str())])
                .body(js_sys::Uint8Array::new(&buffer))
                .map_err(|err| err.to_string())?;
            send_ok(request).await
        };
        if let Err(message) = sent.await {
            notice.set(Some(message));
        }
        open_dir(app_id, &current, &dir, &entries, &notice);
    });
}

/// The address that streams a file to the browser.
fn download_url(app_id: ShortId, path: &str) -> String {
    format!(
        "{API_ENDPOINT}/apps/{app_id}/files/download?path={}",
        js_sys::encode_uri_component(path)
    )
}
