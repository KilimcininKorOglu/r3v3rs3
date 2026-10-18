use super::Route;
use super::resource_page::{
    FormCard, Notice, delete_on_click, form_card, get_json, load, notice_view, reload_callback,
    row_actions, save_result, set_on_click, submit_callback, text_input,
};
use super::settings::{send_json, send_request};
use crate::API_ENDPOINT;
use crate::auth::use_ensure_auth;
use crate::components::data_list::{Column, Row, list_card};
use crate::i18n::use_locale;
use crate::store::{ProxyStore, SessionStore};
use gloo_net::http::Request;
use r3v3rs3_api::{
    auth::{
        AccountCreated, AccountInfo, AccountUpdate, MIN_PASSWORD_LENGTH, NewAccount, PasskeyInfo,
        PasskeyRegisterRequest, Role,
    },
    error::Error,
    i18n::Locale,
    id::ShortId,
    proxy::ProxyEntry,
};
use std::collections::BTreeSet;
use wasm_bindgen_futures::spawn_local;
use web_sys::{HtmlInputElement, HtmlSelectElement};
use yew::prelude::*;
use yew_router::prelude::*;
use yewdux::prelude::*;

#[cfg(target_arch = "wasm32")]
#[wasm_bindgen::prelude::wasm_bindgen(module = "/js/webauthn.js")]
extern "C" {
    #[wasm_bindgen(js_name = isWebAuthnSupported)]
    fn is_web_authn_supported() -> bool;
    #[wasm_bindgen(js_name = createPasskey)]
    fn create_passkey(
        challenge_b64: &str,
        rp_id: &str,
        rp_name: &str,
        user_id_str: &str,
        user_name: &str,
        user_display_name: &str,
    ) -> js_sys::Promise;
}

#[cfg(not(target_arch = "wasm32"))]
fn is_web_authn_supported() -> bool {
    false
}

pub(super) const INPUT_CLASS: &str = "bg-neutral-50 dark:text-neutral-200 dark:bg-neutral-800 dark:border-neutral-600 border border-neutral-300 text-neutral-900 text-sm rounded-lg focus:ring-blue-500 focus:border-blue-500 block w-full p-2.5 disabled:opacity-60";
pub(super) const LABEL_CLASS: &str =
    "block mt-4 mb-2 text-sm font-medium text-neutral-900 dark:text-neutral-200";
pub(super) const HINT_CLASS: &str = "mt-2 text-sm text-neutral-500 dark:text-neutral-400";
const CHECKBOX_CLASS: &str =
    "flex items-center mt-4 text-sm font-medium text-neutral-900 dark:text-neutral-200";

const PATH: &str = "/accounts";

const COLUMNS: [Column; 5] = [
    Column {
        label: "accounts.username",
        class: "whitespace-nowrap",
    },
    Column {
        label: "accounts.role",
        class: "whitespace-nowrap",
    },
    Column {
        label: "accounts.proxies",
        class: "",
    },
    Column {
        label: "accounts.totp",
        class: "w-0 whitespace-nowrap text-center",
    },
    Column {
        label: "accounts.passkeys",
        class: "w-0 whitespace-nowrap text-center",
    },
];

/// Each role with its value in the admin API and the translation key of its name.
const ROLES: [(Role, &str, &str); 3] = [
    (Role::Admin, "admin", "accounts.role_admin"),
    (Role::Editor, "editor", "accounts.role_editor"),
    (Role::Viewer, "viewer", "accounts.role_viewer"),
];

type AccountList = UseStateHandle<Option<Vec<AccountInfo>>>;

/// A saved account. A new account with TOTP shows its secret once.
type AccountNotice = UseStateHandle<Option<Notice<Option<String>>>>;

#[derive(Clone, PartialEq, Default)]
struct Form {
    /// The username of the edited account. `None` adds an account.
    editing: Option<String>,
    username: String,
    /// The password of a new account, or a new password. Empty keeps the password.
    password: String,
    role: Role,
    /// Whether the account sees only the selected proxies.
    restrict: bool,
    proxies: BTreeSet<ShortId>,
    totp: bool,
    passkeys: Vec<PasskeyInfo>,
}

