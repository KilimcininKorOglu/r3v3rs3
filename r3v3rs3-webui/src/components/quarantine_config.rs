//! The auto-quarantine settings card of the security page.

use crate::API_ENDPOINT;
use crate::components::button::{Button, ButtonKind};
use crate::components::proxy_subcomponents::common::{
    HINT_CLASS, INPUT_CLASS, LABEL_CLASS, checked, parse_count, parse_size, state_input, text,
    text_area, toggle, translated,
};
use crate::i18n::use_locale;
use crate::pages::settings::{Notice, fetch_json, notice_view, send_json};
use crate::store::SessionStore;
use gloo_net::http::Request;
use r3v3rs3_api::cidr::{format_cidr_list, parse_cidr_list};
use r3v3rs3_api::i18n::Locale;
use r3v3rs3_api::quarantine::AutoQuarantineConfig;
use yew::prelude::*;
use yewdux::prelude::*;

const CONFIG_PATH: &str = "/security/quarantine/config";

/// The editable text of an [`AutoQuarantineConfig`].
#[derive(Clone, Default, PartialEq)]
struct QuarantineForm {
    enabled: bool,
    max_violations: String,
    window_seconds: String,
    ban_duration_seconds: String,
    /// Comma-separated status codes.
    block_status_codes: String,
    /// One scanner path per line.
    scanner_patterns: String,
    /// Addresses and networks separated by commas, spaces or new lines.
    allowed_ips: String,
}

impl QuarantineForm {
    fn new(config: &AutoQuarantineConfig) -> Self {
        Self {
            enabled: config.enabled,
            max_violations: config.max_violations.to_string(),
            window_seconds: config.window_seconds.to_string(),
            ban_duration_seconds: config.ban_duration_seconds.to_string(),
            block_status_codes: config
                .block_status_codes
                .iter()
                .map(u16::to_string)
                .collect::<Vec<_>>()
                .join(", "),
            scanner_patterns: config.scanner_patterns.join("\n"),
            allowed_ips: format_cidr_list(&config.allowed_ips),
        }
    }

    /// The configuration of the form, or the translated message of its first invalid field.
    fn to_config(&self, locale: Locale) -> Result<AutoQuarantineConfig, String> {
        let config = AutoQuarantineConfig {
            enabled: self.enabled,
            max_violations: parse_count(
                locale,
                &self.max_violations,
                "settings.quarantine_max_violations",
            )?,
            window_seconds: parse_size(
                locale,
                &self.window_seconds,
                "settings.quarantine_window_seconds",
            )?,
            ban_duration_seconds: parse_size(
                locale,
                &self.ban_duration_seconds,
                "settings.quarantine_ban_seconds",
            )?,
            block_status_codes: parse_status_codes(locale, &self.block_status_codes)?,
            scanner_patterns: scanner_patterns(&self.scanner_patterns),
            allowed_ips: translated(locale, parse_cidr_list(&self.allowed_ips))?,
        };
        translated(locale, config.validate())?;
        Ok(config)
    }
}

/// Parses comma-separated HTTP status codes from 100 to 599.
fn parse_status_codes(locale: Locale, value: &str) -> Result<Vec<u16>, String> {
    value
        .split(',')
        .map(str::trim)
        .filter(|item| !item.is_empty())
        .map(|item| {
            item.parse::<u16>()
                .ok()
                .filter(|code| (100..=599).contains(code))
                .ok_or_else(|| {
                    locale
                        .t("settings.quarantine_status_codes_invalid")
                        .to_string()
                })
        })
        .collect()
}

/// The non-empty, trimmed lines of the scanner path field.
fn scanner_patterns(value: &str) -> Vec<String> {
    value
        .lines()
        .map(str::trim)
        .filter(|line| !line.is_empty())
        .map(str::to_string)
        .collect()
}

/// The auto-quarantine settings card: limits, status codes, scanner paths and the allowed list.
#[function_component(QuarantineConfigCard)]
pub fn quarantine_config_card() -> Html {
    let locale = use_locale();
    let (session, _) = use_store::<SessionStore>();
    let form = use_state(QuarantineForm::default);
    let notice = use_state(|| Option::<Notice>::None);
    let saving = use_state(|| false);

    {
        let (form, notice) = (form.clone(), notice.clone());
        use_effect_with(session.info.is_some(), move |signed_in| {
            if *signed_in {
                wasm_bindgen_futures::spawn_local(async move {
                    let request = Request::get(&format!("{API_ENDPOINT}{CONFIG_PATH}"));
                    match fetch_json::<AutoQuarantineConfig>(locale, request).await {
                        Ok(config) => form.set(QuarantineForm::new(&config)),
                        Err(err) => notice.set(Some(Notice::Failed(err))),
                    }
                });
            }
        });
    }

    let onsubmit = submit_callback(locale, &form, &notice, &saving);
    html! {
        <form {onsubmit} class="mt-6 bg-white dark:bg-neutral-800 shadow-sm p-5 border border-neutral-300 dark:border-neutral-700 rounded-md">
            { notice_view(&notice) }
            <h2 class="text-lg font-semibold text-neutral-900 dark:text-neutral-200">{locale.t("settings.quarantine_config_title")}</h2>
            <p class={HINT_CLASS}>{locale.t("settings.quarantine_config_hint")}</p>
            { toggle(state_input(&form, checked, |form, value| form.enabled = value), form.enabled, locale.t("settings.quarantine_enabled"), "mt-4") }
            <div class="grid grid-cols-1 md:grid-cols-3 gap-4">
                { number_field(locale.t("settings.quarantine_max_violations"), &form.max_violations, state_input(&form, text, |form, value| form.max_violations = value)) }
                { number_field(locale.t("settings.quarantine_window_seconds"), &form.window_seconds, state_input(&form, text, |form, value| form.window_seconds = value)) }
                { number_field(locale.t("settings.quarantine_ban_seconds"), &form.ban_duration_seconds, state_input(&form, text, |form, value| form.ban_duration_seconds = value)) }
            </div>
            { list_fields(locale, &form) }
            <div class="mt-5 flex justify-end">
                <Button kind={ButtonKind::Primary} r#type="submit" disabled={*saving}>
                    {locale.t("settings.save")}
                </Button>
            </div>
        </form>
    }
}

