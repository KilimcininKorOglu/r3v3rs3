use super::common::*;
use crate::components::auth_config::AuthForm;
use r3v3rs3_api::access_list::AccessListEntry;
use r3v3rs3_api::cert::{CertInfo, CertKind};
use r3v3rs3_api::cidr::parse_cidr_list;
use r3v3rs3_api::header_rules::{HeaderRule, HeaderRules, parse_header_rules};
use r3v3rs3_api::i18n::Locale;
use r3v3rs3_api::id::ShortId;
use r3v3rs3_api::policy::{AuthPolicy, ConcurrencyLimit, IpFilter, RateLimit, RatePeriod};
use r3v3rs3_api::security_headers::{
    HeaderAuditItem, HeaderAuditStatus, SecurityGrade, SecurityProfile, analyze_response_rules,
};
use std::collections::HashMap;
use yew::prelude::*;

#[derive(Clone, PartialEq, Default)]
pub struct RateLimitForm {
    pub requests: String,
    pub per: RatePeriod,
    pub burst: String,
}

impl RateLimitForm {
    pub fn new(limit: &RateLimit) -> Self {
        Self {
            requests: limit.requests.to_string(),
            per: limit.per,
            burst: limit.burst.to_string(),
        }
    }
}

pub fn rate_limit_view(
    locale: Locale,
    limit: &RateLimitForm,
    on_requests: Callback<Event>,
    on_per: Callback<Event>,
    on_burst: Callback<Event>,
) -> Html {
    html! {
        <div class="grid grid-cols-1 sm:grid-cols-3 gap-x-4">
            <div>
                <label class={LABEL_CLASS}>{locale.t("http_form.requests")}</label>
                <input type="number" min="0" value={limit.requests.clone()} onchange={on_requests} class={INPUT_CLASS} />
            </div>
            <div>
                <label class={LABEL_CLASS}>{locale.t("http_form.per")}</label>
                <select onchange={on_per} class={INPUT_CLASS}>
                    { for RatePeriod::ALL.iter().map(|value| html! {
                        <option value={value.as_str()} selected={*value == limit.per}>{period_label(locale, *value)}</option>
                    }) }
                </select>
            </div>
            <div>
                <label class={LABEL_CLASS}>{locale.t("http_form.burst")}</label>
                <input type="number" min="0" value={limit.burst.clone()} onchange={on_burst} class={INPUT_CLASS} />
            </div>
        </div>
    }
}

pub fn period_label(locale: Locale, period: RatePeriod) -> String {
    locale.t(&format!("period.{}", period.as_str())).to_string()
}

pub fn period(event: &Event) -> RatePeriod {
    select_option(event, &RatePeriod::ALL, |period| period.as_str())
}

pub fn parse_rate_limit(
    locale: Locale,
    form: &RateLimitForm,
    key: &str,
    errors: &mut HashMap<String, String>,
) -> RateLimit {
    RateLimit {
        requests: or_error(
            parse_count(locale, &form.requests, "http_form.requests"),
            key,
            errors,
        ),
        per: form.per,
        burst: or_error(
            parse_count(locale, &form.burst, "http_form.burst"),
            key,
            errors,
        ),
    }
}

#[derive(Clone, PartialEq, Default)]
pub struct ConcurrencyLimitForm {
    pub max: String,
    pub header: String,
}

impl ConcurrencyLimitForm {
    pub fn new(limit: &ConcurrencyLimit) -> Self {
        Self {
            max: limit.max.to_string(),
            header: limit.header.clone().unwrap_or_default(),
        }
    }
}

pub fn concurrency_limit_view(
    locale: Locale,
    limit: &ConcurrencyLimitForm,
    on_max: Callback<Event>,
    on_header: Callback<Event>,
) -> Html {
    html! {
        <div class="grid grid-cols-1 sm:grid-cols-2 gap-x-4">
            <div>
                <label class={LABEL_CLASS}>{locale.t("http_form.concurrency_max")}</label>
                <input type="number" min="0" value={limit.max.clone()} onchange={on_max} class={INPUT_CLASS} />
            </div>
            <div>
                <label class={LABEL_CLASS}>{locale.t("http_form.concurrency_header")}</label>
                <input type="text" autocapitalize="off" placeholder="X-API-Key" value={limit.header.clone()} onchange={on_header} class={INPUT_CLASS} />
            </div>
        </div>
    }
}

pub fn parse_concurrency_limit(
    locale: Locale,
    form: &ConcurrencyLimitForm,
    key: &str,
    errors: &mut HashMap<String, String>,
) -> ConcurrencyLimit {
    let max = or_error(
        parse_count(locale, &form.max, "http_form.concurrency_max"),
        key,
        errors,
    );
    let header = form.header.trim();
    let header = if header.is_empty() {
        None
    } else {
        if !r3v3rs3_api::policy::is_header_name(header) {
            errors.insert(
                key.into(),
                locale.tf("error.invalid_header_name", &[("name", header)]),
            );
        }
        Some(header.to_string())
    };
    ConcurrencyLimit { max, header }
}

