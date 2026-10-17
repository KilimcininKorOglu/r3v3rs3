//! The databases: the apps with a database engine, created and deleted on this page.

use super::Route;
use super::resource_page::{Notice, delete_on_click, notice_view};
use super::settings::fetch_json;
use crate::API_ENDPOINT;
use crate::auth::use_ensure_auth;
use crate::i18n::use_locale;
use crate::store::SessionStore;
use gloo_net::http::Request;
use r3v3rs3_api::i18n::Locale;
use r3v3rs3_api::platform::{
    AppEntry, DatabaseCreated, DatabaseEngine, DatabaseRequest, EngineInfo,
};
use wasm_bindgen_futures::spawn_local;
use yew::prelude::*;
use yew_router::prelude::*;
use yewdux::prelude::*;

const INPUT_CLASS: &str =
    "w-full rounded bg-neutral-800 text-neutral-100 border border-neutral-700 px-2 py-1.5 text-sm";

/// The value of an engine in a select, and back.
fn engine_value(engine: DatabaseEngine) -> &'static str {
    match engine {
        DatabaseEngine::Postgres => "postgres",
        DatabaseEngine::MariaDb => "mariadb",
        DatabaseEngine::MySql => "mysql",
        DatabaseEngine::MongoDb => "mongodb",
        DatabaseEngine::Redis => "redis",
    }
}

