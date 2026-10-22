use crate::API_ENDPOINT;
use crate::i18n::use_locale;
use crate::pages::Route;
use gloo_net::http::Request;
use gloo_timers::callback::Interval;
use r3v3rs3_api::anomaly::{AnomalyAlert, AnomalySeverity, SystemAnomaliesSnapshot};
use r3v3rs3_api::i18n::Locale;
use wasm_bindgen_futures::spawn_local;
use yew::prelude::*;
use yew_router::prelude::*;

#[derive(Properties, PartialEq)]
pub struct Props {
    pub class: Classes,
}

#[function_component(NotificationCenter)]
pub fn notification_center(props: &Props) -> Html {
    let locale = use_locale();
    let navigator = use_navigator();
    let snapshot = use_state(|| Option::<SystemAnomaliesSnapshot>::None);
    let open = use_state(|| false);
    let is_dismissing = use_state(|| false);

    use_polling_effect(snapshot.clone());

    let toggle_open = {
        let open = open.clone();
        Callback::from(move |e: MouseEvent| {
            e.prevent_default();
            open.set(!*open);
        })
    };

    let close_menu = {
        let open = open.clone();
        Callback::from(move |_: MouseEvent| {
            open.set(false);
        })
    };

    let on_dismiss = create_dismiss_callback(snapshot.clone(), is_dismissing.clone());

    let active_anomalies = snapshot
        .as_ref()
        .map(|s| s.active_anomalies.clone())
        .unwrap_or_default();
    let active_count = active_anomalies.len();

    html! {
        <div class="relative flex items-stretch">
            { render_bell_button(&props.class, active_count, *open, toggle_open, locale) }
            if *open {
                <div class="fixed inset-0 z-20" onclick={close_menu} />
                { render_dropdown(locale, navigator, &active_anomalies, *is_dismissing, on_dismiss, open) }
            }
        </div>
    }
}

#[hook]
fn use_polling_effect(snapshot: UseStateHandle<Option<SystemAnomaliesSnapshot>>) {
    use_effect_with((), move |_| {
        let snap = snapshot.clone();
        spawn_local(async move {
            fetch_anomalies(&snap).await;
        });

        let snap = snapshot.clone();
        let interval = Interval::new(10_000, move || {
            let snap = snap.clone();
            spawn_local(async move {
                fetch_anomalies(&snap).await;
            });
        });

        move || drop(interval)
    });
}

fn create_dismiss_callback(
    snapshot: UseStateHandle<Option<SystemAnomaliesSnapshot>>,
    is_dismissing: UseStateHandle<bool>,
) -> Callback<MouseEvent> {
    Callback::from(move |e: MouseEvent| {
        e.prevent_default();
        let snapshot = snapshot.clone();
        let is_dismissing = is_dismissing.clone();
        is_dismissing.set(true);
        spawn_local(async move {
            let res = Request::post(&format!("{API_ENDPOINT}/system/anomalies/dismiss"))
                .send()
                .await;
            if res.is_ok()
                && let Some(mut current) = (*snapshot).clone()
            {
                current.active_anomalies.clear();
                snapshot.set(Some(current));
            }
            is_dismissing.set(false);
        });
    })
}

fn render_bell_button(
    class: &Classes,
    active_count: usize,
    open: bool,
    toggle_open: Callback<MouseEvent>,
    locale: Locale,
) -> Html {
    let badge_text = if active_count > 9 {
        "9+".to_string()
    } else {
        active_count.to_string()
    };

    html! {
        <button
            type="button"
            class={classes!(class.clone(), "relative")}
            onclick={toggle_open}
            title={locale.t("anomalies.notifications_center")}
            aria-label={locale.t("anomalies.notifications_center")}
            aria-haspopup="true"
            aria-expanded={open.to_string()}
        >
            { bell_icon() }
            if active_count > 0 {
                <span class="absolute top-1.5 right-1.5 flex h-4 min-w-4 px-1 items-center justify-center rounded-full bg-red-600 text-[10px] font-bold text-white shadow ring-2 ring-neutral-800 animate-pulse">
                    { badge_text }
                </span>
            }
        </button>
    }
}

fn render_dropdown(
    locale: Locale,
    navigator: Option<Navigator>,
    active_anomalies: &[AnomalyAlert],
    is_dismissing: bool,
    on_dismiss: Callback<MouseEvent>,
    open: UseStateHandle<bool>,
) -> Html {
    html! {
        <div class="absolute right-0 top-full z-30 mt-2 w-80 sm:w-96 rounded-lg border border-neutral-300 dark:border-neutral-700 bg-white dark:bg-neutral-800 shadow-xl overflow-hidden text-neutral-800 dark:text-neutral-100">
            { render_dropdown_header(locale, active_anomalies.len(), is_dismissing, on_dismiss) }
            { render_dropdown_body(locale, active_anomalies) }
            { render_dropdown_footer(locale, navigator, open) }
        </div>
    }
}

