use super::accounts::{HINT_CLASS, INPUT_CLASS, LABEL_CLASS};
use super::resource_page::{
    FormCard, Notice, delete_on_click, form_card, load, notice_view, reload_callback, row_actions,
    save_result, set_on_click, submit_callback, text_input,
};
use super::settings::send_json;
use crate::API_ENDPOINT;
use crate::auth::use_ensure_auth;
use crate::components::auth_config::{AuthConfig, AuthForm, AuthKind};
use crate::components::data_list::{Column, Row, list_card};
use crate::i18n::use_locale;
use crate::store::SessionStore;
use gloo_net::http::Request;
use r3v3rs3_api::{
    access_list::{AccessList, AccessListEntry},
    cidr::{format_cidr_list, parse_cidr_list},
    error::Error,
    i18n::Locale,
    id::ShortId,
    policy::{AuthPolicy, IpFilter},
};
use yew::prelude::*;
use yewdux::prelude::*;

const PATH: &str = "/access_lists";

const COLUMNS: [Column; 3] = [
    Column {
        label: "common.name",
        class: "whitespace-nowrap",
    },
    Column {
        label: "access_lists.ip_filter",
        class: "",
    },
    Column {
        label: "http_form.authentication",
        class: "whitespace-nowrap",
    },
];

type Lists = UseStateHandle<Option<Vec<AccessListEntry>>>;

#[derive(Clone, PartialEq)]
struct Form {
    /// The id of the edited list. `None` adds a list.
    editing: Option<ShortId>,
    name: String,
    allow: String,
    deny: String,
    auth: AuthForm,
}

impl Default for Form {
    fn default() -> Self {
        Self {
            editing: None,
            name: String::new(),
            allow: String::new(),
            deny: String::new(),
            auth: AuthForm::new(&AuthPolicy::None),
        }
    }
}

impl Form {
    fn edit(entry: &AccessListEntry) -> Self {
        let filter = &entry.list.ip_filter;
        Self {
            editing: Some(entry.id),
            name: entry.list.name.clone(),
            allow: format_cidr_list(&filter.allow),
            deny: format_cidr_list(&filter.deny),
            auth: AuthForm::new(&entry.list.auth),
        }
    }

    /// Returns the list, or the first error in the selected language.
    fn parse(&self, locale: Locale) -> Result<AccessList, String> {
        let name = self.name.trim();
        if name.is_empty() {
            return Err(locale.error_message(&Error::AccessListNameRequired));
        }
        let translated = |err: Error| locale.error_message(&err);
        let ip_filter = IpFilter {
            allow: parse_cidr_list(&self.allow).map_err(translated)?,
            deny: parse_cidr_list(&self.deny).map_err(translated)?,
        };
        Ok(AccessList {
            name: name.to_string(),
            ip_filter,
            auth: self.auth.parse(locale)?,
        })
    }
}

#[function_component(AccessLists)]
pub fn access_lists() -> Html {
    use_ensure_auth();
    let locale = use_locale();
    let (session, _) = use_store::<SessionStore>();
    let lists = use_state(|| Option::<Vec<AccessListEntry>>::None);
    let form = use_state(Form::default);
    let notice = use_state(|| Option::<Notice<()>>::None);

    let lists_cloned = lists.clone();
    use_effect_with((), move |_| load(lists_cloned, PATH));

    // The admin API rejects a change from an account that cannot change the ports.
    let can_edit = session.can_edit();
    let rows = lists
        .as_deref()
        .unwrap_or_default()
        .iter()
        .map(|entry| list_row(locale, entry, can_edit, &form, &lists))
        .collect::<Vec<_>>();
    html! {
        <>
            { list_card(locale, lists.is_some(), "access_lists.empty", &COLUMNS, &rows) }
            if can_edit {
                { form_view(locale, &form, &notice, &lists) }
            }
        </>
    }
}

fn list_row(
    locale: Locale,
    entry: &AccessListEntry,
    can_edit: bool,
    form: &UseStateHandle<Form>,
    lists: &Lists,
) -> Row {
    let actions = if can_edit {
        let question = locale.tf("access_lists.confirm_delete", &[("name", &entry.list.name)]);
        let path = format!("{PATH}/{}", entry.id);
        let delete = delete_on_click(locale, question, path, reload_callback(lists, PATH));
        row_actions(locale, set_on_click(form, Form::edit(entry)), Some(delete))
    } else {
        html! {}
    };
    Row {
        key: entry.id.to_string(),
        cells: vec![
            html! { <>{entry.list.name.clone()}</> },
            html! { <>{ip_filter_label(locale, &entry.list.ip_filter)}</> },
            html! { <>{AuthKind::of(&entry.list.auth).label(locale)}</> },
        ],
        actions,
    }
}

