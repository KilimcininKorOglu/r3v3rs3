// Copyright 2026 r3v3rs3 Authors
// SPDX-License-Identifier: MIT

use crate::components::http_proxy_config::ErrorPageForm;
use crate::i18n::use_locale;
use r3v3rs3_api::error_page_templates::{TEMPLATES, render_template, status_title_and_description};
use r3v3rs3_api::i18n::Locale;
use std::collections::HashMap;
use web_sys::{HtmlInputElement, HtmlTextAreaElement};
use yew::prelude::*;

const MAX_BODY_BYTES: usize = 65536;

/// View mode for the error page designer card.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum ViewMode {
    Code,
    Preview,
    Split,
}

/// Responsive preview viewport width.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum PreviewViewport {
    Desktop,
    Tablet,
    Mobile,
}

impl PreviewViewport {
    pub fn container_class(self) -> &'static str {
        match self {
            Self::Desktop => "w-full",
            Self::Tablet => "max-w-[768px] mx-auto",
            Self::Mobile => "max-w-[375px] mx-auto",
        }
    }
}

pub fn parse_status_code(status_str: &str) -> u16 {
    status_str.trim().parse::<u16>().unwrap_or(404)
}

pub fn format_byte_size(bytes: usize) -> String {
    if bytes < 1024 {
        format!("{bytes} B")
    } else {
        format!("{:.1} KB", bytes as f64 / 1024.0)
    }
}

#[derive(Properties, PartialEq, Clone)]
pub struct ErrorPagesEditorProps {
    pub error_pages: Vec<ErrorPageForm>,
    pub on_change: Callback<Vec<ErrorPageForm>>,
    pub errors: HashMap<String, String>,
}

#[function_component(ErrorPagesEditor)]
pub fn error_pages_editor(props: &ErrorPagesEditorProps) -> Html {
    let locale = use_locale();
    let pages = &props.error_pages;

    let on_add_page = {
        let on_change = props.on_change.clone();
        let pages = pages.clone();
        Callback::from(move |initial_status: Option<u16>| {
            let mut updated = pages.clone();
            let status_code = initial_status.unwrap_or(404);
            let template_body = render_template("minimal_dark", status_code);
            updated.push(ErrorPageForm {
                status: status_code.to_string(),
                body: template_body,
            });
            on_change.emit(updated);
        })
    };

    html! {
        <div class="mt-6 p-4 rounded-lg bg-neutral-100 dark:bg-neutral-800/60 border border-neutral-200 dark:border-neutral-700">
            <div class="flex items-center justify-between mb-4 flex-wrap gap-2">
                <div>
                    <label class="block text-sm font-semibold text-neutral-900 dark:text-neutral-100">
                        {locale.t("http_form.error_pages")}
                    </label>
                    <p class="text-xs text-neutral-500 dark:text-neutral-400 mt-0.5">
                        {locale.t("http_form.error_pages_hint")}
                    </p>
                </div>
                <button
                    type="button"
                    onclick={
                        let on_add = on_add_page.clone();
                        Callback::from(move |_| on_add.emit(None))
                    }
                    class="px-3 py-1.5 text-xs font-medium rounded-lg bg-primary-600 hover:bg-primary-700 text-white transition-colors"
                >
                    {locale.t("http_form.add_error_page")}
                </button>
            </div>

            if pages.is_empty() {
                { empty_state_view(locale, on_add_page) }
            } else {
                <div class="space-y-6">
                    { for pages.iter().enumerate().map(|(index, page)| {
                        let on_remove = {
                            let on_change = props.on_change.clone();
                            let pages = pages.clone();
                            Callback::from(move |_| {
                                let mut updated = pages.clone();
                                if index < updated.len() {
                                    updated.remove(index);
                                    on_change.emit(updated);
                                }
                            })
                        };
                        let on_status_change = {
                            let on_change = props.on_change.clone();
                            let pages = pages.clone();
                            Callback::from(move |new_status: String| {
                                let mut updated = pages.clone();
                                if let Some(item) = updated.get_mut(index) {
                                    item.status = new_status;
                                    on_change.emit(updated);
                                }
                            })
                        };
                        let on_body_change = {
                            let on_change = props.on_change.clone();
                            let pages = pages.clone();
                            Callback::from(move |new_body: String| {
                                let mut updated = pages.clone();
                                if let Some(item) = updated.get_mut(index) {
                                    item.body = new_body;
                                    on_change.emit(updated);
                                }
                            })
                        };
                        let status_err = props.errors.get(&format!("error_pages_{index}_status")).cloned();
                        let body_err = props.errors.get(&format!("error_pages_{index}_body")).cloned();

                        html! {
                            <SingleErrorPageCard
                                key={index}
                                page={page.clone()}
                                index={index}
                                status_error={status_err}
                                body_error={body_err}
                                on_status_change={on_status_change}
                                on_body_change={on_body_change}
                                on_remove={on_remove}
                            />
                        }
                    }) }
                </div>
            }
        </div>
    }
}

