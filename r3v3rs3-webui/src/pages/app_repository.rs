//! The repository and the branch of a Git or a Compose app. With a Git provider connection the
//! form lists the repositories of the connected account and the branches of the chosen one.

use super::accounts::{HINT_CLASS, INPUT_CLASS, LABEL_CLASS};
use super::app_form::{AppForm, text_field};
use super::git_connections::provider_name;
use super::settings::fetch_json;
use crate::API_ENDPOINT;
use gloo_net::http::Request;
use r3v3rs3_api::git_connection::{ConnectionStatus, GitConnectionEntry, GitRepository, RepoName};
use r3v3rs3_api::i18n::Locale;
use r3v3rs3_api::id::ShortId;
use wasm_bindgen_futures::spawn_local;
use web_sys::{HtmlInputElement, HtmlSelectElement};
use yew::prelude::*;

const BUTTON_CLASS: &str = "px-3 rounded-lg border border-neutral-300 dark:border-neutral-600 text-sm hover:bg-neutral-100 hover:dark:bg-neutral-900";

type Loaded<T> = UseStateHandle<Option<Result<Vec<T>, String>>>;

#[derive(Properties, PartialEq)]
pub struct Props {
    pub form: UseStateHandle<AppForm>,
    pub connections: Vec<GitConnectionEntry>,
    pub locale: Locale,
}

#[function_component(RepositoryPicker)]
pub fn repository_picker(props: &Props) -> Html {
    let (locale, form) = (props.locale, &props.form);
    let search = use_state(String::new);
    // The submitted search, with a counter so that the same text searches again.
    let query = use_state(|| (String::new(), 0_u32));
    let repositories: Loaded<GitRepository> = use_state(|| None);
    let branches: Loaded<String> = use_state(|| None);

    let connection = props
        .connections
        .iter()
        .find(|entry| entry.id.to_string() == form.connection)
        .cloned();
    let usable = connection
        .as_ref()
        .filter(|entry| entry.status == ConnectionStatus::Connected);
    let id = usable.map(|entry| entry.id);
    let repository =
        usable.and_then(|entry| RepoName::of_clone_url(&entry.url, form.repository.trim()));

    let loaded = repositories.clone();
    use_effect_with((id, (*query).clone()), move |(id, (search, _))| {
        load_repositories(locale, *id, search.clone(), loaded);
    });
    let loaded = branches.clone();
    use_effect_with((id, repository.clone()), move |(id, repository)| {
        load_branches(locale, *id, repository.clone(), loaded);
    });

    html! {
        <>
            { connection_select(locale, form, &props.connections) }
            if connection.is_some() && usable.is_none() {
                <p class="mt-1 text-sm text-red-600 dark:text-red-500">{locale.t("apps.connection_not_connected")}</p>
            }
            if usable.is_some() {
                { search_view(locale, &search, &query) }
                { repository_select(locale, form, &repositories) }
            }
            { text_field(locale, form, "apps.repository", Some("apps.repository_hint"), |f| &mut f.repository) }
            { branch_view(locale, form, &branches) }
        </>
    }
}

fn load_repositories(
    locale: Locale,
    id: Option<ShortId>,
    search: String,
    loaded: Loaded<GitRepository>,
) {
    loaded.set(None);
    let Some(id) = id else {
        return;
    };
    spawn_local(async move {
        let mut path = format!("{API_ENDPOINT}/git/connections/{id}/repositories");
        if !search.trim().is_empty() {
            path = format!("{path}?search={}", encode(search.trim()));
        }
        loaded.set(Some(fetch_json(locale, Request::get(&path)).await));
    });
}

fn load_branches(
    locale: Locale,
    id: Option<ShortId>,
    repository: Option<RepoName>,
    loaded: Loaded<String>,
) {
    loaded.set(None);
    let (Some(id), Some(repository)) = (id, repository) else {
        return;
    };
    spawn_local(async move {
        let path = format!(
            "{API_ENDPOINT}/git/connections/{id}/branches?repository={}",
            encode(repository.as_str())
        );
        loaded.set(Some(fetch_json(locale, Request::get(&path)).await));
    });
}

fn encode(value: &str) -> String {
    url::form_urlencoded::byte_serialize(value.as_bytes()).collect()
}

/// A select whose change writes the chosen value with `apply`.
fn select(options: Html, apply: impl Fn(String) + 'static) -> Html {
    let onchange = Callback::from(move |event: Event| {
        let select: HtmlSelectElement = event.target_unchecked_into();
        apply(select.value());
    });
    html! { <select {onchange} class={INPUT_CLASS}>{options}</select> }
}

fn update(form: &UseStateHandle<AppForm>, change: impl Fn(&mut AppForm) + 'static) -> impl Fn() {
    let form = form.clone();
    move || {
        let mut updated = (*form).clone();
        change(&mut updated);
        form.set(updated);
    }
}

