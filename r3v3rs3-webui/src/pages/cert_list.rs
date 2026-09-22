use crate::API_ENDPOINT;
use crate::auth::use_ensure_auth;
use crate::components::acme_form::dns_provider_label;
use crate::components::data_list::{
    Column, DANGER_LINK_CLASS, LINK_CLASS, Row, active_toggle, list_card, status_badge,
};
use crate::dialog;
use crate::format::{format_duration, unix_now};
use crate::i18n::use_locale;
use crate::pages::Route;
use crate::pages::self_sign::SelfSignQuery;
use crate::pages::settings::{failure_box, send_request, success_box};
use crate::store::{AcmeStore, CertStore, SessionStore};
use gloo_net::http::Request;
use r3v3rs3_api::acme::AcmeInfo;
use r3v3rs3_api::cert::{
    CertInfo, CertKind, DeleteCertResult, DeleteCertStatus, DeleteCertsRequest, ExpiryState,
    SelfSignedCertKind, UploadQuery, expiry_state,
};
use r3v3rs3_api::discovery::DiscoverySource;
use r3v3rs3_api::i18n::Locale;
use r3v3rs3_api::id::ShortId;
use serde_derive::{Deserialize, Serialize};
use std::{collections::HashSet, time::Duration};
use yew::prelude::*;
use yew_router::prelude::*;
use yewdux::prelude::*;

#[derive(Clone, Copy, Default, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum CertsTab {
    #[default]
    Server,
    Client,
    Root,
    Acme,
}

#[derive(Default, Clone, Serialize, Deserialize)]
pub struct CertsQuery {
    #[serde(default)]
    pub tab: CertsTab,
}

impl CertsTab {
    /// Returns the tab that lists certificates of this kind.
    pub fn for_kind(kind: CertKind) -> Self {
        match kind {
            CertKind::Server => CertsTab::Server,
            CertKind::Client => CertsTab::Client,
            CertKind::Root => CertsTab::Root,
        }
    }

    /// Returns the kind of the certificates in this tab. The ACME tab lists no certificates.
    fn cert_kind(self) -> Option<CertKind> {
        match self {
            CertsTab::Server => Some(CertKind::Server),
            CertsTab::Client => Some(CertKind::Client),
            CertsTab::Root => Some(CertKind::Root),
            CertsTab::Acme => None,
        }
    }

    fn label_key(self) -> &'static str {
        match self {
            CertsTab::Server => "certs.tab_server",
            CertsTab::Client => "certs.tab_client",
            CertsTab::Root => "certs.tab_root",
            CertsTab::Acme => "certs.tab_acme",
        }
    }
}

const TABS: [CertsTab; 4] = [
    CertsTab::Server,
    CertsTab::Client,
    CertsTab::Root,
    CertsTab::Acme,
];

const SERVER_COLUMNS: [Column; 4] = [
    Column {
        label: "certs.subject_names",
        class: "whitespace-nowrap",
    },
    Column {
        label: "certs.issuer",
        class: "",
    },
    Column {
        label: "certs.digest",
        class: "",
    },
    Column {
        label: "certs.expires_on",
        class: "",
    },
];

const ROOT_COLUMNS: [Column; 4] = [
    Column {
        label: "certs.issuer",
        class: "",
    },
    Column {
        label: "certs.digest",
        class: "",
    },
    Column {
        label: "certs.private_key",
        class: "",
    },
    Column {
        label: "certs.expires_on",
        class: "",
    },
];

const ACME_COLUMNS: [Column; 4] = [
    Column {
        label: "certs.subject_names",
        class: "",
    },
    Column {
        label: "certs.provider",
        class: "",
    },
    Column {
        label: "certs.renews_on",
        class: "",
    },
    Column {
        label: "common.active",
        class: "w-0 whitespace-nowrap text-center",
    },
];

const EMPTY_LIST: &str = "common.list_empty";

