//! The page for managing Projects and Environments hierarchy.

use super::accounts::{INPUT_CLASS, LABEL_CLASS};
use super::resource_page::{FormCard, add_button, form_modal};
use super::settings::send_json;
use crate::API_ENDPOINT;
use crate::auth::use_ensure_auth;
use crate::components::action_button::{ActionIconButton, ActionVariant};
use crate::components::button::{Button, ButtonKind, ButtonSize};
use crate::components::data_list::{Column, ListContent, ListOptions, Row, list_card, list_view};
use crate::components::modal::{Modal, ModalSize};
use crate::components::tabs::{TabItem, Tabs};
use crate::dialog;
use crate::format::format_time;
use crate::i18n::use_locale;
use crate::store::SessionStore;
use gloo_net::http::Request;
use r3v3rs3_api::i18n::Locale;
use r3v3rs3_api::id::ShortId;
use r3v3rs3_api::platform::{
    EnvDiffEntry, EnvDiffStatus, EnvEntry, EnvironmentDiffReport, EnvironmentEnvSpec,
    EnvironmentSyncMode, EnvironmentSyncReport, EnvironmentSyncRequest, Project,
    ProjectEnvironmentSummary, ProjectSpec, ProjectSummary,
};
use wasm_bindgen_futures::spawn_local;
use web_sys::{HtmlInputElement, HtmlSelectElement};
use yew::prelude::*;
use yewdux::prelude::*;

const PATH: &str = "/projects";

const SELECT: &str = "rounded-lg border border-neutral-300 bg-neutral-50 text-neutral-900 dark:border-neutral-600 dark:bg-neutral-700 dark:text-white px-2.5 py-1.5 text-xs focus:ring-primary-500 focus:border-primary-500";

const DIFF_COLUMNS: [Column; 4] = [
    Column {
        label: "apps.env_key",
        class: "font-semibold",
    },
    Column {
        label: "projects.source_env",
        class: "",
    },
    Column {
        label: "projects.target_env",
        class: "",
    },
    Column {
        label: "projects.diff_status",
        class: "whitespace-nowrap",
    },
];

const COLUMNS: [Column; 5] = [
    Column {
        label: "projects.name",
        class: "whitespace-nowrap font-medium text-neutral-900 dark:text-neutral-100",
    },
    Column {
        label: "projects.description",
        class: "whitespace-nowrap text-neutral-600 dark:text-neutral-400",
    },
    Column {
        label: "projects.environments",
        class: "whitespace-nowrap",
    },
    Column {
        label: "projects.apps",
        class: "whitespace-nowrap text-neutral-600 dark:text-neutral-400",
    },
    Column {
        label: "env_groups.last_updated",
        class: "whitespace-nowrap text-neutral-600 dark:text-neutral-400",
    },
];

type ProjectsList = UseStateHandle<Option<Vec<ProjectSummary>>>;

#[derive(Clone, PartialEq, Default)]
struct ProjectFormData {
    editing_id: Option<ShortId>,
    name: String,
    description: String,
    default_envs: String,
}

#[derive(Clone, Copy, PartialEq, Eq, Default, Debug)]
enum EnvModalTab {
    #[default]
    Variables,
    CompareSync,
}

#[derive(Clone, PartialEq, Default, Debug)]
struct EnvModalState {
    project_id: ShortId,
    project_name: String,
    environments: Vec<ProjectEnvironmentSummary>,
    selected_env_id: ShortId,
    active_tab: EnvModalTab,
    variables: Vec<EnvVarRow>,
    diff_source_id: Option<ShortId>,
    diff_target_id: Option<ShortId>,
    diff_report: Option<EnvironmentDiffReport>,
    sync_status_msg: Option<String>,
}

#[derive(Clone, PartialEq, Default, Debug)]
struct EnvVarRow {
    key: String,
    value: String,
    secret: bool,
    stored_secret: bool,
}

impl EnvVarRow {
    fn from_entry(entry: EnvEntry) -> Self {
        Self {
            key: entry.key.as_str().to_string(),
            stored_secret: entry.secret && entry.value.is_none(),
            value: entry.value.unwrap_or_default(),
            secret: entry.secret,
        }
    }

    fn to_entry(&self, locale: Locale) -> Result<EnvEntry, String> {
        let key = self
            .key
            .trim()
            .parse()
            .map_err(|err| locale.error_message(&err))?;
        let keeps_value = self.secret && self.stored_secret;
        let value = match self.value.is_empty() {
            false => Some(self.value.clone()),
            true if keeps_value => None,
            true => {
                return Err(locale.tf("apps.env_value_required", &[("key", self.key.trim())]));
            }
        };
        Ok(EnvEntry {
            key,
            value,
            secret: self.secret,
        })
    }
}

