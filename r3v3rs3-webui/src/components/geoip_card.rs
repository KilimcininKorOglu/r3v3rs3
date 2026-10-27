//! GeoIP database status inspection and configuration component in Security Center.

use crate::API_ENDPOINT;
use crate::format::{format_bytes, format_unix_time};
use crate::i18n::use_locale;
use gloo_net::http::Request;
use r3v3rs3_api::geoip::{GeoIpStatus, UpdateGeoIpConfigRequest};
use r3v3rs3_api::i18n::Locale;
use std::path::PathBuf;
use wasm_bindgen_futures::spawn_local;
use web_sys::HtmlInputElement;
use yew::prelude::*;

const INPUT_CLASS: &str = "bg-neutral-50 border border-neutral-300 text-neutral-900 text-sm rounded-lg focus:ring-blue-500 focus:border-blue-500 block w-full p-2.5 dark:bg-neutral-700 dark:border-neutral-600 dark:placeholder-neutral-400 dark:text-white dark:focus:ring-blue-500 dark:focus:border-blue-500";
const HINT_CLASS: &str = "mt-1.5 text-xs text-neutral-500 dark:text-neutral-400";
const BUTTON_CLASS: &str = "inline-flex justify-center items-center text-neutral-500 bg-neutral-50 dark:text-neutral-200 dark:bg-neutral-800 border border-neutral-300 dark:border-neutral-600 focus:outline-none hover:bg-neutral-100 hover:dark:bg-neutral-900 focus:ring-4 focus:ring-neutral-200 dark:focus:ring-neutral-600 font-medium rounded-lg text-sm px-4 py-2";
const PRIMARY_BUTTON_CLASS: &str = "inline-flex justify-center items-center text-white bg-blue-600 hover:bg-blue-700 focus:ring-4 focus:ring-blue-300 font-medium rounded-lg text-sm px-4 py-2 dark:bg-blue-600 dark:hover:bg-blue-700 focus:outline-none dark:focus:ring-blue-800";

#[derive(Clone, Default, PartialEq)]
struct GeoIpFormData {
    account_id: String,
    license_key: String,
    database: String,
    auto_update: bool,
    update_interval_days: u32,
    license_key_configured: bool,
}

#[function_component(GeoIpCard)]
pub fn geoip_card() -> Html {
    let locale = use_locale();
    let status = use_state(|| Option::<GeoIpStatus>::None);
    let form_data = use_state(GeoIpFormData::default);
    let form_initialized = use_state(|| false);
    let notice = use_state(|| Option::<(bool, String)>::None);
    let is_updating = use_state(|| false);
    let is_saving = use_state(|| false);
    let reload = use_state(|| 0);

    let reload_val = *reload;
    let status_handle = status.clone();
    let form_data_handle = form_data.clone();
    let form_init_handle = form_initialized.clone();
    use_effect_with(reload_val, move |_| {
        spawn_local(async move {
            let url = format!("{API_ENDPOINT}/security/geoip/status");
            let Ok(res) = Request::get(&url).send().await else {
                return;
            };
            let Ok(st) = res.json::<GeoIpStatus>().await else {
                return;
            };
            if !*form_init_handle {
                form_data_handle.set(GeoIpFormData {
                    account_id: st.account_id.clone().unwrap_or_default(),
                    license_key: String::new(),
                    database: st.database_path.clone().unwrap_or_default(),
                    auto_update: st.auto_update,
                    update_interval_days: st.update_interval_days,
                    license_key_configured: st.license_key_configured,
                });
                form_init_handle.set(true);
            }
            status_handle.set(Some(st));
        });
    });

    let on_update_click = make_update_callback(
        locale,
        status.clone(),
        reload.clone(),
        notice.clone(),
        is_updating.clone(),
    );

    let on_save_click = make_save_callback(
        locale,
        form_data.clone(),
        reload.clone(),
        notice.clone(),
        is_saving.clone(),
    );

    html! {
        <div class="space-y-6">
            { render_notice(&notice) }
            { render_status_card(locale, status.as_ref(), &on_update_click, *is_updating) }
            { render_config_card(locale, &form_data, &on_save_click, *is_saving) }
        </div>
    }
}