#[function_component(CertList)]
pub fn cert_list() -> Html {
    use_ensure_auth();
    let locale = use_locale();

    let location = use_location().unwrap();
    let query = location.query::<CertsQuery>().unwrap_or_default();
    let tab = use_state(|| query.tab);

    let (certs, certs_dispatcher) = use_store::<CertStore>();
    let (acme, acme_dispatcher) = use_store::<AcmeStore>();
    let (session, _) = use_store::<SessionStore>();
    let can_edit = session.can_edit();
    // Before the session loads, no certificate is marked as expiring.
    let warning = session
        .info
        .as_ref()
        .map(|info| info.cert_expiry_warning)
        .unwrap_or_default();
    let selected = use_state(HashSet::<ShortId>::new);
    let notice = use_state(|| Option::<Result<String, String>>::None);

    use_cert_lists(certs_dispatcher, acme_dispatcher);

    let navigator = use_navigator().unwrap();

    let navigator_cloned = navigator.clone();
    let self_sign_kind = match *tab == CertsTab::Client {
        true => SelfSignedCertKind::Client,
        false => SelfSignedCertKind::Server,
    };
    let self_sign_onclick = Callback::from(move |_| {
        let _ = navigator_cloned.push_with_query(
            &Route::SelfSign,
            &SelfSignQuery {
                kind: self_sign_kind,
            },
        );
    });

    let navigator_cloned = navigator.clone();
    let upload_kind = tab.cert_kind().unwrap_or(CertKind::Server);
    let upload_onclick = Callback::from(move |_| {
        let _ =
            navigator_cloned.push_with_query(&Route::Upload, &UploadQuery { kind: upload_kind });
    });

    let navigator_cloned = navigator.clone();
    let new_acme_onclick = Callback::from(move |_| {
        navigator_cloned.push(&Route::NewAcme);
    });

    let kind = tab.cert_kind();
    let cert_list = certs
        .entries
        .iter()
        .filter(|cert| Some(cert.kind) == kind)
        .collect::<Vec<_>>();
    // A deleted certificate or a certificate of another tab is no longer selected.
    let chosen = cert_list
        .iter()
        .map(|cert| cert.id)
        .filter(|id| selected.contains(id))
        .collect::<Vec<_>>();
    let delete_selected = delete_selected_onclick(locale, chosen.clone(), &selected, &notice);
    let body = body_view(
        locale,
        *tab,
        &CertLists {
            certs: &certs,
            acme: &acme,
            filtered: &cert_list,
        },
        &selected,
        &navigator,
        CertRowOpts { can_edit, warning },
    );

    let active_index = use_state(|| -1);
    html! {
        <>
        <div class="flex flex-col mb-4 lg:float-left">
            <div class="text-sm font-medium text-center text-neutral-500 dark:text-neutral-300">
                <ul class="flex overflow-x-auto lg:flex-col lg:w-48 lg:mr-2 -mb-px">
                    { tabs_view(locale, &navigator, &TabStates { tab: tab.clone(), selected: selected.clone(), notice: notice.clone(), active_index: active_index.clone() }) }

                </ul>
            </div>
        </div>
            { delete_notice(&notice) }
            { body }
            if can_edit && !chosen.is_empty() {
            <div class="flex justify-end mt-4">
                <button onclick={delete_selected} class="inline-flex items-center px-4 py-2 text-sm font-medium text-red-600 dark:text-red-500 bg-white dark:bg-neutral-800 border border-neutral-300 dark:border-neutral-700 rounded-lg hover:bg-neutral-100 hover:dark:bg-neutral-900 focus:z-10 focus:ring-4 focus:ring-neutral-200 dark:focus:ring-neutral-600">
                    {locale.tf("certs.delete_selected", &[("count", &chosen.len().to_string())])}
                </button>
            </div>
            }
            if can_edit {
            <div class="flex justify-end rounded-md mt-4 sm:ml-auto" role="group">
                if matches!(*tab, CertsTab::Server | CertsTab::Client) {
                    <button onclick={self_sign_onclick} class="inline-flex items-center px-4 py-2 text-sm font-medium text-neutral-500 dark:text-neutral-200 bg-white dark:bg-neutral-800 border border-neutral-300 dark:border-neutral-700 rounded-l-lg hover:bg-neutral-100 hover:dark:bg-neutral-900 focus:z-10 focus:ring-4 focus:ring-neutral-200 dark:focus:ring-neutral-600">
                        <img src="/assets/icons/create.svg" class="w-4 h-4 mr-1 text-neutral-500" />
                        {locale.t("certs.self_sign")}
                    </button>
                    <button onclick={upload_onclick} class="inline-flex items-center px-4 py-2 text-sm font-medium text-neutral-500 dark:text-neutral-200 bg-white dark:bg-neutral-800 border border-l-0 border-neutral-300 dark:border-neutral-700 rounded-r-lg hover:bg-neutral-100 hover:dark:bg-neutral-900 focus:z-10 focus:ring-4 focus:ring-neutral-200 dark:focus:ring-neutral-600">
                        <img src="/assets/icons/cloud-upload.svg" class="w-4 h-4 mr-1" />
                        {locale.t("common.upload")}
                    </button>
                } else if *tab == CertsTab::Root {
                    <button onclick={upload_onclick} class="inline-flex items-center px-4 py-2 text-sm font-medium text-neutral-500 dark:text-neutral-200 bg-white dark:bg-neutral-800 border border-neutral-300 dark:border-neutral-700 rounded-lg hover:bg-neutral-100 hover:dark:bg-neutral-900 focus:z-10 focus:ring-4 focus:ring-neutral-200 dark:focus:ring-neutral-600">
                        <img src="/assets/icons/cloud-upload.svg" class="w-4 h-4 mr-1" />
                        {locale.t("common.upload")}
                    </button>
                } else {
                    <button onclick={new_acme_onclick} class="inline-flex items-center px-4 py-2 text-sm font-medium text-neutral-500 dark:text-neutral-200 bg-white dark:bg-neutral-800 border border-neutral-300 dark:border-neutral-700 rounded-lg hover:bg-neutral-100 hover:dark:bg-neutral-900 focus:z-10 focus:ring-4 focus:ring-neutral-200 dark:focus:ring-neutral-600">
                        <img src="/assets/icons/add.svg" class="w-4 h-4 mr-1" />
                        {locale.t("common.add")}
                    </button>
                }
            </div>
            }
        </>
    }
}

