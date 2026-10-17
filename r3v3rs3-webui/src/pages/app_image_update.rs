//! The image updates section of an app. Shows the current tag and digest, checks for newer
//! tags and updated digests, and lets the user deploy updates.

use super::accounts::HINT_CLASS;
use super::resource_page::BUTTON_CLASS;
use super::settings::fetch_json;
use crate::API_ENDPOINT;
use crate::dialog::{self, Icon};
use crate::format::format_time;
use gloo_net::http::Request;
use r3v3rs3_api::i18n::Locale;
use r3v3rs3_api::platform::{AppEntry, AppSource, DeploymentEntry, ImageUpdateStatus};
use wasm_bindgen_futures::spawn_local;
use yew::prelude::*;

const PRIMARY_BUTTON_CLASS: &str = "inline-flex justify-center items-center text-white bg-blue-600 hover:bg-blue-700 focus:ring-4 focus:ring-blue-300 font-medium rounded-lg text-sm px-4 py-2 dark:bg-blue-600 dark:hover:bg-blue-700 dark:focus:ring-blue-800 disabled:opacity-50";

#[derive(Properties, PartialEq)]
pub struct Props {
    pub app: AppEntry,
    pub locale: Locale,
    pub onchanged: Callback<AppEntry>,
}

#[function_component(AppImageUpdate)]
pub fn app_image_update(props: &Props) -> Html {
    let locale = props.locale;
    let AppSource::Image { ref image } = props.app.spec.source else {
        return html! {};
    };

    let checking = use_state(|| false);
    let deploying = use_state(|| false);
    let check_onclick = check_callback(locale, &props.app, &checking, &props.onchanged);
    let deploy_onclick = deploy_callback(locale, &props.app, &deploying, &props.onchanged);

    html! {
        <div class="mt-4 bg-white dark:bg-neutral-800 shadow-sm p-5 border border-neutral-300 dark:border-neutral-700 rounded-md">
            <div class="flex items-center justify-between">
                <div>
                    <h2 class="text-lg font-semibold text-neutral-900 dark:text-neutral-200">
                        {locale.t("apps.image_updates")}
                    </h2>
                    <p class={HINT_CLASS}>{image.as_str()}</p>
                </div>
                <button
                    type="button"
                    onclick={check_onclick}
                    disabled={*checking}
                    class={BUTTON_CLASS}
                >
                    if *checking {
                        { locale.t("apps.checking_update") }
                    } else {
                        { locale.t("apps.check_update") }
                    }
                </button>
            </div>
            { status_view(locale, props.app.update.as_ref(), deploy_onclick, *deploying) }
        </div>
    }
}

fn status_view(
    locale: Locale,
    update: Option<&ImageUpdateStatus>,
    deploy_onclick: Callback<MouseEvent>,
    deploying: bool,
) -> Html {
    let Some(status) = update else {
        return html! {
            <p class="text-xs text-neutral-500 mt-3">{locale.t("apps.auto_deploy_hint")}</p>
        };
    };

    let last_checked = format!(
        "{} {}",
        locale.t("apps.last_checked"),
        format_time(status.checked_at)
    );
    if status.update_available {
        banner_view(locale, status, deploy_onclick, deploying, &last_checked)
    } else {
        html! {
            <div class="mt-3">
                <p class="text-sm text-neutral-600 dark:text-neutral-400">
                    {locale.t("apps.update_not_found")}
                </p>
                <p class="text-xs text-neutral-500 mt-1">{last_checked}</p>
            </div>
        }
    }
}