fn engine_of(value: &str) -> Option<DatabaseEngine> {
    match value {
        "postgres" => Some(DatabaseEngine::Postgres),
        "mariadb" => Some(DatabaseEngine::MariaDb),
        "mysql" => Some(DatabaseEngine::MySql),
        "mongodb" => Some(DatabaseEngine::MongoDb),
        "redis" => Some(DatabaseEngine::Redis),
        _ => None,
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

    let signed_in = session.info.is_some();
    let can_edit = session.can_edit();
    {
        let (databases, engines, notice) = (databases.clone(), engines.clone(), notice.clone());
        use_effect_with(signed_in, move |&signed_in| {
            if signed_in {
                let (databases, engines, notice) =
                    (databases.clone(), engines.clone(), notice.clone());
                spawn_local(async move {
                    match fetch_json::<Vec<AppEntry>>(
                        Locale::En,
                        Request::get(&format!("{API_ENDPOINT}/databases")),
                    )
                    .await
                    {
                        Ok(list) => databases.set(list),
                        Err(message) => notice.set(Some(Notice::Failed(message))),
                    }
                    if let Ok(list) = fetch_json::<Vec<EngineInfo>>(
                        Locale::En,
                        Request::get(&format!("{API_ENDPOINT}/databases/engines")),
                    )
                    .await
                    {
                        engines.set(list);
                    }
                });
            }
        });
    }

    if session.info.is_none() {
        return html!();
    }
    let reload = {
        let (databases, notice) = (databases.clone(), notice.clone());
        Callback::from(move |_: ()| {
            let (databases, notice) = (databases.clone(), notice.clone());
            spawn_local(async move {
                match fetch_json::<Vec<AppEntry>>(
                    Locale::En,
                    Request::get(&format!("{API_ENDPOINT}/databases")),
                )
                .await
                {
                    Ok(list) => databases.set(list),
                    Err(message) => notice.set(Some(Notice::Failed(message))),
                }
            });
        })
    };
    html! {
        <>
            <h1 class="text-xl font-semibold mb-3">{locale.t("nav.databases")}</h1>
            { notice_view(&notice, |_| html!()) }
            { (*created)
                .clone()
                .map(|value| created_view(locale, value, &created))
                .unwrap_or_default() }
            if can_edit {
                <AddForm
                    locale={locale}
                    engines={(*engines).clone()}
                    saved={reload.clone()}
                    created={created}
                />
            }
            { list_view(locale, &databases, can_edit, reload.clone()) }
        </>
    }
}

/// The port that the clients of the database reach on the server.
fn port_of(app: &AppEntry) -> u16 {
    app.spec.host_port.unwrap_or(app.spec.port)
}

/// The connection string of a database, without a password.
fn connection(app: &AppEntry) -> String {
    let Some(engine) = app.spec.database else {
        return String::new();
    };
    let info = engine.info();
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

/// The table of the databases.
fn list_view(locale: Locale, databases: &[AppEntry], can_edit: bool, reload: Callback<()>) -> Html {
    if databases.is_empty() {
        return html! { <p class="text-neutral-400 mt-2">{locale.t("databases.empty")}</p> };
    }
    let rows = databases
        .iter()
        .map(|app| {
            let engine = app
                .spec
                .database
                .map(|engine| engine.info().label.clone())
                .unwrap_or_default();
            let delete = can_edit.then(|| {
                let question =
                    locale.tf("databases.confirm_delete", &[("name", app.name.as_str())]);
                delete_on_click(
                    locale,
                    question,
                    format!("/databases/{}", app.id),
                    reload.clone(),
                )
            });
            html! {
                <tr class="border-b border-neutral-700">
                    <td class="py-1.5">{app.name.to_string()}</td>
                    <td class="py-1.5 text-neutral-400">{engine}</td>
                    <td class="py-1.5 font-mono text-neutral-400">{port_of(app).to_string()}</td>
                    <td class="py-1.5 font-mono text-neutral-400">{connection(app)}</td>
                    <td class="py-1.5 text-right">
                        <Link<Route> to={Route::AppView { id: app.id }} classes="text-sm text-neutral-400 hover:text-neutral-200 mr-3">
                            {locale.t("common.view")}
                        </Link<Route>>
                        if let Some(delete) = delete {
                            <a class="text-red-400 hover:text-red-300" onclick={delete}>
                                {locale.t("common.delete")}
                            </a>
                        }
                    </td>
                </tr>
            }
        })
        .collect::<Vec<_>>();
    html! {
        <table class="w-full text-sm mt-3">
            <thead>
                <tr class="text-left text-neutral-400">
                    <th class="py-1.5 font-medium">{locale.t("common.name")}</th>
                    <th class="py-1.5 font-medium">{locale.t("databases.engine")}</th>
                    <th class="py-1.5 font-medium">{locale.t("common.port")}</th>
                    <th class="py-1.5 font-medium">{locale.t("databases.connection")}</th>
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
    clear: &UseStateHandle<Option<DatabaseCreated>>,
) -> Html {
    let info = created.app.spec.database.map(|engine| engine.info());
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
    let engine = use_state(|| Option::<DatabaseEngine>::None);
    let selected =
        (*engine).and_then(|engine| engines.iter().find(|info| info.engine == engine).cloned());
    let version = use_state(String::new);
    let name = use_state(String::new);
    let host_port = use_state(String::new);
    let notice = use_state(|| Option::<String>::None);
    let busy = use_state(|| false);

    let pick_engine = {
        let engine = engine.clone();
        let version = version.clone();
        Callback::from(move |event: Event| {
            let select: web_sys::HtmlSelectElement = event.target_unchecked_into();
            engine.set(engine_of(&select.value()));
            version.set(String::new());
        })
    };
    let submit = {
        let (name, version, host_port, engine, notice, busy) = (
            name.clone(),
            version.clone(),
            host_port.clone(),
            engine.clone(),
            notice.clone(),
            busy.clone(),
        );
        let saved = props.saved.clone();
        let created = props.created.clone();
        Callback::from(move |event: SubmitEvent| {
            event.prevent_default();
            let Some(engine) = *engine else {
                notice.set(Some(locale.t("databases.engine_required").to_string()));
                return;
            };
            let Ok(parsed_name) = name.trim().parse() else {
                notice.set(Some(locale.t("databases.name_invalid").to_string()));
                return;
            };
            let request = DatabaseRequest {
                name: parsed_name,
                engine,
                version: version.trim().to_string(),
                host_port: host_port.trim().parse().ok(),
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
                            let value = engine_value(info.engine);
                            let is_selected = selected.as_ref().map(|s| s.engine) == Some(info.engine);
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
                <label class="block">
                    <span class="text-sm text-neutral-400">{locale.t("common.name")}</span>
                    <input type="text" class={INPUT_CLASS} value={(*name).clone()}
                        oninput={{
                            let name = name.clone();
                            Callback::from(move |event: InputEvent| {
                                let input: web_sys::HtmlInputElement = event.target_unchecked_into();
                                name.set(input.value());
                            })
                        }}
                    />
                </label>
                <label class="block">
                    <span class="text-sm text-neutral-400">{locale.t("databases.host_port")}</span>
                    <input type="text" class={INPUT_CLASS} placeholder={locale.t("databases.host_port_hint")} value={(*host_port).clone()}
                        oninput={{
                            let host_port = host_port.clone();
                            Callback::from(move |event: InputEvent| {
                                let input: web_sys::HtmlInputElement = event.target_unchecked_into();
                                host_port.set(input.value());
                            })
                        }}
                    />
                </label>
            </div>
            <div class="flex justify-end mt-3">
                <button type="submit" class="px-3 py-1.5 rounded bg-neutral-100 text-neutral-900 font-medium disabled:opacity-50" disabled={*busy}>
                    {locale.t("databases.create")}
                </button>
            </div>
        </form>
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