fn render_dropdown_header(
    locale: Locale,
    count: usize,
    is_dismissing: bool,
    on_dismiss: Callback<MouseEvent>,
) -> Html {
    html! {
        <div class="flex items-center justify-between px-4 py-3 border-b border-neutral-200 dark:border-neutral-700 bg-neutral-50 dark:bg-neutral-850">
            <div class="flex items-center gap-2">
                <span class="font-semibold text-sm">{locale.t("anomalies.notifications_center")}</span>
                if count > 0 {
                    <span class="px-2 py-0.5 text-xs font-semibold rounded-full bg-red-100 text-red-700 dark:bg-red-900/40 dark:text-red-300">
                        {format!("{} {}", count, locale.t("anomalies.active_badge"))}
                    </span>
                }
            </div>
            if count > 0 {
                <button
                    type="button"
                    disabled={is_dismissing}
                    onclick={on_dismiss}
                    class="text-xs font-medium text-neutral-500 hover:text-red-600 dark:text-neutral-400 dark:hover:text-red-400 transition"
                >
                    {locale.t("anomalies.clear_all")}
                </button>
            }
        </div>
    }
}

fn render_dropdown_body(locale: Locale, active_anomalies: &[AnomalyAlert]) -> Html {
    html! {
        <div class="max-h-80 overflow-y-auto divide-y divide-neutral-200 dark:divide-neutral-700">
            if active_anomalies.is_empty() {
                <div class="flex flex-col items-center justify-center p-6 text-center text-neutral-500 dark:text-neutral-400">
                    <svg xmlns="http://www.w3.org/2000/svg" class="w-8 h-8 mb-2 text-emerald-500" fill="none" viewBox="0 0 24 24" stroke="currentColor">
                        <path stroke-linecap="round" stroke-linejoin="round" stroke-width="2" d="M9 12l2 2 4-4m6 2a9 9 0 11-18 0 9 9 0 0118 0z" />
                    </svg>
                    <span class="text-xs">{locale.t("anomalies.no_active_anomalies")}</span>
                </div>
            } else {
                { for active_anomalies.iter().map(|alert| render_alert_item(locale, alert)) }
            }
        </div>
    }
}

fn render_dropdown_footer(
    locale: Locale,
    navigator: Option<Navigator>,
    open: UseStateHandle<bool>,
) -> Html {
    let Some(nav) = navigator else {
        return html! {};
    };

    let on_traffic = {
        let nav = nav.clone();
        let open = open.clone();
        Callback::from(move |_: MouseEvent| {
            open.set(false);
            nav.push(&Route::Traffic);
        })
    };

    let on_settings = {
        let open = open.clone();
        Callback::from(move |_: MouseEvent| {
            open.set(false);
            nav.push(&Route::Settings);
        })
    };

    html! {
        <div class="flex items-center justify-between px-4 py-2 bg-neutral-50 dark:bg-neutral-850 border-t border-neutral-200 dark:border-neutral-700 text-xs">
            <button type="button" class="text-blue-600 dark:text-blue-400 hover:underline" onclick={on_traffic}>
                {locale.t("anomalies.view_traffic")}
            </button>
            <button type="button" class="text-neutral-600 dark:text-neutral-300 hover:underline" onclick={on_settings}>
                {locale.t("anomalies.rules_card_title")}
            </button>
        </div>
    }
}

async fn fetch_anomalies(snapshot: &UseStateHandle<Option<SystemAnomaliesSnapshot>>) {
    let res = Request::get(&format!("{API_ENDPOINT}/system/anomalies"))
        .send()
        .await;
    if let Ok(response) = res
        && response.ok()
        && let Ok(data) = response.json::<SystemAnomaliesSnapshot>().await
    {
        snapshot.set(Some(data));
    }
}

fn render_alert_item(locale: Locale, alert: &AnomalyAlert) -> Html {
    let is_critical = alert.severity == AnomalySeverity::Critical;
    let badge_class = if is_critical {
        "bg-red-100 text-red-700 dark:bg-red-950/60 dark:text-red-300 border border-red-300 dark:border-red-800"
    } else {
        "bg-amber-100 text-amber-800 dark:bg-amber-950/60 dark:text-amber-300 border border-amber-300 dark:border-amber-800"
    };
    let severity_text = if is_critical {
        locale.t("anomalies.critical")
    } else {
        locale.t("anomalies.warning")
    };

    html! {
        <div class="p-3 text-xs flex flex-col gap-1 hover:bg-neutral-50 dark:hover:bg-neutral-750 transition">
            <div class="flex items-center justify-between gap-2">
                <span class={classes!("px-1.5", "py-0.5", "rounded", "font-mono", "font-bold", "text-[10px]", badge_class)}>
                    {severity_text}
                </span>
                <span class="text-neutral-500 font-mono text-[10px]">
                    {format!("{:.1} {} / {:.0} {}", alert.current_value, alert.unit, alert.threshold_value, alert.unit)}
                </span>
            </div>
            <div class="font-medium text-neutral-900 dark:text-neutral-100 mt-0.5">
                {&alert.summary}
            </div>
        </div>
    }
}

fn bell_icon() -> Html {
    html! {
        <svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 24 24" class="w-5 h-5" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round">
            <path d="M18 8A6 6 0 0 0 6 8c0 7-3 9-3 9h18s-3-2-3-9" />
            <path d="M13.73 21a2 2 0 0 1-3.46 0" />
        </svg>
    }
}