#[function_component(Projects)]
pub fn projects_page() -> Html {
    use_ensure_auth();
    let locale = use_locale();
    let (session, _) = use_store::<SessionStore>();
    let list: ProjectsList = use_state(|| None);
    let reload = use_state(|| 0_u32);
    let form: UseStateHandle<Option<ProjectFormData>> = use_state(|| None);
    let env_modal: UseStateHandle<Option<EnvModalState>> = use_state(|| None);

    let signed_in = session.info.is_some();
    let list_cloned = list.clone();
    use_effect_with((signed_in, *reload), move |&(signed_in, _)| {
        if signed_in {
            load_projects(list_cloned);
        }
        || ()
    });

    let on_new = {
        let form = form.clone();
        Callback::from(move |_| {
            form.set(Some(ProjectFormData {
                editing_id: None,
                name: String::new(),
                description: String::new(),
                default_envs: "production, staging, development".into(),
            }));
        })
    };

    html! {
        <div class="space-y-6">
            <div>
                <h1 class="text-2xl font-bold tracking-tight text-neutral-900 dark:text-neutral-100">
                    {locale.t("projects.title")}
                </h1>
                <p class="text-sm text-neutral-500 dark:text-neutral-400">
                    {locale.t("projects.subtitle")}
                </p>
            </div>

            <div>
                {render_projects_table(locale, &list, &form, &reload, session.can_edit(), &env_modal)}
                if session.can_edit() {
                    { add_button(locale, on_new) }
                }
            </div>
            {render_project_form(locale, &form, &reload)}
            {render_env_modal(locale, &env_modal, &reload)}
        </div>
    }
}

fn load_projects(list: ProjectsList) {
    spawn_local(async move {
        let url = format!("{API_ENDPOINT}{PATH}");
        let Ok(response) = Request::get(&url).send().await else {
            return;
        };
        let Ok(projects) = response.json::<Vec<ProjectSummary>>().await else {
            return;
        };
        list.set(Some(projects));
    });
}

fn render_projects_table(
    locale: Locale,
    list: &ProjectsList,
    form: &UseStateHandle<Option<ProjectFormData>>,
    reload: &UseStateHandle<u32>,
    can_edit: bool,
    env_modal: &UseStateHandle<Option<EnvModalState>>,
) -> Html {
    let loaded = list.is_some();
    let empty_vec = Vec::new();
    let projects = list.as_deref().unwrap_or(&empty_vec);

    let rows = projects
        .iter()
        .map(|p| render_project_row(locale, p, form, reload, can_edit, env_modal))
        .collect::<Vec<_>>();

    list_card(locale, loaded, "projects.empty", &COLUMNS, &rows)
}

fn render_project_row(
    locale: Locale,
    project: &ProjectSummary,
    form: &UseStateHandle<Option<ProjectFormData>>,
    reload: &UseStateHandle<u32>,
    can_edit: bool,
    env_modal: &UseStateHandle<Option<EnvModalState>>,
) -> Row {
    let p = project.clone();
    let edit_cb = {
        let form = form.clone();
        let p = p.clone();
        Callback::from(move |e: MouseEvent| {
            e.prevent_default();
            form.set(Some(ProjectFormData {
                editing_id: Some(p.id),
                name: p.name.clone(),
                description: p.description.clone().unwrap_or_default(),
                default_envs: String::new(),
            }));
        })
    };

    let del_cb = {
        let reload = reload.clone();
        let p_id = p.id;
        let p_name = p.name.clone();
        Callback::from(move |e: MouseEvent| {
            e.prevent_default();
            let reload = reload.clone();
            let p_name = p_name.clone();
            spawn_local(async move {
                let question = format!("{} ({p_name})", locale.t("projects.delete_confirm"));
                let confirmed = dialog::confirm(locale, &question).await;
                if confirmed {
                    let url = format!("{API_ENDPOINT}{PATH}/{p_id}");
                    match Request::delete(&url).send().await {
                        Ok(res) if res.ok() => {
                            reload.set(*reload + 1);
                        }
                        Ok(res) => {
                            let msg = crate::pages::settings::error_message(locale, res).await;
                            dialog::message(locale, &msg, dialog::Icon::Error).await;
                        }
                        Err(err) => {
                            dialog::message(locale, &err.to_string(), dialog::Icon::Error).await;
                        }
                    }
                }
            });
        })
    };

    let env_cb = {
        let env_modal = env_modal.clone();
        let p_id = p.id;
        Callback::from(move |e: MouseEvent| {
            e.prevent_default();
            open_env_modal(p_id, env_modal.clone());
        })
    };

    let actions = if can_edit {
        html! {
            <div class="flex items-center space-x-1.5 text-sm">
                <ActionIconButton
                    variant={ActionVariant::Files}
                    title={locale.t("projects.environments").to_string()}
                    onclick={env_cb}
                />
                <ActionIconButton
                    variant={ActionVariant::Edit}
                    title={locale.t("projects.edit").to_string()}
                    onclick={edit_cb}
                />
                <ActionIconButton
                    variant={ActionVariant::Delete}
                    title={locale.t("projects.delete").to_string()}
                    onclick={del_cb}
                />
            </div>
        }
    } else {
        html!()
    };

    Row {
        key: p.id.to_string(),
        cells: vec![
            html! { <span class="font-semibold text-neutral-900 dark:text-neutral-100">{&p.name}</span> },
            html! { <span class="text-neutral-500">{p.description.as_deref().unwrap_or("-")}</span> },
            html! {
                <span class="inline-flex items-center px-2 py-0.5 rounded text-xs font-medium bg-neutral-100 text-neutral-800 dark:bg-neutral-800 dark:text-neutral-200">
                    {format!("{} {}", p.environments_count, locale.t("projects.environments"))}
                </span>
            },
            html! {
                <span class="inline-flex items-center px-2 py-0.5 rounded text-xs font-medium bg-blue-50 text-blue-700 dark:bg-blue-900/30 dark:text-blue-300">
                    {format!("{} {}", p.apps_count, locale.t("projects.apps"))}
                </span>
            },
            html! { <span class="text-neutral-500">{format_time(p.updated_at)}</span> },
        ],
        actions,
    }
}

