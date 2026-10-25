//! The web terminal into the running container of an app, with multi-tab support,
//! session command history memory, and quick maintenance snippets.

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
use std::collections::HashMap;
use std::rc::Rc;
use wasm_bindgen::JsCast;
use wasm_bindgen::prelude::*;
use yew::prelude::*;
use yew_router::prelude::*;
use yewdux::prelude::*;

#[derive(Clone, PartialEq, Debug)]
pub struct TerminalTab {
    pub id: usize,
    pub title: String,
    pub shell: String,
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum SnippetCategory {
    All,
    System,
    Files,
    Network,
    Env,
    Db,
}

#[derive(Clone, Copy, Debug)]
pub struct SnippetItem {
    pub category: SnippetCategory,
    pub command: &'static str,
    pub desc: &'static str,
}

const SYSTEM_SNIPPETS: &[SnippetItem] = &[
    SnippetItem {
        category: SnippetCategory::System,
        command: "ps aux",
        desc: "List running processes",
    },
    SnippetItem {
        category: SnippetCategory::System,
        command: "top -b -n 1",
        desc: "CPU & memory usage snapshot",
    },
    SnippetItem {
        category: SnippetCategory::System,
        command: "df -h",
        desc: "Filesystem disk space",
    },
    SnippetItem {
        category: SnippetCategory::System,
        command: "free -m",
        desc: "RAM usage in MB",
    },
    SnippetItem {
        category: SnippetCategory::System,
        command: "uptime",
        desc: "System uptime and load",
    },
    SnippetItem {
        category: SnippetCategory::System,
        command: "uname -a",
        desc: "Kernel & architecture info",
    },
];

const FILES_SNIPPETS: &[SnippetItem] = &[
    SnippetItem {
        category: SnippetCategory::Files,
        command: "ls -lah",
        desc: "Detailed file listing",
    },
    SnippetItem {
        category: SnippetCategory::Files,
        command: "pwd",
        desc: "Current working directory",
    },
    SnippetItem {
        category: SnippetCategory::Files,
        command: "du -sh * | sort -h",
        desc: "Directory disk usage",
    },
    SnippetItem {
        category: SnippetCategory::Files,
        command: "find . -maxdepth 2 -type f",
        desc: "Recent files in path",
    },
];

const NETWORK_SNIPPETS: &[SnippetItem] = &[
    SnippetItem {
        category: SnippetCategory::Network,
        command: "ip addr || ifconfig",
        desc: "Network interfaces & IPs",
    },
    SnippetItem {
        category: SnippetCategory::Network,
        command: "netstat -tuln || ss -tuln",
        desc: "Listening TCP/UDP ports",
    },
    SnippetItem {
        category: SnippetCategory::Network,
        command: "curl -I localhost",
        desc: "Test local HTTP endpoint",
    },
    SnippetItem {
        category: SnippetCategory::Network,
        command: "cat /etc/hosts",
        desc: "View hosts configuration",
    },
    SnippetItem {
        category: SnippetCategory::Network,
        command: "cat /etc/resolv.conf",
        desc: "View DNS nameservers",
    },
];

const ENV_SNIPPETS: &[SnippetItem] = &[
    SnippetItem {
        category: SnippetCategory::Env,
        command: "env | sort",
        desc: "Sorted environment variables",
    },
    SnippetItem {
        category: SnippetCategory::Env,
        command: "whoami && id",
        desc: "Current user identity and groups",
    },
];

fn get_db_snippets(engine: &str) -> &'static [SnippetItem] {
    match engine {
        "postgres" => &[
            SnippetItem {
                category: SnippetCategory::Db,
                command: "\\l",
                desc: "List databases",
            },
            SnippetItem {
                category: SnippetCategory::Db,
                command: "\\dt",
                desc: "List tables",
            },
            SnippetItem {
                category: SnippetCategory::Db,
                command: "\\di",
                desc: "List indexes",
            },
            SnippetItem {
                category: SnippetCategory::Db,
                command: "\\conninfo",
                desc: "Connection info",
            },
            SnippetItem {
                category: SnippetCategory::Db,
                command: "SELECT version();",
                desc: "PostgreSQL version",
            },
        ],
        "mysql" | "mariadb" => &[
            SnippetItem {
                category: SnippetCategory::Db,
                command: "SHOW DATABASES;",
                desc: "List databases",
            },
            SnippetItem {
                category: SnippetCategory::Db,
                command: "SHOW TABLES;",
                desc: "List tables",
            },
            SnippetItem {
                category: SnippetCategory::Db,
                command: "STATUS;",
                desc: "Server status",
            },
            SnippetItem {
                category: SnippetCategory::Db,
                command: "SHOW PROCESSLIST;",
                desc: "Active queries",
            },
            SnippetItem {
                category: SnippetCategory::Db,
                command: "SELECT VERSION();",
                desc: "Engine version",
            },
        ],
        "redis" => &[
            SnippetItem {
                category: SnippetCategory::Db,
                command: "INFO",
                desc: "Redis statistics",
            },
            SnippetItem {
                category: SnippetCategory::Db,
                command: "DBSIZE",
                desc: "Key count",
            },
            SnippetItem {
                category: SnippetCategory::Db,
                command: "PING",
                desc: "Ping test",
            },
            SnippetItem {
                category: SnippetCategory::Db,
                command: "CLIENT LIST",
                desc: "Connected clients",
            },
        ],
        "mongodb" => &[
            SnippetItem {
                category: SnippetCategory::Db,
                command: "show dbs",
                desc: "List databases",
            },
            SnippetItem {
                category: SnippetCategory::Db,
                command: "show collections",
                desc: "List collections",
            },
            SnippetItem {
                category: SnippetCategory::Db,
                command: "db.stats()",
                desc: "Database stats",
            },
        ],
        _ => &[],
    }
}

fn all_snippets_for_app(app: Option<&AppEntry>) -> Vec<SnippetItem> {
    let mut list = Vec::new();
    list.extend_from_slice(SYSTEM_SNIPPETS);
    list.extend_from_slice(FILES_SNIPPETS);
    list.extend_from_slice(NETWORK_SNIPPETS);
    list.extend_from_slice(ENV_SNIPPETS);
    if let Some(engine) = app.and_then(|a| a.spec.database.as_deref()) {
        list.extend_from_slice(get_db_snippets(engine));
    }
    list
}

#[derive(Properties, PartialEq)]
pub struct Props {
    pub id: ShortId,
}

