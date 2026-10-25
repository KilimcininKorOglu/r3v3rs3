//! The databases: the apps with a database engine, created and deleted on this page.

use super::Route;
use super::resource_page::{Notice, delete_on_click, notice_view};
use super::settings::fetch_json;
use crate::API_ENDPOINT;
use crate::auth::use_ensure_auth;
use crate::i18n::use_locale;
use crate::store::SessionStore;
use gloo_net::http::Request;
use r3v3rs3_api::container::ResourceLimits;
use r3v3rs3_api::i18n::Locale;
use r3v3rs3_api::id::ShortId;
use r3v3rs3_api::platform::{
    AppEntry, DatabaseCreated, DatabaseReplicaRequest, DatabaseRequest, EngineInfo,
    S3BackupSnapshot,
};
use wasm_bindgen_futures::spawn_local;
use yew::prelude::*;
use yew_router::prelude::*;
use yewdux::prelude::*;

const INPUT_CLASS: &str =
    "w-full rounded bg-neutral-800 text-neutral-100 border border-neutral-700 px-2 py-1.5 text-sm";

/// Reads the databases, the engines and the apps, and the first failure goes to the notice.
fn load_all(
    locale: Locale,
    databases: &UseStateHandle<Vec<AppEntry>>,
    engines: &UseStateHandle<Vec<EngineInfo>>,
    apps: &UseStateHandle<Vec<AppEntry>>,
    notice: &UseStateHandle<Option<Notice<()>>>,
) {
    let (databases, engines, apps, notice) = (
        databases.clone(),
        engines.clone(),
        apps.clone(),
        notice.clone(),
    );
    spawn_local(async move {
        match fetch_json::<Vec<AppEntry>>(
            locale,
            Request::get(&format!("{API_ENDPOINT}/databases")),
        )
        .await
        {
            Ok(list) => databases.set(list),
            Err(message) => notice.set(Some(Notice::Failed(message))),
        }
        if let Ok(list) = fetch_json::<Vec<EngineInfo>>(
            locale,
            Request::get(&format!("{API_ENDPOINT}/databases/engines")),
        )
        .await
        {
            engines.set(list);
        }
        if let Ok(list) =
            fetch_json::<Vec<AppEntry>>(locale, Request::get(&format!("{API_ENDPOINT}/apps"))).await
        {
            apps.set(list);
        }
    });
}

/// Reads the databases and the apps again after a change.
fn reload_lists(
    locale: Locale,
    databases: &UseStateHandle<Vec<AppEntry>>,
    apps: &UseStateHandle<Vec<AppEntry>>,
    notice: &UseStateHandle<Option<Notice<()>>>,
) {
    let (databases, apps, notice) = (databases.clone(), apps.clone(), notice.clone());
    spawn_local(async move {
        match fetch_json::<Vec<AppEntry>>(
            locale,
            Request::get(&format!("{API_ENDPOINT}/databases")),
        )
        .await
        {
            Ok(list) => databases.set(list),
            Err(message) => notice.set(Some(Notice::Failed(message))),
        }
        if let Ok(list) =
            fetch_json::<Vec<AppEntry>>(locale, Request::get(&format!("{API_ENDPOINT}/apps"))).await
        {
            apps.set(list);
        }
    });
}

/// The engine info of a database app, from the catalog.
fn engine_of(engines: &[EngineInfo], key: &str) -> Option<EngineInfo> {
    engines.iter().find(|info| info.engine == key).cloned()
}

fn refresh_callback(
    locale: Locale,
    engines: &UseStateHandle<Vec<EngineInfo>>,
    notice: &UseStateHandle<Option<Notice<()>>>,
    refreshing: &UseStateHandle<bool>,
) -> Callback<MouseEvent> {
    let (engines, notice, refreshing) = (engines.clone(), notice.clone(), refreshing.clone());
    Callback::from(move |_| {
        if *refreshing {
            return;
        }
        refreshing.set(true);
        let (engines, notice, refreshing) = (engines.clone(), notice.clone(), refreshing.clone());
        spawn_local(async move {
            refreshing.set(false);
            match fetch_json::<Vec<EngineInfo>>(
                locale,
                Request::post(&format!("{API_ENDPOINT}/databases/engines/refresh")),
            )
            .await
            {
                Ok(list) => {
                    engines.set(list);
                    notice.set(Some(Notice::Saved(())));
                }
                Err(message) => notice.set(Some(Notice::Failed(message))),
            }
        });
    })
}