fn connection_select(
    locale: Locale,
    form: &UseStateHandle<AppForm>,
    connections: &[GitConnectionEntry],
) -> Html {
    let mut options = vec![html! {
        <option value="" selected={form.connection.is_empty()}>{locale.t("apps.connection_none")}</option>
    }];
    options.extend(connections.iter().map(|entry| {
        let id = entry.id.to_string();
        let label = format!("{} ({})", entry.name, provider_name(entry.provider));
        html! { <option value={id.clone()} selected={id == form.connection}>{label}</option> }
    }));
    // A connection that the list does not hold still shows its id.
    let known = connections
        .iter()
        .any(|entry| entry.id.to_string() == form.connection);
    if !form.connection.is_empty() && !known {
        let id = form.connection.clone();
        options.push(html! { <option value={id.clone()} selected=true>{id}</option> });
    }
    let state = form.clone();
    let apply = move |value: String| update(&state, move |f| f.connection = value.clone())();
    html! {
        <>
            <label class={LABEL_CLASS}>{locale.t("apps.connection")}</label>
            { select(options.into_iter().collect(), apply) }
            <p class={HINT_CLASS}>{locale.t("apps.connection_hint")}</p>
        </>
    }
}

fn search_view(
    locale: Locale,
    search: &UseStateHandle<String>,
    query: &UseStateHandle<(String, u32)>,
) -> Html {
    let run = {
        let (search, query) = (search.clone(), query.clone());
        move || query.set(((*search).clone(), query.1.wrapping_add(1)))
    };
    let onclick = {
        let run = run.clone();
        Callback::from(move |_: MouseEvent| run())
    };
    // Enter searches instead of sending the app form.
    let onkeydown = Callback::from(move |event: KeyboardEvent| {
        if event.key() == "Enter" {
            event.prevent_default();
            run();
        }
    });
    let state = search.clone();
    let oninput = Callback::from(move |event: InputEvent| {
        let input: HtmlInputElement = event.target_unchecked_into();
        state.set(input.value());
    });
    html! {
        <>
            <label class={LABEL_CLASS}>{locale.t("apps.repository_search")}</label>
            <div class="flex gap-2">
                <input type="search" autocapitalize="off" autocomplete="off" value={(**search).clone()} {oninput} {onkeydown} class={INPUT_CLASS} />
                <button type="button" {onclick} class={BUTTON_CLASS}>{locale.t("apps.search")}</button>
            </div>
        </>
    }
}

fn repository_select(
    locale: Locale,
    form: &UseStateHandle<AppForm>,
    repositories: &Loaded<GitRepository>,
) -> Html {
    let list = match &**repositories {
        None => return html! { <p class={HINT_CLASS}>{locale.t("apps.loading")}</p> },
        Some(Err(message)) => return error_line(message),
        Some(Ok(list)) => list.clone(),
    };
    let mut options =
        vec![html! { <option value="" selected=true>{locale.t("apps.repository_pick")}</option> }];
    options.extend(list.iter().enumerate().map(|(index, repository)| {
        html! { <option value={index.to_string()}>{repository.full_name.to_string()}</option> }
    }));
    let state = form.clone();
    let apply = move |value: String| {
        let Some(chosen) = value
            .parse::<usize>()
            .ok()
            .and_then(|index| list.get(index))
        else {
            return;
        };
        let chosen = chosen.clone();
        update(&state, move |f| choose(f, &chosen))();
    };
    select(options.into_iter().collect(), apply)
}

/// Takes the clone address and the default branch of a chosen repository.
fn choose(form: &mut AppForm, repository: &GitRepository) {
    form.repository.clone_from(&repository.clone_url);
    if let Some(branch) = &repository.default_branch {
        form.branch.clone_from(branch);
    }
}

fn branch_view(locale: Locale, form: &UseStateHandle<AppForm>, branches: &Loaded<String>) -> Html {
    match &**branches {
        Some(Ok(list)) if !list.is_empty() => branch_select(locale, form, list),
        Some(Err(message)) => html! {
            <>
                { text_field(locale, form, "apps.branch", None, |f| &mut f.branch) }
                { error_line(message) }
            </>
        },
        _ => text_field(locale, form, "apps.branch", None, |f| &mut f.branch),
    }
}

fn branch_select(locale: Locale, form: &UseStateHandle<AppForm>, list: &[String]) -> Html {
    let mut names = list.to_vec();
    // The branch of the app stays in the list when the provider did not return it.
    if !names.contains(&form.branch) {
        names.insert(0, form.branch.clone());
    }
    let options = names
        .iter()
        .map(|name| html! { <option value={name.clone()} selected={*name == form.branch}>{name.clone()}</option> })
        .collect::<Html>();
    let state = form.clone();
    let apply = move |value: String| update(&state, move |f| f.branch = value.clone())();
    html! {
        <>
            <label class={LABEL_CLASS}>{locale.t("apps.branch")}</label>
            { select(options, apply) }
        </>
    }
}

fn error_line(message: &str) -> Html {
    html! { <p class="mt-1 text-sm text-red-600 dark:text-red-500">{message.to_string()}</p> }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_chosen_repository_fills_the_address_and_the_branch() {
        let mut form = AppForm::default();
        let repository = GitRepository {
            full_name: "owner/shop".parse().unwrap(),
            clone_url: "https://github.com/owner/shop.git".into(),
            default_branch: Some("trunk".into()),
            private: true,
        };
        choose(&mut form, &repository);
        assert_eq!(
            (form.repository.as_str(), form.branch.as_str()),
            ("https://github.com/owner/shop.git", "trunk")
        );
    }
}
