use super::common::*;
use crate::components::action_button::{ActionIconButton, ActionVariant};
use crate::components::error_pages_editor::ErrorPagesEditor;
use r3v3rs3_api::cache::CacheConfig;
use r3v3rs3_api::compression::{
    Compression, CompressionAlgorithm, format_mime_list, parse_mime_list,
};
use r3v3rs3_api::compression_dictionary::CompressionDictionary;
use r3v3rs3_api::error::Error;
use r3v3rs3_api::error_page_theme::ErrorPageTheme;
use r3v3rs3_api::i18n::Locale;
use r3v3rs3_api::id::ShortId;
use r3v3rs3_api::priority::StreamPriorityConfig;
use r3v3rs3_api::proxy::{CustomErrorPage, MaintenanceConfig};
use std::collections::HashMap;
use std::time::Duration;
use yew::prelude::*;

#[derive(Clone, PartialEq, Default)]
pub struct CacheForm {
    pub enabled: bool,
    pub max_size: String,
    pub max_entry_size: String,
    pub default_ttl: String,
    pub stale_while_revalidate: String,
}

impl CacheForm {
    pub fn new(cache: &CacheConfig) -> Self {
        Self {
            enabled: cache.enabled,
            max_size: cache.max_size.to_string(),
            max_entry_size: cache.max_entry_size.to_string(),
            default_ttl: cache.default_ttl.as_secs().to_string(),
            stale_while_revalidate: cache.stale_while_revalidate.as_secs().to_string(),
        }
    }
}

pub fn cache_view(locale: Locale, form: &CacheForm, onchange: Callback<CacheForm>) -> Html {
    let update = |apply: fn(&mut CacheForm, &Event)| {
        let (form, onchange) = (form.clone(), onchange.clone());
        Callback::from(move |event: Event| {
            let mut next = form.clone();
            apply(&mut next, &event);
            onchange.emit(next);
        })
    };
    html! {
        <>
            <div>
                { toggle(update(|form, event| form.enabled = checked(event)), form.enabled, locale.t("http_form.enable_cache"), "mt-4") }
            </div>
            if form.enabled {
                <div class="grid grid-cols-1 sm:grid-cols-3 gap-x-4">
                    <div>
                        <label class={LABEL_CLASS}>{locale.t("http_form.memory_limit")}</label>
                        <input type="number" min="1" value={form.max_size.clone()} onchange={update(|form, event| form.max_size = text(event))} class={INPUT_CLASS} />
                    </div>
                    <div>
                        <label class={LABEL_CLASS}>{locale.t("http_form.max_response_size")}</label>
                        <input type="number" min="1" value={form.max_entry_size.clone()} onchange={update(|form, event| form.max_entry_size = text(event))} class={INPUT_CLASS} />
                    </div>
                    <div>
                        <label class={LABEL_CLASS}>{locale.t("http_form.default_ttl")}</label>
                        <input type="number" min="0" value={form.default_ttl.clone()} onchange={update(|form, event| form.default_ttl = text(event))} class={INPUT_CLASS} />
                    </div>
                    <div>
                        <label class={LABEL_CLASS}>{locale.t("http_form.stale_while_revalidate")}</label>
                        <input type="number" min="0" value={form.stale_while_revalidate.clone()} onchange={update(|form, event| form.stale_while_revalidate = text(event))} class={INPUT_CLASS} />
                    </div>
                </div>
            }
        </>
    }
}

