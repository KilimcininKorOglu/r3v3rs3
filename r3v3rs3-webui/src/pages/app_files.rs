//! The files of an app: the volumes of its running container.

use super::settings::{failure_box, fetch_json, send_request};
use crate::API_ENDPOINT;
use crate::auth::use_ensure_auth;
use crate::components::action_button::{ActionIconButton, ActionVariant};
use crate::components::app_tabs::{AppTab, AppTabs};
use crate::components::button::{Button, ButtonKind, ButtonSize, button_class};
use crate::components::data_list::{Column, ListContent, ListOptions, Row, list_view};
use crate::components::modal::{Modal, ModalSize};
use crate::dialog;
use crate::i18n::use_locale;
use crate::store::SessionStore;
use gloo_net::http::Request;
use r3v3rs3_api::i18n::Locale;
use r3v3rs3_api::id::ShortId;
use r3v3rs3_api::platform::{AppEntry, FileEntry, VolumeBackupEntry};
use web_sys::HtmlInputElement;
use yew::prelude::*;
use yewdux::prelude::*;

/// The largest file that the editor opens as text.
const MAX_EDIT_SIZE: usize = 1024 * 1024;

const CARD: &str = "bg-white dark:bg-neutral-800 shadow-sm border border-neutral-300 dark:border-neutral-700 rounded-md p-4 sm:p-6";
const SELECT: &str = "rounded-lg border border-neutral-300 bg-neutral-50 text-neutral-900 dark:border-neutral-600 dark:bg-neutral-700 dark:text-white px-2.5 py-1.5 text-sm focus:ring-primary-500 focus:border-primary-500";

const FILE_COLUMNS: [Column; 2] = [
    Column {
        label: "apps.files_name",
        class: "font-mono break-all",
    },
    Column {
        label: "apps.files_size",
        class: "whitespace-nowrap",
    },
];

const BACKUP_COLUMNS: [Column; 2] = [
    Column {
        label: "apps.volume_backup_id",
        class: "font-mono text-xs break-all",
    },
    Column {
        label: "apps.volume_backup_size",
        class: "font-mono text-xs whitespace-nowrap",
    },
];

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

/// The state of the page and the values every part of it uses.
#[derive(Clone)]
struct Ctx {
    locale: Locale,
    app_id: ShortId,
    dir: UseStateHandle<String>,
    entries: UseStateHandle<Vec<FileEntry>>,
    backups: UseStateHandle<Vec<VolumeBackupEntry>>,
    notice: UseStateHandle<Option<String>>,
    editing: UseStateHandle<Option<Editing>>,
}

impl Ctx {
    /// Reads the entries of `path` and moves the list there.
    fn open_dir(&self, path: &str) {
        open_dir(
            self.locale,
            self.app_id,
            path,
            &self.dir,
            &self.entries,
            &self.notice,
        );
    }

    /// Reads the current directory again.
    fn refresh(&self) {
        self.open_dir(&self.dir.to_string());
    }
}

#[function_component(AppFiles)]
pub fn app_files(props: &Props) -> Html {
    use_ensure_auth();
    let locale = use_locale();
    let (session, _) = use_store::<SessionStore>();
    let app = use_state(|| Option::<AppEntry>::None);
    let failed = use_state(|| Option::<String>::None);
    let ctx = Ctx {
        locale,
        app_id: props.id,
        dir: use_state(String::new),
        entries: use_state(Vec::<FileEntry>::new),
        backups: use_state(Vec::<VolumeBackupEntry>::new),
        notice: use_state(|| Option::<String>::None),
        editing: use_state(|| Option::<Editing>::None),
    };

    let signed_in = session.info.is_some();
    {
        let (app, failed, app_id) = (app.clone(), failed.clone(), ctx.app_id);
        use_effect_with(signed_in, move |&signed_in| {
            if signed_in {
                load_app(locale, app_id, &app, &failed);
            }
        });
    }
    {
        let ctx = ctx.clone();
        use_effect_with((*app).clone(), move |app| {
            let Some(mount) = app.as_ref().and_then(|app| app.spec.volumes.first()) else {
                return;
            };
            if ctx.dir.is_empty() {
                ctx.open_dir(mount.target.as_str());
            }
        });
    }
    let vol_name = (*app)
        .as_ref()
        .map(|a| current_volume(a, &ctx.dir))
        .unwrap_or_default();
    {
        let ctx = ctx.clone();
        use_effect_with((signed_in, vol_name.clone()), move |(signed_in, vol)| {
            if *signed_in && !vol.is_empty() {
                load_volume_backups(&ctx, vol);
            }
        });
    }

    if !signed_in {
        return html!();
    }
    let Some(app_entry) = (*app).clone() else {
        return failed.as_deref().map(failure_box).unwrap_or_default();
    };
    render_files_page(&ctx, &app_entry, &vol_name)
}

