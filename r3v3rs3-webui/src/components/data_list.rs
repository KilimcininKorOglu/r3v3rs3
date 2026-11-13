use r3v3rs3_api::i18n::Locale;
use std::collections::{HashMap, HashSet};
use yew::prelude::*;

pub const LINK_CLASS: &str =
    "cursor-pointer font-medium text-blue-600 dark:text-blue-400 hover:underline py-2 md:py-0";
pub const DANGER_LINK_CLASS: &str =
    "cursor-pointer font-medium text-red-600 dark:text-red-500 hover:underline py-2 md:py-0";

/// A column of a list. The list shows a table on medium and wider screens and a card for each
/// row on narrower screens.
pub struct Column {
    /// The translation key of the column name.
    pub label: &'static str,
    /// Extra classes of the header and body cells of the table.
    pub class: &'static str,
}

pub struct Row {
    pub key: String,
    /// One cell for each column. The first cell is the title of the card.
    pub cells: Vec<Html>,
    pub actions: Html,
}

/// The multi-selection properties of a list.
#[derive(Clone, PartialEq)]
pub struct SelectionProps {
    pub selected: HashSet<String>,
    pub on_toggle: Callback<String>,
    pub on_toggle_all: Callback<()>,
    pub all_selected: bool,
    pub selectable_keys: Option<HashSet<String>>,
    pub bulk_actions: Option<Html>,
    pub on_clear: Option<Callback<MouseEvent>>,
}

impl SelectionProps {
    pub fn is_selectable(&self, key: &str) -> bool {
        match &self.selectable_keys {
            Some(keys) => keys.contains(key),
            None => true,
        }
    }
}

/// How a list is drawn. The default is the list of a list page: its own card, the actions
/// column and the large empty message.
#[derive(Default, Clone, PartialEq)]
pub struct ListOptions {
    /// Draws no card around the list, for a list inside a card or a modal.
    pub embedded: bool,
    /// Smaller text and padding, for a list inside a form or a modal.
    pub compact: bool,
    /// Draws no actions column.
    pub no_actions: bool,
    /// A `max-h-*` class. The list scrolls inside this height and keeps its header visible.
    pub max_height: Option<&'static str>,
    /// Column names that replace the translated labels, for names that are not translated
    /// (an address, a hash) or that hold a value.
    pub headings: Option<Vec<String>>,
    /// Extra classes of the rows, by row key.
    pub row_class: HashMap<String, &'static str>,
    /// Makes the rows clickable; the callback gets the row key.
    pub on_row_click: Option<Callback<String>>,
    /// The key of the highlighted row.
    pub selected_key: Option<String>,
}

/// The box of a list page with optional multi-selection.
pub fn selectable_list_card(
    locale: Locale,
    loaded: bool,
    empty: &'static str,
    columns: &[Column],
    rows: &[Row],
    selection: Option<&SelectionProps>,
) -> Html {
    list_view(
        locale,
        ListContent {
            loaded,
            empty,
            columns,
            rows,
        },
        selection,
        &ListOptions::default(),
    )
}

/// The box of a list page. It shows a spinner until the list is loaded, then the empty message
/// or the list. `empty` is a translation key.
pub fn list_card(
    locale: Locale,
    loaded: bool,
    empty: &'static str,
    columns: &[Column],
    rows: &[Row],
) -> Html {
    selectable_list_card(locale, loaded, empty, columns, rows, None)
}

/// The content of a list: the rows and what to show before they load or when there is none.
pub struct ListContent<'a> {
    pub loaded: bool,
    /// The translation key of the empty message.
    pub empty: &'static str,
    pub columns: &'a [Column],
    pub rows: &'a [Row],
}

const CARD: &str = "relative overflow-x-auto bg-white dark:bg-neutral-800 shadow-sm border border-neutral-300 dark:border-neutral-700 rounded-md";
const EMBEDDED: &str = "relative overflow-x-auto";

/// A list drawn with `options`. Every list of the WebUI goes through this function.
pub fn list_view(
    locale: Locale,
    content: ListContent,
    selection: Option<&SelectionProps>,
    options: &ListOptions,
) -> Html {
    let frame = classes!(
        if options.embedded { EMBEDDED } else { CARD },
        options
            .max_height
            .map(|height| classes!(height, "overflow-y-auto"))
    );
    let body = if !content.loaded {
        spinner()
    } else if content.rows.is_empty() {
        empty_message(locale.t(content.empty), options.embedded || options.compact)
    } else {
        let bulk_bar = selection.and_then(|sel| bulk_toolbar(locale, sel));
        html! {
            <>
                { bulk_bar }
                { data_list(locale, &content, selection, options) }
            </>
        }
    };
    html! { <div class={frame}>{ body }</div> }
}

