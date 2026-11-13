//! The accounts of one database with their passwords: the root account and the tenants of a
//! shared server. Only an Admin account opens it; it also sets new passwords and deletes the
//! tenants that no app uses.

use super::resource_page::delete_on_click;
use super::settings::{failure_box, fetch_json, post_json};
use crate::API_ENDPOINT;
use crate::components::action_button::{ActionIconButton, ActionVariant};
use crate::components::button::{Button, ButtonKind};
use crate::components::data_list::{Column, ListContent, ListOptions, Row, list_view};
use crate::components::modal::{Modal, ModalSize};
use crate::dialog::{self, Icon};
use gloo_net::http::Request;
use r3v3rs3_api::i18n::Locale;
use r3v3rs3_api::id::ShortId;
use r3v3rs3_api::platform::{
    AppEntry, DatabaseAccount, DatabaseCredentials, DatabasePasswordRequest, DatabasePasswordReset,
    DatabaseTenant,
};
use wasm_bindgen_futures::spawn_local;
use yew::prelude::*;

const COLUMNS: [Column; 5] = [
    Column {
        label: "databases.account",
        class: "whitespace-nowrap",
    },
    Column {
        label: "databases.user",
        class: "font-mono",
    },
    Column {
        label: "databases.password",
        class: "font-mono break-all",
    },
    Column {
        label: "databases.connection",
        class: "font-mono break-all",
    },
    Column {
        label: "databases.apps",
        class: "",
    },
];

#[derive(Properties, PartialEq)]
pub struct CredentialsPanelProps {
    pub locale: Locale,
    pub database: AppEntry,
    /// Every app, to name the apps of a tenant.
    pub apps: Vec<AppEntry>,
    pub close: Callback<()>,
}

/// What the panel read from the admin API.
#[derive(Clone, PartialEq, Default)]
struct Loaded {
    credentials: Option<DatabaseCredentials>,
    tenants: Vec<DatabaseTenant>,
    error: Option<String>,
}

fn load(locale: Locale, id: ShortId, state: UseStateHandle<Loaded>) {
    spawn_local(async move {
        let credentials = fetch_json::<DatabaseCredentials>(
            locale,
            Request::get(&format!("{API_ENDPOINT}/databases/{id}/credentials")),
        )
        .await;
        let tenants = fetch_json::<Vec<DatabaseTenant>>(
            locale,
            Request::get(&format!("{API_ENDPOINT}/databases/{id}/tenants")),
        )
        .await;
        let loaded = match (credentials, tenants) {
            (Ok(credentials), Ok(tenants)) => Loaded {
                credentials: Some(credentials),
                tenants,
                error: None,
            },
            (Err(error), _) | (_, Err(error)) => Loaded {
                error: Some(error),
                ..Loaded::default()
            },
        };
        state.set(loaded);
    });
}

/// The names of the apps of a tenant, separated by commas.
fn app_names(apps: &[AppEntry], tenants: &[DatabaseTenant], tenant: &str) -> String {
    let Some(entry) = tenants.iter().find(|entry| entry.name == tenant) else {
        return String::new();
    };
    entry
        .apps
        .iter()
        .map(|id| {
            apps.iter()
                .find(|app| app.id == *id)
                .map_or_else(|| id.to_string(), |app| app.name.to_string())
        })
        .collect::<Vec<_>>()
        .join(", ")
}

/// The message after a new password: the apps whose new deployment did not start.
fn reset_message(locale: Locale, reset: &DatabasePasswordReset) -> String {
    if reset.failed.is_empty() {
        return locale.t("databases.password_reset").to_string();
    }
    let failed = reset
        .failed
        .iter()
        .map(ToString::to_string)
        .collect::<Vec<_>>()
        .join(", ");
    locale.tf("databases.password_reset_failed", &[("apps", &failed)])
}

fn reset_on_click(
    locale: Locale,
    id: ShortId,
    tenant: Option<String>,
    reload: Callback<()>,
) -> Callback<MouseEvent> {
    Callback::from(move |event: MouseEvent| {
        event.prevent_default();
        let (tenant, reload) = (tenant.clone(), reload.clone());
        let question = locale.t("databases.confirm_password_reset").to_string();
        dialog::confirm_then(locale, question, async move {
            let body = DatabasePasswordRequest { tenant };
            let request = Request::post(&format!("{API_ENDPOINT}/databases/{id}/password"));
            match post_json::<DatabasePasswordReset>(locale, request, &body).await {
                Ok(reset) => {
                    reload.emit(());
                    dialog::message(locale, &reset_message(locale, &reset), Icon::Success).await;
                }
                Err(message) => dialog::message(locale, &message, Icon::Error).await,
            }
        });
    })
}