impl Form {
    fn edit(info: &AccountInfo) -> Self {
        Self {
            editing: Some(info.username.clone()),
            username: info.username.clone(),
            password: String::new(),
            role: info.role,
            restrict: info.proxies.is_some(),
            proxies: info.proxies.clone().unwrap_or_default(),
            totp: info.totp,
            passkeys: info.passkeys.clone(),
        }
    }
}

/// The request that the form sends.
enum Change {
    Add(NewAccount),
    Update {
        username: String,
        update: AccountUpdate,
    },
}

#[function_component(Accounts)]
pub fn accounts() -> Html {
    use_ensure_auth();
    let locale = use_locale();
    let (session, _) = use_store::<SessionStore>();
    let (proxies, proxies_dispatch) = use_store::<ProxyStore>();
    let accounts = use_state(|| Option::<Vec<AccountInfo>>::None);
    let form = use_state(Form::default);
    let notice = use_state(|| Option::<Notice<Option<String>>>::None);

    let accounts_cloned = accounts.clone();
    use_effect_with((), move |_| {
        load(accounts_cloned, PATH);
        spawn_local(async move {
            if let Ok(entries) = get_json::<Vec<ProxyEntry>>("/proxies").await {
                proxies_dispatch.reduce(|state| {
                    ProxyStore {
                        entries,
                        loaded: true,
                        ..(*state).clone()
                    }
                    .into()
                });
            }
        });
    });

    use_effect_with(accounts.clone(), {
        let form = form.clone();
        move |accounts| {
            if let Some(editing) = &form.editing
                && let Some(list) = accounts.as_ref()
                && let Some(info) = list.iter().find(|acc| &acc.username == editing)
                && form.passkeys != info.passkeys
            {
                let mut updated = (*form).clone();
                updated.passkeys = info.passkeys.clone();
                form.set(updated);
            }
        }
    });

    if session.info.is_some() && !session.is_admin() {
        return html! { <Redirect<Route> to={Route::Home} /> };
    }

    let own = session
        .info
        .as_ref()
        .map(|info| info.username.clone())
        .unwrap_or_default();
    let rows = accounts
        .as_deref()
        .unwrap_or_default()
        .iter()
        .map(|info| account_row(locale, info, &proxies.entries, &own, &form, &accounts))
        .collect::<Vec<_>>();
    let onsubmit = submit(locale, &form, &notice, &accounts);
    html! {
        <>
            { list_card(locale, accounts.is_some(), "accounts.empty", &COLUMNS, &rows) }
            { form_view(locale, &form, &notice, &proxies.entries, &own, onsubmit, &accounts) }
        </>
    }
}

fn submit(
    locale: Locale,
    form: &UseStateHandle<Form>,
    notice: &AccountNotice,
    accounts: &AccountList,
) -> Callback<SubmitEvent> {
    let done = save_result(form, notice, reload_callback(accounts, PATH));
    let form = form.clone();
    submit_callback(
        move || parse_form(locale, &form).ok(),
        move |change| save(locale, change),
        done,
    )
}

fn account_row(
    locale: Locale,
    info: &AccountInfo,
    proxies: &[ProxyEntry],
    own: &str,
    form: &UseStateHandle<Form>,
    accounts: &AccountList,
) -> Row {
    // An admin cannot delete its own account.
    let delete = (info.username != own).then(|| {
        let question = locale.tf("accounts.confirm_delete", &[("username", &info.username)]);
        let path = format!("{PATH}/{}", path_segment(&info.username));
        delete_on_click(locale, question, path, reload_callback(accounts, PATH))
    });
    let totp = if info.totp { "common.yes" } else { "common.no" };
    let passkeys_text = if info.passkeys.is_empty() {
        "-".to_string()
    } else {
        info.passkeys.len().to_string()
    };
    Row {
        key: info.username.clone(),
        cells: vec![
            html! { <>{info.username.clone()}</> },
            html! { <>{locale.t(role_key(info.role))}</> },
            html! { <>{proxies_label(locale, info.proxies.as_ref(), proxies)}</> },
            html! { <>{locale.t(totp)}</> },
            html! { <>{passkeys_text}</> },
        ],
        actions: row_actions(locale, set_on_click(form, Form::edit(info)), delete),
    }
}

