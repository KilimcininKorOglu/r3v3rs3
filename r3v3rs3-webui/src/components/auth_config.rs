use super::http_proxy_config::{BUTTON_CLASS, HINT_CLASS, INPUT_CLASS, LABEL_CLASS};
use crate::i18n::use_locale;
use r3v3rs3_api::i18n::Locale;
use r3v3rs3_api::policy::{
    AuthPolicy, BasicAuth, BasicAuthUser, BearerAuth, BearerToken, DEFAULT_FORWARD_AUTH_TIMEOUT,
    ForwardAuth, OidcAuth, default_oidc_scopes, is_header_name,
};
use r3v3rs3_api::proxy::ServerUrl;
use std::collections::HashSet;
use std::str::FromStr;
use std::time::Duration;

const MAX_FORWARD_AUTH_TIMEOUT_SECS: u64 = 300;
use wasm_bindgen::{JsCast, UnwrapThrowExt};
use web_sys::{HtmlInputElement, HtmlSelectElement};
use yew::prelude::*;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AuthKind {
    None,
    Basic,
    Bearer,
    Forward,
    Session,
    Oidc,
}

impl AuthKind {
    const ALL: [AuthKind; 6] = [
        AuthKind::None,
        AuthKind::Basic,
        AuthKind::Bearer,
        AuthKind::Forward,
        AuthKind::Session,
        AuthKind::Oidc,
    ];

    pub fn of(policy: &AuthPolicy) -> Self {
        match policy {
            AuthPolicy::None => Self::None,
            AuthPolicy::Basic(_) => Self::Basic,
            AuthPolicy::Bearer(_) => Self::Bearer,
            AuthPolicy::Forward(_) => Self::Forward,
            AuthPolicy::Session => Self::Session,
            AuthPolicy::Oidc(_) => Self::Oidc,
        }
    }

    fn value(self) -> &'static str {
        match self {
            Self::None => "none",
            Self::Basic => "basic",
            Self::Bearer => "bearer",
            Self::Forward => "forward",
            Self::Session => "session",
            Self::Oidc => "oidc",
        }
    }

    pub fn label(self, locale: Locale) -> &'static str {
        locale.t(match self {
            Self::None => "auth.kind_none",
            Self::Basic => "auth.kind_basic",
            Self::Bearer => "auth.kind_bearer",
            Self::Forward => "auth.kind_forward",
            Self::Session => "auth.kind_session",
            Self::Oidc => "auth.kind_oidc",
        })
    }
}

#[derive(Clone, PartialEq)]
pub struct AuthForm {
    kind: AuthKind,
    realm: String,
    users: Vec<UserForm>,
    tokens: Vec<TokenForm>,
    forward_url: String,
    forward_headers: String,
    forward_timeout: String,
    oidc_issuer_url: String,
    oidc_authorization_url: String,
    oidc_token_url: String,
    oidc_userinfo_url: String,
    oidc_client_id: String,
    oidc_client_secret: String,
    oidc_client_secret_set: bool,
    oidc_scopes: String,
    oidc_allowed_emails: String,
}

#[derive(Clone, Default, PartialEq)]
struct UserForm {
    username: String,
    password: String,
    /// The server keeps the current password when the form sends no new one.
    password_set: bool,
}

#[derive(Clone, Default, PartialEq)]
struct TokenForm {
    name: String,
    token: String,
    /// The server keeps the current token when the form sends no new one.
    token_set: bool,
}