fn make_update_callback(
    locale: Locale,
    status: UseStateHandle<Option<GeoIpStatus>>,
    reload: UseStateHandle<usize>,
    notice: UseStateHandle<Option<(bool, String)>>,
    is_updating: UseStateHandle<bool>,
) -> Callback<MouseEvent> {
    Callback::from(move |_: MouseEvent| {
        if *is_updating {
            return;
        }
        is_updating.set(true);
        let status = status.clone();
        let reload = reload.clone();
        let notice = notice.clone();
        let is_updating = is_updating.clone();
        spawn_local(async move {
            let url = format!("{API_ENDPOINT}/security/geoip/update");
            match Request::post(&url).send().await {
                Ok(res) if res.ok() => {
                    if let Ok(st) = res.json::<GeoIpStatus>().await {
                        status.set(Some(st));
                    }
                    notice.set(Some((
                        true,
                        locale.t("settings.geoip_update_success").to_string(),
                    )));
                    reload.set(*reload + 1);
                }
                _ => {
                    notice.set(Some((
                        false,
                        locale.t("security.geoip_update_failed").to_string(),
                    )));
                }
            }
            is_updating.set(false);
        });
    })
}

fn make_save_callback(
    locale: Locale,
    form_data: UseStateHandle<GeoIpFormData>,
    reload: UseStateHandle<usize>,
    notice: UseStateHandle<Option<(bool, String)>>,
    is_saving: UseStateHandle<bool>,
) -> Callback<MouseEvent> {
    Callback::from(move |_: MouseEvent| {
        if *is_saving {
            return;
        }
        is_saving.set(true);
        let form_data = form_data.clone();
        let reload = reload.clone();
        let notice = notice.clone();
        let is_saving = is_saving.clone();
        spawn_local(async move {
            let req_body = build_update_request(&form_data);
            let url = format!("{API_ENDPOINT}/security/geoip/config");
            let req = Request::put(&url).json(&req_body);
            match req {
                Ok(req) => match req.send().await {
                    Ok(res) if res.ok() => {
                        notice.set(Some((
                            true,
                            locale.t("security.geoip_save_success").to_string(),
                        )));
                        reload.set(*reload + 1);
                    }
                    _ => {
                        notice.set(Some((
                            false,
                            locale.t("security.geoip_save_failed").to_string(),
                        )));
                    }
                },
                Err(_) => {
                    notice.set(Some((
                        false,
                        locale.t("security.geoip_save_failed").to_string(),
                    )));
                }
            }
            is_saving.set(false);
        });
    })
}

fn build_update_request(form_data: &GeoIpFormData) -> UpdateGeoIpConfigRequest {
    UpdateGeoIpConfigRequest {
        account_id: (!form_data.account_id.trim().is_empty())
            .then(|| form_data.account_id.trim().to_string()),
        license_key: (!form_data.license_key.trim().is_empty())
            .then(|| form_data.license_key.trim().to_string()),
        database: (!form_data.database.trim().is_empty())
            .then(|| PathBuf::from(form_data.database.trim())),
        auto_update: form_data.auto_update,
        update_interval_days: form_data.update_interval_days.clamp(1, 90),
    }
}

fn render_notice(notice: &Option<(bool, String)>) -> Html {
    let Some((success, msg)) = notice else {
        return html! {};
    };
    let cls = if *success {
        "p-4 text-sm text-green-800 rounded-lg bg-green-50 dark:bg-neutral-800 dark:text-green-400 border border-green-200 dark:border-green-800"
    } else {
        "p-4 text-sm text-red-800 rounded-lg bg-red-50 dark:bg-neutral-800 dark:text-red-400 border border-red-200 dark:border-red-800"
    };
    html! {
        <div class={cls} role="alert">
            {msg}
        </div>
    }
}

fn render_status_card(
    locale: Locale,
    status: Option<&GeoIpStatus>,
    on_update_click: &Callback<MouseEvent>,
    is_updating: bool,
) -> Html {
    html! {
        <div class="bg-white dark:bg-neutral-800 shadow-sm p-5 border border-neutral-300 dark:border-neutral-700 rounded-md">
            <div class="flex flex-col sm:flex-row sm:items-center sm:justify-between mb-4">
                <div>
                    <h2 class="text-lg font-semibold text-neutral-900 dark:text-neutral-200">
                        {locale.t("settings.geoip")}
                    </h2>
                    <p class="mt-1 text-sm text-neutral-500 dark:text-neutral-400">
                        {locale.t("settings.geoip_database_hint")}
                    </p>
                </div>
                <div class="mt-2 sm:mt-0">
                    { render_status_badge(locale, status) }
                </div>
            </div>
            if let Some(st) = status {
                { render_details_grid(locale, st) }
            }
            <div class="flex flex-col mt-4 sm:flex-row sm:items-center sm:justify-end">
                <button
                    type="button"
                    onclick={on_update_click.clone()}
                    disabled={is_updating}
                    class={BUTTON_CLASS}
                >
                    if is_updating {
                        {locale.t("settings.geoip_updating")}
                    } else {
                        {locale.t("settings.geoip_update_now")}
                    }
                </button>
            </div>
        </div>
    }
}

