//! The targets of the deployment platform: the Docker Engine of the server and the servers that
//! run `r3v3rs3 agent`. An admin adds an agent target and gets its one-time enrollment token with
//! the commands that start the agent.

use super::Route;
use super::accounts::{HINT_CLASS, INPUT_CLASS, LABEL_CLASS};
use super::resource_page::{
    BUTTON_CLASS, FormCard, Notice, delete_on_click, form_card, load, notice_view, reload_callback,
    save_result, submit_callback, text_input,
};
use super::settings::{fetch_json, send_request};
use crate::API_ENDPOINT;
use crate::auth::use_ensure_auth;
use crate::components::data_list::{
    Column, DANGER_LINK_CLASS, LINK_CLASS, Row, list_card, status_badge,
};
use crate::dialog::{self, Icon};
use crate::format::format_time;
use crate::i18n::use_locale;
use crate::store::SessionStore;
use gloo_net::http::Request;
use gloo_timers::callback::Interval;
use r3v3rs3_api::container::AppName;
use r3v3rs3_api::i18n::Locale;
use r3v3rs3_api::platform::{
    PrunePolicy, PruneResult, TargetEntry, TargetKind, TargetRequest, TargetToken,
};
use wasm_bindgen_futures::spawn_local;
use web_sys::{HtmlInputElement, HtmlTextAreaElement};
use yew::prelude::*;
use yew_router::prelude::*;
use yewdux::prelude::*;

const PATH: &str = "/targets";

/// The refresh interval of the list, which shows whether each agent is connected.
const REFRESH_MS: u32 = 10_000;

const INSTALL_SCRIPT: &str =
    "https://raw.githubusercontent.com/KilimcininKorOglu/r3v3rs3/main/install.sh";

const COLUMNS: [Column; 5] = [
    Column {
        label: "common.name",
        class: "whitespace-nowrap",
    },
    Column {
        label: "targets.kind",
        class: "whitespace-nowrap",
    },
    Column {
        label: "targets.status",
        class: "whitespace-nowrap",
    },
    Column {
        label: "targets.last_seen",
        class: "whitespace-nowrap",
    },
    Column {
        label: "targets.version",
        class: "whitespace-nowrap",
    },
];

type TargetList = UseStateHandle<Option<Vec<TargetEntry>>>;
type TokenNotice = UseStateHandle<Option<Notice<TargetToken>>>;

#[derive(Clone, PartialEq, Default)]
struct Form {
    name: String,
}

#[function_component(Targets)]
pub fn targets() -> Html {
    use_ensure_auth();
    let locale = use_locale();
    let (session, _) = use_store::<SessionStore>();
    let targets = use_state(|| Option::<Vec<TargetEntry>>::None);
    let form = use_state(Form::default);
    let notice = use_state(|| Option::<Notice<TargetToken>>::None);

    // The admin API answers 401 until the session loads, so the list waits for it.
    let signed_in = session.info.is_some();
    let targets_cloned = targets.clone();
    use_effect_with(signed_in, move |&signed_in| {
        let interval = signed_in.then(|| {
            let load_now = move || load(targets_cloned.clone(), PATH);
            load_now();
            Interval::new(REFRESH_MS, load_now)
        });
        move || drop(interval)
    });

    if signed_in && !session.can_read_platform() {
        return html! { <Redirect<Route> to={Route::Home} /> };
    }
    let admin = session.is_admin();
    let rows = targets
        .as_deref()
        .unwrap_or_default()
        .iter()
        .map(|target| target_row(locale, target, admin, &targets, &notice))
        .collect::<Vec<_>>();
    let token = notice_view(&notice, |token: &TargetToken| token_view(locale, token));
    html! {
        <>
            { token }
            { upgrade_all_button(locale, &targets, admin) }
            { list_card(locale, targets.is_some(), "targets.empty", &COLUMNS, &rows) }
            if admin {
                { form_view(locale, &form, &notice, &targets) }
                <PruneSettings />
            }
        </>
    }
}

