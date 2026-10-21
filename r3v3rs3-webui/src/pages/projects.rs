//! The page for managing Projects and Environments hierarchy.

use super::accounts::{INPUT_CLASS, LABEL_CLASS};
use super::resource_page::BUTTON_CLASS;
use super::settings::send_json;
use crate::API_ENDPOINT;
use crate::auth::use_ensure_auth;
use crate::components::data_list::{Column, DANGER_LINK_CLASS, Row, list_card};
use crate::dialog;
use crate::format::format_time;
use crate::i18n::use_locale;
use crate::store::SessionStore;
use gloo_net::http::Request;
use r3v3rs3_api::i18n::Locale;
use r3v3rs3_api::id::ShortId;
use r3v3rs3_api::platform::{EnvEntry, EnvironmentEnvSpec, Project, ProjectSpec, ProjectSummary};
use wasm_bindgen_futures::spawn_local;
use web_sys::HtmlInputElement;
use yew::prelude::*;
use yewdux::prelude::*;

const PATH: &str = "/projects";

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

#[derive(Clone, PartialEq, Default)]
struct EnvModalState {
    project_id: ShortId,
    env_id: ShortId,
    env_name: String,
    variables: Vec<EnvVarRow>,
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
            <div class="flex items-center justify-between">
                <div>
                    <h1 class="text-2xl font-bold tracking-tight text-neutral-900 dark:text-neutral-100">
                        {locale.t("projects.title")}
                    </h1>
                    <p class="text-sm text-neutral-500 dark:text-neutral-400">
                        {locale.t("projects.subtitle")}
                    </p>
                </div>
                {session.can_edit().then(|| html! {
                    <button
                        type="button"
                        class={BUTTON_CLASS}
                        onclick={on_new}
                    >
                        {locale.t("projects.new")}
                    </button>
                })}
            </div>

