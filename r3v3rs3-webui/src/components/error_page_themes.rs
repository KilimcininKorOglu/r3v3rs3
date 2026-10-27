// Copyright 2026 r3v3rs3 Authors
// SPDX-License-Identifier: MIT

use crate::API_ENDPOINT;
use crate::components::action_button::{ActionIconButton, ActionVariant};
use crate::dialog;
use crate::i18n::use_locale;
use crate::pages::settings::{Notice, fetch_json, notice_view, send_request};
use gloo_net::http::Request;
use r3v3rs3_api::error_page_theme::ErrorPageTheme;
use r3v3rs3_api::i18n::Locale;
use r3v3rs3_api::id::ShortId;
use yew::prelude::*;

fn error_page_theme_status_badges(pages: &[r3v3rs3_api::proxy::CustomErrorPage]) -> Html {
    let badges = pages.iter().take(6).map(|p| p.status).collect::<Vec<_>>();
    let extra = if pages.len() > 6 {
        Some(pages.len() - 6)
    } else {
        None
    };
    html! {
        <div class="flex items-center gap-1 flex-wrap">
            { for badges.into_iter().map(|code| html! {
                <span class="px-1.5 py-0.5 rounded text-[10px] font-mono bg-neutral-100 dark:bg-neutral-700 text-neutral-600 dark:text-neutral-300">
                    {code.to_string()}
                </span>
            }) }
            if let Some(count) = extra {
                <span class="text-[10px] text-neutral-400">
                    {format!("+{count}")}
                </span>
            }
        </div>
    }
}

fn error_page_theme_row(
    locale: Locale,
    theme: &ErrorPageTheme,
    on_delete: &Callback<(ShortId, String)>,
) -> Html {
    let theme_id = theme.id;
    let theme_name = theme.name.clone();
    let on_del = {
        let on_delete = on_delete.clone();
        Callback::from(move |_| on_delete.emit((theme_id, theme_name.clone())))
    };
    html! {
        <tr class="border-b dark:border-neutral-700 hover:bg-neutral-50 dark:hover:bg-neutral-700/50">
            <td class="px-4 py-2 font-medium text-neutral-900 dark:text-white">
                <div class="flex items-center gap-2">
                    <span class="font-semibold">{&theme.name}</span>
                    <span class="text-[10px] font-mono px-1.5 py-0.5 rounded bg-neutral-100 dark:bg-neutral-700 text-neutral-500">
                        {theme.id.to_string()}
                    </span>
                </div>
            </td>
            <td class="px-4 py-2 text-neutral-500 dark:text-neutral-400">
                {&theme.description}
            </td>
            <td class="px-4 py-2">
                <div class="flex items-center gap-2 flex-wrap">
                    <span class="px-2 py-0.5 rounded text-[11px] font-medium bg-blue-100 text-blue-800 dark:bg-blue-900/60 dark:text-blue-300">
                        {format!("{} {}", theme.pages.len(), locale.t("settings.error_page_theme_pages_suffix"))}
                    </span>
                    { error_page_theme_status_badges(&theme.pages) }
                </div>
            </td>
            <td class="px-4 py-2 text-right">
                <ActionIconButton
                    variant={ActionVariant::Delete}
                    title={locale.t("common.delete").to_string()}
                    onclick={on_del}
                />
            </td>
        </tr>
    }
}

fn error_page_themes_table(
    locale: Locale,
    list: &[ErrorPageTheme],
    on_delete: &Callback<(ShortId, String)>,
) -> Html {
    if list.is_empty() {
        return html! {
            <div class="text-xs text-neutral-400 py-3 text-center">
                {locale.t("settings.no_error_page_themes")}
            </div>
        };
    }
    html! {
        <div class="overflow-x-auto">
            <table class="w-full text-xs text-left text-neutral-600 dark:text-neutral-300">
                <thead class="text-xs uppercase bg-neutral-100 dark:bg-neutral-700/50 text-neutral-500 dark:text-neutral-400">
                    <tr>
                        <th class="px-4 py-2">{locale.t("settings.error_page_theme_name")}</th>
                        <th class="px-4 py-2">{locale.t("settings.error_page_theme_desc")}</th>
                        <th class="px-4 py-2">{locale.t("settings.error_page_theme_pages")}</th>
                        <th class="px-4 py-2 text-right">{locale.t("common.actions")}</th>
                    </tr>
                </thead>
                <tbody>
                    { for list.iter().map(|theme| error_page_theme_row(locale, theme, on_delete)) }
                </tbody>
            </table>
        </div>
    }
}