pub fn parse_cache(
    locale: Locale,
    form: &CacheForm,
    key: &str,
    errors: &mut HashMap<String, String>,
) -> CacheConfig {
    let cache = CacheConfig {
        enabled: form.enabled,
        max_size: or_error(
            parse_size(locale, &form.max_size, "http_form.memory_limit_name"),
            key,
            errors,
        ),
        max_entry_size: or_error(
            parse_size(
                locale,
                &form.max_entry_size,
                "http_form.max_response_size_name",
            ),
            key,
            errors,
        ),
        default_ttl: Duration::from_secs(or_error(
            parse_size(locale, &form.default_ttl, "http_form.default_ttl_name"),
            key,
            errors,
        )),
        stale_while_revalidate: Duration::from_secs(or_error(
            parse_size(
                locale,
                &form.stale_while_revalidate,
                "http_form.stale_while_revalidate_name",
            ),
            key,
            errors,
        )),
    };
    if let Err(err) = cache.validate() {
        errors.insert(key.to_string(), locale.error_message(&err));
    }
    cache
}

#[derive(Clone, PartialEq, Default)]
pub struct CompressionForm {
    pub algorithms: Vec<CompressionAlgorithm>,
    pub min_size: String,
    pub mime_types: String,
    pub adaptive_level: bool,
    pub precompressed: bool,
    pub dictionaries: Vec<CompressionDictionary>,
    pub new_dict_name: String,
    pub new_dict_match: String,
    pub new_dict_path: String,
    pub new_dict_data: String,
    pub show_add_dict: bool,
}

impl CompressionForm {
    pub fn new(compression: &Compression) -> Self {
        Self {
            algorithms: compression.algorithms.clone(),
            min_size: compression.min_size.to_string(),
            mime_types: format_mime_list(&compression.mime_types),
            adaptive_level: compression.adaptive_level,
            precompressed: compression.precompressed,
            dictionaries: compression.dictionaries.clone(),
            new_dict_name: String::new(),
            new_dict_match: "/api/*".to_string(),
            new_dict_path: "/.well-known/dict/api.dict".to_string(),
            new_dict_data: String::new(),
            show_add_dict: false,
        }
    }
}

