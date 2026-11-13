// Copyright 2026 r3v3rs3 Authors
// SPDX-License-Identifier: MIT

use crate::API_ENDPOINT;
use crate::components::button::{Button, ButtonKind, ButtonSize};
use crate::components::data_list::{Column, ListContent, ListOptions, Row, list_view};
use crate::components::error_pages_editor::ErrorPagesEditor;
use crate::components::http_proxy_config::{ErrorPageForm, parse_error_pages};
use crate::components::modal::{Modal, ModalSize};
use crate::dialog;
use crate::i18n::use_locale;
use crate::pages::resource_page::{AddIcon, row_actions};
use crate::pages::settings::{Notice, fetch_json, notice_view, post_json, send_json, send_request};
use gloo_net::http::Request;
use r3v3rs3_api::error_page_theme::ErrorPageTheme;
use r3v3rs3_api::i18n::Locale;
use r3v3rs3_api::id::ShortId;
use std::collections::HashMap;
use std::str::FromStr;
use web_sys::HtmlInputElement;
use yew::prelude::*;

const MODAL_INPUT_CLASS: &str = "w-full px-3 py-2 text-xs rounded-lg border border-neutral-300 dark:border-neutral-600 bg-white dark:bg-neutral-800 text-neutral-900 dark:text-neutral-100 focus:outline-none focus:ring-2 focus:ring-primary-500";

#[derive(Clone, PartialEq, Default)]
struct ThemeModalState {
    open: bool,
    is_new: bool,
    id: String,
    name: String,
    description: String,
    pages: Vec<ErrorPageForm>,
    saving: bool,
    errors: HashMap<String, String>,
}

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

const COLUMNS: [Column; 3] = [
    Column {
        label: "settings.error_page_theme_name",
        class: "",
    },
    Column {
        label: "settings.error_page_theme_desc",
        class: "",
    },
    Column {
        label: "settings.error_page_theme_pages",
        class: "",
    },
];

fn error_page_theme_row(
    locale: Locale,
    theme: &ErrorPageTheme,
    on_edit: &Callback<ErrorPageTheme>,
    on_delete: &Callback<(ShortId, String)>,
) -> Row {
    let theme_id = theme.id;
    let theme_name = theme.name.clone();
    let on_del = {
        let on_delete = on_delete.clone();
        Callback::from(move |_| on_delete.emit((theme_id, theme_name.clone())))
    };
    let on_edit_click = {
        let on_edit = on_edit.clone();
        let theme = theme.clone();
        Callback::from(move |_| on_edit.emit(theme.clone()))
    };
    Row {
        key: theme.id.to_string(),
        cells: vec![
            html! {
                <div class="flex items-center gap-2">
                    <span>{&theme.name}</span>
                    <span class="text-[10px] font-mono px-1.5 py-0.5 rounded bg-neutral-100 dark:bg-neutral-700 text-neutral-500 dark:text-neutral-300">
                        {theme.id.to_string()}
                    </span>
                </div>
            },
            html! { <span class="text-neutral-500 dark:text-neutral-400">{&theme.description}</span> },
            html! {
                <div class="flex items-center gap-2 flex-wrap">
                    <span class="px-2 py-0.5 rounded text-[11px] font-medium bg-primary-100 text-primary-800 dark:bg-primary-900/60 dark:text-primary-300">
                        {format!("{} {}", theme.pages.len(), locale.t("settings.error_page_theme_pages_suffix"))}
                    </span>
                    { error_page_theme_status_badges(&theme.pages) }
                </div>
            },
        ],
        actions: row_actions(locale, on_edit_click, Some(on_del)),
    }
}

fn error_page_themes_table(
    locale: Locale,
    list: &[ErrorPageTheme],
    on_edit: &Callback<ErrorPageTheme>,
    on_delete: &Callback<(ShortId, String)>,
) -> Html {
    let rows = list
        .iter()
        .map(|theme| error_page_theme_row(locale, theme, on_edit, on_delete))
        .collect::<Vec<_>>();
    let content = ListContent {
        loaded: true,
        empty: "settings.no_error_page_themes",
        columns: &COLUMNS,
        rows: &rows,
    };
    let options = ListOptions {
        embedded: true,
        ..Default::default()
    };
    list_view(locale, content, None, &options)
}