fn s3_load_callback(
    locale: Locale,
    s3_snapshots: &UseStateHandle<Vec<S3BackupSnapshot>>,
    s3_loading: &UseStateHandle<bool>,
    notice: &UseStateHandle<Option<Notice<()>>>,
) -> Callback<()> {
    let (s3_snapshots, s3_loading, notice) =
        (s3_snapshots.clone(), s3_loading.clone(), notice.clone());
    Callback::from(move |()| {
        let (s3_snapshots, s3_loading, notice) =
            (s3_snapshots.clone(), s3_loading.clone(), notice.clone());
        s3_loading.set(true);
        spawn_local(async move {
            s3_loading.set(false);
            match fetch_json::<Vec<S3BackupSnapshot>>(
                locale,
                Request::get(&format!("{API_ENDPOINT}/databases/s3/backups")),
            )
            .await
            {
                Ok(list) => s3_snapshots.set(list),
                Err(e) => notice.set(Some(Notice::Failed(e))),
            }
        });
    })
}

fn s3_toggle_callback(
    s3_open: &UseStateHandle<bool>,
    load_s3: Callback<()>,
) -> Callback<MouseEvent> {
    let s3_open = s3_open.clone();
    Callback::from(move |_| {
        let next = !*s3_open;
        s3_open.set(next);
        if next {
            load_s3.emit(());
        }
    })
}

fn db_header_bar(
    locale: Locale,
    can_edit: bool,
    refreshing: bool,
    toggle_s3: Callback<MouseEvent>,
    refresh_engines: Callback<MouseEvent>,
) -> Html {
    html! {
        <div class="flex items-center justify-between mb-3">
            <h1 class="text-xl font-semibold">{locale.t("nav.databases")}</h1>
            <div class="flex items-center space-x-2">
                <button type="button" class="px-3 py-1.5 rounded bg-blue-600 text-white hover:bg-blue-500 text-sm shrink-0" onclick={toggle_s3}>
                    {locale.t("databases.s3_browser")}
                </button>
                if can_edit {
                    <button type="button" class="px-3 py-1.5 rounded bg-neutral-700 text-neutral-100 hover:bg-neutral-600 disabled:opacity-50 text-sm shrink-0" disabled={refreshing} onclick={refresh_engines}>
                        {locale.t("databases.refresh")}
                    </button>
                }
            </div>
        </div>
    }
}

#[function_component(Databases)]
pub fn databases() -> Html {
    use_ensure_auth();
    let locale = use_locale();
    let (session, _) = use_store::<SessionStore>();
    let databases = use_state(Vec::<AppEntry>::new);
    let engines = use_state(Vec::<EngineInfo>::new);
    let notice = use_state(|| Option::<Notice<()>>::None);
    let created = use_state(|| Option::<DatabaseCreated>::None);
    let apps = use_state(Vec::<AppEntry>::new);
    let refreshing = use_state(|| false);
    let s3_open = use_state(|| false);
    let s3_snapshots = use_state(Vec::<S3BackupSnapshot>::new);
    let s3_filter = use_state(String::new);
    let s3_loading = use_state(|| false);

    let signed_in = session.info.is_some();
    let can_edit = session.can_edit();
    {
        let (databases, engines, notice, apps) = (
            databases.clone(),
            engines.clone(),
            notice.clone(),
            apps.clone(),
        );
        use_effect_with(signed_in, move |&signed_in| {
            if signed_in {
                load_all(locale, &databases, &engines, &apps, &notice);
            }
        });
    }

    if session.info.is_none() {
        return html!();
    }
    let reload = {
        let (databases, notice, apps) = (databases.clone(), notice.clone(), apps.clone());
        Callback::from(move |_: ()| {
            reload_lists(locale, &databases, &apps, &notice);
        })
    };
    let refresh_engines = refresh_callback(locale, &engines, &notice, &refreshing);
    let load_s3 = s3_load_callback(locale, &s3_snapshots, &s3_loading, &notice);
    let toggle_s3 = s3_toggle_callback(&s3_open, load_s3.clone());
    let header = db_header_bar(locale, can_edit, *refreshing, toggle_s3, refresh_engines);
    html! {
        <>
            { header }
            { notice_view(&notice, |_| html! { {locale.t("databases.refreshed")} }) }
            if *s3_open {
                { s3_browser_panel(S3BrowserContext {
                    locale,
                    snapshots: &s3_snapshots,
                    engines: &engines,
                    filter: &s3_filter,
                    loading: *s3_loading,
                    can_edit,
                    load: load_s3.clone(),
                    saved: reload.clone(),
                    notice: notice.clone(),
                    created: created.clone(),
                }) }
            }
            { (*created)
                .clone()
                .map(|value| created_view(locale, value, &engines, &created))
                .unwrap_or_default() }
            if can_edit {
                <AddForm
                    locale={locale}
                    engines={(*engines).clone()}
                    saved={reload.clone()}
                    created={created.clone()}
                />
            }
            { list_view(
                &databases,
                &apps,
                RowContext {
                    locale,
                    engines: &engines,
                    can_edit,
                    reload: reload.clone(),
                    created: &created,
                    notice: &notice,
                },
            ) }
        </>
    }
}

