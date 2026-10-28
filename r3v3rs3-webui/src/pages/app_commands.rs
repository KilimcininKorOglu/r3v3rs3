//! Scheduled commands of an app. Shows configured scheduled commands, adds new ones,
//! and triggers immediate runs.

use super::accounts::{HINT_CLASS, INPUT_CLASS};
use super::settings::{fetch_json, send_request};
use crate::API_ENDPOINT;
use crate::auth::use_ensure_auth;
use crate::components::action_button::{ActionIconButton, ActionVariant};
use crate::dialog::{self, Icon};
use crate::i18n::use_locale;
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

#[derive(Clone)]
struct CommandFormState {
    name: UseStateHandle<String>,
    schedule: UseStateHandle<String>,
    command: UseStateHandle<String>,
    enabled: UseStateHandle<bool>,
    adding: UseStateHandle<bool>,
}

impl CommandFormState {
    fn reset(&self) {
        self.name.set(String::new());
        self.schedule.set(String::new());
        self.command.set(String::new());
        self.adding.set(false);
    }
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
    let form = CommandFormState {
        name: use_state(String::new),
        schedule: use_state(String::new),
        command: use_state(String::new),
        enabled: use_state(|| true),
        adding: use_state(|| false),
    };
    let running_id = use_state(|| Option::<ShortId>::None);
    let active_output = use_state(|| Option::<(String, String)>::None);
    let editing_cmd = use_state(|| Option::<AppScheduledCommand>::None);

    {
        let commands = commands.clone();
        use_effect_with(id, move |&id| {
            load_commands(locale, id, commands);
        });
    }

    let toggle_adding = {
        let adding = form.adding.clone();
        Callback::from(move |_: MouseEvent| adding.set(!*adding))
    };

    let on_submit = add_command_submit(locale, id, &form, &commands);

    let on_close_edit = {
        let editing_cmd = editing_cmd.clone();
        Callback::from(move |()| editing_cmd.set(None))
    };

    let on_saved_edit = {
        let commands = commands.clone();
        Callback::from(move |()| load_commands(locale, id, commands.clone()))
    };