struct TerminalState {
    pub app: UseStateHandle<Option<AppEntry>>,
    pub failed: UseStateHandle<Option<String>>,
    pub tabs: UseStateHandle<Vec<TerminalTab>>,
    pub active_tab_id: UseStateHandle<usize>,
    pub next_tab_id: UseStateHandle<usize>,
    pub custom_input: UseStateHandle<String>,
    pub history: UseStateHandle<Vec<String>>,
    pub history_index: UseStateHandle<Option<usize>>,
    pub show_history_modal: UseStateHandle<bool>,
    pub history_search: UseStateHandle<String>,
    pub show_snippets: UseStateHandle<bool>,
    pub snippet_category: UseStateHandle<SnippetCategory>,
    pub copied: UseStateHandle<bool>,
    pub sessions_ref: Rc<RefCell<HashMap<usize, TerminalSession>>>,
}

#[hook]
fn use_terminal_state(app_id: ShortId, signed_in: bool) -> TerminalState {
    let app = use_state(|| Option::<AppEntry>::None);
    let failed = use_state(|| Option::<String>::None);
    let tabs = use_state(initial_tabs);
    let active_tab_id = use_state(|| 1usize);
    let next_tab_id = use_state(|| 2usize);

    let custom_input = use_state(String::new);
    let history = use_state(Vec::<String>::new);
    let history_index = use_state(|| Option::<usize>::None);
    let show_history_modal = use_state(|| false);
    let history_search = use_state(String::new);

    let show_snippets = use_state(|| false);
    let snippet_category = use_state(|| SnippetCategory::All);

    let copied = use_state(|| false);
    let sessions_ref = use_mut_ref(HashMap::<usize, TerminalSession>::new);

    use_app_fetch(app_id, signed_in, &app, &failed, &tabs);
    use_tab_sessions_lifecycle(app_id, &app, &tabs, &active_tab_id, &sessions_ref);
    use_cleanup_on_unmount(&sessions_ref);

    TerminalState {
        app,
        failed,
        tabs,
        active_tab_id,
        next_tab_id,
        custom_input,
        history,
        history_index,
        show_history_modal,
        history_search,
        show_snippets,
        snippet_category,
        copied,
        sessions_ref,
    }
}

#[function_component(AppTerminal)]
pub fn app_terminal(props: &Props) -> Html {
    use_ensure_auth();
    let locale = use_locale();
    let (session, _) = use_store::<SessionStore>();
    let signed_in = session.info.is_some();
    let state = use_terminal_state(props.id, signed_in);

    if !signed_in {
        return html!();
    }
    render_terminal_page(locale, props.id, &state)
}

fn render_terminal_page(locale: Locale, app_id: ShortId, state: &TerminalState) -> Html {
    let title = terminal_title(locale, (*state.app).as_ref());
    let active_tab_id = state.active_tab_id.clone();
    let on_select_tab = Callback::from(move |id: usize| active_tab_id.set(id));
    let on_close_tab =
        make_close_tab_action(&state.tabs, &state.active_tab_id, &state.sessions_ref);
    let on_new_tab = make_new_tab_action(&state.tabs, &state.active_tab_id, &state.next_tab_id);

    let on_submit = make_submit_action(
        &state.custom_input,
        &state.active_tab_id,
        &state.sessions_ref,
        &state.history,
        &state.history_index,
    );
    let on_run_snippet =
        make_run_snippet_action(&state.active_tab_id, &state.sessions_ref, &state.history);
    let on_insert_snippet = make_insert_snippet_action(&state.custom_input);

    let on_copy = copy_action(&state.active_tab_id, &state.sessions_ref, &state.copied);
    let on_clear = clear_action(&state.active_tab_id, &state.sessions_ref);
    let toolbar_actions = ToolbarActions {
        on_submit,
        on_clear,
        on_copy,
    };

    html! {
        <>
            <div class="flex items-center justify-between mb-3">
                <h1 class="text-xl font-semibold">{title}</h1>
                <Link<Route> to={Route::AppDashboard { id: app_id }} classes="text-sm text-neutral-400 hover:text-neutral-200">
                    {locale.t("common.back")}
                </Link<Route>>
            </div>
            { status_view(locale, &state.failed, &state.app) }
            { tab_bar_view(locale, &state.tabs, *state.active_tab_id, (*state.app).as_ref(), on_select_tab, on_close_tab, on_new_tab) }
            { toolbar_view(locale, state, &toolbar_actions) }
            { render_optional_snippet_panel(locale, state, on_run_snippet, on_insert_snippet) }
            { render_tab_screens(&state.tabs, *state.active_tab_id) }
            { render_optional_history_modal(locale, state) }
        </>
    }
}

fn render_tab_screens(tabs: &[TerminalTab], active_tab_id: usize) -> Html {
    html! {
        <div class="relative w-full h-[70vh] rounded-b bg-black overflow-hidden border border-neutral-800">
            { for tabs.iter().map(|tab| {
                let is_active = tab.id == active_tab_id;
                let tab_id = tab.id;
                html! {
                    <div
                        key={tab_id}
                        id={format!("xterm-tab-container-{}", tab_id)}
                        class={if is_active { "w-full h-full" } else { "hidden" }}
                    />
                }
            })}
        </div>
    }
}

fn render_optional_snippet_panel(
    locale: Locale,
    state: &TerminalState,
    on_run: Callback<&'static str>,
    on_insert: Callback<&'static str>,
) -> Html {
    if !*state.show_snippets {
        return html!();
    }
    snippet_panel_view(
        locale,
        (*state.app).as_ref(),
        &state.snippet_category,
        on_run,
        on_insert,
    )
}

fn render_optional_history_modal(locale: Locale, state: &TerminalState) -> Html {
    if !*state.show_history_modal {
        return html!();
    }
    let show_history_modal = state.show_history_modal.clone();
    let on_close_history = Callback::from(move |()| show_history_modal.set(false));
    let on_run_from_history = make_run_from_history_action(
        &state.active_tab_id,
        &state.sessions_ref,
        &state.show_history_modal,
    );
    let on_insert_from_history =
        make_insert_from_history_action(&state.custom_input, &state.show_history_modal);
    let history = state.history.clone();
    let on_clear_history = Callback::from(move |()| history.set(Vec::new()));

    history_modal_view(
        locale,
        &state.history,
        &state.history_search,
        on_close_history,
        on_run_from_history,
        on_insert_from_history,
        on_clear_history,
    )
}