/// The port that the clients of the database reach on the server.
fn port_of(app: &AppEntry) -> u16 {
    app.spec.host_port.unwrap_or(app.spec.port)
}

/// The connection string of a database, without a password.
fn connection(app: &AppEntry, engines: &[EngineInfo]) -> String {
    let Some(engine) = app.spec.database.as_deref() else {
        return String::new();
    };
    let Some(info) = engine_of(engines, engine) else {
        return String::new();
    };
    let port = port_of(app);
    let host = format!("127.0.0.1:{port}");
    match (info.user.is_empty(), info.default_db.is_empty()) {
        (true, _) => format!("{}://{host}", info.url_scheme),
        (false, true) => format!("{}://{}:***@{host}", info.url_scheme, info.user),
        (false, false) => format!(
            "{}://{}:***@{host}/{}",
            info.url_scheme, info.user, info.default_db
        ),
    }
}

struct RowContext<'a> {
    locale: Locale,
    engines: &'a [EngineInfo],
    can_edit: bool,
    reload: Callback<()>,
    created: &'a UseStateHandle<Option<DatabaseCreated>>,
    notice: &'a UseStateHandle<Option<Notice<()>>>,
}

fn admin_ui_store_id(engine: &str) -> &'static str {
    match engine {
        "postgres" => "pgadmin",
        "mysql" | "mariadb" => "phpmyadmin",
        "redis" => "redis-commander",
        "mongodb" => "mongo-express",
        _ => "adminer",
    }
}

fn db_actions(ctx: &RowContext<'_>, app: &AppEntry) -> Html {
    let is_replica = app.spec.replica_of.is_some();
    let delete = ctx.can_edit.then(|| {
        let question = ctx
            .locale
            .tf("databases.confirm_delete", &[("name", app.name.as_str())]);
        delete_on_click(
            ctx.locale,
            question,
            format!("/databases/{}", app.id),
            ctx.reload.clone(),
        )
    });
    html! {
        <td class="py-1.5 text-right">
            <Link<Route> to={Route::DatabaseBackups { id: app.id }} classes="text-sm text-neutral-400 hover:text-neutral-200 mr-3">
                {ctx.locale.t("backups.nav")}
            </Link<Route>>
            { render_extra_actions(ctx, app, is_replica) }
            <Link<Route> to={Route::AppDashboard { id: app.id }} classes="text-sm text-neutral-400 hover:text-neutral-200 mr-3">
                {ctx.locale.t("common.view")}
            </Link<Route>>
            if let Some(delete) = delete {
                <a class="text-red-400 hover:text-red-300 cursor-pointer" onclick={delete}>
                    {ctx.locale.t("common.delete")}
                </a>
            }
        </td>
    }
}

fn render_extra_actions(ctx: &RowContext<'_>, app: &AppEntry, is_replica: bool) -> Html {
    let admin_ui_btn = app.spec.database.as_deref().map(|engine| {
        let store = admin_ui_store_id(engine).to_string();
        html! {
            <Link<Route>
                to={Route::NewStoreApp { store }}
                classes="text-sm text-purple-400 hover:text-purple-300 mr-3"
            >
                {ctx.locale.t("databases.admin_ui")}
            </Link<Route>>
        }
    });
    let terminal_btn = (app.target.to_string() == r3v3rs3_api::platform::LOCAL_TARGET).then(|| {
        html! {
            <Link<Route>
                to={Route::AppTerminal { id: app.id }}
                classes="text-sm text-amber-400 hover:text-amber-300 cursor-pointer mr-3"
            >
                {ctx.locale.t("databases.cli")}
            </Link<Route>>
        }
    });
    let clone_btn = ctx.can_edit.then(|| {
        clone_action(
            ctx.locale,
            app.id,
            app.name.as_str(),
            ctx.reload.clone(),
            ctx.created.clone(),
            ctx.notice.clone(),
        )
    });
    let replica_btn = (ctx.can_edit && !is_replica).then(|| {
        replica_action(
            ctx.locale,
            app.id,
            app.name.as_str(),
            ctx.reload.clone(),
            ctx.created.clone(),
            ctx.notice.clone(),
        )
    });
    html! {
        <>
            if let Some(cli) = terminal_btn {
                {cli}
            }
            if let Some(admin_ui) = admin_ui_btn {
                {admin_ui}
            }
            if let Some(replica) = replica_btn {
                <a class="text-sm text-green-400 hover:text-green-300 cursor-pointer mr-3" onclick={replica}>
                    {ctx.locale.t("databases.create_replica")}
                </a>
            }
            if let Some(clone) = clone_btn {
                <a class="text-sm text-blue-400 hover:text-blue-300 cursor-pointer mr-3" onclick={clone}>
                    {ctx.locale.t("databases.clone")}
                </a>
            }
        </>
    }
}