fn open_env_modal(project_id: ShortId, modal: UseStateHandle<Option<EnvModalState>>) {
    spawn_local(async move {
        let url = format!("{API_ENDPOINT}{PATH}/{project_id}");
        let Ok(res) = Request::get(&url).send().await else {
            return;
        };
        let Ok(p) = res.json::<Project>().await else {
            return;
        };
        if let Some(first_env) = p.environments.first() {
            let env_url = format!(
                "{API_ENDPOINT}{PATH}/{project_id}/environments/{}/env",
                first_env.id
            );
            let vars = if let Ok(v_res) = Request::get(&env_url).send().await {
                v_res.json::<Vec<EnvEntry>>().await.unwrap_or_default()
            } else {
                Vec::new()
            };
            let diff_source_id = Some(first_env.id);
            let diff_target_id = p.environments.get(1).map(|e| e.id);
            modal.set(Some(EnvModalState {
                project_id,
                project_name: p.name,
                selected_env_id: first_env.id,
                environments: p.environments,
                active_tab: EnvModalTab::Variables,
                variables: vars.into_iter().map(EnvVarRow::from_entry).collect(),
                diff_source_id,
                diff_target_id,
                diff_report: None,
                sync_status_msg: None,
            }));
        }
    });
}

fn switch_env_tab(
    modal: UseStateHandle<Option<EnvModalState>>,
    project_id: ShortId,
    env_id: ShortId,
) {
    spawn_local(async move {
        let env_url = format!("{API_ENDPOINT}{PATH}/{project_id}/environments/{env_id}/env");
        let vars = if let Ok(v_res) = Request::get(&env_url).send().await {
            v_res.json::<Vec<EnvEntry>>().await.unwrap_or_default()
        } else {
            Vec::new()
        };
        modal.set(modal.as_ref().map(|st| {
            let mut updated = st.clone();
            updated.selected_env_id = env_id;
            updated.active_tab = EnvModalTab::Variables;
            updated.variables = vars.into_iter().map(EnvVarRow::from_entry).collect();
            updated.sync_status_msg = None;
            updated
        }));
    });
}

fn switch_compare_tab(modal: UseStateHandle<Option<EnvModalState>>, project_id: ShortId) {
    if let Some(st) = modal.as_ref() {
        let source_id = st.diff_source_id.unwrap_or(st.selected_env_id);
        let target_id = st.diff_target_id.or_else(|| {
            st.environments
                .iter()
                .find(|e| e.id != source_id)
                .map(|e| e.id)
        });
        let mut updated = st.clone();
        updated.active_tab = EnvModalTab::CompareSync;
        updated.diff_source_id = Some(source_id);
        updated.diff_target_id = target_id;
        updated.sync_status_msg = None;
        modal.set(Some(updated));

        if let Some(tgt) = target_id
            && tgt != source_id
        {
            fetch_environment_diff(modal, project_id, source_id, tgt);
        }
    }
}

fn fetch_environment_diff(
    modal: UseStateHandle<Option<EnvModalState>>,
    project_id: ShortId,
    source_id: ShortId,
    target_id: ShortId,
) {
    spawn_local(async move {
        let url =
            format!("{API_ENDPOINT}{PATH}/{project_id}/environments/{source_id}/diff/{target_id}");
        if let Ok(res) = Request::get(&url).send().await
            && let Ok(report) = res.json::<EnvironmentDiffReport>().await
        {
            modal.set(modal.as_ref().map(|st| {
                let mut updated = st.clone();
                updated.diff_report = Some(report);
                updated
            }));
        }
    });
}

fn run_environment_sync(
    locale: Locale,
    modal: UseStateHandle<Option<EnvModalState>>,
    reload: UseStateHandle<u32>,
    req: EnvironmentSyncRequest,
    project_id: ShortId,
) {
    spawn_local(async move {
        let url = format!("{API_ENDPOINT}{PATH}/{project_id}/environments/sync");
        let Ok(req_builder) = Request::post(&url).json(&req) else {
            return;
        };
        let resp = match req_builder.send().await {
            Ok(r) => r,
            Err(err) => {
                dialog::message(locale, &err.to_string(), dialog::Icon::Error).await;
                return;
            }
        };
        if !resp.ok() {
            let msg = crate::pages::settings::error_message(locale, resp).await;
            dialog::message(locale, &msg, dialog::Icon::Error).await;
            return;
        }
        if let Ok(report) = resp.json::<EnvironmentSyncReport>().await {
            let msg = locale.tf(
                "projects.sync_success",
                &[("count", &report.synced_count.to_string())],
            );
            fetch_environment_diff(
                modal.clone(),
                project_id,
                req.source_env_id,
                req.target_env_id,
            );
            modal.set(modal.as_ref().map(|st| {
                let mut updated = st.clone();
                updated.sync_status_msg = Some(msg);
                updated
            }));
            reload.set(*reload + 1);
        }
    });
}

