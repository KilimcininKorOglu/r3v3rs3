//! The Git provider connections of the deployment platform. An admin creates a GitHub App from
//! this page, or registers an OAuth application at GitLab or Gitea, adds it here and connects it.
//! The app form then lists the repositories of the connected account.

use super::Route;
use super::accounts::{HINT_CLASS, INPUT_CLASS, LABEL_CLASS};
use super::github_app;
use super::resource_page::{
    FormCard, Notice, delete_on_click, form_card, load, notice_view, reload_callback, save_result,
    set_on_click, submit_callback, text_input,
};
use super::settings::{failure_box, fetch_json, send_json, success_box};
use super::targets::copy_box;
use crate::API_ENDPOINT;
use crate::auth::use_ensure_auth;
use crate::components::data_list::{
    Column, DANGER_LINK_CLASS, LINK_CLASS, Row, list_card, status_badge,
};
use crate::dialog::{self, Icon};
use crate::i18n::use_locale;
use crate::store::SessionStore;
use gloo_net::http::Request;
use r3v3rs3_api::git_connection::{
    AuthorizeRequest, AuthorizeResponse, ConnectionStatus, GITHUB_URL, GitConnectionEntry,
    GitConnectionRequest, GitProvider, GithubAppRequest, OAUTH_CALLBACK_PATH, PlatformSettings,
};
use r3v3rs3_api::i18n::Locale;
use r3v3rs3_api::id::ShortId;
use web_sys::HtmlSelectElement;
use yew::prelude::*;
use yew_router::prelude::*;
use yewdux::prelude::*;

const PATH: &str = "/git/connections";

const SETTINGS_PATH: &str = "/platform/settings";

/// The callback errors that the server sends back in the `error` parameter.
const CALLBACK_ERRORS: [&str; 5] = [
    "git_authorization_refused",
    "oauth_state_invalid",
    "git_provider_failed",
    "id_not_found",
    "platform_failed",
];

const PROVIDERS: [(GitProvider, &str); 3] = [
    (GitProvider::Github, "GitHub"),
    (GitProvider::Gitlab, "GitLab"),
    (GitProvider::Gitea, "Gitea"),
];

const COLUMNS: [Column; 5] = [
    Column {
        label: "common.name",
        class: "whitespace-nowrap",
    },
    Column {
        label: "git_connections.provider",
        class: "whitespace-nowrap",
    },
    Column {
        label: "git_connections.address",
        class: "",
    },
    Column {
        label: "common.status",
        class: "whitespace-nowrap",
    },
    Column {
        label: "git_connections.account",
        class: "whitespace-nowrap",
    },
];

type Connections = UseStateHandle<Option<Vec<GitConnectionEntry>>>;

/// The result of the installation of a GitHub App that GitHub returned from.
type Installation = UseStateHandle<Option<Result<(), String>>>;

/// What a valid form sends.
#[derive(Debug, PartialEq)]
enum Change {
    /// Adds a connection, or replaces the connection with the id.
    Connection(Option<ShortId>, GitConnectionRequest),
    /// Starts the creation of a GitHub App.
    GithubApp(GithubAppRequest),
}

#[derive(Clone, PartialEq)]
struct Form {
    /// The id of the edited connection. `None` adds a connection.
    editing: Option<ShortId>,
    name: String,
    provider: GitProvider,
    url: String,
    /// The organization that owns a new GitHub App.
    organization: String,
    client_id: String,
    client_secret: String,
}

impl Default for Form {
    fn default() -> Self {
        Self {
            editing: None,
            name: String::new(),
            provider: GitProvider::Github,
            url: String::new(),
            organization: String::new(),
            client_id: String::new(),
            client_secret: String::new(),
        }
    }
}

impl Form {
    fn edit(entry: &GitConnectionEntry) -> Self {
        Self {
            editing: Some(entry.id),
            name: entry.name.to_string(),
            provider: entry.provider,
            url: entry.url.to_string(),
            organization: String::new(),
            client_id: entry.client_id.clone(),
            client_secret: String::new(),
        }
    }