fn number_field(label: &'static str, value: &str, onchange: Callback<Event>) -> Html {
    html! {
        <div>
            <label class={LABEL_CLASS}>{label}</label>
            <input type="number" min="1" value={value.to_string()} {onchange} class={INPUT_CLASS} />
        </div>
    }
}

fn list_fields(locale: Locale, form: &UseStateHandle<QuarantineForm>) -> Html {
    html! {
        <>
            <label class={LABEL_CLASS}>{locale.t("settings.quarantine_status_codes")}</label>
            <input type="text" value={form.block_status_codes.clone()} onchange={state_input(form, text, |form, value| form.block_status_codes = value)} class={INPUT_CLASS} placeholder="404, 403" />
            <p class={HINT_CLASS}>{locale.t("settings.quarantine_status_codes_hint")}</p>

            <label class={LABEL_CLASS}>{locale.t("settings.quarantine_scanner_patterns")}</label>
            <textarea rows="5" value={form.scanner_patterns.clone()} onchange={state_input(form, text_area, |form, value| form.scanner_patterns = value)} class={classes!(INPUT_CLASS, "font-mono")} />
            <p class={HINT_CLASS}>{locale.t("settings.quarantine_scanner_patterns_hint")}</p>

            <label class={LABEL_CLASS}>{locale.t("settings.quarantine_allowed_ips")}</label>
            <input type="text" autocapitalize="off" value={form.allowed_ips.clone()} onchange={state_input(form, text, |form, value| form.allowed_ips = value)} class={classes!(INPUT_CLASS, "font-mono")} placeholder="198.51.100.0/24, 203.0.113.9" />
            <p class={HINT_CLASS}>{locale.t("settings.quarantine_allowed_ips_hint")}</p>
        </>
    }
}

/// Validates the form, saves the configuration and shows the result in the notice.
fn submit_callback(
    locale: Locale,
    form: &UseStateHandle<QuarantineForm>,
    notice: &UseStateHandle<Option<Notice>>,
    saving: &UseStateHandle<bool>,
) -> Callback<SubmitEvent> {
    let (form, notice, saving) = (form.clone(), notice.clone(), saving.clone());
    Callback::from(move |event: SubmitEvent| {
        event.prevent_default();
        let config = match form.to_config(locale) {
            Ok(config) => config,
            Err(err) => return notice.set(Some(Notice::Failed(err))),
        };
        let (notice, saving) = (notice.clone(), saving.clone());
        saving.set(true);
        wasm_bindgen_futures::spawn_local(async move {
            let request = Request::put(&format!("{API_ENDPOINT}{CONFIG_PATH}"));
            notice.set(Some(match send_json(locale, request, &config).await {
                Ok(()) => Notice::Success(locale.t("settings.quarantine_config_saved").to_string()),
                Err(err) => Notice::Failed(err),
            }));
            saving.set(false);
        });
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_form_round_trips_the_configuration() {
        let config = AutoQuarantineConfig {
            allowed_ips: vec!["198.51.100.0/24".parse().unwrap()],
            ..Default::default()
        };
        let form = QuarantineForm::new(&config);
        assert_eq!(form.to_config(Locale::En), Ok(config));
    }

    #[test]
    fn the_form_refuses_invalid_fields() {
        let valid = QuarantineForm::new(&AutoQuarantineConfig::default());
        for form in [
            QuarantineForm {
                block_status_codes: "404, 99".into(),
                ..valid.clone()
            },
            QuarantineForm {
                allowed_ips: "198.51.100.0/33".into(),
                ..valid.clone()
            },
            QuarantineForm {
                max_violations: "0".into(),
                ..valid.clone()
            },
        ] {
            assert!(form.to_config(Locale::En).is_err());
        }
    }

    #[test]
    fn every_quarantine_setting_text_is_translated() {
        for key in [
            "settings.quarantine_config_title",
            "settings.quarantine_config_hint",
            "settings.quarantine_enabled",
            "settings.quarantine_max_violations",
            "settings.quarantine_window_seconds",
            "settings.quarantine_ban_seconds",
            "settings.quarantine_status_codes",
            "settings.quarantine_status_codes_hint",
            "settings.quarantine_status_codes_invalid",
            "settings.quarantine_scanner_patterns",
            "settings.quarantine_scanner_patterns_hint",
            "settings.quarantine_allowed_ips",
            "settings.quarantine_allowed_ips_hint",
            "settings.quarantine_config_saved",
        ] {
            assert_ne!(Locale::Tr.t(key), key);
            assert_ne!(Locale::En.t(key), key);
        }
    }
}