fn initial_tabs() -> Vec<TerminalTab> {
    let shell = initial_shell();
    let title = format!("Tab 1 ({shell})");
    vec![TerminalTab {
        id: 1,
        title,
        shell,
    }]
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
    tabs: &UseStateHandle<Vec<TerminalTab>>,
) {
    let app = app.clone();
    let failed = failed.clone();
    let tabs = tabs.clone();
    use_effect_with(signed_in, move |&signed_in| {
        if signed_in {
            let (app, failed, tabs) = (app.clone(), failed.clone(), tabs.clone());
            wasm_bindgen_futures::spawn_local(async move {
                match fetch_json::<AppEntry>(
                    Locale::En,
                    Request::get(&format!("{API_ENDPOINT}/apps/{app_id}")),
                )
                .await
                {
                    Ok(entry) => {
                        apply_db_shell_default(&entry, &tabs);
                        app.set(Some(entry));
                    }
                    Err(message) => failed.set(Some(message)),
                }
            });
        }
    });
}

fn apply_db_shell_default(entry: &AppEntry, tabs: &UseStateHandle<Vec<TerminalTab>>) {
    if let Some(ref db) = entry.spec.database {
        let tool = db_tool_name(db);
        let current = (**tabs).clone();
        if current.len() == 1 && current[0].shell == "sh" {
            let updated = vec![TerminalTab {
                id: current[0].id,
                title: format!("Tab 1 ({tool})"),
                shell: tool.to_string(),
            }];
            tabs.set(updated);
        }
    }
}

#[hook]
fn use_tab_sessions_lifecycle(
    app_id: ShortId,
    app: &UseStateHandle<Option<AppEntry>>,
    tabs: &UseStateHandle<Vec<TerminalTab>>,
    active_tab_id: &UseStateHandle<usize>,
    sessions_ref: &Rc<RefCell<HashMap<usize, TerminalSession>>>,
) {
    let tabs_dep = (**tabs).clone();
    let app_dep = (**app).clone();
    let active_id = **active_tab_id;
    let sessions_ref = sessions_ref.clone();

    use_effect_with(
        (app_dep, tabs_dep, active_id),
        move |(app, tabs, active_id)| {
            if let Some(app) = app
                && app.target.to_string() == LOCAL_TARGET
            {
                for tab in tabs.iter() {
                    let has_session = sessions_ref.borrow().contains_key(&tab.id);
                    if !has_session
                        && let Some(session) = start_terminal_for_tab(app_id, tab.id, &tab.shell)
                    {
                        sessions_ref.borrow_mut().insert(tab.id, session);
                    }
                }

                let cur_active = *active_id;
                let sessions_ref_timer = sessions_ref.clone();
                gloo_timers::callback::Timeout::new(50, move || {
                    if let Some(session) = sessions_ref_timer.borrow().get(&cur_active) {
                        session.fit();
                        session.focus();
                    }
                })
                .forget();
            }

            let _ = sessions_ref.clone();
            move || ()
        },
    );
}

#[hook]
fn use_cleanup_on_unmount(sessions_ref: &Rc<RefCell<HashMap<usize, TerminalSession>>>) {
    let sessions_ref = sessions_ref.clone();
    use_effect_with((), move |_| {
        let sessions = sessions_ref.clone();
        move || {
            sessions.borrow_mut().clear();
        }
    });
}

fn tab_bar_view(
    locale: Locale,
    tabs: &[TerminalTab],
    active_tab_id: usize,
    app: Option<&AppEntry>,
    on_select_tab: Callback<usize>,
    on_close_tab: Callback<usize>,
    on_new_tab: Callback<String>,
) -> Html {
    let db_tool = app
        .and_then(|a| a.spec.database.as_deref())
        .map(db_tool_name);

    html! {
        <div class="flex items-center justify-between gap-2 px-2 pt-2 bg-neutral-900 border-x border-t border-neutral-800 rounded-t overflow-x-auto">
            <div class="flex items-center gap-1">
                { for tabs.iter().map(|tab| render_tab_item(tab, active_tab_id, tabs.len(), &on_select_tab, &on_close_tab)) }
            </div>
            { quick_new_tab_buttons(locale, db_tool, &on_new_tab) }
        </div>
    }
}

fn render_tab_item(
    tab: &TerminalTab,
    active_tab_id: usize,
    total_tabs: usize,
    on_select: &Callback<usize>,
    on_close: &Callback<usize>,
) -> Html {
    let is_active = tab.id == active_tab_id;
    let tab_id = tab.id;
    let on_click = {
        let on_select = on_select.clone();
        Callback::from(move |_| on_select.emit(tab_id))
    };
    let on_close_click = {
        let on_close = on_close.clone();
        Callback::from(move |e: MouseEvent| {
            e.stop_propagation();
            on_close.emit(tab_id);
        })
    };

    let base_classes = if is_active {
        "flex items-center gap-2 px-3 py-1.5 text-xs font-semibold rounded-t bg-black text-white border-t-2 border-t-blue-500 border-x border-neutral-800 cursor-pointer transition-colors"
    } else {
        "flex items-center gap-2 px-3 py-1.5 text-xs font-medium rounded-t bg-neutral-800/60 text-neutral-400 hover:text-neutral-200 hover:bg-neutral-800 border-t-2 border-t-transparent border-x border-neutral-800/40 cursor-pointer transition-colors"
    };

    html! {
        <div class={base_classes} onclick={on_click}>
            <svg class="w-3.5 h-3.5 text-neutral-400" fill="none" stroke="currentColor" viewBox="0 0 24 24">
                <path stroke-linecap="round" stroke-linejoin="round" stroke-width="2" d="M8 9l3 3-3 3m5 0h3M5 20h14a2 2 0 002-2V6a2 2 0 00-2-2H5a2 2 0 00-2 2v12a2 2 0 002 2z" />
            </svg>
            <span>{&tab.title}</span>
            if total_tabs > 1 {
                <button
                    type="button"
                    class="ml-1 p-0.5 text-neutral-400 hover:text-red-400 hover:bg-neutral-700/60 rounded transition-colors"
                    onclick={on_close_click}
                    title="Close Tab"
                >
                    <svg class="w-3 h-3" fill="none" stroke="currentColor" viewBox="0 0 24 24">
                        <path stroke-linecap="round" stroke-linejoin="round" stroke-width="2" d="M6 18L18 6M6 6l12 12" />
                    </svg>
                </button>
            }
        </div>
    }
}

