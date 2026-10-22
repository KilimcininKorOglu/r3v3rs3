use crate::i18n::use_locale;
use r3v3rs3_api::geoip::{
    CountryInfo, REGIONAL_PRESETS, RegionalPreset, add_country, country_name, format_countries,
    parse_countries, remove_country, search_countries,
};
use r3v3rs3_api::i18n::Locale;
use web_sys::HtmlInputElement;
use yew::prelude::*;

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum GeoIpMode {
    Off,
    Allowlist,
    Denylist,
    Advanced,
}

#[derive(Properties, PartialEq, Clone)]
pub struct GeoIpRuleWizardProps {
    pub allow_countries: String,
    pub deny_countries: String,
    pub on_allow_change: Callback<String>,
    pub on_deny_change: Callback<String>,
}

#[function_component(GeoIpRuleWizard)]
pub fn geoip_rule_wizard(props: &GeoIpRuleWizardProps) -> Html {
    let locale = use_locale();
    let is_tr = locale == Locale::Tr;
    let search_query = use_state(String::new);
    let show_raw = use_state(|| false);

    let allow_list = parse_countries(&props.allow_countries);
    let deny_list = parse_countries(&props.deny_countries);
    let current_mode = determine_active_mode(&allow_list, &deny_list);
    let mode_state = use_state(|| current_mode);

    // Sync mode_state if external props change
    {
        let mode_state = mode_state.clone();
        use_effect_with(
            (props.allow_countries.clone(), props.deny_countries.clone()),
            move |(allow, deny)| {
                let a = parse_countries(allow);
                let d = parse_countries(deny);
                let detected = determine_active_mode(&a, &d);
                if *mode_state != detected {
                    mode_state.set(detected);
                }
            },
        );
    }

    let active_mode = *mode_state;
    let on_mode_select = {
        let mode_state = mode_state.clone();
        let on_allow_change = props.on_allow_change.clone();
        let on_deny_change = props.on_deny_change.clone();
        Callback::from(move |new_mode: GeoIpMode| {
            mode_state.set(new_mode);
            match new_mode {
                GeoIpMode::Off => {
                    on_allow_change.emit(String::new());
                    on_deny_change.emit(String::new());
                }
                GeoIpMode::Allowlist => {
                    on_deny_change.emit(String::new());
                }
                GeoIpMode::Denylist => {
                    on_allow_change.emit(String::new());
                }
                GeoIpMode::Advanced => {}
            }
        })
    };

    let on_apply_preset = {
        let on_allow_change = props.on_allow_change.clone();
        let on_deny_change = props.on_deny_change.clone();
        Callback::from(move |(mode, preset_countries): (GeoIpMode, Vec<String>)| {
            let formatted = format_countries(&preset_countries);
            match mode {
                GeoIpMode::Denylist => {
                    on_deny_change.emit(formatted);
                    on_allow_change.emit(String::new());
                }
                _ => {
                    on_allow_change.emit(formatted);
                    on_deny_change.emit(String::new());
                }
            }
        })
    };

    let on_remove_country = {
        let on_allow_change = props.on_allow_change.clone();
        let on_deny_change = props.on_deny_change.clone();
        let allow_list = allow_list.clone();
        let deny_list = deny_list.clone();
        Callback::from(move |(is_allow, code): (bool, String)| {
            if is_allow {
                let updated = remove_country(&allow_list, &code);
                on_allow_change.emit(format_countries(&updated));
            } else {
                let updated = remove_country(&deny_list, &code);
                on_deny_change.emit(format_countries(&updated));
            }
        })
    };

    let on_add_country = {
        let on_allow_change = props.on_allow_change.clone();
        let on_deny_change = props.on_deny_change.clone();
        let allow_list = allow_list.clone();
        let deny_list = deny_list.clone();
        let search_query = search_query.clone();
        Callback::from(move |(is_allow, code): (bool, String)| {
            if is_allow {
                let updated = add_country(&allow_list, &code);
                on_allow_change.emit(format_countries(&updated));
            } else {
                let updated = add_country(&deny_list, &code);
                on_deny_change.emit(format_countries(&updated));
            }
            search_query.set(String::new());
        })
    };

    let editor_ctx = RuleEditorContext {
        locale,
        search_query: &search_query,
        on_add: on_add_country,
        on_remove: on_remove_country,
    };

    html! {
        <div class="rounded-xl border border-neutral-200 dark:border-neutral-700 bg-neutral-50/50 dark:bg-neutral-850/50 p-4 space-y-4 shadow-sm">
            { header_view(locale) }
            { mode_selector_view(locale, active_mode, on_mode_select) }
            { summary_banner_view(locale, active_mode, allow_list.len(), deny_list.len()) }

            if active_mode != GeoIpMode::Off {
                <div class="space-y-4 pt-1">
                    { presets_bar_view(locale, is_tr, active_mode, on_apply_preset) }
                    { active_rules_editor_view(
                        &editor_ctx,
                        active_mode,
                        &allow_list,
                        &deny_list,
                    ) }
                    { raw_inputs_toggle_view(
                        locale,
                        &show_raw,
                        props,
                    ) }
                </div>
            }
        </div>
    }
}