pub fn compression_view(
    locale: Locale,
    form: &CompressionForm,
    onchange: Callback<CompressionForm>,
) -> Html {
    let dicts = form.dictionaries.clone();
    let on_toggle_algo = {
        let (form, onchange) = (form.clone(), onchange.clone());
        move |algorithm: CompressionAlgorithm| {
            let (form, onchange) = (form.clone(), onchange.clone());
            Callback::from(move |event: Event| {
                let mut next = form.clone();
                next.algorithms.retain(|value| *value != algorithm);
                if checked(&event) {
                    next.algorithms.push(algorithm);
                }
                onchange.emit(next);
            })
        }
    };
    let on_toggle_adaptive = {
        let (form, onchange) = (form.clone(), onchange.clone());
        Callback::from(move |event: Event| {
            let mut next = form.clone();
            next.adaptive_level = checked(&event);
            onchange.emit(next);
        })
    };
    let on_toggle_precompressed = {
        let (form, onchange) = (form.clone(), onchange.clone());
        Callback::from(move |event: Event| {
            let mut next = form.clone();
            next.precompressed = checked(&event);
            onchange.emit(next);
        })
    };
    let on_min_size = {
        let (form, onchange) = (form.clone(), onchange.clone());
        Callback::from(move |event: Event| {
            let mut next = form.clone();
            next.min_size = text(&event);
            onchange.emit(next);
        })
    };
    let on_mime_types = {
        let (form, onchange) = (form.clone(), onchange.clone());
        Callback::from(move |event: Event| {
            let mut next = form.clone();
            next.mime_types = text(&event);
            onchange.emit(next);
        })
    };
    let on_toggle_add_dict = {
        let (form, onchange) = (form.clone(), onchange.clone());
        Callback::from(move |_| {
            let mut next = form.clone();
            next.show_add_dict = !next.show_add_dict;
            onchange.emit(next);
        })
    };

    html! {
        <>
            <div class="flex flex-wrap gap-x-6">
                { for CompressionAlgorithm::ALL.into_iter().map(|algorithm| toggle(
                    on_toggle_algo(algorithm),
                    form.algorithms.contains(&algorithm),
                    algorithm.label(),
                    "mt-4",
                )) }
            </div>
            <div class="flex flex-wrap gap-x-6 mt-2">
                { toggle(
                    on_toggle_adaptive,
                    form.adaptive_level,
                    locale.t("http_form.adaptive_compression"),
                    "mt-2",
                ) }
                { toggle(
                    on_toggle_precompressed,
                    form.precompressed,
                    locale.t("http_form.precompressed"),
                    "mt-2",
                ) }
            </div>
            <div class="grid grid-cols-1 sm:grid-cols-3 gap-x-4">
                <div>
                    <label class={LABEL_CLASS}>{locale.t("http_form.min_size")}</label>
                    <input type="number" min="0" value={form.min_size.clone()} onchange={on_min_size} class={INPUT_CLASS} />
                </div>
                <div class="sm:col-span-2">
                    <label class={LABEL_CLASS}>{locale.t("http_form.media_types")}</label>
                    <input type="text" autocapitalize="off" placeholder="text/*, application/json" value={form.mime_types.clone()} onchange={on_mime_types} class={INPUT_CLASS} />
                </div>
            </div>

            <div class="mt-6 border-t border-neutral-200 dark:border-neutral-700 pt-4">
                <div class="flex items-center justify-between mb-3">
                    <div>
                        <h4 class="text-sm font-semibold text-neutral-900 dark:text-neutral-100">
                            {locale.t("http_form.compression_dictionaries")}
                        </h4>
                        <p class="text-xs text-neutral-500 dark:text-neutral-400 mt-0.5">
                            {locale.t("http_form.compression_dictionaries_hint")}
                        </p>
                    </div>
                    <button
                        type="button"
                        onclick={on_toggle_add_dict}
                        class="text-xs px-3 py-1.5 rounded bg-neutral-100 hover:bg-neutral-200 dark:bg-neutral-700 dark:hover:bg-neutral-600 text-neutral-800 dark:text-neutral-200 font-medium transition"
                    >
                        {if form.show_add_dict {
                            locale.t("dialog.cancel")
                        } else {
                            locale.t("http_form.add_dictionary")
                        }}
                    </button>
                </div>

                if form.show_add_dict {
                    { add_dictionary_card(locale, form, &onchange) }
                }

                if !dicts.is_empty() {
                    <div class="overflow-x-auto rounded border border-neutral-200 dark:border-neutral-700">
                        <table class="w-full text-xs text-left">
                            <thead class="bg-neutral-50 dark:bg-neutral-800 text-neutral-600 dark:text-neutral-400 border-b border-neutral-200 dark:border-neutral-700">
                                <tr>
                                    <th class="px-3 py-2 font-medium">{locale.t("http_form.dictionary_name")}</th>
                                    <th class="px-3 py-2 font-medium">{locale.t("http_form.dictionary_match")}</th>
                                    <th class="px-3 py-2 font-medium">{locale.t("http_form.dictionary_path")}</th>
                                    <th class="px-3 py-2 font-medium">{"SHA-256"}</th>
                                    <th class="px-3 py-2 text-right font-medium">{locale.t("common.delete")}</th>
                                </tr>
                            </thead>
                            <tbody class="divide-y divide-neutral-200 dark:divide-neutral-700">
                                { for dicts.into_iter().enumerate().map(|(idx, dict)| {
                                    let on_delete = {
                                        let (form, onchange) = (form.clone(), onchange.clone());
                                        Callback::from(move |_| {
                                            let mut next = form.clone();
                                            if idx < next.dictionaries.len() {
                                                next.dictionaries.remove(idx);
                                                onchange.emit(next);
                                            }
                                        })
                                    };
                                    html! {
                                        <tr class="hover:bg-neutral-50/50 dark:hover:bg-neutral-800/50">
                                            <td class="px-3 py-2 font-medium text-neutral-900 dark:text-neutral-100">{dict.name}</td>
                                            <td class="px-3 py-2 font-mono text-neutral-600 dark:text-neutral-300">{dict.match_pattern}</td>
                                            <td class="px-3 py-2 font-mono text-neutral-600 dark:text-neutral-300">{dict.path}</td>
                                            <td class="px-3 py-2 font-mono text-neutral-500 text-[10px] truncate max-w-[160px]" title={dict.sha256_sf.clone()}>
                                                {dict.sha256_sf}
                                            </td>
                                            <td class="px-3 py-2 text-right">
                                                <ActionIconButton
                                                    variant={ActionVariant::Delete}
                                                    title={locale.t("common.delete").to_string()}
                                                    onclick={on_delete}
                                                />
                                            </td>
                                        </tr>
                                    }
                                }) }
                            </tbody>
                        </table>
                    </div>
                }
            </div>
        </>
    }
}

