//! Interactive gRPC Server Reflection and Call Tester component.

use crate::API_ENDPOINT;
use crate::components::http_proxy_config::{HINT_CLASS, INPUT_CLASS};
use crate::i18n::use_locale;
use gloo_net::http::Request;
use r3v3rs3_api::grpc::{
    GrpcInvokeRequest, GrpcInvokeResponse, GrpcReflectRequest, GrpcReflectResponse,
    GrpcServiceInfo, grpc_status_name,
};
use r3v3rs3_api::i18n::Locale;
use r3v3rs3_api::id::ShortId;
use std::collections::HashMap;
use wasm_bindgen::{JsCast, UnwrapThrowExt};
use web_sys::{HtmlInputElement, HtmlSelectElement, HtmlTextAreaElement};
use yew::prelude::*;

#[derive(Properties, PartialEq)]
pub struct Props {
    #[prop_or_default]
    pub proxy_id: Option<ShortId>,
    #[prop_or_default]
    pub default_target: Option<String>,
}

pub fn parse_metadata_lines(input: &str) -> HashMap<String, String> {
    let mut map = HashMap::new();
    for line in input.lines() {
        let trimmed = line.trim();
        if let Some((k, v)) = trimmed.split_once(':') {
            let key = k.trim().to_string();
            let val = v.trim().to_string();
            if !key.is_empty() {
                map.insert(key, val);
            }
        }
    }
    map
}

pub fn format_status_badge(code: u32) -> (&'static str, &'static str) {
    if code == 0 {
        (
            "bg-green-100 text-green-800 dark:bg-green-900/60 dark:text-green-300",
            "OK (0)",
        )
    } else {
        (
            "bg-red-100 text-red-800 dark:bg-red-900/60 dark:text-red-300",
            grpc_status_name(code),
        )
    }
}

fn make_input_callback(state: &UseStateHandle<String>) -> Callback<Event> {
    let state = state.clone();
    Callback::from(move |e: Event| {
        let input: HtmlInputElement = e.target().unwrap_throw().dyn_into().unwrap_throw();
        state.set(input.value());
    })
}

fn make_textarea_callback(state: &UseStateHandle<String>) -> Callback<Event> {
    let state = state.clone();
    Callback::from(move |e: Event| {
        let area: HtmlTextAreaElement = e.target().unwrap_throw().dyn_into().unwrap_throw();
        state.set(area.value());
    })
}

fn toggle_state(state: &UseStateHandle<bool>) -> Callback<MouseEvent> {
    let state = state.clone();
    Callback::from(move |_| state.set(!*state))
}

fn default_json() -> String {
    "{}".to_string()
}

struct DiscoverHandles<'a> {
    proxy_id: Option<ShortId>,
    target_url: &'a UseStateHandle<String>,
    is_reflecting: &'a UseStateHandle<bool>,
    services: &'a UseStateHandle<Vec<GrpcServiceInfo>>,
    selected_service: &'a UseStateHandle<Option<String>>,
    selected_method: &'a UseStateHandle<Option<String>>,
    reflect_error: &'a UseStateHandle<Option<String>>,
}

fn make_discover_callback(h: DiscoverHandles<'_>) -> Callback<MouseEvent> {
    let proxy_id = h.proxy_id;
    let target_url = (**h.target_url).clone();
    let is_reflecting = h.is_reflecting.clone();
    let services = h.services.clone();
    let selected_service = h.selected_service.clone();
    let selected_method = h.selected_method.clone();
    let reflect_error = h.reflect_error.clone();

    Callback::from(move |_| {
        let target_url = target_url.clone();
        let is_reflecting = is_reflecting.clone();
        let services = services.clone();
        let selected_service = selected_service.clone();
        let selected_method = selected_method.clone();
        let reflect_error = reflect_error.clone();

        is_reflecting.set(true);
        reflect_error.set(None);
        wasm_bindgen_futures::spawn_local(async move {
            execute_discover(
                proxy_id,
                target_url,
                services,
                selected_service,
                selected_method,
                reflect_error,
                is_reflecting,
            )
            .await;
        });
    })
}

struct InvokeHandles<'a> {
    proxy_id: Option<ShortId>,
    target_url: &'a UseStateHandle<String>,
    selected_service: &'a UseStateHandle<Option<String>>,
    selected_method: &'a UseStateHandle<Option<String>>,
    payload: &'a UseStateHandle<String>,
    metadata: &'a UseStateHandle<String>,
    is_invoking: &'a UseStateHandle<bool>,
    invoke_result: &'a UseStateHandle<Option<Result<GrpcInvokeResponse, String>>>,
}