pub fn parse_country_list(input: &str) -> Vec<String> {
    input
        .split(',')
        .map(|s| s.trim().to_ascii_uppercase())
        .filter(|s| !s.is_empty())
        .collect()
}

pub struct AccessFilterParams<'a> {
    pub access_list: Option<ShortId>,
    pub allow: &'a str,
    pub deny: &'a str,
    pub allow_countries: &'a str,
    pub deny_countries: &'a str,
    pub auth_form: &'a AuthForm,
}

pub fn parse_access_filter(
    locale: Locale,
    params: &AccessFilterParams<'_>,
    errors: &mut HashMap<String, String>,
) -> (IpFilter, AuthPolicy) {
    if params.access_list.is_some() {
        return Default::default();
    }
    let ip_filter = IpFilter {
        allow: or_error(
            translated(locale, parse_cidr_list(params.allow)),
            "allow",
            errors,
        ),
        deny: or_error(
            translated(locale, parse_cidr_list(params.deny)),
            "deny",
            errors,
        ),
        allow_countries: parse_country_list(params.allow_countries),
        deny_countries: parse_country_list(params.deny_countries),
    };
    (
        ip_filter,
        or_error(params.auth_form.parse(locale), "auth", errors),
    )
}

pub fn access_list_view(
    locale: Locale,
    onchange: Callback<Event>,
    selected: Option<ShortId>,
    lists: &[AccessListEntry],
    hint_key: &'static str,
) -> Html {
    let unknown = selected.filter(|id| lists.iter().all(|entry| entry.id != *id));
    let options = html! {
        <>
            <option value="" selected={selected.is_none()}>{locale.t("http_form.access_list_none")}</option>
            { for lists.iter().map(|entry| html! {
                <option value={entry.id.to_string()} selected={selected == Some(entry.id)}>{entry.list.name.clone()}</option>
            }) }
            if let Some(id) = unknown {
                <option value={id.to_string()} selected=true>{id.to_string()}</option>
            }
        </>
    };
    select_field(
        locale.t("http_form.access_list"),
        onchange,
        options,
        Some(locale.t(hint_key)),
    )
}

#[hook]
pub fn use_access_lists() -> UseStateHandle<Vec<AccessListEntry>> {
    use_loaded_list("/access_lists", |_| true)
}

pub fn is_upstream_client_cert(cert: &CertInfo) -> bool {
    cert.kind == CertKind::Client && cert.has_private_key
}

pub fn client_cert_view(
    locale: Locale,
    onchange: Callback<Event>,
    selected: Option<ShortId>,
    certs: &[CertInfo],
) -> Html {
    let options = html! {
        <>
            <option value="" selected={selected.is_none()}>{locale.t("proxy_form.client_cert_none")}</option>
            { for certs.iter().map(|cert| html! {
                <option value={cert.id.to_string()} selected={selected == Some(cert.id)}>{client_cert_label(cert)}</option>
            }) }
        </>
    };
    select_field(
        locale.t("proxy_form.client_cert"),
        onchange,
        options,
        Some(locale.t("proxy_form.client_cert_hint")),
    )
}

pub fn parse_optional_id(value: &str) -> Option<ShortId> {
    if value.is_empty() {
        return None;
    }
    value.parse().ok()
}

pub fn client_cert_label(cert: &CertInfo) -> String {
    let names = cert
        .san
        .iter()
        .map(ToString::to_string)
        .collect::<Vec<_>>()
        .join(", ");
    format!("{names} ({})", cert.id)
}

#[hook]
pub fn use_client_certs() -> UseStateHandle<Vec<CertInfo>> {
    use_loaded_list("/certs", is_upstream_client_cert)
}

pub fn security_grade_badge(grade: SecurityGrade, score: u8) -> Html {
    let (bg, label) = match grade {
        SecurityGrade::APlus => (
            "bg-emerald-500/10 border-emerald-500/30 text-emerald-400",
            "A+",
        ),
        SecurityGrade::A => (
            "bg-emerald-500/10 border-emerald-500/30 text-emerald-400",
            "A",
        ),
        SecurityGrade::B => ("bg-lime-500/10 border-lime-500/30 text-lime-400", "B"),
        SecurityGrade::C => ("bg-amber-500/10 border-amber-500/30 text-amber-400", "C"),
        SecurityGrade::D => ("bg-orange-500/10 border-orange-500/30 text-orange-400", "D"),
        SecurityGrade::F => ("bg-rose-500/10 border-rose-500/30 text-rose-400", "F"),
    };

    html! {
        <div class={classes!("flex", "items-center", "gap-2", "px-2.5", "py-1", "rounded-md", "border", bg)}>
            <span class="font-bold text-sm leading-none">{label}</span>
            <span class="text-xs opacity-80">{format!("{score}/100")}</span>
        </div>
    }
}