pub fn determine_active_mode(allow: &[String], deny: &[String]) -> GeoIpMode {
    if !allow.is_empty() && !deny.is_empty() {
        GeoIpMode::Advanced
    } else if !allow.is_empty() {
        GeoIpMode::Allowlist
    } else if !deny.is_empty() {
        GeoIpMode::Denylist
    } else {
        GeoIpMode::Off
    }
}

fn header_view(locale: Locale) -> Html {
    html! {
        <div class="flex items-start justify-between gap-3 border-b border-neutral-200 dark:border-neutral-700/80 pb-3">
            <div>
                <h3 class="text-sm font-semibold text-neutral-900 dark:text-neutral-100 flex items-center gap-2">
                    <svg class="w-4 h-4 text-blue-600 dark:text-blue-400" fill="none" stroke="currentColor" viewBox="0 0 24 24">
                        <path stroke-linecap="round" stroke-linejoin="round" stroke-width="2" d="M3.055 11H5a2 2 0 012 2v1a2 2 0 002 2 2 2 0 012 2v2.945M8 3.935V5.5A2.5 2.5 0 0010.5 8h.5a2 2 0 012 2 2 2 0 104 0 2 2 0 012-2h1.064M15 20.488V18a2 2 0 012-2h3.064M21 12a9 9 0 11-18 0 9 9 0 0118 0z" />
                    </svg>
                    { locale.t("geoip.title") }
                </h3>
                <p class="text-xs text-neutral-500 dark:text-neutral-400 mt-0.5">
                    { locale.t("geoip.desc") }
                </p>
            </div>
        </div>
    }
}

fn mode_selector_view(locale: Locale, current: GeoIpMode, on_select: Callback<GeoIpMode>) -> Html {
    let modes = [
        (GeoIpMode::Off, locale.t("geoip.mode_off")),
        (GeoIpMode::Allowlist, locale.t("geoip.mode_allow")),
        (GeoIpMode::Denylist, locale.t("geoip.mode_deny")),
        (GeoIpMode::Advanced, locale.t("geoip.mode_advanced")),
    ];

    html! {
        <div class="inline-flex rounded-lg p-1 bg-neutral-200 dark:bg-neutral-800 text-xs font-medium w-full sm:w-auto">
            { for modes.into_iter().map(|(mode, label)| {
                let is_active = current == mode;
                let onclick = {
                    let on_select = on_select.clone();
                    Callback::from(move |_: MouseEvent| on_select.emit(mode))
                };
                let class_name = if is_active {
                    "flex-1 sm:flex-initial text-center px-3 py-1.5 rounded-md bg-white dark:bg-neutral-700 text-neutral-900 dark:text-neutral-100 shadow-sm font-semibold transition-all"
                } else {
                    "flex-1 sm:flex-initial text-center px-3 py-1.5 rounded-md text-neutral-600 dark:text-neutral-400 hover:text-neutral-900 dark:hover:text-neutral-200 transition-all"
                };
                html! {
                    <button type="button" class={class_name} onclick={onclick}>
                        { label }
                    </button>
                }
            }) }
        </div>
    }
}