struct EditorLayoutContext<'a> {
    pub locale: Locale,
    pub view_mode: ViewMode,
    pub viewport: PreviewViewport,
    pub body: &'a str,
    pub body_error: Option<&'a String>,
    pub on_body_change: Callback<String>,
    pub body_len: usize,
    pub is_oversized: bool,
}

#[derive(Properties, PartialEq, Clone)]
pub struct SingleErrorPageCardProps {
    pub page: ErrorPageForm,
    pub index: usize,
    pub status_error: Option<String>,
    pub body_error: Option<String>,
    pub on_status_change: Callback<String>,
    pub on_body_change: Callback<String>,
    pub on_remove: Callback<()>,
}

#[function_component(SingleErrorPageCard)]
pub fn single_error_page_card(props: &SingleErrorPageCardProps) -> Html {
    let locale = use_locale();
    let view_mode = use_state(|| ViewMode::Split);
    let viewport = use_state(|| PreviewViewport::Desktop);
    let parsed_status = parse_status_code(&props.page.status);
    let (status_title, _) = status_title_and_description(parsed_status);

    let on_select_preset = {
        let on_status_change = props.on_status_change.clone();
        Callback::from(move |preset: u16| {
            on_status_change.emit(preset.to_string());
        })
    };

    let on_apply_template = {
        let on_body_change = props.on_body_change.clone();
        let status = parsed_status;
        Callback::from(move |tpl_id: &'static str| {
            let rendered = render_template(tpl_id, status);
            on_body_change.emit(rendered);
        })
    };

    let body_len = props.page.body.len();
    let is_oversized = body_len > MAX_BODY_BYTES;

    let ctx = EditorLayoutContext {
        locale,
        view_mode: *view_mode,
        viewport: *viewport,
        body: &props.page.body,
        body_error: props.body_error.as_ref(),
        on_body_change: props.on_body_change.clone(),
        body_len,
        is_oversized,
    };

    html! {
        <div class="p-4 border rounded-xl border-neutral-300 dark:border-neutral-700 bg-white dark:bg-neutral-900 shadow-sm space-y-4">
            <div class="flex items-start justify-between flex-wrap gap-3 pb-3 border-b border-neutral-200 dark:border-neutral-800">
                <div class="flex items-center gap-3 flex-wrap">
                    <div class="w-28">
                        <label class="block text-xs font-semibold text-neutral-600 dark:text-neutral-400 mb-1">
                            {locale.t("http_form.error_page_status")}
                        </label>
                        <input
                            type="number"
                            min="400"
                            max="599"
                            value={props.page.status.clone()}
                            oninput={
                                let on_change = props.on_status_change.clone();
                                Callback::from(move |e: InputEvent| {
                                    if let Some(target) = e.target_dyn_into::<HtmlInputElement>() {
                                        on_change.emit(target.value());
                                    }
                                })
                            }
                            class="w-full px-2.5 py-1.5 text-sm font-semibold rounded-lg border border-neutral-300 dark:border-neutral-600 bg-neutral-50 dark:bg-neutral-800 text-neutral-900 dark:text-neutral-100 focus:outline-none focus:ring-2 focus:ring-primary-500"
                            placeholder="404"
                        />
                    </div>
                    <div class="mt-4">
                        <span class="inline-flex items-center px-2.5 py-1 rounded-md text-xs font-bold bg-neutral-100 dark:bg-neutral-800 text-neutral-800 dark:text-neutral-200 border border-neutral-200 dark:border-neutral-700">
                            {status_title}
                        </span>
                    </div>
                    <div class="mt-4 flex items-center gap-1">
                        { status_presets_view(on_select_preset) }
                    </div>
                </div>

                <div class="flex items-center gap-2">
                    <button
                        type="button"
                        onclick={
                            let on_remove = props.on_remove.clone();
                            Callback::from(move |_| on_remove.emit(()))
                        }
                        class="text-xs font-medium text-red-600 hover:text-red-700 dark:text-red-400 hover:underline px-2 py-1"
                    >
                        {locale.t("http_form.remove_error_page")}
                    </button>
                </div>
            </div>

            if let Some(err) = &props.status_error {
                <div class="text-xs text-red-600 dark:text-red-400 font-medium">{err}</div>
            }

            <div class="flex items-center justify-between flex-wrap gap-2 pt-1">
                { templates_selector_view(locale, on_apply_template) }

                <div class="flex items-center gap-2 flex-wrap">
                    { view_mode_buttons_view(locale, *view_mode, {
                        let view_mode = view_mode.clone();
                        Callback::from(move |mode: ViewMode| view_mode.set(mode))
                    }) }

                    if *view_mode != ViewMode::Code {
                        { viewport_buttons_view(locale, *viewport, {
                            let viewport = viewport.clone();
                            Callback::from(move |vp: PreviewViewport| viewport.set(vp))
                        }) }
                    }
                </div>
            </div>

            { card_editor_content(&ctx) }
        </div>
    }
}

