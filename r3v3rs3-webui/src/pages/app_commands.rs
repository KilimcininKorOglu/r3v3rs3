//! Scheduled commands of an app. Shows configured scheduled commands, adds new ones,
//! and triggers immediate runs.

use super::accounts::{HINT_CLASS, INPUT_CLASS};
use super::resource_page::{FormCard, add_button, failed_view, form_modal};
use super::settings::{fetch_json, send_json, send_request};
use crate::API_ENDPOINT;
use crate::auth::use_ensure_auth;
use crate::components::action_button::{ActionIconButton, ActionVariant};
use crate::components::button::{Button, ButtonKind};
use crate::components::data_list::{Column, ListContent, ListOptions, Row, list_view};
use crate::components::modal::{Modal, ModalSize};
use crate::dialog::{self, Icon};
use crate::i18n::use_locale;
use crate::pages::resource_page::Notice;
use crate::store::SessionStore;
use gloo_net::http::Request;
use r3v3rs3_api::i18n::Locale;
use r3v3rs3_api::id::ShortId;
use r3v3rs3_api::platform::{
    AppEntry, AppScheduledCommand, ScheduledCommandRequest, ScheduledCommandRunResult,
};
use wasm_bindgen_futures::spawn_local;
use web_sys::{HtmlInputElement, MouseEvent};
use yew::prelude::*;
use yewdux::prelude::*;

const COLUMNS: [Column; 4] = [
    Column {
        label: "apps.command_name",
        class: "font-medium",
    },
    Column {
        label: "apps.command_schedule",
        class: "font-mono text-xs text-primary-600 dark:text-primary-400 whitespace-nowrap",
    },
    Column {
        label: "apps.command_cmd",
        class: "font-mono text-xs break-all",
    },
    Column {
        label: "apps.command_last_run",
        class: "text-xs whitespace-nowrap",
    },
];

/// The command the modal of the form adds or changes.
#[derive(Clone, PartialEq)]
enum Editor {
    New,
    Existing(AppScheduledCommand),
}

#[derive(Properties, PartialEq)]
pub struct Props {
    pub id: ShortId,
    pub locale: Locale,
}

#[function_component(AppCommands)]
pub fn app_commands(props: &Props) -> Html {
    let locale = props.locale;
    let id = props.id;
    let commands = use_state(|| Option::<Vec<AppScheduledCommand>>::None);
    let running_id = use_state(|| Option::<ShortId>::None);
    let active_output = use_state(|| Option::<(String, String)>::None);
    let editor = use_state(|| Option::<Editor>::None);

    {
        let commands = commands.clone();
        use_effect_with(id, move |&id| {
            load_commands(locale, id, commands);
        });
    }

    let open_new = {
        let editor = editor.clone();
        Callback::from(move |_: MouseEvent| editor.set(Some(Editor::New)))
    };
    let on_close_editor = {
        let editor = editor.clone();
        Callback::from(move |()| editor.set(None))
    };
    let on_saved = {
        let commands = commands.clone();
        Callback::from(move |()| load_commands(locale, id, commands.clone()))
    };
    let ctx = RowContext {
        locale,
        app_id: id,
        commands: &commands,
        running_id: &running_id,
        active_output: &active_output,
        editor: &editor,
    };

    html! {
        <div class="bg-white dark:bg-neutral-800 shadow-sm border border-neutral-300 dark:border-neutral-700 rounded-md p-4 sm:p-6">
            <div class="mb-4">
                <h3 class="text-base font-semibold text-neutral-900 dark:text-neutral-100">
                    {locale.t("apps.commands_title")}
                </h3>
                <p class={HINT_CLASS}>{locale.t("apps.commands_desc")}</p>
            </div>
            { render_commands_list(&ctx) }
            { add_button(locale, open_new) }
            if let Some((name, output)) = (*active_output).clone() {
                { render_output_modal(locale, &name, &output, &active_output) }
            }
            if let Some(editor) = (*editor).clone() {
                <CommandModal app_id={id} {locale} {editor} on_close={on_close_editor} {on_saved} />
            }
        </div>
    }
}