            {render_projects_table(locale, &list, &form, &reload, session.can_edit(), &env_modal)}
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
                let confirmed = dialog::confirm(
                    locale,
                    &format!("Are you sure you want to delete project {p_name}?"),
                )
                .await;
                if confirmed {
                    let url = format!("{API_ENDPOINT}{PATH}/{p_id}");
                    if let Ok(res) = Request::delete(&url).send().await
                        && res.ok()
                    {
                        reload.set(*reload + 1);
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
            <div class="flex items-center space-x-3 text-sm">
                <button type="button" class="text-primary-600 hover:underline dark:text-primary-400 font-medium" onclick={env_cb}>
                    {locale.t("projects.environments")}
                </button>
                <button type="button" class="text-neutral-600 hover:underline dark:text-neutral-400 font-medium" onclick={edit_cb}>
                    {locale.t("projects.edit")}
                </button>
                <button type="button" class={DANGER_LINK_CLASS} onclick={del_cb}>
                    {locale.t("projects.delete")}
                </button>
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
            modal.set(Some(EnvModalState {
                project_id,
                env_id: first_env.id,
                env_name: first_env.name.clone(),
                variables: vars.into_iter().map(EnvVarRow::from_entry).collect(),
            }));
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

    let on_cancel = {
        let form = form.clone();
        Callback::from(move |_| form.set(None))
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
                let success = if let Some(id) = data.editing_id {
                    let url = format!("{API_ENDPOINT}{PATH}/{id}");
                    send_json(locale, Request::put(&url), &spec).await.is_ok()
                } else {
                    let url = format!("{API_ENDPOINT}{PATH}");
                    send_json(locale, Request::post(&url), &spec).await.is_ok()
                };
                if success {
                    form.set(None);
                    reload.set(*reload + 1);
                }
            });
        })
    };

    html! {
        <div class="fixed inset-0 z-50 flex items-center justify-center bg-black/50 p-4">
            <div class="w-full max-w-md rounded-xl bg-white p-6 shadow-xl dark:bg-neutral-800">
                <h3 class="text-lg font-bold text-neutral-900 dark:text-neutral-100">{title}</h3>
                <form onsubmit={on_submit} class="mt-4 space-y-4">
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
                    {if !is_editing {
                        html! {
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
                    } else {
                        html!()
                    }}
                    <div class="flex justify-end space-x-3 pt-4">
                        <button type="button" class="btn btn-secondary" onclick={on_cancel}>
                            {locale.t("common.cancel")}
                        </button>
                        <button type="submit" class={BUTTON_CLASS}>
                            {if is_editing { locale.t("common.update") } else { locale.t("common.create") }}
                        </button>
                    </div>
                </form>
            </div>
        </div>
    }
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
        Callback::from(move |_| env_modal.set(None))
    };

    let on_add_var = {
        let env_modal = env_modal.clone();
        let state = state.clone();
        Callback::from(move |_| {
            let mut updated = state.clone();
            updated.variables.push(EnvVarRow::default());
            env_modal.set(Some(updated));
        })
    };

    let on_save = {
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
                    state.project_id, state.env_id
                );
                if send_json(locale, Request::put(&url), &spec).await.is_ok() {
                    env_modal.set(None);
                    reload.set(*reload + 1);
                }
            });
        })
    };

    html! {
        <div class="fixed inset-0 z-50 flex items-center justify-center bg-black/50 p-4">
            <div class="w-full max-w-2xl rounded-xl bg-white p-6 shadow-xl dark:bg-neutral-800 max-h-[85vh] flex flex-col">
                <div class="flex items-center justify-between border-b pb-3 dark:border-neutral-700">
                    <div>
                        <h3 class="text-lg font-bold text-neutral-900 dark:text-neutral-100">
                            {format!("{} - {}", locale.t("projects.env_vars"), state.env_name)}
                        </h3>
                        <p class="text-xs text-neutral-500">
                            {locale.t("projects.inherited_env_vars")}
                        </p>
                    </div>
                    <button type="button" class="text-neutral-400 hover:text-neutral-600" onclick={on_close.clone()}>
                        {"✕"}
                    </button>
                </div>

                <div class="flex-1 overflow-y-auto py-4 space-y-3">
                    {state.variables.iter().enumerate().map(|(idx, row)| {
                        render_var_row(idx, row, env_modal.clone(), state.clone())
                    }).collect::<Html>()}

                    <button
                        type="button"
                        class="text-xs text-primary-600 hover:underline font-semibold dark:text-primary-400 mt-2"
                        onclick={on_add_var}
                    >
                        {format!("+ {}", locale.t("env_groups.add_variable"))}
                    </button>
                </div>

                <div class="flex justify-end space-x-3 border-t pt-3 dark:border-neutral-700">
                    <button type="button" class="btn btn-secondary" onclick={on_close}>
                        {locale.t("common.cancel")}
                    </button>
                    <button type="button" class={BUTTON_CLASS} onclick={on_save}>
                        {locale.t("common.update")}
                    </button>
                </div>
            </div>
        </div>
    }
}

fn render_var_row(
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
        <div class="flex items-center space-x-2">
            <input
                type="text"
                class="input text-xs w-1/3"
                placeholder="KEY"
                value={row.key.clone()}
                oninput={on_key}
            />
            <input
                type={if row.secret { "password" } else { "text" }}
                class="input text-xs flex-1"
                placeholder="VALUE"
                value={row.value.clone()}
                oninput={on_val}
            />
            <label class="flex items-center space-x-1 text-xs text-neutral-600 dark:text-neutral-400">
                <input
                    type="checkbox"
                    checked={row.secret}
                    onchange={on_secret}
                />
                <span>{"Secret"}</span>
            </label>
            <button
                type="button"
                class="text-red-500 hover:text-red-700 text-xs px-2"
                onclick={on_del}
            >
                {"✕"}
            </button>
        </div>
    }
}