impl AuthForm {
    pub fn new(policy: &AuthPolicy) -> Self {
        let mut form = Self {
            kind: AuthKind::of(policy),
            realm: String::new(),
            users: vec![UserForm::default()],
            tokens: vec![TokenForm::default()],
            forward_url: String::new(),
            forward_headers: String::new(),
            forward_timeout: DEFAULT_FORWARD_AUTH_TIMEOUT.as_secs().to_string(),
            oidc_issuer_url: String::new(),
            oidc_authorization_url: String::new(),
            oidc_token_url: String::new(),
            oidc_userinfo_url: String::new(),
            oidc_client_id: String::new(),
            oidc_client_secret: String::new(),
            oidc_client_secret_set: false,
            oidc_scopes: default_oidc_scopes().join(" "),
            oidc_allowed_emails: String::new(),
        };
        match policy {
            AuthPolicy::None | AuthPolicy::Session => {}
            AuthPolicy::Basic(basic) => {
                form.realm = basic.realm.clone();
                form.users = basic.users.iter().map(UserForm::new).collect();
            }
            AuthPolicy::Bearer(bearer) => {
                form.tokens = bearer.tokens.iter().map(TokenForm::new).collect();
            }
            AuthPolicy::Forward(forward) => {
                form.forward_url = forward.url.to_string();
                form.forward_headers = forward.response_headers.join(", ");
                form.forward_timeout = forward.timeout.as_secs().max(1).to_string();
            }
            AuthPolicy::Oidc(oidc) => {
                form.oidc_issuer_url = oidc.issuer_url.clone();
                form.oidc_authorization_url = oidc.authorization_url.clone();
                form.oidc_token_url = oidc.token_url.clone();
                form.oidc_userinfo_url = oidc.userinfo_url.clone();
                form.oidc_client_id = oidc.client_id.clone();
                form.oidc_client_secret_set =
                    oidc.client_secret_set || !oidc.client_secret.is_empty();
                form.oidc_scopes = oidc.scopes.join(" ");
                form.oidc_allowed_emails = oidc.allowed_emails.join(", ");
            }
        }
        form
    }

    /// Returns the policy, or the first error in the selected language.
    pub fn parse(&self, locale: Locale) -> Result<AuthPolicy, String> {
        match self.kind {
            AuthKind::None => Ok(AuthPolicy::None),
            AuthKind::Basic => self.parse_basic(locale).map(AuthPolicy::Basic),
            AuthKind::Bearer => self.parse_bearer(locale).map(AuthPolicy::Bearer),
            AuthKind::Forward => self
                .parse_forward(locale)
                .map(|forward| AuthPolicy::Forward(Box::new(forward))),
            AuthKind::Session => Ok(AuthPolicy::Session),
            AuthKind::Oidc => self
                .parse_oidc(locale)
                .map(|oidc| AuthPolicy::Oidc(Box::new(oidc))),
        }
    }

    fn parse_oidc(&self, locale: Locale) -> Result<OidcAuth, String> {
        let client_id = self.oidc_client_id.trim().to_string();
        if client_id.is_empty() {
            return Err(locale.t("auth.client_id_required").to_string());
        }
        let issuer_url = self.oidc_issuer_url.trim().to_string();
        let authorization_url = self.oidc_authorization_url.trim().to_string();
        let token_url = self.oidc_token_url.trim().to_string();
        let userinfo_url = self.oidc_userinfo_url.trim().to_string();
        if issuer_url.is_empty() && (authorization_url.is_empty() || token_url.is_empty()) {
            return Err(locale.t("auth.oidc_endpoints_required").to_string());
        }
        let scopes = if self.oidc_scopes.trim().is_empty() {
            default_oidc_scopes()
        } else {
            self.oidc_scopes
                .split_whitespace()
                .map(str::to_string)
                .collect()
        };
        let allowed_emails = self
            .oidc_allowed_emails
            .split(',')
            .map(str::trim)
            .filter(|e| !e.is_empty())
            .map(str::to_string)
            .collect();
        Ok(OidcAuth {
            issuer_url,
            authorization_url,
            token_url,
            userinfo_url,
            client_id,
            client_secret: self.oidc_client_secret.trim().to_string(),
            client_secret_set: self.oidc_client_secret_set,
            scopes,
            allowed_emails,
        })
    }