fn account_fields(
    locale: Locale,
    form: &UseStateHandle<Form>,
    own_account: bool,
    proxies: &[ProxyEntry],
    accounts: &AccountList,
) -> Html {
    let editing = form.editing.is_some();
    let password_label = if editing {
        "accounts.new_password"
    } else {
        "accounts.password"
    };
    html! {
        <>
            <label class={LABEL_CLASS}>{locale.t("accounts.username")}</label>
            <input type="text" autocapitalize="off" autocomplete="off" value={form.username.clone()} oninput={text_input(form, |f| &mut f.username)} disabled={editing} class={INPUT_CLASS} />
            <label class={LABEL_CLASS}>{locale.t(password_label)}</label>
            <input type="password" autocomplete="new-password" value={form.password.clone()} oninput={text_input(form, |f| &mut f.password)} class={INPUT_CLASS} />
            if editing {
                <p class={HINT_CLASS}>{locale.t("accounts.new_password_hint")}</p>
            }
            <label class={LABEL_CLASS}>{locale.t("accounts.role")}</label>
            { role_select(locale, form, own_account) }
            <p class={HINT_CLASS}>{locale.t("accounts.role_hint")}</p>
            if !form.role.is_admin() {
                { proxy_scope(locale, form, proxies) }
            }
            if !editing {
                { checkbox(form, locale.t("accounts.totp_enable"), |f| &mut f.totp) }
            }
            if editing {
                { passkey_list_view(locale, form, own_account, accounts) }
            }
        </>
    }
}

fn form_view(
    locale: Locale,
    form: &UseStateHandle<Form>,
    notice: &AccountNotice,
    proxies: &[ProxyEntry],
    own: &str,
    onsubmit: Callback<SubmitEvent>,
    accounts: &AccountList,
) -> Html {
    let parsed = parse_form(locale, form);
    let editing = form.editing.clone();
    let title = match &editing {
        Some(username) => locale.tf("accounts.edit_title", &[("username", username)]),
        None => locale.t("accounts.add_title").to_string(),
    };
    let touched = editing.is_some() || !form.username.is_empty() || !form.password.is_empty();
    let own_account = editing.as_deref() == Some(own);
    let fields = account_fields(locale, form, own_account, proxies, accounts);
    let notice = notice_view(notice, |secret: &Option<String>| {
        html! {
            <>
                <p>{locale.t("accounts.saved")}</p>
                if let Some(secret) = secret {
                    <p class="mt-2 break-all">{locale.tf("accounts.totp_secret", &[("secret", secret)])}</p>
                }
            </>
        }
    });
    let card = FormCard {
        title,
        notice,
        fields,
        error: parsed.as_ref().err().filter(|_| touched).cloned(),
        cancel: editing
            .is_some()
            .then(|| set_on_click(form, Form::default())),
        save_label: "accounts.save",
        can_save: parsed.is_ok(),
    };
    form_card(locale, card, onsubmit)
}

fn checkbox(
    form: &UseStateHandle<Form>,
    label: &'static str,
    select: fn(&mut Form) -> &mut bool,
) -> Html {
    let mut current = (**form).clone();
    let checked = *select(&mut current);
    let form = form.clone();
    let onchange = Callback::from(move |event: Event| {
        let input: HtmlInputElement = event.target_unchecked_into();
        let mut updated = (*form).clone();
        *select(&mut updated) = input.checked();
        form.set(updated);
    });
    checkbox_view(label.to_string(), checked, onchange)
}

fn checkbox_view(label: String, checked: bool, onchange: Callback<Event>) -> Html {
    html! {
        <label class={CHECKBOX_CLASS}>
            <input type="checkbox" {checked} {onchange} class="w-4 h-4 mr-2 rounded" />
            {label}
        </label>
    }
}