fn db_row(ctx: &RowContext<'_>, app: &AppEntry, users: &str) -> Html {
    let is_replica = app.spec.replica_of.is_some();
    let engine = app
        .spec
        .database
        .as_deref()
        .and_then(|key| engine_of(ctx.engines, key))
        .map(|info| info.label.clone())
        .unwrap_or_else(|| app.spec.database.clone().unwrap_or_default());
    html! {
        <tr class="border-b border-neutral-700">
            <td class="py-1.5 font-medium">
                {app.name.to_string()}
                if is_replica {
                    <span class="ml-2 px-1.5 py-0.5 text-xs rounded bg-neutral-700 text-neutral-300">
                        {ctx.locale.t("databases.read_replica")}
                    </span>
                }
                if app.spec.project_id.is_some() {
                    <span class="ml-2 px-1.5 py-0.5 text-xs rounded bg-emerald-900/40 text-emerald-300 border border-emerald-700">
                        {ctx.locale.t("projects.project")}
                    </span>
                }
            </td>
            <td class="py-1.5 text-neutral-400">{engine}</td>
            <td class="py-1.5 font-mono text-neutral-400">{port_of(app).to_string()}</td>
            <td class="py-1.5 font-mono text-neutral-400">{connection(app, ctx.engines)}</td>
            <td class="py-1.5 text-neutral-400">{users}</td>
            { db_actions(ctx, app) }
        </tr>
    }
}

/// The table of the databases, with the number of the apps that use each one.
fn list_view(databases: &[AppEntry], apps: &[AppEntry], ctx: RowContext<'_>) -> Html {
    if databases.is_empty() {
        return html! { <p class="text-neutral-400 mt-2">{ctx.locale.t("databases.empty")}</p> };
    }
    let rows = databases
        .iter()
        .map(|app| {
            let users = apps
                .iter()
                .filter(|a| a.spec.uses_database == Some(app.id))
                .map(|a| a.name.to_string())
                .collect::<Vec<_>>()
                .join(", ");
            db_row(&ctx, app, &users)
        })
        .collect::<Vec<_>>();
    html! {
        <table class="w-full text-sm mt-3">
            <thead>
                <tr class="text-left text-neutral-400">
                    <th class="py-1.5 font-medium">{ctx.locale.t("common.name")}</th>
                    <th class="py-1.5 font-medium">{ctx.locale.t("databases.engine")}</th>
                    <th class="py-1.5 font-medium">{ctx.locale.t("common.port")}</th>
                    <th class="py-1.5 font-medium">{ctx.locale.t("databases.connection")}</th>
                    <th class="py-1.5 font-medium">{ctx.locale.t("databases.apps")}</th>
                    <th />
                </tr>
            </thead>
            <tbody>{ for rows }</tbody>
        </table>
    }
}

/// The panel that shows the password and the connection of a new database once.
fn created_view(
    locale: Locale,
    created: DatabaseCreated,
    engines: &[EngineInfo],
    clear: &UseStateHandle<Option<DatabaseCreated>>,
) -> Html {
    let info = created
        .app
        .spec
        .database
        .as_deref()
        .and_then(|key| engine_of(engines, key));
    let password = created.password.clone().unwrap_or_default();
    let url = match info {
        Some(info) => {
            let scheme = info.url_scheme.clone();
            let database = created.database.clone();
            match (info.user.is_empty(), database.is_empty()) {
                (true, true) => format!("{scheme}://127.0.0.1:{}", created.host_port),
                (false, true) => format!(
                    "{scheme}://{}:{password}@127.0.0.1:{}",
                    created.user, created.host_port
                ),
                (false, false) => format!(
                    "{scheme}://{}:{password}@127.0.0.1:{}/{database}",
                    created.user, created.host_port
                ),
                (true, false) => format!("{scheme}://127.0.0.1:{}/{database}", created.host_port),
            }
        }
        None => String::new(),
    };
    let close = {
        let clear = clear.clone();
        Callback::from(move |_| clear.set(None))
    };
    html! {
        <div class="rounded border border-neutral-600 p-4 mb-4">
            <p class="font-medium mb-2">{locale.t("databases.created")}</p>
            if !password.is_empty() {
                <p class="mb-2">
                    {locale.t("databases.password_once")}{" "}
                    <code class="font-mono break-all">{password}</code>
                </p>
            }
            <p class="mb-3">
                {locale.t("databases.connection")}{" "}
                <code class="font-mono break-all">{url}</code>
            </p>
            <button type="button" class="px-3 py-1.5 rounded bg-neutral-100 text-neutral-900 font-medium" onclick={close}>
                {locale.t("common.ok")}
            </button>
        </div>
    }
}