/// Loads the certificates and the ACME requests into the stores.
#[hook]
fn use_cert_lists(certs: Dispatch<CertStore>, acme: Dispatch<AcmeStore>) {
    use_effect_with((), move |_| {
        wasm_bindgen_futures::spawn_local(async move {
            if let Ok(res) = get_cert_list().await {
                certs.set(CertStore {
                    entries: res,
                    loaded: true,
                });
            }
            if let Ok(res) = get_acme_list().await {
                acme.set(AcmeStore {
                    entries: res,
                    loaded: true,
                });
            }
        });
    });
}

/// The entries of the tabs. `filtered` holds the certificates of the open tab.
struct CertLists<'a> {
    certs: &'a CertStore,
    acme: &'a AcmeStore,
    filtered: &'a [&'a CertInfo],
}

/// The options that every row of the list shares.
#[derive(Clone, Copy)]
struct CertRowOpts {
    can_edit: bool,
    /// The period before the expiry that marks a certificate as expiring.
    warning: Duration,
}

/// The list of the open tab.
fn body_view(
    locale: Locale,
    tab: CertsTab,
    lists: &CertLists<'_>,
    selected: &UseStateHandle<HashSet<ShortId>>,
    navigator: &Navigator,
    opts: CertRowOpts,
) -> Html {
    match tab {
        CertsTab::Server | CertsTab::Client => {
            let rows = cert_rows(locale, lists.filtered, selected, opts, server_row);
            list_card(
                locale,
                lists.certs.loaded,
                EMPTY_LIST,
                &SERVER_COLUMNS,
                &rows,
            )
        }
        CertsTab::Root => {
            let rows = cert_rows(locale, lists.filtered, selected, opts, root_row);
            list_card(locale, lists.certs.loaded, EMPTY_LIST, &ROOT_COLUMNS, &rows)
        }
        CertsTab::Acme => {
            let rows = lists
                .acme
                .entries
                .iter()
                .map(|entry| acme_row(locale, entry, navigator, opts.can_edit))
                .collect::<Vec<_>>();
            list_card(locale, lists.acme.loaded, EMPTY_LIST, &ACME_COLUMNS, &rows)
        }
    }
}