fn make_invoke_callback(h: InvokeHandles<'_>) -> Callback<MouseEvent> {
    let proxy_id = h.proxy_id;
    let target_url = (**h.target_url).clone();
    let service = (**h.selected_service).clone();
    let method = (**h.selected_method).clone();
    let payload = (**h.payload).clone();
    let metadata = (**h.metadata).clone();
    let is_invoking = h.is_invoking.clone();
    let invoke_result = h.invoke_result.clone();

    Callback::from(move |_| {
        let Some(service) = service.clone() else {
            return;
        };
        let Some(method) = method.clone() else { return };

        let target = if target_url.trim().is_empty() {
            None
        } else {
            Some(target_url.trim().to_string())
        };

        let parsed_payload = serde_json::from_str(&payload).unwrap_or(serde_json::Value::Null);

        let req_body = GrpcInvokeRequest {
            target_url: target,
            service,
            method,
            payload: parsed_payload,
            metadata: parse_metadata_lines(&metadata),
        };

        let is_invoking = is_invoking.clone();
        let invoke_result = invoke_result.clone();
        is_invoking.set(true);
        invoke_result.set(None);

        wasm_bindgen_futures::spawn_local(async move {
            execute_invoke(proxy_id, req_body, invoke_result, is_invoking).await;
        });
    })
}

#[function_component(GrpcTester)]
pub fn grpc_tester(props: &Props) -> Html {
    let locale = use_locale();
    let is_open = use_state(bool::default);
    let target_str = props.default_target.clone().unwrap_or_default();
    let target_url = use_state(move || target_str);
    let is_reflecting = use_state(bool::default);
    let is_invoking = use_state(bool::default);
    let services = use_state(Vec::<GrpcServiceInfo>::new);
    let selected_service = use_state(Option::<String>::default);
    let selected_method = use_state(Option::<String>::default);
    let payload = use_state(default_json);
    let metadata = use_state(String::new);
    let reflect_error = use_state(Option::<String>::default);
    let invoke_result = use_state(Option::<Result<GrpcInvokeResponse, String>>::default);

    let toggle_open = toggle_state(&is_open);
    let on_target_change = make_input_callback(&target_url);
    let on_payload_change = make_textarea_callback(&payload);
    let on_metadata_change = make_textarea_callback(&metadata);

    let on_discover = make_discover_callback(DiscoverHandles {
        proxy_id: props.proxy_id,
        target_url: &target_url,
        is_reflecting: &is_reflecting,
        services: &services,
        selected_service: &selected_service,
        selected_method: &selected_method,
        reflect_error: &reflect_error,
    });

    let on_invoke = make_invoke_callback(InvokeHandles {
        proxy_id: props.proxy_id,
        target_url: &target_url,
        selected_service: &selected_service,
        selected_method: &selected_method,
        payload: &payload,
        metadata: &metadata,
        is_invoking: &is_invoking,
        invoke_result: &invoke_result,
    });

    html! {
        <div class="mt-8 border border-neutral-300 dark:border-neutral-700 rounded-lg bg-neutral-50 dark:bg-neutral-800/60 p-5">
            { header_view(locale, *is_open, toggle_open) }
            if *is_open {
                { content_panel_view(ContentPanelProps {
                    locale,
                    target_url: &target_url,
                    on_target_change,
                    is_reflecting: *is_reflecting,
                    on_discover,
                    reflect_error: &reflect_error,
                    services: &services,
                    selected_service: &selected_service,
                    selected_method: &selected_method,
                    payload: &payload,
                    on_payload_change,
                    metadata: &metadata,
                    on_metadata_change,
                    is_invoking: *is_invoking,
                    on_invoke,
                    invoke_result: &invoke_result,
                }) }
            }
        </div>
    }
}

struct ContentPanelProps<'a> {
    locale: Locale,
    target_url: &'a UseStateHandle<String>,
    on_target_change: Callback<Event>,
    is_reflecting: bool,
    on_discover: Callback<MouseEvent>,
    reflect_error: &'a Option<String>,
    services: &'a UseStateHandle<Vec<GrpcServiceInfo>>,
    selected_service: &'a UseStateHandle<Option<String>>,
    selected_method: &'a UseStateHandle<Option<String>>,
    payload: &'a UseStateHandle<String>,
    on_payload_change: Callback<Event>,
    metadata: &'a UseStateHandle<String>,
    on_metadata_change: Callback<Event>,
    is_invoking: bool,
    on_invoke: Callback<MouseEvent>,
    invoke_result: &'a Option<Result<GrpcInvokeResponse, String>>,
}