fn banner_view(
    locale: Locale,
    status: &ImageUpdateStatus,
    deploy_onclick: Callback<MouseEvent>,
    deploying: bool,
    last_checked: &str,
) -> Html {
    let current_tag = status.current_tag.as_deref().unwrap_or("-");
    let latest_tag = status.latest_tag.as_deref().unwrap_or(current_tag);
    html! {
        <div class="mt-3 p-4 border border-blue-200 dark:border-blue-800 bg-blue-50 dark:bg-neutral-900 rounded-md">
            <div class="flex flex-col sm:flex-row sm:items-center sm:justify-between gap-3">
                <div>
                    <p class="font-medium text-blue-900 dark:text-blue-200 text-sm">
                        {locale.t("apps.update_found_banner")}
                    </p>
                    <p class="text-xs text-neutral-600 dark:text-neutral-400 mt-1">
                        <span class="font-semibold">{locale.t("apps.current_version")}{" "}</span>
                        {current_tag}
                        if let Some(digest) = &status.current_digest {
                            <span class="ml-2 font-mono text-[10px] text-neutral-400">{short_digest(digest)}</span>
                        }
                    </p>
                    <p class="text-xs text-neutral-600 dark:text-neutral-400 mt-0.5">
                        <span class="font-semibold">{locale.t("apps.latest_version")}{" "}</span>
                        <span class="text-green-600 dark:text-green-400 font-medium">{latest_tag}</span>
                        if let Some(digest) = &status.latest_digest {
                            <span class="ml-2 font-mono text-[10px] text-neutral-400">{short_digest(digest)}</span>
                        }
                    </p>
                    <p class="text-xs text-neutral-500 mt-1">{last_checked}</p>
                </div>
                <button
                    type="button"
                    onclick={deploy_onclick}
                    disabled={deploying}
                    class={PRIMARY_BUTTON_CLASS}
                >
                    {locale.t("apps.deploy_update")}
                </button>
            </div>
        </div>
    }
}

pub fn short_digest(digest: &str) -> &str {
    if digest.len() > 19 {
        &digest[..19]
    } else {
        digest
    }
}

fn check_callback(
    locale: Locale,
    app: &AppEntry,
    checking: &UseStateHandle<bool>,
    onchanged: &Callback<AppEntry>,
) -> Callback<MouseEvent> {
    let (id, checking, onchanged) = (app.id, checking.clone(), onchanged.clone());
    Callback::from(move |e: MouseEvent| {
        e.prevent_default();
        let (checking, onchanged) = (checking.clone(), onchanged.clone());
        checking.set(true);
        spawn_local(async move {
            let path = format!("{API_ENDPOINT}/apps/{id}/check_update");
            let req = Request::post(&path);
            let result: Result<ImageUpdateStatus, String> = fetch_json(locale, req).await;
            match result {
                Ok(_) => {
                    let get_path = format!("{API_ENDPOINT}/apps/{id}");
                    if let Ok(updated) =
                        fetch_json::<AppEntry>(locale, Request::get(&get_path)).await
                    {
                        onchanged.emit(updated);
                    }
                }
                Err(err) => {
                    dialog::message(locale, &err, Icon::Error).await;
                }
            }
            checking.set(false);
        });
    })
}

fn deploy_callback(
    locale: Locale,
    app: &AppEntry,
    deploying: &UseStateHandle<bool>,
    onchanged: &Callback<AppEntry>,
) -> Callback<MouseEvent> {
    let (id, name, deploying, onchanged) = (
        app.id,
        app.name.clone(),
        deploying.clone(),
        onchanged.clone(),
    );
    Callback::from(move |e: MouseEvent| {
        e.prevent_default();
        let question = locale.tf("apps.confirm_deploy", &[("name", name.as_str())]);
        let (deploying, onchanged) = (deploying.clone(), onchanged.clone());
        dialog::confirm_then(locale, question, async move {
            deploying.set(true);
            let path = format!("{API_ENDPOINT}/apps/{id}/deploy");
            let req = Request::post(&path);
            let result: Result<DeploymentEntry, String> = fetch_json(locale, req).await;
            match result {
                Ok(_) => {
                    let get_path = format!("{API_ENDPOINT}/apps/{id}");
                    if let Ok(updated) =
                        fetch_json::<AppEntry>(locale, Request::get(&get_path)).await
                    {
                        onchanged.emit(updated);
                    }
                }
                Err(err) => {
                    dialog::message(locale, &err, Icon::Error).await;
                }
            }
            deploying.set(false);
        });
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn short_digest_truncates_long_hashes() {
        assert_eq!(
            short_digest("sha256:1234567890abcdef"),
            "sha256:1234567890ab"
        );
        assert_eq!(short_digest("short"), "short");
    }
}