fn render_files_page(ctx: &Ctx, app: &AppEntry, vol_name: &str) -> Html {
    let locale = ctx.locale;
    let writable = writable_at(app, &ctx.dir);
    html! {
        <>
            <AppTabs
                id={app.id}
                name={app.name.to_string()}
                active={AppTab::Files}
                domains={app.spec.domains.clone()}
                is_database={app.spec.database.is_some()}
            />
            if let Some(message) = ctx.notice.as_deref() {
                <div class="mb-4">{ failure_box(message) }</div>
            }
            if app.spec.volumes.is_empty() {
                <div class={CARD}>
                    <p class="text-sm text-neutral-500 dark:text-neutral-400">{locale.t("apps.files_no_volumes")}</p>
                </div>
            } else {
                { browser_view(ctx, app, vol_name, writable) }
                { volume_backups_view(ctx, vol_name, writable) }
            }
            { editor_view(ctx) }
        </>
    }
}

/// Reads the app of the page once the session is up.
fn load_app(
    locale: Locale,
    app_id: ShortId,
    app: &UseStateHandle<Option<AppEntry>>,
    failed: &UseStateHandle<Option<String>>,
) {
    let (app, failed) = (app.clone(), failed.clone());
    wasm_bindgen_futures::spawn_local(async move {
        match fetch_json::<AppEntry>(
            locale,
            Request::get(&format!("{API_ENDPOINT}/apps/{app_id}")),
        )
        .await
        {
            Ok(entry) => app.set(Some(entry)),
            Err(message) => failed.set(Some(message)),
        }
    });
}

/// The volume selector, the path bar, the toolbar and the list of the current directory.
fn browser_view(ctx: &Ctx, app: &AppEntry, vol_name: &str, writable: bool) -> Html {
    let rows = ctx
        .entries
        .iter()
        .map(|entry| entry_row(ctx, writable, entry))
        .collect::<Vec<_>>();
    let content = ListContent {
        loaded: true,
        empty: "apps.files_empty",
        columns: &FILE_COLUMNS,
        rows: &rows,
    };
    let options = ListOptions {
        embedded: true,
        ..Default::default()
    };
    html! {
        <div class={CARD}>
            { toolbar_view(ctx, app, vol_name, writable) }
            <div class="mt-4 rounded-md border border-neutral-300 dark:border-neutral-700">
                { list_view(ctx.locale, content, None, &options) }
            </div>
        </div>
    }
}

