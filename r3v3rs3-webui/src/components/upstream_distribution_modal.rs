use crate::format::{format_bytes, format_latency};
use r3v3rs3_api::{
    i18n::Locale,
    upstream::{CircuitState, UpstreamHealth},
};
use yew::prelude::*;

#[derive(Properties, PartialEq, Clone)]
pub struct UpstreamDistributionModalProps {
    pub locale: Locale,
    pub proxy_name: String,
    pub upstreams: Vec<UpstreamHealth>,
    pub onclose: Callback<()>,
}

pub fn color_for_index(index: usize) -> (&'static str, &'static str, &'static str) {
    match index % 6 {
        0 => ("bg-emerald-500", "text-emerald-500", "border-emerald-500"),
        1 => ("bg-blue-500", "text-blue-500", "border-blue-500"),
        2 => ("bg-purple-500", "text-purple-500", "border-purple-500"),
        3 => ("bg-amber-500", "text-amber-500", "border-amber-500"),
        4 => ("bg-cyan-500", "text-cyan-500", "border-cyan-500"),
        _ => ("bg-rose-500", "text-rose-500", "border-rose-500"),
    }
}

pub fn calc_percentage(part: u64, total: u64) -> f64 {
    if total == 0 {
        0.0
    } else {
        (part as f64 / total as f64) * 100.0
    }
}

pub fn calc_error_rate(errors: u64, requests: u64) -> f64 {
    let total = errors + requests;
    if total == 0 {
        0.0
    } else {
        (errors as f64 / total as f64) * 100.0
    }
}

pub fn format_rtt(rtt_ms: Option<f64>) -> String {
    rtt_ms.map_or_else(|| "—".to_string(), format_latency)
}

#[function_component(UpstreamDistributionModal)]
pub fn upstream_distribution_modal(props: &UpstreamDistributionModalProps) -> Html {
    let total_reqs: u64 = props.upstreams.iter().map(|u| u.total_requests).sum();
    let total_weight: u64 = props
        .upstreams
        .iter()
        .map(|u| u.weight as u64)
        .sum::<u64>()
        .max(1);
    let total_active: usize = props.upstreams.iter().map(|u| u.active_connections).sum();
    let total_errors: u64 = props.upstreams.iter().map(|u| u.total_errors).sum();
    let total_bytes: u64 = props.upstreams.iter().map(|u| u.total_bytes).sum();

    let on_backdrop = {
        let onclose = props.onclose.clone();
        Callback::from(move |e: MouseEvent| {
            e.prevent_default();
            onclose.emit(());
        })
    };

    let on_card_click = Callback::from(|e: MouseEvent| {
        e.stop_propagation();
    });

    html! {
        <div class="fixed inset-0 z-50 flex items-center justify-center p-4 bg-black/60 backdrop-blur-sm" onclick={on_backdrop}>
            <div class="bg-white dark:bg-neutral-900 border border-neutral-200 dark:border-neutral-800 rounded-2xl shadow-2xl max-w-2xl w-full p-6 flex flex-col gap-5 max-h-[90vh] overflow-y-auto" onclick={on_card_click}>
                { render_header(props.locale, &props.proxy_name, total_reqs, &props.onclose) }
                { render_summary_cards(props.locale, total_reqs, total_active, total_errors, total_bytes) }
                { render_distribution_bar(&props.upstreams, total_reqs) }
                { render_upstreams_table(props.locale, &props.upstreams, total_reqs, total_weight) }
            </div>
        </div>
    }
}

fn render_header(
    locale: Locale,
    proxy_name: &str,
    total_reqs: u64,
    onclose: &Callback<()>,
) -> Html {
    let on_close_click = {
        let onclose = onclose.clone();
        Callback::from(move |e: MouseEvent| {
            e.prevent_default();
            onclose.emit(());
        })
    };
    html! {
        <div class="flex items-start justify-between border-b border-neutral-100 dark:border-neutral-800 pb-3">
            <div>
                <h3 class="text-base font-bold text-neutral-900 dark:text-neutral-100 flex items-center gap-2">
                    <svg class="w-5 h-5 text-blue-500" fill="none" stroke="currentColor" viewBox="0 0 24 24">
                        <path stroke-linecap="round" stroke-linejoin="round" stroke-width="2" d="M9 19v-6a2 2 0 00-2-2H5a2 2 0 00-2 2v6a2 2 0 002 2h2a2 2 0 002-2zm0 0V9a2 2 0 012-2h2a2 2 0 012 2v10m-6 0a2 2 0 002 2h2a2 2 0 002-2m0 0V5a2 2 0 012-2h2a2 2 0 012 2v14a2 2 0 01-2 2h-2a2 2 0 01-2-2z" />
                    </svg>
                    { locale.t("proxies.distribution_title") }
                </h3>
                <p class="text-xs text-neutral-500 dark:text-neutral-400 mt-0.5">
                    { format!("{} • {} {}", proxy_name, total_reqs, locale.t("proxies.dist_requests")) }
                </p>
            </div>
            <button
                type="button"
                onclick={on_close_click}
                class="text-neutral-400 hover:text-neutral-600 dark:hover:text-neutral-200 p-1 rounded-lg hover:bg-neutral-100 dark:hover:bg-neutral-800 transition"
            >
                <svg class="w-5 h-5" fill="none" stroke="currentColor" viewBox="0 0 24 24">
                    <path stroke-linecap="round" stroke-linejoin="round" stroke-width="2" d="M6 18L18 6M6 6l12 12" />
                </svg>
            </button>
        </div>
    }
}

