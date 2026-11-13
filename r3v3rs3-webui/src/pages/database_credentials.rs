//! The accounts of one database with their passwords: the root account and the tenants of a
//! shared server. Only an Admin account opens it; it also sets new passwords and deletes the
//! tenants that no app uses.

use super::resource_page::delete_on_click;
use super::settings::{fetch_json, post_json};
use crate::API_ENDPOINT;
use crate::components::action_button::{ActionIconButton, ActionVariant};
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

fn account_row(ctx: &RowContext<'_>, account: &DatabaseAccount) -> Html {
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
    html! {
        <tr key={label.clone()} class="border-b border-neutral-700 align-top">
            <td class="py-1.5 font-medium">{label}</td>
            <td class="py-1.5 font-mono">{account.user.clone()}</td>
            <td class="py-1.5 font-mono break-all">{account.password.clone()}</td>
            <td class="py-1.5 font-mono break-all">{account.url.clone()}</td>
            <td class="py-1.5 text-neutral-400">{apps}</td>
            <td class="py-1.5 text-right">
                <div class="inline-flex items-center justify-end gap-1.5">
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
                </div>
            </td>
        </tr>
    }
}

fn accounts_table(ctx: &RowContext<'_>, credentials: &DatabaseCredentials) -> Html {
    let locale = ctx.locale;
    let rows = std::iter::once(&credentials.root)
        .chain(credentials.tenants.iter())
        .map(|account| account_row(ctx, account))
        .collect::<Vec<_>>();
    html! {
        <table class="w-full text-sm">
            <thead>
                <tr class="text-left text-neutral-400">
                    <th class="py-1.5 font-medium">{locale.t("databases.account")}</th>
                    <th class="py-1.5 font-medium">{locale.t("databases.user")}</th>
                    <th class="py-1.5 font-medium">{locale.t("databases.password")}</th>
                    <th class="py-1.5 font-medium">{locale.t("databases.connection")}</th>
                    <th class="py-1.5 font-medium">{locale.t("databases.apps")}</th>
                    <th />
                </tr>
            </thead>
            <tbody>{ for rows }</tbody>
        </table>
    }
}

/// The accounts of a database with their passwords, above the list of the databases.
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
    html! {
        <div class="rounded border border-neutral-600 p-4 mb-4">
            <div class="flex items-center justify-between mb-2">
                <p class="font-medium">{title}</p>
                <button type="button" class="px-3 py-1.5 rounded bg-neutral-700 text-neutral-100 hover:bg-neutral-600 text-sm" onclick={close}>
                    {locale.t("common.ok")}
                </button>
            </div>
            <p class="text-sm text-neutral-400 mb-3">{locale.t("databases.credentials_hint")}</p>
            if let Some(error) = loaded.error.clone() {
                <p class="text-red-500">{error}</p>
            }
            if let Some(credentials) = loaded.credentials.as_ref() {
                { accounts_table(&ctx, credentials) }
            }
        </div>
    }
}