    /// The change, or the first error in the selected language. A new GitHub connection creates
    /// a GitHub App that returns to `callback`, and an existing one changes only its name.
    fn parse(&self, locale: Locale, callback: &str) -> Result<Change, String> {
        match (self.provider, self.editing) {
            (GitProvider::Github, None) => self.github_app(locale, callback).map(Change::GithubApp),
            (GitProvider::Github, Some(id)) => {
                let request = GitConnectionRequest {
                    name: self
                        .name
                        .trim()
                        .parse()
                        .map_err(|err| locale.error_message(&err))?,
                    provider: GitProvider::Github,
                    url: None,
                    client_id: self.client_id.clone(),
                    client_secret: None,
                };
                Ok(Change::Connection(Some(id), request))
            }
            _ => self
                .oauth_application(locale)
                .map(|request| Change::Connection(self.editing, request)),
        }
    }

    fn github_app(&self, locale: Locale, callback: &str) -> Result<GithubAppRequest, String> {
        let translated = |err| locale.error_message(&err);
        Ok(GithubAppRequest {
            name: self.name.trim().parse().map_err(translated)?,
            url: non_empty(&self.url)
                .map(str::parse)
                .transpose()
                .map_err(translated)?,
            organization: non_empty(&self.organization)
                .map(str::parse)
                .transpose()
                .map_err(translated)?,
            redirect_uri: callback.parse().map_err(translated)?,
        })
    }

    /// The OAuth application of a GitLab or Gitea connection.
    fn oauth_application(&self, locale: Locale) -> Result<GitConnectionRequest, String> {
        let translated = |err| locale.error_message(&err);
        let url = non_empty(&self.url)
            .map(str::parse)
            .transpose()
            .map_err(translated)?;
        let request = GitConnectionRequest {
            name: self.name.trim().parse().map_err(translated)?,
            provider: self.provider,
            url,
            client_id: self.client_id.trim().to_string(),
            client_secret: non_empty(&self.client_secret).map(str::to_string),
        };
        request.validate().map_err(translated)?;
        if self.editing.is_none() && request.client_secret.is_none() {
            return Err(locale
                .t("git_connections.client_secret_required")
                .to_string());
        }
        Ok(request)
    }
}

fn non_empty(value: &str) -> Option<&str> {
    Some(value.trim()).filter(|value| !value.is_empty())
}

#[function_component(GitConnections)]
pub fn git_connections() -> Html {
    use_ensure_auth();
    let locale = use_locale();
    let (session, _) = use_store::<SessionStore>();
    let location = use_location();
    let connections = use_state(Option::<Vec<GitConnectionEntry>>::default);
    let form = use_state(Form::default);
    let notice = use_state(Option::<Notice<()>>::default);
    // The callback of the provider returns here with the result in the query.
    let query = location.as_ref().map_or("", |l| l.query_str());
    let callback = use_state(|| callback_result(query));
    // GitHub returns here after the installation of a GitHub App.
    let installed = use_state(|| github_app::installed_id(query));
    let installation = use_state(Option::<Result<(), String>>::default);
    let navigator = use_navigator();

    // The admin API answers 401 until the session loads, so the list waits for it.
    let signed_in = session.info.is_some();
    let (connections_cloned, installed_cloned) = (connections.clone(), installed.clone());
    let installation_cloned = installation.clone();
    use_effect_with(signed_in, move |&signed_in| {
        if signed_in {
            let installed = *installed_cloned;
            start(
                locale,
                installed,
                installation_cloned,
                connections_cloned,
                navigator,
            );
        }
    });

    if signed_in && !(session.can_read_platform() && session.can_edit()) {
        return html! { <Redirect<Route> to={Route::Home} /> };
    }
    let admin = session.is_admin();
    let rows = connection_rows(locale, admin, &form, &connections);
    html! {
        <>
            { callback_view(locale, &callback) }
            { installation_view(locale, &installation) }
            { list_card(locale, connections.is_some(), "git_connections.empty", &COLUMNS, &rows) }
            if admin {
                { form_view(locale, &form, &notice, &connections) }
                <PublicUrl {locale} />
            }
        </>
    }
}

/// The result of an authorization that the provider sent back in the query of the page: whether
/// it succeeded, and the translation key of its message.
fn callback_result(query: &str) -> Option<(bool, String)> {
    let query = query.trim_start_matches('?');
    let value = |name: &str| {
        query
            .split('&')
            .find_map(|pair| pair.strip_prefix(name)?.strip_prefix('='))
    };
    if value("connected").is_some() {
        return Some((true, "git_connections.callback_connected".to_string()));
    }
    let code = value("error")?;
    let key = CALLBACK_ERRORS
        .iter()
        .find(|known| **known == code)
        .map_or("git_connections.callback_failed".to_string(), |code| {
            format!("git_connections.callback_{code}")
        });
    Some((false, key))
}