fn summary_banner_view(
    locale: Locale,
    mode: GeoIpMode,
    allow_count: usize,
    deny_count: usize,
) -> Html {
    match mode {
        GeoIpMode::Off => html! {
            <div class="rounded-lg p-2.5 bg-neutral-100 dark:bg-neutral-800 text-xs text-neutral-600 dark:text-neutral-400 flex items-center gap-2">
                <span class="text-base">{"🌐"}</span>
                <span>{ locale.t("geoip.active_rule_summary_off") }</span>
            </div>
        },
        GeoIpMode::Allowlist => html! {
            <div class="rounded-lg p-2.5 bg-blue-50 dark:bg-blue-950/40 border border-blue-200 dark:border-blue-800/60 text-xs text-blue-800 dark:text-blue-300 flex items-center gap-2">
                <span class="text-base">{"🛡️"}</span>
                <span>{ locale.t("geoip.active_rule_summary_allow").replace("{count}", &allow_count.to_string()) }</span>
            </div>
        },
        GeoIpMode::Denylist => html! {
            <div class="rounded-lg p-2.5 bg-amber-50 dark:bg-amber-950/40 border border-amber-200 dark:border-amber-800/60 text-xs text-amber-800 dark:text-amber-300 flex items-center gap-2">
                <span class="text-base">{"🚫"}</span>
                <span>{ locale.t("geoip.active_rule_summary_deny").replace("{count}", &deny_count.to_string()) }</span>
            </div>
        },
        GeoIpMode::Advanced => html! {
            <div class="rounded-lg p-2.5 bg-purple-50 dark:bg-purple-950/40 border border-purple-200 dark:border-purple-800/60 text-xs text-purple-800 dark:text-purple-300 flex items-center gap-2">
                <span class="text-base">{"⚙️"}</span>
                <span>{ format!("Allowed: {} countries | Blocked: {} countries", allow_count, deny_count) }</span>
            </div>
        },
    }
}

fn presets_bar_view(
    locale: Locale,
    is_tr: bool,
    active_mode: GeoIpMode,
    on_apply: Callback<(GeoIpMode, Vec<String>)>,
) -> Html {
    let on_clear = {
        let on_apply = on_apply.clone();
        Callback::from(move |_: MouseEvent| on_apply.emit((active_mode, Vec::new())))
    };

    html! {
        <div class="space-y-1.5">
            <span class="text-xs font-semibold uppercase tracking-wider text-neutral-500 dark:text-neutral-400">
                { locale.t("geoip.presets_title") }
            </span>
            <div class="flex flex-wrap items-center gap-1.5">
                { for REGIONAL_PRESETS.iter().map(|preset| render_preset_button(preset, is_tr, active_mode, on_apply.clone())) }
                <button
                    type="button"
                    onclick={on_clear}
                    class="text-xs px-2.5 py-1 rounded-md border border-neutral-300 dark:border-neutral-700 bg-white dark:bg-neutral-800 hover:bg-neutral-100 dark:hover:bg-neutral-750 text-neutral-600 dark:text-neutral-300 font-medium transition-colors"
                >
                    { locale.t("geoip.clear_all") }
                </button>
            </div>
        </div>
    }
}

fn render_preset_button(
    preset: &'static RegionalPreset,
    is_tr: bool,
    mode: GeoIpMode,
    on_apply: Callback<(GeoIpMode, Vec<String>)>,
) -> Html {
    let name = if is_tr {
        preset.name_tr
    } else {
        preset.name_en
    };
    let list: Vec<String> = preset.countries.iter().map(|&c| c.to_string()).collect();
    let onclick = {
        let on_apply = on_apply.clone();
        Callback::from(move |_: MouseEvent| {
            on_apply.emit((mode, list.clone()));
        })
    };

    html! {
        <button
            type="button"
            onclick={onclick}
            class="text-xs px-2.5 py-1 rounded-md border border-blue-200 dark:border-blue-800/80 bg-blue-50/70 dark:bg-blue-950/30 hover:bg-blue-100 dark:hover:bg-blue-900/50 text-blue-700 dark:text-blue-300 font-medium transition-colors"
        >
            { name }
        </button>
    }
}

struct RuleEditorContext<'a> {
    locale: Locale,
    search_query: &'a UseStateHandle<String>,
    on_add: Callback<(bool, String)>,
    on_remove: Callback<(bool, String)>,
}

fn active_rules_editor_view(
    ctx: &RuleEditorContext<'_>,
    mode: GeoIpMode,
    allow: &[String],
    deny: &[String],
) -> Html {
    match mode {
        GeoIpMode::Allowlist => {
            country_section_view(ctx, true, ctx.locale.t("geoip.allow_label"), allow)
        }
        GeoIpMode::Denylist => {
            country_section_view(ctx, false, ctx.locale.t("geoip.deny_label"), deny)
        }
        GeoIpMode::Advanced => html! {
            <div class="space-y-4">
                { country_section_view(
                    ctx,
                    true,
                    ctx.locale.t("geoip.allow_label"),
                    allow,
                ) }
                { country_section_view(
                    ctx,
                    false,
                    ctx.locale.t("geoip.deny_label"),
                    deny,
                ) }
            </div>
        },
        GeoIpMode::Off => html! {},
    }
}