/// The form that installs a database of one engine and version.
#[derive(Properties, PartialEq)]
struct AddFormProps {
    locale: Locale,
    engines: Vec<EngineInfo>,
    saved: Callback<()>,
    created: UseStateHandle<Option<DatabaseCreated>>,
}

#[function_component(AddForm)]
fn add_form(props: &AddFormProps) -> Html {
    let locale = props.locale;
    let engines = props.engines.clone();
    let engine = use_state(|| Option::<String>::None);
    let selected = (*engine)
        .as_deref()
        .and_then(|key| engines.iter().find(|info| info.engine == key).cloned());
    let version = use_state(String::new);
    let name = use_state(String::new);
    let host_port = use_state(String::new);
    let memory_mb = use_state(String::new);
    let swap_mb = use_state(String::new);
    let cpus = use_state(String::new);
    let notice = use_state(|| Option::<String>::None);
    let busy = use_state(|| false);

    let pick_engine = {
        let engine = engine.clone();
        let version = version.clone();
        Callback::from(move |event: Event| {
            let select: web_sys::HtmlSelectElement = event.target_unchecked_into();
            engine.set(Some(select.value()));
            version.set(String::new());
        })
    };
    let submit = {
        let (name, version, host_port, engine, memory_mb, swap_mb, cpus, notice, busy) = (
            name.clone(),
            version.clone(),
            host_port.clone(),
            engine.clone(),
            memory_mb.clone(),
            swap_mb.clone(),
            cpus.clone(),
            notice.clone(),
            busy.clone(),
        );
        let saved = props.saved.clone();
        let created = props.created.clone();
        Callback::from(move |event: SubmitEvent| {
            event.prevent_default();
            let Some(engine) = (*engine).clone() else {
                notice.set(Some(locale.t("databases.engine_required").to_string()));
                return;
            };
            let Ok(parsed_name) = name.trim().parse() else {
                notice.set(Some(locale.t("databases.name_invalid").to_string()));
                return;
            };
            let limits = parse_db_limits(&memory_mb, &cpus, &swap_mb);
            let request = DatabaseRequest {
                name: parsed_name,
                engine,
                version: version.trim().to_string(),
                host_port: host_port.trim().parse().ok(),
                limits,
                project_id: None,
                environment_id: None,
            };
            let (notice, busy, saved, created) =
                (notice.clone(), busy.clone(), saved.clone(), created.clone());
            spawn_local(send_create(request, notice, busy, saved, created));
        })
    };
    let versions: &[String] = selected
        .as_ref()
        .map(|info| info.versions.as_slice())
        .unwrap_or(&[]);
    html! {
        <form onsubmit={submit} class="rounded border border-neutral-700 p-4 mb-4">
            <p class="font-medium mb-3">{locale.t("databases.add")}</p>
            if let Some(message) = notice.as_ref() {
                <p class="text-red-500 mb-2">{message.clone()}</p>
            }
            <div class="grid gap-3 md:grid-cols-4">
                <label class="block">
                    <span class="text-sm text-neutral-400">{locale.t("databases.engine")}</span>
                    <select class={INPUT_CLASS} onchange={pick_engine}>
                        <option value="">{locale.t("databases.pick_engine")}</option>
                        { for engines.iter().map(|info| {
                            let value = info.engine.clone();
                            let is_selected =
                                selected.as_ref().map(|s| s.engine.clone()) == Some(value.clone());
                            html! { <option {value} selected={is_selected}>{info.label.clone()}</option> }
                        }) }
                    </select>
                </label>
                <label class="block">
                    <span class="text-sm text-neutral-400">{locale.t("databases.version")}</span>
                    <select class={INPUT_CLASS}
                        value={(*version).clone()}
                        onchange={{
                            let version = version.clone();
                            Callback::from(move |event: Event| {
                                let select: web_sys::HtmlSelectElement = event.target_unchecked_into();
                                version.set(select.value());
                            })
                        }}
                    >
                        <option value="">{locale.t("databases.pick_version")}</option>
                        { for versions.iter().map(|tag| {
                            let value = tag.clone();
                            let is_selected = *version == value;
                            html! { <option {value} selected={is_selected}>{tag.clone()}</option> }
                        }) }
                    </select>
                </label>
                { form_input(locale.t("common.name").into(), "".into(), &name) }
                { form_input(locale.t("databases.host_port").into(), locale.t("databases.host_port_hint").into(), &host_port) }
            </div>
            <div class="grid gap-3 md:grid-cols-3 mt-3">
                { form_input(locale.t("databases.memory_mb").into(), "512".into(), &memory_mb) }
                { form_input(locale.t("databases.swap_mb").into(), "0".into(), &swap_mb) }
                { form_input(locale.t("databases.cpus").into(), "1.0".into(), &cpus) }
            </div>
            <div class="flex justify-end mt-3">
                <button type="submit" class="px-3 py-1.5 rounded bg-neutral-100 text-neutral-900 font-medium disabled:opacity-50" disabled={*busy}>
                    {locale.t("databases.create")}
                </button>
            </div>
        </form>
    }
}