fn quick_new_tab_buttons(
    locale: Locale,
    db_tool: Option<&'static str>,
    on_new_tab: &Callback<String>,
) -> Html {
    let shells = ["sh", "bash", "zsh"];
    html! {
        <div class="flex items-center gap-1 pb-1 ml-auto">
            <span class="text-[11px] text-neutral-500 mr-1 hidden sm:inline">{format!("{}:", locale.t("apps.terminal_new_tab"))}</span>
            { for shells.into_iter().map(|s| render_new_tab_chip(s, on_new_tab)) }
            if let Some(tool) = db_tool {
                { render_new_db_chip(tool, on_new_tab) }
            }
        </div>
    }
}

fn render_new_tab_chip(shell: &'static str, on_new_tab: &Callback<String>) -> Html {
    let on_click = {
        let on_new_tab = on_new_tab.clone();
        Callback::from(move |_| on_new_tab.emit(shell.to_string()))
    };
    html! {
        <button
            type="button"
            class="px-2 py-0.5 text-[11px] font-medium rounded bg-neutral-800 hover:bg-neutral-700 text-neutral-300 hover:text-white transition-colors cursor-pointer"
            onclick={on_click}
            title={format!("+ {shell}")}
        >
            {format!("+ {shell}")}
        </button>
    }
}

fn render_new_db_chip(tool: &'static str, on_new_tab: &Callback<String>) -> Html {
    let on_click = {
        let on_new_tab = on_new_tab.clone();
        Callback::from(move |_| on_new_tab.emit(tool.to_string()))
    };
    html! {
        <button
            type="button"
            class="px-2 py-0.5 text-[11px] font-medium rounded bg-amber-950/60 border border-amber-800/60 hover:bg-amber-900/60 text-amber-300 hover:text-amber-200 transition-colors cursor-pointer"
            onclick={on_click}
            title={format!("+ {tool} (DB)")}
        >
            {format!("+ {tool}")}
        </button>
    }
}

struct ToolbarActions {
    pub on_submit: Callback<()>,
    pub on_clear: Callback<MouseEvent>,
    pub on_copy: Callback<MouseEvent>,
}

fn toolbar_view(locale: Locale, state: &TerminalState, actions: &ToolbarActions) -> Html {
    html! {
        <div class="flex flex-wrap items-center justify-between gap-2 p-2 bg-neutral-900 border-x border-b border-neutral-800">
            { render_custom_command_input(locale, &state.custom_input, &state.history, &state.history_index, actions.on_submit.clone()) }
            <div class="flex items-center gap-1.5 ml-auto">
                { render_snippets_toggle(locale, &state.show_snippets) }
                { render_history_button(locale, &state.history, &state.show_history_modal) }
                { render_session_actions(locale, *state.copied, actions.on_clear.clone(), actions.on_copy.clone()) }
            </div>
        </div>
    }
}

fn render_custom_command_input(
    locale: Locale,
    custom_input: &UseStateHandle<String>,
    history: &UseStateHandle<Vec<String>>,
    history_index: &UseStateHandle<Option<usize>>,
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
    let onkeydown = handle_key_down(history, history_index, custom_input, on_submit.clone());
    let onclick = Callback::from(move |_| on_submit.emit(()));

    html! {
        <div class="flex items-center gap-1.5">
            <input
                type="text"
                class="px-2.5 py-1 text-xs bg-neutral-950 border border-neutral-700 rounded text-neutral-200 placeholder-neutral-500 focus:outline-none focus:border-blue-500 w-44 sm:w-64 font-mono"
                placeholder={locale.t("apps.terminal_custom")}
                value={(**custom_input).clone()}
                {oninput}
                {onkeydown}
            />
            <button
                type="button"
                class="px-2.5 py-1 text-xs font-medium rounded bg-blue-600 text-white hover:bg-blue-500 transition-colors cursor-pointer"
                {onclick}
            >
                {locale.t("apps.terminal_run")}
            </button>
            <span class="text-[11px] text-neutral-500 hidden md:inline ml-1">
                {locale.t("apps.terminal_history_hint")}
            </span>
        </div>
    }
}

fn handle_key_down(
    history: &UseStateHandle<Vec<String>>,
    history_index: &UseStateHandle<Option<usize>>,
    custom_input: &UseStateHandle<String>,
    on_submit: Callback<()>,
) -> Callback<KeyboardEvent> {
    let history = history.clone();
    let history_index = history_index.clone();
    let custom_input = custom_input.clone();
    Callback::from(move |e: KeyboardEvent| {
        let key = e.key();
        if key == "Enter" {
            e.prevent_default();
            on_submit.emit(());
        } else if key == "ArrowUp" {
            e.prevent_default();
            navigate_history_up(&history, &history_index, &custom_input);
        } else if key == "ArrowDown" {
            e.prevent_default();
            navigate_history_down(&history, &history_index, &custom_input);
        }
    })
}

fn navigate_history_up(
    history: &UseStateHandle<Vec<String>>,
    history_index: &UseStateHandle<Option<usize>>,
    custom_input: &UseStateHandle<String>,
) {
    let list = &**history;
    if let Some(next_idx) = history_up_index(list.len(), **history_index) {
        history_index.set(Some(next_idx));
        if let Some(cmd) = list.get(next_idx) {
            custom_input.set(cmd.clone());
        }
    }
}

fn navigate_history_down(
    history: &UseStateHandle<Vec<String>>,
    history_index: &UseStateHandle<Option<usize>>,
    custom_input: &UseStateHandle<String>,
) {
    let list = &**history;
    match history_down_index(list.len(), **history_index) {
        Some(next_idx) => {
            history_index.set(Some(next_idx));
            if let Some(cmd) = list.get(next_idx) {
                custom_input.set(cmd.clone());
            }
        }
        None => {
            history_index.set(None);
            custom_input.set(String::new());
        }
    }
}

pub fn history_up_index(len: usize, current: Option<usize>) -> Option<usize> {
    if len == 0 {
        None
    } else {
        Some(match current {
            None => len.saturating_sub(1),
            Some(idx) => idx.saturating_sub(1),
        })
    }
}

pub fn history_down_index(len: usize, current: Option<usize>) -> Option<usize> {
    match current {
        Some(idx) if idx + 1 < len => Some(idx + 1),
        _ => None,
    }
}