fn spinner() -> Html {
    html! {
        <svg aria-hidden="true" role="status" class="w-8 h-8 mx-auto my-7 text-neutral-200 animate-spin" viewBox="0 0 100 101" fill="none" xmlns="http://www.w3.org/2000/svg">
        <path d="M100 50.5908C100 78.2051 77.6142 100.591 50 100.591C22.3858 100.591 0 78.2051 0 50.5908C0 22.9766 22.3858 0.59082 50 0.59082C77.6142 0.59082 100 22.9766 100 50.5908ZM9.08144 50.5908C9.08144 73.1895 27.4013 91.5094 50 91.5094C72.5987 91.5094 90.9186 73.1895 90.9186 50.5908C90.9186 27.9921 72.5987 9.67226 50 9.67226C27.4013 9.67226 9.08144 27.9921 9.08144 50.5908Z" fill="#ccc"/>
        <path d="M93.9676 39.0409C96.393 38.4038 97.8624 35.9116 97.0079 33.5539C95.2932 28.8227 92.871 24.3692 89.8167 20.348C85.8452 15.1192 80.8826 10.7238 75.2124 7.41289C69.5422 4.10194 63.2754 1.94025 56.7698 1.05124C51.7666 0.367541 46.6976 0.446843 41.7345 1.27873C39.2613 1.69328 37.813 4.19778 38.4501 6.62326C39.0873 9.04874 41.5694 10.4717 44.0505 10.1071C47.8511 9.54855 51.7191 9.52689 55.5402 10.0491C60.8642 10.7766 65.9928 12.5457 70.6331 15.2552C75.2735 17.9648 79.3347 21.5619 82.5849 25.841C84.9175 28.9121 86.7997 32.2913 88.1811 35.8758C89.083 38.2158 91.5421 39.6781 93.9676 39.0409Z" fill="#888"/>
        </svg>
    }
}

fn empty_message(text: &str, small: bool) -> Html {
    let class = if small {
        "py-6 px-4 text-sm text-neutral-500 dark:text-neutral-400 text-center"
    } else {
        "my-8 px-4 sm:px-16 text-lg sm:text-xl font-bold text-neutral-500 dark:text-neutral-300 text-center"
    };
    html! { <p {class}>{ text.to_string() }</p> }
}

fn bulk_toolbar(locale: Locale, sel: &SelectionProps) -> Option<Html> {
    if sel.selected.is_empty() {
        return None;
    }
    let clear = sel.on_clear.clone();
    Some(html! {
        <div class="flex flex-wrap items-center justify-between gap-3 px-4 py-2.5 bg-neutral-100 dark:bg-neutral-800 border-b border-neutral-300 dark:border-neutral-700">
            <div class="flex items-center gap-3">
                <span class="text-sm font-medium text-neutral-800 dark:text-neutral-200">
                    {locale.tf("common.selected_count", &[("count", &sel.selected.len().to_string())])}
                </span>
                if let Some(onclick) = clear {
                    <button type="button" {onclick} class="text-xs text-neutral-500 hover:text-neutral-700 dark:text-neutral-400 dark:hover:text-neutral-200 underline cursor-pointer">
                        {locale.t("common.clear_selection")}
                    </button>
                }
            </div>
            if let Some(actions) = &sel.bulk_actions {
                <div class="flex items-center gap-2">
                    {actions.clone()}
                </div>
            }
        </div>
    })
}

const CHECKBOX: &str = "w-4 h-4 text-primary-600 bg-neutral-100 border-neutral-300 rounded focus:ring-primary-500 dark:focus:ring-primary-600 dark:ring-offset-neutral-800 dark:bg-neutral-700 dark:border-neutral-600 cursor-pointer";

/// The padding of the cells of a regular and a compact list.
fn cell_padding(options: &ListOptions) -> (&'static str, &'static str) {
    if options.compact {
        ("px-3 py-2", "px-3 py-2")
    } else {
        ("px-4 py-3", "px-4 py-4")
    }
}

fn heading(locale: Locale, column: &Column, index: usize, options: &ListOptions) -> String {
    options
        .headings
        .as_ref()
        .and_then(|headings| headings.get(index))
        .cloned()
        .unwrap_or_else(|| locale.t(column.label).to_string())
}