#[function_component(ErrorPageThemesCard)]
pub fn error_page_themes_card() -> Html {
    let locale = use_locale();
    let reload = use_state(|| 0);
    let list = use_state(Vec::<ErrorPageTheme>::new);
    let notice = use_state(|| Option::<Notice>::None);

    let reload_val = *reload;
    let list_handle = list.clone();
    use_effect_with(reload_val, move |_| {
        wasm_bindgen_futures::spawn_local(async move {
            if let Ok(entries) = fetch_json::<Vec<ErrorPageTheme>>(
                locale,
                Request::get(&format!("{API_ENDPOINT}/error-page-themes")),
            )
            .await
            {
                list_handle.set(entries);
            }
        });
    });

    let on_reset_defaults = {
        let reload = reload.clone();
        let notice = notice.clone();
        Callback::from(move |_| {
            let reload = reload.clone();
            let notice = notice.clone();
            let question = locale
                .t("settings.error_page_theme_reset_confirm")
                .to_string();
            dialog::confirm_then(locale, question, async move {
                let req =
                    Request::post(&format!("{API_ENDPOINT}/error-page-themes/reset-defaults"));
                if let Ok(built) = req.build() {
                    match send_request(locale, built).await {
                        Ok(_) => {
                            notice.set(Some(Notice::Success(
                                locale
                                    .t("settings.error_page_theme_reset_success")
                                    .to_string(),
                            )));
                            reload.set(*reload + 1);
                        }
                        Err(err) => notice.set(Some(Notice::Failed(err))),
                    }
                }
            });
        })
    };

    let on_delete = {
        let reload = reload.clone();
        let notice = notice.clone();
        Callback::from(move |(id, name): (ShortId, String)| {
            let reload = reload.clone();
            let notice = notice.clone();
            let question = locale.tf(
                "settings.error_page_theme_delete_confirm",
                &[("name", &name)],
            );
            dialog::confirm_then(locale, question, async move {
                let req = Request::delete(&format!("{API_ENDPOINT}/error-page-themes/{id}"));
                if let Ok(built) = req.build() {
                    match send_request(locale, built).await {
                        Ok(_) => {
                            notice.set(Some(Notice::Success(
                                locale.t("settings.error_page_theme_deleted").to_string(),
                            )));
                            reload.set(*reload + 1);
                        }
                        Err(err) => notice.set(Some(Notice::Failed(err))),
                    }
                }
            });
        })
    };

    html! {
        <div class="bg-white dark:bg-neutral-800 shadow-sm p-5 border border-neutral-300 dark:border-neutral-700 rounded-md">
            { notice_view(&notice) }
            <div class="flex items-center justify-between mb-4 flex-wrap gap-2">
                <div>
                    <h2 class="text-lg font-semibold text-neutral-900 dark:text-neutral-200">
                        {locale.t("settings.error_page_themes")}
                    </h2>
                    <p class="text-xs text-neutral-500 dark:text-neutral-400 mt-0.5">
                        {locale.t("settings.error_page_themes_desc")}
                    </p>
                </div>
                <button
                    type="button"
                    onclick={on_reset_defaults}
                    class="px-3 py-1.5 text-xs font-medium rounded-lg border border-neutral-300 dark:border-neutral-600 bg-neutral-50 dark:bg-neutral-700 text-neutral-700 dark:text-neutral-200 hover:bg-neutral-100 dark:hover:bg-neutral-600 transition-colors"
                >
                    {locale.t("settings.error_page_theme_reset")}
                </button>
            </div>
            { error_page_themes_table(locale, &list, &on_delete) }
        </div>
    }
}
