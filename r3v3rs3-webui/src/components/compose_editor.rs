use crate::components::compose_topology::ComposeTopologyGraph;
use r3v3rs3_api::i18n::Locale;
use r3v3rs3_api::platform::{parse_compose_services, parse_compose_topology};
use web_sys::HtmlTextAreaElement;
use yew::prelude::*;

const TEMPLATE_NGINX: &str = r#"services:
  web:
    image: nginx:alpine
    restart: unless-stopped
    ports:
      - "${PORT:-80}:80"
"#;

const TEMPLATE_NODE_POSTGRES: &str = r#"services:
  app:
    image: node:20-alpine
    restart: unless-stopped
    ports:
      - "${PORT:-3000}:3000"
    environment:
      - DATABASE_URL=${DATABASE_URL}
      - NODE_ENV=production
    depends_on:
      - db
  db:
    image: postgres:17-alpine
    restart: unless-stopped
    environment:
      - POSTGRES_PASSWORD=${POSTGRES_PASSWORD:-secret}
"#;

const TEMPLATE_PYTHON_REDIS: &str = r#"services:
  api:
    image: python:3.12-alpine
    restart: unless-stopped
    ports:
      - "${PORT:-8000}:8000"
    environment:
      - REDIS_URL=${REDIS_URL:-redis://cache:6379}
    depends_on:
      - cache
  cache:
    image: redis:alpine
    restart: unless-stopped
"#;

const TEMPLATE_WORDPRESS: &str = r#"services:
  wordpress:
    image: wordpress:latest
    restart: unless-stopped
    ports:
      - "${PORT:-80}:80"
    environment:
      - WORDPRESS_DB_HOST=db:3306
      - WORDPRESS_DB_PASSWORD=${DB_PASSWORD:-secret}
    depends_on:
      - db
  db:
    image: mysql:8.0
    restart: unless-stopped
    environment:
      - MYSQL_ROOT_PASSWORD=${DB_PASSWORD:-secret}
"#;

#[derive(Clone, Copy, PartialEq, Eq)]
enum EditorTab {
    Edit,
    Preview,
    Topology,
}

#[derive(Properties, PartialEq)]
pub struct ComposeEditorProps {
    pub value: String,
    pub onchange: Callback<String>,
    pub locale: Locale,
    #[prop_or_default]
    pub suggestions: Vec<String>,
    #[prop_or_default]
    pub onservice_selected: Option<Callback<String>>,
    #[prop_or_default]
    pub current_service: Option<String>,
}

#[function_component(ComposeEditor)]
pub fn compose_editor(props: &ComposeEditorProps) -> Html {
    let tab = use_state(|| EditorTab::Edit);
    let show_vars = use_state(|| false);
    let show_templates = use_state(|| false);

    let validation = use_memo(props.value.clone(), |val| {
        if val.trim().is_empty() {
            Ok(Vec::new())
        } else {
            parse_compose_services(val)
        }
    });

    let error_line = parse_error_line(validation.as_ref().as_ref().err().map(|s| s.as_str()));

    let oninput = {
        let onchange = props.onchange.clone();
        Callback::from(move |e: InputEvent| {
            if let Some(target) = e.target_dyn_into::<HtmlTextAreaElement>() {
                onchange.emit(target.value());
            }
        })
    };

    let on_apply_template = {
        let onchange = props.onchange.clone();
        let show_templates = show_templates.clone();
        Callback::from(move |content: &'static str| {
            onchange.emit(content.to_string());
            show_templates.set(false);
        })
    };

    let on_insert_var = {
        let value = props.value.clone();
        let onchange = props.onchange.clone();
        Callback::from(move |var: String| {
            onchange.emit(append_variable_token(&value, &var));
        })
    };

    html! {
        <div class="border border-neutral-700 rounded-lg overflow-hidden bg-neutral-900 text-neutral-100 flex flex-col">
            { render_toolbar(props.locale, &tab, &show_vars, &show_templates) }
            { render_templates_menu(props.locale, &show_templates, &on_apply_template) }
            { render_variables_bar(props.locale, &show_vars, &props.suggestions, &on_insert_var) }
            { render_validation_status(props.locale, validation.as_ref(), props.current_service.as_deref(), props.onservice_selected.as_ref()) }
            { render_tab_body(*tab, &props.value, error_line, &oninput, props.locale, props.current_service.as_deref()) }
        </div>
    }
}

fn append_variable_token(value: &str, var: &str) -> String {
    let insertion = format!("${{{var}}}");
    if value.ends_with(' ') || value.ends_with(':') || value.ends_with('\n') {
        format!("{value}{insertion}")
    } else {
        format!("{value} {insertion}")
    }
}