fn render_theme_modal_id_field(locale: Locale, modal: &UseStateHandle<ThemeModalState>) -> Html {
    if !modal.is_new {
        return html! {
            <div>
                <label class="block text-xs font-medium text-neutral-500 dark:text-neutral-400 mb-1">
                    {locale.t("settings.error_page_theme_id")}
                </label>
                <span class="inline-block px-2.5 py-1 text-xs font-mono font-semibold rounded bg-neutral-100 dark:bg-neutral-800 text-neutral-700 dark:text-neutral-300">
                    {&modal.id}
                </span>
            </div>
        };
    }
    let oninput = {
        let modal = modal.clone();
        Callback::from(move |e: InputEvent| {
            let val = e.target_unchecked_into::<HtmlInputElement>().value();
            let mut u = (*modal).clone();
            u.id = val;
            modal.set(u);
        })
    };
    html! {
        <div>
            <label class="block text-xs font-medium text-neutral-700 dark:text-neutral-300 mb-1">
                {locale.t("settings.error_page_theme_id")}
            </label>
            <input
                type="text"
                class={MODAL_INPUT_CLASS}
                placeholder="e.g. brand-theme"
                value={modal.id.clone()}
                {oninput}
            />
            <p class="text-[11px] text-neutral-400 mt-1">
                {locale.t("settings.error_page_theme_id_hint")}
            </p>
        </div>
    }
}

fn render_theme_modal_fields(locale: Locale, modal: &UseStateHandle<ThemeModalState>) -> Html {
    let on_name_input = {
        let modal = modal.clone();
        Callback::from(move |e: InputEvent| {
            let val = e.target_unchecked_into::<HtmlInputElement>().value();
            let mut u = (*modal).clone();
            u.name = val;
            u.errors.remove("name");
            modal.set(u);
        })
    };
    let on_desc_input = {
        let modal = modal.clone();
        Callback::from(move |e: InputEvent| {
            let val = e.target_unchecked_into::<HtmlInputElement>().value();
            let mut u = (*modal).clone();
            u.description = val;
            modal.set(u);
        })
    };
    html! {
        <div class="space-y-4">
            <div class="grid grid-cols-1 sm:grid-cols-2 gap-4">
                { render_theme_modal_id_field(locale, modal) }
                <div>
                    <label class="block text-xs font-medium text-neutral-700 dark:text-neutral-300 mb-1">
                        {locale.t("settings.error_page_theme_name")}
                    </label>
                    <input
                        type="text"
                        class={MODAL_INPUT_CLASS}
                        placeholder={locale.t("settings.error_page_theme_name_placeholder").to_string()}
                        value={modal.name.clone()}
                        oninput={on_name_input}
                    />
                    if let Some(err) = modal.errors.get("name") {
                        <p class="text-[11px] text-red-500 mt-1">{err}</p>
                    }
                </div>
            </div>
            <div>
                <label class="block text-xs font-medium text-neutral-700 dark:text-neutral-300 mb-1">
                    {locale.t("settings.error_page_theme_desc")}
                </label>
                <input
                    type="text"
                    class={MODAL_INPUT_CLASS}
                    placeholder={locale.t("settings.error_page_theme_desc_placeholder").to_string()}
                    value={modal.description.clone()}
                    oninput={on_desc_input}
                />
            </div>
        </div>
    }
}

