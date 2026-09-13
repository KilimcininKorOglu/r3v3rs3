use std::fmt::Display;

use crate::auth::use_ensure_auth;
use crate::components::data_list::{
    active_toggle, list_card, Column, Row, DANGER_LINK_CLASS, LINK_CLASS,
};
use crate::format::format_duration;
use crate::pages::Route;
use crate::store::{AcmeStore, CertStore};
use crate::API_ENDPOINT;
use gloo_net::http::Request;
use r3v3rs3_api::acme::AcmeInfo;
use r3v3rs3_api::cert::{CertInfo, CertKind, UploadQuery};
use r3v3rs3_api::id::ShortId;
use serde_derive::{Deserialize, Serialize};
use yew::prelude::*;
use yew_router::prelude::*;
use yewdux::prelude::*;

#[derive(Clone, Copy, Default, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum CertsTab {
    #[default]
    Server,
    Root,
    Acme,
}

#[derive(Default, Clone, Serialize, Deserialize)]
pub struct CertsQuery {
    #[serde(default)]
    pub tab: CertsTab,
}

impl Display for CertsTab {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            CertsTab::Server => write!(f, "Server Certs"),
            CertsTab::Root => write!(f, "Root Certs"),
            CertsTab::Acme => write!(f, "ACME"),
        }
    }
}

const TABS: [CertsTab; 3] = [CertsTab::Server, CertsTab::Root, CertsTab::Acme];

const SERVER_COLUMNS: [Column; 4] = [
    Column {
        label: "Subject Names",
        class: "whitespace-nowrap",
    },
    Column {
        label: "Issuer",
        class: "",
    },
    Column {
        label: "Digest",
        class: "",
    },
    Column {
        label: "Expires on",
        class: "",
    },
];

const ROOT_COLUMNS: [Column; 4] = [
    Column {
        label: "Issuer",
        class: "",
    },
    Column {
        label: "Digest",
        class: "",
    },
    Column {
        label: "Private Key",
        class: "",
    },
    Column {
        label: "Expires on",
        class: "",
    },
];

const ACME_COLUMNS: [Column; 4] = [
    Column {
        label: "Subject Names",
        class: "",
    },
    Column {
        label: "Provider",
        class: "",
    },
    Column {
        label: "Renews on",
        class: "",
    },
    Column {
        label: "Active",
        class: "w-0 whitespace-nowrap text-center",
    },
];

const EMPTY_LIST: &str = "List is empty.";