fn role_select(locale: Locale, form: &UseStateHandle<Form>, disabled: bool) -> Html {
    let current = form.role;
    let form = form.clone();
    let onchange = Callback::from(move |event: Event| {
        let select: HtmlSelectElement = event.target_unchecked_into();
        let mut updated = (*form).clone();
        updated.role = parse_role(&select.value());
        form.set(updated);
    });
    html! {
        <select {onchange} {disabled} class={INPUT_CLASS}>
            { for ROLES.iter().map(|(role, value, key)| html! {
                <option value={*value} selected={*role == current}>{locale.t(key)}</option>
            }) }
        </select>
    }
}

/// The option that limits the account to a proxy list, and the proxies of the list.
fn proxy_scope(locale: Locale, form: &UseStateHandle<Form>, proxies: &[ProxyEntry]) -> Html {
    html! {
        <>
            { checkbox(form, locale.t("accounts.restrict"), |f| &mut f.restrict) }
            <p class={HINT_CLASS}>{locale.t("accounts.restrict_hint")}</p>
            if form.restrict {
                <div class="mt-2 grid gap-x-4 sm:grid-cols-2">
                    { for proxies.iter().map(|entry| proxy_checkbox(form, entry)) }
                </div>
            }
        </>
    }
}

fn proxy_checkbox(form: &UseStateHandle<Form>, entry: &ProxyEntry) -> Html {
    let id = entry.id;
    let checked = form.proxies.contains(&id);
    let form = form.clone();
    let onchange = Callback::from(move |_: Event| {
        let mut updated = (*form).clone();
        if !updated.proxies.remove(&id) {
            updated.proxies.insert(id);
        }
        form.set(updated);
    });
    checkbox_view(proxy_name(entry), checked, onchange)
}

/// Checks the form. The error is a message in the selected language.
fn parse_form(locale: Locale, form: &Form) -> Result<Change, String> {
    let password = form.password.as_str();
    let too_short = || {
        locale.error_message(&Error::PasswordTooShort {
            min: MIN_PASSWORD_LENGTH,
        })
    };
    let short = password.chars().count() < MIN_PASSWORD_LENGTH;
    // An admin sees every proxy.
    let proxies = (!form.role.is_admin() && form.restrict).then(|| form.proxies.clone());
    match &form.editing {
        None if form.username.trim().is_empty() => {
            Err(locale.t("accounts.username_required").to_string())
        }
        None if short => Err(too_short()),
        None => Ok(Change::Add(NewAccount {
            username: form.username.trim().to_string(),
            password: password.to_string(),
            role: form.role,
            proxies,
            totp: form.totp,
        })),
        Some(_) if !password.is_empty() && short => Err(too_short()),
        Some(username) => Ok(Change::Update {
            username: username.clone(),
            update: AccountUpdate {
                role: form.role,
                proxies,
                password: (!password.is_empty()).then(|| password.to_string()),
            },
        }),
    }
}

/// The entry of `ROLES` that matches. No match is the role with the fewest rights.
fn find_role(matches: impl Fn(&(Role, &str, &str)) -> bool) -> (Role, &'static str, &'static str) {
    ROLES.into_iter().find(matches).unwrap_or(ROLES[2])
}

fn role_key(role: Role) -> &'static str {
    find_role(|(item, _, _)| *item == role).2
}

fn parse_role(value: &str) -> Role {
    find_role(|(_, item, _)| *item == value).0
}

/// The names of the proxies that the account sees.
fn proxies_label(
    locale: Locale,
    selected: Option<&BTreeSet<ShortId>>,
    proxies: &[ProxyEntry],
) -> String {
    let Some(selected) = selected else {
        return locale.t("accounts.all_proxies").to_string();
    };
    if selected.is_empty() {
        return locale.t("accounts.no_proxies").to_string();
    }
    selected
        .iter()
        .map(|id| {
            proxies
                .iter()
                .find(|entry| entry.id == *id)
                .map_or_else(|| id.to_string(), proxy_name)
        })
        .collect::<Vec<_>>()
        .join(", ")
}

fn proxy_name(entry: &ProxyEntry) -> String {
    if entry.proxy.name.is_empty() {
        entry.id.to_string()
    } else {
        entry.proxy.name.clone()
    }
}

