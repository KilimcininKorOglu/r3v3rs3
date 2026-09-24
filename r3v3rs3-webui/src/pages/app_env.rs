//! The environment variables of an app. The admin API never returns the value of a secret
//! variable, so an unchanged secret row sends no value and keeps the stored one.

use super::accounts::{HINT_CLASS, INPUT_CLASS};
use super::resource_page::{FormCard, Notice, form_card, notice_view};
use super::settings::{fetch_json, send_json};
use crate::API_ENDPOINT;
use crate::components::data_list::{DANGER_LINK_CLASS, LINK_CLASS};
use gloo_net::http::Request;
use r3v3rs3_api::i18n::Locale;
use r3v3rs3_api::id::ShortId;
use r3v3rs3_api::platform::EnvEntry;
use wasm_bindgen_futures::spawn_local;
use web_sys::HtmlInputElement;
use yew::prelude::*;

#[derive(Clone, PartialEq, Default, Debug)]
struct EnvRow {
    key: String,
    value: String,
    secret: bool,
    /// Whether the admin API holds a secret value for the key. The row then keeps that value
    /// while its value input stays empty.
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

/// Checks every row that is not blank. The error is a message in the selected language.
fn parse_rows(locale: Locale, rows: &[EnvRow]) -> Result<Vec<EnvEntry>, String> {
    rows.iter()
        .filter(|row| !row.is_blank())
        .map(|row| row.parse(locale))
        .collect()
}

type Rows = UseStateHandle<Option<Vec<EnvRow>>>;

#[derive(Properties, PartialEq)]
pub struct Props {
    pub id: ShortId,
    pub locale: Locale,
}

#[function_component(AppEnv)]
pub fn app_env(props: &Props) -> Html {
    let locale = props.locale;
    let id = props.id;
    let rows = use_state(|| Option::<Vec<EnvRow>>::None);
    let notice = use_state(|| Option::<Notice<()>>::None);

    let (rows_cloned, notice_cloned) = (rows.clone(), notice.clone());
    use_effect_with(id, move |id| load(locale, *id, rows_cloned, notice_cloned));

    let Some(current) = (*rows).clone() else {
        return html! { { notice_view(&notice, |_: &()| html! {}) } };
    };
    let parsed = parse_rows(locale, &current);
    let onsubmit = submit(locale, id, &rows, &notice);
    let fields = html! {
        <>
            <p class={HINT_CLASS}>{locale.t("apps.env_hint")}</p>
            { for (0..current.len()).map(|index| row_view(locale, &rows, &current, index)) }
            <div class="mt-4">
                <a class={LINK_CLASS} onclick={add_row(&rows)}>{locale.t("apps.env_add")}</a>
            </div>
        </>
    };
    let card = FormCard {
        title: locale.t("apps.env").to_string(),
        notice: notice_view(&notice, |_: &()| html! { {locale.t("apps.env_saved")} }),
        fields,
        error: parsed.as_ref().err().cloned(),
        cancel: None,
        save_label: "apps.env_save",
        can_save: parsed.is_ok(),
    };
    form_card(locale, card, onsubmit)
}

fn row_view(locale: Locale, rows: &Rows, current: &[EnvRow], index: usize) -> Html {
    let row = &current[index];
    let placeholder = if row.secret && row.stored_secret {
        locale.t("apps.env_secret_unchanged")
    } else {
        ""
    };
    let value_type = if row.secret { "password" } else { "text" };
    html! {
        <div class="grid gap-2 mt-2 sm:grid-cols-[1fr_2fr_auto_auto] sm:items-center">
            <input type="text" autocapitalize="off" autocomplete="off" aria-label={locale.t("apps.env_key")} placeholder={locale.t("apps.env_key")} value={row.key.clone()} oninput={row_text(rows, index, |r| &mut r.key)} class={INPUT_CLASS} />
            <input type={value_type} autocomplete="off" aria-label={locale.t("apps.env_value")} {placeholder} value={row.value.clone()} oninput={row_text(rows, index, |r| &mut r.value)} class={INPUT_CLASS} />
            <label class="flex items-center text-sm text-neutral-900 dark:text-neutral-200">
                <input type="checkbox" checked={row.secret} onchange={row_secret(rows, index)} class="w-4 h-4 mr-2 rounded" />
                {locale.t("apps.env_secret")}
            </label>
            <a class={DANGER_LINK_CLASS} onclick={remove_row(rows, index)}>{locale.t("common.delete")}</a>
        </div>
    }
}

/// Changes one row of the list.
fn update_row(rows: &Rows, index: usize, change: impl Fn(&mut EnvRow) + 'static) -> impl Fn() {
    let rows = rows.clone();
    move || {
        let mut updated = (*rows).clone().unwrap_or_default();
        if let Some(row) = updated.get_mut(index) {
            change(row);
        }
        rows.set(Some(updated));
    }
}

fn row_text(
    rows: &Rows,
    index: usize,
    select: fn(&mut EnvRow) -> &mut String,
) -> Callback<InputEvent> {
    let rows = rows.clone();
    Callback::from(move |event: InputEvent| {
        let value = event.target_unchecked_into::<HtmlInputElement>().value();
        update_row(&rows, index, move |row| *select(row) = value.clone())();
    })
}

fn row_secret(rows: &Rows, index: usize) -> Callback<Event> {
    let rows = rows.clone();
    Callback::from(move |event: Event| {
        let checked = event.target_unchecked_into::<HtmlInputElement>().checked();
        update_row(&rows, index, move |row| row.secret = checked)();
    })
}

fn remove_row(rows: &Rows, index: usize) -> Callback<MouseEvent> {
    let rows = rows.clone();
    Callback::from(move |event: MouseEvent| {
        event.prevent_default();
        let mut updated = (*rows).clone().unwrap_or_default();
        if index < updated.len() {
            updated.remove(index);
        }
        rows.set(Some(updated));
    })
}

fn add_row(rows: &Rows) -> Callback<MouseEvent> {
    let rows = rows.clone();
    Callback::from(move |event: MouseEvent| {
        event.prevent_default();
        let mut updated = (*rows).clone().unwrap_or_default();
        updated.push(EnvRow::default());
        rows.set(Some(updated));
    })
}

fn submit(
    locale: Locale,
    id: ShortId,
    rows: &Rows,
    notice: &UseStateHandle<Option<Notice<()>>>,
) -> Callback<SubmitEvent> {
    let (rows, notice) = (rows.clone(), notice.clone());
    Callback::from(move |event: SubmitEvent| {
        event.prevent_default();
        let Ok(entries) = parse_rows(locale, (*rows).as_deref().unwrap_or_default()) else {
            return;
        };
        let (rows, notice) = (rows.clone(), notice.clone());
        spawn_local(async move {
            let path = format!("{API_ENDPOINT}/apps/{id}/env");
            match send_json(locale, Request::put(&path), &entries).await {
                Ok(()) => {
                    notice.set(Some(Notice::Saved(())));
                    load(locale, id, rows, notice);
                }
                Err(message) => notice.set(Some(Notice::Failed(message))),
            }
        });
    })
}

/// Reads the variables. A failed read shows its message.
fn load(locale: Locale, id: ShortId, rows: Rows, notice: UseStateHandle<Option<Notice<()>>>) {
    spawn_local(async move {
        let path = format!("{API_ENDPOINT}/apps/{id}/env");
        match fetch_json::<Vec<EnvEntry>>(locale, Request::get(&path)).await {
            Ok(entries) => rows.set(Some(entries.into_iter().map(EnvRow::from_entry).collect())),
            Err(message) => notice.set(Some(Notice::Failed(message))),
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::{json, to_value};

    fn row(key: &str, value: &str, secret: bool, stored_secret: bool) -> EnvRow {
        EnvRow {
            key: key.into(),
            value: value.into(),
            secret,
            stored_secret,
        }
    }

    #[test]
    fn an_unchanged_secret_keeps_its_stored_value() {
        let rows = [
            row(" MODE ", "production", false, false),
            row("TOKEN", "", true, true),
            row("", "", false, false),
        ];
        let entries = parse_rows(Locale::En, &rows).unwrap();
        assert_eq!(
            to_value(&entries).unwrap(),
            json!([
                {"key": "MODE", "value": "production", "secret": false},
                {"key": "TOKEN", "secret": true}
            ])
        );
    }

    #[test]
    fn a_row_without_a_stored_secret_needs_a_value() {
        let cases = [row("TOKEN", "", true, false), row("TOKEN", "", false, true)];
        for case in cases {
            assert_eq!(
                parse_rows(Locale::Tr, &[case]),
                Err(Locale::Tr.tf("apps.env_value_required", &[("key", "TOKEN")]))
            );
        }
        assert!(parse_rows(Locale::En, &[row("1BAD", "x", false, false)]).is_err());
    }

    #[test]
    fn a_secret_entry_of_the_admin_api_is_a_stored_secret() {
        let entry: EnvEntry =
            serde_json::from_value(json!({"key": "TOKEN", "secret": true})).unwrap();
        assert_eq!(EnvRow::from_entry(entry), row("TOKEN", "", true, true));
    }
}