pub fn add_dictionary_card(
    locale: Locale,
    form: &CompressionForm,
    onchange: &Callback<CompressionForm>,
) -> Html {
    let on_add = {
        let (form, onchange) = (form.clone(), onchange.clone());
        Callback::from(move |_| {
            let mut next = form.clone();
            let name = next.new_dict_name.trim().to_string();
            let match_pattern = next.new_dict_match.trim().to_string();
            let path = next.new_dict_path.trim().to_string();
            let raw_data = next.new_dict_data.clone();
            if !name.is_empty()
                && !match_pattern.is_empty()
                && !path.is_empty()
                && !raw_data.is_empty()
            {
                let mut dict = CompressionDictionary {
                    id: ShortId::new(),
                    name,
                    match_pattern,
                    path,
                    raw_data,
                    sha256_sf: String::new(),
                };
                dict.ensure_hash();
                next.dictionaries.push(dict);
                next.new_dict_name.clear();
                next.new_dict_data.clear();
                next.show_add_dict = false;
                onchange.emit(next);
            }
        })
    };
    let on_name = {
        let (form, onchange) = (form.clone(), onchange.clone());
        Callback::from(move |event: Event| {
            let mut next = form.clone();
            next.new_dict_name = text(&event);
            onchange.emit(next);
        })
    };
    let on_match = {
        let (form, onchange) = (form.clone(), onchange.clone());
        Callback::from(move |event: Event| {
            let mut next = form.clone();
            next.new_dict_match = text(&event);
            onchange.emit(next);
        })
    };
    let on_path = {
        let (form, onchange) = (form.clone(), onchange.clone());
        Callback::from(move |event: Event| {
            let mut next = form.clone();
            next.new_dict_path = text(&event);
            onchange.emit(next);
        })
    };
    let on_data = {
        let (form, onchange) = (form.clone(), onchange.clone());
        Callback::from(move |event: Event| {
            let mut next = form.clone();
            next.new_dict_data = text(&event);
            onchange.emit(next);
        })
    };

    html! {
        <div class="mb-4 p-3 rounded-lg bg-neutral-50 dark:bg-neutral-800/60 border border-neutral-200 dark:border-neutral-700 text-xs space-y-3">
            <div class="grid grid-cols-1 sm:grid-cols-3 gap-3">
                <div>
                    <label class={LABEL_CLASS}>{locale.t("http_form.dictionary_name")}</label>
                    <input
                        type="text"
                        placeholder="API Schema v1"
                        value={form.new_dict_name.clone()}
                        onchange={on_name}
                        class={INPUT_CLASS}
                    />
                </div>
                <div>
                    <label class={LABEL_CLASS}>{locale.t("http_form.dictionary_match")}</label>
                    <input
                        type="text"
                        placeholder="/api/*"
                        value={form.new_dict_match.clone()}
                        onchange={on_match}
                        class={INPUT_CLASS}
                    />
                </div>
                <div>
                    <label class={LABEL_CLASS}>{locale.t("http_form.dictionary_path")}</label>
                    <input
                        type="text"
                        placeholder="/.well-known/dict/api.dict"
                        value={form.new_dict_path.clone()}
                        onchange={on_path}
                        class={INPUT_CLASS}
                    />
                </div>
            </div>
            <div>
                <label class={LABEL_CLASS}>{locale.t("http_form.dictionary_raw_data")}</label>
                <textarea
                    rows="3"
                    placeholder="{\"status\":200,\"data\":[],\"meta\":{}}"
                    value={form.new_dict_data.clone()}
                    onchange={on_data}
                    class={INPUT_CLASS}
                />
            </div>
            <div class="flex justify-end">
                <button
                    type="button"
                    onclick={on_add}
                    class="px-3 py-1.5 rounded bg-primary-600 hover:bg-primary-700 text-white font-medium transition"
                >
                    {locale.t("http_form.save_dictionary")}
                </button>
            </div>
        </div>
    }
}