pub fn security_profile_buttons(locale: Locale, on_apply: Callback<SecurityProfile>) -> Html {
    let profiles = [
        (SecurityProfile::Strict, "http_form.profile_strict"),
        (SecurityProfile::Balanced, "http_form.profile_balanced"),
        (SecurityProfile::ApiOnly, "http_form.profile_api_only"),
        (
            SecurityProfile::SpaFrontend,
            "http_form.profile_spa_frontend",
        ),
    ];

    html! {
        <div class="flex flex-wrap items-center gap-1.5">
            <span class="text-xs text-neutral-400 font-medium mr-1">{format!("{}:", locale.t("http_form.security_profiles"))}</span>
            { for profiles.into_iter().map(|(prof, key)| {
                let on_click = {
                    let on_apply = on_apply.clone();
                    Callback::from(move |_| on_apply.emit(prof))
                };
                html! {
                    <button
                        type="button"
                        onclick={on_click}
                        class="px-2 py-0.5 text-xs font-medium rounded border border-neutral-700 bg-neutral-800 hover:bg-neutral-700 text-neutral-200 transition-colors cursor-pointer"
                    >
                        {locale.t(key)}
                    </button>
                }
            }) }
        </div>
    }
}

pub fn security_audit_item_badge(locale: Locale, item: &HeaderAuditItem) -> Html {
    let (color, icon, status_text) = match item.status {
        HeaderAuditStatus::Present => (
            "text-emerald-400 bg-emerald-500/10 border-emerald-500/20",
            "✓",
            locale.t("http_form.audit_present"),
        ),
        HeaderAuditStatus::Weak => (
            "text-amber-400 bg-amber-500/10 border-amber-500/20",
            "⚠",
            locale.t("http_form.audit_weak"),
        ),
        HeaderAuditStatus::Missing => (
            "text-rose-400 bg-rose-500/10 border-rose-500/20",
            "✕",
            locale.t("http_form.audit_missing"),
        ),
    };

    html! {
        <div class={classes!("flex", "items-center", "justify-between", "px-2", "py-1", "rounded", "border", "text-xs", color)} title={format!("{}: {}", locale.t("http_form.audit_risk"), item.risk)}>
            <div class="flex items-center gap-1.5 truncate">
                <span class="font-bold shrink-0">{icon}</span>
                <span class="font-mono text-[11px] truncate">{&item.header_name}</span>
            </div>
            <span class="text-[10px] uppercase font-semibold opacity-85 shrink-0 ml-1">{status_text}</span>
        </div>
    }
}

pub fn security_wizard_panel(
    locale: Locale,
    response_rules_text: &str,
    on_apply: Callback<SecurityProfile>,
) -> Html {
    let parsed_rules = parse_header_rules(response_rules_text).unwrap_or_default();
    let audit = analyze_response_rules(&parsed_rules);

    html! {
        <div class="mb-3 p-3 rounded-lg border border-neutral-800 bg-neutral-900/60 flex flex-col gap-2.5">
            <div class="flex items-center justify-between flex-wrap gap-2">
                <div class="flex items-center gap-2">
                    <span class="text-sm font-semibold text-neutral-200">
                        {locale.t("http_form.security_headers_wizard")}
                    </span>
                    { security_grade_badge(audit.grade, audit.score) }
                </div>
                { security_profile_buttons(locale, on_apply) }
            </div>

            <div class="grid grid-cols-1 sm:grid-cols-2 lg:grid-cols-3 gap-1.5 mt-1">
                { for audit.items.iter().map(|item| security_audit_item_badge(locale, item)) }
            </div>
        </div>
    }
}

pub fn header_rules_view(
    locale: Locale,
    request: &str,
    response: &str,
    on_request: Callback<Event>,
    on_response: Callback<Event>,
    on_apply_profile: Callback<SecurityProfile>,
) -> Html {
    html! {
        <div>
            { security_wizard_panel(locale, response, on_apply_profile) }
            <div class="grid grid-cols-1 sm:grid-cols-2 gap-x-4">
                <div>
                    <label class={LABEL_CLASS}>{locale.t("http_form.request_headers")}</label>
                    <textarea rows="4" autocapitalize="off" spellcheck="false" placeholder={"set X-Request-Id: {request_id}\nremove X-Debug"} value={request.to_string()} onchange={on_request} class={classes!(INPUT_CLASS, "font-mono")} />
                </div>
                <div>
                    <label class={LABEL_CLASS}>{locale.t("http_form.response_headers")}</label>
                    <textarea rows="4" autocapitalize="off" spellcheck="false" placeholder={"set X-Frame-Options: DENY\nremove Server"} value={response.to_string()} onchange={on_response} class={classes!(INPUT_CLASS, "font-mono")} />
                </div>
            </div>
        </div>
    }
}

pub fn parse_header_rules_form(
    locale: Locale,
    request: &str,
    response: &str,
    key: &str,
    errors: &mut HashMap<String, String>,
) -> HeaderRules {
    HeaderRules {
        request: or_error(
            parse_lines(
                locale,
                request,
                "http_form.request_headers_error",
                HeaderRule::parse_line,
            ),
            key,
            errors,
        ),
        response: or_error(
            parse_lines(
                locale,
                response,
                "http_form.response_headers_error",
                HeaderRule::parse_line,
            ),
            key,
            errors,
        ),
    }
}