fn data_list(
    locale: Locale,
    content: &ListContent,
    selection: Option<&SelectionProps>,
    options: &ListOptions,
) -> Html {
    let text_size = if options.compact {
        "text-xs"
    } else {
        "text-sm"
    };
    html! {
        <>
            <table class={classes!("hidden", "md:table", "w-full", text_size, "text-left", "text-neutral-600", "dark:text-neutral-200")}>
                { table_head(locale, content.columns, selection, options) }
                <tbody>
                    { for content.rows.iter().map(|row| table_row(content.columns, row, selection, options)) }
                </tbody>
            </table>
            <ul class={classes!("md:hidden", "divide-y", "divide-neutral-300", "dark:divide-neutral-700", text_size, "text-neutral-600", "dark:text-neutral-200")}>
                { for content.rows.iter().map(|row| card(locale, content.columns, row, selection, options)) }
            </ul>
        </>
    }
}

fn table_head(
    locale: Locale,
    columns: &[Column],
    selection: Option<&SelectionProps>,
    options: &ListOptions,
) -> Html {
    let (head_padding, _) = cell_padding(options);
    let sticky = options
        .max_height
        .map(|_| "sticky top-0 z-10 bg-white dark:bg-neutral-800");
    let header_checkbox = selection.map(|sel| {
        let on_toggle_all = sel.on_toggle_all.clone();
        html! {
            <th scope="col" class={classes!(head_padding, "w-4")}>
                <input
                    type="checkbox"
                    checked={sel.all_selected}
                    onchange={Callback::from(move |_: Event| on_toggle_all.emit(()))}
                    class={CHECKBOX}
                />
            </th>
        }
    });
    html! {
        <thead class={classes!("text-xs", "text-neutral-800", "dark:text-neutral-200", "uppercase", "border-b", "border-neutral-300", "dark:border-neutral-700", sticky)}>
            <tr>
                { header_checkbox }
                { for columns.iter().enumerate().map(|(index, column)| html! {
                    <th scope="col" class={classes!(head_padding, column.class)}>{heading(locale, column, index, options)}</th>
                }) }
                if !options.no_actions {
                    <th scope="col" class={head_padding}><span class="sr-only">{locale.t("common.actions")}</span></th>
                }
            </tr>
        </thead>
    }
}

fn row_checkbox(row: &Row, selection: Option<&SelectionProps>) -> Option<Html> {
    let sel = selection?;
    if !sel.is_selectable(&row.key) {
        return Some(html! {});
    }
    let on_toggle = sel.on_toggle.clone();
    let key = row.key.clone();
    Some(html! {
        <input
            type="checkbox"
            checked={sel.selected.contains(&row.key)}
            onclick={Callback::from(|event: MouseEvent| event.stop_propagation())}
            onchange={Callback::from(move |_: Event| on_toggle.emit(key.clone()))}
            class={CHECKBOX}
        />
    })
}

/// The classes and the click handler that the options give a row.
fn row_behavior(row: &Row, options: &ListOptions) -> (Classes, Option<Callback<MouseEvent>>) {
    let selected = options.selected_key.as_deref() == Some(row.key.as_str());
    let onclick = options.on_row_click.clone().map(|click| {
        let key = row.key.clone();
        Callback::from(move |_: MouseEvent| click.emit(key.clone()))
    });
    let class = classes!(
        options.row_class.get(&row.key).copied(),
        onclick
            .as_ref()
            .map(|_| { "cursor-pointer hover:bg-neutral-50 dark:hover:bg-neutral-700/50" }),
        selected.then_some("bg-primary-50 dark:bg-primary-950/40")
    );
    (class, onclick)
}

fn table_row(
    columns: &[Column],
    row: &Row,
    selection: Option<&SelectionProps>,
    options: &ListOptions,
) -> Html {
    let (_, padding) = cell_padding(options);
    let checkbox = row_checkbox(row, selection)
        .map(|checkbox| html! { <td class={classes!("w-4", padding)}>{ checkbox }</td> });
    let (behavior, onclick) = row_behavior(row, options);
    html! {
        <tr key={row.key.clone()} class={classes!("border-b", "last:border-b-0", "border-neutral-200", "dark:border-neutral-700", behavior)} {onclick}>
            { checkbox }
            { for columns.iter().zip(&row.cells).enumerate().map(|(index, (column, cell))| html! {
                <td class={classes!(padding, column.class, (index == 0).then_some("font-medium text-neutral-900 dark:text-neutral-200"))}>{cell.clone()}</td>
            }) }
            if !options.no_actions {
                <td class={classes!(padding, "w-0", "whitespace-nowrap")}>
                    <div class="flex items-center justify-end gap-1.5">{row.actions.clone()}</div>
                </td>
            }
        </tr>
    }
}