fn render_theme_modal_footer(
    locale: Locale,
    saving: bool,
    on_close: &Callback<()>,
    on_save: &Callback<()>,
) -> Html {
    let on_cancel_click = {
        let on_close = on_close.clone();
        Callback::from(move |e: MouseEvent| {
            e.prevent_default();
            on_close.emit(());
        })
    };
    let on_save_click = {
        let on_save = on_save.clone();
        Callback::from(move |e: MouseEvent| {
            e.prevent_default();
            on_save.emit(());
        })
    };
    html! {
        <>
            <Button kind={ButtonKind::Secondary} onclick={on_cancel_click}>
                {locale.t("common.cancel")}
            </Button>
            <Button kind={ButtonKind::Primary} disabled={saving} onclick={on_save_click}>
                if saving {
                    {locale.t("common.loading")}
                } else {
                    {locale.t("settings.save")}
                }
            </Button>
        </>
    }
}

fn render_theme_modal(
    locale: Locale,
    modal: &UseStateHandle<ThemeModalState>,
    on_close: &Callback<()>,
    on_save: &Callback<()>,
) -> Html {
    if !modal.open {
        return html! {};
    }
    let on_pages_change = {
        let modal = modal.clone();
        Callback::from(move |pages: Vec<ErrorPageForm>| {
            let mut u = (*modal).clone();
            u.pages = pages;
            modal.set(u);
        })
    };

    let title = if modal.is_new {
        locale.t("settings.error_page_theme_new")
    } else {
        locale.t("settings.error_page_theme_edit")
    };
    let footer = render_theme_modal_footer(locale, modal.saving, on_close, on_save);
    html! {
        <Modal {title} on_close={on_close.clone()} size={ModalSize::Xl} {footer}>
            <div class="flex flex-col gap-4">
                if let Some(err) = modal.errors.get("submit") {
                    <div class="p-3 text-xs rounded-lg bg-red-100 dark:bg-red-900/40 text-red-700 dark:text-red-300">
                        {err}
                    </div>
                }
                { render_theme_modal_fields(locale, modal) }
                <ErrorPagesEditor
                    error_pages={modal.pages.clone()}
                    on_change={on_pages_change}
                    errors={modal.errors.clone()}
                />
            </div>
        </Modal>
    }
}