fn upgrade_all_button(locale: Locale, targets: &TargetList, admin: bool) -> Html {
    let has_online_agents = targets
        .as_deref()
        .unwrap_or_default()
        .iter()
        .any(|t| t.kind == TargetKind::Agent && t.online);
    if !admin || !has_online_agents {
        return html! {};
    }
    let question = locale.t("targets.confirm_upgrade_all").to_string();
    let onclick = upgrade_on_click(locale, question, format!("{PATH}/upgrade"), targets);
    html! {
        <div class="mb-4 flex justify-end">
            <button type="button" class={BUTTON_CLASS} {onclick}>
                {locale.t("targets.upgrade_all")}
            </button>
        </div>
    }
}

fn target_row(
    locale: Locale,
    target: &TargetEntry,
    admin: bool,
    targets: &TargetList,
    notice: &TokenNotice,
) -> Row {
    let (status, color) = target_status(target);
    let kind = match target.kind {
        TargetKind::Local => "targets.kind_local",
        TargetKind::Agent => "targets.kind_agent",
    };
    let last_seen = target.last_seen_at.map(format_time).unwrap_or_default();
    let version = target.version.clone().unwrap_or_default();
    let actions = if admin {
        target_actions(locale, target, targets, notice)
    } else {
        html! {}
    };
    Row {
        key: target.id.to_string(),
        cells: vec![
            html! { <>{target.name.clone()}</> },
            html! { <>{locale.t(kind)}</> },
            status_badge(locale.t(status), color),
            html! { <span class="whitespace-nowrap">{last_seen}</span> },
            html! { <span class="font-mono">{version}</span> },
        ],
        actions,
    }
}

/// The translation key and the dot color of the state of a target.
pub(super) fn target_status(target: &TargetEntry) -> (&'static str, &'static str) {
    if target.drained {
        ("targets.status_drained", "bg-purple-500")
    } else if !target.enrolled {
        ("targets.status_pending", "bg-yellow-400")
    } else if target.online {
        ("targets.status_online", "bg-green-500")
    } else {
        ("targets.status_offline", "bg-red-500")
    }
}

fn target_actions(
    locale: Locale,
    target: &TargetEntry,
    targets: &TargetList,
    notice: &TokenNotice,
) -> Html {
    let name = target.name.as_str();
    let path = format!("{PATH}/{}", target.id);
    let drain_button = if target.drained {
        let question = locale.tf("targets.confirm_undrain", &[("name", name)]);
        let onclick = drain_on_click(locale, question, format!("{path}/undrain"), targets);
        html! {
            <a class={LINK_CLASS} {onclick}>{locale.t("targets.undrain")}</a>
        }
    } else {
        let question = locale.tf("targets.confirm_drain", &[("name", name)]);
        let onclick = drain_on_click(locale, question, format!("{path}/drain"), targets);
        html! {
            <a class={LINK_CLASS} {onclick}>{locale.t("targets.drain")}</a>
        }
    };

    if target.kind == TargetKind::Local {
        return drain_button;
    }

    let question = locale.tf("targets.confirm_delete", &[("name", name)]);
    let delete = delete_on_click(
        locale,
        question,
        path.clone(),
        reload_callback(targets, PATH),
    );
    let question = locale.tf("targets.confirm_new_token", &[("name", name)]);
    let renew = new_token_on_click(locale, question, format!("{path}/token"), notice, targets);
    let upgrade = target.online.then(|| {
        let question = locale.tf("targets.confirm_upgrade", &[("name", name)]);
        let onclick = upgrade_on_click(locale, question, format!("{path}/upgrade"), targets);
        html! {
            <a class={LINK_CLASS} {onclick}>{locale.t("targets.upgrade")}</a>
        }
    });
    html! {
        <>
            { drain_button }
            { upgrade }
            <a class={LINK_CLASS} onclick={renew}>{locale.t("targets.new_token")}</a>
            <a class={DANGER_LINK_CLASS} onclick={delete}>{locale.t("common.delete")}</a>
        </>
    }
}

fn drain_on_click(
    locale: Locale,
    question: String,
    path: String,
    targets: &TargetList,
) -> Callback<MouseEvent> {
    let reload = reload_callback(targets, PATH);
    Callback::from(move |event: MouseEvent| {
        event.prevent_default();
        let (path, reload) = (path.clone(), reload.clone());
        dialog::confirm_then(locale, question.clone(), async move {
            let request = Request::post(&format!("{API_ENDPOINT}{path}"));
            match fetch_json::<serde_json::Value>(locale, request).await {
                Ok(_) => {
                    reload.emit(());
                }
                Err(err) => {
                    dialog::message(locale, &err, Icon::Error).await;
                }
            }
        });
    })
}