/// Loads the list, or first connects the GitHub App that GitHub returned from.
fn start(
    locale: Locale,
    installed: Option<ShortId>,
    installation: Installation,
    connections: Connections,
    navigator: Option<Navigator>,
) {
    match installed {
        Some(id) => install(locale, id, installation, connections),
        None => load(connections, PATH),
    }
    // The result stays on the page, not in the address.
    if let Some(navigator) = navigator {
        navigator.replace(&Route::GitConnections);
    }
}

/// Connects the connection to the installation of its GitHub App, then loads the list.
fn install(locale: Locale, id: ShortId, installation: Installation, connections: Connections) {
    wasm_bindgen_futures::spawn_local(async move {
        let result = github_app::install(locale, id).await.map(|_| ());
        installation.set(Some(result));
        load(connections, PATH);
    });
}

fn installation_view(locale: Locale, result: &Option<Result<(), String>>) -> Html {
    match result {
        Some(Ok(())) => success_box(html! { {locale.t("git_connections.callback_installed")} }),
        Some(Err(message)) => failure_box(message),
        None => html! {},
    }
}

fn callback_view(locale: Locale, result: &Option<(bool, String)>) -> Html {
    match result {
        Some((true, key)) => success_box(html! { {locale.t(key)} }),
        Some((false, key)) => failure_box(locale.t(key)),
        None => html! {},
    }
}

fn connection_rows(
    locale: Locale,
    admin: bool,
    form: &UseStateHandle<Form>,
    connections: &Connections,
) -> Vec<Row> {
    connections
        .as_deref()
        .unwrap_or_default()
        .iter()
        .map(|entry| connection_row(locale, entry, admin, form, connections))
        .collect()
}

fn connection_row(
    locale: Locale,
    entry: &GitConnectionEntry,
    admin: bool,
    form: &UseStateHandle<Form>,
    connections: &Connections,
) -> Row {
    let (status, color) = connection_status(entry.status);
    let actions = if admin {
        admin_actions(locale, entry, form, connections)
    } else {
        html! {}
    };
    Row {
        key: entry.id.to_string(),
        cells: vec![
            html! { <>{entry.name.to_string()}</> },
            html! { <>{provider_name(entry.provider)}</> },
            address_view(entry),
            status_badge(locale.t(status), color),
            html! { <>{entry.account.clone().unwrap_or_default()}</> },
        ],
        actions,
    }
}

/// The address of the provider, or the page of the GitHub App of a GitHub connection.
fn address_view(entry: &GitConnectionEntry) -> Html {
    match &entry.app_url {
        Some(app_url) => html! {
            <a class={classes!(LINK_CLASS, "font-mono", "break-all")} href={app_url.clone()} target="_blank" rel="noopener noreferrer">{app_url.clone()}</a>
        },
        None => html! { <span class="font-mono break-all">{entry.url.to_string()}</span> },
    }
}

/// The translation key and the dot color of the state of a connection.
fn connection_status(status: ConnectionStatus) -> (&'static str, &'static str) {
    match status {
        ConnectionStatus::Connected => ("git_connections.status_connected", "bg-green-500"),
        ConnectionStatus::NotConnected => ("git_connections.status_not_connected", "bg-yellow-400"),
        ConnectionStatus::Expired => ("git_connections.status_expired", "bg-red-500"),
    }
}

pub(super) fn provider_name(provider: GitProvider) -> &'static str {
    PROVIDERS
        .iter()
        .find(|(item, _)| *item == provider)
        .map_or("", |(_, name)| name)
}

fn admin_actions(
    locale: Locale,
    entry: &GitConnectionEntry,
    form: &UseStateHandle<Form>,
    connections: &Connections,
) -> Html {
    let label = match (entry.provider, entry.status) {
        (GitProvider::Github, ConnectionStatus::NotConnected) => "git_connections.install",
        (_, ConnectionStatus::NotConnected) => "git_connections.connect",
        _ => "git_connections.reconnect",
    };
    let question = locale.tf(
        "git_connections.confirm_delete",
        &[("name", entry.name.as_str())],
    );
    let path = format!("{PATH}/{}", entry.id);
    let delete = delete_on_click(locale, question, path, reload_callback(connections, PATH));
    html! {
        <>
            <a class={LINK_CLASS} onclick={connect_on_click(locale, entry.id)}>{locale.t(label)}</a>
            <a class={LINK_CLASS} onclick={set_on_click(form, Form::edit(entry))}>{locale.t("common.edit")}</a>
            <a class={DANGER_LINK_CLASS} onclick={delete}>{locale.t("common.delete")}</a>
        </>
    }
}