pub fn parse_compression(
    locale: Locale,
    form: &CompressionForm,
    key: &str,
    errors: &mut HashMap<String, String>,
) -> Compression {
    Compression {
        algorithms: form.algorithms.clone(),
        min_size: or_error(
            parse_size(locale, &form.min_size, "http_form.min_size_name"),
            key,
            errors,
        ),
        mime_types: or_error(
            translated(locale, parse_mime_list(&form.mime_types)),
            key,
            errors,
        ),
        adaptive_level: form.adaptive_level,
        precompressed: form.precompressed,
        dictionaries: form.dictionaries.clone(),
    }
}

#[derive(Clone, PartialEq, Default)]
pub struct MaintenanceForm {
    pub enabled: bool,
    pub status: String,
    pub body: String,
    pub retry_after: String,
}

impl MaintenanceForm {
    pub fn new(config: &MaintenanceConfig) -> Self {
        Self {
            enabled: config.enabled,
            status: config.status.to_string(),
            body: config.body.clone(),
            retry_after: config
                .retry_after
                .map(|secs| secs.to_string())
                .unwrap_or_default(),
        }
    }
}

pub fn maintenance_view(
    locale: Locale,
    form: &MaintenanceForm,
    onchange: Callback<MaintenanceForm>,
    errors: &HashMap<String, String>,
) -> Html {
    let update = |apply: fn(&mut MaintenanceForm, &Event)| {
        let (form, onchange) = (form.clone(), onchange.clone());
        Callback::from(move |event: Event| {
            let mut next = form.clone();
            apply(&mut next, &event);
            onchange.emit(next);
        })
    };
    html! {
        <div class="mt-6 p-4 rounded-lg bg-neutral-100 dark:bg-neutral-800/60 border border-neutral-200 dark:border-neutral-700">
            <label class="block text-sm font-semibold text-neutral-900 dark:text-neutral-100">{locale.t("http_form.maintenance")}</label>
            <p class={HINT_CLASS}>{locale.t("http_form.maintenance_hint")}</p>
            { toggle(update(|form, event| form.enabled = checked(event)), form.enabled, locale.t("http_form.maintenance_enabled"), "mt-3") }
            if form.enabled {
                <div class="grid gap-4 sm:grid-cols-2 mt-4">
                    <div>
                        <label class={LABEL_CLASS}>{locale.t("http_form.maintenance_status")}</label>
                        <input type="number" min="100" max="599" value={form.status.clone()} onchange={update(|form, event| form.status = text(event))} class={INPUT_CLASS} placeholder="503" />
                        { error_view(errors.get("maintenance_status")) }
                    </div>
                    <div>
                        <label class={LABEL_CLASS}>{locale.t("http_form.maintenance_retry_after")}</label>
                        <input type="number" min="0" value={form.retry_after.clone()} onchange={update(|form, event| form.retry_after = text(event))} class={INPUT_CLASS} placeholder="3600" />
                        { error_view(errors.get("maintenance_retry_after")) }
                    </div>
                </div>
                <label class={classes!(LABEL_CLASS, "mt-3")}>{locale.t("http_form.maintenance_body")}</label>
                <textarea rows="3" autocapitalize="off" spellcheck="false" placeholder={locale.t("http_form.maintenance_body_placeholder")} value={form.body.clone()} onchange={update(|form, event| form.body = text_area(event))} class={INPUT_CLASS} />
                <p class={HINT_CLASS}>{locale.t("http_form.maintenance_body_hint")}</p>
            }
        </div>
    }
}