fn render_tab_body(
    tab: EditorTab,
    value: &str,
    error_line: Option<usize>,
    oninput: &Callback<InputEvent>,
    locale: Locale,
    current_service: Option<&str>,
) -> Html {
    match tab {
        EditorTab::Edit => html! {
            <div class="flex font-mono text-xs leading-6 overflow-x-auto bg-neutral-950">
                { render_gutter(value, error_line) }
                <textarea
                    class="flex-1 bg-transparent text-neutral-200 p-2 outline-none resize-y min-h-[280px] font-mono text-xs leading-6"
                    value={value.to_string()}
                    oninput={oninput.clone()}
                    placeholder={locale.t("compose.editor_title")}
                    spellcheck="false"
                />
            </div>
        },
        EditorTab::Topology => render_editor_topology(locale, value, current_service),
        EditorTab::Preview => html! {
            <div class="p-3 bg-neutral-950 min-h-[280px] overflow-x-auto font-mono text-xs leading-6">
                { render_preview(value) }
            </div>
        },
    }
}

fn render_editor_topology(locale: Locale, value: &str, current_service: Option<&str>) -> Html {
    match parse_compose_topology(value, None, current_service) {
        Ok(topo) => html! {
            <div class="p-3 bg-neutral-950 min-h-[280px]">
                <ComposeTopologyGraph topology={topo} {locale} />
            </div>
        },
        Err(err) => html! {
            <div class="p-6 bg-neutral-950 min-h-[280px] flex items-center justify-center text-center">
                <div class="text-xs text-rose-400 font-mono">
                    <span class="font-bold">{locale.t("compose.syntax_error")}{": "}</span>
                    {err}
                </div>
            </div>
        },
    }
}

fn parse_error_line(err: Option<&str>) -> Option<usize> {
    let err = err?;
    let rest = err.strip_prefix("Line ")?;
    let (num_str, _) = rest.split_once(':')?;
    num_str.parse().ok()
}

fn render_toolbar(
    locale: Locale,
    tab: &UseStateHandle<EditorTab>,
    show_vars: &UseStateHandle<bool>,
    show_templates: &UseStateHandle<bool>,
) -> Html {
    let set_edit = {
        let tab = tab.clone();
        Callback::from(move |_| tab.set(EditorTab::Edit))
    };
    let set_preview = {
        let tab = tab.clone();
        Callback::from(move |_| tab.set(EditorTab::Preview))
    };
    let set_topology = {
        let tab = tab.clone();
        Callback::from(move |_| tab.set(EditorTab::Topology))
    };
    let toggle_vars = {
        let show_vars = show_vars.clone();
        Callback::from(move |_| show_vars.set(!*show_vars))
    };
    let toggle_templates = {
        let show_templates = show_templates.clone();
        Callback::from(move |_| show_templates.set(!*show_templates))
    };

    let edit_class = tab_button_class(**tab == EditorTab::Edit);
    let preview_class = tab_button_class(**tab == EditorTab::Preview);
    let topology_class = tab_button_class(**tab == EditorTab::Topology);

    html! {
        <div class="flex items-center justify-between px-3 py-2 bg-neutral-900 border-b border-neutral-700/80 flex-wrap gap-2">
            <div class="flex items-center gap-1.5">
                <span class="text-xs font-semibold text-neutral-300 mr-2 flex items-center gap-1">
                    <svg class="w-3.5 h-3.5 text-primary-400" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2">
                        <path d="M14 2H6a2 2 0 0 0-2 2v16a2 2 0 0 0 2 2h12a2 2 0 0 0 2-2V8z"></path>
                        <polyline points="14 2 14 8 20 8"></polyline>
                        <line x1="16" y1="13" x2="8" y2="13"></line>
                        <line x1="16" y1="17" x2="8" y2="17"></line>
                    </svg>
                    {locale.t("compose.editor_title")}
                </span>
                <div class="inline-flex rounded-md p-0.5 bg-neutral-900 border border-neutral-700">
                    <button type="button" class={edit_class} onclick={set_edit}>
                        {locale.t("compose.tab_edit")}
                    </button>
                    <button type="button" class={topology_class} onclick={set_topology}>
                        {locale.t("compose.tab_topology")}
                    </button>
                    <button type="button" class={preview_class} onclick={set_preview}>
                        {locale.t("compose.tab_preview")}
                    </button>
                </div>
            </div>
            <div class="flex items-center gap-2">
                <button
                    type="button"
                    class="px-2.5 py-1 text-xs font-medium rounded border border-neutral-700 bg-neutral-800 hover:bg-neutral-700 text-neutral-200 flex items-center gap-1"
                    onclick={toggle_vars}
                >
                    <span class="text-amber-400 font-bold">{"${}"}</span>
                    {locale.t("compose.insert_var")}
                </button>
                <button
                    type="button"
                    class="px-2.5 py-1 text-xs font-medium rounded border border-neutral-700 bg-neutral-800 hover:bg-neutral-700 text-neutral-200 flex items-center gap-1"
                    onclick={toggle_templates}
                >
                    <svg class="w-3.5 h-3.5 text-neutral-400" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2">
                        <rect x="3" y="3" width="18" height="18" rx="2" ry="2"></rect>
                        <line x1="3" y1="9" x2="21" y2="9"></line>
                        <line x1="9" y1="21" x2="9" y2="9"></line>
                    </svg>
                    {locale.t("compose.templates")}
                </button>
            </div>
        </div>
    }
}

