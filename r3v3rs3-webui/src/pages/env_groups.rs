//! The page of the environment variable groups shared across applications.

use super::Route;
use super::accounts::{INPUT_CLASS, LABEL_CLASS};
use super::resource_page::{
    AddIcon, FormCard, FormState, Notice, add_button, close_callback, delete_on_click, failed_view,
    form_modal, load, open_on_click, reload_callback, row_actions, save_result, saved_view,
    submit_callback, text_input, then_close, use_form_modal,
};
use super::settings::{fetch_json, send_json};
use crate::API_ENDPOINT;
use crate::auth::use_ensure_auth;
use crate::components::action_button::{ActionIconButton, ActionVariant};
use crate::components::button::{Button, ButtonKind, ButtonSize};
use crate::components::data_list::{Column, Row, list_card};
use crate::dialog::{self, Icon};
use crate::format::format_time;
use crate::i18n::use_locale;
use crate::store::SessionStore;
use gloo_net::http::Request;
use r3v3rs3_api::i18n::Locale;
use r3v3rs3_api::id::ShortId;
use r3v3rs3_api::platform::{EnvEntry, EnvGroup, EnvGroupSpec, EnvGroupSummary};
use wasm_bindgen_futures::spawn_local;
use web_sys::HtmlInputElement;
use yew::prelude::*;
use yew_router::prelude::*;
use yewdux::prelude::*;

const PATH: &str = "/env-groups";

const COLUMNS: [Column; 4] = [
    Column {
        label: "env_groups.name",
        class: "whitespace-nowrap font-medium text-neutral-900 dark:text-neutral-100",
    },
    Column {
        label: "env_groups.description",
        class: "whitespace-nowrap text-neutral-600 dark:text-neutral-400",
    },
    Column {
        label: "env_groups.variables",
        class: "whitespace-nowrap",
    },
    Column {
        label: "env_groups.last_updated",
        class: "whitespace-nowrap text-neutral-600 dark:text-neutral-400",
    },
];

type GroupsList = UseStateHandle<Option<Vec<EnvGroupSummary>>>;

#[derive(Clone, PartialEq, Default, Debug)]
struct EnvRow {
    key: String,
    value: String,
    secret: bool,
    stored_secret: bool,
}

impl EnvRow {
    fn from_entry(entry: EnvEntry) -> Self {
        Self {
            key: entry.key.as_str().to_string(),
            stored_secret: entry.secret && entry.value.is_none(),
            value: entry.value.unwrap_or_default(),
            secret: entry.secret,
        }
    }

    fn is_blank(&self) -> bool {
        self.key.trim().is_empty() && self.value.is_empty()
    }

