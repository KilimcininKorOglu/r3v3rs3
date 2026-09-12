use super::http_proxy_config::{BUTTON_CLASS, HINT_CLASS, INPUT_CLASS, LABEL_CLASS};
use r3v3rs3_api::policy::{AuthPolicy, BasicAuth, BasicAuthUser};
use std::collections::HashSet;
use wasm_bindgen::{JsCast, UnwrapThrowExt};
use web_sys::{HtmlInputElement, HtmlSelectElement};
use yew::prelude::*;

#[derive(Clone, Copy, PartialEq)]
pub enum AuthKind {
    None,
    Basic,
}

impl AuthKind {
    const ALL: [AuthKind; 2] = [AuthKind::None, AuthKind::Basic];

    fn value(self) -> &'static str {
        match self {
            Self::None => "none",
            Self::Basic => "basic",
        }
    }

    fn label(self) -> &'static str {
        match self {
            Self::None => "None",
            Self::Basic => "Basic Auth",
        }
    }
}

#[derive(Clone, PartialEq)]
pub struct AuthForm {
    kind: AuthKind,
    realm: String,
    users: Vec<UserForm>,
}

#[derive(Clone, Default, PartialEq)]
struct UserForm {
    username: String,
    password: String,
    password_hash: String,
}

impl AuthForm {
    pub fn new(policy: &AuthPolicy) -> Self {
        match policy {
            AuthPolicy::None => Self {
                kind: AuthKind::None,
                realm: String::new(),
                users: vec![UserForm::default()],
            },
            AuthPolicy::Basic(basic) => Self {
                kind: AuthKind::Basic,
                realm: basic.realm.clone(),
                users: basic.users.iter().map(UserForm::new).collect(),
            },
        }
    }

    pub fn parse(&self) -> Result<AuthPolicy, String> {
        match self.kind {
            AuthKind::None => Ok(AuthPolicy::None),
            AuthKind::Basic => self.parse_basic().map(AuthPolicy::Basic),
        }
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
        let mut usernames = HashSet::new();
        if let Some(user) = users.iter().find(|user| !usernames.insert(&user.username)) {
            return Err(format!("Username {} is used more than once", user.username));
        }
        Ok(BasicAuth {
            realm: self.realm.trim().into(),
            users,
        })
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
        </>
    }
}

fn user_view(props: &Props, index: usize, user: &UserForm) -> Html {
    let placeholder = if user.password_hash.is_empty() {
        "Password"
    } else {
        "Unchanged"
    };
    let remove = form_update(props, move |form, _: MouseEvent| {
        if form.users.len() > 1 {
            form.users.remove(index);
        }
    });
    html! {
        <div class="grid grid-cols-[1fr_1fr_auto] gap-2 mt-2">
            <input type="text" autocapitalize="off" autocomplete="off" placeholder="Username" value={user.username.clone()} onchange={form_input(props, move |form, value| update_user(form, index, |user| user.username = value))} class={INPUT_CLASS} />
            <input type="password" autocomplete="new-password" {placeholder} value={user.password.clone()} onchange={form_input(props, move |form, value| update_user(form, index, |user| user.password = value))} class={INPUT_CLASS} />
            <button type="button" onclick={remove} disabled={props.form.users.len() <= 1} class={classes!(BUTTON_CLASS, "rounded-lg")}>
                <img src="/assets/icons/remove.svg" class="w-4 h-4" />
            </button>
        </div>
    }
}

fn add_user(props: &Props) -> Callback<MouseEvent> {
    form_update(props, |form, _| form.users.push(UserForm::default()))
}

fn update_user(form: &mut AuthForm, index: usize, update: impl FnOnce(&mut UserForm)) {
    if let Some(user) = form.users.get_mut(index) {
        update(user);
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
        }
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
