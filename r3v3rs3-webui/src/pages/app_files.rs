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
use r3v3rs3_api::platform::{AppEntry, FileEntry, VolumeBackupEntry};
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
    let backups = use_state(Vec::<VolumeBackupEntry>::new);
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
    {
        let app_val = (*app).clone();
        let (dir, entries, notice) = (dir.clone(), entries.clone(), notice.clone());
        use_effect_with(app_val, move |app| {
            let Some(mount) = app.as_ref().and_then(|app| app.spec.volumes.first()) else {
                return;
            };
            if dir.is_empty() {
                open_dir(app_id, mount.target.as_str(), &dir, &entries, &notice);
            }
        });
    }
    let vol_name = (*app)
        .as_ref()
        .map(|a| current_volume(a, &dir))
        .unwrap_or_default();
    {
        let backups = backups.clone();
        let vol_name = vol_name.clone();
        use_effect_with((signed_in, vol_name), move |(signed_in, vol)| {
            if *signed_in && !vol.is_empty() {
                load_volume_backups(app_id, vol, &backups);
            }
        });
    }

    if !signed_in {
        return html!();
    }
    let Some(app_entry) = (*app).clone() else {
        return failed.as_deref().map(failed_view).unwrap_or_default();
    };
    render_files_page(
        locale, app_id, &app_entry, &vol_name, &dir, &entries, &backups, &notice, &editing,
    )
}

#[allow(clippy::too_many_arguments)]
fn render_files_page(
    locale: Locale,
    app_id: ShortId,
    app: &AppEntry,
    vol_name: &str,
    dir: &UseStateHandle<String>,
    entries: &UseStateHandle<Vec<FileEntry>>,
    backups: &UseStateHandle<Vec<VolumeBackupEntry>>,
    notice: &UseStateHandle<Option<String>>,
    editing: &UseStateHandle<Option<Editing>>,
) -> Html {
    let title = locale.tf("apps.files_title", &[("name", app.name.as_str())]);
    let writable = writable_at(app, dir);
    let actions = build_row_actions(app_id, locale, dir, entries, notice, editing);
    let uploaded = {
        let (dir, entries, notice) = (dir.clone(), entries.clone(), notice.clone());
        Callback::from(move |e: Event| upload(e, app_id, &dir, &entries, &notice))
    };
    let imported = {
        let (dir, entries, notice) = (dir.clone(), entries.clone(), notice.clone());
        let vol = vol_name.to_string();
        Callback::from(move |e: Event| import_archive(e, app_id, &vol, &dir, &entries, &notice))
    };
    html! {
        <>
            <div class="flex items-center justify-between mb-3">
                <h1 class="text-xl font-semibold">{title}</h1>
                <Link<Route> to={Route::AppView { id: app_id }} classes="text-sm text-neutral-400 hover:text-neutral-200">
                    {locale.t("common.back")}
                </Link<Route>>
            </div>
            { notice.as_deref().map(failed_view).unwrap_or_default() }
            if app.spec.volumes.is_empty() {
                <p class="text-neutral-400">{locale.t("apps.files_no_volumes")}</p>
            } else {
                { browser_view(locale, app_id, app, vol_name, writable, dir, entries, notice, uploaded, imported, &actions) }
                { volume_backups_view(locale, app_id, vol_name, writable, backups, notice, dir, entries) }
            }
            { editor_view(locale, app_id, editing, dir, entries, notice) }
        </>
    }
}