fn form_view(
    locale: Locale,
    form: &UseStateHandle<Form>,
    notice: &UseStateHandle<Option<Notice<()>>>,
    lists: &Lists,
) -> Html {
    let parsed = form.parse(locale);
    let title = match form.editing {
        Some(_) => locale.tf("access_lists.edit_title", &[("name", &form.name)]),
        None => locale.t("access_lists.add_title").to_string(),
    };
    let touched = form.editing.is_some() || !form.name.is_empty();
    let auth_onchange = {
        let form = form.clone();
        Callback::from(move |auth: AuthForm| {
            form.set(Form {
                auth,
                ..(*form).clone()
            })
        })
    };
    let fields = html! {
        <>
            <label class={LABEL_CLASS}>{locale.t("common.name")}</label>
            <input type="text" autocomplete="off" placeholder="Office" value={form.name.clone()} oninput={text_input(form, |f| &mut f.name)} class={INPUT_CLASS} />

            <label class={LABEL_CLASS}>{locale.t("http_form.allow")}</label>
            <input type="text" autocapitalize="off" placeholder="192.168.0.0/16" value={form.allow.clone()} oninput={text_input(form, |f| &mut f.allow)} class={INPUT_CLASS} />
            <p class={HINT_CLASS}>{locale.t("http_form.allow_hint")}</p>

            <label class={LABEL_CLASS}>{locale.t("http_form.deny")}</label>
            <input type="text" autocapitalize="off" placeholder="203.0.113.0/24" value={form.deny.clone()} oninput={text_input(form, |f| &mut f.deny)} class={INPUT_CLASS} />
            <p class={HINT_CLASS}>{locale.t("http_form.deny_hint")}</p>

            <label class={LABEL_CLASS}>{locale.t("http_form.authentication")}</label>
            <AuthConfig form={form.auth.clone()} onchange={auth_onchange} />
        </>
    };
    let card = FormCard {
        title,
        notice: notice_view(
            notice,
            |()| html! { <p>{locale.t("access_lists.saved")}</p> },
        ),
        fields,
        error: parsed.as_ref().err().filter(|_| touched).cloned(),
        cancel: form
            .editing
            .is_some()
            .then(|| set_on_click(form, Form::default())),
        save_label: "access_lists.save",
        can_save: parsed.is_ok(),
    };
    form_card(locale, card, submit(locale, form, notice, lists))
}

fn submit(
    locale: Locale,
    form: &UseStateHandle<Form>,
    notice: &UseStateHandle<Option<Notice<()>>>,
    lists: &Lists,
) -> Callback<SubmitEvent> {
    let done = save_result(form, notice, reload_callback(lists, PATH));
    let form = form.clone();
    submit_callback(
        move || form.parse(locale).ok().map(|list| (form.editing, list)),
        move |(editing, list)| save(locale, editing, list),
        done,
    )
}

/// The allowed and the denied address blocks of a filter.
fn ip_filter_label(locale: Locale, filter: &IpFilter) -> String {
    if filter.is_empty() {
        return locale.t("access_lists.all_addresses").to_string();
    }
    [
        ("access_lists.allowed", &filter.allow),
        ("access_lists.denied", &filter.deny),
    ]
    .into_iter()
    .filter(|(_, nets)| !nets.is_empty())
    .map(|(key, nets)| locale.tf(key, &[("list", &format_cidr_list(nets))]))
    .collect::<Vec<_>>()
    .join(" ")
}

/// Adds the list, or replaces the list with the id `editing`.
async fn save(locale: Locale, editing: Option<ShortId>, list: AccessList) -> Result<(), String> {
    let request = match editing {
        Some(id) => Request::put(&format!("{API_ENDPOINT}{PATH}/{id}")),
        None => Request::post(&format!("{API_ENDPOINT}{PATH}")),
    };
    send_json(locale, request, &list).await
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::{json, to_value};

    #[test]
    fn a_form_round_trips_a_list_and_rejects_an_invalid_one() {
        let form = Form {
            name: " Office ".into(),
            allow: "10.0.0.0/8, 192.168.1.0/24".into(),
            deny: "10.0.0.1".into(),
            ..Default::default()
        };
        let list = form.parse(Locale::En).unwrap();
        assert_eq!(
            to_value(&list).unwrap(),
            json!({"name": "Office", "ip_filter": {"allow": ["10.0.0.0/8", "192.168.1.0/24"], "deny": ["10.0.0.1/32"]}})
        );
        let entry = AccessListEntry {
            id: "office".parse().unwrap(),
            list: list.clone(),
        };
        let edited = Form::edit(&entry);
        assert_eq!(edited.editing, Some(entry.id));
        assert_eq!(edited.parse(Locale::En).unwrap(), list);

        let unnamed = Form {
            name: " ".into(),
            ..form.clone()
        };
        assert_eq!(
            unnamed.parse(Locale::Tr),
            Err(Locale::Tr.error_message(&Error::AccessListNameRequired))
        );
        let invalid = Form {
            allow: "10.0.0.0/33".into(),
            ..form
        };
        assert!(invalid.parse(Locale::En).is_err());
    }

    #[test]
    fn the_list_shows_the_address_blocks_and_the_authentication() {
        let filter = IpFilter {
            allow: parse_cidr_list("10.0.0.0/8").unwrap(),
            deny: parse_cidr_list("10.0.0.1/32, 10.0.0.2/32").unwrap(),
        };
        assert_eq!(
            ip_filter_label(Locale::En, &filter),
            "Allowed: 10.0.0.0/8 Denied: 10.0.0.1/32, 10.0.0.2/32"
        );
        assert_eq!(
            ip_filter_label(Locale::En, &IpFilter::default()),
            "All IP addresses"
        );
        assert_eq!(
            AuthKind::of(&AuthPolicy::Session).label(Locale::En),
            "Admin Session"
        );
    }
}