fn content_panel_view(p: ContentPanelProps<'_>) -> Html {
    html! {
        <div class="mt-4 pt-4 border-t border-neutral-200 dark:border-neutral-700 space-y-4">
            <p class="text-xs text-neutral-500 dark:text-neutral-400">
                {p.locale.t("grpc.tester_desc")}
            </p>
            { target_input_view(p.locale, p.target_url, p.on_target_change, p.is_reflecting, p.on_discover) }
            if let Some(err) = p.reflect_error {
                <div class="p-3 bg-red-50 dark:bg-red-950/40 border border-red-200 dark:border-red-800 rounded-md text-xs text-red-700 dark:text-red-300">
                    {err}
                </div>
            }
            { service_method_view(p.locale, p.services, p.selected_service, p.selected_method) }
            { payload_view(p.locale, p.payload, p.on_payload_change, p.metadata, p.on_metadata_change) }
            { invoke_button_view(p.locale, p.is_invoking, p.selected_method.is_some(), p.on_invoke) }
            if let Some(res) = p.invoke_result {
                { invoke_result_view(p.locale, res) }
            }
        </div>
    }
}

async fn execute_discover(
    proxy_id: Option<ShortId>,
    target_url: String,
    services: UseStateHandle<Vec<GrpcServiceInfo>>,
    selected_service: UseStateHandle<Option<String>>,
    selected_method: UseStateHandle<Option<String>>,
    reflect_error: UseStateHandle<Option<String>>,
    is_reflecting: UseStateHandle<bool>,
) {
    let target = if target_url.trim().is_empty() {
        None
    } else {
        Some(target_url.trim().to_string())
    };

    let url = match proxy_id {
        Some(id) => format!("{API_ENDPOINT}/proxies/{id}/grpc/reflect"),
        None => format!("{API_ENDPOINT}/grpc/reflect"),
    };

    let req = Request::post(&url).json(&GrpcReflectRequest {
        target_url: target,
        service_name: None,
    });

    match req {
        Ok(builder) => match builder.send().await {
            Ok(resp) if resp.ok() => match resp.json::<GrpcReflectResponse>().await {
                Ok(data) => {
                    let first_svc = data.services.first().map(|s| s.name.clone());
                    let first_method = data
                        .services
                        .first()
                        .and_then(|s| s.methods.first())
                        .map(|m| m.name.clone());
                    selected_service.set(first_svc);
                    selected_method.set(first_method);
                    services.set(data.services);
                }
                Err(err) => reflect_error.set(Some(err.to_string())),
            },
            Ok(resp) => {
                let text = resp.text().await.unwrap_or_else(|_| "Error".to_string());
                reflect_error.set(Some(text));
            }
            Err(err) => reflect_error.set(Some(err.to_string())),
        },
        Err(err) => reflect_error.set(Some(err.to_string())),
    }
    is_reflecting.set(false);
}

async fn execute_invoke(
    proxy_id: Option<ShortId>,
    req_body: GrpcInvokeRequest,
    invoke_result: UseStateHandle<Option<Result<GrpcInvokeResponse, String>>>,
    is_invoking: UseStateHandle<bool>,
) {
    let url = match proxy_id {
        Some(id) => format!("{API_ENDPOINT}/proxies/{id}/grpc/invoke"),
        None => format!("{API_ENDPOINT}/grpc/invoke"),
    };

    let req = Request::post(&url).json(&req_body);
    match req {
        Ok(builder) => match builder.send().await {
            Ok(resp) if resp.ok() => match resp.json::<GrpcInvokeResponse>().await {
                Ok(data) => invoke_result.set(Some(Ok(data))),
                Err(err) => invoke_result.set(Some(Err(err.to_string()))),
            },
            Ok(resp) => {
                let text = resp.text().await.unwrap_or_else(|_| "Error".to_string());
                invoke_result.set(Some(Err(text)));
            }
            Err(err) => invoke_result.set(Some(Err(err.to_string()))),
        },
        Err(err) => invoke_result.set(Some(Err(err.to_string()))),
    }
    is_invoking.set(false);
}