/// Percent-encodes a username for a path segment.
fn path_segment(value: &str) -> String {
    value
        .bytes()
        .map(|byte| {
            if byte.is_ascii_alphanumeric() || b"-._~@".contains(&byte) {
                char::from(byte).to_string()
            } else {
                format!("%{byte:02X}")
            }
        })
        .collect()
}

/// Sends the change. A new account with TOTP returns its secret.
async fn save(locale: Locale, change: Change) -> Result<Option<String>, String> {
    match change {
        Change::Add(account) => {
            let request = Request::post(&format!("{API_ENDPOINT}{PATH}"))
                .json(&account)
                .map_err(|err| err.to_string())?;
            let created: AccountCreated = send_request(locale, request)
                .await?
                .json()
                .await
                .map_err(|err| err.to_string())?;
            Ok(created.totp_secret)
        }
        Change::Update { username, update } => {
            let url = format!("{API_ENDPOINT}{PATH}/{}", path_segment(&username));
            send_json(locale, Request::put(&url), &update)
                .await
                .map(|()| None)
        }
    }
}

fn passkey_list_view(
    locale: Locale,
    form: &UseStateHandle<Form>,
    own_account: bool,
    accounts: &AccountList,
) -> Html {
    let form_for_add = form.clone();
    let accounts_for_add = accounts.clone();
    let on_add_passkey = Callback::from(move |_| {
        let form = form_for_add.clone();
        let accounts = accounts_for_add.clone();
        spawn_local(async move {
            add_passkey_flow(locale, form, accounts).await;
        });
    });

    html! {
        <div class="mt-4 pt-4 border-t border-neutral-200 dark:border-neutral-700">
            <div class="flex justify-between items-center mb-2">
                <label class="text-sm font-medium text-neutral-900 dark:text-neutral-200">{locale.t("accounts.passkeys")}</label>
                if own_account && is_web_authn_supported() {
                    <button type="button" class="text-xs font-semibold px-2.5 py-1 rounded bg-neutral-800 text-white dark:bg-neutral-700 hover:bg-neutral-600 transition flex items-center gap-1 cursor-pointer" onclick={on_add_passkey}>
                        <svg class="w-3.5 h-3.5" fill="none" stroke="currentColor" viewBox="0 0 24 24"><path stroke-linecap="round" stroke-linejoin="round" stroke-width="2" d="M12 4v16m8-8H4"></path></svg>
                        {locale.t("accounts.add_passkey")}
                    </button>
                }
            </div>
            if form.passkeys.is_empty() {
                <p class={HINT_CLASS}>{locale.t("accounts.no_passkeys")}</p>
            } else {
                <div class="space-y-2 mt-2">
                    { for form.passkeys.iter().map(|pk| passkey_item(locale, pk, own_account, accounts)) }
                </div>
            }
        </div>
    }
}

fn passkey_item(
    locale: Locale,
    pk: &PasskeyInfo,
    own_account: bool,
    accounts: &AccountList,
) -> Html {
    let id = pk.id.clone();
    let accounts = accounts.clone();
    let on_delete = Callback::from(move |_| {
        let id = id.clone();
        let accounts = accounts.clone();
        spawn_local(async move {
            delete_passkey_action(locale, id, accounts).await;
        });
    });

    html! {
        <div class="flex justify-between items-center p-2 rounded border border-neutral-200 dark:border-neutral-700 bg-white dark:bg-neutral-800">
            <div class="flex items-center gap-2">
                <svg class="w-4 h-4 text-neutral-500" fill="none" stroke="currentColor" viewBox="0 0 24 24"><path stroke-linecap="round" stroke-linejoin="round" stroke-width="2" d="M15 7a2 2 0 012 2m4 0a6 6 0 01-7.743 5.743L11 17H9v2H7v2H4a1 1 0 01-1-1v-2.586a1 1 0 01.293-.707l5.964-5.964A6 6 0 1121 9z"></path></svg>
                <span class="text-sm font-medium text-neutral-800 dark:text-neutral-200">{&pk.name}</span>
            </div>
            if own_account {
                <button type="button" class="text-red-600 hover:text-red-800 dark:hover:text-red-400 p-1 cursor-pointer" onclick={on_delete}>
                    <svg class="w-4 h-4" fill="none" stroke="currentColor" viewBox="0 0 24 24"><path stroke-linecap="round" stroke-linejoin="round" stroke-width="2" d="M19 7l-.867 12.142A2 2 0 0116.138 21H7.862a2 2 0 01-1.995-1.858L5 7m5 4v6m4-6v6m1-10V4a1 1 0 00-1-1h-4a1 1 0 00-1 1v3M4 7h16"></path></svg>
                </button>
            }
        </div>
    }
}

