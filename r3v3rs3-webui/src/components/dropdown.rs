use yew::prelude::*;

#[derive(Properties, PartialEq)]
pub struct Props {
    /// Classes of the button that opens the menu.
    pub class: Classes,
    /// Extra classes of the menu.
    #[prop_or_default]
    pub menu_class: Classes,
    /// The accessible name and the tooltip of the button.
    pub label: AttrValue,
    /// The content of the button.
    pub button: Html,
    /// The menu items. A click in the menu closes it.
    pub children: Html,
}

#[function_component(Dropdown)]
pub fn dropdown(props: &Props) -> Html {
    let open = use_state(|| false);

    let toggle_onclick = {
        let open = open.clone();
        Callback::from(move |_: MouseEvent| open.set(!*open))
    };
    let close_onclick = {
        let open = open.clone();
        Callback::from(move |_: MouseEvent| open.set(false))
    };

    html! {
        <div class="relative flex items-stretch">
            <button type="button" onclick={toggle_onclick} class={props.class.clone()} title={props.label.clone()} aria-label={props.label.clone()} aria-haspopup="menu" aria-expanded={open.to_string()}>
                { props.button.clone() }
            </button>
            if *open {
                <div class="fixed inset-0 z-10" onclick={close_onclick.clone()}></div>
                <ul role="menu" onclick={close_onclick} class={classes!("absolute", "right-0", "top-full", "z-20", "mt-1", "py-1", "rounded-md", "shadow-lg", "border", "border-neutral-300", "dark:border-neutral-700", "bg-white", "dark:bg-neutral-800", "text-sm", "text-neutral-700", "dark:text-neutral-200", props.menu_class.clone())}>
                    { props.children.clone() }
                </ul>
            }
        </div>
    }
}