fn handle_reset_defaults(
    locale: Locale,
    reload: &UseStateHandle<usize>,
    notice: &UseStateHandle<Option<Notice>>,
) {
    let reload = reload.clone();
    let notice = notice.clone();
    let question = locale
        .t("settings.error_page_theme_reset_confirm")
        .to_string();
    dialog::confirm_then(locale, question, async move {
        let req = Request::post(&format!("{API_ENDPOINT}/error-page-themes/reset-defaults"));
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
}

fn handle_delete_theme(
    locale: Locale,
    reload: &UseStateHandle<usize>,
    notice: &UseStateHandle<Option<Notice>>,
    id: ShortId,
    name: String,
) {
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
}

fn resolve_theme_id(modal: &ThemeModalState) -> Result<ShortId, String> {
    if modal.is_new {
        let trimmed = modal.id.trim();
        if trimmed.is_empty() {
            Ok(ShortId::new())
        } else {
            ShortId::from_str(trimmed).map_err(|e| e.to_string())
        }
    } else {
        ShortId::from_str(modal.id.trim()).map_err(|e| e.to_string())
    }
}

async fn execute_save_theme(
    locale: Locale,
    theme: ErrorPageTheme,
    is_new: bool,
) -> Result<(), String> {
    if is_new {
        post_json::<ShortId>(
            locale,
            Request::post(&format!("{API_ENDPOINT}/error-page-themes")),
            &theme,
        )
        .await
        .map(|_| ())
    } else {
        send_json(
            locale,
            Request::put(&format!("{API_ENDPOINT}/error-page-themes/{}", theme.id)),
            &theme,
        )
        .await
    }
}

fn submit_theme_save(
    locale: Locale,
    modal: &UseStateHandle<ThemeModalState>,
    reload: &UseStateHandle<usize>,
    notice: &UseStateHandle<Option<Notice>>,
) {
    let trimmed_name = modal.name.trim().to_string();
    if trimmed_name.is_empty() {
        let mut u = (**modal).clone();
        u.errors.insert(
            "name".to_string(),
            locale
                .t("settings.error_page_theme_name_required")
                .to_string(),
        );
        modal.set(u);
        return;
    }
    let theme_id = match resolve_theme_id(modal) {
        Ok(id) => id,
        Err(_) => {
            let mut u = (**modal).clone();
            u.errors.insert(
                "id".to_string(),
                locale.t("settings.error_page_theme_id_invalid").to_string(),
            );
            modal.set(u);
            return;
        }
    };
    let mut parse_errors = HashMap::new();
    let pages = parse_error_pages(locale, &modal.pages, &mut parse_errors);
    if !parse_errors.is_empty() {
        let mut u = (**modal).clone();
        u.errors = parse_errors;
        modal.set(u);
        return;
    }
    let theme = ErrorPageTheme {
        id: theme_id,
        name: trimmed_name,
        description: modal.description.trim().to_string(),
        pages,
    };
    let is_new = modal.is_new;
    let mut u = (**modal).clone();
    u.saving = true;
    u.errors.clear();
    modal.set(u);

    let modal = modal.clone();
    let reload = reload.clone();
    let notice = notice.clone();
    wasm_bindgen_futures::spawn_local(async move {
        match execute_save_theme(locale, theme, is_new).await {
            Ok(_) => {
                notice.set(Some(Notice::Success(
                    locale.t("settings.error_page_theme_saved").to_string(),
                )));
                modal.set(ThemeModalState::default());
                reload.set(*reload + 1);
            }
            Err(err) => {
                let mut u = (*modal).clone();
                u.saving = false;
                u.errors.insert("submit".to_string(), err);
                modal.set(u);
            }
        }
    });
}

#[function_component(ErrorPageThemesCard)]
pub fn error_page_themes_card() -> Html {
    let locale = use_locale();
    let reload = use_state(|| 0);
    let list = use_state(Vec::<ErrorPageTheme>::new);
    let notice = use_state(|| Option::<Notice>::None);
    let modal = use_state(ThemeModalState::default);

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
        Callback::from(move |_| handle_reset_defaults(locale, &reload, &notice))
    };

    let on_delete = {
        let reload = reload.clone();
        let notice = notice.clone();
        Callback::from(move |(id, name): (ShortId, String)| {
            handle_delete_theme(locale, &reload, &notice, id, name);
        })
    };

    let on_edit = {
        let modal = modal.clone();
        Callback::from(move |theme: ErrorPageTheme| {
            let pages = theme
                .pages
                .into_iter()
                .map(|p| ErrorPageForm {
                    status: p.status.to_string(),
                    body: p.body,
                })
                .collect();
            modal.set(ThemeModalState {
                open: true,
                is_new: false,
                id: theme.id.to_string(),
                name: theme.name,
                description: theme.description,
                pages,
                saving: false,
                errors: HashMap::new(),
            });
        })
    };

    let on_open_create = {
        let modal = modal.clone();
        Callback::from(move |_| {
            modal.set(ThemeModalState {
                open: true,
                is_new: true,
                id: String::new(),
                name: String::new(),
                description: String::new(),
                pages: Vec::new(),
                saving: false,
                errors: HashMap::new(),
            });
        })
    };

    let on_close_modal = {
        let modal = modal.clone();
        Callback::from(move |_| modal.set(ThemeModalState::default()))
    };

    let on_save_modal = {
        let modal = modal.clone();
        let reload = reload.clone();
        let notice = notice.clone();
        Callback::from(move |_| submit_theme_save(locale, &modal, &reload, &notice))
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
                <div class="flex items-center gap-2">
                    <Button kind={ButtonKind::Secondary} size={ButtonSize::Sm} onclick={on_reset_defaults}>
                        {locale.t("settings.error_page_theme_reset")}
                    </Button>
                    <Button kind={ButtonKind::Primary} size={ButtonSize::Sm} onclick={on_open_create}>
                        <AddIcon />
                        {locale.t("settings.error_page_theme_add")}
                    </Button>
                </div>
            </div>
            { error_page_themes_table(locale, &list, &on_edit, &on_delete) }
            { render_theme_modal(locale, &modal, &on_close_modal, &on_save_modal) }
        </div>
    }
}