fn upgrade_on_click(
    locale: Locale,
    question: String,
    path: String,
    targets: &TargetList,
) -> Callback<MouseEvent> {
    let reload = reload_callback(targets, PATH);
    Callback::from(move |event: MouseEvent| {
        event.prevent_default();
        let (path, reload) = (path.clone(), reload.clone());
        dialog::confirm_then(locale, question.clone(), async move {
            let request = Request::post(&format!("{API_ENDPOINT}{path}"));
            match fetch_json::<serde_json::Value>(locale, request).await {
                Ok(_) => {
                    dialog::message(locale, locale.t("targets.upgrade_started"), Icon::Success)
                        .await;
                    reload.emit(());
                }
                Err(err) => {
                    dialog::message(locale, &err, Icon::Error).await;
                }
            }
        });
    })
}

/// Asks `question`, replaces the token of the target at `path` and shows the new token.
fn new_token_on_click(
    locale: Locale,
    question: String,
    path: String,
    notice: &TokenNotice,
    targets: &TargetList,
) -> Callback<MouseEvent> {
    let reload = reload_callback(targets, PATH);
    let notice = notice.clone();
    Callback::from(move |event: MouseEvent| {
        event.prevent_default();
        let (path, notice, reload) = (path.clone(), notice.clone(), reload.clone());
        dialog::confirm_then(locale, question.clone(), async move {
            let request = Request::post(&format!("{API_ENDPOINT}{path}"));
            match fetch_json::<TargetToken>(locale, request).await {
                Ok(token) => {
                    notice.set(Some(Notice::Saved(token)));
                    reload.emit(());
                }
                Err(message) => dialog::message(locale, &message, Icon::Error).await,
            }
        });
    })
}

fn form_view(
    locale: Locale,
    form: &UseStateHandle<Form>,
    notice: &TokenNotice,
    targets: &TargetList,
) -> Html {
    let parsed = form.name.parse::<AppName>();
    let done = save_result(form, notice, reload_callback(targets, PATH));
    let state = form.clone();
    let onsubmit = submit_callback(
        move || state.name.parse::<AppName>().ok(),
        move |name| add(locale, name),
        done,
    );
    let fields = html! {
        <>
            <label class={LABEL_CLASS}>{locale.t("common.name")}</label>
            <input type="text" autocapitalize="off" autocomplete="off" value={form.name.clone()} oninput={text_input(form, |f| &mut f.name)} class={INPUT_CLASS} />
            <p class={HINT_CLASS}>{locale.t("targets.name_hint")}</p>
        </>
    };
    let card = FormCard {
        title: locale.t("targets.add_title").to_string(),
        notice: html! {},
        fields,
        error: parsed
            .as_ref()
            .err()
            .filter(|_| !form.name.is_empty())
            .map(|err| locale.error_message(err)),
        cancel: None,
        save_label: "common.add",
        can_save: parsed.is_ok(),
    };
    form_card(locale, card, onsubmit)
}

async fn add(locale: Locale, name: AppName) -> Result<TargetToken, String> {
    let request = Request::post(&format!("{API_ENDPOINT}{PATH}"))
        .json(&TargetRequest { name })
        .map_err(|err| err.to_string())?;
    send_request(locale, request)
        .await?
        .json()
        .await
        .map_err(|err| err.to_string())
}

/// The new token with the commands that enroll an agent with it.
fn token_view(locale: Locale, token: &TargetToken) -> Html {
    let master = master_address(token);
    let name = token.target.name.as_str();
    html! {
        <>
            <p>{locale.tf("targets.token_created", &[("name", name)])}</p>
            <p class="mt-1">{locale.t("targets.token_once")}</p>
            { copy_box(locale.t("targets.token"), &token.token, 1) }
            { copy_box(locale.t("targets.install_command"), &install_command(&master, &token.token), 3) }
        </>
    }
}

