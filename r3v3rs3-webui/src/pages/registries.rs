//! The page of the container registry credentials for private image pulls.

use super::Route;
use super::accounts::{HINT_CLASS, INPUT_CLASS, LABEL_CLASS};
use super::resource_page::{
    FormCard, FormState, Notice, add_button, close_callback, delete_on_click, failed_view,
    form_modal, load, open_on_click, reload_callback, row_actions, save_result, saved_view,
    submit_callback, text_input, then_close, use_form_modal,
};
use super::settings::send_json;
use crate::API_ENDPOINT;
use crate::auth::use_ensure_auth;
use crate::components::data_list::{Column, Row, list_card};
use crate::i18n::use_locale;
use crate::store::SessionStore;
use gloo_net::http::Request;
use r3v3rs3_api::i18n::Locale;
use r3v3rs3_api::id::ShortId;
use r3v3rs3_api::platform::{RegistryEntry, RegistryRequest};
use yew::prelude::*;
use yew_router::prelude::*;
use yewdux::prelude::*;

const PATH: &str = "/registries";

const COLUMNS: [Column; 2] = [
    Column {
        label: "registries.server",
        class: "whitespace-nowrap font-medium text-neutral-900 dark:text-neutral-100",
    },
    Column {
        label: "registries.username",
        class: "whitespace-nowrap",
    },
];

type RegistriesList = UseStateHandle<Option<Vec<RegistryEntry>>>;

#[derive(Clone, PartialEq, Default)]
struct Form {
    editing: Option<ShortId>,
    server: String,
    username: String,
    password: String,
}

impl Form {
    fn edit(entry: &RegistryEntry) -> Self {
        Self {
            editing: Some(entry.id),
            server: entry.server.clone(),
            username: entry.username.clone(),
            password: String::new(),
        }
    }

    fn parse(&self, locale: Locale) -> Result<RegistryRequest, String> {
        let server = self.server.trim();
        if server.is_empty() {
            return Err(locale.t("registries.server_required").to_string());
        }
        let username = self.username.trim();
        if username.is_empty() {
            return Err(locale.t("registries.username_required").to_string());
        }
        let password = self.password.trim();
        if self.editing.is_none() && password.is_empty() {
            return Err(locale.t("registries.password_required").to_string());
        }
        let password = if password.is_empty() {
            None
        } else {
            Some(password.to_string())
        };
        Ok(RegistryRequest {
            server: server.to_string(),
            username: username.to_string(),
            password,
        })
    }
}

#[function_component(Registries)]
pub fn registries() -> Html {
    use_ensure_auth();
    let locale = use_locale();
    let (session, _) = use_store::<SessionStore>();
    let registries = use_state(|| Option::<Vec<RegistryEntry>>::None);
    let (form, open) = use_form_modal::<Form>();
    let notice = use_state(|| Option::<Notice<()>>::None);

    let signed_in = session.info.is_some();
    let list_cloned = registries.clone();
    use_effect_with(signed_in, move |&signed_in| {
        if signed_in {
            load(locale, list_cloned, PATH);
        }
        || ()
    });

    if signed_in && !session.can_read_platform() {
        return html! { <Redirect<Route> to={Route::Home} /> };
    }
    let can_edit = session.is_admin();
    let rows = registries
        .as_deref()
        .unwrap_or_default()
        .iter()
        .map(|entry| list_row(locale, entry, can_edit, (&form, &open), &registries))
        .collect::<Vec<_>>();

    html! {
        <>
            { saved_view(&notice, |()| html! { <p>{locale.t("registries.saved")}</p> }) }
            { list_card(locale, registries.is_some(), "registries.no_registries", &COLUMNS, &rows) }
            if can_edit {
                { add_button(locale, open_on_click(&form, &open, Form::default())) }
                if *open {
                    { form_view(locale, (&form, &open), &notice, &registries) }
                }
            }
        </>
    }
}