fn card(
    locale: Locale,
    columns: &[Column],
    row: &Row,
    selection: Option<&SelectionProps>,
    options: &ListOptions,
) -> Html {
    let mut cells = columns.iter().enumerate().zip(&row.cells);
    let title = cells.next().map(|(_, cell)| cell.clone());
    let checkbox = row_checkbox(row, selection);
    let (behavior, onclick) = row_behavior(row, options);
    html! {
        <li key={row.key.clone()} class={classes!("px-4", "py-3", behavior)} {onclick}>
            <div class="flex items-center gap-2 mb-1">
                { checkbox }
                if let Some(title) = title {
                    <div class="font-medium text-neutral-900 dark:text-neutral-200 break-words">{title}</div>
                }
            </div>
            <dl>
                { for cells.map(|((index, column), cell)| html! {
                    <div class="flex items-center justify-between gap-4 py-1">
                        <dt class="shrink-0 text-neutral-500 dark:text-neutral-400">{heading(locale, column, index, options)}</dt>
                        <dd class="min-w-0 text-right break-words">{cell.clone()}</dd>
                    </div>
                }) }
            </dl>
            if !options.no_actions {
                <div class="flex flex-wrap items-center justify-end gap-1.5 mt-2">{row.actions.clone()}</div>
            }
        </li>
    }
}

/// A switch that turns a list entry on or off. A disabled switch only shows the state.
pub fn active_toggle(active: bool, disabled: bool, onchange: Callback<Event>) -> Html {
    let cursor = if disabled {
        "cursor-not-allowed opacity-50"
    } else {
        "cursor-pointer"
    };
    html! {
        <label class={classes!("relative", "inline-flex", "items-center", "mt-1", cursor)}>
            <input {onchange} {disabled} type="checkbox" checked={active} class="sr-only peer" />
            <div class="w-9 h-4 bg-neutral-200 dark:bg-neutral-600 peer-focus:outline-none peer-focus:ring-4 peer-focus:ring-primary-300 rounded-full peer peer-checked:after:translate-x-full peer-checked:after:border-white after:content-[''] after:absolute after:top-[2px] after:left-[2px] after:bg-white after:border-neutral-300 after:border after:rounded-full after:h-3 after:w-4 after:transition-all peer-checked:bg-primary-600"></div>
        </label>
    }
}

/// A colored dot and the name of a state.
pub fn status_badge(text: &str, color: &'static str) -> Html {
    html! {
        <span class="inline-flex items-center">
            <span class={classes!("h-2.5", "w-2.5", "shrink-0", "rounded-full", "mr-2", color)}></span>
            {text}
        </span>
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const COLUMNS: [Column; 2] = [
        Column {
            label: "common.name",
            class: "",
        },
        Column {
            label: "common.status",
            class: "",
        },
    ];

    #[test]
    fn a_heading_replaces_the_translated_label() {
        let options = ListOptions {
            headings: Some(vec!["IP".into()]),
            ..Default::default()
        };
        assert_eq!(heading(Locale::En, &COLUMNS[0], 0, &options), "IP");
        assert_eq!(heading(Locale::En, &COLUMNS[1], 1, &options), "Status");
    }

    #[test]
    fn a_compact_list_has_smaller_cells() {
        let compact = ListOptions {
            compact: true,
            ..Default::default()
        };
        assert_eq!(cell_padding(&compact), ("px-3 py-2", "px-3 py-2"));
        assert_eq!(
            cell_padding(&ListOptions::default()),
            ("px-4 py-3", "px-4 py-4")
        );
    }

    #[test]
    fn only_a_clickable_row_hovers() {
        let row = Row {
            key: "a".into(),
            cells: vec![],
            actions: html! {},
        };
        let (plain, onclick) = row_behavior(&row, &ListOptions::default());
        assert!(onclick.is_none() && !plain.to_string().contains("hover:"));
        let clickable = ListOptions {
            on_row_click: Some(Callback::noop()),
            selected_key: Some("a".into()),
            ..Default::default()
        };
        let (class, onclick) = row_behavior(&row, &clickable);
        let class = class.to_string();
        assert!(onclick.is_some() && class.contains("hover:") && class.contains("bg-primary-50"));
    }
}