fn header_view(locale: Locale, is_open: bool, toggle_open: Callback<MouseEvent>) -> Html {
    html! {
        <div class="flex items-center justify-between cursor-pointer select-none" onclick={toggle_open}>
            <div class="flex items-center gap-2">
                <svg class="w-5 h-5 text-indigo-600 dark:text-indigo-400" fill="none" stroke="currentColor" viewBox="0 0 24 24">
                    <path stroke-linecap="round" stroke-linejoin="round" stroke-width="2" d="M8 9l3 3-3 3m5 0h3M5 20h14a2 2 0 002-2V6a2 2 0 00-2-2H5a2 2 0 00-2 2v12a2 2 0 002 2z" />
                </svg>
                <span class="text-sm font-semibold text-neutral-900 dark:text-neutral-100">
                    {locale.t("grpc.tester_title")}
                </span>
                <span class="text-xs px-2 py-0.5 rounded-full bg-indigo-100 text-indigo-800 dark:bg-indigo-900/60 dark:text-indigo-300">
                    {"gRPC Reflection"}
                </span>
            </div>
            <svg class={classes!("w-5", "h-5", "text-neutral-500", "transition-transform", is_open.then_some("rotate-180"))} fill="none" stroke="currentColor" viewBox="0 0 24 24">
                <path stroke-linecap="round" stroke-linejoin="round" stroke-width="2" d="M19 9l-7 7-7-7" />
            </svg>
        </div>
    }
}

fn target_input_view(
    locale: Locale,
    target_url: &UseStateHandle<String>,
    on_change: Callback<Event>,
    is_reflecting: bool,
    on_discover: Callback<MouseEvent>,
) -> Html {
    html! {
        <div class="space-y-1">
            <label class="block text-xs font-medium text-neutral-700 dark:text-neutral-300">
                {locale.t("grpc.target_url")}
            </label>
            <div class="flex gap-2">
                <input
                    type="text"
                    value={(**target_url).clone()}
                    onchange={on_change}
                    placeholder="http://127.0.0.1:50051"
                    class={INPUT_CLASS}
                />
                <button
                    type="button"
                    onclick={on_discover}
                    disabled={is_reflecting}
                    class="px-4 py-2 bg-indigo-600 hover:bg-indigo-700 disabled:opacity-50 text-white rounded-lg text-sm font-medium transition-colors shrink-0 flex items-center gap-2"
                >
                    if is_reflecting {
                        <span class="animate-spin inline-block w-4 h-4 border-2 border-white border-t-transparent rounded-full"></span>
                        {locale.t("grpc.discovering")}
                    } else {
                        {locale.t("grpc.discover_button")}
                    }
                </button>
            </div>
            <p class={HINT_CLASS}>{locale.t("grpc.target_url_hint")}</p>
        </div>
    }
}