/// A read-only text that a click selects, so that it can be copied.
pub(super) fn copy_box(label: &str, text: &str, rows: u8) -> Html {
    let onclick = Callback::from(|event: MouseEvent| {
        let area: HtmlTextAreaElement = event.target_unchecked_into();
        area.select();
    });
    html! {
        <>
            <label class="block mt-3 mb-1 text-sm font-medium">{label.to_string()}</label>
            <textarea readonly=true rows={rows.to_string()} {onclick} value={text.to_string()} class="w-full p-2 font-mono text-xs rounded border border-green-400 dark:border-green-800 bg-white dark:bg-neutral-900 text-neutral-900 dark:text-neutral-200" />
        </>
    }
}

/// The address that the agent dials: the `agent_host` of the master, or the host of this page.
fn master_address(token: &TargetToken) -> String {
    let host = token.agent_host.clone().unwrap_or_else(|| {
        web_sys::window()
            .and_then(|window| window.location().hostname().ok())
            .unwrap_or_default()
    });
    format!("{host}:{}", token.agent_port)
}

fn install_command(master: &str, token: &str) -> String {
    format!(
        "curl -fsSL {INSTALL_SCRIPT} | sudo bash -s -- --agent --master {} --token {}",
        shell_word(master),
        shell_word(token)
    )
}

/// A value as one shell word. The admin sets `agent_host`, so it can hold any character.
fn shell_word(value: &str) -> String {
    let plain = !value.is_empty()
        && value
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || "._-:[]".contains(c));
    if plain {
        value.to_string()
    } else {
        format!("'{}'", value.replace('\'', r"'\''"))
    }
}