/// The callback address on the origin of this page.
fn callback_url() -> String {
    let origin = web_sys::window()
        .and_then(|window| window.location().origin().ok())
        .unwrap_or_default();
    format!("{origin}{OAUTH_CALLBACK_PATH}")
}

/// Starts the authorization and opens the page of the provider.
fn connect_on_click(locale: Locale, id: ShortId) -> Callback<MouseEvent> {
    Callback::from(move |event: MouseEvent| {
        event.prevent_default();
        wasm_bindgen_futures::spawn_local(async move {
            if let Err(message) = authorize(locale, id).await {
                dialog::message(locale, &message, Icon::Error).await;
            }
        });
    })
}

async fn authorize(locale: Locale, id: ShortId) -> Result<(), String> {
    let body = AuthorizeRequest {
        redirect_uri: callback_url()
            .parse()
            .map_err(|err| locale.error_message(&err))?,
    };
    let request = Request::post(&format!("{API_ENDPOINT}{PATH}/{id}/authorize"))
        .json(&body)
        .map_err(|err| err.to_string())?;
    let response: AuthorizeResponse = super::settings::send_request(locale, request)
        .await?
        .json()
        .await
        .map_err(|err| err.to_string())?;
    let window = web_sys::window().ok_or("no window")?;
    window
        .location()
        .set_href(&response.url)
        .map_err(|_| "the provider page did not open".to_string())
}

fn form_view(
    locale: Locale,
    form: &UseStateHandle<Form>,
    notice: &UseStateHandle<Option<Notice<()>>>,
    connections: &Connections,
) -> Html {
    let parsed = form.parse(locale, &callback_url());
    let title = match form.editing {
        Some(_) => locale.tf("git_connections.edit_title", &[("name", &form.name)]),
        None => locale.t("git_connections.add_title").to_string(),
    };
    let touched = form.editing.is_some() || !form.name.is_empty();
    let card = FormCard {
        title,
        notice: notice_view(
            notice,
            |()| html! { <p>{locale.t("git_connections.saved")}</p> },
        ),
        fields: fields_view(locale, form),
        error: parsed.as_ref().err().filter(|_| touched).cloned(),
        cancel: form
            .editing
            .is_some()
            .then(|| set_on_click(form, Form::default())),
        save_label: if form.provider == GitProvider::Github && form.editing.is_none() {
            "git_connections.create_github_app"
        } else {
            "git_connections.save"
        },
        can_save: parsed.is_ok(),
    };
    form_card(locale, card, submit(locale, form, notice, connections))
}

fn fields_view(locale: Locale, form: &UseStateHandle<Form>) -> Html {
    let provider_fields = match (form.provider, form.editing) {
        (GitProvider::Github, None) => github_app::fields_view(
            locale,
            url_input(form, GITHUB_URL),
            html! { <input type="text" autocapitalize="off" autocomplete="off" placeholder="my-team" value={form.organization.clone()} oninput={text_input(form, |f| &mut f.organization)} class={INPUT_CLASS} /> },
        ),
        (GitProvider::Github, Some(_)) => html! {
            <p class={HINT_CLASS}>{locale.t("git_connections.github_edit_hint")}</p>
        },
        _ => oauth_fields_view(locale, form),
    };
    html! {
        <>
            <label class={LABEL_CLASS}>{locale.t("common.name")}</label>
            <input type="text" autocapitalize="off" autocomplete="off" placeholder="github-team" value={form.name.clone()} oninput={text_input(form, |f| &mut f.name)} class={INPUT_CLASS} />

            { provider_select(locale, form) }
            { provider_fields }
        </>
    }
}

fn url_input(form: &UseStateHandle<Form>, placeholder: &'static str) -> Html {
    html! {
        <input type="url" autocapitalize="off" autocomplete="off" {placeholder} value={form.url.clone()} oninput={text_input(form, |f| &mut f.url)} class={INPUT_CLASS} />
    }
}