#[function_component(CertList)]
pub fn cert_list() -> Html {
    use_ensure_auth();

    let location = use_location().unwrap();
    let query = location.query::<CertsQuery>().unwrap_or_default();
    let tab = use_state(|| query.tab);

    let (certs, certs_dispatcher) = use_store::<CertStore>();
    let (acme, acme_dispatcher) = use_store::<AcmeStore>();

    use_effect_with((), move |_| {
        wasm_bindgen_futures::spawn_local(async move {
            if let Ok(res) = get_cert_list().await {
                certs_dispatcher.set(CertStore {
                    entries: res,
                    loaded: true,
                });
            }
            if let Ok(res) = get_acme_list().await {
                acme_dispatcher.set(AcmeStore {
                    entries: res,
                    loaded: true,
                });
            }
        });
    });

    let navigator = use_navigator().unwrap();

    let navigator_cloned = navigator.clone();
    let self_sign_onclick = Callback::from(move |_| {
        navigator_cloned.push(&Route::SelfSign);
    });

    let navigator_cloned = navigator.clone();
    let tab_cloned = tab.clone();
    let upload_onclick = Callback::from(move |_| {
        let _ = navigator_cloned.push_with_query(
            &Route::Upload,
            &UploadQuery {
                kind: if *tab_cloned == CertsTab::Server {
                    CertKind::Server
                } else {
                    CertKind::Root
                },
            },
        );
    });

    let navigator_cloned = navigator.clone();
    let new_acme_onclick = Callback::from(move |_| {
        navigator_cloned.push(&Route::NewAcme);
    });

    let kind = if *tab == CertsTab::Server {
        CertKind::Server
    } else {
        CertKind::Root
    };
    let cert_list = certs
        .entries
        .iter()
        .filter(|cert| cert.kind == kind)
        .collect::<Vec<_>>();
    let body = match *tab {
        CertsTab::Server => {
            let rows = cert_list
                .iter()
                .map(|entry| server_row(entry))
                .collect::<Vec<_>>();
            list_card(certs.loaded, EMPTY_LIST, &SERVER_COLUMNS, &rows)
        }
        CertsTab::Root => {
            let rows = cert_list
                .iter()
                .map(|entry| root_row(entry))
                .collect::<Vec<_>>();
            list_card(certs.loaded, EMPTY_LIST, &ROOT_COLUMNS, &rows)
        }
        CertsTab::Acme => {
            let rows = acme
                .entries
                .iter()
                .map(|entry| acme_row(entry, &navigator))
                .collect::<Vec<_>>();
            list_card(acme.loaded, EMPTY_LIST, &ACME_COLUMNS, &rows)
        }
    };

    let active_index = use_state(|| -1);
    html! {
        <>
        <div class="flex flex-col mb-4 lg:float-left">
            <div class="text-sm font-medium text-center text-neutral-500 dark:text-neutral-300">
                <ul class="flex overflow-x-auto lg:flex-col lg:w-48 lg:mr-2 -mb-px">
                    { TABS.into_iter().map(|item| {
                        let navigator = navigator.clone();
                        let active_index = active_index.clone();
                        let is_active = item == *tab;
                        let tab = tab.clone();
                        let onclick = Callback::from(move |_|  {
                            tab.set(item);
                            active_index.set(-1);
                            let _ = navigator.push_with_query(&Route::Certs, &CertsQuery { tab: item });
                        });
                        let class = if is_active {
                            vec!["bg-neutral-100", "dark:bg-neutral-800", "text-neutral-600", "dark:text-neutral-200", "dark:text-neutral-100", "active"]
                        } else {
                            vec!["border-transparent", "dark:border-transparent"]
                        };
                        html! {
                            <li class="mr-2 shrink-0 lg:mr-0">
                                <a {onclick} class={classes!("inline-block", "cursor-pointer", "border-2", "border-neutral-400", "px-4", "py-2", "rounded-md", "hover:bg-neutral-100", "dark:hover:bg-neutral-800", "whitespace-nowrap", "w-full", "lg:py-3", "lg:mb-2", class)}>{item.to_string()}</a>
                            </li>
                        }
                    }).collect::<Html>() }

                </ul>
            </div>
        </div>
            { body }
            <div class="flex justify-end rounded-md mt-4 sm:ml-auto" role="group">
                if *tab == CertsTab::Server {
                    <button onclick={self_sign_onclick} class="inline-flex items-center px-4 py-2 text-sm font-medium text-neutral-500 dark:text-neutral-200 bg-white dark:bg-neutral-800 border border-neutral-300 dark:border-neutral-700 rounded-l-lg hover:bg-neutral-100 hover:dark:bg-neutral-900 focus:z-10 focus:ring-4 focus:ring-neutral-200 dark:focus:ring-neutral-600">
                        <img src="/assets/icons/create.svg" class="w-4 h-4 mr-1 text-neutral-500" />
                        {"Self-sign"}
                    </button>
                    <button onclick={upload_onclick} class="inline-flex items-center px-4 py-2 text-sm font-medium text-neutral-500 dark:text-neutral-200 bg-white dark:bg-neutral-800 border border-l-0 border-neutral-300 dark:border-neutral-700 rounded-r-lg hover:bg-neutral-100 hover:dark:bg-neutral-900 focus:z-10 focus:ring-4 focus:ring-neutral-200 dark:focus:ring-neutral-600">
                        <img src="/assets/icons/cloud-upload.svg" class="w-4 h-4 mr-1" />
                        {"Upload"}
                    </button>
                } else if *tab == CertsTab::Root {
                    <button onclick={upload_onclick} class="inline-flex items-center px-4 py-2 text-sm font-medium text-neutral-500 dark:text-neutral-200 bg-white dark:bg-neutral-800 border border-neutral-300 dark:border-neutral-700 rounded-lg hover:bg-neutral-100 hover:dark:bg-neutral-900 focus:z-10 focus:ring-4 focus:ring-neutral-200 dark:focus:ring-neutral-600">
                        <img src="/assets/icons/cloud-upload.svg" class="w-4 h-4 mr-1" />
                        {"Upload"}
                    </button>
                } else {
                    <button onclick={new_acme_onclick} class="inline-flex items-center px-4 py-2 text-sm font-medium text-neutral-500 dark:text-neutral-200 bg-white dark:bg-neutral-800 border border-neutral-300 dark:border-neutral-700 rounded-lg hover:bg-neutral-100 hover:dark:bg-neutral-900 focus:z-10 focus:ring-4 focus:ring-neutral-200 dark:focus:ring-neutral-600">
                        <img src="/assets/icons/add.svg" class="w-4 h-4 mr-1" />
                        {"Add"}
                    </button>
                }
            </div>
        </>
    }
}

fn delete_cert_onclick(id: ShortId) -> Callback<MouseEvent> {
    Callback::from(move |e: MouseEvent| {
        e.prevent_default();
        if gloo_dialogs::confirm(&format!("Are you sure to delete {id}?")) {
            wasm_bindgen_futures::spawn_local(async move {
                let _ = delete_server_cert(id).await;
            });
        }
    })
}