fn render_project_form(
    locale: Locale,
    form: &UseStateHandle<Option<ProjectFormData>>,
    reload: &UseStateHandle<u32>,
) -> Html {
    let Some(data) = form.as_ref() else {
        return html!();
    };

    let is_editing = data.editing_id.is_some();
    let title = if is_editing {
        locale.t("projects.edit")
    } else {
        locale.t("projects.new")
    };

    let name = data.name.clone();
    let desc = data.description.clone();
    let def_envs = data.default_envs.clone();

    let on_name_change = {
        let form = form.clone();
        let data = data.clone();
        Callback::from(move |e: InputEvent| {
            if let Some(target) = e.target_dyn_into::<HtmlInputElement>() {
                let mut updated = data.clone();
                updated.name = target.value();
                form.set(Some(updated));
            }
        })
    };

    let on_desc_change = {
        let form = form.clone();
        let data = data.clone();
        Callback::from(move |e: InputEvent| {
            if let Some(target) = e.target_dyn_into::<HtmlInputElement>() {
                let mut updated = data.clone();
                updated.description = target.value();
                form.set(Some(updated));
            }
        })
    };

    let on_envs_change = {
        let form = form.clone();
        let data = data.clone();
        Callback::from(move |e: InputEvent| {
            if let Some(target) = e.target_dyn_into::<HtmlInputElement>() {
                let mut updated = data.clone();
                updated.default_envs = target.value();
                form.set(Some(updated));
            }
        })
    };

    let on_close = {
        let form = form.clone();
        Callback::from(move |()| form.set(None))
    };

    let on_submit = {
        let form = form.clone();
        let reload = reload.clone();
        let data = data.clone();
        Callback::from(move |e: SubmitEvent| {
            e.prevent_default();
            let form = form.clone();
            let reload = reload.clone();
            let data = data.clone();
            spawn_local(async move {
                let spec = ProjectSpec {
                    name: data.name.clone(),
                    description: (!data.description.trim().is_empty())
                        .then(|| data.description.clone()),
                    default_environments: data
                        .default_envs
                        .split(',')
                        .map(str::trim)
                        .filter(|s| !s.is_empty())
                        .map(ToString::to_string)
                        .collect(),
                };
                let res = if let Some(id) = data.editing_id {
                    let url = format!("{API_ENDPOINT}{PATH}/{id}");
                    send_json(locale, Request::put(&url), &spec).await
                } else {
                    let url = format!("{API_ENDPOINT}{PATH}");
                    send_json(locale, Request::post(&url), &spec).await
                };
                match res {
                    Ok(()) => {
                        form.set(None);
                        reload.set(*reload + 1);
                    }
                    Err(err) => {
                        dialog::message(locale, &err, dialog::Icon::Error).await;
                    }
                }
            });
        })
    };

    let fields = html! {
        <div class="space-y-4">
            <div>
                <label class={LABEL_CLASS}>{locale.t("projects.name")}</label>
                <input
                    type="text"
                    class={INPUT_CLASS}
                    value={name}
                    oninput={on_name_change}
                    placeholder={locale.t("projects.name_placeholder")}
                    required=true
                />
            </div>
            <div>
                <label class={LABEL_CLASS}>{locale.t("projects.description")}</label>
                <input
                    type="text"
                    class={INPUT_CLASS}
                    value={desc}
                    oninput={on_desc_change}
                />
            </div>
            if !is_editing {
                <div>
                    <label class={LABEL_CLASS}>{locale.t("projects.default_envs")}</label>
                    <input
                        type="text"
                        class={INPUT_CLASS}
                        value={def_envs}
                        oninput={on_envs_change}
                        placeholder={locale.t("projects.default_envs_placeholder")}
                    />
                </div>
            }
        </div>
    };
    let card = FormCard {
        title: title.to_string(),
        notice: html! {},
        fields,
        error: None,
        cancel: None,
        save_label: if is_editing {
            "common.update"
        } else {
            "common.create"
        },
        can_save: true,
    };
    form_modal(locale, card, on_submit, on_close)
}

fn render_env_modal(
    locale: Locale,
    env_modal: &UseStateHandle<Option<EnvModalState>>,
    reload: &UseStateHandle<u32>,
) -> Html {
    let Some(state) = env_modal.as_ref() else {
        return html!();
    };

    let on_close = {
        let env_modal = env_modal.clone();
        Callback::from(move |()| env_modal.set(None))
    };
    let close = {
        let on_close = on_close.clone();
        Callback::from(move |_: MouseEvent| on_close.emit(()))
    };
    let is_variables = state.active_tab == EnvModalTab::Variables;
    let footer = html! {
        <>
            <Button kind={ButtonKind::Secondary} onclick={close}>
                {locale.t(if is_variables { "common.cancel" } else { "common.close" })}
            </Button>
            if is_variables {
                <Button kind={ButtonKind::Primary} onclick={env_save_callback(locale, state, env_modal, reload)}>
                    {locale.t("common.update")}
                </Button>
            }
        </>
    };
    let title = format!("{} - {}", locale.t("projects.env_vars"), state.project_name);

    html! {
        <Modal {title} {on_close} size={ModalSize::Xl} {footer}>
            <p class="text-xs text-neutral-500 dark:text-neutral-400 mb-2">
                {locale.t("projects.inherited_env_vars")}
            </p>
            {render_env_modal_tabs(locale, state, env_modal)}
            if is_variables {
                {render_env_variables_tab(locale, state, env_modal)}
            } else {
                {render_compare_sync_tab(locale, state, env_modal, reload)}
            }
        </Modal>
    }
}