fn empty_state_view(locale: Locale, on_add: Callback<Option<u16>>) -> Html {
    html! {
        <div class="p-8 text-center rounded-lg border-2 border-dashed border-neutral-300 dark:border-neutral-700">
            <svg class="w-10 h-10 mx-auto text-neutral-400 dark:text-neutral-500 mb-3" fill="none" viewBox="0 0 24 24" stroke="currentColor">
                <path stroke-linecap="round" stroke-linejoin="round" stroke-width="1.5" d="M12 9v2m0 4h.01m-6.938 4h13.856c1.54 0 2.502-1.667 1.732-3L13.732 4c-.77-1.333-2.694-1.333-3.464 0L3.34 16c-.77 1.333.192 3 1.732 3z" />
            </svg>
            <p class="text-sm font-medium text-neutral-700 dark:text-neutral-300">
                {locale.t("http_form.error_page_empty_preview")}
            </p>
            <div class="mt-4 flex items-center justify-center gap-2 flex-wrap">
                <button
                    type="button"
                    onclick={
                        let on_add = on_add.clone();
                        Callback::from(move |_| on_add.emit(Some(404)))
                    }
                    class="px-3 py-1.5 text-xs font-semibold rounded-lg bg-neutral-200 dark:bg-neutral-700 hover:bg-neutral-300 dark:hover:bg-neutral-600 text-neutral-800 dark:text-neutral-200 transition-colors"
                >
                    {"+ 404 Not Found"}
                </button>
                <button
                    type="button"
                    onclick={
                        let on_add = on_add.clone();
                        Callback::from(move |_| on_add.emit(Some(502)))
                    }
                    class="px-3 py-1.5 text-xs font-semibold rounded-lg bg-neutral-200 dark:bg-neutral-700 hover:bg-neutral-300 dark:hover:bg-neutral-600 text-neutral-800 dark:text-neutral-200 transition-colors"
                >
                    {"+ 502 Bad Gateway"}
                </button>
                <button
                    type="button"
                    onclick={
                        let on_add = on_add.clone();
                        Callback::from(move |_| on_add.emit(Some(503)))
                    }
                    class="px-3 py-1.5 text-xs font-semibold rounded-lg bg-neutral-200 dark:bg-neutral-700 hover:bg-neutral-300 dark:hover:bg-neutral-600 text-neutral-800 dark:text-neutral-200 transition-colors"
                >
                    {"+ 503 Maintenance"}
                </button>
            </div>
        </div>
    }
}

fn status_presets_view(on_select: Callback<u16>) -> Html {
    let presets = [404, 502, 503, 500, 403, 429];
    html! {
        <div class="flex items-center gap-1">
            { for presets.iter().map(|&code| {
                let on_select = on_select.clone();
                html! {
                    <button
                        type="button"
                        onclick={Callback::from(move |_| on_select.emit(code))}
                        class="px-2 py-0.5 text-xs font-mono font-medium rounded bg-neutral-100 hover:bg-neutral-200 dark:bg-neutral-800 dark:hover:bg-neutral-700 text-neutral-700 dark:text-neutral-300 border border-neutral-300 dark:border-neutral-600 transition-colors"
                    >
                        {code}
                    </button>
                }
            }) }
        </div>
    }
}