fn form_input(label: AttrValue, placeholder: AttrValue, value: &UseStateHandle<String>) -> Html {
    let handle = value.clone();
    html! {
        <label class="block">
            <span class="text-sm text-neutral-400">{label}</span>
            <input type="text" class={INPUT_CLASS} {placeholder} value={(**value).clone()}
                oninput={Callback::from(move |event: InputEvent| {
                    let input: web_sys::HtmlInputElement = event.target_unchecked_into();
                    handle.set(input.value());
                })}
            />
        </label>
    }
}

fn parse_db_limits(memory_mb: &str, cpus: &str, swap_mb: &str) -> ResourceLimits {
    let mem = memory_mb
        .trim()
        .parse::<u64>()
        .ok()
        .map(|mb| mb * 1024 * 1024);
    let nano = cpus
        .trim()
        .parse::<f64>()
        .ok()
        .map(|c| (c * 1_000_000_000.0) as u64);
    let swap = match (mem, swap_mb.trim().parse::<u64>().ok()) {
        (Some(m), Some(s)) => Some(m as i64 + (s * 1024 * 1024) as i64),
        _ => None,
    };
    ResourceLimits {
        memory_bytes: mem,
        nano_cpus: nano,
        memory_swap_bytes: swap,
    }
}

/// Posts the new database and shows its password panel, or the reason of a failure.
async fn send_create(
    request: DatabaseRequest,
    notice: UseStateHandle<Option<String>>,
    busy: UseStateHandle<bool>,
    saved: Callback<()>,
    created: UseStateHandle<Option<DatabaseCreated>>,
) {
    busy.set(true);
    let sent = Request::post(&format!("{API_ENDPOINT}/databases"))
        .json(&request)
        .map(|builder| builder.send());
    match sent {
        Ok(sent) => match sent.await {
            Ok(response) if response.ok() => match response.json::<DatabaseCreated>().await {
                Ok(created_db) => {
                    notice.set(None);
                    created.set(Some(created_db));
                    saved.emit(());
                }
                Err(err) => notice.set(Some(err.to_string())),
            },
            Ok(response) => notice.set(Some(response.status_text())),
            Err(err) => notice.set(Some(err.to_string())),
        },
        Err(err) => notice.set(Some(err.to_string())),
    }
    busy.set(false);
}

/// Clones an existing database and shows its connection and password panel.
async fn send_clone(
    id: ShortId,
    name: String,
    created: UseStateHandle<Option<DatabaseCreated>>,
    notice: UseStateHandle<Option<Notice<()>>>,
    saved: Callback<()>,
) {
    let body = serde_json::json!({ "name": name });
    let req = Request::post(&format!("{API_ENDPOINT}/databases/{id}/clone"))
        .header("Content-Type", "application/json")
        .body(body.to_string());
    let sent = match req {
        Ok(b) => b.send().await,
        Err(e) => {
            notice.set(Some(Notice::Failed(e.to_string())));
            return;
        }
    };
    match sent {
        Ok(res) if res.ok() => match res.json::<DatabaseCreated>().await {
            Ok(data) => {
                created.set(Some(data));
                notice.set(Some(Notice::Saved(())));
                saved.emit(());
            }
            Err(e) => notice.set(Some(Notice::Failed(e.to_string()))),
        },
        Ok(res) => {
            let msg = res.text().await.unwrap_or_default();
            notice.set(Some(Notice::Failed(msg)));
        }
        Err(e) => notice.set(Some(Notice::Failed(e.to_string()))),
    }
}

fn clone_action(
    locale: Locale,
    id: ShortId,
    name: &str,
    reload: Callback<()>,
    created: UseStateHandle<Option<DatabaseCreated>>,
    notice: UseStateHandle<Option<Notice<()>>>,
) -> Callback<MouseEvent> {
    let default_name = format!("{name}-clone");
    Callback::from(move |_| {
        let (default_name, reload, created, notice) = (
            default_name.clone(),
            reload.clone(),
            created.clone(),
            notice.clone(),
        );
        spawn_local(async move {
            let prompt = locale.t("databases.clone_prompt");
            if let Some(name) = crate::dialog::prompt(locale, prompt, &default_name).await {
                let trimmed = name.trim().to_string();
                if !trimmed.is_empty() {
                    send_clone(id, trimmed, created, notice, reload).await;
                }
            }
        });
    })
}