/// The volume selector, the current path and the buttons above the list.
fn toolbar_view(ctx: &Ctx, app: &AppEntry, vol_name: &str, writable: bool) -> Html {
    let locale = ctx.locale;
    let on_volume = {
        let ctx = ctx.clone();
        Callback::from(move |event: Event| {
            let select: web_sys::HtmlSelectElement = event.target_unchecked_into();
            ctx.open_dir(&select.value());
        })
    };
    let on_parent = {
        let ctx = ctx.clone();
        Callback::from(move |_: MouseEvent| ctx.open_dir(&parent_of(&ctx.dir)))
    };
    let uploaded = {
        let ctx = ctx.clone();
        Callback::from(move |event: Event| upload(&ctx, event))
    };
    let imported = {
        let (ctx, vol) = (ctx.clone(), vol_name.to_string());
        Callback::from(move |event: Event| import_archive(&ctx, &vol, event))
    };
    let has_volume = !vol_name.is_empty();
    html! {
        <div class="flex flex-wrap items-center gap-3">
            <select class={SELECT} onchange={on_volume}>
                { for app.spec.volumes.iter().map(|mount| {
                    let value = mount.target.as_str().to_string();
                    let selected = value == *ctx.dir;
                    html! { <option key={value.clone()} {value} {selected}>{mount.volume.to_string()}</option> }
                }) }
            </select>
            <span class="font-mono text-sm text-neutral-700 dark:text-neutral-300 break-all">{(*ctx.dir).clone()}</span>
            <Button kind={ButtonKind::Secondary} size={ButtonSize::Sm} onclick={on_parent}>
                {locale.t("apps.files_parent")}
            </Button>
            <div class="flex flex-wrap items-center gap-2 sm:ml-auto">
                if writable {
                    <label class={classes!(button_class(ButtonKind::Primary, ButtonSize::Sm))}>
                        {locale.t("common.upload")}
                        <input type="file" class="hidden" onchange={uploaded} />
                    </label>
                }
                if has_volume {
                    <a href={volume_archive_url(ctx.app_id, vol_name)} class={classes!(button_class(ButtonKind::Secondary, ButtonSize::Sm))}>
                        {locale.t("apps.volume_export_archive")}
                    </a>
                }
                if writable && has_volume {
                    <label class={classes!(button_class(ButtonKind::Secondary, ButtonSize::Sm))}>
                        {locale.t("apps.volume_import_archive")}
                        <input type="file" accept=".tar,.tar.gz,.tgz" class="hidden" onchange={imported} />
                    </label>
                }
            </div>
        </div>
    }
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

/// Reads the entries of `path` and moves the list there.
fn open_dir(
    locale: Locale,
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
            locale,
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

/// The row of one entry: its name, its size and its actions.
fn entry_row(ctx: &Ctx, writable: bool, entry: &FileEntry) -> Row {
    let locale = ctx.locale;
    let path = child_path(&ctx.dir, &entry.name);
    let (name, size) = if entry.dir {
        (
            format!("{}/", entry.name),
            locale.t("apps.files_dir").to_string(),
        )
    } else {
        (entry.name.clone(), format_bytes(entry.size))
    };
    let actions = if entry.dir {
        dir_actions(ctx, path.clone())
    } else {
        file_actions(ctx, writable, path.clone())
    };
    Row {
        key: path,
        cells: vec![html! { {name} }, html! { {size} }],
        actions,
    }
}

fn dir_actions(ctx: &Ctx, path: String) -> Html {
    let open = {
        let ctx = ctx.clone();
        Callback::from(move |_: MouseEvent| ctx.open_dir(&path))
    };
    html! {
        <ActionIconButton variant={ActionVariant::View} title={ctx.locale.t("common.view").to_string()} onclick={open} />
    }
}

fn file_actions(ctx: &Ctx, writable: bool, path: String) -> Html {
    let locale = ctx.locale;
    let edit = {
        let (ctx, path) = (ctx.clone(), path.clone());
        Callback::from(move |_: MouseEvent| open_editor(&ctx, &path))
    };
    let remove = {
        let (ctx, path) = (ctx.clone(), path.clone());
        Callback::from(move |_: MouseEvent| confirm_delete(&ctx, path.clone()))
    };
    html! {
        <>
            <ActionIconButton
                variant={ActionVariant::Download}
                title={locale.t("certs.download").to_string()}
                href={download_url(ctx.app_id, &path)}
            />
            if writable {
                <ActionIconButton variant={ActionVariant::Edit} title={locale.t("common.edit").to_string()} onclick={edit} />
                <ActionIconButton variant={ActionVariant::Delete} title={locale.t("common.delete").to_string()} onclick={remove} />
            }
        </>
    }
}

/// Downloads the file and opens the editor with its text.
fn open_editor(ctx: &Ctx, path: &str) {
    let (editing, notice, locale, app_id) = (
        ctx.editing.clone(),
        ctx.notice.clone(),
        ctx.locale,
        ctx.app_id,
    );
    let path = path.to_string();
    wasm_bindgen_futures::spawn_local(async move {
        match read_text(locale, app_id, &path).await {
            Ok(text) => editing.set(Some(Editing { path, text })),
            Err(message) => notice.set(Some(message)),
        }
    });
}

/// The file as text, or the reason why it does not open in the editor.
async fn read_text(locale: Locale, app_id: ShortId, path: &str) -> Result<String, String> {
    let request = Request::get(&download_url(app_id, path))
        .build()
        .map_err(|err| err.to_string())?;
    let response = send_request(locale, request).await?;
    let bytes = response.binary().await.map_err(|err| err.to_string())?;
    if bytes.len() > MAX_EDIT_SIZE {
        return Err(locale.t("apps.files_too_large").to_string());
    }
    String::from_utf8(bytes).map_err(|_| locale.t("apps.files_binary").to_string())
}

/// The editor of one file: its text in a textarea of a modal, saved back over the endpoint.
fn editor_view(ctx: &Ctx) -> Html {
    let Some(file) = (*ctx.editing).clone() else {
        return html!();
    };
    let locale = ctx.locale;
    let on_close = {
        let editing = ctx.editing.clone();
        Callback::from(move |()| editing.set(None))
    };
    let cancel = {
        let on_close = on_close.clone();
        Callback::from(move |_: MouseEvent| on_close.emit(()))
    };
    let on_input = {
        let editing = ctx.editing.clone();
        Callback::from(move |event: InputEvent| {
            let area: web_sys::HtmlTextAreaElement = event.target_unchecked_into();
            if let Some(file) = (*editing).clone() {
                editing.set(Some(Editing {
                    path: file.path,
                    text: area.value(),
                }));
            }
        })
    };
    let save = {
        let ctx = ctx.clone();
        Callback::from(move |_: MouseEvent| {
            let Some(file) = (*ctx.editing).clone() else {
                return;
            };
            ctx.editing.set(None);
            save_file(&ctx, &file);
        })
    };
    let footer = html! {
        <>
            <Button kind={ButtonKind::Secondary} onclick={cancel}>{locale.t("common.cancel")}</Button>
            <Button kind={ButtonKind::Primary} onclick={save}>{locale.t("apps.files_save")}</Button>
        </>
    };
    html! {
        <Modal title={file.path.clone()} {on_close} size={ModalSize::Xl} {footer}>
            <textarea
                class="w-full h-[50vh] font-mono text-sm rounded-lg border border-neutral-300 bg-neutral-50 text-neutral-900 dark:border-neutral-600 dark:bg-neutral-900 dark:text-neutral-100 p-2 focus:ring-primary-500 focus:border-primary-500"
                value={file.text}
                oninput={on_input}
            />
        </Modal>
    }
}

/// Writes the text of the editor back into the file and reads the directory again.
fn save_file(ctx: &Ctx, file: &Editing) {
    let request = match Request::put(&format!("{API_ENDPOINT}/apps/{}/files", ctx.app_id))
        .header("content-type", "application/octet-stream")
        .query([("path", file.path.as_str())])
        .body(js_sys::Uint8Array::from(file.text.as_bytes()))
    {
        Ok(request) => request,
        Err(err) => return ctx.notice.set(Some(err.to_string())),
    };
    let ctx = ctx.clone();
    wasm_bindgen_futures::spawn_local(async move {
        if let Err(message) = send_ok(ctx.locale, request).await {
            ctx.notice.set(Some(message));
            return;
        }
        ctx.refresh();
    });
}

/// Asks for a confirmation and deletes the file, then reads the directory again.
fn confirm_delete(ctx: &Ctx, path: String) {
    let locale = ctx.locale;
    let question = locale.tf("apps.files_delete_confirm", &[("path", path.as_str())]);
    let request = match Request::delete(&format!("{API_ENDPOINT}/apps/{}/files", ctx.app_id))
        .query([("path", path.as_str())])
        .build()
    {
        Ok(request) => request,
        Err(err) => return ctx.notice.set(Some(err.to_string())),
    };
    let ctx = ctx.clone();
    dialog::confirm_then(locale, question, async move {
        if let Err(message) = send_ok(locale, request).await {
            ctx.notice.set(Some(message));
            return;
        }
        ctx.refresh();
    });
}

/// Sends `request` and accepts only a 2xx response. The error is the message of the admin API in
/// the selected language.
async fn send_ok(locale: Locale, request: Request) -> Result<(), String> {
    send_request(locale, request).await.map(|_| ())
}

/// Uploads the file of the input into the current directory.
fn upload(ctx: &Ctx, event: Event) {
    let input: HtmlInputElement = event.target_unchecked_into();
    let Some(file) = input.files().and_then(|files| files.get(0)) else {
        return;
    };
    // An emptied input sends the change event again when the same file is chosen next time.
    input.set_value("");
    let target = child_path(&ctx.dir, &file.name());
    let ctx = ctx.clone();
    wasm_bindgen_futures::spawn_local(async move {
        let sent = async {
            let buffer = file
                .array_buffer()
                .await
                .map_err(|err| err.as_string().unwrap_or_default())?;
            let request = Request::put(&format!("{API_ENDPOINT}/apps/{}/files", ctx.app_id))
                .header("content-type", "application/octet-stream")
                .query([("path", target.as_str())])
                .body(js_sys::Uint8Array::new(&buffer))
                .map_err(|err| err.to_string())?;
            send_ok(ctx.locale, request).await
        };
        if let Err(message) = sent.await {
            ctx.notice.set(Some(message));
        }
        ctx.refresh();
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

fn volume_backups_view(ctx: &Ctx, vol_name: &str, writable: bool) -> Html {
    let locale = ctx.locale;
    let on_create = {
        let (ctx, vol_name) = (ctx.clone(), vol_name.to_string());
        Callback::from(move |_: MouseEvent| create_snapshot(&ctx, &vol_name))
    };
    let rows = ctx
        .backups
        .iter()
        .map(|backup| backup_row(ctx, vol_name, writable, backup))
        .collect::<Vec<_>>();
    let content = ListContent {
        loaded: true,
        empty: "apps.volume_no_backups",
        columns: &BACKUP_COLUMNS,
        rows: &rows,
    };
    let options = ListOptions {
        embedded: true,
        compact: true,
        ..Default::default()
    };
    html! {
        <div class={classes!(CARD, "mt-6")}>
            <div class="flex flex-wrap items-center justify-between gap-3 mb-4">
                <div>
                    <h2 class="text-base font-semibold text-neutral-900 dark:text-neutral-100">{locale.t("apps.volume_backups_title")}</h2>
                    <p class="text-xs text-neutral-500 dark:text-neutral-400 font-mono mt-0.5">{vol_name.to_string()}</p>
                </div>
                if writable && !vol_name.is_empty() {
                    <Button kind={ButtonKind::Success} size={ButtonSize::Sm} onclick={on_create}>
                        {locale.t("apps.volume_backup_create")}
                    </Button>
                }
            </div>
            <div class="rounded-md border border-neutral-300 dark:border-neutral-700">
                { list_view(locale, content, None, &options) }
            </div>
        </div>
    }
}

fn backup_row(ctx: &Ctx, volume: &str, writable: bool, backup: &VolumeBackupEntry) -> Row {
    let locale = ctx.locale;
    let backup_id = backup.id.clone();
    let on_restore = {
        let (ctx, volume, backup_id) = (ctx.clone(), volume.to_string(), backup_id.clone());
        Callback::from(move |_: MouseEvent| restore_snapshot(&ctx, &volume, &backup_id))
    };
    let on_delete = {
        let (ctx, volume, backup_id) = (ctx.clone(), volume.to_string(), backup_id.clone());
        Callback::from(move |_: MouseEvent| delete_snapshot(&ctx, &volume, &backup_id))
    };
    Row {
        key: backup_id.clone(),
        cells: vec![
            html! { {backup_id.clone()} },
            html! { {format_bytes(backup.size)} },
        ],
        actions: html! {
            <>
                <ActionIconButton
                    variant={ActionVariant::Download}
                    title={locale.t("apps.volume_backup_download").to_string()}
                    href={volume_backup_download_url(ctx.app_id, volume, &backup_id)}
                />
                if writable {
                    <ActionIconButton variant={ActionVariant::Restore} title={locale.t("apps.volume_backup_restore").to_string()} onclick={on_restore} />
                    <ActionIconButton variant={ActionVariant::Delete} title={locale.t("apps.volume_backup_delete").to_string()} onclick={on_delete} />
                }
            </>
        },
    }
}

pub(crate) use crate::format::format_bytes;

fn load_volume_backups(ctx: &Ctx, volume: &str) {
    let (backups, notice, locale, app_id) = (
        ctx.backups.clone(),
        ctx.notice.clone(),
        ctx.locale,
        ctx.app_id,
    );
    let volume = volume.to_string();
    wasm_bindgen_futures::spawn_local(async move {
        let request = Request::get(&format!(
            "{API_ENDPOINT}/apps/{app_id}/volumes/{volume}/backups"
        ));
        match fetch_json::<Vec<VolumeBackupEntry>>(locale, request).await {
            Ok(entries) => backups.set(entries),
            Err(message) => notice.set(Some(message)),
        }
    });
}

fn create_snapshot(ctx: &Ctx, volume: &str) {
    let (backups, notice, locale, app_id) = (
        ctx.backups.clone(),
        ctx.notice.clone(),
        ctx.locale,
        ctx.app_id,
    );
    let volume = volume.to_string();
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

fn restore_snapshot(ctx: &Ctx, volume: &str, backup_id: &str) {
    let locale = ctx.locale;
    let question = locale.tf(
        "apps.volume_backup_confirm_restore",
        &[("volume", volume), ("id", backup_id)],
    );
    let request = match Request::post(&format!(
        "{API_ENDPOINT}/apps/{}/volumes/{volume}/backups/{backup_id}/restore",
        ctx.app_id
    ))
    .build()
    {
        Ok(request) => request,
        Err(err) => return ctx.notice.set(Some(err.to_string())),
    };
    let ctx = ctx.clone();
    dialog::confirm_then(locale, question, async move {
        if let Err(msg) = send_ok(locale, request).await {
            ctx.notice.set(Some(msg));
            return;
        }
        ctx.refresh();
    });
}

fn delete_snapshot(ctx: &Ctx, volume: &str, backup_id: &str) {
    let locale = ctx.locale;
    let question = locale.tf("apps.volume_backup_confirm_delete", &[("id", backup_id)]);
    let request = match Request::delete(&format!(
        "{API_ENDPOINT}/apps/{}/volumes/{volume}/backups/{backup_id}",
        ctx.app_id
    ))
    .build()
    {
        Ok(request) => request,
        Err(err) => return ctx.notice.set(Some(err.to_string())),
    };
    let (backups, notice) = (ctx.backups.clone(), ctx.notice.clone());
    let backup_id = backup_id.to_string();
    dialog::confirm_then(locale, question, async move {
        if let Err(msg) = send_ok(locale, request).await {
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

fn import_archive(ctx: &Ctx, volume: &str, event: Event) {
    let input: HtmlInputElement = event.target_unchecked_into();
    let Some(file) = input.files().and_then(|files| files.get(0)) else {
        return;
    };
    input.set_value("");
    let volume = volume.to_string();
    let ctx = ctx.clone();
    wasm_bindgen_futures::spawn_local(async move {
        let sent = async {
            let buffer = file
                .array_buffer()
                .await
                .map_err(|err| err.as_string().unwrap_or_default())?;
            let request = Request::post(&format!(
                "{API_ENDPOINT}/apps/{}/volumes/{volume}/archive",
                ctx.app_id
            ))
            .header("content-type", "application/gzip")
            .body(js_sys::Uint8Array::new(&buffer))
            .map_err(|err| err.to_string())?;
            send_ok(ctx.locale, request).await
        };
        if let Err(message) = sent.await {
            ctx.notice.set(Some(message));
        }
        ctx.refresh();
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