fn service_method_view(
    locale: Locale,
    services: &UseStateHandle<Vec<GrpcServiceInfo>>,
    selected_service: &UseStateHandle<Option<String>>,
    selected_method: &UseStateHandle<Option<String>>,
) -> Html {
    if services.is_empty() {
        return html! {};
    }

    let on_service_change = {
        let services = services.clone();
        let selected_service = selected_service.clone();
        let selected_method = selected_method.clone();
        Callback::from(move |e: Event| {
            let sel: HtmlSelectElement = e.target().unwrap_throw().dyn_into().unwrap_throw();
            let val = sel.value();
            if val.is_empty() {
                selected_service.set(None);
                selected_method.set(None);
            } else {
                let first_method = services
                    .iter()
                    .find(|s| s.name == val)
                    .and_then(|s| s.methods.first())
                    .map(|m| m.name.clone());
                selected_service.set(Some(val));
                selected_method.set(first_method);
            }
        })
    };

    let on_method_change = {
        let selected_method = selected_method.clone();
        Callback::from(move |e: Event| {
            let sel: HtmlSelectElement = e.target().unwrap_throw().dyn_into().unwrap_throw();
            let val = sel.value();
            if val.is_empty() {
                selected_method.set(None);
            } else {
                selected_method.set(Some(val));
            }
        })
    };

    let current_service_obj = services
        .iter()
        .find(|s| selected_service.as_ref().is_some_and(|cur| cur == &s.name));

    let current_method_obj = current_service_obj.and_then(|s| {
        s.methods
            .iter()
            .find(|m| selected_method.as_ref().is_some_and(|cur| cur == &m.name))
    });

    html! {
        <div class="space-y-3 p-3 bg-white dark:bg-neutral-900 border border-neutral-200 dark:border-neutral-700 rounded-lg">
            <div class="grid grid-cols-1 sm:grid-cols-2 gap-4">
                <div>
                    <label class="block mb-1 text-xs font-medium text-neutral-700 dark:text-neutral-300">
                        {locale.t("grpc.service")}
                    </label>
                    <select onchange={on_service_change} class={INPUT_CLASS}>
                        { services.iter().map(|s| html! {
                            <option value={s.name.clone()} selected={selected_service.as_ref().is_some_and(|cur| cur == &s.name)}>
                                {&s.name}
                            </option>
                        }).collect::<Html>() }
                    </select>
                </div>
                <div>
                    <label class="block mb-1 text-xs font-medium text-neutral-700 dark:text-neutral-300">
                        {locale.t("grpc.method")}
                    </label>
                    <select onchange={on_method_change} class={INPUT_CLASS}>
                        if let Some(svc) = current_service_obj {
                            { svc.methods.iter().map(|m| html! {
                                <option value={m.name.clone()} selected={selected_method.as_ref().is_some_and(|cur| cur == &m.name)}>
                                    {&m.name}
                                </option>
                            }).collect::<Html>() }
                        }
                    </select>
                </div>
            </div>
            if let Some(m) = current_method_obj {
                <div class="flex items-center gap-4 text-xs text-neutral-600 dark:text-neutral-400 pt-2 border-t border-neutral-100 dark:border-neutral-800">
                    <div>
                        <span class="font-semibold text-neutral-700 dark:text-neutral-300">{locale.t("grpc.input_type")}{": "}</span>
                        <code>{&m.input_type}</code>
                    </div>
                    <div>
                        <span class="font-semibold text-neutral-700 dark:text-neutral-300">{locale.t("grpc.output_type")}{": "}</span>
                        <code>{&m.output_type}</code>
                    </div>
                </div>
            }
        </div>
    }
}

fn payload_view(
    locale: Locale,
    payload: &UseStateHandle<String>,
    on_payload_change: Callback<Event>,
    metadata: &UseStateHandle<String>,
    on_metadata_change: Callback<Event>,
) -> Html {
    html! {
        <div class="grid grid-cols-1 sm:grid-cols-2 gap-4">
            <div>
                <label class="block mb-1 text-xs font-medium text-neutral-700 dark:text-neutral-300">
                    {locale.t("grpc.request_payload")}
                </label>
                <textarea
                    rows="6"
                    value={(**payload).clone()}
                    onchange={on_payload_change}
                    class={format!("{INPUT_CLASS} font-mono text-xs")}
                />
            </div>
            <div>
                <label class="block mb-1 text-xs font-medium text-neutral-700 dark:text-neutral-300">
                    {locale.t("grpc.metadata")}
                </label>
                <textarea
                    rows="6"
                    value={(**metadata).clone()}
                    onchange={on_metadata_change}
                    placeholder={"authorization: Bearer token\nx-client-id: test"}
                    class={format!("{INPUT_CLASS} font-mono text-xs")}
                />
                <p class={HINT_CLASS}>{locale.t("grpc.metadata_hint")}</p>
            </div>
        </div>
    }
}

fn invoke_button_view(
    locale: Locale,
    is_invoking: bool,
    can_invoke: bool,
    on_invoke: Callback<MouseEvent>,
) -> Html {
    html! {
        <div>
            <button
                type="button"
                onclick={on_invoke}
                disabled={is_invoking || !can_invoke}
                class="px-5 py-2.5 bg-emerald-600 hover:bg-emerald-700 disabled:opacity-50 text-white rounded-lg text-sm font-medium transition-colors flex items-center gap-2"
            >
                if is_invoking {
                    <span class="animate-spin inline-block w-4 h-4 border-2 border-white border-t-transparent rounded-full"></span>
                    {locale.t("grpc.invoking")}
                } else {
                    <svg class="w-4 h-4" fill="none" stroke="currentColor" viewBox="0 0 24 24">
                        <path stroke-linecap="round" stroke-linejoin="round" stroke-width="2" d="M14 5l7 7m0 0l-7 7m7-7H3" />
                    </svg>
                    {locale.t("grpc.invoke_button")}
                }
            </button>
        </div>
    }
}