    fn parse_forward(&self, locale: Locale) -> Result<ForwardAuth, String> {
        let url = ServerUrl::from_str(self.forward_url.trim())
            .map_err(|err| locale.error_message(&err))?;
        let response_headers = self
            .forward_headers
            .split(',')
            .map(str::trim)
            .filter(|name| !name.is_empty())
            .map(str::to_string)
            .collect::<Vec<_>>();
        if let Some(name) = response_headers.iter().find(|name| !is_header_name(name)) {
            return Err(locale.tf("error.invalid_header_name", &[("name", name)]));
        }
        let timeout = self
            .forward_timeout
            .trim()
            .parse::<u64>()
            .ok()
            .filter(|secs| (1..=MAX_FORWARD_AUTH_TIMEOUT_SECS).contains(secs))
            .ok_or_else(|| {
                locale.tf(
                    "auth.timeout_range",
                    &[("max", &MAX_FORWARD_AUTH_TIMEOUT_SECS.to_string())],
                )
            })?;
        Ok(ForwardAuth {
            url,
            response_headers,
            timeout: Duration::from_secs(timeout),
        })
    }

    fn parse_bearer(&self, locale: Locale) -> Result<BearerAuth, String> {
        let tokens = self
            .tokens
            .iter()
            .map(|token| token.parse(locale))
            .collect::<Result<Vec<_>, _>>()?;
        if tokens.is_empty() {
            return Err(locale.t("auth.at_least_one_token").into());
        }
        if let Some(name) = duplicate(tokens.iter().map(|token| &token.name)) {
            return Err(locale.tf("auth.duplicate_token_name", &[("name", name)]));
        }
        Ok(BearerAuth { tokens })
    }

    fn parse_basic(&self, locale: Locale) -> Result<BasicAuth, String> {
        let users = self
            .users
            .iter()
            .map(|user| user.parse(locale))
            .collect::<Result<Vec<_>, _>>()?;
        if users.is_empty() {
            return Err(locale.t("auth.at_least_one_user").into());
        }
        if let Some(username) = duplicate(users.iter().map(|user| &user.username)) {
            return Err(locale.tf("auth.duplicate_username", &[("username", username)]));
        }
        Ok(BasicAuth {
            realm: self.realm.trim().into(),
            users,
        })
    }
}

fn duplicate<'a>(names: impl Iterator<Item = &'a String>) -> Option<&'a String> {
    let mut seen = HashSet::new();
    names.into_iter().find(|name| !seen.insert(*name))
}

impl TokenForm {
    fn new(token: &BearerToken) -> Self {
        Self {
            name: token.name.clone(),
            token: String::new(),
            token_set: token.token_set || !token.token_hash.is_empty(),
        }
    }

    fn parse(&self, locale: Locale) -> Result<BearerToken, String> {
        let token = BearerToken {
            name: self.name.trim().into(),
            token: self.token.trim().into(),
            token_hash: String::new(),
            token_set: self.token_set,
        };
        if token.name.is_empty() {
            return Err(locale.t("auth.token_name_required").into());
        }
        if token.token.is_empty() && !token.token_set {
            return Err(locale.tf("auth.token_required", &[("name", &token.name)]));
        }
        if !token.token.is_empty() && token.token.len() < BearerToken::MIN_LENGTH {
            return Err(locale.tf(
                "auth.token_too_short",
                &[
                    ("name", &token.name),
                    ("min", &BearerToken::MIN_LENGTH.to_string()),
                ],
            ));
        }
        Ok(token)
    }
}

impl UserForm {
    fn new(user: &BasicAuthUser) -> Self {
        Self {
            username: user.username.clone(),
            password: String::new(),
            password_set: user.password_set || !user.password_hash.is_empty(),
        }
    }

    fn parse(&self, locale: Locale) -> Result<BasicAuthUser, String> {
        let user = BasicAuthUser {
            username: self.username.trim().into(),
            password: self.password.clone(),
            password_hash: String::new(),
            password_set: self.password_set,
        };
        if let Some(err) = user.username_error() {
            return Err(username_error_message(locale, err).into());
        }
        if user.password.is_empty() && !user.password_set {
            return Err(locale.tf("auth.password_required", &[("username", &user.username)]));
        }
        Ok(user)
    }
}