fn render_snippets_toggle(locale: Locale, show_snippets: &UseStateHandle<bool>) -> Html {
    let is_open = **show_snippets;
    let classes = if is_open {
        "inline-flex items-center gap-1.5 px-2.5 py-1 text-xs font-semibold rounded bg-amber-600/30 text-amber-300 border border-amber-600/60 transition-colors cursor-pointer"
    } else {
        "inline-flex items-center gap-1.5 px-2.5 py-1 text-xs font-medium rounded bg-neutral-800 text-neutral-300 hover:bg-neutral-700 hover:text-white transition-colors cursor-pointer"
    };
    let on_click = {
        let show_snippets = show_snippets.clone();
        Callback::from(move |_| show_snippets.set(!is_open))
    };
    html! {
        <button type="button" class={classes} onclick={on_click} title={locale.t("apps.terminal_snippets_title")}>
            <svg class="w-3.5 h-3.5 text-amber-400" fill="currentColor" viewBox="0 0 20 20">
                <path fill-rule="evenodd" d="M11.3 1.046A1 1 0 0112 2v5h4a1 1 0 01.82 1.573l-7 10A1 1 0 018 18v-5H4a1 1 0 01-.82-1.573l7-10a1 1 0 011.12-.38z" clip-rule="evenodd" />
            </svg>
            <span>{locale.t("apps.terminal_snippets")}</span>
        </button>
    }
}

fn render_history_button(
    locale: Locale,
    history: &UseStateHandle<Vec<String>>,
    show_history_modal: &UseStateHandle<bool>,
) -> Html {
    let count = history.len();
    let on_click = {
        let show_history_modal = show_history_modal.clone();
        Callback::from(move |_| show_history_modal.set(true))
    };
    html! {
        <button
            type="button"
            class="inline-flex items-center gap-1.5 px-2.5 py-1 text-xs font-medium rounded bg-neutral-800 text-neutral-300 hover:bg-neutral-700 hover:text-white transition-colors cursor-pointer"
            onclick={on_click}
            title={locale.t("apps.terminal_history_title")}
        >
            <svg class="w-3.5 h-3.5 text-cyan-400" fill="none" stroke="currentColor" viewBox="0 0 24 24">
                <path stroke-linecap="round" stroke-linejoin="round" stroke-width="2" d="M12 8v4l3 3m6-3a9 9 0 11-18 0 9 9 0 0118 0z" />
            </svg>
            <span>{locale.t("apps.terminal_history")}</span>
            if count > 0 {
                <span class="px-1.5 py-0.2 text-[10px] font-mono rounded-full bg-neutral-700 text-neutral-200">
                    {count}
                </span>
            }
        </button>
    }
}

fn render_session_actions(
    locale: Locale,
    copied: bool,
    on_clear: Callback<MouseEvent>,
    on_copy: Callback<MouseEvent>,
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
        <>
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
        </>
    }
}

fn snippet_panel_view(
    locale: Locale,
    app: Option<&AppEntry>,
    active_category: &UseStateHandle<SnippetCategory>,
    on_run_snippet: Callback<&'static str>,
    on_insert_snippet: Callback<&'static str>,
) -> Html {
    let all_snippets = all_snippets_for_app(app);
    let category = **active_category;
    let filtered: Vec<SnippetItem> = all_snippets
        .into_iter()
        .filter(|s| category == SnippetCategory::All || s.category == category)
        .collect();

    let has_db = app.and_then(|a| a.spec.database.as_deref()).is_some();

    html! {
        <div class="p-3 mb-2 bg-neutral-900 border border-neutral-800 rounded-lg space-y-2.5">
            <div class="flex flex-wrap items-center justify-between gap-2 border-b border-neutral-800 pb-2">
                <div class="flex items-center gap-2">
                    <span class="text-xs font-semibold text-neutral-200 flex items-center gap-1.5">
                        <svg class="w-4 h-4 text-amber-400" fill="currentColor" viewBox="0 0 20 20">
                            <path fill-rule="evenodd" d="M11.3 1.046A1 1 0 0112 2v5h4a1 1 0 01.82 1.573l-7 10A1 1 0 018 18v-5H4a1 1 0 01-.82-1.573l7-10a1 1 0 011.12-.38z" clip-rule="evenodd" />
                        </svg>
                        {locale.t("apps.terminal_snippets_title")}
                    </span>
                    <span class="text-[11px] text-neutral-500">{locale.t("apps.terminal_snippets_hint")}</span>
                </div>
                { category_pills(locale, active_category, has_db) }
            </div>
            <div class="flex flex-wrap gap-1.5 max-h-36 overflow-y-auto">
                { for filtered.into_iter().map(|s| render_snippet_chip(s, &on_run_snippet, &on_insert_snippet)) }
            </div>
        </div>
    }
}

fn category_pills(
    locale: Locale,
    active_category: &UseStateHandle<SnippetCategory>,
    has_db: bool,
) -> Html {
    let categories = [
        (SnippetCategory::All, locale.t("apps.terminal_snippets_all")),
        (
            SnippetCategory::System,
            locale.t("apps.terminal_snippets_system"),
        ),
        (
            SnippetCategory::Files,
            locale.t("apps.terminal_snippets_files"),
        ),
        (
            SnippetCategory::Network,
            locale.t("apps.terminal_snippets_network"),
        ),
        (SnippetCategory::Env, locale.t("apps.terminal_snippets_env")),
    ];

    html! {
        <div class="flex flex-wrap items-center gap-1 text-xs">
            { for categories.into_iter().map(|(cat, label)| render_category_pill(cat, label, active_category)) }
            if has_db {
                { render_category_pill(SnippetCategory::Db, locale.t("apps.terminal_snippets_db"), active_category) }
            }
        </div>
    }
}

fn render_category_pill(
    category: SnippetCategory,
    label: &str,
    active_category: &UseStateHandle<SnippetCategory>,
) -> Html {
    let is_active = **active_category == category;
    let classes = if is_active {
        "px-2 py-0.5 rounded text-[11px] font-semibold bg-blue-600 text-white cursor-pointer"
    } else {
        "px-2 py-0.5 rounded text-[11px] font-medium bg-neutral-800 text-neutral-400 hover:text-neutral-200 cursor-pointer"
    };
    let on_click = {
        let active_category = active_category.clone();
        Callback::from(move |_| active_category.set(category))
    };
    html! {
        <button type="button" class={classes} onclick={on_click}>
            {label}
        </button>
    }
}