fn list_row(
    locale: Locale,
    entry: &RegistryEntry,
    can_edit: bool,
    (form, open): FormState<Form>,
    registries: &RegistriesList,
) -> Row {
    let actions = if can_edit {
        let question = locale.tf("registries.delete_confirm", &[("server", &entry.server)]);
        let path = format!("{PATH}/{}", entry.id);
        let delete = delete_on_click(
            locale,
            question,
            path,
            reload_callback(locale, registries, PATH),
        );
        row_actions(
            locale,
            open_on_click(form, open, Form::edit(entry)),
            Some(delete),
        )
    } else {
        html! {}
    };
    Row {
        key: entry.id.to_string(),
        cells: vec![
            html! { <>{entry.server.clone()}</> },
            html! { <>{entry.username.clone()}</> },
        ],
        actions,
    }
}

fn form_view(
    locale: Locale,
    (form, open): FormState<Form>,
    notice: &UseStateHandle<Option<Notice<()>>>,
    registries: &RegistriesList,
) -> Html {
    let parsed = form.parse(locale);
    let is_editing = form.editing.is_some();
    let title = if is_editing {
        locale.t("registries.edit").to_string()
    } else {
        locale.t("registries.add").to_string()
    };
    let touched = is_editing || !form.server.is_empty() || !form.username.is_empty();
    let fields = html! {
        <>
            <label class={LABEL_CLASS}>{locale.t("registries.server")}</label>
            <input
                type="text"
                autocomplete="off"
                placeholder={locale.t("registries.server_hint")}
                value={form.server.clone()}
                oninput={text_input(form, |f| &mut f.server)}
                class={INPUT_CLASS}
            />

            <label class={LABEL_CLASS}>{locale.t("registries.username")}</label>
            <input
                type="text"
                autocomplete="off"
                value={form.username.clone()}
                oninput={text_input(form, |f| &mut f.username)}
                class={INPUT_CLASS}
            />

            <label class={LABEL_CLASS}>{locale.t("registries.password")}</label>
            <input
                type="password"
                autocomplete="new-password"
                value={form.password.clone()}
                oninput={text_input(form, |f| &mut f.password)}
                class={INPUT_CLASS}
            />
            if is_editing {
                <p class={HINT_CLASS}>{locale.t("registries.password_hint")}</p>
            }
        </>
    };

    let card = FormCard {
        title,
        notice: failed_view(notice),
        fields,
        error: parsed.as_ref().err().filter(|_| touched).cloned(),
        cancel: None,
        save_label: if is_editing {
            "common.update"
        } else {
            "registries.add"
        },
        can_save: parsed.is_ok(),
    };

    let onsubmit = submit(locale, (form, open), notice, registries);
    form_modal(locale, card, onsubmit, close_callback(form, open))
}

fn submit(
    locale: Locale,
    (form, open): FormState<Form>,
    notice: &UseStateHandle<Option<Notice<()>>>,
    registries: &RegistriesList,
) -> Callback<SubmitEvent> {
    let saved = then_close(reload_callback(locale, registries, PATH), open);
    let done = save_result(form, notice, saved);
    let form = form.clone();
    submit_callback(
        move || {
            form.parse(locale)
                .ok()
                .map(|request| (form.editing, request))
        },
        move |(editing, request)| save(locale, editing, request),
        done,
    )
}

async fn save(
    locale: Locale,
    editing: Option<ShortId>,
    request: RegistryRequest,
) -> Result<(), String> {
    let builder = match editing {
        Some(id) => Request::put(&format!("{API_ENDPOINT}{PATH}/{id}")),
        None => Request::post(&format!("{API_ENDPOINT}{PATH}")),
    };
    send_json(locale, builder, &request).await
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_validates_required_fields() {
        let locale = Locale::En;
        let mut form = Form::default();

        assert!(form.parse(locale).is_err());

        form.server = "ghcr.io".into();
        assert!(form.parse(locale).is_err());

        form.username = "octocat".into();
        assert!(form.parse(locale).is_err());

        form.password = "secret".into();
        let req = form.parse(locale).unwrap();
        assert_eq!(req.server, "ghcr.io");
        assert_eq!(req.username, "octocat");
        assert_eq!(req.password.as_deref(), Some("secret"));

        // When editing, password can be empty
        form.editing = Some(ShortId::new());
        form.password = "".into();
        let edit_req = form.parse(locale).unwrap();
        assert!(edit_req.password.is_none());
    }
}