pub fn parse_maintenance(
    locale: Locale,
    form: &MaintenanceForm,
    errors: &mut HashMap<String, String>,
) -> MaintenanceConfig {
    let status = form.status.trim().parse::<u16>().unwrap_or(503);
    if !(100..=599).contains(&status) {
        errors.insert(
            "maintenance_status".into(),
            locale.error_message(&Error::InvalidFixedStatus { status }),
        );
    }
    let retry_after = if form.retry_after.trim().is_empty() {
        None
    } else {
        match form.retry_after.trim().parse::<u32>() {
            Ok(val) => Some(val),
            Err(_) => {
                errors.insert(
                    "maintenance_retry_after".into(),
                    locale.tf(
                        "http_form.whole_number",
                        &[("name", locale.t("http_form.maintenance_retry_after"))],
                    ),
                );
                None
            }
        }
    };
    MaintenanceConfig {
        enabled: form.enabled,
        status,
        body: form.body.clone(),
        retry_after,
    }
}

#[derive(Clone, PartialEq, Default)]
pub struct ErrorPageForm {
    pub status: String,
    pub body: String,
}

pub fn error_page_theme_select(
    locale: Locale,
    onchange: Callback<Event>,
    selected: Option<ShortId>,
    themes: &[ErrorPageTheme],
) -> Html {
    let unknown = selected.filter(|id| themes.iter().all(|t| t.id != *id));
    let options = html! {
        <>
            <option value="" selected={selected.is_none()}>{locale.t("http_form.error_page_theme_none")}</option>
            { for themes.iter().map(|theme| html! {
                <option value={theme.id.to_string()} selected={selected == Some(theme.id)}>
                    {format!("{} ({} {})", theme.name, theme.pages.len(), locale.t("http_form.pages_count_suffix"))}
                </option>
            }) }
            if let Some(id) = unknown {
                <option value={id.to_string()} selected=true>{id.to_string()}</option>
            }
        </>
    };
    select_field(
        locale.t("http_form.error_page_theme"),
        onchange,
        options,
        Some(locale.t("http_form.error_page_theme_hint")),
    )
}

pub fn dynamic_variables_banner(locale: Locale) -> Html {
    html! {
        <div class="mt-3 p-3 bg-neutral-50 dark:bg-neutral-800/40 rounded-lg border border-neutral-200 dark:border-neutral-700">
            <span class="text-xs font-semibold text-neutral-700 dark:text-neutral-300 block mb-1.5">
                {locale.t("http_form.dynamic_variables_title")}
            </span>
            <div class="flex flex-wrap gap-1.5 text-xs">
                <span class="inline-flex items-center px-2 py-0.5 rounded bg-blue-100 text-blue-800 dark:bg-blue-900/60 dark:text-blue-300 font-mono">
                    {"{{status}}"}
                </span>
                <span class="inline-flex items-center px-2 py-0.5 rounded bg-blue-100 text-blue-800 dark:bg-blue-900/60 dark:text-blue-300 font-mono">
                    {"{{message}}"}
                </span>
                <span class="inline-flex items-center px-2 py-0.5 rounded bg-blue-100 text-blue-800 dark:bg-blue-900/60 dark:text-blue-300 font-mono">
                    {"{{request_id}}"}
                </span>
                <span class="inline-flex items-center px-2 py-0.5 rounded bg-blue-100 text-blue-800 dark:bg-blue-900/60 dark:text-blue-300 font-mono">
                    {"{{host}}"}
                </span>
                <span class="inline-flex items-center px-2 py-0.5 rounded bg-blue-100 text-blue-800 dark:bg-blue-900/60 dark:text-blue-300 font-mono">
                    {"{{client_ip}}"}
                </span>
            </div>
            <p class="mt-1.5 text-xs text-neutral-500 dark:text-neutral-400">
                {locale.t("http_form.dynamic_variables_hint")}
            </p>
        </div>
    }
}