fn render_templates_menu(
    locale: Locale,
    show: &UseStateHandle<bool>,
    on_apply: &Callback<&'static str>,
) -> Html {
    if !**show {
        return html! {};
    }

    let templates = [
        ("compose.template_nginx", TEMPLATE_NGINX),
        ("compose.template_node_postgres", TEMPLATE_NODE_POSTGRES),
        ("compose.template_python_redis", TEMPLATE_PYTHON_REDIS),
        ("compose.template_wordpress", TEMPLATE_WORDPRESS),
    ];

    html! {
        <div class="p-2.5 bg-neutral-800 border-b border-neutral-700 flex flex-wrap gap-2 items-center">
            <span class="text-xs text-neutral-400 font-medium mr-1">{locale.t("compose.templates")}{":"}</span>
            { for templates.iter().map(|(label, content)| {
                let on_apply = on_apply.clone();
                let c = *content;
                let onclick = Callback::from(move |_| on_apply.emit(c));
                html! {
                    <button
                        type="button"
                        class="px-2 py-1 text-xs rounded bg-neutral-900 border border-neutral-700 hover:border-primary-500 hover:text-primary-300 text-neutral-300 transition-colors"
                        {onclick}
                    >
                        {locale.t(label)}
                    </button>
                }
            }) }
        </div>
    }
}

fn render_variables_bar(
    locale: Locale,
    show: &UseStateHandle<bool>,
    suggestions: &[String],
    on_insert: &Callback<String>,
) -> Html {
    if !**show {
        return html! {};
    }

    let default_vars = [
        "PORT",
        "DATABASE_URL",
        "POSTGRES_PASSWORD",
        "REDIS_URL",
        "NODE_ENV",
        "SECRET_KEY",
    ];

    let mut all_vars = suggestions.to_vec();
    for d in default_vars {
        if !all_vars.contains(&d.to_string()) {
            all_vars.push(d.to_string());
        }
    }

    html! {
        <div class="p-2.5 bg-neutral-800 border-b border-neutral-700 flex flex-wrap gap-1.5 items-center">
            <span class="text-xs text-neutral-400 font-medium mr-1">{locale.t("compose.variables_available")}</span>
            { for all_vars.into_iter().map(|var| {
                let on_insert = on_insert.clone();
                let v = var.clone();
                let onclick = Callback::from(move |_| on_insert.emit(v.clone()));
                html! {
                    <button
                        type="button"
                        class="px-1.5 py-0.5 text-xs font-mono rounded bg-neutral-900 border border-amber-800/60 text-amber-300 hover:bg-amber-950/60 transition-colors"
                        {onclick}
                    >
                        {format!("${{{var}}}")}
                    </button>
                }
            }) }
        </div>
    }
}

fn render_validation_status(
    locale: Locale,
    status: &Result<Vec<String>, String>,
    current_service: Option<&str>,
    onservice_selected: Option<&Callback<String>>,
) -> Html {
    match status {
        Ok(services) => {
            html! {
                <div class="px-3 py-1.5 bg-emerald-950/40 border-b border-emerald-800/40 text-emerald-400 text-xs flex items-center justify-between flex-wrap gap-2">
                    <div class="flex items-center gap-1.5">
                        <svg class="w-3.5 h-3.5 text-emerald-400" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2">
                            <polyline points="20 6 9 17 4 12"></polyline>
                        </svg>
                        <span class="font-medium">{locale.t("compose.syntax_valid")}</span>
                    </div>
                    { render_services_list(locale, services, current_service, onservice_selected) }
                </div>
            }
        }
        Err(err) => {
            html! {
                <div class="px-3 py-1.5 bg-rose-950/50 border-b border-rose-800/50 text-rose-300 text-xs flex items-center gap-1.5">
                    <svg class="w-3.5 h-3.5 text-rose-400 flex-shrink-0" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2">
                        <circle cx="12" cy="12" r="10"></circle>
                        <line x1="12" y1="8" x2="12" y2="12"></line>
                        <line x1="12" y1="16" x2="12.01" y2="16"></line>
                    </svg>
                    <span class="font-semibold">{locale.t("compose.syntax_error")}{":"}</span>
                    <span class="font-mono">{err}</span>
                </div>
            }
        }
    }
}

