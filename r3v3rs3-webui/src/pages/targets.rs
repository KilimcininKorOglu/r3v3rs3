//! The targets of the deployment platform: the Docker Engine of the server and the servers that
//! run `r3v3rs3 agent`. An admin adds an agent target and gets its one-time enrollment token with
//! the commands that start the agent.

use super::Route;
use super::accounts::{HINT_CLASS, INPUT_CLASS, LABEL_CLASS};
use super::resource_page::{
    FormCard, Notice, delete_on_click, form_card, load, notice_view, reload_callback, save_result,
    submit_callback, text_input,
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
use r3v3rs3_api::platform::{TargetEntry, TargetKind, TargetRequest, TargetToken};
use web_sys::HtmlTextAreaElement;
use yew::prelude::*;
use yew_router::prelude::*;
use yewdux::prelude::*;

const PATH: &str = "/targets";

/// The refresh interval of the list, which shows whether each agent is connected.
const REFRESH_MS: u32 = 10_000;

const INSTALL_SCRIPT: &str =
    "https://raw.githubusercontent.com/KilimcininKorOglu/r3v3rs3/main/install.sh";

/// The image with the docker CLI, which the Compose apps need.
const AGENT_IMAGE: &str = "ghcr.io/kilimcininkoroglu/r3v3rs3:latest-platform";

/// The data directory of an agent in a container. It has the same path on the host, because
/// Compose resolves the bind mounts of a service on the host.
const AGENT_DATA_DIR: &str = "/var/lib/r3v3rs3-agent";

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
            { list_card(locale, targets.is_some(), "targets.empty", &COLUMNS, &rows) }
            if admin {
                { form_view(locale, &form, &notice, &targets) }
            }
        </>
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
    let actions = if admin && target.kind == TargetKind::Agent {
        agent_actions(locale, target, targets, notice)
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
    if !target.enrolled {
        ("targets.status_pending", "bg-yellow-400")
    } else if target.online {
        ("targets.status_online", "bg-green-500")
    } else {
        ("targets.status_offline", "bg-red-500")
    }
}

fn agent_actions(
    locale: Locale,
    target: &TargetEntry,
    targets: &TargetList,
    notice: &TokenNotice,
) -> Html {
    let name = target.name.as_str();
    let path = format!("{PATH}/{}", target.id);
    let question = locale.tf("targets.confirm_delete", &[("name", name)]);
    let delete = delete_on_click(
        locale,
        question,
        path.clone(),
        reload_callback(targets, PATH),
    );
    let question = locale.tf("targets.confirm_new_token", &[("name", name)]);
    let renew = new_token_on_click(locale, question, format!("{path}/token"), notice, targets);
    html! {
        <>
            <a class={LINK_CLASS} onclick={renew}>{locale.t("targets.new_token")}</a>
            <a class={DANGER_LINK_CLASS} onclick={delete}>{locale.t("common.delete")}</a>
        </>
    }
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
            { copy_box(locale.t("targets.docker_command"), &docker_command(&master, &token.token), 5) }
        </>
    }
}

/// A read-only text that a click selects, so that it can be copied.
fn copy_box(label: &str, text: &str, rows: u8) -> Html {
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

fn docker_command(master: &str, token: &str) -> String {
    format!(
        "docker run -d --name r3v3rs3-agent --restart unless-stopped --network host \
         --stop-signal SIGINT -v /var/run/docker.sock:/var/run/docker.sock \
         -v {AGENT_DATA_DIR}:{AGENT_DATA_DIR} --entrypoint /usr/bin/r3v3rs3 {AGENT_IMAGE} \
         agent --master {} --data-dir {AGENT_DATA_DIR} --token {}",
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

#[cfg(test)]
mod tests {
    use super::*;

    fn target(enrolled: bool, online: bool) -> TargetEntry {
        TargetEntry {
            id: "fzn-txd".parse().unwrap(),
            name: "edge".into(),
            kind: TargetKind::Agent,
            last_seen_at: None,
            enrolled,
            online,
            version: None,
        }
    }

    #[test]
    fn every_state_of_a_target_has_a_translated_name() {
        let cases = [
            (target(false, false), "targets.status_pending"),
            (target(true, true), "targets.status_online"),
            (target(true, false), "targets.status_offline"),
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
        let docker = docker_command("[2001:db8::1]:9443", "abc.def");
        assert!(docker.contains(AGENT_IMAGE), "{docker}");
        assert!(
            docker.ends_with(
                "--master [2001:db8::1]:9443 --data-dir /var/lib/r3v3rs3-agent --token abc.def"
            ),
            "{docker}"
        );
        assert_eq!(shell_word("a b;c'd"), r"'a b;c'\''d'");
        assert_eq!(shell_word(""), "''");
    }
}