fn country_section_view(
    ctx: &RuleEditorContext<'_>,
    is_allow: bool,
    title: &'static str,
    codes: &[String],
) -> Html {
    let is_tr = ctx.locale == Locale::Tr;
    let header_color = if is_allow {
        "text-emerald-700 dark:text-emerald-400"
    } else {
        "text-red-700 dark:text-red-400"
    };

    html! {
        <div class="space-y-2 p-3 rounded-lg border border-neutral-200 dark:border-neutral-700/80 bg-white dark:bg-neutral-800">
            <div class="flex items-center justify-between">
                <span class={classes!("text-xs", "font-bold", header_color)}>
                    { title }
                </span>
                <span class="text-xs text-neutral-500 font-mono">
                    { format!("{}: {}", ctx.locale.t("geoip.selected_countries").replace("{count}", &codes.len().to_string()), codes.len()) }
                </span>
            </div>

            { chips_list_view(is_tr, is_allow, codes, ctx.on_remove.clone()) }
            { country_search_input_view(ctx.locale, is_tr, is_allow, codes, ctx.search_query, ctx.on_add.clone()) }
        </div>
    }
}

fn chips_list_view(
    is_tr: bool,
    is_allow: bool,
    codes: &[String],
    on_remove: Callback<(bool, String)>,
) -> Html {
    if codes.is_empty() {
        return html! {
            <p class="text-xs text-neutral-400 dark:text-neutral-500 italic py-1">
                { "No countries configured." }
            </p>
        };
    }

    let chip_class = if is_allow {
        "inline-flex items-center gap-1.5 px-2 py-1 rounded text-xs font-medium bg-emerald-50 dark:bg-emerald-950/50 border border-emerald-200 dark:border-emerald-800 text-emerald-800 dark:text-emerald-200"
    } else {
        "inline-flex items-center gap-1.5 px-2 py-1 rounded text-xs font-medium bg-red-50 dark:bg-red-950/50 border border-red-200 dark:border-red-800 text-red-800 dark:text-red-200"
    };

    html! {
        <div class="flex flex-wrap gap-1.5 max-h-36 overflow-y-auto pr-1">
            { for codes.iter().map(|code| {
                let name = country_name(code, is_tr);
                let remove_code = code.clone();
                let on_remove_click = {
                    let on_remove = on_remove.clone();
                    Callback::from(move |_: MouseEvent| {
                        on_remove.emit((is_allow, remove_code.clone()));
                    })
                };
                html! {
                    <span class={chip_class}>
                        <span class="font-mono font-bold">{ code }</span>
                        if !name.is_empty() {
                            <span class="text-neutral-500 dark:text-neutral-400">{"·"}</span>
                            <span>{ name }</span>
                        }
                        <button
                            type="button"
                            onclick={on_remove_click}
                            class="hover:opacity-75 font-bold ml-0.5 text-xs focus:outline-none"
                            title="Remove"
                        >
                            {"✕"}
                        </button>
                    </span>
                }
            }) }
        </div>
    }
}

fn country_search_input_view(
    locale: Locale,
    is_tr: bool,
    is_allow: bool,
    existing: &[String],
    search_query: &UseStateHandle<String>,
    on_add: Callback<(bool, String)>,
) -> Html {
    let query_str = (**search_query).clone();
    let on_input = {
        let search_query = search_query.clone();
        Callback::from(move |e: InputEvent| {
            if let Some(input) = e.target_dyn_into::<HtmlInputElement>() {
                search_query.set(input.value());
            }
        })
    };

    let search_results = if query_str.trim().is_empty() {
        Vec::new()
    } else {
        search_countries(&query_str, is_tr, 6)
    };

    html! {
        <div class="relative pt-1">
            <input
                type="text"
                placeholder={locale.t("geoip.search_placeholder")}
                value={query_str.clone()}
                oninput={on_input}
                class="w-full text-xs px-2.5 py-1.5 rounded border border-neutral-300 dark:border-neutral-700 bg-white dark:bg-neutral-800 text-neutral-900 dark:text-neutral-100 placeholder-neutral-400 focus:outline-none focus:ring-1 focus:ring-blue-500"
            />
            if !search_results.is_empty() {
                <div class="absolute left-0 right-0 mt-1 max-h-48 overflow-y-auto rounded-lg border border-neutral-200 dark:border-neutral-700 bg-white dark:bg-neutral-800 shadow-lg z-20 divide-y divide-neutral-100 dark:divide-neutral-700/60">
                    { for search_results.into_iter().map(|country| render_search_result_item(country, is_tr, is_allow, existing, on_add.clone())) }
                </div>
            }
        </div>
    }
}

