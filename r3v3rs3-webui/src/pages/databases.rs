//! The databases: the apps with a database engine, created and deleted on this page.

use super::Route;
use super::accounts::{INPUT_CLASS, LABEL_CLASS};
use super::database_credentials::CredentialsPanel;
use super::resource_page::{Notice, add_button, delete_on_click, notice_view};
use super::settings::{fetch_json, success_box};
use crate::API_ENDPOINT;
use crate::auth::use_ensure_auth;
use crate::components::action_button::{ActionIconButton, ActionVariant};
use crate::components::button::{Button, ButtonKind, ButtonSize, button_class};
use crate::components::data_list::{Column, Row, list_card};
use crate::components::modal::{Modal, ModalSize};
use crate::i18n::use_locale;
use crate::store::SessionStore;
use gloo_net::http::Request;
use r3v3rs3_api::container::ResourceLimits;
use r3v3rs3_api::i18n::Locale;
use r3v3rs3_api::id::ShortId;
use r3v3rs3_api::platform::{
    AppEntry, DatabaseCreated, DatabaseReplicaRequest, DatabaseRequest, EngineInfo,
};
use wasm_bindgen_futures::spawn_local;
use yew::prelude::*;
use yew_router::prelude::*;
use yewdux::prelude::*;

const COLUMNS: [Column; 5] = [
    Column {
        label: "common.name",
        class: "whitespace-nowrap",
    },
    Column {
        label: "databases.engine",
        class: "whitespace-nowrap",
    },
    Column {
        label: "common.port",
        class: "whitespace-nowrap font-mono",
    },
    Column {
        label: "databases.connection",
        class: "font-mono break-all",
    },
    Column {
        label: "databases.apps",
        class: "",
    },
];

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
            let refreshed = fetch_json::<Vec<EngineInfo>>(
                locale,
                Request::post(&format!("{API_ENDPOINT}/databases/engines/refresh")),
            )
            .await;
            refreshing.set(false);
            match refreshed {
                Ok(list) => {
                    engines.set(list);
                    notice.set(Some(Notice::Saved(())));
                }
                Err(message) => notice.set(Some(Notice::Failed(message))),
            }
        });
    })
}