/// The rows of the certificates, each with its selection box.
fn cert_rows(
    locale: Locale,
    entries: &[&CertInfo],
    selected: &UseStateHandle<HashSet<ShortId>>,
    opts: CertRowOpts,
    row: fn(Locale, &CertInfo, bool, Duration, Html) -> Row,
) -> Vec<Row> {
    entries
        .iter()
        .map(|entry| {
            let select = select_box(locale, entry, selected, opts.can_edit);
            row(locale, entry, opts.can_edit, opts.warning, select)
        })
        .collect()
}

/// The states that a tab change resets.
struct TabStates {
    tab: UseStateHandle<CertsTab>,
    selected: UseStateHandle<HashSet<ShortId>>,
    notice: UseStateHandle<Option<Result<String, String>>>,
    active_index: UseStateHandle<i32>,
}

impl TabStates {
    /// Opens the tab and resets the selection, the notice and the open menu.
    fn onclick(&self, navigator: &Navigator, item: CertsTab) -> Callback<MouseEvent> {
        let navigator = navigator.clone();
        let tab = self.tab.clone();
        let active_index = self.active_index.clone();
        let selected = self.selected.clone();
        let notice = self.notice.clone();
        Callback::from(move |_| {
            tab.set(item);
            active_index.set(-1);
            selected.set(HashSet::new());
            notice.set(None);
            let _ = navigator.push_with_query(&Route::Certs, &CertsQuery { tab: item });
        })
    }
}

fn tabs_view(locale: Locale, navigator: &Navigator, states: &TabStates) -> Html {
    TABS.into_iter()
        .map(|item| {
            let onclick = states.onclick(navigator, item);
            let class = tab_class(item == *states.tab);
            html! {
                <li class="mr-2 shrink-0 lg:mr-0">
                    <a {onclick} class={classes!("inline-block", "cursor-pointer", "border-2", "border-neutral-400", "px-4", "py-2", "rounded-md", "hover:bg-neutral-100", "dark:hover:bg-neutral-800", "whitespace-nowrap", "w-full", "lg:py-3", "lg:mb-2", class)}>{locale.t(item.label_key())}</a>
                </li>
            }
        })
        .collect()
}

fn tab_class(is_active: bool) -> Vec<&'static str> {
    match is_active {
        true => vec![
            "bg-neutral-100",
            "dark:bg-neutral-800",
            "text-neutral-600",
            "dark:text-neutral-200",
            "dark:text-neutral-100",
            "active",
        ],
        false => vec!["border-transparent", "dark:border-transparent"],
    }
}

fn delete_cert_onclick(locale: Locale, id: ShortId) -> Callback<MouseEvent> {
    Callback::from(move |e: MouseEvent| {
        e.prevent_default();
        let question = locale.tf("common.confirm_delete", &[("id", &id.to_string())]);
        dialog::confirm_then(locale, question, async move {
            let _ = delete_server_cert(id).await;
        });
    })
}