fn replica_action(
    locale: Locale,
    id: ShortId,
    name: &str,
    reload: Callback<()>,
    created: UseStateHandle<Option<DatabaseCreated>>,
    notice: UseStateHandle<Option<Notice<()>>>,
) -> Callback<MouseEvent> {
    let default_name = format!("{name}-replica");
    Callback::from(move |_| {
        let (default_name, reload, created, notice) = (
            default_name.clone(),
            reload.clone(),
            created.clone(),
            notice.clone(),
        );
        spawn_local(async move {
            let prompt = locale.t("databases.replica_prompt");
            if let Some(name) = crate::dialog::prompt(locale, prompt, &default_name).await {
                let trimmed = name.trim().to_string();
                if !trimmed.is_empty() {
                    send_replica(id, trimmed, created, notice, reload).await;
                }
            }
        });
    })
}

/// Creates a read replica of an existing database and displays its details.
async fn send_replica(
    id: ShortId,
    name: String,
    created: UseStateHandle<Option<DatabaseCreated>>,
    notice: UseStateHandle<Option<Notice<()>>>,
    saved: Callback<()>,
) {
    let Ok(app_name) = name.parse() else {
        return;
    };
    let payload = DatabaseReplicaRequest {
        name: app_name,
        host_port: None,
        limits: ResourceLimits::default(),
        project_id: None,
        environment_id: None,
    };
    let Ok(body) = serde_json::to_string(&payload) else {
        return;
    };
    let req = Request::post(&format!("{API_ENDPOINT}/databases/{id}/replicas"))
        .header("Content-Type", "application/json")
        .body(body);
    let sent = match req {
        Ok(b) => b.send().await,
        Err(e) => {
            notice.set(Some(Notice::Failed(e.to_string())));
            return;
        }
    };
    match sent {
        Ok(res) if res.ok() => match res.json::<DatabaseCreated>().await {
            Ok(data) => {
                created.set(Some(data));
                notice.set(Some(Notice::Saved(())));
                saved.emit(());
            }
            Err(e) => notice.set(Some(Notice::Failed(e.to_string()))),
        },
        Ok(res) => {
            let msg = res.text().await.unwrap_or_default();
            notice.set(Some(Notice::Failed(msg)));
        }
        Err(e) => notice.set(Some(Notice::Failed(e.to_string()))),
    }
}

fn format_size(size: u64) -> String {
    if size >= 1024 * 1024 {
        format!("{:.1} MiB", size as f64 / (1024.0 * 1024.0))
    } else {
        format!("{:.1} KiB", size as f64 / 1024.0)
    }
}

async fn send_s3_restore(
    locale: Locale,
    snap: S3BackupSnapshot,
    new_name: String,
    engine: String,
    created: UseStateHandle<Option<DatabaseCreated>>,
    notice: UseStateHandle<Option<Notice<()>>>,
    saved: Callback<()>,
) {
    let payload = serde_json::json!({
        "key": snap.key,
        "new_database_name": new_name,
        "engine": engine,
    });
    let req = Request::post(&format!("{API_ENDPOINT}/databases/s3/restore"))
        .header("Content-Type", "application/json")
        .body(payload.to_string());
    match req {
        Ok(b) => match b.send().await {
            Ok(res) if res.ok() => {
                crate::dialog::message(
                    locale,
                    locale.t("databases.s3_restored"),
                    crate::dialog::Icon::Success,
                )
                .await;
                created.set(None);
                notice.set(Some(Notice::Saved(())));
                saved.emit(());
            }
            Ok(res) => {
                let msg = res.text().await.unwrap_or_default();
                notice.set(Some(Notice::Failed(msg)));
            }
            Err(e) => notice.set(Some(Notice::Failed(e.to_string()))),
        },
        Err(e) => notice.set(Some(Notice::Failed(e.to_string()))),
    }
}

fn restore_s3_action(
    locale: Locale,
    snap: &S3BackupSnapshot,
    engines: &[EngineInfo],
    saved: Callback<()>,
    created: UseStateHandle<Option<DatabaseCreated>>,
    notice: UseStateHandle<Option<Notice<()>>>,
) -> Callback<MouseEvent> {
    let snap = snap.clone();
    let default_name = format!("{}-restored", snap.database_name);
    let engine = engines
        .iter()
        .find(|e| snap.database_name.starts_with(&e.engine))
        .map(|e| e.engine.clone())
        .unwrap_or_else(|| "postgres".to_string());
    Callback::from(move |_| {
        let (snap, default_name, engine, saved, created, notice) = (
            snap.clone(),
            default_name.clone(),
            engine.clone(),
            saved.clone(),
            created.clone(),
            notice.clone(),
        );
        spawn_local(async move {
            let prompt = locale.t("databases.s3_new_name_prompt");
            if let Some(name) = crate::dialog::prompt(locale, prompt, &default_name).await {
                let trimmed = name.trim().to_string();
                if !trimmed.is_empty() {
                    send_s3_restore(locale, snap, trimmed, engine, created, notice, saved).await;
                }
            }
        });
    })
}

