//! The standard modal dialog of the WebUI. Every in-page dialog that is not a SweetAlert2
//! message uses it, so all of them share one look in the light and the dark theme.

use crate::i18n::use_locale;
use gloo_events::EventListener;
use wasm_bindgen::JsCast;
use yew::prelude::*;

#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
pub enum ModalSize {
    Sm,
    #[default]
    Md,
    Lg,
    Xl,
}

impl ModalSize {
    fn class(self) -> &'static str {
        match self {
            ModalSize::Sm => "max-w-md",
            ModalSize::Md => "max-w-xl",
            ModalSize::Lg => "max-w-2xl",
            ModalSize::Xl => "max-w-4xl",
        }
    }
}

#[derive(Properties, PartialEq)]
pub struct ModalProps {
    pub title: AttrValue,
    /// Closes the modal. Without it the modal has no close button, and neither Escape nor a
    /// click on the backdrop closes it.
    #[prop_or_default]
    pub on_close: Option<Callback<()>>,
    #[prop_or_default]
    pub size: ModalSize,
    /// The buttons below the content, aligned to the right.
    #[prop_or_default]
    pub footer: Option<Html>,
    #[prop_or_default]
    pub children: Html,
}

const OVERLAY: &str =
    "fixed inset-0 z-50 flex items-center justify-center p-4 bg-black/60 backdrop-blur-sm";
const PANEL: &str = "w-full flex flex-col bg-white dark:bg-neutral-800 border border-neutral-300 dark:border-neutral-700 rounded-xl shadow-xl max-h-[90vh] text-neutral-900 dark:text-neutral-100";
const HEADER: &str = "flex items-center justify-between gap-4 px-5 py-4 border-b border-neutral-200 dark:border-neutral-700";
const BODY: &str = "px-5 py-4 overflow-y-auto";
const FOOTER: &str = "flex flex-wrap items-center justify-end gap-2 px-5 py-3 border-t border-neutral-200 dark:border-neutral-700";
const CLOSE: &str = "p-1 rounded-lg text-neutral-400 hover:text-neutral-700 hover:bg-neutral-100 dark:hover:text-neutral-200 dark:hover:bg-neutral-700 focus:outline-none focus:ring-2 focus:ring-neutral-300 dark:focus:ring-neutral-600 cursor-pointer";

#[function_component(Modal)]
pub fn modal(props: &ModalProps) -> Html {
    let locale = use_locale();
    use_escape_closes(props.on_close.clone());
    let on_backdrop = props.on_close.clone().map(|close| {
        Callback::from(move |event: MouseEvent| {
            if event.target() == event.current_target() {
                close.emit(());
            }
        })
    });
    let close_button = props.on_close.clone().map(|close| {
        html! {
            <button type="button" class={CLOSE} aria-label={locale.t("common.close")} title={locale.t("common.close")} onclick={Callback::from(move |_| close.emit(()))}>
                <svg class="w-5 h-5" fill="none" stroke="currentColor" viewBox="0 0 24 24">
                    <path stroke-linecap="round" stroke-linejoin="round" stroke-width="2" d="M6 18L18 6M6 6l12 12" />
                </svg>
            </button>
        }
    });
    html! {
        <div class={OVERLAY} role="dialog" aria-modal="true" onclick={on_backdrop}>
            <div class={classes!(PANEL, props.size.class())}>
                <div class={HEADER}>
                    <h2 class="text-lg font-semibold">{ props.title.clone() }</h2>
                    { close_button }
                </div>
                <div class={BODY}>{ props.children.clone() }</div>
                if let Some(footer) = &props.footer {
                    <div class={FOOTER}>{ footer.clone() }</div>
                }
            </div>
        </div>
    }
}

/// Closes the modal when the Escape key is pressed.
#[hook]
fn use_escape_closes(on_close: Option<Callback<()>>) {
    use_effect_with(on_close, |on_close| {
        let listener = on_close.clone().and_then(|close| {
            let document = web_sys::window()?.document()?;
            Some(EventListener::new(&document, "keydown", move |event| {
                let is_escape = event
                    .dyn_ref::<web_sys::KeyboardEvent>()
                    .is_some_and(|key| key.key() == "Escape");
                if is_escape {
                    close.emit(());
                }
            }))
        });
        move || drop(listener)
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_panel_has_a_light_and_a_dark_surface() {
        assert!(PANEL.contains("bg-white") && PANEL.contains("dark:bg-neutral-800"));
        assert!(PANEL.contains("border-neutral-300") && PANEL.contains("dark:border-neutral-700"));
    }

    #[test]
    fn the_overlay_sits_above_the_page() {
        assert!(OVERLAY.contains("fixed inset-0") && OVERLAY.contains("z-50"));
    }

    #[test]
    fn every_size_has_its_own_width() {
        let sizes = [ModalSize::Sm, ModalSize::Md, ModalSize::Lg, ModalSize::Xl];
        let widths: std::collections::HashSet<&str> = sizes.iter().map(|s| s.class()).collect();
        assert_eq!(widths.len(), sizes.len());
    }
}