/// The checkbox that selects a certificate for deletion. A certificate that service discovery
/// manages, or an account that cannot change the certificates, gets none.
fn select_box(
    locale: Locale,
    entry: &CertInfo,
    selected: &UseStateHandle<HashSet<ShortId>>,
    can_edit: bool,
) -> Html {
    if !can_edit || entry.source.is_some() {
        return html! {};
    }
    let id = entry.id;
    let checked = selected.contains(&id);
    let selected = selected.clone();
    let onchange = Callback::from(move |_: Event| {
        let mut ids = (*selected).clone();
        if !ids.remove(&id) {
            ids.insert(id);
        }
        selected.set(ids);
    });
    html! {
        <input type="checkbox" {checked} {onchange} aria-label={locale.t("certs.select")} class="w-4 h-4 mr-2 rounded align-middle" />
    }
}

/// Deletes the selected certificates after a confirmation and shows what happened to each one.
fn delete_selected_onclick(
    locale: Locale,
    ids: Vec<ShortId>,
    selected: &UseStateHandle<HashSet<ShortId>>,
    notice: &UseStateHandle<Option<Result<String, String>>>,
) -> Callback<MouseEvent> {
    let selected = selected.clone();
    let notice = notice.clone();
    Callback::from(move |_: MouseEvent| {
        let count = ids.len().to_string();
        let question = locale.tf("certs.confirm_delete_selected", &[("count", &count)]);
        let ids = ids.clone();
        let selected = selected.clone();
        let notice = notice.clone();
        dialog::confirm_then(locale, question, async move {
            let result = delete_certs(locale, ids).await;
            selected.set(HashSet::new());
            notice.set(Some(
                result.and_then(|results| delete_summary(locale, &results)),
            ));
        });
    })
}

fn delete_notice(notice: &Option<Result<String, String>>) -> Html {
    match notice {
        Some(Ok(message)) => {
            success_box(html! { <span class="block sm:inline">{message.clone()}</span> })
        }
        Some(Err(message)) => failure_box(message),
        None => html! {},
    }
}

/// The message of a delete request. It is an error when a certificate is not deleted, and it
/// names each such certificate with the cause.
fn delete_summary(locale: Locale, results: &[DeleteCertResult]) -> Result<String, String> {
    let deleted = results
        .iter()
        .filter(|result| result.status == DeleteCertStatus::Deleted)
        .count()
        .to_string();
    let kept = results
        .iter()
        .filter(|result| result.status != DeleteCertStatus::Deleted)
        .map(|result| format!("{} ({})", result.id, locale.t(status_key(result.status))))
        .collect::<Vec<_>>();
    if kept.is_empty() {
        return Ok(locale.tf("certs.deleted_selected", &[("count", &deleted)]));
    }
    Err(locale.tf(
        "certs.kept_selected",
        &[("count", &deleted), ("kept", &kept.join(", "))],
    ))
}

fn status_key(status: DeleteCertStatus) -> &'static str {
    match status {
        DeleteCertStatus::Deleted => "certs.status_deleted",
        DeleteCertStatus::InUse => "certs.status_in_use",
        DeleteCertStatus::ReadOnly => "certs.status_read_only",
        DeleteCertStatus::NotFound => "certs.status_not_found",
        DeleteCertStatus::Failed => "certs.status_failed",
    }
}

async fn delete_certs(locale: Locale, ids: Vec<ShortId>) -> Result<Vec<DeleteCertResult>, String> {
    let request = Request::post(&format!("{API_ENDPOINT}/certs/delete"))
        .json(&DeleteCertsRequest { ids })
        .map_err(|err| err.to_string())?;
    send_request(locale, request)
        .await?
        .json()
        .await
        .map_err(|err| err.to_string())
}

/// Downloads the certificate. A file with a private key needs a confirmation first.
fn download_onclick(locale: Locale, id: ShortId, has_private_key: bool) -> Callback<MouseEvent> {
    Callback::from(move |e: MouseEvent| {
        e.prevent_default();
        let download = async move {
            location::assign(&format!("{API_ENDPOINT}/certs/{id}/download"));
        };
        if has_private_key {
            let question = locale.tf("certs.confirm_download", &[("id", &id.to_string())]);
            dialog::confirm_then(locale, question, download);
        } else {
            wasm_bindgen_futures::spawn_local(download);
        }
    })
}