fn render_summary_cards(
    locale: Locale,
    requests: u64,
    active: usize,
    errors: u64,
    bytes: u64,
) -> Html {
    let error_rate = calc_error_rate(errors, requests);
    let bytes_formatted = format_bytes(bytes);

    html! {
        <div class="grid grid-cols-2 sm:grid-cols-4 gap-3">
            <div class="p-3 rounded-xl bg-neutral-50 dark:bg-neutral-800/60 border border-neutral-100 dark:border-neutral-800">
                <span class="text-[11px] text-neutral-500 dark:text-neutral-400 font-medium block">{ locale.t("proxies.dist_requests") }</span>
                <span class="text-base font-bold text-neutral-900 dark:text-neutral-100 font-mono">{ requests }</span>
            </div>
            <div class="p-3 rounded-xl bg-neutral-50 dark:bg-neutral-800/60 border border-neutral-100 dark:border-neutral-800">
                <span class="text-[11px] text-neutral-500 dark:text-neutral-400 font-medium block">{ locale.t("proxies.dist_active") }</span>
                <span class="text-base font-bold text-blue-600 dark:text-blue-400 font-mono">{ active }</span>
            </div>
            <div class="p-3 rounded-xl bg-neutral-50 dark:bg-neutral-800/60 border border-neutral-100 dark:border-neutral-800">
                <span class="text-[11px] text-neutral-500 dark:text-neutral-400 font-medium block">{ locale.t("proxies.dist_errors") }</span>
                <span class={classes!("text-base", "font-bold", "font-mono", if errors > 0 { "text-red-500" } else { "text-neutral-700 dark:text-neutral-300" })}>
                    { format!("{:.1}%", error_rate) }
                </span>
            </div>
            <div class="p-3 rounded-xl bg-neutral-50 dark:bg-neutral-800/60 border border-neutral-100 dark:border-neutral-800">
                <span class="text-[11px] text-neutral-500 dark:text-neutral-400 font-medium block">{ locale.t("proxies.dist_throughput") }</span>
                <span class="text-base font-bold text-neutral-900 dark:text-neutral-100 font-mono">{ bytes_formatted }</span>
            </div>
        </div>
    }
}

fn render_distribution_bar(upstreams: &[UpstreamHealth], total_reqs: u64) -> Html {
    if upstreams.is_empty() {
        return html! {};
    }

    html! {
        <div class="flex flex-col gap-1.5">
            <div class="w-full h-3 rounded-full overflow-hidden flex bg-neutral-100 dark:bg-neutral-800 border border-neutral-200/60 dark:border-neutral-700/60">
                { for upstreams.iter().enumerate().map(|(idx, u)| {
                    let pct = if total_reqs > 0 {
                        calc_percentage(u.total_requests, total_reqs)
                    } else {
                        100.0 / (upstreams.len() as f64)
                    };
                    let (bg_color, _, _) = color_for_index(idx);
                    let style = format!("width: {:.1}%", pct);
                    let title = format!("{}: {:.1}% ({} reqs)", u.addr, pct, u.total_requests);
                    html! {
                        <div class={classes!("h-full", bg_color, "transition-all")} {style} {title} />
                    }
                }) }
            </div>
        </div>
    }
}

fn render_upstreams_table(
    locale: Locale,
    upstreams: &[UpstreamHealth],
    total_reqs: u64,
    total_weight: u64,
) -> Html {
    if upstreams.is_empty() {
        return html! {
            <p class="text-sm text-neutral-500 dark:text-neutral-400 text-center py-4">{ locale.t("proxies.dist_empty") }</p>
        };
    }

    html! {
        <div class="flex flex-col gap-2">
            { for upstreams.iter().enumerate().map(|(idx, u)| {
                render_upstream_row(locale, idx, u, total_reqs, total_weight)
            }) }
        </div>
    }
}