struct RowContext<'a> {
    locale: Locale,
    id: ShortId,
    apps: &'a [AppEntry],
    tenants: &'a [DatabaseTenant],
    reload: Callback<()>,
}

fn account_row(ctx: &RowContext<'_>, account: &DatabaseAccount) -> Row {
    let locale = ctx.locale;
    let label = account
        .tenant
        .clone()
        .unwrap_or_else(|| locale.t("databases.root_account").to_string());
    let apps = account
        .tenant
        .as_deref()
        .map(|tenant| app_names(ctx.apps, ctx.tenants, tenant))
        .unwrap_or_default();
    // A root account without a password, a Redis from before the password, gets its first
    // password through the same button.
    let reset = reset_on_click(locale, ctx.id, account.tenant.clone(), ctx.reload.clone());
    let delete = account
        .tenant
        .as_ref()
        .filter(|_| apps.is_empty())
        .map(|tenant| {
            let question = locale.tf("databases.confirm_delete_tenant", &[("name", tenant)]);
            let path = format!("/databases/{}/tenants/{tenant}", ctx.id);
            delete_on_click(locale, question, path, ctx.reload.clone())
        });
    Row {
        key: label.clone(),
        cells: vec![
            html! { {label} },
            html! { {account.user.clone()} },
            html! { {account.password.clone()} },
            html! { {account.url.clone()} },
            html! { {apps} },
        ],
        actions: html! {
            <>
                <ActionIconButton
                    variant={ActionVariant::Restore}
                    title={locale.t("databases.reset_password").to_string()}
                    onclick={reset}
                />
                if let Some(onclick) = delete {
                    <ActionIconButton
                        variant={ActionVariant::Delete}
                        title={locale.t("databases.delete_tenant").to_string()}
                        {onclick}
                    />
                }
            </>
        },
    }
}

fn accounts_table(ctx: &RowContext<'_>, credentials: Option<&DatabaseCredentials>) -> Html {
    let rows = credentials
        .map(|credentials| {
            std::iter::once(&credentials.root)
                .chain(credentials.tenants.iter())
                .map(|account| account_row(ctx, account))
                .collect::<Vec<_>>()
        })
        .unwrap_or_default();
    let content = ListContent {
        loaded: credentials.is_some(),
        empty: "common.list_empty",
        columns: &COLUMNS,
        rows: &rows,
    };
    let options = ListOptions {
        embedded: true,
        compact: true,
        ..Default::default()
    };
    list_view(ctx.locale, content, None, &options)
}

/// The accounts of a database with their passwords, in a modal over the list of the databases.
#[function_component(CredentialsPanel)]
pub fn credentials_panel(props: &CredentialsPanelProps) -> Html {
    let locale = props.locale;
    let id = props.database.id;
    let loaded = use_state(Loaded::default);
    let reload = {
        let loaded = loaded.clone();
        Callback::from(move |()| load(locale, id, loaded.clone()))
    };
    {
        let reload = reload.clone();
        use_effect_with(id, move |_| reload.emit(()));
    }
    let close = {
        let close = props.close.clone();
        Callback::from(move |_: MouseEvent| close.emit(()))
    };
    let title = locale.tf(
        "databases.credentials_of",
        &[("name", props.database.name.as_str())],
    );
    let ctx = RowContext {
        locale,
        id,
        apps: &props.apps,
        tenants: &loaded.tenants,
        reload,
    };
    let footer = html! {
        <Button kind={ButtonKind::Secondary} onclick={close}>{locale.t("common.close")}</Button>
    };
    html! {
        <Modal {title} on_close={props.close.clone()} size={ModalSize::Xl} {footer}>
            <p class="text-sm text-neutral-500 dark:text-neutral-400 mb-3">{locale.t("databases.credentials_hint")}</p>
            if let Some(error) = loaded.error.clone() {
                { failure_box(&error) }
            } else {
                { accounts_table(&ctx, loaded.credentials.as_ref()) }
            }
        </Modal>
    }
}