/// The download action, and the delete action of a certificate that service discovery does not
/// manage. An account that cannot change the certificates gets no action.
fn cert_actions(locale: Locale, entry: &CertInfo, has_private_key: bool, can_edit: bool) -> Html {
    if !can_edit {
        return html! {};
    }
    html! {
        <>
            <a class={LINK_CLASS} onclick={download_onclick(locale, entry.id, has_private_key)}>{locale.t("certs.download")}</a>
            if entry.source.is_none() {
                <a class={DANGER_LINK_CLASS} onclick={delete_cert_onclick(locale, entry.id)}>{locale.t("common.delete")}</a>
            }
        </>
    }
}

/// The name of the discovery provider of the certificate. The title names the resource.
fn source_badge(locale: Locale, entry: &CertInfo) -> Html {
    let Some(source) = &entry.source else {
        return html! {};
    };
    html! {
        <span title={discovered_title(locale, source)} class="ml-2 px-2 py-0.5 text-xs font-medium rounded bg-neutral-200 text-neutral-700 dark:bg-neutral-700 dark:text-neutral-200">
            {source.provider.name()}
        </span>
    }
}

fn discovered_title(locale: Locale, source: &DiscoverySource) -> String {
    locale.tf("certs.discovered_title", &[("resource", &source.resource)])
}

/// The expiry date with a badge when the certificate expires within `warning` or has expired.
fn expiry_cell(locale: Locale, not_after: i64, warning: Duration) -> Html {
    let date = format_duration(locale, not_after);
    match expiry_state(not_after, unix_now(), warning) {
        ExpiryState::Valid => html! { <>{date}</> },
        ExpiryState::Expiring => html! {
            <>{status_badge(locale.t("certs.expiring"), "bg-yellow-400")}<div>{date}</div></>
        },
        ExpiryState::Expired => html! {
            <>{status_badge(locale.t("certs.expired"), "bg-red-500")}<div>{date}</div></>
        },
    }
}

fn server_row(
    locale: Locale,
    entry: &CertInfo,
    can_edit: bool,
    warning: Duration,
    select: Html,
) -> Row {
    let subject_names = entry
        .san
        .iter()
        .map(|name| name.to_string())
        .collect::<Vec<_>>()
        .join(", ");
    Row {
        key: entry.id.to_string(),
        cells: vec![
            html! { <>{select}{subject_names}{source_badge(locale, entry)}</> },
            html! { <>{entry.issuer.clone()}</> },
            html! { <>{entry.id.to_string()}</> },
            expiry_cell(locale, entry.not_after, warning),
        ],
        actions: cert_actions(locale, entry, true, can_edit),
    }
}

fn root_row(
    locale: Locale,
    entry: &CertInfo,
    can_edit: bool,
    warning: Duration,
    select: Html,
) -> Row {
    let private_key = if entry.has_private_key {
        "common.yes"
    } else {
        "common.no"
    };
    Row {
        key: entry.id.to_string(),
        cells: vec![
            html! { <>{select}{entry.issuer.clone()}{source_badge(locale, entry)}</> },
            html! { <>{entry.id.to_string()}</> },
            html! { <>{locale.t(private_key)}</> },
            expiry_cell(locale, entry.not_after, warning),
        ],
        actions: cert_actions(locale, entry, entry.has_private_key, can_edit),
    }
}