fn render_env_modal_tabs(
    locale: Locale,
    state: &EnvModalState,
    env_modal: &UseStateHandle<Option<EnvModalState>>,
) -> Html {
    let project_id = state.project_id;
    let is_compare_active = state.active_tab == EnvModalTab::CompareSync;

    let on_compare_click = {
        let env_modal = env_modal.clone();
        Callback::from(move |e: MouseEvent| {
            e.prevent_default();
            switch_compare_tab(env_modal.clone(), project_id);
        })
    };

    let mut tabs = state
        .environments
        .iter()
        .map(|env| {
            let env_id = env.id;
            let is_selected =
                state.active_tab == EnvModalTab::Variables && state.selected_env_id == env_id;
            let modal_ref = env_modal.clone();
            let on_tab = Callback::from(move |e: MouseEvent| {
                e.prevent_default();
                switch_env_tab(modal_ref.clone(), project_id, env_id);
            });
            TabItem::click(env.name.clone(), on_tab, is_selected)
        })
        .collect::<Vec<_>>();
    tabs.push(TabItem::click(
        format!("⇄ {}", locale.t("projects.compare_sync_tab")),
        on_compare_click,
        is_compare_active,
    ));
    html! { <Tabs {tabs} /> }
}

fn render_env_variables_tab(
    locale: Locale,
    state: &EnvModalState,
    env_modal: &UseStateHandle<Option<EnvModalState>>,
) -> Html {
    let on_add_var = {
        let env_modal = env_modal.clone();
        let state = state.clone();
        Callback::from(move |_: MouseEvent| {
            let mut updated = state.clone();
            updated.variables.push(EnvVarRow::default());
            env_modal.set(Some(updated));
        })
    };

    html! {
        <div class="py-4 space-y-3 min-h-[200px]">
            {state.variables.iter().enumerate().map(|(idx, row)| {
                render_var_row(locale, idx, row, env_modal.clone(), state.clone())
            }).collect::<Html>()}
            <Button kind={ButtonKind::Secondary} size={ButtonSize::Sm} onclick={on_add_var}>
                {locale.t("env_groups.add_variable")}
            </Button>
        </div>
    }
}

/// Saves the variables of the selected environment.
fn env_save_callback(
    locale: Locale,
    state: &EnvModalState,
    env_modal: &UseStateHandle<Option<EnvModalState>>,
    reload: &UseStateHandle<u32>,
) -> Callback<MouseEvent> {
    {
        let env_modal = env_modal.clone();
        let reload = reload.clone();
        let state = state.clone();
        Callback::from(move |e: MouseEvent| {
            e.prevent_default();
            let env_modal = env_modal.clone();
            let reload = reload.clone();
            let state = state.clone();
            spawn_local(async move {
                let parsed_vars = state
                    .variables
                    .iter()
                    .filter(|v| !v.key.trim().is_empty())
                    .map(|v| v.to_entry(locale))
                    .collect::<Result<Vec<_>, _>>();

                let Ok(variables) = parsed_vars else {
                    return;
                };

                let spec = EnvironmentEnvSpec { variables };
                let url = format!(
                    "{API_ENDPOINT}{PATH}/{}/environments/{}/env",
                    state.project_id, state.selected_env_id
                );
                match send_json(locale, Request::put(&url), &spec).await {
                    Ok(()) => {
                        env_modal.set(None);
                        reload.set(*reload + 1);
                    }
                    Err(err) => {
                        dialog::message(locale, &err, dialog::Icon::Error).await;
                    }
                }
            });
        })
    }
}

fn create_source_change_callback(
    env_modal: UseStateHandle<Option<EnvModalState>>,
    state: EnvModalState,
    project_id: ShortId,
) -> Callback<Event> {
    Callback::from(move |e: Event| {
        if let Some(target) = e.target_dyn_into::<HtmlSelectElement>()
            && let Ok(id) = target.value().parse::<ShortId>()
        {
            let mut updated = state.clone();
            updated.diff_source_id = Some(id);
            env_modal.set(Some(updated));
            if let Some(tgt) = state.diff_target_id
                && tgt != id
            {
                fetch_environment_diff(env_modal.clone(), project_id, id, tgt);
            }
        }
    })
}

fn create_target_change_callback(
    env_modal: UseStateHandle<Option<EnvModalState>>,
    state: EnvModalState,
    project_id: ShortId,
) -> Callback<Event> {
    Callback::from(move |e: Event| {
        if let Some(target) = e.target_dyn_into::<HtmlSelectElement>()
            && let Ok(id) = target.value().parse::<ShortId>()
        {
            let mut updated = state.clone();
            updated.diff_target_id = Some(id);
            env_modal.set(Some(updated));
            if let Some(src) = state.diff_source_id
                && src != id
            {
                fetch_environment_diff(env_modal.clone(), project_id, src, id);
            }
        }
    })
}