fn render_search_result_item(
    country: &'static CountryInfo,
    is_tr: bool,
    is_allow: bool,
    existing: &[String],
    on_add: Callback<(bool, String)>,
) -> Html {
    let code = country.code.to_string();
    let is_already_added = existing.iter().any(|c| c.eq_ignore_ascii_case(&code));
    let name = if is_tr {
        country.name_tr
    } else {
        country.name_en
    };
    let onclick = {
        let on_add = on_add.clone();
        let code = code.clone();
        Callback::from(move |_: MouseEvent| {
            if !is_already_added {
                on_add.emit((is_allow, code.clone()));
            }
        })
    };

    html! {
        <div
            onclick={onclick}
            class={if is_already_added {
                "flex items-center justify-between px-3 py-1.5 text-xs text-neutral-400 dark:text-neutral-500 bg-neutral-50 dark:bg-neutral-850 cursor-not-allowed"
            } else {
                "flex items-center justify-between px-3 py-1.5 text-xs hover:bg-neutral-100 dark:hover:bg-neutral-700 cursor-pointer"
            }}
        >
            <div class="flex items-center gap-2">
                <span class="font-mono font-bold text-neutral-800 dark:text-neutral-200">{ country.code }</span>
                <span class="text-neutral-600 dark:text-neutral-300">{ name }</span>
            </div>
            if is_already_added {
                <span class="text-xs text-neutral-400">{"✓ Added"}</span>
            } else {
                <span class="text-xs font-semibold text-blue-600 dark:text-blue-400">{"+ Add"}</span>
            }
        </div>
    }
}

fn raw_inputs_toggle_view(
    locale: Locale,
    show_raw: &UseStateHandle<bool>,
    props: &GeoIpRuleWizardProps,
) -> Html {
    let on_toggle_click = {
        let show_raw = show_raw.clone();
        Callback::from(move |_: MouseEvent| show_raw.set(!*show_raw))
    };

    let on_allow_input = {
        let on_allow_change = props.on_allow_change.clone();
        Callback::from(move |e: InputEvent| {
            if let Some(input) = e.target_dyn_into::<HtmlInputElement>() {
                on_allow_change.emit(input.value());
            }
        })
    };

    let on_deny_input = {
        let on_deny_change = props.on_deny_change.clone();
        Callback::from(move |e: InputEvent| {
            if let Some(input) = e.target_dyn_into::<HtmlInputElement>() {
                on_deny_change.emit(input.value());
            }
        })
    };

    html! {
        <div class="pt-2 border-t border-neutral-200 dark:border-neutral-750">
            <button
                type="button"
                onclick={on_toggle_click}
                class="text-xs text-neutral-500 hover:text-neutral-700 dark:hover:text-neutral-300 font-medium flex items-center gap-1"
            >
                <svg class={if **show_raw { "w-3 h-3 rotate-90 transition-transform" } else { "w-3 h-3 transition-transform" }} fill="none" stroke="currentColor" viewBox="0 0 24 24">
                    <path stroke-linecap="round" stroke-linejoin="round" stroke-width="2" d="M9 5l7 7-7 7" />
                </svg>
                { locale.t("geoip.custom_input_hint") }
            </button>

            if **show_raw {
                <div class="grid grid-cols-1 sm:grid-cols-2 gap-3 pt-2">
                    <div>
                        <label class="block text-xs font-medium text-neutral-600 dark:text-neutral-400 mb-1">
                            { locale.t("geoip.allow_label") }
                        </label>
                        <input
                            type="text"
                            value={props.allow_countries.clone()}
                            oninput={on_allow_input}
                            placeholder="TR, US, DE"
                            class="w-full text-xs px-2.5 py-1.5 rounded border border-neutral-300 dark:border-neutral-700 bg-white dark:bg-neutral-800 font-mono"
                        />
                    </div>
                    <div>
                        <label class="block text-xs font-medium text-neutral-600 dark:text-neutral-400 mb-1">
                            { locale.t("geoip.deny_label") }
                        </label>
                        <input
                            type="text"
                            value={props.deny_countries.clone()}
                            oninput={on_deny_input}
                            placeholder="RU, CN"
                            class="w-full text-xs px-2.5 py-1.5 rounded border border-neutral-300 dark:border-neutral-700 bg-white dark:bg-neutral-800 font-mono"
                        />
                    </div>
                </div>
            }
        </div>
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_determine_active_mode() {
        assert_eq!(determine_active_mode(&[], &[]), GeoIpMode::Off);
        assert_eq!(
            determine_active_mode(&["TR".to_string()], &[]),
            GeoIpMode::Allowlist
        );
        assert_eq!(
            determine_active_mode(&[], &["RU".to_string()]),
            GeoIpMode::Denylist
        );
        assert_eq!(
            determine_active_mode(&["TR".to_string()], &["RU".to_string()]),
            GeoIpMode::Advanced
        );
    }
}