fn templates_selector_view(locale: Locale, on_apply: Callback<&'static str>) -> Html {
    html! {
        <div class="flex items-center gap-1.5 flex-wrap">
            <span class="text-xs font-semibold text-neutral-500 dark:text-neutral-400 mr-1">
                {locale.t("http_form.error_page_templates")}{":"}
            </span>
            { for TEMPLATES.iter().map(|tpl| {
                let on_apply = on_apply.clone();
                let id = tpl.id;
                let name = localized_template_name(locale, id);
                html! {
                    <button
                        type="button"
                        onclick={Callback::from(move |_| on_apply.emit(id))}
                        title={tpl.description}
                        class="px-2 py-1 text-xs font-medium rounded-md bg-neutral-100 dark:bg-neutral-800 hover:bg-primary-50 dark:hover:bg-primary-950/40 text-neutral-700 dark:text-neutral-300 border border-neutral-200 dark:border-neutral-700 hover:border-primary-400 dark:hover:border-primary-600 transition-colors"
                    >
                        {name}
                    </button>
                }
            }) }
        </div>
    }
}

fn localized_template_name(locale: Locale, id: &str) -> &str {
    let key = match id {
        "minimal_dark" => "http_form.error_page_template_minimal_dark",
        "modern_glass" => "http_form.error_page_template_modern_glass",
        "cyber_terminal" => "http_form.error_page_template_cyber_terminal",
        "playful_illustration" => "http_form.error_page_template_playful_illustration",
        "maintenance_status" => "http_form.error_page_template_maintenance_status",
        _ => "http_form.error_page_template_minimal_dark",
    };
    locale.t(key)
}

fn view_mode_buttons_view(
    locale: Locale,
    current: ViewMode,
    on_select: Callback<ViewMode>,
) -> Html {
    let modes = [
        (ViewMode::Code, locale.t("http_form.error_page_edit_code")),
        (ViewMode::Preview, locale.t("http_form.error_page_preview")),
        (ViewMode::Split, locale.t("http_form.error_page_split_view")),
    ];
    html! {
        <div class="inline-flex rounded-lg border border-neutral-200 dark:border-neutral-700 p-0.5 bg-neutral-100 dark:bg-neutral-800">
            { for modes.into_iter().map(|(mode, label)| {
                let on_select = on_select.clone();
                let is_active = current == mode;
                let active_class = if is_active {
                    "bg-white dark:bg-neutral-700 text-neutral-900 dark:text-neutral-100 shadow-sm"
                } else {
                    "text-neutral-600 dark:text-neutral-400 hover:text-neutral-900 dark:hover:text-neutral-200"
                };
                html! {
                    <button
                        type="button"
                        onclick={Callback::from(move |_| on_select.emit(mode))}
                        class={classes!("px-2.5", "py-1", "text-xs", "font-medium", "rounded-md", "transition-colors", active_class)}
                    >
                        {label}
                    </button>
                }
            }) }
        </div>
    }
}

fn viewport_buttons_view(
    locale: Locale,
    current: PreviewViewport,
    on_select: Callback<PreviewViewport>,
) -> Html {
    let viewports = [
        (
            PreviewViewport::Desktop,
            locale.t("http_form.error_page_viewport_desktop"),
        ),
        (
            PreviewViewport::Tablet,
            locale.t("http_form.error_page_viewport_tablet"),
        ),
        (
            PreviewViewport::Mobile,
            locale.t("http_form.error_page_viewport_mobile"),
        ),
    ];
    html! {
        <div class="inline-flex rounded-lg border border-neutral-200 dark:border-neutral-700 p-0.5 bg-neutral-100 dark:bg-neutral-800">
            { for viewports.into_iter().map(|(vp, label)| {
                let on_select = on_select.clone();
                let is_active = current == vp;
                let active_class = if is_active {
                    "bg-white dark:bg-neutral-700 text-neutral-900 dark:text-neutral-100 shadow-sm"
                } else {
                    "text-neutral-600 dark:text-neutral-400 hover:text-neutral-900 dark:hover:text-neutral-200"
                };
                html! {
                    <button
                        type="button"
                        onclick={Callback::from(move |_| on_select.emit(vp))}
                        class={classes!("px-2", "py-1", "text-xs", "font-medium", "rounded-md", "transition-colors", active_class)}
                    >
                        {label}
                    </button>
                }
            }) }
        </div>
    }
}

