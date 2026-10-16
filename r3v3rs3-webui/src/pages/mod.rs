use r3v3rs3_api::{cert::CertKind, id::ShortId};
use serde_derive::{Deserialize, Serialize};
use yew::prelude::*;
use yew_router::prelude::*;

mod access_lists;
mod accounts;
mod app_deployments;
mod app_env;
mod app_form;
mod app_git_token;
mod app_list;
mod app_log;
mod app_repository;
mod app_store;
mod app_terminal;
mod app_view;
mod app_webhook;
mod audit;
pub mod cert_list;
mod dns_credentials;
mod git_connections;
mod github_app;
mod log_view;
mod login;
mod logout;
mod new_acme;
mod new_port;
mod new_proxy;
mod port_list;
mod port_view;
mod proxy_list;
mod proxy_view;
mod resource_page;
mod self_sign;
mod settings;
mod store_env;
mod targets;
mod upload;

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize, Routable)]
#[serde(rename_all = "snake_case")]
pub enum Route {
    #[at("/")]
    Home,
    #[at("/login")]
    Login,
    #[at("/logout")]
    Logout,
    #[at("/ports")]
    Ports,
    #[at("/ports/new")]
    NewPort,
    #[at("/ports/:id")]
    PortView { id: ShortId },
    #[at("/ports/:id/log")]
    PortLogView { id: ShortId },
    #[at("/proxies")]
    Proxies,
    #[at("/proxies/:id/log")]
    ProxyLogView { id: ShortId },
    #[at("/certs")]
    Certs,
    #[at("/certs/client")]
    ClientCerts,
    #[at("/certs/root")]
    RootCerts,
    #[at("/certs/acme")]
    AcmeCerts,
    #[at("/certs/self_sign")]
    SelfSign,
    #[at("/certs/upload")]
    Upload,
    #[at("/certs/new_acme")]
    NewAcme,
    #[at("/certs/dns_credentials")]
    DnsCredentials,
    #[at("/certs/:id/log")]
    CertLogView { id: String },
    #[at("/proxies/new")]
    NewProxy,
    #[at("/proxies/:id")]
    ProxyView { id: ShortId },
    #[at("/access_lists")]
    AccessLists,
    #[at("/apps")]
    Apps,
    #[at("/apps/new")]
    NewApp,
    #[at("/apps/store")]
    AppStore,
    #[at("/apps/store/:store")]
    NewStoreApp { store: String },
    #[at("/apps/:id")]
    AppView { id: ShortId },
    #[at("/apps/:id/deployments")]
    AppDeployments { id: ShortId },
    #[at("/apps/:id/log")]
    AppLog { id: ShortId },
    #[at("/apps/:id/terminal")]
    AppTerminal { id: ShortId },
    #[at("/targets")]
    Targets,
    #[at("/git_connections")]
    GitConnections,
    #[at("/settings")]
    Settings,
    #[at("/accounts")]
    Accounts,
    #[at("/audit")]
    Audit,
    #[not_found]
    #[at("/404")]
    NotFound,
}

impl Route {
    pub fn root(&self) -> Option<Route> {
        match self {
            Route::Home => Some(Route::Home),
            Route::Ports | Route::NewPort | Route::PortView { .. } | Route::PortLogView { .. } => {
                Some(Route::Ports)
            }
            Route::Certs
            | Route::ClientCerts
            | Route::RootCerts
            | Route::AcmeCerts
            | Route::SelfSign
            | Route::Upload
            | Route::NewAcme
            | Route::DnsCredentials
            | Route::CertLogView { .. } => Some(Route::Certs),
            Route::Proxies
            | Route::NewProxy
            | Route::ProxyView { .. }
            | Route::ProxyLogView { .. } => Some(Route::Proxies),
            Route::Apps
            | Route::NewApp
            | Route::AppStore
            | Route::NewStoreApp { .. }
            | Route::AppView { .. }
            | Route::AppDeployments { .. }
            | Route::AppLog { .. }
            | Route::AppTerminal { .. } => Some(Route::Apps),
            Route::AccessLists
            | Route::Targets
            | Route::GitConnections
            | Route::Settings
            | Route::Accounts
            | Route::Audit => Some(self.clone()),
            _ => None,
        }
    }
}

pub fn switch(routes: Route) -> Html {
    match routes {
        Route::Home => html! { <Redirect<Route> to={Route::Ports}/> },
        Route::Login => html! { <login::Login /> },
        Route::Logout => html! { <logout::Logout /> },
        Route::Ports => html! { <port_list::PortList /> },
        Route::NewPort => html! { <new_port::NewPort /> },
        Route::PortView { id } => html! { <port_view::PortView {id} /> },
        Route::PortLogView { id } => html! { <log_view::LogView id={id.to_string()} /> },
        Route::Proxies => html! { <proxy_list::ProxyList /> },
        Route::ProxyLogView { id } => html! { <log_view::LogView id={id.to_string()} /> },
        Route::ProxyView { id } => html! { <proxy_view::ProxyView {id} /> },
        Route::NewProxy => html! { <new_proxy::NewProxy /> },
        Route::AccessLists => html! { <access_lists::AccessLists /> },
        Route::Apps => html! { <app_list::AppList /> },
        Route::AppStore => html! { <app_store::AppStore /> },
        Route::NewStoreApp { store } => {
            html! { <app_view::AppView store={Some(store)} /> }
        }
        Route::NewApp => html! { <app_view::AppView /> },
        Route::AppView { id } => html! { <app_view::AppView id={Some(id)} /> },
        Route::AppDeployments { id } => html! { <app_deployments::AppDeployments {id} /> },
        Route::AppLog { id } => html! { <app_log::AppLogView {id} /> },
        Route::AppTerminal { id } => html! { <app_terminal::AppTerminal {id} /> },
        Route::Targets => html! { <targets::Targets /> },
        Route::GitConnections => html! { <git_connections::GitConnections /> },
        Route::Certs => {
            html! { <cert_list::CertList kind={Some(CertKind::Server)} acme={false} /> }
        }
        Route::ClientCerts => {
            html! { <cert_list::CertList kind={Some(CertKind::Client)} acme={false} /> }
        }
        Route::RootCerts => {
            html! { <cert_list::CertList kind={Some(CertKind::Root)} acme={false} /> }
        }
        Route::AcmeCerts => html! { <cert_list::CertList kind={None} acme={true} /> },
        Route::SelfSign => html! { <self_sign::SelfSign /> },
        Route::NewAcme => html! { <new_acme::NewAcme /> },
        Route::DnsCredentials => html! { <dns_credentials::DnsCredentials /> },
        Route::CertLogView { id } => html! { <log_view::LogView {id} /> },
        Route::Upload => html! { <upload::Upload /> },
        Route::Settings => html! { <settings::Settings /> },
        Route::Accounts => html! { <accounts::Accounts /> },
        Route::Audit => html! { <audit::AuditLog /> },
        Route::NotFound => html! { <Redirect<Route> to={Route::Home}/> },
    }
}