pub struct ErrorPagesViewProps<'a> {
    pub error_pages: &'a [ErrorPageForm],
    pub error_page_theme: Option<ShortId>,
    pub hide_branding: bool,
    pub on_pages_change: Callback<Vec<ErrorPageForm>>,
    pub on_theme_change: Callback<Event>,
    pub on_branding_change: Callback<Event>,
    pub errors: &'a HashMap<String, String>,
    pub themes: &'a [ErrorPageTheme],
}

pub fn error_pages_view(locale: Locale, props: &ErrorPagesViewProps<'_>) -> Html {
    let theme_select = error_page_theme_select(
        locale,
        props.on_theme_change.clone(),
        props.error_page_theme,
        props.themes,
    );
    html! {
        <div class="mt-6">
            { theme_select }
            { dynamic_variables_banner(locale) }
            { toggle(props.on_branding_change.clone(), props.hide_branding, locale.t("http_form.hide_branding"), "mt-4") }
            <p class={HINT_CLASS}>{locale.t("http_form.hide_branding_hint")}</p>
            <div class="mt-4">
                <ErrorPagesEditor
                    error_pages={props.error_pages.to_vec()}
                    on_change={props.on_pages_change.clone()}
                    errors={props.errors.clone()}
                />
            </div>
        </div>
    }
}

pub fn parse_error_pages(
    locale: Locale,
    pages: &[ErrorPageForm],
    errors: &mut HashMap<String, String>,
) -> Vec<CustomErrorPage> {
    let mut result = Vec::new();
    for (index, page) in pages.iter().enumerate() {
        let key = format!("error_pages_{index}_status");
        let status = match page.status.trim().parse::<u16>() {
            Ok(code) if (400..=599).contains(&code) => code,
            _ => {
                errors.insert(
                    key,
                    locale.error_message(&Error::InvalidErrorPageStatus {
                        status: page.status.trim().parse::<u16>().unwrap_or(0),
                    }),
                );
                400
            }
        };
        let body = page.body.clone();
        if body.len() > 65536 {
            let body_key = format!("error_pages_{index}_body");
            errors.insert(
                body_key,
                locale.error_message(&Error::ErrorPageBodyTooLarge { max: 65536 }),
            );
        }
        result.push(CustomErrorPage { status, body });
    }
    result
}

#[hook]
pub fn use_error_page_themes() -> UseStateHandle<Vec<ErrorPageTheme>> {
    use_loaded_list("/error-page-themes", |_| true)
}

pub fn stream_priority_subtoggles(
    locale: Locale,
    config: &StreamPriorityConfig,
    on_inject_header: Callback<Event>,
    on_adaptive_concurrency: Callback<Event>,
) -> Html {
    if !config.enabled {
        return html! {};
    }
    html! {
        <div class="ml-4 pl-4 border-l-2 border-primary-200 dark:border-primary-800 space-y-4 my-4">
            <div>
                { toggle(on_inject_header, config.inject_header, locale.t("proxies.stream_priority_inject_header"), "my-2") }
                <p class={HINT_CLASS}>{locale.t("proxies.stream_priority_inject_header_hint")}</p>
            </div>
            <div>
                { toggle(on_adaptive_concurrency, config.adaptive_concurrency, locale.t("proxies.stream_priority_adaptive_concurrency"), "my-2") }
                <p class={HINT_CLASS}>{locale.t("proxies.stream_priority_adaptive_concurrency_hint")}</p>
            </div>
        </div>
    }
}