fn render_compare_controls(
    locale: Locale,
    state: &EnvModalState,
    env_modal: &UseStateHandle<Option<EnvModalState>>,
) -> Html {
    let project_id = state.project_id;
    let source_id = state.diff_source_id;
    let target_id = state.diff_target_id;

    let on_source_change =
        create_source_change_callback(env_modal.clone(), state.clone(), project_id);
    let on_target_change =
        create_target_change_callback(env_modal.clone(), state.clone(), project_id);

    let on_compare_btn = {
        let env_modal = env_modal.clone();
        Callback::from(move |e: MouseEvent| {
            e.prevent_default();
            if let (Some(src), Some(tgt)) = (source_id, target_id)
                && src != tgt
            {
                fetch_environment_diff(env_modal.clone(), project_id, src, tgt);
            }
        })
    };

    html! {
        <div class="flex flex-wrap items-center gap-3 bg-neutral-50 dark:bg-neutral-900/50 p-3 rounded-lg border border-neutral-200 dark:border-neutral-700">
            <div class="flex items-center space-x-2">
                <label class="text-xs font-medium text-neutral-600 dark:text-neutral-400">
                    {locale.t("projects.source_env")}
                </label>
                <select class={SELECT} onchange={on_source_change}>
                    <option key="" value="" selected={source_id.is_none()}>{locale.t("projects.select_environment")}</option>
                    {state.environments.iter().map(|env| {
                        let selected = Some(env.id) == source_id;
                        html! { <option key={env.id.to_string()} value={env.id.to_string()} selected={selected}>{&env.name}</option> }
                    }).collect::<Html>()}
                </select>
            </div>

            <span class="text-neutral-400">{"➔"}</span>

            <div class="flex items-center space-x-2">
                <label class="text-xs font-medium text-neutral-600 dark:text-neutral-400">
                    {locale.t("projects.target_env")}
                </label>
                <select class={SELECT} onchange={on_target_change}>
                    <option key="" value="" selected={target_id.is_none()}>{locale.t("projects.select_environment")}</option>
                    {state.environments.iter().map(|env| {
                        let selected = Some(env.id) == target_id;
                        html! { <option key={env.id.to_string()} value={env.id.to_string()} selected={selected}>{&env.name}</option> }
                    }).collect::<Html>()}
                </select>
            </div>

            <Button kind={ButtonKind::Secondary} size={ButtonSize::Sm} onclick={on_compare_btn}>
                {locale.t("projects.compare")}
            </Button>
        </div>
    }
}

fn render_compare_sync_tab(
    locale: Locale,
    state: &EnvModalState,
    env_modal: &UseStateHandle<Option<EnvModalState>>,
    reload: &UseStateHandle<u32>,
) -> Html {
    html! {
        <div class="flex-1 overflow-y-auto py-4 space-y-4">
            {render_compare_controls(locale, state, env_modal)}

            {if let Some(msg) = &state.sync_status_msg {
                html! {
                    <div class="p-3 text-xs rounded-lg bg-green-50 text-green-700 dark:bg-green-900/30 dark:text-green-300 border border-green-200 dark:border-green-800 flex items-center justify-between">
                        <span>{"✓ "}{msg}</span>
                    </div>
                }
            } else {
                html!()
            }}

            {if let Some(report) = &state.diff_report {
                html! {
                    <div class="space-y-3">
                        <div class="flex items-center justify-between flex-wrap gap-2">
                            {render_diff_summary(locale, report)}
                            {render_diff_actions(locale, state, report, env_modal, reload)}
                        </div>
                        {render_diff_table(locale, state, report, env_modal, reload)}
                    </div>
                }
            } else {
                html! {
                    <div class="text-center py-10 text-neutral-400 text-xs">
                        {locale.t("projects.no_diff_data")}
                    </div>
                }
            }}
        </div>
    }
}

fn render_diff_summary(locale: Locale, report: &EnvironmentDiffReport) -> Html {
    html! {
        <div class="flex flex-wrap items-center gap-2 text-xs">
            <span class="px-2.5 py-1 rounded bg-neutral-100 dark:bg-neutral-700 text-neutral-700 dark:text-neutral-300 font-medium">
                {locale.tf("projects.diff_total", &[("count", &report.total_keys.to_string())])}
            </span>
            <span class="px-2.5 py-1 rounded bg-green-50 dark:bg-green-900/30 text-green-700 dark:text-green-300 font-medium">
                {locale.tf("projects.diff_identical", &[("count", &report.identical.to_string())])}
            </span>
            <span class="px-2.5 py-1 rounded bg-amber-50 dark:bg-amber-900/30 text-amber-700 dark:text-amber-300 font-medium">
                {locale.tf("projects.diff_different", &[("count", &report.different_values.to_string())])}
            </span>
            <span class="px-2.5 py-1 rounded bg-orange-50 dark:bg-orange-900/30 text-orange-700 dark:text-orange-300 font-medium">
                {locale.tf("projects.diff_missing_target", &[("count", &report.missing_in_target.to_string())])}
            </span>
            <span class="px-2.5 py-1 rounded bg-blue-50 dark:bg-blue-900/30 text-blue-700 dark:text-blue-300 font-medium">
                {locale.tf("projects.diff_missing_source", &[("count", &report.missing_in_source.to_string())])}
            </span>
        </div>
    }
}