fn render_services_list(
    locale: Locale,
    services: &[String],
    current_service: Option<&str>,
    onservice_selected: Option<&Callback<String>>,
) -> Html {
    if services.is_empty() {
        return html! {
            <span class="text-neutral-500 text-xs italic">{locale.t("compose.no_services")}</span>
        };
    }

    html! {
        <div class="flex items-center gap-1.5 flex-wrap">
            <span class="text-xs text-neutral-400 mr-1">{locale.t("compose.detected_services")}{":"}</span>
            { for services.iter().map(|svc| {
                let is_selected = current_service == Some(svc.as_str());
                let pill_class = if is_selected {
                    "px-2 py-0.5 text-xs font-mono rounded bg-primary-600 text-white font-medium ring-1 ring-primary-400"
                } else {
                    "px-2 py-0.5 text-xs font-mono rounded bg-neutral-800 text-neutral-300 hover:bg-neutral-700"
                };

                let onclick = onservice_selected.map(|cb| {
                    let cb = cb.clone();
                    let s = svc.clone();
                    Callback::from(move |_| cb.emit(s.clone()))
                });

                html! {
                    <button
                        type="button"
                        class={pill_class}
                        title={locale.t("compose.routing_service_hint")}
                        {onclick}
                    >
                        {svc}
                    </button>
                }
            }) }
        </div>
    }
}

fn render_gutter(value: &str, error_line: Option<usize>) -> Html {
    let line_count = value.lines().count().max(1);
    html! {
        <div class="select-none text-right text-neutral-500 py-2 pl-3 pr-2 bg-neutral-900 border-r border-neutral-800 w-10">
            { for (1..=line_count).map(|num| {
                let is_err = error_line == Some(num);
                let num_class = if is_err {
                    "text-rose-400 font-bold bg-rose-950/60 block px-0.5 rounded"
                } else {
                    "block"
                };
                html! {
                    <span class={num_class}>{num}</span>
                }
            }) }
        </div>
    }
}

fn render_preview(value: &str) -> Html {
    html! {
        <div class="flex flex-col">
            { for value.lines().enumerate().map(|(idx, line)| {
                html! {
                    <div class="flex leading-6 hover:bg-neutral-900/60">
                        <span class="w-10 text-right pr-3 select-none text-neutral-600 font-mono text-xs">{idx + 1}</span>
                        <div class="flex-1 font-mono text-xs whitespace-pre">{ render_preview_line(line) }</div>
                    </div>
                }
            }) }
        </div>
    }
}

fn render_preview_line(line: &str) -> Html {
    let trimmed = line.trim_start();
    if trimmed.starts_with('#') {
        return html! { <span class="text-neutral-500 italic">{line}</span> };
    }

    if let Some((key, val)) = line.split_once(':') {
        let is_top_key = !line.starts_with("  ") && !line.is_empty();
        let key_class = if is_top_key {
            "text-purple-400 font-bold"
        } else {
            "text-sky-300"
        };

        html! {
            <>
                <span class={key_class}>{key}</span>
                <span class="text-neutral-400">{":"}</span>
                { render_tokens_with_vars(val) }
            </>
        }
    } else {
        render_tokens_with_vars(line)
    }
}

fn render_tokens_with_vars(text: &str) -> Html {
    if !text.contains("${") {
        return html! { <span class="text-neutral-300">{text}</span> };
    }

    let mut parts = Vec::new();
    let mut rest = text;

    while let Some(start) = rest.find("${") {
        if start > 0 {
            parts.push(html! { <span class="text-neutral-300">{&rest[..start]}</span> });
        }
        let after_start = &rest[start..];
        if let Some(end) = after_start.find('}') {
            let var_token = &after_start[..=end];
            parts.push(html! {
                <span class="text-amber-300 bg-amber-950/60 font-semibold px-1 rounded border border-amber-800/40">
                    {var_token}
                </span>
            });
            rest = &after_start[end + 1..];
        } else {
            parts.push(html! { <span class="text-amber-300">{after_start}</span> });
            rest = "";
            break;
        }
    }

    if !rest.is_empty() {
        parts.push(html! { <span class="text-neutral-300">{rest}</span> });
    }

    html! { <>{ for parts }</> }
}

fn tab_button_class(active: bool) -> &'static str {
    if active {
        "px-2.5 py-1 text-xs font-medium rounded bg-neutral-800 text-white shadow-sm"
    } else {
        "px-2.5 py-1 text-xs font-medium text-neutral-400 hover:text-white"
    }
}