fn render_snippet_chip(
    snippet: SnippetItem,
    on_run: &Callback<&'static str>,
    on_insert: &Callback<&'static str>,
) -> Html {
    let cmd = snippet.command;
    let on_run_click = {
        let on_run = on_run.clone();
        Callback::from(move |_| on_run.emit(cmd))
    };
    let on_insert_click = {
        let on_insert = on_insert.clone();
        Callback::from(move |_| on_insert.emit(cmd))
    };

    html! {
        <div class="inline-flex items-center rounded bg-neutral-800/80 border border-neutral-700/60 overflow-hidden text-xs">
            <button
                type="button"
                class="px-2 py-1 text-neutral-200 hover:text-white hover:bg-neutral-700/80 font-mono text-[11px] transition-colors cursor-pointer"
                onclick={on_run_click}
                title={snippet.desc}
            >
                {cmd}
            </button>
            <button
                type="button"
                class="px-1.5 py-1 text-neutral-400 hover:text-amber-300 hover:bg-neutral-700 border-l border-neutral-700/60 transition-colors cursor-pointer"
                onclick={on_insert_click}
                title="Insert into input"
            >
                <svg class="w-3 h-3" fill="none" stroke="currentColor" viewBox="0 0 24 24">
                    <path stroke-linecap="round" stroke-linejoin="round" stroke-width="2" d="M19 14l-7 7m0 0l-7-7m7 7V3" />
                </svg>
            </button>
        </div>
    }
}

fn history_modal_view(
    locale: Locale,
    history: &[String],
    search_query: &UseStateHandle<String>,
    on_close: Callback<()>,
    on_run: Callback<String>,
    on_insert: Callback<String>,
    on_clear: Callback<()>,
) -> Html {
    let query = search_query.to_lowercase();
    let filtered: Vec<String> = history
        .iter()
        .filter(|c| query.is_empty() || c.to_lowercase().contains(&query))
        .cloned()
        .collect();

    let on_search_input = {
        let search_query = search_query.clone();
        Callback::from(move |e: InputEvent| {
            if let Some(input) = e.target_dyn_into::<web_sys::HtmlInputElement>() {
                search_query.set(input.value());
            }
        })
    };
    let on_close_click = {
        let on_close = on_close.clone();
        Callback::from(move |_| on_close.emit(()))
    };

    html! {
        <div class="fixed inset-0 z-50 flex items-center justify-center p-4 bg-black/70 backdrop-blur-sm" onclick={on_close_click.clone()}>
            <div class="bg-neutral-900 border border-neutral-700 rounded-2xl shadow-2xl max-w-xl w-full p-5 text-neutral-100 space-y-4" onclick={Callback::from(|e: MouseEvent| e.stop_propagation())}>
                <div class="flex items-center justify-between border-b border-neutral-800 pb-3">
                    <div class="flex items-center gap-2">
                        <span class="p-1 rounded bg-neutral-800 text-neutral-300">
                            <svg class="w-4 h-4 text-cyan-400" fill="none" stroke="currentColor" viewBox="0 0 24 24">
                                <path stroke-linecap="round" stroke-linejoin="round" stroke-width="2" d="M12 8v4l3 3m6-3a9 9 0 11-18 0 9 9 0 0118 0z" />
                            </svg>
                        </span>
                        <h3 class="text-base font-semibold">{locale.t("apps.terminal_history_title")}</h3>
                        <span class="text-xs px-2 py-0.5 rounded-full bg-neutral-800 text-neutral-400 font-mono">
                            {history.len()}
                        </span>
                    </div>
                    <button type="button" class="text-neutral-400 hover:text-white p-1 rounded hover:bg-neutral-800 transition" onclick={on_close_click.clone()}>
                        <svg class="w-4 h-4" fill="none" stroke="currentColor" viewBox="0 0 24 24">
                            <path stroke-linecap="round" stroke-linejoin="round" stroke-width="2" d="M6 18L18 6M6 6l12 12" />
                        </svg>
                    </button>
                </div>
                <input
                    type="text"
                    class="w-full px-3 py-1.5 text-xs bg-neutral-950 border border-neutral-700 rounded-lg text-neutral-200 placeholder-neutral-500 focus:outline-none focus:border-blue-500"
                    placeholder={locale.t("apps.terminal_history_search")}
                    value={(**search_query).clone()}
                    oninput={on_search_input}
                />
                <div class="max-h-72 overflow-y-auto space-y-1.5 pr-1">
                    if filtered.is_empty() {
                        <p class="text-xs text-neutral-500 py-6 text-center">{locale.t("apps.terminal_history_empty")}</p>
                    } else {
                        { for filtered.into_iter().rev().map(|cmd| render_history_row(locale, cmd, &on_run, &on_insert)) }
                    }
                </div>
                <div class="flex items-center justify-between border-t border-neutral-800 pt-3">
                    <button
                        type="button"
                        class="px-2.5 py-1 text-xs text-red-400 hover:text-red-300 hover:bg-red-950/40 border border-red-800/40 rounded transition cursor-pointer"
                        onclick={Callback::from(move |_| on_clear.emit(()))}
                    >
                        {locale.t("apps.terminal_history_clear")}
                    </button>
                    <button
                        type="button"
                        class="px-3 py-1 text-xs bg-neutral-800 text-neutral-200 hover:bg-neutral-700 rounded transition cursor-pointer"
                        onclick={on_close_click}
                    >
                        {locale.t("apps.terminal_history_close")}
                    </button>
                </div>
            </div>
        </div>
    }
}