    fn parse(&self, locale: Locale) -> Result<EnvEntry, String> {
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

#[derive(Clone, PartialEq, Default)]
struct Form {
    editing: Option<ShortId>,
    name: String,
    description: String,
    variables: Vec<EnvRow>,
}

impl Form {
    fn edit(group: &EnvGroup) -> Self {
        Self {
            editing: Some(group.id),
            name: group.name.clone(),
            description: group.description.clone().unwrap_or_default(),
            variables: group
                .variables
                .iter()
                .cloned()
                .map(EnvRow::from_entry)
                .collect(),
        }
    }

    fn parse(&self, locale: Locale) -> Result<EnvGroupSpec, String> {
        let name = self.name.trim();
        if name.is_empty() {
            return Err(locale.t("env_groups.invalid_name").to_string());
        }
        let description = if self.description.trim().is_empty() {
            None
        } else {
            Some(self.description.trim().to_string())
        };
        let mut variables = Vec::new();
        let mut seen = std::collections::HashSet::new();
        for row in &self.variables {
            if row.is_blank() {
                continue;
            }
            let entry = row.parse(locale)?;
            if !seen.insert(entry.key.as_str().to_string()) {
                return Err(locale.tf("env_groups.duplicate_key", &[("key", entry.key.as_str())]));
            }
            variables.push(entry);
        }
        Ok(EnvGroupSpec {
            name: name.to_string(),
            description,
            variables,
        })
    }
}

#[function_component(EnvGroups)]
pub fn env_groups() -> Html {
    use_ensure_auth();
    let locale = use_locale();
    let (session, _) = use_store::<SessionStore>();
    let groups = use_state(|| Option::<Vec<EnvGroupSummary>>::None);
    let (form, open) = use_form_modal::<Form>();
    let notice = use_state(|| Option::<Notice<()>>::None);

    let signed_in = session.info.is_some();
    let list_cloned = groups.clone();
    use_effect_with(signed_in, move |&signed_in| {
        if signed_in {
            load(list_cloned, PATH);
        }
        || ()
    });

    if signed_in && !session.can_read_platform() {
        return html! { <Redirect<Route> to={Route::Home} /> };
    }
    let can_edit = session.can_edit();
    let rows = groups
        .as_deref()
        .unwrap_or_default()
        .iter()
        .map(|entry| list_row(locale, entry, can_edit, (&form, &open), &groups))
        .collect::<Vec<_>>();

    html! {
        <>
            { saved_view(&notice, |()| html! { <p>{locale.t("env_groups.created")}</p> }) }
            { list_card(locale, groups.is_some(), "env_groups.empty", &COLUMNS, &rows) }
            if can_edit {
                { add_section(locale, (&form, &open), &notice, &groups) }
            }
        </>
    }
}

/// The add button and the modal of the form.
fn add_section(
    locale: Locale,
    (form, open): FormState<Form>,
    notice: &UseStateHandle<Option<Notice<()>>>,
    groups: &GroupsList,
) -> Html {
    html! {
        <>
            { add_button(locale, open_on_click(form, open, Form::default())) }
            if **open {
                { form_view(locale, (form, open), notice, groups) }
            }
        </>
    }
}

fn list_row(
    locale: Locale,
    entry: &EnvGroupSummary,
    can_edit: bool,
    (form, open): FormState<Form>,
    list: &GroupsList,
) -> Row {
    let actions = match can_edit {
        true => {
            let (form, open) = (form.clone(), open.clone());
            let id = entry.id;
            let edit = Callback::from(move |e: MouseEvent| {
                e.prevent_default();
                let (form, open) = (form.clone(), open.clone());
                spawn_local(async move {
                    let request = Request::get(&format!("{API_ENDPOINT}{PATH}/{id}"));
                    match fetch_json::<EnvGroup>(locale, request).await {
                        Ok(group) => {
                            form.set(Form::edit(&group));
                            open.set(true);
                        }
                        Err(message) => dialog::message(locale, &message, Icon::Error).await,
                    }
                });
            });
            let question = locale.tf("env_groups.delete_confirm", &[("name", &entry.name)]);
            let delete = delete_on_click(
                locale,
                question,
                format!("{PATH}/{}", entry.id),
                reload_callback(list, PATH),
            );
            row_actions(locale, edit, Some(delete))
        }
        false => html! {},
    };
    let desc = entry.description.as_deref().unwrap_or("-");
    let count = locale.tf(
        "env_groups.variable_count",
        &[("count", &entry.variable_count.to_string())],
    );
    Row {
        key: entry.id.to_string(),
        cells: vec![
            html! { {&entry.name} },
            html! { {desc} },
            html! { {count} },
            html! { {format_time(entry.updated_at)} },
        ],
        actions,
    }
}

fn form_view(
    locale: Locale,
    (form, open): FormState<Form>,
    notice: &UseStateHandle<Option<Notice<()>>>,
    groups: &GroupsList,
) -> Html {
    let parsed = form.parse(locale);
    let title = match form.editing {
        Some(_) => locale.t("env_groups.edit"),
        None => locale.t("env_groups.new"),
    };
    let touched = form.editing.is_some() || !form.name.is_empty();
    let card = FormCard {
        title: title.to_string(),
        notice: failed_view(notice),
        fields: fields_view(locale, form),
        error: parsed.as_ref().err().filter(|_| touched).cloned(),
        cancel: None,
        save_label: "env_groups.save",
        can_save: parsed.is_ok(),
    };
    form_modal(
        locale,
        card,
        submit(locale, (form, open), notice, groups),
        close_callback(form, open),
    )
}

fn fields_view(locale: Locale, form: &UseStateHandle<Form>) -> Html {
    let form_for_add = form.clone();
    let add_var = Callback::from(move |e: MouseEvent| {
        e.prevent_default();
        let mut updated = (*form_for_add).clone();
        updated.variables.push(EnvRow::default());
        form_for_add.set(updated);
    });

    html! {
        <>
            <label class={LABEL_CLASS}>{locale.t("env_groups.name")}</label>
            <input
                type="text"
                autocapitalize="off"
                autocomplete="off"
                value={form.name.clone()}
                oninput={text_input(form, |f| &mut f.name)}
                class={INPUT_CLASS}
            />
            <label class={LABEL_CLASS}>{locale.t("env_groups.description")}</label>
            <input
                type="text"
                autocapitalize="off"
                autocomplete="off"
                value={form.description.clone()}
                oninput={text_input(form, |f| &mut f.description)}
                class={INPUT_CLASS}
            />
            <div class="mt-4 pt-4 border-t border-neutral-200 dark:border-neutral-700">
                <div class="flex items-center justify-between mb-2">
                    <label class={LABEL_CLASS}>{locale.t("env_groups.variables")}</label>
                    <Button kind={ButtonKind::Secondary} size={ButtonSize::Sm} onclick={add_var}>
                        <AddIcon />
                        {locale.t("env_groups.add_variable")}
                    </Button>
                </div>
                { for (0..form.variables.len()).map(|idx| var_row_view(locale, form, idx)) }
            </div>
        </>
    }
}

fn var_row_view(locale: Locale, form: &UseStateHandle<Form>, index: usize) -> Html {
    let row = &form.variables[index];
    let placeholder = if row.secret && row.stored_secret {
        locale.t("apps.env_secret_unchanged")
    } else {
        ""
    };
    let val_type = if row.secret { "password" } else { "text" };
    let form_for_remove = form.clone();
    let remove_row = Callback::from(move |e: MouseEvent| {
        e.prevent_default();
        let mut updated = (*form_for_remove).clone();
        if index < updated.variables.len() {
            updated.variables.remove(index);
            form_for_remove.set(updated);
        }
    });

    html! {
        <div class="grid gap-2 mt-2 sm:grid-cols-[1fr_2fr_auto_auto] sm:items-center">
            <input
                type="text"
                autocapitalize="off"
                autocomplete="off"
                aria-label={locale.t("env_groups.key")}
                placeholder={locale.t("env_groups.key")}
                value={row.key.clone()}
                oninput={var_text_input(form, index, |r| &mut r.key)}
                class={INPUT_CLASS}
            />
            <input
                type={val_type}
                autocomplete="off"
                aria-label={locale.t("env_groups.value")}
                {placeholder}
                value={row.value.clone()}
                oninput={var_text_input(form, index, |r| &mut r.value)}
                class={INPUT_CLASS}
            />
            <label class="flex items-center text-sm text-neutral-900 dark:text-neutral-200">
                <input
                    type="checkbox"
                    checked={row.secret}
                    onchange={var_secret_toggle(form, index)}
                    class="w-4 h-4 mr-2 rounded"
                />
                {locale.t("env_groups.secret")}
            </label>
            <ActionIconButton
                variant={ActionVariant::Delete}
                title={locale.t("common.delete").to_string()}
                onclick={remove_row}
            />
        </div>
    }
}

fn var_text_input(
    form: &UseStateHandle<Form>,
    index: usize,
    select: fn(&mut EnvRow) -> &mut String,
) -> Callback<InputEvent> {
    let form = form.clone();
    Callback::from(move |event: InputEvent| {
        let input: HtmlInputElement = event.target_unchecked_into();
        let mut updated = (*form).clone();
        if let Some(row) = updated.variables.get_mut(index) {
            *select(row) = input.value();
            form.set(updated);
        }
    })
}

fn var_secret_toggle(form: &UseStateHandle<Form>, index: usize) -> Callback<Event> {
    let form = form.clone();
    Callback::from(move |event: Event| {
        let input: HtmlInputElement = event.target_unchecked_into();
        let mut updated = (*form).clone();
        if let Some(row) = updated.variables.get_mut(index) {
            row.secret = input.checked();
            form.set(updated);
        }
    })
}

fn submit(
    locale: Locale,
    (form, open): FormState<Form>,
    notice: &UseStateHandle<Option<Notice<()>>>,
    groups: &GroupsList,
) -> Callback<SubmitEvent> {
    let (form_cloned, locale_cloned) = (form.clone(), locale);
    let parse = move || form_cloned.parse(locale_cloned).ok();
    let editing = form.editing;
    let save = move |spec: EnvGroupSpec| async move {
        let path = match editing {
            Some(id) => format!("{PATH}/{id}"),
            None => PATH.to_string(),
        };
        let request = match editing {
            Some(_) => Request::put(&format!("{API_ENDPOINT}{path}")),
            None => Request::post(&format!("{API_ENDPOINT}{path}")),
        };
        send_json(locale, request, &spec).await
    };
    submit_callback(
        parse,
        save,
        save_result(
            form,
            notice,
            then_close(reload_callback(groups, PATH), open),
        ),
    )
}