fn render_diff_actions(
    locale: Locale,
    state: &EnvModalState,
    report: &EnvironmentDiffReport,
    env_modal: &UseStateHandle<Option<EnvModalState>>,
    reload: &UseStateHandle<u32>,
) -> Html {
    let on_sync_missing = {
        let env_modal = env_modal.clone();
        let reload = reload.clone();
        let project_id = state.project_id;
        let source_id = report.source_env_id;
        let target_id = report.target_env_id;
        Callback::from(move |e: MouseEvent| {
            e.prevent_default();
            let req = EnvironmentSyncRequest {
                source_env_id: source_id,
                target_env_id: target_id,
                mode: EnvironmentSyncMode::SyncMissing,
                keys: Vec::new(),
            };
            run_environment_sync(locale, env_modal.clone(), reload.clone(), req, project_id);
        })
    };

    let on_clone_all = {
        let env_modal = env_modal.clone();
        let reload = reload.clone();
        let project_id = state.project_id;
        let source_id = report.source_env_id;
        let target_id = report.target_env_id;
        Callback::from(move |e: MouseEvent| {
            e.prevent_default();
            let env_modal = env_modal.clone();
            let reload = reload.clone();
            spawn_local(async move {
                let confirmed = dialog::confirm(locale, locale.t("projects.clone_confirm")).await;
                if confirmed {
                    let req = EnvironmentSyncRequest {
                        source_env_id: source_id,
                        target_env_id: target_id,
                        mode: EnvironmentSyncMode::CloneAll,
                        keys: Vec::new(),
                    };
                    run_environment_sync(locale, env_modal, reload, req, project_id);
                }
            });
        })
    };

    html! {
        <div class="flex items-center space-x-2">
            if report.missing_in_target > 0 {
                <Button kind={ButtonKind::Primary} size={ButtonSize::Sm} onclick={on_sync_missing}>
                    {format!("{} ({})", locale.t("projects.sync_missing"), report.missing_in_target)}
                </Button>
            }
            <Button kind={ButtonKind::Warning} size={ButtonSize::Sm} onclick={on_clone_all}>
                {locale.t("projects.clone_all")}
            </Button>
        </div>
    }
}

fn render_diff_table(
    locale: Locale,
    state: &EnvModalState,
    report: &EnvironmentDiffReport,
    env_modal: &UseStateHandle<Option<EnvModalState>>,
    reload: &UseStateHandle<u32>,
) -> Html {
    let rows = report
        .entries
        .iter()
        .map(|entry| render_diff_row(locale, state, entry, env_modal, reload))
        .collect::<Vec<_>>();
    let content = ListContent {
        loaded: true,
        empty: "projects.no_diff_data",
        columns: &DIFF_COLUMNS,
        rows: &rows,
    };
    let options = ListOptions {
        embedded: true,
        compact: true,
        headings: Some(vec![
            locale.t("apps.env_key").to_string(),
            format!(
                "{} ({})",
                locale.t("projects.source_env"),
                report.source_env_name
            ),
            format!(
                "{} ({})",
                locale.t("projects.target_env"),
                report.target_env_name
            ),
            locale.t("projects.diff_status").to_string(),
        ]),
        ..Default::default()
    };
    html! {
        <div class="rounded-md border border-neutral-300 dark:border-neutral-700">
            { list_view(locale, content, None, &options) }
        </div>
    }
}

fn render_diff_row(
    locale: Locale,
    state: &EnvModalState,
    entry: &EnvDiffEntry,
    modal: &UseStateHandle<Option<EnvModalState>>,
    reload: &UseStateHandle<u32>,
) -> Row {
    let key_str = entry.key.as_str().to_string();
    let is_syncable = entry.source_present
        && (entry.status == EnvDiffStatus::MissingInTarget
            || entry.status == EnvDiffStatus::Different);

    let on_sync_key = {
        let modal = modal.clone();
        let reload = reload.clone();
        let project_id = state.project_id;
        let source_id = state.diff_source_id;
        let target_id = state.diff_target_id;
        let key_clone = entry.key.clone();
        Callback::from(move |e: MouseEvent| {
            e.prevent_default();
            if let (Some(src), Some(tgt)) = (source_id, target_id) {
                let req = EnvironmentSyncRequest {
                    source_env_id: src,
                    target_env_id: tgt,
                    mode: EnvironmentSyncMode::SelectedKeys,
                    keys: vec![key_clone.clone()],
                };
                run_environment_sync(locale, modal.clone(), reload.clone(), req, project_id);
            }
        })
    };

    Row {
        key: key_str.clone(),
        cells: vec![
            html! {
                <div class="flex items-center gap-1.5">
                    <span>{&key_str}</span>
                    {(entry.source_secret || entry.target_secret).then(|| html! {
                        <span class="inline-flex items-center px-1.5 py-0.5 rounded text-[10px] font-medium bg-neutral-200 text-neutral-700 dark:bg-neutral-700 dark:text-neutral-300">
                            {"🔒"}
                        </span>
                    })}
                </div>
            },
            render_diff_val_cell(
                locale,
                entry.source_present,
                entry.source_secret,
                &entry.source_value,
            ),
            render_diff_val_cell(
                locale,
                entry.target_present,
                entry.target_secret,
                &entry.target_value,
            ),
            render_diff_status_badge(locale, entry.status),
        ],
        actions: html! {
            if is_syncable {
                <ActionIconButton
                    variant={ActionVariant::Restore}
                    title={locale.t("projects.sync_this_key").to_string()}
                    onclick={on_sync_key}
                />
            }
        },
    }
}

