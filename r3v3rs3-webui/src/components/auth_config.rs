use super::http_proxy_config::{BUTTON_CLASS, HINT_CLASS, INPUT_CLASS, LABEL_CLASS};
use r3v3rs3_api::policy::{
    is_header_name, AuthPolicy, BasicAuth, BasicAuthUser, BearerAuth, BearerToken, ForwardAuth,
    DEFAULT_FORWARD_AUTH_TIMEOUT,
};
use r3v3rs3_api::proxy::ServerUrl;
use std::collections::HashSet;
use std::str::FromStr;
use std::time::Duration;

const MAX_FORWARD_AUTH_TIMEOUT_SECS: u64 = 300;
use wasm_bindgen::{JsCast, UnwrapThrowExt};
use web_sys::{HtmlInputElement, HtmlSelectElement};
use yew::prelude::*;

#[derive(Clone, Copy, PartialEq)]
pub enum AuthKind {
    None,
    Basic,
    Bearer,
    Forward,
}

impl AuthKind {
    const ALL: [AuthKind; 4] = [
        AuthKind::None,
        AuthKind::Basic,
        AuthKind::Bearer,
        AuthKind::Forward,
    ];

    fn value(self) -> &'static str {
        match self {
            Self::None => "none",
            Self::Basic => "basic",
            Self::Bearer => "bearer",
            Self::Forward => "forward",
        }
    }

    fn label(self) -> &'static str {
        match self {
            Self::None => "None",
            Self::Basic => "Basic Auth",
            Self::Bearer => "Bearer Token",
            Self::Forward => "Forward Auth",
        }
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
}

#[derive(Clone, Default, PartialEq)]
struct UserForm {
    username: String,
    password: String,
    password_hash: String,
}

#[derive(Clone, Default, PartialEq)]
struct TokenForm {
    name: String,
    token: String,
    token_hash: String,
}

impl AuthForm {
    pub fn new(policy: &AuthPolicy) -> Self {
        let mut form = Self {
            kind: AuthKind::None,
            realm: String::new(),
            users: vec![UserForm::default()],
            tokens: vec![TokenForm::default()],
            forward_url: String::new(),
            forward_headers: String::new(),
            forward_timeout: DEFAULT_FORWARD_AUTH_TIMEOUT.as_secs().to_string(),
        };
        match policy {
            AuthPolicy::None => {}
            AuthPolicy::Basic(basic) => {
                form.kind = AuthKind::Basic;
                form.realm = basic.realm.clone();
                form.users = basic.users.iter().map(UserForm::new).collect();
            }
            AuthPolicy::Bearer(bearer) => {
                form.kind = AuthKind::Bearer;
                form.tokens = bearer.tokens.iter().map(TokenForm::new).collect();
            }
            AuthPolicy::Forward(forward) => {
                form.kind = AuthKind::Forward;
                form.forward_url = forward.url.to_string();
                form.forward_headers = forward.response_headers.join(", ");
                form.forward_timeout = forward.timeout.as_secs().max(1).to_string();
            }
        }
        form
    }

    pub fn parse(&self) -> Result<AuthPolicy, String> {
        match self.kind {
            AuthKind::None => Ok(AuthPolicy::None),
            AuthKind::Basic => self.parse_basic().map(AuthPolicy::Basic),
            AuthKind::Bearer => self.parse_bearer().map(AuthPolicy::Bearer),
            AuthKind::Forward => self
                .parse_forward()
                .map(|forward| AuthPolicy::Forward(Box::new(forward))),
        }
    }

    fn parse_forward(&self) -> Result<ForwardAuth, String> {
        let url = ServerUrl::from_str(self.forward_url.trim()).map_err(|err| err.to_string())?;
        let response_headers = self
            .forward_headers
            .split(',')
            .map(str::trim)
            .filter(|name| !name.is_empty())
            .map(str::to_string)
            .collect::<Vec<_>>();
        if let Some(name) = response_headers.iter().find(|name| !is_header_name(name)) {
            return Err(format!("Invalid header name: {name}"));
        }
        let timeout = self
            .forward_timeout
            .trim()
            .parse::<u64>()
            .ok()
            .filter(|secs| (1..=MAX_FORWARD_AUTH_TIMEOUT_SECS).contains(secs))
            .ok_or_else(|| {
                format!(
                    "Timeout must be a whole number of seconds from 1 to {MAX_FORWARD_AUTH_TIMEOUT_SECS}"
                )
            })?;
        Ok(ForwardAuth {
            url,
            response_headers,
            timeout: Duration::from_secs(timeout),
        })
    }

