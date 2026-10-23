use yew::prelude::*;

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum ActionVariant {
    Edit,
    View,
    Terminal,
    Files,
    Metrics,
    Logs,
    Play,
    Pause,
    Download,
    Restore,
    Delete,
}

#[derive(Properties, PartialEq, Clone)]
pub struct ActionIconButtonProps {
    pub variant: ActionVariant,
    pub title: String,
    #[prop_or_default]
    pub onclick: Option<Callback<MouseEvent>>,
    #[prop_or_default]
    pub href: Option<String>,
    #[prop_or_default]
    pub download: Option<String>,
    #[prop_or_default]
    pub disabled: bool,
}

pub fn action_variant_classes(variant: ActionVariant) -> &'static str {
    match variant {
        ActionVariant::Edit => {
            "text-blue-600 dark:text-blue-400 hover:bg-blue-50 dark:hover:bg-blue-950/40 focus:ring-blue-300 dark:focus:ring-blue-800"
        }
        ActionVariant::View => {
            "text-neutral-500 dark:text-neutral-400 hover:bg-neutral-100 dark:hover:bg-neutral-800 focus:ring-neutral-300 dark:focus:ring-neutral-700"
        }
        ActionVariant::Terminal => {
            "text-emerald-600 dark:text-emerald-400 hover:bg-emerald-50 dark:hover:bg-emerald-950/40 focus:ring-emerald-300 dark:focus:ring-emerald-800"
        }
        ActionVariant::Files => {
            "text-amber-600 dark:text-amber-400 hover:bg-amber-50 dark:hover:bg-amber-950/40 focus:ring-amber-300 dark:focus:ring-amber-800"
        }
        ActionVariant::Metrics => {
            "text-cyan-600 dark:text-cyan-400 hover:bg-cyan-50 dark:hover:bg-cyan-950/40 focus:ring-cyan-300 dark:focus:ring-cyan-800"
        }
        ActionVariant::Logs => {
            "text-indigo-600 dark:text-indigo-400 hover:bg-indigo-50 dark:hover:bg-indigo-950/40 focus:ring-indigo-300 dark:focus:ring-indigo-800"
        }
        ActionVariant::Play => {
            "text-emerald-600 dark:text-emerald-400 hover:bg-emerald-50 dark:hover:bg-emerald-950/40 focus:ring-emerald-300 dark:focus:ring-emerald-800"
        }
        ActionVariant::Pause => {
            "text-orange-500 dark:text-orange-400 hover:bg-orange-50 dark:hover:bg-orange-950/40 focus:ring-orange-300 dark:focus:ring-orange-800"
        }
        ActionVariant::Download => {
            "text-neutral-600 dark:text-neutral-300 hover:bg-neutral-100 dark:hover:bg-neutral-800 focus:ring-neutral-300 dark:focus:ring-neutral-700"
        }
        ActionVariant::Restore => {
            "text-teal-600 dark:text-teal-400 hover:bg-teal-50 dark:hover:bg-teal-950/40 focus:ring-teal-300 dark:focus:ring-teal-800"
        }
        ActionVariant::Delete => {
            "text-red-600 dark:text-red-400 hover:bg-red-50 dark:hover:bg-red-950/40 focus:ring-red-300 dark:focus:ring-red-800"
        }
    }
}

fn render_icon_svg(variant: ActionVariant) -> Html {
    match variant {
        ActionVariant::Edit => html! {
            <svg class="w-4 h-4" fill="none" stroke="currentColor" viewBox="0 0 24 24">
                <path stroke-linecap="round" stroke-linejoin="round" stroke-width="2" d="M15.232 5.232l3.536 3.536m-2.036-5.036a2.5 2.5 0 113.536 3.536L6.5 21.036H3v-3.572L16.732 3.732z" />
            </svg>
        },
        ActionVariant::View => html! {
            <svg class="w-4 h-4" fill="none" stroke="currentColor" viewBox="0 0 24 24">
                <path stroke-linecap="round" stroke-linejoin="round" stroke-width="2" d="M15 12a3 3 0 11-6 0 3 3 0 016 0z" />
                <path stroke-linecap="round" stroke-linejoin="round" stroke-width="2" d="M2.458 12C3.732 7.943 7.523 5 12 5c4.478 0 8.268 2.943 9.542 7-1.274 4.057-5.064 7-9.542 7-4.477 0-8.268-2.943-9.542-7z" />
            </svg>
        },
        ActionVariant::Terminal => html! {
            <svg class="w-4 h-4" fill="none" stroke="currentColor" viewBox="0 0 24 24">
                <path stroke-linecap="round" stroke-linejoin="round" stroke-width="2" d="M8 9l3 3-3 3m5 0h3M5 20h14a2 2 0 002-2V6a2 2 0 00-2-2H5a2 2 0 00-2 2v12a2 2 0 002 2z" />
            </svg>
        },
        ActionVariant::Files => html! {
            <svg class="w-4 h-4" fill="none" stroke="currentColor" viewBox="0 0 24 24">
                <path stroke-linecap="round" stroke-linejoin="round" stroke-width="2" d="M3 7v10a2 2 0 002 2h14a2 2 0 002-2V9a2 2 0 00-2-2h-6l-2-2H5a2 2 0 00-2 2z" />
            </svg>
        },
        ActionVariant::Metrics => html! {
            <svg class="w-4 h-4" fill="none" stroke="currentColor" viewBox="0 0 24 24">
                <path stroke-linecap="round" stroke-linejoin="round" stroke-width="2" d="M9 19v-6a2 2 0 00-2-2H5a2 2 0 00-2 2v6a2 2 0 002 2h2a2 2 0 002-2zm0 0V9a2 2 0 012-2h2a2 2 0 012 2v10m-6 0a2 2 0 002 2h2a2 2 0 002-2m0 0V5a2 2 0 012-2h2a2 2 0 012 2v14a2 2 0 01-2 2h-2a2 2 0 01-2-2z" />
            </svg>
        },
        _ => render_secondary_icon_svg(variant),
    }
}