fn acme_row(locale: Locale, entry: &AcmeInfo, navigator: &Navigator, can_edit: bool) -> Row {
    let id = entry.id;
    let delete_onclick = Callback::from(move |e: MouseEvent| {
        e.prevent_default();
        let question = locale.tf("common.confirm_delete", &[("id", &id.to_string())]);
        dialog::confirm_then(locale, question, async move {
            let _ = delete_acme(id).await;
        });
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

    let renewal = entry
        .next_renewal
        .map(|time| format_duration(locale, time))
        .unwrap_or_default();
    Row {
        key: id.to_string(),
        cells: vec![
            html! { <>{entry.identifiers.join(", ")}</> },
            html! { <>{acme_provider_text(entry)}</> },
            html! { <>{renewal}</> },
            active_toggle(entry.config.active, !can_edit, onchange),
        ],
        actions: html! {
            <>
                <a class={LINK_CLASS} onclick={log_onclick}>{locale.t("common.log")}</a>
                if can_edit {
                    <a class={DANGER_LINK_CLASS} onclick={delete_onclick}>{locale.t("common.delete")}</a>
                }
            </>
        },
    }
}

/// The provider with the challenge, for example `Let's Encrypt (DNS-01, Cloudflare)`.
fn acme_provider_text(entry: &AcmeInfo) -> String {
    let challenge = entry.challenge_type.to_uppercase();
    match &entry.dns_provider {
        Some(name) => format!(
            "{} ({challenge}, {})",
            entry.config.provider,
            dns_provider_label(name)
        ),
        None => format!("{} ({challenge})", entry.config.provider),
    }
}

pub async fn get_cert_list() -> Result<Vec<CertInfo>, gloo_net::Error> {
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_certificate_kind_has_its_own_tab() {
        for kind in [CertKind::Server, CertKind::Client, CertKind::Root] {
            assert_eq!(CertsTab::for_kind(kind).cert_kind(), Some(kind));
        }
        assert_eq!(CertsTab::Acme.cert_kind(), None);
    }

    #[test]
    fn the_title_of_a_discovered_certificate_names_its_resource() {
        let source = DiscoverySource {
            provider: r3v3rs3_api::discovery::DiscoveryProvider::Kubernetes,
            resource: "secret default/app-tls".into(),
        };
        assert_eq!(
            discovered_title(Locale::En, &source),
            "Service discovery manages this certificate: secret default/app-tls"
        );
        assert_eq!(
            discovered_title(Locale::Tr, &source),
            "Bu sertifikayı servis keşfi yönetir: secret default/app-tls"
        );
    }

    #[test]
    fn the_summary_of_a_delete_request_names_each_certificate_that_stays() {
        let result = |id: &str, status| DeleteCertResult {
            id: id.parse().unwrap(),
            status,
        };
        assert_eq!(
            delete_summary(Locale::En, &[result("abc", DeleteCertStatus::Deleted)]),
            Ok("Deleted certificates: 1.".to_string())
        );
        let results = [
            result("abc", DeleteCertStatus::Deleted),
            result("abd", DeleteCertStatus::InUse),
            result("abe", DeleteCertStatus::ReadOnly),
        ];
        assert_eq!(
            delete_summary(Locale::En, &results),
            Err(
                "Deleted certificates: 1. Not deleted: abd (in use), abe (managed by service discovery)"
                    .to_string()
            )
        );
    }

    #[test]
    fn the_acme_provider_shows_the_challenge_and_the_dns_provider() {
        let mut entry = AcmeInfo {
            id: "acme1".parse().unwrap(),
            config: r3v3rs3_api::acme::AcmeConfig {
                provider: "Let's Encrypt".into(),
                ..Default::default()
            },
            identifiers: vec!["*.example.com".into()],
            challenge_type: "dns-01".into(),
            dns_provider: Some("route53".into()),
            next_renewal: None,
        };
        assert_eq!(
            acme_provider_text(&entry),
            "Let's Encrypt (DNS-01, Route 53)"
        );
        entry.challenge_type = "http-01".into();
        entry.dns_provider = None;
        assert_eq!(acme_provider_text(&entry), "Let's Encrypt (HTTP-01)");
        entry.challenge_type = "tls-alpn-01".into();
        assert_eq!(acme_provider_text(&entry), "Let's Encrypt (TLS-ALPN-01)");
    }
}
