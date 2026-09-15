use crate::{
    components::data_list::status_badge, i18n::use_locale, store::ClusterStore, API_ENDPOINT,
};
use gloo_net::http::Request;
use r3v3rs3_api::cluster::{ClusterState, ClusterStatus};
use yew::prelude::*;
use yew_router::prelude::*;
use yewdux::prelude::*;

/// A warning on every page while this node serves the last state without the cluster store.
///
/// The banner reads the status again on each page change, so a status that the login page could
/// not read shows after the login.
#[function_component(ClusterBanner)]
pub fn cluster_banner() -> Html {
    let locale = use_locale();
    let (store, dispatch) = use_store::<ClusterStore>();
    let path = use_location().map(|location| location.path().to_string());

    use_effect_with(path, move |_| {
        wasm_bindgen_futures::spawn_local(async move {
            if let Ok(status) = get_status().await {
                dispatch.set(ClusterStore { status });
            }
        });
    });

    if store.status.state != ClusterState::Degraded {
        return html! {};
    }
    html! {
        <div class="mb-4 bg-red-100 border border-red-400 text-red-700 dark:bg-red-950 dark:border-red-800 dark:text-red-300 px-4 py-3 rounded" role="alert">
            {locale.t("cluster.degraded")}
        </div>
    }
}

/// The state, the role and the applied revision of this node. The card is empty when the node
/// does not use a cluster store.
#[function_component(ClusterStatusCard)]
pub fn cluster_status_card() -> Html {
    let locale = use_locale();
    let (store, _) = use_store::<ClusterStore>();
    let status = &store.status;
    let Some((state_key, color)) = state_badge(status.state) else {
        return html! {};
    };
    let role = locale.t(if status.leader {
        "cluster.leader"
    } else {
        "cluster.follower"
    });
    let revision = locale.tf(
        "cluster.revision",
        &[("revision", &status.revision.to_string())],
    );
    html! {
        <div class="mt-4 bg-white dark:bg-neutral-800 shadow-sm p-5 border border-neutral-300 dark:border-neutral-700 rounded-md">
            <h2 class="text-lg font-semibold text-neutral-900 dark:text-neutral-200">{locale.t("cluster.title")}</h2>
            <div class="mt-3 flex flex-wrap items-center gap-x-4 text-sm text-neutral-900 dark:text-neutral-200">
                <span class="font-medium">{&status.node_name}</span>
                { status_badge(locale.t(state_key), color) }
                <span class="text-neutral-500 dark:text-neutral-400">{role}</span>
                <span class="text-neutral-500 dark:text-neutral-400">{revision}</span>
            </div>
            if let Some(error) = &status.error {
                <p class="mt-1 text-sm text-red-600 dark:text-red-500">{error}</p>
            }
        </div>
    }
}

/// The translation key and the dot color of the state. A node without a cluster store has no
/// badge.
fn state_badge(state: ClusterState) -> Option<(&'static str, &'static str)> {
    match state {
        ClusterState::Disabled => None,
        ClusterState::Syncing => Some(("cluster.state_syncing", "bg-yellow-400")),
        ClusterState::Synced => Some(("cluster.state_synced", "bg-green-500")),
        ClusterState::Degraded => Some(("cluster.state_degraded", "bg-red-500")),
    }
}

async fn get_status() -> Result<ClusterStatus, gloo_net::Error> {
    Request::get(&format!("{API_ENDPOINT}/cluster/status"))
        .send()
        .await?
        .json()
        .await
}

#[cfg(test)]
mod tests {
    use super::*;
    use r3v3rs3_api::i18n::Locale;

    #[test]
    fn every_cluster_state_except_disabled_has_a_translated_name() {
        assert_eq!(state_badge(ClusterState::Disabled), None);
        for state in [
            ClusterState::Syncing,
            ClusterState::Synced,
            ClusterState::Degraded,
        ] {
            let (key, _) = state_badge(state).unwrap();
            assert_ne!(Locale::Tr.t(key), key);
            assert_ne!(Locale::En.t(key), key);
        }
        for key in [
            "cluster.degraded",
            "cluster.follower",
            "cluster.leader",
            "cluster.revision",
            "cluster.title",
        ] {
            assert_ne!(Locale::Tr.t(key), key);
        }
    }
}