/// Translates a message of `BasicAuthUser::username_error`. An unknown message stays as it is.
fn username_error_message(locale: Locale, message: &'static str) -> &'static str {
    match message {
        "Username is required" => locale.t("auth.username_required"),
        "Username must not contain a colon" => locale.t("auth.username_colon"),
        "Username must not contain control characters" => locale.t("auth.username_control"),
        _ => message,
    }
}

#[derive(Properties, PartialEq)]
pub struct Props {
    pub form: AuthForm,
    pub onchange: Callback<AuthForm>,
}

#[function_component(AuthConfig)]
pub fn auth_config(props: &Props) -> Html {
    let locale = use_locale();
    let form = &props.form;
    html! {
        <>
            <select onchange={form_input(props, |form, value| form.kind = AuthKind::ALL.into_iter().find(|kind| kind.value() == value).unwrap_or(AuthKind::None))} class={classes!(INPUT_CLASS, "mt-2")}>
                { for AuthKind::ALL.iter().map(|kind| html! {
                    <option value={kind.value()} selected={*kind == form.kind}>{kind.label(locale)}</option>
                }) }
            </select>
            if form.kind == AuthKind::Basic {
                <label class={LABEL_CLASS}>{locale.t("auth.realm")}</label>
                <input type="text" placeholder="r3v3rs3" value={form.realm.clone()} onchange={form_input(props, |form, value| form.realm = value)} class={INPUT_CLASS} />

                <label class={LABEL_CLASS}>{locale.t("auth.users")}</label>
                { for form.users.iter().enumerate().map(|(index, user)| user_view(locale, props, index, user)) }
                <button type="button" onclick={add_user(props)} class={classes!(BUTTON_CLASS, "mt-2", "rounded-lg")}>{locale.t("auth.add_user")}</button>
                <p class={HINT_CLASS}>{locale.t("auth.basic_hint")}</p>
            }
            if form.kind == AuthKind::Bearer {
                <label class={LABEL_CLASS}>{locale.t("auth.tokens")}</label>
                { for form.tokens.iter().enumerate().map(|(index, token)| token_view(locale, props, index, token)) }
                <button type="button" onclick={form_update(props, |form, _: MouseEvent| form.tokens.push(TokenForm::default()))} class={classes!(BUTTON_CLASS, "mt-2", "rounded-lg")}>{locale.t("auth.add_token")}</button>
                <p class={HINT_CLASS}>{locale.t("auth.bearer_hint")}</p>
            }
            if form.kind == AuthKind::Forward {
                <label class={LABEL_CLASS}>{locale.t("auth.forward_url")}</label>
                <input type="url" placeholder="http://127.0.0.1:4180/oauth2/auth" value={form.forward_url.clone()} onchange={form_input(props, |form, value| form.forward_url = value)} class={INPUT_CLASS} />

                <label class={LABEL_CLASS}>{locale.t("auth.copy_response_headers")}</label>
                <input type="text" autocapitalize="off" placeholder="X-Auth-Request-User, X-Auth-Request-Email" value={form.forward_headers.clone()} onchange={form_input(props, |form, value| form.forward_headers = value)} class={INPUT_CLASS} />

                <label class={LABEL_CLASS}>{locale.t("auth.timeout")}</label>
                <input type="number" min="1" max="300" value={form.forward_timeout.clone()} onchange={form_input(props, |form, value| form.forward_timeout = value)} class={INPUT_CLASS} />
                <p class={HINT_CLASS}>{locale.t("auth.forward_hint")}</p>
            }
            if form.kind == AuthKind::Session {
                <p class={HINT_CLASS}>{locale.t("auth.session_hint")}</p>
            }
            if form.kind == AuthKind::Oidc {
                <label class={LABEL_CLASS}>{locale.t("auth.issuer_url")}</label>
                <input type="url" placeholder="https://accounts.google.com" value={form.oidc_issuer_url.clone()} onchange={form_input(props, |form, value| form.oidc_issuer_url = value)} class={INPUT_CLASS} />

                <label class={LABEL_CLASS}>{locale.t("auth.client_id")}</label>
                <input type="text" autocapitalize="off" placeholder="your-client-id" value={form.oidc_client_id.clone()} onchange={form_input(props, |form, value| form.oidc_client_id = value)} class={INPUT_CLASS} />

                <label class={LABEL_CLASS}>{locale.t("auth.client_secret")}</label>
                <input type="password" autocomplete="new-password" placeholder={secret_placeholder(locale, form.oidc_client_secret_set, "auth.client_secret")} value={form.oidc_client_secret.clone()} onchange={form_input(props, |form, value| form.oidc_client_secret = value)} class={INPUT_CLASS} />

                <label class={LABEL_CLASS}>{locale.t("auth.allowed_emails")}</label>
                <input type="text" autocapitalize="off" placeholder="*@example.com, admin@corp.com" value={form.oidc_allowed_emails.clone()} onchange={form_input(props, |form, value| form.oidc_allowed_emails = value)} class={INPUT_CLASS} />

                <label class={LABEL_CLASS}>{locale.t("auth.scopes")}</label>
                <input type="text" autocapitalize="off" placeholder="openid email profile" value={form.oidc_scopes.clone()} onchange={form_input(props, |form, value| form.oidc_scopes = value)} class={INPUT_CLASS} />

                <label class={LABEL_CLASS}>{locale.t("auth.authorization_url")}</label>
                <input type="url" placeholder="https://.../authorize" value={form.oidc_authorization_url.clone()} onchange={form_input(props, |form, value| form.oidc_authorization_url = value)} class={INPUT_CLASS} />

                <label class={LABEL_CLASS}>{locale.t("auth.token_url")}</label>
                <input type="url" placeholder="https://.../token" value={form.oidc_token_url.clone()} onchange={form_input(props, |form, value| form.oidc_token_url = value)} class={INPUT_CLASS} />

                <label class={LABEL_CLASS}>{locale.t("auth.userinfo_url")}</label>
                <input type="url" placeholder="https://.../userinfo" value={form.oidc_userinfo_url.clone()} onchange={form_input(props, |form, value| form.oidc_userinfo_url = value)} class={INPUT_CLASS} />

                <p class={HINT_CLASS}>{locale.t("auth.oidc_hint")}</p>
            }
        </>
    }
}

fn user_view(locale: Locale, props: &Props, index: usize, user: &UserForm) -> Html {
    let name = html! {
        <input type="text" autocapitalize="off" autocomplete="off" placeholder={locale.t("login.username")} value={user.username.clone()} onchange={form_input(props, move |form, value| update_item(&mut form.users, index, |user| user.username = value))} class={INPUT_CLASS} />
    };
    let secret = html! {
        <input type="password" autocomplete="new-password" placeholder={secret_placeholder(locale, user.password_set, "login.password")} value={user.password.clone()} onchange={form_input(props, move |form, value| update_item(&mut form.users, index, |user| user.password = value))} class={INPUT_CLASS} />
    };
    let remove = form_update(props, move |form, _: MouseEvent| {
        remove_item(&mut form.users, index)
    });
    credential_row(name, secret, remove, props.form.users.len())
}

fn token_view(locale: Locale, props: &Props, index: usize, token: &TokenForm) -> Html {
    let name = html! {
        <input type="text" autocapitalize="off" autocomplete="off" placeholder={locale.t("common.name")} value={token.name.clone()} onchange={form_input(props, move |form, value| update_item(&mut form.tokens, index, |token| token.name = value))} class={INPUT_CLASS} />
    };
    let secret = html! {
        <input type="password" autocomplete="off" placeholder={secret_placeholder(locale, token.token_set, "auth.token")} value={token.token.clone()} onchange={form_input(props, move |form, value| update_item(&mut form.tokens, index, |token| token.token = value))} class={INPUT_CLASS} />
    };
    let remove = form_update(props, move |form, _: MouseEvent| {
        remove_item(&mut form.tokens, index)
    });
    credential_row(name, secret, remove, props.form.tokens.len())
}

fn credential_row(name: Html, secret: Html, remove: Callback<MouseEvent>, rows: usize) -> Html {
    html! {
        <div class="grid grid-cols-[1fr_auto] sm:grid-cols-[1fr_1fr_auto] gap-2 mt-2">
            <div class="col-span-2 sm:col-span-1">{ name }</div>
            { secret }
            <button type="button" onclick={remove} disabled={rows <= 1} class={classes!(BUTTON_CLASS, "rounded-lg")}>
                <img src="/assets/icons/remove.svg" class="w-4 h-4" />
            </button>
        </div>
    }
}

fn secret_placeholder(locale: Locale, set: bool, label_key: &'static str) -> &'static str {
    if set {
        locale.t("auth.unchanged")
    } else {
        locale.t(label_key)
    }
}

fn add_user(props: &Props) -> Callback<MouseEvent> {
    form_update(props, |form, _| form.users.push(UserForm::default()))
}

fn update_item<T>(items: &mut [T], index: usize, update: impl FnOnce(&mut T)) {
    if let Some(item) = items.get_mut(index) {
        update(item);
    }
}

fn remove_item<T>(items: &mut Vec<T>, index: usize) {
    if items.len() > 1 && index < items.len() {
        items.remove(index);
    }
}

/// Returns a callback that applies `update` to a copy of the form and emits the copy.
fn form_update<E: 'static>(
    props: &Props,
    update: impl Fn(&mut AuthForm, E) + 'static,
) -> Callback<E> {
    let form = props.form.clone();
    let onchange = props.onchange.clone();
    Callback::from(move |event: E| {
        let mut form = form.clone();
        update(&mut form, event);
        onchange.emit(form);
    })
}

fn form_input(props: &Props, update: impl Fn(&mut AuthForm, String) + 'static) -> Callback<Event> {
    form_update(props, move |form, event: Event| {
        update(form, event_value(&event))
    })
}

fn event_value(event: &Event) -> String {
    let target = event.target().unwrap_throw();
    match target.dyn_into::<HtmlInputElement>() {
        Ok(input) => input.value(),
        Err(target) => target.unchecked_into::<HtmlSelectElement>().value(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn basic_form(users: Vec<UserForm>) -> AuthForm {
        AuthForm {
            kind: AuthKind::Basic,
            realm: " Staff ".into(),
            users,
            ..AuthForm::new(&AuthPolicy::None)
        }
    }

    #[test]
    fn forward_form_validates_fields() {
        let form = |url: &str, headers: &str, timeout: &str| AuthForm {
            kind: AuthKind::Forward,
            forward_url: url.into(),
            forward_headers: headers.into(),
            forward_timeout: timeout.into(),
            ..AuthForm::new(&AuthPolicy::None)
        };

        let policy = form(
            "http://127.0.0.1:4180/auth",
            " X-Auth-User , ,X-Auth-Email",
            "5",
        )
        .parse(Locale::En)
        .unwrap();
        let AuthPolicy::Forward(forward) = &policy else {
            panic!("expected forward auth");
        };
        assert_eq!(
            forward.response_headers,
            vec!["X-Auth-User", "X-Auth-Email"]
        );
        assert_eq!(forward.timeout, Duration::from_secs(5));
        assert_eq!(AuthForm::new(&policy).parse(Locale::En).unwrap(), policy);

        assert!(form("not a url", "", "5").parse(Locale::En).is_err());
        assert_eq!(
            form("http://127.0.0.1/auth", "X Auth", "5").parse(Locale::Tr),
            Err(Locale::Tr.tf("error.invalid_header_name", &[("name", "X Auth")]))
        );
        assert!(
            form("http://127.0.0.1/auth", "", "0")
                .parse(Locale::En)
                .is_err()
        );
        assert!(
            form("http://127.0.0.1/auth", "", "301")
                .parse(Locale::En)
                .is_err()
        );
    }

    #[test]
    fn bearer_form_validates_tokens() {
        let form = |tokens: Vec<TokenForm>| AuthForm {
            kind: AuthKind::Bearer,
            tokens,
            ..AuthForm::new(&AuthPolicy::None)
        };
        let token = |name: &str, token: &str, token_set: bool| TokenForm {
            name: name.into(),
            token: token.into(),
            token_set,
        };

        let AuthPolicy::Bearer(bearer) = form(vec![token(" ci ", "0123456789abcdef", false)])
            .parse(Locale::En)
            .unwrap()
        else {
            panic!("expected bearer auth");
        };
        assert_eq!(bearer.tokens[0].name, "ci");
        assert_eq!(bearer.tokens[0].token, "0123456789abcdef");
        assert!(form(vec![token("ci", "", true)]).parse(Locale::En).is_ok());

        assert!(form(vec![]).parse(Locale::En).is_err());
        assert!(
            form(vec![token("", "0123456789abcdef", false)])
                .parse(Locale::En)
                .is_err()
        );
        assert!(
            form(vec![token("ci", "", false)])
                .parse(Locale::En)
                .is_err()
        );
        assert!(
            form(vec![token("ci", "short", false)])
                .parse(Locale::En)
                .is_err()
        );
        assert!(
            form(vec![token("ci", "", true), token("ci", "", true)])
                .parse(Locale::En)
                .is_err()
        );
    }

    #[test]
    fn session_form_round_trips() {
        let form = AuthForm::new(&AuthPolicy::Session);
        assert!(form.kind == AuthKind::Session);
        assert_eq!(form.parse(Locale::En).unwrap(), AuthPolicy::Session);
    }

    fn user(username: &str, password: &str) -> UserForm {
        UserForm {
            username: username.into(),
            password: password.into(),
            password_set: false,
        }
    }

    #[test]
    fn basic_form_round_trips_without_the_password() {
        let form = basic_form(vec![user(" alice ", "secret")]);
        let AuthPolicy::Basic(basic) = form.parse(Locale::En).unwrap() else {
            panic!("expected basic auth");
        };
        assert_eq!(basic.realm, "Staff");
        assert_eq!(basic.users[0].username, "alice");
        assert_eq!(basic.users[0].password, "secret");

        // The admin API returns a user with a password without its hash.
        let sealed = BasicAuthUser {
            password: String::new(),
            password_set: true,
            ..basic.users[0].clone()
        };
        let form = AuthForm::new(&AuthPolicy::Basic(BasicAuth {
            users: vec![sealed.clone()],
            ..basic
        }));
        assert_eq!(form.users[0].password, "");
        let AuthPolicy::Basic(basic) = form.parse(Locale::En).unwrap() else {
            panic!("expected basic auth");
        };
        assert_eq!(basic.users, vec![sealed]);
    }

    #[test]
    fn basic_form_rejects_invalid_users() {
        assert!(basic_form(vec![]).parse(Locale::En).is_err());
        assert!(
            basic_form(vec![user("", "secret")])
                .parse(Locale::En)
                .is_err()
        );
        assert!(
            basic_form(vec![user("alice", "")])
                .parse(Locale::En)
                .is_err()
        );
        assert!(
            basic_form(vec![user("alice", "a"), user("alice", "b")])
                .parse(Locale::En)
                .is_err()
        );
        assert!(
            AuthForm::new(&AuthPolicy::None)
                .parse(Locale::En)
                .unwrap()
                .is_none()
        );
    }

    #[test]
    fn oidc_form_round_trips_and_validates() {
        let oidc = OidcAuth {
            issuer_url: "https://accounts.google.com".into(),
            client_id: "google-client-id".into(),
            client_secret: "secret123".into(),
            client_secret_set: true,
            scopes: vec!["openid".into(), "email".into()],
            allowed_emails: vec!["*@example.com".into()],
            ..Default::default()
        };
        let form = AuthForm::new(&AuthPolicy::Oidc(Box::new(oidc.clone())));
        assert_eq!(form.kind, AuthKind::Oidc);
        assert_eq!(form.oidc_issuer_url, "https://accounts.google.com");
        assert_eq!(form.oidc_client_id, "google-client-id");
        assert!(form.oidc_client_secret_set);

        let parsed = form.parse(Locale::En).unwrap();
        let AuthPolicy::Oidc(parsed_oidc) = parsed else {
            panic!("expected oidc policy");
        };
        assert_eq!(parsed_oidc.issuer_url, oidc.issuer_url);
        assert_eq!(parsed_oidc.client_id, oidc.client_id);
        assert_eq!(parsed_oidc.scopes, oidc.scopes);
        assert_eq!(parsed_oidc.allowed_emails, oidc.allowed_emails);

        let invalid = AuthForm {
            oidc_client_id: "".into(),
            ..form.clone()
        };
        assert!(invalid.parse(Locale::En).is_err());
    }
}