fn render_history_row(
    locale: Locale,
    cmd: String,
    on_run: &Callback<String>,
    on_insert: &Callback<String>,
) -> Html {
    let cmd_run = cmd.clone();
    let on_run_click = {
        let on_run = on_run.clone();
        Callback::from(move |_| on_run.emit(cmd_run.clone()))
    };
    let cmd_insert = cmd.clone();
    let on_insert_click = {
        let on_insert = on_insert.clone();
        Callback::from(move |_| on_insert.emit(cmd_insert.clone()))
    };
    let cmd_copy = cmd.clone();
    let on_copy_click = Callback::from(move |_| copy_to_clipboard(&cmd_copy));

    html! {
        <div class="flex items-center justify-between gap-2 p-2 rounded bg-neutral-950/80 border border-neutral-800 hover:border-neutral-700 transition">
            <span class="font-mono text-xs text-neutral-200 truncate select-all">{&cmd}</span>
            <div class="flex items-center gap-1 shrink-0">
                <button
                    type="button"
                    class="p-1 rounded text-emerald-400 hover:bg-emerald-950/40 transition cursor-pointer"
                    onclick={on_run_click}
                    title={locale.t("apps.terminal_history_run")}
                >
                    <svg class="w-3.5 h-3.5" fill="none" stroke="currentColor" viewBox="0 0 24 24">
                        <path stroke-linecap="round" stroke-linejoin="round" stroke-width="2" d="M14.752 11.168l-3.197-2.132A1 1 0 0010 9.87v4.263a1 1 0 001.555.832l3.197-2.132a1 1 0 000-1.664z" />
                    </svg>
                </button>
                <button
                    type="button"
                    class="p-1 rounded text-amber-400 hover:bg-amber-950/40 transition cursor-pointer"
                    onclick={on_insert_click}
                    title={locale.t("apps.terminal_history_insert")}
                >
                    <svg class="w-3.5 h-3.5" fill="none" stroke="currentColor" viewBox="0 0 24 24">
                        <path stroke-linecap="round" stroke-linejoin="round" stroke-width="2" d="M19 14l-7 7m0 0l-7-7m7 7V3" />
                    </svg>
                </button>
                <button
                    type="button"
                    class="p-1 rounded text-neutral-400 hover:text-white hover:bg-neutral-800 transition cursor-pointer"
                    onclick={on_copy_click}
                    title={locale.t("apps.terminal_copy")}
                >
                    <svg class="w-3.5 h-3.5" fill="none" stroke="currentColor" viewBox="0 0 24 24">
                        <path stroke-linecap="round" stroke-linejoin="round" stroke-width="2" d="M8 16H6a2 2 0 01-2-2V6a2 2 0 012-2h8a2 2 0 012 2v2m-6 12h8a2 2 0 002-2v-8a2 2 0 00-2-2h-8a2 2 0 00-2 2v8a2 2 0 002 2z" />
                    </svg>
                </button>
            </div>
        </div>
    }
}

fn make_close_tab_action(
    tabs: &UseStateHandle<Vec<TerminalTab>>,
    active_tab_id: &UseStateHandle<usize>,
    sessions_ref: &Rc<RefCell<HashMap<usize, TerminalSession>>>,
) -> Callback<usize> {
    let tabs = tabs.clone();
    let active_tab_id = active_tab_id.clone();
    let sessions_ref = sessions_ref.clone();
    Callback::from(move |tab_id: usize| {
        let current_tabs = (*tabs).clone();
        if current_tabs.len() <= 1 {
            return;
        }
        sessions_ref.borrow_mut().remove(&tab_id);
        let filtered: Vec<TerminalTab> = current_tabs
            .into_iter()
            .filter(|t| t.id != tab_id)
            .collect();
        if *active_tab_id == tab_id
            && let Some(next_tab) = filtered.last()
        {
            active_tab_id.set(next_tab.id);
        }
        tabs.set(filtered);
    })
}

fn make_new_tab_action(
    tabs: &UseStateHandle<Vec<TerminalTab>>,
    active_tab_id: &UseStateHandle<usize>,
    next_tab_id: &UseStateHandle<usize>,
) -> Callback<String> {
    let tabs = tabs.clone();
    let active_tab_id = active_tab_id.clone();
    let next_tab_id = next_tab_id.clone();
    Callback::from(move |shell: String| {
        let new_id = *next_tab_id;
        next_tab_id.set(new_id + 1);
        let title = format!("Tab {} ({shell})", new_id);
        let mut list = (*tabs).clone();
        list.push(TerminalTab {
            id: new_id,
            title,
            shell,
        });
        tabs.set(list);
        active_tab_id.set(new_id);
    })
}

fn make_submit_action(
    custom_input: &UseStateHandle<String>,
    active_tab_id: &UseStateHandle<usize>,
    sessions_ref: &Rc<RefCell<HashMap<usize, TerminalSession>>>,
    history: &UseStateHandle<Vec<String>>,
    history_index: &UseStateHandle<Option<usize>>,
) -> Callback<()> {
    let custom_input = custom_input.clone();
    let active_tab_id = active_tab_id.clone();
    let sessions_ref = sessions_ref.clone();
    let history = history.clone();
    let history_index = history_index.clone();
    Callback::from(move |()| {
        let cmd = custom_input.trim();
        if !cmd.is_empty() {
            let active_id = *active_tab_id;
            if let Some(session) = sessions_ref.borrow().get(&active_id) {
                let _ = session.send_input(&format!("{cmd}\n"));
                session.focus();
            }
            push_history(&history, cmd);
            custom_input.set(String::new());
            history_index.set(None);
        }
    })
}

fn make_run_snippet_action(
    active_tab_id: &UseStateHandle<usize>,
    sessions_ref: &Rc<RefCell<HashMap<usize, TerminalSession>>>,
    history: &UseStateHandle<Vec<String>>,
) -> Callback<&'static str> {
    let active_tab_id = active_tab_id.clone();
    let sessions_ref = sessions_ref.clone();
    let history = history.clone();
    Callback::from(move |cmd: &'static str| {
        let active_id = *active_tab_id;
        if let Some(session) = sessions_ref.borrow().get(&active_id) {
            let _ = session.send_input(&format!("{cmd}\n"));
            session.focus();
        }
        push_history(&history, cmd);
    })
}

fn make_insert_snippet_action(custom_input: &UseStateHandle<String>) -> Callback<&'static str> {
    let custom_input = custom_input.clone();
    Callback::from(move |cmd: &'static str| {
        custom_input.set(cmd.to_string());
    })
}

fn make_run_from_history_action(
    active_tab_id: &UseStateHandle<usize>,
    sessions_ref: &Rc<RefCell<HashMap<usize, TerminalSession>>>,
    show_history_modal: &UseStateHandle<bool>,
) -> Callback<String> {
    let active_tab_id = active_tab_id.clone();
    let sessions_ref = sessions_ref.clone();
    let show_history_modal = show_history_modal.clone();
    Callback::from(move |cmd: String| {
        let active_id = *active_tab_id;
        if let Some(session) = sessions_ref.borrow().get(&active_id) {
            let _ = session.send_input(&format!("{cmd}\n"));
            session.focus();
        }
        show_history_modal.set(false);
    })
}

fn make_insert_from_history_action(
    custom_input: &UseStateHandle<String>,
    show_history_modal: &UseStateHandle<bool>,
) -> Callback<String> {
    let custom_input = custom_input.clone();
    let show_history_modal = show_history_modal.clone();
    Callback::from(move |cmd: String| {
        custom_input.set(cmd);
        show_history_modal.set(false);
    })
}