fn db_header_bar(
    locale: Locale,
    can_edit: bool,
    refreshing: bool,
    refresh_engines: Callback<MouseEvent>,
) -> Html {
    html! {
        <div class="flex flex-wrap items-center justify-between gap-3 mb-4">
            <h1 class="text-xl font-semibold text-neutral-900 dark:text-neutral-100">{locale.t("nav.databases")}</h1>
            <div class="flex items-center gap-2">
                <Link<Route>
                    to={Route::Backups}
                    classes={button_class(ButtonKind::Secondary, ButtonSize::Md)}
                >
                    <svg class="w-4 h-4" fill="none" stroke="currentColor" viewBox="0 0 24 24">
                        <path stroke-linecap="round" stroke-linejoin="round" stroke-width="2" d="M7 16a4 4 0 01-.88-7.903A5 5 0 1115.9 6L16 6a5 5 0 011 9.9M15 13l-3-3m0 0l-3 3m3-3v12" />
                    </svg>
                    {locale.t("backups.nav")}
                </Link<Route>>
                if can_edit {
                    <Button kind={ButtonKind::Secondary} disabled={refreshing} onclick={refresh_engines}>
                        {locale.t("databases.refresh")}
                    </Button>
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
    let viewing = use_state(|| Option::<AppEntry>::None);
    let adding = use_state(|| false);

    let signed_in = session.info.is_some();
    let can_edit = session.can_edit();
    let is_admin = session.is_admin();
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
    let navigator = use_navigator();
    let reload = {
        let (databases, notice, apps) = (databases.clone(), notice.clone(), apps.clone());
        Callback::from(move |_: ()| {
            reload_lists(locale, &databases, &apps, &notice);
        })
    };
    let refresh_engines = refresh_callback(locale, &engines, &notice, &refreshing);
    let header = db_header_bar(locale, can_edit, *refreshing, refresh_engines);
    let rows = database_rows(
        &databases,
        &apps,
        &RowContext {
            locale,
            engines: &engines,
            can_edit,
            is_admin,
            reload: reload.clone(),
            created: &created,
            notice: &notice,
            viewing: &viewing,
            navigator,
        },
    );
    html! {
        <>
            { header }
            { notice_view(&notice, |_| html! { {locale.t("databases.refreshed")} }) }
            { credentials_view(locale, &viewing, &apps) }
            { (*created)
                .clone()
                .map(|value| created_view(locale, value, &engines, &created))
                .unwrap_or_default() }
            { list_card(locale, true, "databases.empty", &COLUMNS, &rows) }
            if can_edit {
                { add_button(locale, open_callback(&adding)) }
                if *adding {
                    <AddForm
                        locale={locale}
                        engines={(*engines).clone()}
                        saved={reload}
                        created={created.clone()}
                        close={close_callback(&adding)}
                    />
                }
            }
        </>
    }
}

fn open_callback(adding: &UseStateHandle<bool>) -> Callback<MouseEvent> {
    let adding = adding.clone();
    Callback::from(move |_| adding.set(true))
}

fn close_callback(adding: &UseStateHandle<bool>) -> Callback<()> {
    let adding = adding.clone();
    Callback::from(move |()| adding.set(false))
}

/// The credentials panel of the database in `viewing`.
fn credentials_view(
    locale: Locale,
    viewing: &UseStateHandle<Option<AppEntry>>,
    apps: &[AppEntry],
) -> Html {
    let Some(database) = (**viewing).clone() else {
        return html! {};
    };
    let close = {
        let viewing = viewing.clone();
        Callback::from(move |()| viewing.set(None))
    };
    let key = database.id.to_string();
    html! {
        <CredentialsPanel
            {key}
            {locale}
            {database}
            apps={apps.to_vec()}
            {close}
        />
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
    /// Only an Admin account reads the passwords of a database.
    is_admin: bool,
    reload: Callback<()>,
    created: &'a UseStateHandle<Option<DatabaseCreated>>,
    notice: &'a UseStateHandle<Option<Notice<()>>>,
    /// The database whose credentials panel is open.
    viewing: &'a UseStateHandle<Option<AppEntry>>,
    navigator: Option<Navigator>,
}

/// The button that opens the credentials panel of a database, for an Admin account.
fn credentials_button(ctx: &RowContext<'_>, app: &AppEntry) -> Html {
    if !ctx.is_admin {
        return html!();
    }
    let onclick = {
        let (viewing, app) = (ctx.viewing.clone(), app.clone());
        Callback::from(move |e: MouseEvent| {
            e.prevent_default();
            viewing.set(Some(app.clone()));
        })
    };
    html! {
        <ActionIconButton
            variant={ActionVariant::View}
            title={ctx.locale.t("databases.credentials").to_string()}
            {onclick}
        />
    }
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

/// A callback that opens `route`.
fn route_callback(navigator: &Option<Navigator>, route: Route) -> Callback<MouseEvent> {
    let navigator = navigator.clone();
    Callback::from(move |e: MouseEvent| {
        e.prevent_default();
        if let Some(nav) = &navigator {
            nav.push(&route);
        }
    })
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
    let id = app.id;
    html! {
        <>
            { credentials_button(ctx, app) }
            <ActionIconButton
                variant={ActionVariant::Metrics}
                title={ctx.locale.t("common.view").to_string()}
                onclick={route_callback(&ctx.navigator, Route::AppDashboard { id })}
            />
            <ActionIconButton
                variant={ActionVariant::Files}
                title={ctx.locale.t("backups.nav").to_string()}
                onclick={route_callback(&ctx.navigator, Route::DatabaseBackups { id })}
            />
            { render_nav_action_buttons(ctx, app) }
            { render_lifecycle_action_buttons(ctx, app, is_replica) }
            if let Some(delete) = delete {
                <ActionIconButton
                    variant={ActionVariant::Delete}
                    title={ctx.locale.t("common.delete").to_string()}
                    onclick={delete}
                />
            }
        </>
    }
}

fn render_nav_action_buttons(ctx: &RowContext<'_>, app: &AppEntry) -> Html {
    let admin_ui_btn = app.spec.database.as_deref().map(|engine| {
        let store = admin_ui_store_id(engine).to_string();
        html! {
            <ActionIconButton
                variant={ActionVariant::Play}
                title={ctx.locale.t("databases.admin_ui").to_string()}
                onclick={route_callback(&ctx.navigator, Route::NewStoreApp { store })}
            />
        }
    });
    let terminal_btn = (app.target.to_string() == r3v3rs3_api::platform::LOCAL_TARGET).then(|| {
        html! {
            <ActionIconButton
                variant={ActionVariant::Terminal}
                title={ctx.locale.t("databases.cli").to_string()}
                onclick={route_callback(&ctx.navigator, Route::AppTerminal { id: app.id })}
            />
        }
    });
    html! {
        <>
            if let Some(cli) = terminal_btn {
                {cli}
            }
            if let Some(admin_ui) = admin_ui_btn {
                {admin_ui}
            }
        </>
    }
}

fn render_lifecycle_action_buttons(ctx: &RowContext<'_>, app: &AppEntry, is_replica: bool) -> Html {
    let clone_btn = ctx.can_edit.then(|| {
        let onclick = clone_action(
            ctx.locale,
            app.id,
            app.name.as_str(),
            ctx.reload.clone(),
            ctx.created.clone(),
            ctx.notice.clone(),
        );
        html! {
            <ActionIconButton
                variant={ActionVariant::Edit}
                title={ctx.locale.t("databases.clone").to_string()}
                onclick={onclick}
            />
        }
    });
    let replica_btn = (ctx.can_edit && !is_replica).then(|| {
        let onclick = replica_action(
            ctx.locale,
            app.id,
            app.name.as_str(),
            ctx.reload.clone(),
            ctx.created.clone(),
            ctx.notice.clone(),
        );
        html! {
            <ActionIconButton
                variant={ActionVariant::Restore}
                title={ctx.locale.t("databases.create_replica").to_string()}
                onclick={onclick}
            />
        }
    });
    html! {
        <>
            if let Some(replica) = replica_btn {
                {replica}
            }
            if let Some(clone) = clone_btn {
                {clone}
            }
        </>
    }
}

const BADGE: &str = "ml-2 px-1.5 py-0.5 text-xs rounded border";

/// The name of a database with its replica, project or shared badge.
fn name_cell(locale: Locale, app: &AppEntry) -> Html {
    let is_replica = app.spec.replica_of.is_some();
    html! {
        <>
            {app.name.to_string()}
            if is_replica {
                <span class={classes!(BADGE, "bg-neutral-100", "text-neutral-700", "border-neutral-300", "dark:bg-neutral-700", "dark:text-neutral-300", "dark:border-neutral-600")}>
                    {locale.t("databases.read_replica")}
                </span>
            }
            if app.spec.project_id.is_some() {
                <span class={classes!(BADGE, "bg-emerald-50", "text-emerald-700", "border-emerald-300", "dark:bg-emerald-900/40", "dark:text-emerald-300", "dark:border-emerald-700")}>
                    {locale.t("projects.project")}
                </span>
            } else if !is_replica {
                <span class={classes!(BADGE, "bg-sky-50", "text-sky-700", "border-sky-300", "dark:bg-sky-900/40", "dark:text-sky-300", "dark:border-sky-700")}>
                    {locale.t("databases.shared")}
                </span>
            }
        </>
    }
}

fn db_row(ctx: &RowContext<'_>, app: &AppEntry, users: String) -> Row {
    let engine = app
        .spec
        .database
        .as_deref()
        .and_then(|key| engine_of(ctx.engines, key))
        .map(|info| info.label.clone())
        .unwrap_or_else(|| app.spec.database.clone().unwrap_or_default());
    Row {
        key: app.id.to_string(),
        cells: vec![
            name_cell(ctx.locale, app),
            html! { {engine} },
            html! { {port_of(app).to_string()} },
            html! { {connection(app, ctx.engines)} },
            html! { {users} },
        ],
        actions: db_actions(ctx, app),
    }
}

/// The rows of the databases, with the names of the apps that use each one.
fn database_rows(databases: &[AppEntry], apps: &[AppEntry], ctx: &RowContext<'_>) -> Vec<Row> {
    databases
        .iter()
        .map(|app| {
            let users = apps
                .iter()
                .filter(|a| a.spec.uses_database == Some(app.id))
                .map(|a| a.name.to_string())
                .collect::<Vec<_>>()
                .join(", ");
            db_row(ctx, app, users)
        })
        .collect()
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
    success_box(html! {
        <>
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
            <Button kind={ButtonKind::Secondary} size={ButtonSize::Sm} onclick={close}>
                {locale.t("common.ok")}
            </Button>
        </>
    })
}

/// The form that installs a database of one engine and version.
#[derive(Properties, PartialEq)]
struct AddFormProps {
    locale: Locale,
    engines: Vec<EngineInfo>,
    saved: Callback<()>,
    created: UseStateHandle<Option<DatabaseCreated>>,
    /// Closes the modal of the form.
    close: Callback<()>,
}

/// The fields of the form, each a text the account types.
#[derive(Clone)]
struct AddFields {
    name: UseStateHandle<String>,
    version: UseStateHandle<String>,
    host_port: UseStateHandle<String>,
    memory_mb: UseStateHandle<String>,
    swap_mb: UseStateHandle<String>,
    cpus: UseStateHandle<String>,
}

#[hook]
fn use_add_fields() -> AddFields {
    AddFields {
        name: use_state(String::new),
        version: use_state(String::new),
        host_port: use_state(String::new),
        memory_mb: use_state(String::new),
        swap_mb: use_state(String::new),
        cpus: use_state(String::new),
    }
}

/// Keeps the value of a select element equal to `value` after its options change.
#[hook]
fn use_select_value(value: String, options: usize) -> NodeRef {
    let node = use_node_ref();
    {
        let node = node.clone();
        use_effect_with((value, options), move |(val, _)| {
            if let Some(el) = node.cast::<web_sys::HtmlSelectElement>() {
                el.set_value(val);
            }
        });
    }
    node
}

#[function_component(AddForm)]
fn add_form(props: &AddFormProps) -> Html {
    let locale = props.locale;
    let engines = props.engines.clone();
    let engine = use_state(|| Option::<String>::None);
    let selected = (*engine)
        .as_deref()
        .and_then(|key| engines.iter().find(|info| info.engine == key).cloned());
    let versions: Vec<String> = selected
        .as_ref()
        .map(|info| info.versions.clone())
        .unwrap_or_default();
    let fields = use_add_fields();
    let notice = use_state(|| Option::<String>::None);
    let busy = use_state(|| false);
    let engine_ref = use_select_value(
        engine.as_deref().unwrap_or_default().to_string(),
        engines.len(),
    );
    let version_ref = use_select_value((*fields.version).clone(), versions.len());

    let pick_engine = {
        let engine = engine.clone();
        let version = fields.version.clone();
        Callback::from(move |event: Event| {
            let select: web_sys::HtmlSelectElement = event.target_unchecked_into();
            engine.set(Some(select.value()));
            version.set(String::new());
        })
    };
    let submit = submit_callback(props, &engine, &fields, (&notice, &busy));
    let footer_cancel = {
        let close = props.close.clone();
        Callback::from(move |_| close.emit(()))
    };
    html! {
        <Modal title={locale.t("databases.add")} on_close={props.close.clone()} size={ModalSize::Lg}>
            <form onsubmit={submit}>
                if let Some(message) = notice.as_ref() {
                    <p class="mb-2 text-sm text-red-600 dark:text-red-500">{message.clone()}</p>
                }
                <div class="grid gap-x-4 md:grid-cols-2">
                    <label class="block">
                        <span class={LABEL_CLASS}>{locale.t("databases.engine")}</span>
                        <select ref={engine_ref} class={INPUT_CLASS} onchange={pick_engine}>
                            <option key="" value="" selected={engine.is_none()}>{locale.t("databases.pick_engine")}</option>
                            { for engines.iter().map(|info| {
                                let value = info.engine.clone();
                                let is_selected =
                                    selected.as_ref().map(|s| &s.engine) == Some(&value);
                                html! { <option key={value.clone()} {value} selected={is_selected}>{info.label.clone()}</option> }
                            }) }
                        </select>
                    </label>
                    { version_select(locale, &fields.version, &versions, version_ref) }
                    { form_input(locale.t("common.name").into(), "".into(), &fields.name) }
                    { form_input(locale.t("databases.host_port").into(), locale.t("databases.host_port_hint").into(), &fields.host_port) }
                </div>
                <div class="grid gap-x-4 md:grid-cols-3">
                    { form_input(locale.t("databases.memory_mb").into(), "512".into(), &fields.memory_mb) }
                    { form_input(locale.t("databases.swap_mb").into(), "0".into(), &fields.swap_mb) }
                    { form_input(locale.t("databases.cpus").into(), "1.0".into(), &fields.cpus) }
                </div>
                <div class="flex flex-col-reverse gap-2 mt-6 sm:flex-row sm:justify-end">
                    <Button kind={ButtonKind::Secondary} onclick={footer_cancel}>
                        {locale.t("common.cancel")}
                    </Button>
                    <Button kind={ButtonKind::Primary} r#type="submit" disabled={*busy}>
                        {locale.t("databases.create")}
                    </Button>
                </div>
            </form>
        </Modal>
    }
}

fn version_select(
    locale: Locale,
    version: &UseStateHandle<String>,
    versions: &[String],
    node: NodeRef,
) -> Html {
    let onchange = {
        let version = version.clone();
        Callback::from(move |event: Event| {
            let select: web_sys::HtmlSelectElement = event.target_unchecked_into();
            version.set(select.value());
        })
    };
    html! {
        <label class="block">
            <span class={LABEL_CLASS}>{locale.t("databases.version")}</span>
            <select ref={node} class={INPUT_CLASS} {onchange}>
                <option key="" value="" selected={version.is_empty()}>{locale.t("databases.pick_version")}</option>
                { for versions.iter().map(|tag| {
                    let value = tag.clone();
                    let is_selected = **version == value;
                    html! { <option key={value.clone()} {value} selected={is_selected}>{tag.clone()}</option> }
                }) }
            </select>
        </label>
    }
}

/// Checks the form and sends the new database.
fn submit_callback(
    props: &AddFormProps,
    engine: &UseStateHandle<Option<String>>,
    fields: &AddFields,
    (notice, busy): (&UseStateHandle<Option<String>>, &UseStateHandle<bool>),
) -> Callback<SubmitEvent> {
    let locale = props.locale;
    let (engine, fields, notice, busy) =
        (engine.clone(), fields.clone(), notice.clone(), busy.clone());
    let (saved, created, close) = (
        props.saved.clone(),
        props.created.clone(),
        props.close.clone(),
    );
    Callback::from(move |event: SubmitEvent| {
        event.prevent_default();
        let Some(engine) = (*engine).clone() else {
            notice.set(Some(locale.t("databases.engine_required").to_string()));
            return;
        };
        let Ok(parsed_name) = fields.name.trim().parse() else {
            notice.set(Some(locale.t("databases.name_invalid").to_string()));
            return;
        };
        let limits = parse_db_limits(&fields.memory_mb, &fields.cpus, &fields.swap_mb);
        let request = DatabaseRequest {
            name: parsed_name,
            engine,
            version: fields.version.trim().to_string(),
            host_port: fields.host_port.trim().parse().ok(),
            limits,
            project_id: None,
            environment_id: None,
        };
        let done = Done {
            notice: notice.clone(),
            busy: busy.clone(),
            saved: saved.clone(),
            created: created.clone(),
            close: close.clone(),
        };
        spawn_local(send_create(request, done));
    })
}

fn form_input(label: AttrValue, placeholder: AttrValue, value: &UseStateHandle<String>) -> Html {
    let handle = value.clone();
    html! {
        <label class="block">
            <span class={LABEL_CLASS}>{label}</span>
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
        pids_limit: None,
    }
}

/// What the form updates after it sends a new database.
struct Done {
    notice: UseStateHandle<Option<String>>,
    busy: UseStateHandle<bool>,
    saved: Callback<()>,
    created: UseStateHandle<Option<DatabaseCreated>>,
    close: Callback<()>,
}

/// Posts the new database and shows its password panel, or the reason of a failure.
async fn send_create(request: DatabaseRequest, done: Done) {
    done.busy.set(true);
    let sent = Request::post(&format!("{API_ENDPOINT}/databases"))
        .json(&request)
        .map(|builder| builder.send());
    match sent {
        Ok(sent) => match sent.await {
            Ok(response) if response.ok() => match response.json::<DatabaseCreated>().await {
                Ok(created_db) => {
                    done.notice.set(None);
                    done.created.set(Some(created_db));
                    done.saved.emit(());
                    done.close.emit(());
                }
                Err(err) => done.notice.set(Some(err.to_string())),
            },
            Ok(response) => done.notice.set(Some(response.status_text())),
            Err(err) => done.notice.set(Some(err.to_string())),
        },
        Err(err) => done.notice.set(Some(err.to_string())),
    }
    done.busy.set(false);
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