/// The OAuth application of a GitLab or Gitea connection.
fn oauth_fields_view(locale: Locale, form: &UseStateHandle<Form>) -> Html {
    let secret_hint = if form.editing.is_some() {
        "git_connections.client_secret_keep"
    } else {
        "git_connections.client_secret_hint"
    };
    html! {
        <>
            <p class={HINT_CLASS}>{locale.t("git_connections.register_hint")}</p>
            { copy_box(locale.t("git_connections.callback_url"), &callback_url(), 1) }
            <label class={LABEL_CLASS}>{locale.t("git_connections.address")}</label>
            { url_input(form, url_placeholder(form.provider)) }
            <p class={HINT_CLASS}>{locale.t("git_connections.address_hint")}</p>

            <label class={LABEL_CLASS}>{locale.t("git_connections.client_id")}</label>
            <input type="text" autocapitalize="off" autocomplete="off" value={form.client_id.clone()} oninput={text_input(form, |f| &mut f.client_id)} class={INPUT_CLASS} />

            <label class={LABEL_CLASS}>{locale.t("git_connections.client_secret")}</label>
            <input type="password" autocomplete="new-password" value={form.client_secret.clone()} oninput={text_input(form, |f| &mut f.client_secret)} class={INPUT_CLASS} />
            <p class={HINT_CLASS}>{locale.t(secret_hint)}</p>
        </>
    }
}

fn url_placeholder(provider: GitProvider) -> &'static str {
    match provider {
        GitProvider::Gitlab => "https://gitlab.com",
        _ => "https://gitea.example.com",
    }
}

fn provider_select(locale: Locale, form: &UseStateHandle<Form>) -> Html {
    let options = PROVIDERS
        .iter()
        .map(|(provider, name)| {
            html! { <option value={provider.as_str()} selected={*provider == form.provider}>{*name}</option> }
        })
        .collect::<Html>();
    let state = form.clone();
    let onchange = Callback::from(move |event: Event| {
        let select: HtmlSelectElement = event.target_unchecked_into();
        let mut updated = (*state).clone();
        if let Ok(provider) = select.value().parse() {
            updated.provider = provider;
        }
        state.set(updated);
    });
    html! {
        <>
            <label class={LABEL_CLASS}>{locale.t("git_connections.provider")}</label>
            <select {onchange} disabled={form.editing.is_some()} class={INPUT_CLASS}>{options}</select>
        </>
    }
}

fn submit(
    locale: Locale,
    form: &UseStateHandle<Form>,
    notice: &UseStateHandle<Option<Notice<()>>>,
    connections: &Connections,
) -> Callback<SubmitEvent> {
    let done = save_result(form, notice, reload_callback(connections, PATH));
    let form = form.clone();
    submit_callback(
        move || form.parse(locale, &callback_url()).ok(),
        move |change| save(locale, change),
        done,
    )
}

/// Adds or replaces a connection, or starts the creation of a GitHub App.
async fn save(locale: Locale, change: Change) -> Result<(), String> {
    let (editing, request) = match change {
        Change::GithubApp(request) => return github_app::create(locale, request).await,
        Change::Connection(editing, request) => (editing, request),
    };
    let builder = match editing {
        Some(id) => Request::put(&format!("{API_ENDPOINT}{PATH}/{id}")),
        None => Request::post(&format!("{API_ENDPOINT}{PATH}")),
    };
    send_json(locale, builder, &request).await
}

#[derive(Properties, PartialEq)]
struct PublicUrlProps {
    locale: Locale,
}

/// The public address that the Git providers send the webhook requests to.
#[function_component(PublicUrl)]
fn public_url(props: &PublicUrlProps) -> Html {
    let locale = props.locale;
    let value = use_state(String::new);
    let notice = use_state(|| Option::<Notice<()>>::None);

    let loaded = value.clone();
    let failed = notice.clone();
    use_effect_with((), move |_| {
        wasm_bindgen_futures::spawn_local(async move {
            let path = format!("{API_ENDPOINT}{SETTINGS_PATH}");
            match fetch_json::<PlatformSettings>(locale, Request::get(&path)).await {
                Ok(settings) => loaded.set(
                    settings
                        .public_url
                        .map(|url| url.to_string())
                        .unwrap_or_default(),
                ),
                Err(message) => failed.set(Some(Notice::Failed(message))),
            }
        });
    });

    let parsed = parse_public_url(locale, &value);
    let fields = html! {
        <>
            <label class={LABEL_CLASS}>{locale.t("git_connections.public_url")}</label>
            <input type="url" autocapitalize="off" autocomplete="off" placeholder={origin()} value={(*value).clone()} oninput={string_input(&value)} class={INPUT_CLASS} />
            <p class={HINT_CLASS}>{locale.t("git_connections.public_url_hint")}</p>
        </>
    };
    let card = FormCard {
        title: locale.t("git_connections.public_url_title").to_string(),
        notice: notice_view(
            &notice,
            |()| html! { <p>{locale.t("git_connections.public_url_saved")}</p> },
        ),
        fields,
        error: parsed.as_ref().err().cloned(),
        cancel: None,
        save_label: "git_connections.save",
        can_save: parsed.is_ok(),
    };
    form_card(locale, card, save_public_url(locale, parsed.ok(), &notice))
}