fn preview_iframe_view(locale: Locale, body: &str, viewport: PreviewViewport) -> Html {
    if body.trim().is_empty() {
        html! {
            <div class="h-[360px] flex items-center justify-center p-6 border rounded-lg border-neutral-200 dark:border-neutral-700 bg-neutral-50 dark:bg-neutral-800/40 text-neutral-400 text-xs">
                {locale.t("http_form.error_page_empty_preview")}
            </div>
        }
    } else {
        html! {
            <div class={classes!("transition-all", "duration-200", viewport.container_class())}>
                <iframe
                    class="w-full h-[360px] rounded-lg border border-neutral-300 dark:border-neutral-700 bg-white shadow-inner"
                    srcdoc={body.to_string()}
                    sandbox="allow-same-origin allow-scripts"
                    title="Live Error Page Preview"
                />
            </div>
        }
    }
}

fn code_textarea_view(ctx: &EditorLayoutContext) -> Html {
    let size_color = if ctx.is_oversized {
        "text-red-600 dark:text-red-400 font-bold"
    } else {
        "text-neutral-500 dark:text-neutral-400"
    };
    let on_input = ctx.on_body_change.clone();

    html! {
        <div class="space-y-1">
            <textarea
                rows="14"
                autocapitalize="off"
                spellcheck="false"
                placeholder={ctx.locale.t("http_form.error_page_body_placeholder")}
                value={ctx.body.to_string()}
                oninput={Callback::from(move |e: InputEvent| {
                    if let Some(target) = e.target_dyn_into::<HtmlTextAreaElement>() {
                        on_input.emit(target.value());
                    }
                })}
                class="w-full p-3 font-mono text-xs rounded-lg border border-neutral-300 dark:border-neutral-700 bg-neutral-50 dark:bg-neutral-950 text-neutral-900 dark:text-neutral-100 focus:outline-none focus:ring-2 focus:ring-primary-500 leading-relaxed resize-y"
            />
            <div class="flex items-center justify-between text-xs px-1">
                <div>
                    if let Some(err) = ctx.body_error {
                        <span class="text-red-600 dark:text-red-400 font-medium">{err}</span>
                    }
                </div>
                <div class={size_color}>
                    {format!("{}: {} / {}", ctx.locale.t("http_form.error_page_size"), format_byte_size(ctx.body_len), format_byte_size(MAX_BODY_BYTES))}
                </div>
            </div>
        </div>
    }
}

fn card_editor_content(ctx: &EditorLayoutContext) -> Html {
    match ctx.view_mode {
        ViewMode::Code => code_textarea_view(ctx),
        ViewMode::Preview => preview_iframe_view(ctx.locale, ctx.body, ctx.viewport),
        ViewMode::Split => html! {
            <div class="grid grid-cols-1 lg:grid-cols-2 gap-4 items-start">
                <div>
                    { code_textarea_view(ctx) }
                </div>
                <div class="sticky top-2">
                    { preview_iframe_view(ctx.locale, ctx.body, ctx.viewport) }
                </div>
            </div>
        },
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_status_code() {
        assert_eq!(parse_status_code("404"), 404);
        assert_eq!(parse_status_code("502"), 502);
        assert_eq!(parse_status_code(" 503 "), 503);
        assert_eq!(parse_status_code("invalid"), 404);
        assert_eq!(parse_status_code(""), 404);
    }

    #[test]
    fn test_format_byte_size() {
        assert_eq!(format_byte_size(500), "500 B");
        assert_eq!(format_byte_size(1024), "1.0 KB");
        assert_eq!(format_byte_size(2560), "2.5 KB");
    }

    #[test]
    fn test_viewport_classes() {
        assert_eq!(PreviewViewport::Desktop.container_class(), "w-full");
        assert_eq!(
            PreviewViewport::Tablet.container_class(),
            "max-w-[768px] mx-auto"
        );
        assert_eq!(
            PreviewViewport::Mobile.container_class(),
            "max-w-[375px] mx-auto"
        );
    }
}