fn invoke_result_view(locale: Locale, res: &Result<GrpcInvokeResponse, String>) -> Html {
    match res {
        Ok(data) => {
            let (badge_class, badge_text) = format_status_badge(data.grpc_status);
            let pretty_json = serde_json::to_string_pretty(&data.payload)
                .unwrap_or_else(|_| data.payload.to_string());
            html! {
                <div class="mt-4 p-4 rounded-lg border border-neutral-200 dark:border-neutral-700 bg-white dark:bg-neutral-900 space-y-3">
                    <div class="flex items-center justify-between">
                        <div class="flex items-center gap-2">
                            <span class="text-xs font-medium text-neutral-700 dark:text-neutral-300">{locale.t("grpc.status")}{": "}</span>
                            <span class={classes!("text-xs", "font-semibold", "px-2.5", "py-0.5", "rounded-full", badge_class)}>
                                {badge_text}
                            </span>
                            if let Some(msg) = &data.grpc_message {
                                <span class="text-xs text-neutral-500 italic">
                                    {format!("\"{}\"", msg)}
                                </span>
                            }
                        </div>
                        <div class="text-xs text-neutral-500 dark:text-neutral-400">
                            {format!("{}: {} ms", locale.t("grpc.latency"), data.latency_ms)}
                        </div>
                    </div>
                    <div>
                        <label class="block mb-1 text-xs font-medium text-neutral-700 dark:text-neutral-300">
                            {locale.t("grpc.response")}
                        </label>
                        <pre class="p-3 bg-neutral-900 text-neutral-100 rounded text-xs font-mono overflow-x-auto max-h-80">
                            {pretty_json}
                        </pre>
                    </div>
                    { headers_trailers_view(locale, &data.headers, &data.trailers) }
                </div>
            }
        }
        Err(err) => html! {
            <div class="mt-4 p-3 rounded-lg border border-red-200 dark:border-red-800 bg-red-50 dark:bg-red-950/40 text-xs text-red-700 dark:text-red-300 font-mono">
                {err}
            </div>
        },
    }
}

fn headers_trailers_view(
    locale: Locale,
    headers: &HashMap<String, String>,
    trailers: &HashMap<String, String>,
) -> Html {
    html! {
        <div class="grid grid-cols-1 sm:grid-cols-2 gap-3 text-xs pt-2 border-t border-neutral-100 dark:border-neutral-800">
            <div>
                <span class="font-medium text-neutral-700 dark:text-neutral-300 block mb-1">
                    {locale.t("grpc.response_headers")}
                </span>
                <div class="space-y-1 font-mono text-[11px] text-neutral-600 dark:text-neutral-400 bg-neutral-50 dark:bg-neutral-800/40 p-2 rounded max-h-32 overflow-y-auto">
                    { headers.iter().map(|(k, v)| html! {
                        <div><span class="text-indigo-600 dark:text-indigo-400">{k}</span>{": "}{v}</div>
                    }).collect::<Html>() }
                </div>
            </div>
            <div>
                <span class="font-medium text-neutral-700 dark:text-neutral-300 block mb-1">
                    {locale.t("grpc.response_trailers")}
                </span>
                <div class="space-y-1 font-mono text-[11px] text-neutral-600 dark:text-neutral-400 bg-neutral-50 dark:bg-neutral-800/40 p-2 rounded max-h-32 overflow-y-auto">
                    { trailers.iter().map(|(k, v)| html! {
                        <div><span class="text-emerald-600 dark:text-emerald-400">{k}</span>{": "}{v}</div>
                    }).collect::<Html>() }
                </div>
            </div>
        </div>
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_metadata_lines() {
        let raw = "authorization: Bearer mytoken\ncontent-type: application/grpc\ninvalidline\n  custom-header : custom-value  \n";
        let parsed = parse_metadata_lines(raw);
        assert_eq!(
            parsed.get("authorization").map(|s| s.as_str()),
            Some("Bearer mytoken")
        );
        assert_eq!(
            parsed.get("content-type").map(|s| s.as_str()),
            Some("application/grpc")
        );
        assert_eq!(
            parsed.get("custom-header").map(|s| s.as_str()),
            Some("custom-value")
        );
        assert_eq!(parsed.len(), 3);
    }

    #[test]
    fn test_format_status_badge() {
        let (ok_class, ok_text) = format_status_badge(0);
        assert!(ok_class.contains("green"));
        assert_eq!(ok_text, "OK (0)");

        let (err_class, err_text) = format_status_badge(14);
        assert!(err_class.contains("red"));
        assert_eq!(err_text, "UNAVAILABLE");
    }
}