fn render_diff_status_badge(locale: Locale, status: EnvDiffStatus) -> Html {
    let (label_key, badge_cls) = match status {
        EnvDiffStatus::Identical => (
            "projects.status_identical",
            "bg-green-100 text-green-800 dark:bg-green-900/30 dark:text-green-300",
        ),
        EnvDiffStatus::Different => (
            "projects.status_different",
            "bg-amber-100 text-amber-800 dark:bg-amber-900/30 dark:text-amber-300",
        ),
        EnvDiffStatus::MissingInTarget => (
            "projects.status_missing_target",
            "bg-orange-100 text-orange-800 dark:bg-orange-900/30 dark:text-orange-300",
        ),
        EnvDiffStatus::MissingInSource => (
            "projects.status_missing_source",
            "bg-blue-100 text-blue-800 dark:bg-blue-900/30 dark:text-blue-300",
        ),
    };

    html! {
        <span class={format!("inline-flex items-center px-2 py-0.5 rounded text-xs font-medium {badge_cls}")}>
            {locale.t(label_key)}
        </span>
    }
}

fn render_diff_val_cell(
    locale: Locale,
    present: bool,
    is_secret: bool,
    val_opt: &Option<String>,
) -> Html {
    if !present {
        return html! { <span class="text-neutral-400 italic">{"-"}</span> };
    }
    if is_secret {
        return html! {
            <span class="text-neutral-400 italic text-xs font-mono">
                {locale.t("projects.secret_masked")}
            </span>
        };
    }
    if let Some(val) = val_opt {
        html! { <span class="font-mono text-xs text-neutral-800 dark:text-neutral-200 break-all">{val}</span> }
    } else {
        html! { <span class="text-neutral-400 italic">{"-"}</span> }
    }
}

fn render_var_row(
    locale: Locale,
    idx: usize,
    row: &EnvVarRow,
    modal: UseStateHandle<Option<EnvModalState>>,
    state: EnvModalState,
) -> Html {
    let on_key = {
        let modal = modal.clone();
        let state = state.clone();
        Callback::from(move |e: InputEvent| {
            if let Some(target) = e.target_dyn_into::<HtmlInputElement>() {
                let mut updated = state.clone();
                if let Some(v) = updated.variables.get_mut(idx) {
                    v.key = target.value();
                }
                modal.set(Some(updated));
            }
        })
    };

    let on_val = {
        let modal = modal.clone();
        let state = state.clone();
        Callback::from(move |e: InputEvent| {
            if let Some(target) = e.target_dyn_into::<HtmlInputElement>() {
                let mut updated = state.clone();
                if let Some(v) = updated.variables.get_mut(idx) {
                    v.value = target.value();
                }
                modal.set(Some(updated));
            }
        })
    };

    let on_secret = {
        let modal = modal.clone();
        let state = state.clone();
        Callback::from(move |e: Event| {
            if let Some(target) = e.target_dyn_into::<HtmlInputElement>() {
                let mut updated = state.clone();
                if let Some(v) = updated.variables.get_mut(idx) {
                    v.secret = target.checked();
                }
                modal.set(Some(updated));
            }
        })
    };

    let on_del = {
        let modal = modal.clone();
        let state = state.clone();
        Callback::from(move |_| {
            let mut updated = state.clone();
            if idx < updated.variables.len() {
                updated.variables.remove(idx);
            }
            modal.set(Some(updated));
        })
    };

    html! {
        <div class="flex items-center gap-2">
            <div class="w-1/3">
                <input
                    type="text"
                    class={classes!(INPUT_CLASS, "font-mono")}
                    placeholder={locale.t("apps.env_key")}
                    value={row.key.clone()}
                    oninput={on_key}
                />
            </div>
            <div class="flex-1">
                <input
                    type={if row.secret { "password" } else { "text" }}
                    class={classes!(INPUT_CLASS, "font-mono")}
                    placeholder={locale.t("apps.env_value")}
                    value={row.value.clone()}
                    oninput={on_val}
                />
            </div>
            <label class="flex items-center gap-1 text-xs text-neutral-600 dark:text-neutral-400 whitespace-nowrap">
                <input
                    type="checkbox"
                    class="rounded border-neutral-300 dark:border-neutral-600 text-primary-600 focus:ring-primary-500"
                    checked={row.secret}
                    onchange={on_secret}
                />
                <span>{locale.t("apps.env_secret")}</span>
            </label>
            <ActionIconButton
                variant={ActionVariant::Delete}
                title={locale.t("common.delete").to_string()}
                onclick={on_del}
            />
        </div>
    }
}