fn render_status_badge(locale: Locale, status: Option<&GeoIpStatus>) -> Html {
    let is_loaded = status.is_some_and(|s| s.loaded);
    if is_loaded {
        html! {
            <span class="inline-flex items-center px-2.5 py-0.5 rounded-full text-xs font-medium bg-green-100 text-green-800 dark:bg-green-900 dark:text-green-200">
                {locale.t("settings.geoip_status_loaded")}
            </span>
        }
    } else {
        html! {
            <span class="inline-flex items-center px-2.5 py-0.5 rounded-full text-xs font-medium bg-neutral-100 text-neutral-800 dark:bg-neutral-700 dark:text-neutral-300">
                {locale.t("settings.geoip_status_not_loaded")}
            </span>
        }
    }
}

fn render_details_grid(locale: Locale, st: &GeoIpStatus) -> Html {
    let build_date = st
        .build_epoch
        .map(|epoch| format_unix_time(locale, epoch as i64))
        .unwrap_or_else(|| "-".to_string());
    let size_str = st
        .file_size_bytes
        .map(format_bytes)
        .unwrap_or_else(|| "-".to_string());
    let path_str = st.database_path.as_deref().unwrap_or("-");

    html! {
        <dl class="grid grid-cols-1 gap-x-4 gap-y-3 sm:grid-cols-3 mt-3 pt-3 border-t border-neutral-200 dark:border-neutral-700 text-sm">
            <div>
                <dt class="font-medium text-neutral-500 dark:text-neutral-400">{locale.t("settings.geoip_status_path")}</dt>
                <dd class="mt-1 text-neutral-900 dark:text-neutral-200 font-mono text-xs truncate" title={path_str.to_string()}>{path_str}</dd>
            </div>
            <div>
                <dt class="font-medium text-neutral-500 dark:text-neutral-400">{locale.t("settings.geoip_status_size")}</dt>
                <dd class="mt-1 text-neutral-900 dark:text-neutral-200">{size_str}</dd>
            </div>
            <div>
                <dt class="font-medium text-neutral-500 dark:text-neutral-400">{locale.t("settings.geoip_status_build_epoch")}</dt>
                <dd class="mt-1 text-neutral-900 dark:text-neutral-200">{build_date}</dd>
            </div>
        </dl>
    }
}

fn render_config_card(
    locale: Locale,
    form_data: &UseStateHandle<GeoIpFormData>,
    on_save_click: &Callback<MouseEvent>,
    is_saving: bool,
) -> Html {
    let form = (*form_data).clone();
    html! {
        <div class="bg-white dark:bg-neutral-800 shadow-sm p-5 border border-neutral-300 dark:border-neutral-700 rounded-md">
            <h3 class="text-base font-semibold text-neutral-900 dark:text-neutral-100 mb-4">
                {locale.t("security.tab_geoip")}
            </h3>
            <div class="space-y-4">
                { render_input_fields(locale, form_data, &form) }
                <div class="flex flex-col pt-3 sm:flex-row sm:items-center sm:justify-end border-t border-neutral-200 dark:border-neutral-700">
                    <button
                        type="button"
                        onclick={on_save_click.clone()}
                        disabled={is_saving}
                        class={PRIMARY_BUTTON_CLASS}
                    >
                        {locale.t("security.geoip_save")}
                    </button>
                </div>
            </div>
        </div>
    }
}