struct S3BrowserContext<'a> {
    locale: Locale,
    snapshots: &'a [S3BackupSnapshot],
    engines: &'a [EngineInfo],
    filter: &'a UseStateHandle<String>,
    loading: bool,
    can_edit: bool,
    load: Callback<()>,
    saved: Callback<()>,
    notice: UseStateHandle<Option<Notice<()>>>,
    created: UseStateHandle<Option<DatabaseCreated>>,
}

fn s3_browser_panel(ctx: S3BrowserContext<'_>) -> Html {
    let S3BrowserContext {
        locale,
        snapshots,
        engines,
        filter,
        loading,
        can_edit,
        load,
        saved,
        notice,
        created,
    } = ctx;
    let filter_text = (**filter).to_lowercase();
    let rows = snapshots
        .iter()
        .filter(|s| {
            filter_text.is_empty()
                || s.database_name.to_lowercase().contains(&filter_text)
                || s.key.to_lowercase().contains(&filter_text)
        })
        .map(|snap| {
            let download = format!("{API_ENDPOINT}/databases/s3/download?key={}", snap.key);
            let restore = can_edit.then(|| {
                restore_s3_action(
                    locale,
                    snap,
                    engines,
                    saved.clone(),
                    created.clone(),
                    notice.clone(),
                )
            });
            html! {
                <tr class="border-b border-neutral-700">
                    <td class="py-1.5 font-medium">{&snap.database_name}</td>
                    <td class="py-1.5 font-mono text-neutral-400">{&snap.backup_id}</td>
                    <td class="py-1.5 font-mono text-neutral-400">{format_size(snap.size)}</td>
                    <td class="py-1.5 text-neutral-400">{&snap.last_modified}</td>
                    <td class="py-1.5 text-right">
                        <a class="text-sm text-neutral-400 hover:text-neutral-200 mr-3" href={download} download={format!("{}.dump.gz", snap.backup_id)}>
                            {locale.t("backups.download")}
                        </a>
                        if let Some(restore) = restore {
                            <button type="button" class="text-xs px-2 py-1 rounded bg-blue-600 text-white hover:bg-blue-500 font-medium" onclick={restore}>
                                {locale.t("databases.s3_restore_new")}
                            </button>
                        }
                    </td>
                </tr>
            }
        });
    let on_search = {
        let filter = filter.clone();
        Callback::from(move |e: InputEvent| {
            let input: web_sys::HtmlInputElement = e.target_unchecked_into();
            filter.set(input.value());
        })
    };
    let on_refresh = Callback::from(move |_| load.emit(()));
    html! {
        <div class="rounded border border-neutral-700 bg-neutral-800/80 p-4 mb-4">
            <div class="flex items-center justify-between mb-2">
                <div>
                    <h2 class="text-base font-semibold text-neutral-100">{locale.t("databases.s3_dr_title")}</h2>
                    <p class="text-xs text-neutral-400">{locale.t("databases.s3_dr_desc")}</p>
                </div>
                <button type="button" class="px-2.5 py-1 text-xs rounded bg-neutral-700 text-neutral-200 hover:bg-neutral-600" onclick={on_refresh}>
                    {locale.t("databases.s3_refresh")}
                </button>
            </div>
            <div class="my-3">
                <input
                    type="text"
                    class={INPUT_CLASS}
                    placeholder={locale.t("databases.s3_filter_placeholder").to_string()}
                    value={(**filter).clone()}
                    oninput={on_search}
                />
            </div>
            if loading {
                <p class="text-xs text-neutral-400">{locale.t("common.loading")}</p>
            } else if snapshots.is_empty() {
                <p class="text-xs text-neutral-400">{locale.t("databases.s3_empty")}</p>
            } else {
                <table class="w-full text-sm">
                    <thead>
                        <tr class="text-left text-neutral-400">
                            <th class="py-1.5 font-medium">{locale.t("common.name")}</th>
                            <th class="py-1.5 font-medium">{locale.t("backups.title")}</th>
                            <th class="py-1.5 font-medium">{locale.t("backups.size")}</th>
                            <th class="py-1.5 font-medium">{locale.t("backups.time")}</th>
                            <th />
                        </tr>
                    </thead>
                    <tbody>{ for rows }</tbody>
                </table>
            }
        </div>
    }
}