fn render_upstream_row(
    locale: Locale,
    idx: usize,
    u: &UpstreamHealth,
    total_reqs: u64,
    total_weight: u64,
) -> Html {
    let (bg_color, text_color, _) = color_for_index(idx);
    let target_pct = calc_percentage(u.weight as u64, total_weight);
    let actual_pct = calc_percentage(u.total_requests, total_reqs);
    let error_rate = calc_error_rate(u.total_errors, u.total_requests);
    let bytes_formatted = format_bytes(u.total_bytes);
    let rtt_formatted = format_rtt(u.rtt_ms);

    html! {
        <div class="p-3 rounded-xl border border-neutral-200 dark:border-neutral-800 bg-white dark:bg-neutral-900 flex flex-col gap-2 hover:border-neutral-300 dark:hover:border-neutral-700 transition">
            <div class="flex items-center justify-between gap-2 flex-wrap">
                <div class="flex items-center gap-2">
                    <span class={classes!("w-3", "h-3", "rounded-full", bg_color)} />
                    <span class="font-mono text-xs font-semibold text-neutral-800 dark:text-neutral-200">{ &u.addr }</span>
                    { render_server_badge(locale, u) }
                </div>
                <div class="flex items-center gap-3 text-xs font-mono">
                    <span class="text-neutral-500 dark:text-neutral-400">
                        { locale.tf("proxies.dist_expected", &[("percent", &format!("{:.1}", target_pct))]) }
                    </span>
                    <span class={classes!("font-bold", text_color)}>
                        { locale.tf("proxies.dist_actual", &[("percent", &format!("{:.1}", actual_pct))]) }
                    </span>
                </div>
            </div>

            <div class="grid grid-cols-2 sm:grid-cols-4 gap-2 text-[11px] font-mono text-neutral-500 dark:text-neutral-400 border-t border-neutral-100 dark:border-neutral-800 pt-2">
                <div>
                    <span class="text-[10px] text-neutral-400 uppercase block">{ locale.t("proxies.dist_requests") }</span>
                    <span class="text-neutral-800 dark:text-neutral-200 font-semibold">{ u.total_requests }</span>
                </div>
                <div>
                    <span class="text-[10px] text-neutral-400 uppercase block">{ locale.t("proxies.dist_active") }</span>
                    <span class="text-blue-600 dark:text-blue-400 font-semibold">{ u.active_connections }</span>
                </div>
                <div>
                    <span class="text-[10px] text-neutral-400 uppercase block">{ locale.t("proxies.dist_rtt") }</span>
                    <span class="text-neutral-800 dark:text-neutral-200 font-semibold">{ rtt_formatted }</span>
                </div>
                <div>
                    <span class="text-[10px] text-neutral-400 uppercase block">{ locale.t("proxies.dist_errors") }</span>
                    <span class={if u.total_errors > 0 { "text-red-500 font-semibold" } else { "text-neutral-800 dark:text-neutral-200 font-semibold" }}>
                        { format!("{} ({:.1}%)", u.total_errors, error_rate) }
                    </span>
                </div>
            </div>
            <div class="flex items-center justify-between text-[11px] text-neutral-400 font-mono">
                <span>{ format!("{}: {}", locale.t("proxies.dist_weight"), u.weight) }</span>
                <span>{ format!("{}: {}", locale.t("proxies.dist_throughput"), bytes_formatted) }</span>
            </div>
        </div>
    }
}

fn render_server_badge(locale: Locale, u: &UpstreamHealth) -> Html {
    match u.circuit {
        CircuitState::Open => html! {
            <span class="px-1.5 py-0.5 rounded text-[10px] font-medium bg-amber-100 text-amber-800 dark:bg-amber-950/60 dark:text-amber-300">
                { locale.t("proxies.circuit_open") }
            </span>
        },
        CircuitState::HalfOpen => html! {
            <span class="px-1.5 py-0.5 rounded text-[10px] font-medium bg-yellow-100 text-yellow-800 dark:bg-yellow-950/60 dark:text-yellow-300">
                { locale.t("proxies.circuit_half_open") }
            </span>
        },
        CircuitState::Closed => {
            if u.healthy {
                html! {
                    <span class="px-1.5 py-0.5 rounded text-[10px] font-medium bg-green-100 text-green-800 dark:bg-green-950/60 dark:text-green-300">
                        { locale.t("proxies.healthy_servers").split(' ').next_back().unwrap_or("healthy") }
                    </span>
                }
            } else {
                html! {
                    <span class="px-1.5 py-0.5 rounded text-[10px] font-medium bg-red-100 text-red-800 dark:bg-red-950/60 dark:text-red-300">
                        { u.last_error.as_deref().unwrap_or("unhealthy") }
                    </span>
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_calc_percentage() {
        assert_eq!(calc_percentage(50, 100), 50.0);
        assert_eq!(calc_percentage(0, 100), 0.0);
        assert_eq!(calc_percentage(10, 0), 0.0);
        assert!((calc_percentage(1, 3) - (100.0 / 3.0)).abs() < 1e-6);
    }

    #[test]
    fn test_calc_error_rate() {
        assert_eq!(calc_error_rate(0, 100), 0.0);
        assert_eq!(calc_error_rate(5, 95), 5.0);
        assert_eq!(calc_error_rate(0, 0), 0.0);
    }

    #[test]
    fn test_color_for_index() {
        assert_eq!(color_for_index(0).0, "bg-emerald-500");
        assert_eq!(color_for_index(1).0, "bg-blue-500");
        assert_eq!(color_for_index(5).0, "bg-rose-500");
        assert_eq!(color_for_index(6).0, "bg-emerald-500");
    }
}