fn build_row_actions(
    app_id: ShortId,
    locale: Locale,
    dir: &UseStateHandle<String>,
    entries: &UseStateHandle<Vec<FileEntry>>,
    notice: &UseStateHandle<Option<String>>,
    editing: &UseStateHandle<Option<Editing>>,
) -> RowActions {
    let navigate = {
        let (dir, entries, notice) = (dir.clone(), entries.clone(), notice.clone());
        Callback::from(move |path: String| open_dir(app_id, &path, &dir, &entries, &notice))
    };
    let edit = {
        let (editing, notice) = (editing.clone(), notice.clone());
        Callback::from(move |path: String| open_editor(locale, app_id, &path, &editing, &notice))
    };
    let remove = {
        let (dir, entries, notice) = (dir.clone(), entries.clone(), notice.clone());
        Callback::from(move |path: String| {
            confirm_delete(locale, app_id, path, &dir, &entries, &notice)
        })
    };
    RowActions {
        navigate,
        edit,
        remove,
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
    vol_name: &str,
    writable: bool,
    dir: &UseStateHandle<String>,
    entries: &UseStateHandle<Vec<FileEntry>>,
    notice: &UseStateHandle<Option<String>>,
    uploaded: Callback<Event>,
    imported: Callback<Event>,
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
                if !vol_name.is_empty() {
                    <a
                        href={volume_archive_url(app_id, vol_name)}
                        class="text-sm px-3 py-1.5 rounded bg-neutral-800 text-neutral-200 border border-neutral-700 hover:bg-neutral-700"
                    >
                        {locale.t("apps.volume_export_archive")}
                    </a>
                }
                if writable && !vol_name.is_empty() {
                    <label class="text-sm px-3 py-1.5 rounded bg-neutral-800 text-neutral-200 border border-neutral-700 hover:bg-neutral-700 cursor-pointer">
                        {locale.t("apps.volume_import_archive")}
                        <input type="file" accept=".tar,.tar.gz,.tgz" class="hidden" onchange={imported} />
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

fn current_volume(app: &AppEntry, dir: &str) -> String {
    app.spec
        .volumes
        .iter()
        .find(|mount| dir.starts_with(mount.target.as_str()))
        .map(|mount| mount.volume.to_string())
        .or_else(|| {
            app.spec
                .volumes
                .first()
                .map(|mount| mount.volume.to_string())
        })
        .unwrap_or_default()
}

#[allow(clippy::too_many_arguments)]
fn volume_backups_view(
    locale: Locale,
    app_id: ShortId,
    vol_name: &str,
    writable: bool,
    backups: &UseStateHandle<Vec<VolumeBackupEntry>>,
    notice: &UseStateHandle<Option<String>>,
    dir: &UseStateHandle<String>,
    entries: &UseStateHandle<Vec<FileEntry>>,
) -> Html {
    let on_create = {
        let vol_name = vol_name.to_string();
        let backups = backups.clone();
        let notice = notice.clone();
        Callback::from(move |_| {
            create_snapshot(locale, app_id, &vol_name, &backups, &notice);
        })
    };
    html! {
        <div class="mt-8 border-t border-neutral-800 pt-6">
            <div class="flex items-center justify-between mb-4">
                <div>
                    <h2 class="text-base font-semibold">{locale.t("apps.volume_backups_title")}</h2>
                    <p class="text-xs text-neutral-400 font-mono mt-0.5">{vol_name.to_string()}</p>
                </div>
                if writable && !vol_name.is_empty() {
                    <button
                        type="button"
                        class="text-sm px-3 py-1.5 rounded bg-neutral-100 text-neutral-900 font-medium hover:bg-white"
                        onclick={on_create}
                    >
                        {locale.t("apps.volume_backup_create")}
                    </button>
                }
            </div>
            if backups.is_empty() {
                <p class="text-xs text-neutral-500">{locale.t("apps.volume_no_backups")}</p>
            } else {
                <table class="w-full text-sm">
                    <thead>
                        <tr class="text-left text-xs text-neutral-500 border-b border-neutral-800">
                            <th class="py-1">{"ID"}</th>
                            <th class="py-1 text-right">{"Size"}</th>
                            <th class="py-1 text-right">{"Actions"}</th>
                        </tr>
                    </thead>
                    <tbody>
                        { for backups.iter().cloned().map(|backup| {
                            backup_row(locale, app_id, vol_name, writable, backup, backups, notice, dir, entries)
                        }) }
                    </tbody>
                </table>
            }
        </div>
    }
}

#[allow(clippy::too_many_arguments)]
fn backup_row(
    locale: Locale,
    app_id: ShortId,
    volume: &str,
    writable: bool,
    backup: VolumeBackupEntry,
    backups: &UseStateHandle<Vec<VolumeBackupEntry>>,
    notice: &UseStateHandle<Option<String>>,
    dir: &UseStateHandle<String>,
    entries: &UseStateHandle<Vec<FileEntry>>,
) -> Html {
    let backup_id = backup.id.clone();
    let download_link = volume_backup_download_url(app_id, volume, &backup_id);
    let on_restore = {
        let volume = volume.to_string();
        let backup_id = backup_id.clone();
        let notice = notice.clone();
        let dir = dir.clone();
        let entries = entries.clone();
        Callback::from(move |_| {
            restore_snapshot(locale, app_id, &volume, &backup_id, &notice, &dir, &entries);
        })
    };
    let on_delete = {
        let volume = volume.to_string();
        let backup_id = backup_id.clone();
        let backups = backups.clone();
        let notice = notice.clone();
        Callback::from(move |_| {
            delete_snapshot(locale, app_id, &volume, &backup_id, &backups, &notice);
        })
    };
    html! {
        <tr class="border-b border-neutral-800">
            <td class="py-2 font-mono text-xs">{backup_id}</td>
            <td class="py-2 text-right text-xs text-neutral-400 font-mono">{format_bytes(backup.size)}</td>
            <td class="py-2 text-right space-x-3 text-xs">
                <a href={download_link} class="text-neutral-400 hover:text-neutral-200">
                    {locale.t("apps.volume_backup_download")}
                </a>
                if writable {
                    <button type="button" class="text-neutral-400 hover:text-neutral-200" onclick={on_restore}>
                        {locale.t("apps.volume_backup_restore")}
                    </button>
                    <button type="button" class="text-red-400 hover:text-red-300" onclick={on_delete}>
                        {locale.t("apps.volume_backup_delete")}
                    </button>
                }
            </td>
        </tr>
    }
}

pub(crate) use crate::format::format_bytes;

fn load_volume_backups(
    app_id: ShortId,
    volume: &str,
    backups: &UseStateHandle<Vec<VolumeBackupEntry>>,
) {
    let backups = backups.clone();
    let volume = volume.to_string();
    wasm_bindgen_futures::spawn_local(async move {
        let request = Request::get(&format!(
            "{API_ENDPOINT}/apps/{app_id}/volumes/{volume}/backups"
        ));
        if let Ok(entries) = fetch_json::<Vec<VolumeBackupEntry>>(Locale::En, request).await {
            backups.set(entries);
        }
    });
}

fn create_snapshot(
    locale: Locale,
    app_id: ShortId,
    volume: &str,
    backups: &UseStateHandle<Vec<VolumeBackupEntry>>,
    notice: &UseStateHandle<Option<String>>,
) {
    let volume = volume.to_string();
    let backups = backups.clone();
    let notice = notice.clone();
    wasm_bindgen_futures::spawn_local(async move {
        let request = Request::post(&format!(
            "{API_ENDPOINT}/apps/{app_id}/volumes/{volume}/backups"
        ));
        match fetch_json::<VolumeBackupEntry>(locale, request).await {
            Ok(entry) => {
                let mut list = (*backups).clone();
                list.insert(0, entry);
                backups.set(list);
            }
            Err(msg) => notice.set(Some(msg)),
        }
    });
}

fn restore_snapshot(
    locale: Locale,
    app_id: ShortId,
    volume: &str,
    backup_id: &str,
    notice: &UseStateHandle<Option<String>>,
    dir: &UseStateHandle<String>,
    entries: &UseStateHandle<Vec<FileEntry>>,
) {
    let question = locale.tf(
        "apps.volume_backup_confirm_restore",
        &[("volume", volume), ("id", backup_id)],
    );
    let Ok(request) = Request::post(&format!(
        "{API_ENDPOINT}/apps/{app_id}/volumes/{volume}/backups/{backup_id}/restore"
    ))
    .build() else {
        return;
    };
    let dir = dir.clone();
    let entries = entries.clone();
    let notice = notice.clone();
    let current = dir.to_string();
    dialog::confirm_then(locale, question, async move {
        if let Err(msg) = send_ok(request).await {
            notice.set(Some(msg));
            return;
        }
        open_dir(app_id, &current, &dir, &entries, &notice);
    });
}

fn delete_snapshot(
    locale: Locale,
    app_id: ShortId,
    volume: &str,
    backup_id: &str,
    backups: &UseStateHandle<Vec<VolumeBackupEntry>>,
    notice: &UseStateHandle<Option<String>>,
) {
    let question = locale.tf("apps.volume_backup_confirm_delete", &[("id", backup_id)]);
    let Ok(request) = Request::delete(&format!(
        "{API_ENDPOINT}/apps/{app_id}/volumes/{volume}/backups/{backup_id}"
    ))
    .build() else {
        return;
    };
    let backups = backups.clone();
    let notice = notice.clone();
    let backup_id = backup_id.to_string();
    dialog::confirm_then(locale, question, async move {
        if let Err(msg) = send_ok(request).await {
            notice.set(Some(msg));
            return;
        }
        let list: Vec<VolumeBackupEntry> = (*backups)
            .iter()
            .filter(|b| b.id != backup_id)
            .cloned()
            .collect();
        backups.set(list);
    });
}

fn import_archive(
    event: Event,
    app_id: ShortId,
    volume: &str,
    dir: &UseStateHandle<String>,
    entries: &UseStateHandle<Vec<FileEntry>>,
    notice: &UseStateHandle<Option<String>>,
) {
    let input: HtmlInputElement = event.target_unchecked_into();
    let Some(file) = input.files().and_then(|files| files.get(0)) else {
        return;
    };
    let volume = volume.to_string();
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
            let request = Request::post(&format!(
                "{API_ENDPOINT}/apps/{app_id}/volumes/{volume}/archive"
            ))
            .header("content-type", "application/gzip")
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

fn volume_archive_url(app_id: ShortId, volume: &str) -> String {
    format!("{API_ENDPOINT}/apps/{app_id}/volumes/{volume}/archive")
}

fn volume_backup_download_url(app_id: ShortId, volume: &str, backup_id: &str) -> String {
    format!("{API_ENDPOINT}/apps/{app_id}/volumes/{volume}/backups/{backup_id}/download")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn format_bytes_scales_units() {
        assert_eq!(format_bytes(500), "500 B");
        assert_eq!(format_bytes(2048), "2.0 KB");
        assert_eq!(format_bytes(5 * 1024 * 1024), "5.0 MB");
    }

    #[test]
    fn volume_urls_format_correctly() {
        let id: ShortId = "abcd".parse().unwrap();
        assert_eq!(
            volume_archive_url(id, "shop-data"),
            "/api/apps/abcd/volumes/shop-data/archive"
        );
        assert_eq!(
            volume_backup_download_url(id, "shop-data", "snap1"),
            "/api/apps/abcd/volumes/shop-data/backups/snap1/download"
        );
    }
}