fn render_secondary_icon_svg(variant: ActionVariant) -> Html {
    match variant {
        ActionVariant::Logs => html! {
            <svg class="w-4 h-4" fill="none" stroke="currentColor" viewBox="0 0 24 24">
                <path stroke-linecap="round" stroke-linejoin="round" stroke-width="2" d="M9 12h6m-6 4h6m2 5H7a2 2 0 01-2-2V5a2 2 0 012-2h5.586a1 1 0 01.707.293l5.414 5.414a1 1 0 01.293.707V19a2 2 0 01-2 2z" />
            </svg>
        },
        ActionVariant::Play => html! {
            <svg class="w-4 h-4" fill="currentColor" viewBox="0 0 24 24">
                <path d="M4.5 5.653c0-1.426 1.529-2.33 2.779-1.643l11.54 6.348c1.295.712 1.295 2.573 0 3.285L7.28 19.991c-1.25.687-2.779-.217-2.779-1.643V5.653z" />
            </svg>
        },
        ActionVariant::Pause => html! {
            <svg class="w-4 h-4" fill="none" stroke="currentColor" viewBox="0 0 24 24">
                <path stroke-linecap="round" stroke-linejoin="round" stroke-width="2" d="M10 9v6m4-6v6m7-3a9 9 0 11-18 0 9 9 0 0118 0z" />
            </svg>
        },
        ActionVariant::Download => html! {
            <svg class="w-4 h-4" fill="none" stroke="currentColor" viewBox="0 0 24 24">
                <path stroke-linecap="round" stroke-linejoin="round" stroke-width="2" d="M4 16v1a3 3 0 003 3h10a3 3 0 003-3v-1m-4-4l-4 4m0 0l-4-4m4 4V4" />
            </svg>
        },
        ActionVariant::Restore => html! {
            <svg class="w-4 h-4" fill="none" stroke="currentColor" viewBox="0 0 24 24">
                <path stroke-linecap="round" stroke-linejoin="round" stroke-width="2" d="M4 4v5h.582m15.356 2A8.001 8.001 0 004.582 9m0 0H9m11 11v-5h-.581m0 0a8.003 8.003 0 01-15.357-2m15.357 2H15" />
            </svg>
        },
        ActionVariant::Delete => html! {
            <svg class="w-4 h-4" fill="none" stroke="currentColor" viewBox="0 0 24 24">
                <path stroke-linecap="round" stroke-linejoin="round" stroke-width="2" d="M19 7l-.867 12.142A2 2 0 0116.138 21H7.862a2 2 0 01-1.995-1.858L5 7m5 4v6m4-6v6m1-10V4a1 1 0 00-1-1h-4a1 1 0 00-1 1v3M4 7h16" />
            </svg>
        },
        _ => html!(),
    }
}

#[function_component(ActionIconButton)]
pub fn action_icon_button(props: &ActionIconButtonProps) -> Html {
    let variant_classes = action_variant_classes(props.variant);
    let onclick = props.onclick.clone();
    let disabled = props.disabled;

    let base_classes = classes!(
        "inline-flex",
        "items-center",
        "justify-center",
        "w-8",
        "h-8",
        "rounded-lg",
        "transition-colors",
        "focus:outline-none",
        "focus:ring-2",
        "cursor-pointer",
        variant_classes,
        disabled.then_some("opacity-40 cursor-not-allowed")
    );

    let icon = render_icon_svg(props.variant);

    if let Some(href) = &props.href {
        return html! {
            <a
                href={href.clone()}
                download={props.download.clone()}
                title={props.title.clone()}
                aria-label={props.title.clone()}
                class={base_classes}
            >
                { icon }
            </a>
        };
    }

    let click_cb = Callback::from(move |e: MouseEvent| {
        e.prevent_default();
        if !disabled && let Some(cb) = &onclick {
            cb.emit(e);
        }
    });

    html! {
        <button
            type="button"
            title={props.title.clone()}
            aria-label={props.title.clone()}
            {disabled}
            onclick={click_cb}
            class={base_classes}
        >
            { icon }
        </button>
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_action_variant_classes_coverage() {
        let variants = [
            ActionVariant::Edit,
            ActionVariant::View,
            ActionVariant::Terminal,
            ActionVariant::Files,
            ActionVariant::Metrics,
            ActionVariant::Logs,
            ActionVariant::Play,
            ActionVariant::Pause,
            ActionVariant::Download,
            ActionVariant::Restore,
            ActionVariant::Delete,
        ];
        for v in variants {
            let cls = action_variant_classes(v);
            assert!(!cls.is_empty());
        }
    }
}