async fn fetch_challenge() -> Result<r3v3rs3_api::auth::PasskeyChallenge, String> {
    let response = Request::get(&format!("{API_ENDPOINT}/auth/passkey/challenge"))
        .send()
        .await
        .map_err(|e| e.to_string())?;
    if !response.ok() {
        return Err("failed to get passkey challenge".into());
    }
    response
        .json::<r3v3rs3_api::auth::PasskeyChallenge>()
        .await
        .map_err(|e| e.to_string())
}

#[cfg(target_arch = "wasm32")]
fn js_error_message(err: wasm_bindgen::JsValue) -> String {
    if let Some(s) = err.as_string() {
        return s;
    }
    if let Ok(msg) = js_sys::Reflect::get(&err, &wasm_bindgen::JsValue::from_str("message"))
        && let Some(s) = msg.as_string()
    {
        return s;
    }
    "Passkey operation failed".to_string()
}

async fn register_passkey_request(
    locale: Locale,
    req: &PasskeyRegisterRequest,
) -> Result<(), String> {
    let post_res = Request::post(&format!("{API_ENDPOINT}/auth/passkey/register"))
        .json(req)
        .map_err(|err| err.to_string())?
        .send()
        .await
        .map_err(|err| err.to_string())?;
    if post_res.ok() {
        Ok(())
    } else {
        let msg = post_res
            .text()
            .await
            .unwrap_or_else(|_| locale.t("error.invalid_passkey").to_string());
        Err(msg)
    }
}

async fn add_passkey_flow(locale: Locale, form: UseStateHandle<Form>, accounts: AccountList) {
    let prompt_msg = locale.t("accounts.passkey_name_prompt");
    let Some(name) = crate::dialog::prompt(locale, prompt_msg, "MacBook Touch ID").await else {
        return;
    };
    let name = name.trim().to_string();
    if name.is_empty() {
        return;
    }

    let challenge = match fetch_challenge().await {
        Ok(c) => c,
        Err(e) => {
            crate::dialog::message(locale, &e, crate::dialog::Icon::Error).await;
            return;
        }
    };

    let username = form.username.clone();
    #[cfg(target_arch = "wasm32")]
    let reg_result = {
        let promise = create_passkey(
            &challenge.challenge,
            &challenge.rp_id,
            "r3v3rs3",
            &username,
            &username,
            &username,
        );
        match wasm_bindgen_futures::JsFuture::from(promise).await {
            Ok(js_val) => {
                let s = js_val.as_string().unwrap_or_default();
                serde_json::from_str::<serde_json::Value>(&s).ok()
            }
            Err(err) => {
                let msg = js_error_message(err);
                crate::dialog::message(locale, &msg, crate::dialog::Icon::Error).await;
                return;
            }
        }
    };
    #[cfg(not(target_arch = "wasm32"))]
    let reg_result: Option<serde_json::Value> = {
        let _ = (&challenge, &username);
        None
    };

    let Some(val) = reg_result else {
        return;
    };
    let req = PasskeyRegisterRequest {
        name,
        credential_id: val["credential_id"]
            .as_str()
            .unwrap_or_default()
            .to_string(),
        public_key: val["public_key"].as_str().unwrap_or_default().to_string(),
        client_data_json: val["client_data_json"].as_str().map(str::to_string),
    };

    match register_passkey_request(locale, &req).await {
        Ok(()) => load(accounts, PATH),
        Err(msg) => crate::dialog::message(locale, &msg, crate::dialog::Icon::Error).await,
    }
}