fn copy_action(
    active_tab_id: &UseStateHandle<usize>,
    sessions_ref: &Rc<RefCell<HashMap<usize, TerminalSession>>>,
    copied: &UseStateHandle<bool>,
) -> Callback<MouseEvent> {
    let active_tab_id = active_tab_id.clone();
    let sessions_ref = sessions_ref.clone();
    let copied = copied.clone();
    Callback::from(move |_| {
        let active_id = *active_tab_id;
        if let Some(session) = sessions_ref.borrow().get(&active_id) {
            let text = session.copy_text();
            copy_to_clipboard(&text);
            copied.set(true);
            let copied = copied.clone();
            gloo_timers::callback::Timeout::new(2000, move || copied.set(false)).forget();
        }
    })
}

fn clear_action(
    active_tab_id: &UseStateHandle<usize>,
    sessions_ref: &Rc<RefCell<HashMap<usize, TerminalSession>>>,
) -> Callback<MouseEvent> {
    let active_tab_id = active_tab_id.clone();
    let sessions_ref = sessions_ref.clone();
    Callback::from(move |_| {
        let active_id = *active_tab_id;
        if let Some(session) = sessions_ref.borrow().get(&active_id) {
            session.clear();
        }
    })
}

fn push_history(history: &UseStateHandle<Vec<String>>, cmd: &str) {
    let mut list = (**history).clone();
    if add_to_history(&mut list, cmd) {
        history.set(list);
    }
}

pub fn add_to_history(list: &mut Vec<String>, cmd: &str) -> bool {
    let clean = cmd.trim();
    if clean.is_empty() {
        return false;
    }
    if list.last().map(|s| s.as_str()) != Some(clean) {
        if list.len() >= 100 {
            list.remove(0);
        }
        list.push(clean.to_string());
        true
    } else {
        false
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
    fit: JsValue,
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

    fn send_input(&self, text: &str) -> bool {
        self.socket.send_with_u8_array(text.as_bytes()).is_ok()
    }

    fn focus(&self) {
        call(&self.term, "focus", &[]);
    }

    fn fit(&self) {
        call(&self.fit, "fit", &[]);
        if let Some(size) = size_json(&self.term) {
            let _ = self.socket.send_with_str(&size);
        }
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

    let writer_close = term.clone();
    let on_close = Closure::<dyn FnMut(web_sys::CloseEvent)>::wrap(Box::new(move |_| {
        call(
            &writer_close,
            "writeln",
            &[JsValue::from_str(
                "\r\n\x1b[33m[Session disconnected]\x1b[0m\r\n",
            )],
        );
    }));
    socket.set_onclose(Some(on_close.as_ref().unchecked_ref()));
    on_close.forget();

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

/// Opens the WebSocket and attaches xterm to the tab container element.
fn start_terminal_for_tab(id: ShortId, tab_id: usize, shell: &str) -> Option<TerminalSession> {
    let window = web_sys::window()?;
    let document = window.document()?;
    let element_id = format!("xterm-tab-container-{}", tab_id);
    let element = document.get_element_by_id(&element_id)?;
    element.set_inner_html("");

    let term = construct(&window, "Terminal")?;
    let fit = construct(&window, "FitAddon")?;
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
        fit,
        resize_cb,
    })
}

/// The resize message that the admin endpoint understands.
fn size_json(term: &JsValue) -> Option<String> {
    let cols = field(term, "cols")?;
    let rows = field(term, "rows")?;
    if cols >= 2 && rows >= 2 {
        Some(format!("{{\"cols\":{cols},\"rows\":{rows}}}"))
    } else {
        None
    }
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_db_tool_name() {
        assert_eq!(db_tool_name("postgres"), "psql");
        assert_eq!(db_tool_name("mysql"), "mysql");
        assert_eq!(db_tool_name("mariadb"), "mariadb");
        assert_eq!(db_tool_name("redis"), "redis-cli");
        assert_eq!(db_tool_name("mongodb"), "mongosh");
        assert_eq!(db_tool_name("other"), "cli");
    }

    #[test]
    fn test_get_db_snippets() {
        let pg = get_db_snippets("postgres");
        assert!(!pg.is_empty());
        assert!(pg.iter().any(|s| s.command == "\\l"));

        let my = get_db_snippets("mysql");
        assert!(!my.is_empty());
        assert!(my.iter().any(|s| s.command == "SHOW DATABASES;"));

        let redis = get_db_snippets("redis");
        assert!(!redis.is_empty());
        assert!(redis.iter().any(|s| s.command == "PING"));

        let mongo = get_db_snippets("mongodb");
        assert!(!mongo.is_empty());
        assert!(mongo.iter().any(|s| s.command == "show dbs"));

        let none = get_db_snippets("unknown");
        assert!(none.is_empty());
    }

    #[test]
    fn test_all_snippets_without_db() {
        let snippets = all_snippets_for_app(None);
        assert!(!snippets.is_empty());
        assert!(snippets.iter().any(|s| s.command == "ps aux"));
        assert!(snippets.iter().any(|s| s.command == "ls -lah"));
        assert!(snippets.iter().any(|s| s.command == "env | sort"));
        assert!(!snippets.iter().any(|s| s.category == SnippetCategory::Db));
    }

    #[test]
    fn test_add_to_history() {
        let mut list = Vec::new();
        assert!(!add_to_history(&mut list, "   "));
        assert!(list.is_empty());

        assert!(add_to_history(&mut list, "ls -la"));
        assert_eq!(list, vec!["ls -la"]);

        // Duplicate of last item is ignored
        assert!(!add_to_history(&mut list, "ls -la"));
        assert_eq!(list, vec!["ls -la"]);

        // Different item is added
        assert!(add_to_history(&mut list, "pwd"));
        assert_eq!(list, vec!["ls -la", "pwd"]);

        // Max 100 capacity
        let mut full: Vec<String> = (0..100).map(|i| format!("cmd_{i}")).collect();
        assert!(add_to_history(&mut full, "cmd_100"));
        assert_eq!(full.len(), 100);
        assert_eq!(full[0], "cmd_1");
        assert_eq!(full[99], "cmd_100");
    }

    #[test]
    fn test_history_navigation_up_and_down() {
        assert_eq!(history_up_index(0, None), None);
        assert_eq!(history_up_index(3, None), Some(2));
        assert_eq!(history_up_index(3, Some(2)), Some(1));
        assert_eq!(history_up_index(3, Some(1)), Some(0));
        assert_eq!(history_up_index(3, Some(0)), Some(0));

        assert_eq!(history_down_index(3, None), None);
        assert_eq!(history_down_index(3, Some(0)), Some(1));
        assert_eq!(history_down_index(3, Some(1)), Some(2));
        assert_eq!(history_down_index(3, Some(2)), None);
    }
}