    html! {
        <div class="bg-white dark:bg-neutral-800 shadow-sm border border-neutral-300 dark:border-neutral-700 rounded-md p-4 sm:p-6">
            { render_commands_header(locale, &form, toggle_adding) }
            if *form.adding {
                { render_add_form(locale, &form, on_submit) }
            }
            { render_commands_list(locale, id, &commands, &running_id, &active_output, &editing_cmd) }
            { render_active_modals(locale, id, &active_output, &editing_cmd, on_close_edit, on_saved_edit) }
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

fn render_commands_header(
    locale: Locale,
    form: &CommandFormState,
    toggle_adding: Callback<MouseEvent>,
) -> Html {
    let button_label = if *form.adding {
        locale.t("common.cancel")
    } else {
        locale.t("apps.commands_add")
    };
    html! {
        <div class="flex flex-col sm:flex-row sm:items-center justify-between gap-2 mb-4">
            <div>
                <h3 class="text-base font-semibold text-neutral-900 dark:text-neutral-100">
                    {locale.t("apps.commands_title")}
                </h3>
                <p class={HINT_CLASS}>{locale.t("apps.commands_desc")}</p>
            </div>
            <button
                onclick={toggle_adding}
                type="button"
                class="self-start sm:self-auto inline-flex items-center text-xs sm:text-sm px-3 py-1.5 rounded-lg border border-neutral-300 dark:border-neutral-700 bg-white dark:bg-neutral-800 hover:bg-neutral-50 dark:hover:bg-neutral-700 font-medium"
            >
                {button_label}
            </button>
        </div>
    }
}

fn render_active_modals(
    locale: Locale,
    id: ShortId,
    active_output: &UseStateHandle<Option<(String, String)>>,
    editing_cmd: &UseStateHandle<Option<AppScheduledCommand>>,
    on_close_edit: Callback<()>,
    on_saved_edit: Callback<()>,
) -> Html {
    html! {
        <>
            if let Some((name, output)) = (**active_output).clone() {
                { render_output_modal(locale, &name, &output, active_output) }
            }
            if let Some(cmd) = (**editing_cmd).clone() {
                <EditCommandModal
                    app_id={id}
                    locale={locale}
                    cmd={cmd}
                    on_close={on_close_edit}
                    on_saved={on_saved_edit}
                />
            }
        </>
    }
}

fn render_add_form(
    locale: Locale,
    form: &CommandFormState,
    on_submit: Callback<SubmitEvent>,
) -> Html {
    let on_name = input_handler(&form.name);
    let on_schedule = input_handler(&form.schedule);
    let on_command = input_handler(&form.command);
    let on_toggle_enabled = {
        let enabled = form.enabled.clone();
        Callback::from(move |_: Event| enabled.set(!*enabled))
    };

    html! {
        <form onsubmit={on_submit} class="p-4 mb-4 bg-neutral-50 dark:bg-neutral-900/50 border border-neutral-200 dark:border-neutral-700 rounded-lg space-y-3">
            <div class="grid grid-cols-1 sm:grid-cols-2 gap-3">
                <div>
                    <label class="block text-xs font-medium text-neutral-700 dark:text-neutral-300 mb-1">
                        {locale.t("apps.command_name")}
                    </label>
                    <input
                        type="text"
                        required=true
                        value={(*form.name).clone()}
                        oninput={on_name}
                        class={INPUT_CLASS}
                    />
                </div>
                <div>
                    <label class="block text-xs font-medium text-neutral-700 dark:text-neutral-300 mb-1">
                        {locale.t("apps.command_schedule")}
                    </label>
                    <input
                        type="text"
                        required=true
                        placeholder={locale.t("apps.command_schedule_hint")}
                        value={(*form.schedule).clone()}
                        oninput={on_schedule}
                        class={INPUT_CLASS}
                    />
                </div>
            </div>
            <div>
                <label class="block text-xs font-medium text-neutral-700 dark:text-neutral-300 mb-1">
                    {locale.t("apps.command_cmd")}
                </label>
                <input
                    type="text"
                    required=true
                    placeholder={locale.t("apps.command_cmd_placeholder")}
                    value={(*form.command).clone()}
                    oninput={on_command}
                    class={INPUT_CLASS}
                />
            </div>
            <div class="flex items-center justify-between pt-1">
                <label class="inline-flex items-center gap-2 text-xs text-neutral-700 dark:text-neutral-300 cursor-pointer">
                    <input
                        type="checkbox"
                        checked={*form.enabled}
                        onchange={on_toggle_enabled}
                        class="rounded border-neutral-300 dark:border-neutral-700 text-blue-600 focus:ring-blue-500"
                    />
                    {locale.t("apps.command_enabled")}
                </label>
                <button
                    type="submit"
                    class="text-xs sm:text-sm px-4 py-1.5 rounded-lg bg-blue-600 text-white font-medium hover:bg-blue-700"
                >
                    {locale.t("apps.command_save")}
                </button>
            </div>
        </form>
    }
}

fn render_commands_list(
    locale: Locale,
    app_id: ShortId,
    commands: &UseStateHandle<Option<Vec<AppScheduledCommand>>>,
    running_id: &UseStateHandle<Option<ShortId>>,
    active_output: &UseStateHandle<Option<(String, String)>>,
    editing_cmd: &UseStateHandle<Option<AppScheduledCommand>>,
) -> Html {
    let Some(list) = (**commands).as_ref() else {
        return html! {};
    };
    if list.is_empty() {
        return html! {
            <p class="py-4 text-center text-sm text-neutral-500 dark:text-neutral-400">
                {locale.t("apps.commands_empty")}
            </p>
        };
    }

    html! {
        <div class="overflow-x-auto">
            <table class="w-full text-left text-xs sm:text-sm text-neutral-600 dark:text-neutral-300">
                <thead class="bg-neutral-50 dark:bg-neutral-700/50 text-neutral-700 dark:text-neutral-200 uppercase text-xs">
                    <tr>
                        <th class="py-2 px-3">{locale.t("apps.command_name")}</th>
                        <th class="py-2 px-3">{locale.t("apps.command_schedule")}</th>
                        <th class="py-2 px-3">{locale.t("apps.command_cmd")}</th>
                        <th class="py-2 px-3">{locale.t("apps.command_last_run")}</th>
                        <th class="py-2 px-3 text-right">{locale.t("common.actions")}</th>
                    </tr>
                </thead>
                <tbody class="divide-y divide-neutral-200 dark:divide-neutral-700">
                    { list.iter().map(|cmd| {
                        render_command_row(locale, app_id, cmd, commands, running_id, active_output, editing_cmd)
                    }).collect::<Html>() }
                </tbody>
            </table>
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

fn render_command_row(
    locale: Locale,
    app_id: ShortId,
    cmd: &AppScheduledCommand,
    commands: &UseStateHandle<Option<Vec<AppScheduledCommand>>>,
    running_id: &UseStateHandle<Option<ShortId>>,
    active_output: &UseStateHandle<Option<(String, String)>>,
    editing_cmd: &UseStateHandle<Option<AppScheduledCommand>>,
) -> Html {
    let is_running = **running_id == Some(cmd.id);
    let run_onclick = run_command_callback(locale, app_id, cmd.id, running_id, commands);
    let delete_onclick = delete_command_callback(locale, app_id, cmd, commands);

    let view_output = {
        let active = active_output.clone();
        let name = cmd.name.clone();
        let output = cmd.last_output.clone().unwrap_or_default();
        Callback::from(move |_: MouseEvent| {
            active.set(Some((name.clone(), output.clone())));
        })
    };

    let edit_onclick = {
        let editing = editing_cmd.clone();
        let cmd = cmd.clone();
        Callback::from(move |_: MouseEvent| {
            editing.set(Some(cmd.clone()));
        })
    };

    let status_badge = render_command_status_badge(locale, cmd.last_status.as_ref());

    html! {
        <tr class="hover:bg-neutral-50 dark:hover:bg-neutral-700/30">
            <td class="py-2.5 px-3 font-medium text-neutral-900 dark:text-neutral-100">
                <div class="flex items-center gap-1.5">
                    <span>{&cmd.name}</span>
                    if !cmd.enabled {
                        <span class="text-xs text-neutral-400">{"("}{locale.t("apps.command_disabled")}{")"}</span>
                    }
                </div>
            </td>
            <td class="py-2.5 px-3 font-mono text-xs text-blue-600 dark:text-blue-400">
                {&cmd.schedule}
            </td>
            <td class="py-2.5 px-3 font-mono text-xs max-w-xs truncate" title={cmd.command.clone()}>
                {&cmd.command}
            </td>
            <td class="py-2.5 px-3 text-xs">
                <div class="flex items-center gap-1.5">
                    <span>{format_command_last_run(locale, cmd.last_run_at)}</span>
                    {status_badge}
                </div>
            </td>
            <td class="py-2.5 px-3 text-right">
                <div class="inline-flex items-center gap-1 justify-end">
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
                </div>
            </td>
        </tr>
    }
}

#[derive(Properties, PartialEq, Clone)]
struct EditModalProps {
    app_id: ShortId,
    locale: Locale,
    cmd: AppScheduledCommand,
    on_close: Callback<()>,
    on_saved: Callback<()>,
}

#[function_component(EditCommandModal)]
fn edit_command_modal(props: &EditModalProps) -> Html {
    let locale = props.locale;
    let app_id = props.app_id;
    let cmd = &props.cmd;
    let cmd_id = cmd.id;

    let name = use_state(|| cmd.name.clone());
    let schedule = use_state(|| cmd.schedule.clone());
    let command = use_state(|| cmd.command.clone());
    let enabled = use_state(|| cmd.enabled);
    let saving = use_state(|| false);

    let on_name = input_handler(&name);
    let on_schedule = input_handler(&schedule);
    let on_command = input_handler(&command);
    let on_toggle_enabled = {
        let enabled = enabled.clone();
        Callback::from(move |_: Event| enabled.set(!*enabled))
    };

    let on_close_click = {
        let on_close = props.on_close.clone();
        Callback::from(move |_: MouseEvent| on_close.emit(()))
    };

    let on_submit = {
        let (name, schedule, command, enabled, saving) = (
            name.clone(),
            schedule.clone(),
            command.clone(),
            enabled.clone(),
            saving.clone(),
        );
        let (on_saved, on_close) = (props.on_saved.clone(), props.on_close.clone());
        Callback::from(move |event: SubmitEvent| {
            event.prevent_default();
            let body = ScheduledCommandRequest {
                name: (*name).clone(),
                schedule: (*schedule).clone(),
                command: (*command).clone(),
                enabled: *enabled,
            };
            saving.set(true);
            let (saving, on_saved, on_close) = (saving.clone(), on_saved.clone(), on_close.clone());
            spawn_local(async move {
                let path = format!("{API_ENDPOINT}/apps/{app_id}/commands/{cmd_id}");
                let Ok(req) = Request::put(&path).json(&body) else {
                    saving.set(false);
                    return;
                };
                match send_request(locale, req).await {
                    Ok(_) => {
                        dialog::message(locale, locale.t("apps.command_updated"), Icon::Success)
                            .await;
                        on_saved.emit(());
                        on_close.emit(());
                    }
                    Err(err) => {
                        dialog::message(locale, &err, Icon::Error).await;
                        saving.set(false);
                    }
                }
            });
        })
    };

    html! {
        <div class="fixed inset-0 z-50 flex items-center justify-center p-4 bg-black/60 backdrop-blur-sm">
            <div class="bg-white dark:bg-neutral-800 border border-neutral-300 dark:border-neutral-700 rounded-lg max-w-xl w-full p-4 sm:p-6 shadow-xl space-y-4">
                <div class="flex items-center justify-between border-b border-neutral-200 dark:border-neutral-700 pb-3">
                    <h4 class="font-semibold text-neutral-900 dark:text-neutral-100">
                        {locale.t("apps.command_edit")}
                    </h4>
                    <button onclick={on_close_click.clone()} type="button" class="text-neutral-400 hover:text-neutral-600 dark:hover:text-neutral-200 text-lg">
                        {"✕"}
                    </button>
                </div>
                <form onsubmit={on_submit} class="space-y-3">
                    <div class="grid grid-cols-1 sm:grid-cols-2 gap-3">
                        <div>
                            <label class="block text-xs font-medium text-neutral-700 dark:text-neutral-300 mb-1">
                                {locale.t("apps.command_name")}
                            </label>
                            <input
                                type="text"
                                required=true
                                value={(*name).clone()}
                                oninput={on_name}
                                class={INPUT_CLASS}
                            />
                        </div>
                        <div>
                            <label class="block text-xs font-medium text-neutral-700 dark:text-neutral-300 mb-1">
                                {locale.t("apps.command_schedule")}
                            </label>
                            <input
                                type="text"
                                required=true
                                placeholder={locale.t("apps.command_schedule_hint")}
                                value={(*schedule).clone()}
                                oninput={on_schedule}
                                class={INPUT_CLASS}
                            />
                        </div>
                    </div>
                    <div>
                        <label class="block text-xs font-medium text-neutral-700 dark:text-neutral-300 mb-1">
                            {locale.t("apps.command_cmd")}
                        </label>
                        <input
                            type="text"
                            required=true
                            placeholder={locale.t("apps.command_cmd_placeholder")}
                            value={(*command).clone()}
                            oninput={on_command}
                            class={INPUT_CLASS}
                        />
                    </div>
                    <div class="flex items-center justify-between pt-2 border-t border-neutral-100 dark:border-neutral-700">
                        <label class="inline-flex items-center gap-2 text-xs text-neutral-700 dark:text-neutral-300 cursor-pointer">
                            <input
                                type="checkbox"
                                checked={*enabled}
                                onchange={on_toggle_enabled}
                                class="rounded border-neutral-300 dark:border-neutral-700 text-blue-600 focus:ring-blue-500"
                            />
                            {locale.t("apps.command_enabled")}
                        </label>
                        <div class="flex items-center gap-2">
                            <button
                                onclick={on_close_click}
                                type="button"
                                class="text-xs sm:text-sm px-3 py-1.5 rounded-lg border border-neutral-300 dark:border-neutral-700 hover:bg-neutral-100 dark:hover:bg-neutral-700"
                            >
                                {locale.t("common.cancel")}
                            </button>
                            <button
                                type="submit"
                                disabled={*saving}
                                class="text-xs sm:text-sm px-4 py-1.5 rounded-lg bg-blue-600 text-white font-medium hover:bg-blue-700 disabled:opacity-50"
                            >
                                if *saving {
                                    {locale.t("common.loading")}
                                } else {
                                    {locale.t("apps.command_save")}
                                }
                            </button>
                        </div>
                    </div>
                </form>
            </div>
        </div>
    }
}

fn render_output_modal(
    locale: Locale,
    name: &str,
    output: &str,
    active_output: &UseStateHandle<Option<(String, String)>>,
) -> Html {
    let close = {
        let active = active_output.clone();
        Callback::from(move |_: MouseEvent| active.set(None))
    };
    html! {
        <div class="fixed inset-0 z-50 flex items-center justify-center p-4 bg-black/50">
            <div class="bg-white dark:bg-neutral-800 border border-neutral-300 dark:border-neutral-700 rounded-lg max-w-2xl w-full p-4 sm:p-6 shadow-xl space-y-3">
                <div class="flex items-center justify-between">
                    <h4 class="font-semibold text-neutral-900 dark:text-neutral-100">
                        {name}
                    </h4>
                    <button onclick={close.clone()} type="button" class="text-neutral-400 hover:text-neutral-600 dark:hover:text-neutral-200 text-lg">
                        {"✕"}
                    </button>
                </div>
                <pre class="overflow-auto max-h-80 p-3 bg-neutral-900 text-neutral-100 font-mono text-xs rounded break-words whitespace-pre-wrap">
                    {if output.is_empty() { "No output." } else { output }}
                </pre>
                <div class="text-right">
                    <button onclick={close} type="button" class="text-xs sm:text-sm px-3 py-1.5 rounded-lg border border-neutral-300 dark:border-neutral-700 hover:bg-neutral-100 dark:hover:bg-neutral-700">
                        {locale.t("apps.command_close")}
                    </button>
                </div>
            </div>
        </div>
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
        let result: Result<Vec<AppScheduledCommand>, _> =
            fetch_json(locale, Request::get(&path)).await;
        commands.set(result.ok());
    });
}

fn add_command_submit(
    locale: Locale,
    app_id: ShortId,
    form: &CommandFormState,
    commands: &UseStateHandle<Option<Vec<AppScheduledCommand>>>,
) -> Callback<SubmitEvent> {
    let (form, commands) = (form.clone(), commands.clone());
    Callback::from(move |event: SubmitEvent| {
        event.prevent_default();
        let body = ScheduledCommandRequest {
            name: (*form.name).clone(),
            schedule: (*form.schedule).clone(),
            command: (*form.command).clone(),
            enabled: *form.enabled,
        };
        let (form, commands) = (form.clone(), commands.clone());
        spawn_local(async move {
            let path = format!("{API_ENDPOINT}/apps/{app_id}/commands");
            let Ok(req) = Request::post(&path).json(&body) else {
                return;
            };
            if send_request(locale, req).await.is_ok() {
                form.reset();
                load_commands(locale, app_id, commands);
            }
        });
    })
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
            if send_request(locale, req).await.is_ok() {
                load_commands(locale, app_id, commands);
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