#[derive(Properties, PartialEq)]
pub struct PageProps {
    pub id: ShortId,
}

fn load_app(
    app_id: ShortId,
    app: &UseStateHandle<Option<AppEntry>>,
    failed: &UseStateHandle<Option<String>>,
) {
    let (app, failed) = (app.clone(), failed.clone());
    spawn_local(async move {
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

#[function_component(AppCommandsPage)]
pub fn app_commands_page(props: &PageProps) -> Html {
    use_ensure_auth();
    let locale = use_locale();
    let (session, _) = use_store::<SessionStore>();
    let app = use_state(|| Option::<AppEntry>::None);
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

    if !signed_in {
        return html!();
    }

    html! {
        <div class="space-y-4">
            if let Some(a) = (*app).as_ref() {
                <crate::components::app_tabs::AppTabs
                    id={a.id}
                    name={a.name.to_string()}
                    active={crate::components::app_tabs::AppTab::Commands}
                    domains={a.spec.domains.clone()}
                    is_database={a.spec.database.is_some()}
                />
            }
            if let Some(err) = (*failed).as_ref() {
                <div class="p-4 bg-red-50 dark:bg-red-900/30 text-red-600 dark:text-red-400 rounded-lg text-sm">
                    {err}
                </div>
            }
            <AppCommands id={app_id} {locale} />
        </div>
    }
}

/// The values and handles that every row of the list uses.
struct RowContext<'a> {
    locale: Locale,
    app_id: ShortId,
    commands: &'a UseStateHandle<Option<Vec<AppScheduledCommand>>>,
    running_id: &'a UseStateHandle<Option<ShortId>>,
    active_output: &'a UseStateHandle<Option<(String, String)>>,
    editor: &'a UseStateHandle<Option<Editor>>,
}

fn render_commands_list(ctx: &RowContext<'_>) -> Html {
    let list = (**ctx.commands).as_ref();
    let rows = list
        .map(|list| {
            list.iter()
                .map(|cmd| render_command_row(ctx, cmd))
                .collect::<Vec<_>>()
        })
        .unwrap_or_default();
    let content = ListContent {
        loaded: list.is_some(),
        empty: "apps.commands_empty",
        columns: &COLUMNS,
        rows: &rows,
    };
    let options = ListOptions {
        embedded: true,
        ..Default::default()
    };
    html! {
        <div class="rounded-md border border-neutral-300 dark:border-neutral-700">
            { list_view(ctx.locale, content, None, &options) }
        </div>
    }
}

fn render_command_status_badge(locale: Locale, last_status: Option<&String>) -> Html {
    let Some(status) = last_status else {
        return html! {};
    };
    let is_ok = status == "ok";
    let color = if is_ok {
        "bg-green-100 dark:bg-green-900/30 text-green-700 dark:text-green-300"
    } else {
        "bg-red-100 dark:bg-red-900/30 text-red-700 dark:text-red-300"
    };
    let label = if is_ok {
        locale.t("apps.command_status_success")
    } else {
        locale.t("apps.command_status_failed")
    };
    html! {
        <span class={classes!("px-1.5", "py-0.5", "rounded", "text-xs", "font-medium", color)}>
            {label}
        </span>
    }
}

fn render_command_row(ctx: &RowContext<'_>, cmd: &AppScheduledCommand) -> Row {
    let locale = ctx.locale;
    let is_running = **ctx.running_id == Some(cmd.id);
    let run_onclick =
        run_command_callback(locale, ctx.app_id, cmd.id, ctx.running_id, ctx.commands);
    let delete_onclick = delete_command_callback(locale, ctx.app_id, cmd, ctx.commands);

    let view_output = {
        let active = ctx.active_output.clone();
        let name = cmd.name.clone();
        let output = cmd.last_output.clone().unwrap_or_default();
        Callback::from(move |_: MouseEvent| {
            active.set(Some((name.clone(), output.clone())));
        })
    };

    let edit_onclick = {
        let editor = ctx.editor.clone();
        let cmd = cmd.clone();
        Callback::from(move |_: MouseEvent| {
            editor.set(Some(Editor::Existing(cmd.clone())));
        })
    };

    let status_badge = render_command_status_badge(locale, cmd.last_status.as_ref());

    Row {
        key: cmd.id.to_string(),
        cells: vec![
            html! {
                <div class="flex items-center gap-1.5">
                    <span>{&cmd.name}</span>
                    if !cmd.enabled {
                        <span class="text-xs text-neutral-500 dark:text-neutral-400">{"("}{locale.t("apps.command_disabled")}{")"}</span>
                    }
                </div>
            },
            html! { {&cmd.schedule} },
            html! { {&cmd.command} },
            html! {
                <div class="flex items-center gap-1.5">
                    <span>{format_command_last_run(locale, cmd.last_run_at)}</span>
                    {status_badge}
                </div>
            },
        ],
        actions: html! {
            <>
                if cmd.last_output.is_some() {
                    <ActionIconButton
                        variant={ActionVariant::Terminal}
                        title={locale.t("apps.command_output").to_string()}
                        onclick={view_output}
                    />
                }
                <ActionIconButton
                    variant={ActionVariant::Play}
                    title={locale.t("apps.command_run_now").to_string()}
                    disabled={is_running}
                    onclick={run_onclick}
                />
                <ActionIconButton
                    variant={ActionVariant::Edit}
                    title={locale.t("apps.command_edit").to_string()}
                    onclick={edit_onclick}
                />
                <ActionIconButton
                    variant={ActionVariant::Delete}
                    title={locale.t("common.delete").to_string()}
                    onclick={delete_onclick}
                />
            </>
        },
    }
}

#[derive(Properties, PartialEq, Clone)]
struct CommandModalProps {
    app_id: ShortId,
    locale: Locale,
    editor: Editor,
    on_close: Callback<()>,
    on_saved: Callback<()>,
}

/// The values of the form, and the field handles that change them.
#[derive(Clone)]
struct CommandFields {
    name: UseStateHandle<String>,
    schedule: UseStateHandle<String>,
    command: UseStateHandle<String>,
    enabled: UseStateHandle<bool>,
}

impl CommandFields {
    fn request(&self) -> ScheduledCommandRequest {
        ScheduledCommandRequest {
            name: (*self.name).clone(),
            schedule: (*self.schedule).clone(),
            command: (*self.command).clone(),
            enabled: *self.enabled,
        }
    }
}

#[hook]
fn use_command_fields(cmd: Option<&AppScheduledCommand>) -> CommandFields {
    let text = |select: fn(&AppScheduledCommand) -> &String| {
        cmd.map(|cmd| select(cmd).clone()).unwrap_or_default()
    };
    let (name, schedule, command) = (
        text(|cmd| &cmd.name),
        text(|cmd| &cmd.schedule),
        text(|cmd| &cmd.command),
    );
    let enabled = cmd.is_none_or(|cmd| cmd.enabled);
    CommandFields {
        name: use_state(move || name),
        schedule: use_state(move || schedule),
        command: use_state(move || command),
        enabled: use_state(move || enabled),
    }
}

/// Adds a new command, or changes an existing one, in a modal.
#[function_component(CommandModal)]
fn command_modal(props: &CommandModalProps) -> Html {
    let locale = props.locale;
    let existing = match &props.editor {
        Editor::Existing(cmd) => Some(cmd),
        Editor::New => None,
    };
    let fields = use_command_fields(existing);
    let saving = use_state(|| false);
    let notice = use_state(|| Option::<Notice<()>>::None);

    let path = match existing {
        Some(cmd) => format!("{API_ENDPOINT}/apps/{}/commands/{}", props.app_id, cmd.id),
        None => format!("{API_ENDPOINT}/apps/{}/commands", props.app_id),
    };
    let onsubmit = {
        let (fields, saving, notice) = (fields.clone(), saving.clone(), notice.clone());
        let (on_saved, on_close) = (props.on_saved.clone(), props.on_close.clone());
        let is_update = existing.is_some();
        Callback::from(move |event: SubmitEvent| {
            event.prevent_default();
            let body = fields.request();
            let request = if is_update {
                Request::put(&path)
            } else {
                Request::post(&path)
            };
            saving.set(true);
            let (saving, notice) = (saving.clone(), notice.clone());
            let (on_saved, on_close) = (on_saved.clone(), on_close.clone());
            spawn_local(async move {
                let result = send_json(locale, request, &body).await;
                saving.set(false);
                if let Err(message) = result {
                    notice.set(Some(Notice::Failed(message)));
                    return;
                }
                on_saved.emit(());
                on_close.emit(());
                if is_update {
                    dialog::message(locale, locale.t("apps.command_updated"), Icon::Success).await;
                }
            });
        })
    };
    let (title, save_label) = if existing.is_some() {
        (locale.t("apps.command_edit"), "apps.command_save")
    } else {
        (locale.t("apps.commands_add"), "apps.command_save")
    };
    let card = FormCard {
        title: title.to_string(),
        notice: failed_view(&notice),
        fields: command_fields_view(locale, &fields),
        error: None,
        cancel: None,
        save_label,
        can_save: !*saving,
    };
    form_modal(locale, card, onsubmit, props.on_close.clone())
}

fn command_fields_view(locale: Locale, fields: &CommandFields) -> Html {
    let on_toggle_enabled = {
        let enabled = fields.enabled.clone();
        Callback::from(move |_: Event| enabled.set(!*enabled))
    };
    html! {
        <div class="space-y-3 mt-3">
            <div class="grid grid-cols-1 sm:grid-cols-2 gap-3">
                { text_field(locale.t("apps.command_name"), None, &fields.name) }
                { text_field(locale.t("apps.command_schedule"), Some(locale.t("apps.command_schedule_hint")), &fields.schedule) }
            </div>
            { text_field(locale.t("apps.command_cmd"), Some(locale.t("apps.command_cmd_placeholder")), &fields.command) }
            <label class="inline-flex items-center gap-2 text-xs text-neutral-700 dark:text-neutral-300 cursor-pointer">
                <input
                    type="checkbox"
                    checked={*fields.enabled}
                    onchange={on_toggle_enabled}
                    class="rounded border-neutral-300 dark:border-neutral-700 text-primary-600 focus:ring-primary-500"
                />
                {locale.t("apps.command_enabled")}
            </label>
        </div>
    }
}

fn text_field(label: &str, placeholder: Option<&str>, state: &UseStateHandle<String>) -> Html {
    html! {
        <div>
            <label class="block text-xs font-medium text-neutral-700 dark:text-neutral-300 mb-1">
                {label}
            </label>
            <input
                type="text"
                required=true
                placeholder={placeholder.map(str::to_string)}
                value={(**state).clone()}
                oninput={input_handler(state)}
                class={INPUT_CLASS}
            />
        </div>
    }
}

fn render_output_modal(
    locale: Locale,
    name: &str,
    output: &str,
    active_output: &UseStateHandle<Option<(String, String)>>,
) -> Html {
    let on_close = {
        let active = active_output.clone();
        Callback::from(move |()| active.set(None))
    };
    let close = {
        let on_close = on_close.clone();
        Callback::from(move |_: MouseEvent| on_close.emit(()))
    };
    let footer = html! {
        <Button kind={ButtonKind::Secondary} onclick={close}>{locale.t("apps.command_close")}</Button>
    };
    html! {
        <Modal title={name.to_string()} {on_close} size={ModalSize::Lg} {footer}>
            <pre class="overflow-auto max-h-80 p-3 bg-neutral-100 text-neutral-900 dark:bg-neutral-900 dark:text-neutral-100 font-mono text-xs rounded break-words whitespace-pre-wrap">
                {if output.is_empty() { "No output." } else { output }}
            </pre>
        </Modal>
    }
}

fn input_handler(state: &UseStateHandle<String>) -> Callback<InputEvent> {
    let state = state.clone();
    Callback::from(move |event: InputEvent| {
        let input: HtmlInputElement = event.target_unchecked_into();
        state.set(input.value());
    })
}

fn load_commands(
    locale: Locale,
    id: ShortId,
    commands: UseStateHandle<Option<Vec<AppScheduledCommand>>>,
) {
    spawn_local(async move {
        let path = format!("{API_ENDPOINT}/apps/{id}/commands");
        match fetch_json::<Vec<AppScheduledCommand>>(locale, Request::get(&path)).await {
            Ok(list) => commands.set(Some(list)),
            Err(message) => dialog::message(locale, &message, Icon::Error).await,
        }
    });
}

fn run_command_callback(
    locale: Locale,
    app_id: ShortId,
    cmd_id: ShortId,
    running_id: &UseStateHandle<Option<ShortId>>,
    commands: &UseStateHandle<Option<Vec<AppScheduledCommand>>>,
) -> Callback<MouseEvent> {
    let (running_id, commands) = (running_id.clone(), commands.clone());
    Callback::from(move |_: MouseEvent| {
        let (running_id, commands) = (running_id.clone(), commands.clone());
        running_id.set(Some(cmd_id));
        spawn_local(async move {
            let path = format!("{API_ENDPOINT}/apps/{app_id}/commands/{cmd_id}/run");
            let result: Result<ScheduledCommandRunResult, _> =
                fetch_json(locale, Request::post(&path)).await;
            running_id.set(None);
            match result {
                Ok(res) if res.success => {
                    dialog::message(locale, locale.t("apps.command_run_success"), Icon::Success)
                        .await;
                }
                Ok(res) => {
                    let msg = locale.tf("apps.command_run_failed", &[("error", &res.status)]);
                    dialog::message(locale, &msg, Icon::Error).await;
                }
                Err(err) => {
                    let msg = locale.tf("apps.command_run_failed", &[("error", &err)]);
                    dialog::message(locale, &msg, Icon::Error).await;
                }
            }
            load_commands(locale, app_id, commands);
        });
    })
}

fn delete_command_callback(
    locale: Locale,
    app_id: ShortId,
    cmd: &AppScheduledCommand,
    commands: &UseStateHandle<Option<Vec<AppScheduledCommand>>>,
) -> Callback<MouseEvent> {
    let (cmd_id, cmd_name) = (cmd.id, cmd.name.clone());
    let commands = commands.clone();
    Callback::from(move |_: MouseEvent| {
        let (commands, cmd_name) = (commands.clone(), cmd_name.clone());
        let question = locale.tf("apps.command_delete_confirm", &[("name", &cmd_name)]);
        dialog::confirm_then(locale, question, async move {
            let path = format!("{API_ENDPOINT}/apps/{app_id}/commands/{cmd_id}");
            let Ok(req) = Request::delete(&path).build() else {
                return;
            };
            match send_request(locale, req).await {
                Ok(_) => {
                    load_commands(locale, app_id, commands);
                }
                Err(err) => {
                    dialog::message(locale, &err, Icon::Error).await;
                }
            }
        });
    })
}

fn format_command_last_run(locale: Locale, ts: Option<i64>) -> String {
    let Some(ts) = ts else {
        return locale.t("apps.command_never").to_string();
    };
    crate::format::format_time(ts as u64)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn format_command_last_run_none_returns_never() {
        assert_eq!(format_command_last_run(Locale::En, None), "Never");
    }
}