    fn parse_bearer(&self) -> Result<BearerAuth, String> {
        let tokens = self
            .tokens
            .iter()
            .map(TokenForm::parse)
            .collect::<Result<Vec<_>, _>>()?;
        if tokens.is_empty() {
            return Err("Add at least one token".into());
        }
        if let Some(name) = duplicate(tokens.iter().map(|token| &token.name)) {
            return Err(format!("Token name {name} is used more than once"));
        }
        Ok(BearerAuth { tokens })
    }

    fn parse_basic(&self) -> Result<BasicAuth, String> {
        let users = self
            .users
            .iter()
            .map(UserForm::parse)
            .collect::<Result<Vec<_>, _>>()?;
        if users.is_empty() {
            return Err("Add at least one user".into());
        }
        if let Some(username) = duplicate(users.iter().map(|user| &user.username)) {
            return Err(format!("Username {username} is used more than once"));
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
            token_hash: token.token_hash.clone(),
        }
    }

    fn parse(&self) -> Result<BearerToken, String> {
        let token = BearerToken {
            name: self.name.trim().into(),
            token: self.token.trim().into(),
            token_hash: self.token_hash.clone(),
        };
        if token.name.is_empty() {
            return Err("Token name is required".into());
        }
        if token.token.is_empty() && token.token_hash.is_empty() {
            return Err(format!("Token is required for {}", token.name));
        }
        if !token.token.is_empty() && token.token.len() < BearerToken::MIN_LENGTH {
            return Err(format!(
                "Token for {} must be at least {} characters",
                token.name,
                BearerToken::MIN_LENGTH
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
            password_hash: user.password_hash.clone(),
        }
    }

    fn parse(&self) -> Result<BasicAuthUser, String> {
        let user = BasicAuthUser {
            username: self.username.trim().into(),
            password: self.password.clone(),
            password_hash: self.password_hash.clone(),
        };
        if let Some(err) = user.username_error() {
            return Err(err.into());
        }
        if user.password.is_empty() && user.password_hash.is_empty() {
            return Err(format!("Password is required for {}", user.username));
        }
        Ok(user)
    }
}

#[derive(Properties, PartialEq)]
pub struct Props {
    pub form: AuthForm,
    pub onchange: Callback<AuthForm>,
}

#[function_component(AuthConfig)]
pub fn auth_config(props: &Props) -> Html {
    let form = &props.form;
    html! {
        <>
            <select onchange={form_input(props, |form, value| form.kind = AuthKind::ALL.into_iter().find(|kind| kind.value() == value).unwrap_or(AuthKind::None))} class={classes!(INPUT_CLASS, "mt-2")}>
                { for AuthKind::ALL.iter().map(|kind| html! {
                    <option value={kind.value()} selected={*kind == form.kind}>{kind.label()}</option>
                }) }
            </select>
            if form.kind == AuthKind::Basic {
                <label class={LABEL_CLASS}>{"Realm"}</label>
                <input type="text" placeholder="r3v3rs3" value={form.realm.clone()} onchange={form_input(props, |form, value| form.realm = value)} class={INPUT_CLASS} />

                <label class={LABEL_CLASS}>{"Users"}</label>
                { for form.users.iter().enumerate().map(|(index, user)| user_view(props, index, user)) }
                <button type="button" onclick={add_user(props)} class={classes!(BUTTON_CLASS, "mt-2", "rounded-lg")}>{"Add User"}</button>
                <p class={HINT_CLASS}>{"Clients receive 401 Unauthorized until they send a valid username and password. Passwords are stored as argon2 hashes. Leave the password empty to keep the current password."}</p>
            }
            if form.kind == AuthKind::Bearer {
                <label class={LABEL_CLASS}>{"Tokens"}</label>
                { for form.tokens.iter().enumerate().map(|(index, token)| token_view(props, index, token)) }
                <button type="button" onclick={form_update(props, |form, _: MouseEvent| form.tokens.push(TokenForm::default()))} class={classes!(BUTTON_CLASS, "mt-2", "rounded-lg")}>{"Add Token"}</button>
                <p class={HINT_CLASS}>{"Clients receive 401 Unauthorized until they send Authorization: Bearer with a valid token. Use a random value of at least 16 characters, e.g. openssl rand -hex 32. Tokens are stored as SHA-256 digests. Leave the token empty to keep the current token."}</p>
            }
            if form.kind == AuthKind::Forward {
                <label class={LABEL_CLASS}>{"Auth URL"}</label>
                <input type="url" placeholder="http://127.0.0.1:4180/oauth2/auth" value={form.forward_url.clone()} onchange={form_input(props, |form, value| form.forward_url = value)} class={INPUT_CLASS} />

                <label class={LABEL_CLASS}>{"Copy Response Headers"}</label>
                <input type="text" autocapitalize="off" placeholder="X-Auth-Request-User, X-Auth-Request-Email" value={form.forward_headers.clone()} onchange={form_input(props, |form, value| form.forward_headers = value)} class={INPUT_CLASS} />

                <label class={LABEL_CLASS}>{"Timeout (Seconds)"}</label>
                <input type="number" min="1" max="300" value={form.forward_timeout.clone()} onchange={form_input(props, |form, value| form.forward_timeout = value)} class={INPUT_CLASS} />
                <p class={HINT_CLASS}>{"r3v3rs3 sends a GET request with the client headers and X-Forwarded-Method, X-Forwarded-Proto, X-Forwarded-Host, X-Forwarded-Uri and X-Forwarded-For to this URL. A 2xx response lets the request through and copies the listed headers to the upstream request. Any other response, for example a login redirect, is sent to the client."}</p>
            }
        </>
    }
}

fn user_view(props: &Props, index: usize, user: &UserForm) -> Html {
    let name = html! {
        <input type="text" autocapitalize="off" autocomplete="off" placeholder="Username" value={user.username.clone()} onchange={form_input(props, move |form, value| update_item(&mut form.users, index, |user| user.username = value))} class={INPUT_CLASS} />
    };
    let secret = html! {
        <input type="password" autocomplete="new-password" placeholder={secret_placeholder(&user.password_hash, "Password")} value={user.password.clone()} onchange={form_input(props, move |form, value| update_item(&mut form.users, index, |user| user.password = value))} class={INPUT_CLASS} />
    };
    let remove = form_update(props, move |form, _: MouseEvent| {
        remove_item(&mut form.users, index)
    });
    credential_row(name, secret, remove, props.form.users.len())
}

fn token_view(props: &Props, index: usize, token: &TokenForm) -> Html {
    let name = html! {
        <input type="text" autocapitalize="off" autocomplete="off" placeholder="Name" value={token.name.clone()} onchange={form_input(props, move |form, value| update_item(&mut form.tokens, index, |token| token.name = value))} class={INPUT_CLASS} />
    };
    let secret = html! {
        <input type="password" autocomplete="off" placeholder={secret_placeholder(&token.token_hash, "Token")} value={token.token.clone()} onchange={form_input(props, move |form, value| update_item(&mut form.tokens, index, |token| token.token = value))} class={INPUT_CLASS} />
    };
    let remove = form_update(props, move |form, _: MouseEvent| {
        remove_item(&mut form.tokens, index)
    });
    credential_row(name, secret, remove, props.form.tokens.len())
}

fn credential_row(name: Html, secret: Html, remove: Callback<MouseEvent>, rows: usize) -> Html {
    html! {
        <div class="grid grid-cols-[1fr_1fr_auto] gap-2 mt-2">
            { name }
            { secret }
            <button type="button" onclick={remove} disabled={rows <= 1} class={classes!(BUTTON_CLASS, "rounded-lg")}>
                <img src="/assets/icons/remove.svg" class="w-4 h-4" />
            </button>
        </div>
    }
}

fn secret_placeholder(hash: &str, label: &'static str) -> &'static str {
    if hash.is_empty() {
        label
    } else {
        "Unchanged"
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
        .parse()
        .unwrap();
        let AuthPolicy::Forward(forward) = &policy else {
            panic!("expected forward auth");
        };
        assert_eq!(
            forward.response_headers,
            vec!["X-Auth-User", "X-Auth-Email"]
        );
        assert_eq!(forward.timeout, Duration::from_secs(5));
        assert_eq!(AuthForm::new(&policy).parse().unwrap(), policy);

        assert!(form("not a url", "", "5").parse().is_err());
        assert!(form("http://127.0.0.1/auth", "X Auth", "5")
            .parse()
            .is_err());
        assert!(form("http://127.0.0.1/auth", "", "0").parse().is_err());
        assert!(form("http://127.0.0.1/auth", "", "301").parse().is_err());
    }

    #[test]
    fn bearer_form_validates_tokens() {
        let form = |tokens: Vec<TokenForm>| AuthForm {
            kind: AuthKind::Bearer,
            tokens,
            ..AuthForm::new(&AuthPolicy::None)
        };
        let token = |name: &str, token: &str, token_hash: &str| TokenForm {
            name: name.into(),
            token: token.into(),
            token_hash: token_hash.into(),
        };

        let AuthPolicy::Bearer(bearer) = form(vec![token(" ci ", "0123456789abcdef", "")])
            .parse()
            .unwrap()
        else {
            panic!("expected bearer auth");
        };
        assert_eq!(bearer.tokens[0].name, "ci");
        assert_eq!(bearer.tokens[0].token, "0123456789abcdef");
        assert!(form(vec![token("ci", "", "abc")]).parse().is_ok());

        assert!(form(vec![]).parse().is_err());
        assert!(form(vec![token("", "0123456789abcdef", "")])
            .parse()
            .is_err());
        assert!(form(vec![token("ci", "", "")]).parse().is_err());
        assert!(form(vec![token("ci", "short", "")]).parse().is_err());
        assert!(form(vec![token("ci", "", "a"), token("ci", "", "b")])
            .parse()
            .is_err());
    }

    fn user(username: &str, password: &str, password_hash: &str) -> UserForm {
        UserForm {
            username: username.into(),
            password: password.into(),
            password_hash: password_hash.into(),
        }
    }

    #[test]
    fn basic_form_round_trips_without_the_password() {
        let form = basic_form(vec![user(" alice ", "secret", "")]);
        let AuthPolicy::Basic(basic) = form.parse().unwrap() else {
            panic!("expected basic auth");
        };
        assert_eq!(basic.realm, "Staff");
        assert_eq!(basic.users[0].username, "alice");
        assert_eq!(basic.users[0].password, "secret");

        let sealed = BasicAuthUser {
            password: String::new(),
            password_hash: "$argon2id$hash".into(),
            ..basic.users[0].clone()
        };
        let form = AuthForm::new(&AuthPolicy::Basic(BasicAuth {
            users: vec![sealed.clone()],
            ..basic
        }));
        assert_eq!(form.users[0].password, "");
        let AuthPolicy::Basic(basic) = form.parse().unwrap() else {
            panic!("expected basic auth");
        };
        assert_eq!(basic.users, vec![sealed]);
    }

    #[test]
    fn basic_form_rejects_invalid_users() {
        assert!(basic_form(vec![]).parse().is_err());
        assert!(basic_form(vec![user("", "secret", "")]).parse().is_err());
        assert!(basic_form(vec![user("al:ice", "secret", "")])
            .parse()
            .is_err());
        assert!(basic_form(vec![user("alice", "", "")]).parse().is_err());
        assert!(
            basic_form(vec![user("alice", "a", ""), user("alice", "b", "")])
                .parse()
                .is_err()
        );
        assert!(AuthForm::new(&AuthPolicy::None).parse().unwrap().is_none());
    }
}
