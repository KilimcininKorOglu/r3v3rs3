use crate::{
    API_ENDPOINT, components::data_list::status_badge, i18n::use_locale, store::DiscoveryStore,
};
use gloo_net::http::Request;
use r3v3rs3_api::discovery::{DiscoveryIssue, DiscoveryState, DiscoveryStatus};
use r3v3rs3_api::i18n::Locale;
use yew::prelude::*;
use yewdux::prelude::*;

/// The state, the proxy count and the issues of each discovery provider. The card is empty when
/// no provider runs.
#[function_component(DiscoveryStatusCard)]
pub fn discovery_status_card() -> Html {
    let locale = use_locale();
    let (store, dispatch) = use_store::<DiscoveryStore>();

    use_effect_with((), move |_| {
        wasm_bindgen_futures::spawn_local(async move {
            if let Ok(entries) = get_statuses().await {
                dispatch.set(DiscoveryStore { entries });
            }
        });
    });

    if store.entries.is_empty() {
        return html! {};
    }
    html! {
        <div class="mb-4 bg-white dark:bg-neutral-800 shadow-sm p-5 border border-neutral-300 dark:border-neutral-700 rounded-md">
            <h2 class="text-lg font-semibold text-neutral-900 dark:text-neutral-200">{locale.t("discovery.title")}</h2>
            { for store.entries.iter().map(|status| provider_view(locale, status)) }
        </div>
    }
}

fn provider_view(locale: Locale, status: &DiscoveryStatus) -> Html {
    let (state_key, color) = state_badge(status.state);
    let proxies = locale.tf(
        "discovery.proxies",
        &[("count", &status.proxies.to_string())],
    );
    html! {
        <div class="mt-3">
            <div class="flex flex-wrap items-center gap-x-4 text-sm text-neutral-900 dark:text-neutral-200">
                <span class="font-medium">{status.provider.name()}</span>
                { status_badge(locale.t(state_key), color) }
                <span class="text-neutral-500 dark:text-neutral-400">{proxies}</span>
            </div>
            if let Some(error) = &status.error {
                <p class="mt-1 text-sm text-red-600 dark:text-red-500">{error}</p>
            }
            if !status.issues.is_empty() {
                <ul class="mt-1 list-disc list-inside text-sm text-orange-700 dark:text-orange-400">
                    { for status.issues.iter().map(|issue| html! { <li>{issue_text(issue)}</li> }) }
                </ul>
            }
        </div>
    }
}

/// The translation key and the dot color of the state.
fn state_badge(state: DiscoveryState) -> (&'static str, &'static str) {
    match state {
        DiscoveryState::Connecting => ("discovery.state_connecting", "bg-yellow-400"),
        DiscoveryState::Running => ("discovery.state_running", "bg-green-500"),
        DiscoveryState::Error => ("discovery.state_error", "bg-red-500"),
    }
}

fn issue_text(issue: &DiscoveryIssue) -> String {
    format!("{}: {}", issue.resource, issue.message)
}

async fn get_statuses() -> Result<Vec<DiscoveryStatus>, gloo_net::Error> {
    Request::get(&format!("{API_ENDPOINT}/discovery"))
        .send()
        .await?
        .json()
        .await
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_state_has_a_translated_name() {
        for state in [
            DiscoveryState::Connecting,
            DiscoveryState::Running,
            DiscoveryState::Error,
        ] {
            let (key, _) = state_badge(state);
            assert_ne!(Locale::Tr.t(key), key);
        }
        let issue = DiscoveryIssue {
            resource: "web-1".into(),
            message: "http.app: port not found: https".into(),
        };
        assert_eq!(issue_text(&issue), "web-1: http.app: port not found: https");
    }
}