/// Downloads the certificate. A file with a private key needs a confirmation first.
fn download_onclick(id: ShortId, has_private_key: bool) -> Callback<MouseEvent> {
    Callback::from(move |e: MouseEvent| {
        e.prevent_default();
        if !has_private_key
            || gloo_dialogs::confirm(&format!(
                "Are you sure to download {id}.tar.gz?\nThis file contains the unencrypted private key."
            ))
        {
            location::assign(&format!("{API_ENDPOINT}/certs/{id}/download"));
        }
    })
}

fn cert_actions(entry: &CertInfo, has_private_key: bool) -> Html {
    html! {
        <>
            <a class={LINK_CLASS} onclick={download_onclick(entry.id, has_private_key)}>{"Download"}</a>
            <a class={DANGER_LINK_CLASS} onclick={delete_cert_onclick(entry.id)}>{"Delete"}</a>
        </>
    }
}

fn server_row(entry: &CertInfo) -> Row {
    let subject_names = entry
        .san
        .iter()
        .map(|name| name.to_string())
        .collect::<Vec<_>>()
        .join(", ");
    Row {
        key: entry.id.to_string(),
        cells: vec![
            html! { <>{subject_names}</> },
            html! { <>{entry.issuer.clone()}</> },
            html! { <>{entry.id.to_string()}</> },
            html! { <>{format_duration(entry.not_after)}</> },
        ],
        actions: cert_actions(entry, true),
    }
}

fn root_row(entry: &CertInfo) -> Row {
    let private_key = if entry.has_private_key { "Yes" } else { "No" };
    Row {
        key: entry.id.to_string(),
        cells: vec![
            html! { <>{entry.issuer.clone()}</> },
            html! { <>{entry.id.to_string()}</> },
            html! { <>{private_key}</> },
            html! { <>{format_duration(entry.not_after)}</> },
        ],
        actions: cert_actions(entry, entry.has_private_key),
    }
}

fn acme_row(entry: &AcmeInfo, navigator: &Navigator) -> Row {
    let id = entry.id;
    let delete_onclick = Callback::from(move |e: MouseEvent| {
        e.prevent_default();
        if gloo_dialogs::confirm(&format!("Are you sure to delete {id}?")) {
            wasm_bindgen_futures::spawn_local(async move {
                let _ = delete_acme(id).await;
            });
        }
    });

    let navigator = navigator.clone();
    let log_onclick = Callback::from(move |_| {
        let id = id.to_string();
        navigator.push(&Route::CertLogView { id });
    });

    let onchange = Callback::from(move |_: Event| {
        wasm_bindgen_futures::spawn_local(async move {
            let _ = toggle_acme(id).await;
        });
    });

    Row {
        key: id.to_string(),
        cells: vec![
            html! { <>{entry.identifiers.join(", ")}</> },
            html! { <>{entry.config.provider.to_string()}</> },
            html! { <>{entry.next_renewal.map(format_duration).unwrap_or_default()}</> },
            active_toggle(entry.config.active, onchange),
        ],
        actions: html! {
            <>
                <a class={LINK_CLASS} onclick={log_onclick}>{"Log"}</a>
                <a class={DANGER_LINK_CLASS} onclick={delete_onclick}>{"Delete"}</a>
            </>
        },
    }
}

async fn get_cert_list() -> Result<Vec<CertInfo>, gloo_net::Error> {
    Request::get(&format!("{API_ENDPOINT}/certs"))
        .send()
        .await?
        .json()
        .await
}

async fn get_acme_list() -> Result<Vec<AcmeInfo>, gloo_net::Error> {
    Request::get(&format!("{API_ENDPOINT}/acme"))
        .send()
        .await?
        .json()
        .await
}

async fn delete_server_cert(id: ShortId) -> Result<(), gloo_net::Error> {
    Request::delete(&format!("{API_ENDPOINT}/certs/{id}"))
        .send()
        .await?;
    Ok(())
}

async fn delete_acme(id: ShortId) -> Result<(), gloo_net::Error> {
    Request::delete(&format!("{API_ENDPOINT}/acme/{id}"))
        .send()
        .await?;
    Ok(())
}

async fn toggle_acme(id: ShortId) -> Result<(), gloo_net::Error> {
    let mut acme: AcmeInfo = Request::get(&format!("{API_ENDPOINT}/acme/{id}"))
        .send()
        .await?
        .json()
        .await?;
    acme.config.active = !acme.config.active;
    Request::put(&format!("{API_ENDPOINT}/acme/{id}"))
        .json(&acme.config)?
        .send()
        .await?;
    Ok(())
}

mod location {
    use wasm_bindgen::prelude::*;

    #[wasm_bindgen]
    extern "C" {
        #[wasm_bindgen(js_namespace = location)]
        pub fn assign(url: &str);
    }
}