async fn delete_passkey_action(locale: Locale, id: String, accounts: AccountList) {
    if !crate::dialog::confirm(locale, locale.t("accounts.delete_passkey_confirm")).await {
        return;
    }
    let res = Request::delete(&format!("{API_ENDPOINT}/auth/passkey/{id}"))
        .send()
        .await;
    if let Ok(res) = res
        && res.ok()
    {
        load(accounts, PATH);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use r3v3rs3_api::proxy::Proxy;
    use serde_json::{json, to_value};

    fn web() -> ShortId {
        "web".parse().unwrap()
    }

    #[test]
    fn a_new_account_needs_a_username_and_a_long_password() {
        let form = Form {
            username: " editor ".into(),
            password: "editor-secret".into(),
            role: Role::Editor,
            restrict: true,
            proxies: BTreeSet::from([web()]),
            totp: true,
            ..Default::default()
        };
        let Ok(Change::Add(account)) = parse_form(Locale::En, &form) else {
            panic!("the form is not valid");
        };
        assert_eq!(
            to_value(&account).unwrap(),
            json!({"username": "editor", "password": "editor-secret", "role": "editor", "proxies": ["web"], "totp": true})
        );

        let without_name = Form {
            username: " ".into(),
            ..form.clone()
        };
        assert_eq!(
            parse_form(Locale::En, &without_name).err().as_deref(),
            Some(Locale::En.t("accounts.username_required"))
        );
        let short = Form {
            password: "short".into(),
            ..form
        };
        assert_eq!(
            parse_form(Locale::En, &short).err(),
            Some(Locale::En.error_message(&Error::PasswordTooShort {
                min: MIN_PASSWORD_LENGTH
            }))
        );
    }

    #[test]
    fn an_update_keeps_an_empty_password_and_an_admin_gets_no_proxy_list() {
        let info = AccountInfo {
            username: "admin".into(),
            role: Role::Admin,
            proxies: None,
            totp: false,
            passkeys: Vec::new(),
        };
        let form = Form {
            restrict: true,
            proxies: BTreeSet::from([web()]),
            ..Form::edit(&info)
        };
        let Ok(Change::Update { username, update }) = parse_form(Locale::En, &form) else {
            panic!("the form is not valid");
        };
        assert_eq!(username, "admin");
        assert_eq!(to_value(&update).unwrap(), json!({"role": "admin"}));

        let viewer = Form {
            role: Role::Viewer,
            password: "new-password".into(),
            ..form
        };
        let Ok(Change::Update { update, .. }) = parse_form(Locale::En, &viewer) else {
            panic!("the form is not valid");
        };
        assert_eq!(
            to_value(&update).unwrap(),
            json!({"role": "viewer", "proxies": ["web"], "password": "new-password"})
        );
    }

    #[test]
    fn the_list_names_the_proxies_and_the_path_encodes_the_username() {
        let entry = ProxyEntry {
            id: web(),
            proxy: Proxy {
                name: "Web".into(),
                ..Default::default()
            },
            source: None,
        };
        let entries = [entry];
        assert_eq!(proxies_label(Locale::En, None, &entries), "All proxies");
        assert_eq!(
            proxies_label(Locale::En, Some(&BTreeSet::new()), &entries),
            "No proxies"
        );
        let selected = BTreeSet::from([web()]);
        assert_eq!(proxies_label(Locale::En, Some(&selected), &entries), "Web");
        let unknown = BTreeSet::from(["other".parse().unwrap()]);
        assert_eq!(proxies_label(Locale::En, Some(&unknown), &entries), "other");

        assert_eq!(path_segment("a.b@c-d_e~1"), "a.b@c-d_e~1");
        assert_eq!(path_segment("a#b%ç"), "a%23b%25%C3%A7");
        assert_eq!(parse_role("editor"), Role::Editor);
        assert_eq!(parse_role("owner"), Role::Viewer);
        assert_eq!(role_key(Role::Admin), "accounts.role_admin");
    }
}