fn render_input_fields(
    locale: Locale,
    form_data: &UseStateHandle<GeoIpFormData>,
    form: &GeoIpFormData,
) -> Html {
    let on_path_input = {
        let fd = form_data.clone();
        Callback::from(move |e: InputEvent| {
            if let Some(target) = e.target_dyn_into::<HtmlInputElement>() {
                let mut updated = (*fd).clone();
                updated.database = target.value();
                fd.set(updated);
            }
        })
    };
    let on_acc_input = {
        let fd = form_data.clone();
        Callback::from(move |e: InputEvent| {
            if let Some(target) = e.target_dyn_into::<HtmlInputElement>() {
                let mut updated = (*fd).clone();
                updated.account_id = target.value();
                fd.set(updated);
            }
        })
    };
    let on_key_input = {
        let fd = form_data.clone();
        Callback::from(move |e: InputEvent| {
            if let Some(target) = e.target_dyn_into::<HtmlInputElement>() {
                let mut updated = (*fd).clone();
                updated.license_key = target.value();
                fd.set(updated);
            }
        })
    };
    let on_auto_input = {
        let fd = form_data.clone();
        Callback::from(move |e: Event| {
            if let Some(target) = e.target_dyn_into::<HtmlInputElement>() {
                let mut updated = (*fd).clone();
                updated.auto_update = target.checked();
                fd.set(updated);
            }
        })
    };
    let on_interval_input = {
        let fd = form_data.clone();
        Callback::from(move |e: InputEvent| {
            if let Some(target) = e.target_dyn_into::<HtmlInputElement>() {
                let mut updated = (*fd).clone();
                updated.update_interval_days = target.value().parse().unwrap_or(7);
                fd.set(updated);
            }
        })
    };

    html! {
        <>
            <div>
                <label class="block mb-1 text-sm font-medium text-neutral-900 dark:text-neutral-200">
                    {locale.t("settings.geoip_database")}
                </label>
                <input
                    type="text"
                    value={form.database.clone()}
                    oninput={on_path_input}
                    placeholder="/var/lib/GeoIP/GeoLite2-Country.mmdb"
                    class={INPUT_CLASS}
                />
                <p class={HINT_CLASS}>{locale.t("settings.geoip_database_hint")}</p>
            </div>
            <div>
                <label class="block mb-1 text-sm font-medium text-neutral-900 dark:text-neutral-200">
                    {locale.t("settings.geoip_account_id")}
                </label>
                <input
                    type="text"
                    value={form.account_id.clone()}
                    oninput={on_acc_input}
                    placeholder="123456"
                    class={INPUT_CLASS}
                />
                <p class={HINT_CLASS}>{locale.t("settings.geoip_account_id_hint")}</p>
            </div>
            <div>
                <label class="block mb-1 text-sm font-medium text-neutral-900 dark:text-neutral-200">
                    {locale.t("settings.geoip_license_key")}
                </label>
                <input
                    type="password"
                    value={form.license_key.clone()}
                    oninput={on_key_input}
                    placeholder="••••••••"
                    class={INPUT_CLASS}
                />
                if form.license_key_configured {
                    <p class="mt-1.5 text-xs text-green-600 dark:text-green-400">
                        {locale.t("settings.geoip_license_key_configured")}
                    </p>
                } else {
                    <p class={HINT_CLASS}>{locale.t("settings.geoip_license_key_hint")}</p>
                }
            </div>
            <div class="flex items-center gap-2 pt-2">
                <input
                    type="checkbox"
                    id="geoip_auto_update"
                    checked={form.auto_update}
                    onchange={on_auto_input}
                    class="w-4 h-4 text-blue-600 bg-neutral-100 border-neutral-300 rounded focus:ring-blue-500 dark:focus:ring-blue-600 dark:ring-offset-neutral-800 focus:ring-2 dark:bg-neutral-700 dark:border-neutral-600"
                />
                <label for="geoip_auto_update" class="text-sm font-medium text-neutral-900 dark:text-neutral-200">
                    {locale.t("settings.geoip_auto_update")}
                </label>
            </div>
            <p class={HINT_CLASS}>{locale.t("settings.geoip_auto_update_hint")}</p>
            <div>
                <label class="block mb-1 text-sm font-medium text-neutral-900 dark:text-neutral-200">
                    {locale.t("settings.geoip_interval_days")}
                </label>
                <input
                    type="number"
                    min="1"
                    max="90"
                    value={form.update_interval_days.to_string()}
                    oninput={on_interval_input}
                    class={INPUT_CLASS}
                />
                <p class={HINT_CLASS}>{locale.t("settings.geoip_interval_days_hint")}</p>
            </div>
        </>
    }
}