#[function_component(PruneSettings)]
fn prune_settings() -> Html {
    let locale = use_locale();
    let policy = use_state(PrunePolicy::default);
    let loaded = use_state(|| false);
    let notice = use_state(|| Option::<Notice<&'static str>>::None);
    let busy = use_state(|| false);

    let policy_loaded = loaded.clone();
    let policy_state = policy.clone();
    use_effect_with((), move |()| {
        if !*policy_loaded {
            let policy_state = policy_state.clone();
            let policy_loaded = policy_loaded.clone();
            spawn_local(async move {
                let request = Request::get(&format!("{API_ENDPOINT}/platform/prune"));
                if let Ok(data) = fetch_json::<PrunePolicy>(locale, request).await {
                    policy_state.set(data);
                    policy_loaded.set(true);
                }
            });
        }
        || ()
    });

    let run_prune = run_prune_callback(locale, &busy, &policy);
    let save_policy = save_policy_callback(locale, &busy, &policy, &notice);

    html! {
        <div class="mt-6 p-6 bg-white dark:bg-neutral-800 rounded-lg border border-neutral-200 dark:border-neutral-700 shadow-sm">
            <h3 class="text-lg font-semibold text-neutral-900 dark:text-neutral-100">
                {locale.t("targets.prune_title")}
            </h3>
            <p class="text-sm text-neutral-500 dark:text-neutral-400 mt-1 mb-4">
                {locale.t("targets.prune_desc")}
            </p>
            { notice_view(&notice, |key| html! { {locale.t(key)} }) }
            { prune_toggles_view(locale, &policy) }
            { prune_stats_and_actions(locale, &policy, *busy, run_prune, save_policy) }
        </div>
    }
}

fn prune_toggles_view(locale: Locale, policy: &UseStateHandle<PrunePolicy>) -> Html {
    let toggle_enabled = checkbox_toggle(policy, |p| &mut p.enabled);
    let toggle_images = checkbox_toggle(policy, |p| &mut p.prune_images);
    let toggle_containers = checkbox_toggle(policy, |p| &mut p.prune_containers);
    let toggle_build_cache = checkbox_toggle(policy, |p| &mut p.prune_build_cache);
    let toggle_volumes = checkbox_toggle(policy, |p| &mut p.prune_volumes);
    let interval_input = text_input(policy, |p| &mut p.interval);

    html! {
        <div class="space-y-4">
            <label class="flex items-center text-sm font-medium text-neutral-800 dark:text-neutral-200 cursor-pointer">
                <input type="checkbox" checked={policy.enabled} onchange={toggle_enabled} class="w-4 h-4 mr-2 rounded" />
                {locale.t("targets.prune_enabled")}
            </label>
            <div>
                <label class={LABEL_CLASS}>{locale.t("targets.prune_interval")}</label>
                <input type="text" autocapitalize="off" autocomplete="off" value={policy.interval.clone()} oninput={interval_input} class={INPUT_CLASS} />
            </div>
            <div class="grid grid-cols-1 md:grid-cols-2 gap-3 pt-2">
                <label class="flex items-center text-sm font-medium text-neutral-800 dark:text-neutral-200 cursor-pointer">
                    <input type="checkbox" checked={policy.prune_images} onchange={toggle_images} class="w-4 h-4 mr-2 rounded" />
                    {locale.t("targets.prune_images")}
                </label>
                <label class="flex items-center text-sm font-medium text-neutral-800 dark:text-neutral-200 cursor-pointer">
                    <input type="checkbox" checked={policy.prune_containers} onchange={toggle_containers} class="w-4 h-4 mr-2 rounded" />
                    {locale.t("targets.prune_containers")}
                </label>
                <label class="flex items-center text-sm font-medium text-neutral-800 dark:text-neutral-200 cursor-pointer">
                    <input type="checkbox" checked={policy.prune_build_cache} onchange={toggle_build_cache} class="w-4 h-4 mr-2 rounded" />
                    {locale.t("targets.prune_build_cache")}
                </label>
                <label class="flex items-center text-sm font-medium text-neutral-800 dark:text-neutral-200 cursor-pointer">
                    <input type="checkbox" checked={policy.prune_volumes} onchange={toggle_volumes} class="w-4 h-4 mr-2 rounded" />
                    {locale.t("targets.prune_volumes")}
                </label>
            </div>
        </div>
    }
}

fn prune_stats_and_actions(
    locale: Locale,
    policy: &PrunePolicy,
    busy: bool,
    run_prune: Callback<MouseEvent>,
    save_policy: Callback<MouseEvent>,
) -> Html {
    let last_run = match policy.last_pruned_at {
        Some(secs) => format_time(secs * 1000),
        None => locale.t("targets.prune_never").to_string(),
    };
    let reclaimed = match policy.last_reclaimed_bytes {
        Some(bytes) => format_disk_bytes(bytes),
        None => "0 B".to_string(),
    };
    html! {
        <div class="flex flex-col sm:flex-row sm:items-center justify-between gap-4 pt-4 mt-4 border-t border-neutral-200 dark:border-neutral-700">
            <div class="text-xs text-neutral-500 dark:text-neutral-400 space-y-1">
                <div>{locale.tf("targets.prune_last_run", &[("time", &last_run)])}</div>
                <div>{locale.tf("targets.prune_last_reclaimed", &[("size", &reclaimed)])}</div>
            </div>
            <div class="flex gap-2">
                <button type="button" class={BUTTON_CLASS} disabled={busy} onclick={run_prune}>
                    if busy { {locale.t("targets.prune_running")} } else { {locale.t("targets.prune_run_now")} }
                </button>
                <button type="button" class={BUTTON_CLASS} disabled={busy} onclick={save_policy}>
                    {locale.t("targets.prune_save")}
                </button>
            </div>
        </div>
    }
}

fn run_prune_callback(
    locale: Locale,
    busy: &UseStateHandle<bool>,
    policy: &UseStateHandle<PrunePolicy>,
) -> Callback<MouseEvent> {
    let (busy, policy) = (busy.clone(), policy.clone());
    Callback::from(move |event: MouseEvent| {
        event.prevent_default();
        let (busy, policy) = (busy.clone(), policy.clone());
        dialog::confirm_then(
            locale,
            locale.t("targets.prune_confirm").to_string(),
            async move {
                busy.set(true);
                let request = Request::post(&format!("{API_ENDPOINT}/platform/prune/run"));
                handle_prune_result(locale, request, policy).await;
                busy.set(false);
            },
        );
    })
}

async fn handle_prune_result(
    locale: Locale,
    request: gloo_net::http::RequestBuilder,
    policy: UseStateHandle<PrunePolicy>,
) {
    match fetch_json::<PruneResult>(locale, request).await {
        Ok(result) => {
            let freed = format_disk_bytes(result.space_reclaimed_bytes);
            let msg = locale.tf(
                "targets.prune_result",
                &[
                    ("size", &freed),
                    ("images", &result.images_deleted.to_string()),
                    ("volumes", &result.volumes_deleted.to_string()),
                    ("build_cache", &result.build_cache_deleted.to_string()),
                    ("containers", &result.containers_deleted.to_string()),
                ],
            );
            dialog::message(locale, &msg, Icon::Success).await;
            refresh_prune_policy(locale, policy).await;
        }
        Err(err) => dialog::message(locale, &err, Icon::Error).await,
    }
}

async fn refresh_prune_policy(locale: Locale, policy: UseStateHandle<PrunePolicy>) {
    let req = Request::get(&format!("{API_ENDPOINT}/platform/prune"));
    if let Ok(updated) = fetch_json::<PrunePolicy>(locale, req).await {
        policy.set(updated);
    }
}

fn save_policy_callback(
    locale: Locale,
    busy: &UseStateHandle<bool>,
    policy: &UseStateHandle<PrunePolicy>,
    notice: &UseStateHandle<Option<Notice<&'static str>>>,
) -> Callback<MouseEvent> {
    let (busy, policy, notice) = (busy.clone(), policy.clone(), notice.clone());
    Callback::from(move |event: MouseEvent| {
        event.prevent_default();
        let (busy, policy, notice) = (busy.clone(), policy.clone(), notice.clone());
        spawn_local(async move {
            busy.set(true);
            let val = (*policy).clone();
            let req = Request::put(&format!("{API_ENDPOINT}/platform/prune")).json(&val);
            match req {
                Ok(req) => match send_request(locale, req).await {
                    Ok(_) => notice.set(Some(Notice::Saved("targets.prune_saved"))),
                    Err(err) => notice.set(Some(Notice::Failed(err))),
                },
                Err(err) => notice.set(Some(Notice::Failed(err.to_string()))),
            }
            busy.set(false);
        });
    })
}

fn checkbox_toggle(
    policy: &UseStateHandle<PrunePolicy>,
    select: fn(&mut PrunePolicy) -> &mut bool,
) -> Callback<Event> {
    let policy = policy.clone();
    Callback::from(move |event: Event| {
        let input: HtmlInputElement = event.target_unchecked_into();
        let mut updated = (*policy).clone();
        *select(&mut updated) = input.checked();
        policy.set(updated);
    })
}

fn format_disk_bytes(bytes: u64) -> String {
    const GIB: f64 = 1024.0 * 1024.0 * 1024.0;
    const TIB: f64 = 1024.0 * GIB;
    let b = bytes as f64;
    if b >= TIB {
        format!("{:.1} TiB", b / TIB)
    } else if b >= GIB {
        format!("{:.1} GiB", b / GIB)
    } else {
        format!("{:.1} MiB", b / (1024.0 * 1024.0))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn target(enrolled: bool, online: bool, drained: bool) -> TargetEntry {
        TargetEntry {
            id: "fzn-txd".parse().unwrap(),
            name: "edge".into(),
            kind: TargetKind::Agent,
            last_seen_at: None,
            enrolled,
            online,
            drained,
            version: None,
        }
    }

    #[test]
    fn every_state_of_a_target_has_a_translated_name() {
        let cases = [
            (target(false, false, false), "targets.status_pending"),
            (target(true, true, false), "targets.status_online"),
            (target(true, false, false), "targets.status_offline"),
            (target(true, true, true), "targets.status_drained"),
            (target(false, false, true), "targets.status_drained"),
        ];
        for (target, expected) in cases {
            let (key, _) = target_status(&target);
            assert_eq!(key, expected);
            assert_ne!(Locale::En.t(key), key);
            assert_ne!(Locale::Tr.t(key), key);
        }
    }

    #[test]
    fn the_commands_carry_the_master_and_the_token_as_shell_words() {
        let install = install_command("master.example.com:9443", "abc.def");
        assert!(
            install.ends_with("--agent --master master.example.com:9443 --token abc.def"),
            "{install}"
        );
        assert_eq!(shell_word("a b;c'd"), r"'a b;c'\''d'");
        assert_eq!(shell_word(""), "''");
    }

    #[test]
    fn format_disk_bytes_handles_mib_gib_tib() {
        assert_eq!(format_disk_bytes(500 * 1024 * 1024), "500.0 MiB");
        assert_eq!(format_disk_bytes(2 * 1024 * 1024 * 1024), "2.0 GiB");
        assert_eq!(format_disk_bytes(3 * 1024 * 1024 * 1024 * 1024), "3.0 TiB");
    }
}