fn origin() -> String {
    web_sys::window()
        .and_then(|window| window.location().origin().ok())
        .unwrap_or_default()
}

fn parse_public_url(locale: Locale, value: &str) -> Result<PlatformSettings, String> {
    let public_url = non_empty(value)
        .map(|url| url.parse())
        .transpose()
        .map_err(|err| locale.error_message(&err))?;
    Ok(PlatformSettings { public_url })
}

fn string_input(state: &UseStateHandle<String>) -> Callback<InputEvent> {
    let state = state.clone();
    Callback::from(move |event: InputEvent| {
        let input: web_sys::HtmlInputElement = event.target_unchecked_into();
        state.set(input.value());
    })
}

fn save_public_url(
    locale: Locale,
    settings: Option<PlatformSettings>,
    notice: &UseStateHandle<Option<Notice<()>>>,
) -> Callback<SubmitEvent> {
    let notice = notice.clone();
    Callback::from(move |event: SubmitEvent| {
        event.prevent_default();
        let (Some(settings), notice) = (settings.clone(), notice.clone()) else {
            return;
        };
        wasm_bindgen_futures::spawn_local(async move {
            let request = Request::put(&format!("{API_ENDPOINT}{SETTINGS_PATH}"));
            let result = send_json(locale, request, &settings).await;
            notice.set(Some(match result {
                Ok(()) => Notice::Saved(()),
                Err(message) => Notice::Failed(message),
            }));
        });
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_callback_query_names_its_result() {
        assert!(callback_result("").is_none());
        let connected = callback_result("?connected=bcd-fgh");
        assert_eq!(connected.map(|(ok, _)| ok), Some(true));
        for code in CALLBACK_ERRORS {
            let (ok, key) = callback_result(&format!("?error={code}")).unwrap();
            assert!(!ok);
            assert_ne!(Locale::En.t(&key), key);
            assert_ne!(Locale::Tr.t(&key), key);
        }
        let (_, unknown) = callback_result("?error=<script>").unwrap();
        assert_eq!(unknown, "git_connections.callback_failed");
    }

    const CALLBACK: &str = "https://r3v3rs3.example.com/oauth/git/callback";

    #[test]
    fn a_form_needs_a_secret_only_for_a_new_connection() {
        let mut form = Form {
            name: "team".into(),
            provider: GitProvider::Gitlab,
            client_id: "client".into(),
            ..Form::default()
        };
        assert!(form.parse(Locale::En, CALLBACK).is_err());
        form.editing = Some("bcd-fgh".parse().unwrap());
        let Ok(Change::Connection(_, request)) = form.parse(Locale::En, CALLBACK) else {
            panic!("not a connection change");
        };
        assert_eq!((request.url, request.client_secret), (None, None));
        form.provider = GitProvider::Gitea;
        assert!(form.parse(Locale::En, CALLBACK).is_err());
        form.url = "https://git.example.com".into();
        assert!(form.parse(Locale::En, CALLBACK).is_ok());
    }

    #[test]
    fn a_new_github_connection_creates_a_github_app() {
        let mut form = Form {
            name: "hub".into(),
            organization: "my-team".into(),
            ..Form::default()
        };
        let Ok(Change::GithubApp(request)) = form.parse(Locale::En, CALLBACK) else {
            panic!("not a GitHub App");
        };
        assert_eq!(request.url, None);
        assert_eq!(request.redirect_uri.as_str(), CALLBACK);
        assert_eq!(
            request.organization.map(|o| o.to_string()).as_deref(),
            Some("my-team")
        );
        form.organization = "my_team".into();
        assert!(form.parse(Locale::En, CALLBACK).is_err());

        let edited = Form {
            editing: Some("bcd-fgh".parse().unwrap()),
            name: "renamed".into(),
            client_id: "Iv1.abc".into(),
            ..Form::default()
        };
        let Ok(Change::Connection(Some(_), request)) = edited.parse(Locale::En, CALLBACK) else {
            panic!("not a rename");
        };
        assert_eq!(request.client_id, "Iv1.abc");
        assert_eq!(request.client_secret, None);
    }
}
