use r3v3rs3_api::i18n::Locale;
use yew::prelude::*;

pub const LINK_CLASS: &str =
    "cursor-pointer font-medium text-blue-600 dark:text-blue-400 hover:underline py-2 md:py-0";
pub const WARNING_LINK_CLASS: &str =
    "cursor-pointer font-medium text-orange-600 dark:text-orange-500 hover:underline py-2 md:py-0";
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

/// The box of a list page. It shows a spinner until the list is loaded, then the empty message
/// or the list. `empty` is a translation key.
pub fn list_card(
    locale: Locale,
    loaded: bool,
    empty: &'static str,
    columns: &[Column],
    rows: &[Row],
) -> Html {
    html! {
        <div class="relative overflow-x-auto bg-white dark:bg-neutral-800 shadow-sm border border-neutral-300 dark:border-neutral-700 rounded-md">
            if !loaded {
                <svg aria-hidden="true" role="status" class="w-8 h-8 mx-auto my-7 text-neutral-200 animate-spin" viewBox="0 0 100 101" fill="none" xmlns="http://www.w3.org/2000/svg">
                <path d="M100 50.5908C100 78.2051 77.6142 100.591 50 100.591C22.3858 100.591 0 78.2051 0 50.5908C0 22.9766 22.3858 0.59082 50 0.59082C77.6142 0.59082 100 22.9766 100 50.5908ZM9.08144 50.5908C9.08144 73.1895 27.4013 91.5094 50 91.5094C72.5987 91.5094 90.9186 73.1895 90.9186 50.5908C90.9186 27.9921 72.5987 9.67226 50 9.67226C27.4013 9.67226 9.08144 27.9921 9.08144 50.5908Z" fill="#ccc"/>
                <path d="M93.9676 39.0409C96.393 38.4038 97.8624 35.9116 97.0079 33.5539C95.2932 28.8227 92.871 24.3692 89.8167 20.348C85.8452 15.1192 80.8826 10.7238 75.2124 7.41289C69.5422 4.10194 63.2754 1.94025 56.7698 1.05124C51.7666 0.367541 46.6976 0.446843 41.7345 1.27873C39.2613 1.69328 37.813 4.19778 38.4501 6.62326C39.0873 9.04874 41.5694 10.4717 44.0505 10.1071C47.8511 9.54855 51.7191 9.52689 55.5402 10.0491C60.8642 10.7766 65.9928 12.5457 70.6331 15.2552C75.2735 17.9648 79.3347 21.5619 82.5849 25.841C84.9175 28.9121 86.7997 32.2913 88.1811 35.8758C89.083 38.2158 91.5421 39.6781 93.9676 39.0409Z" fill="#888"/>
                </svg>
            } else if rows.is_empty() {
                <p class="my-8 px-4 sm:px-16 text-lg sm:text-xl font-bold text-neutral-500 dark:text-neutral-300 text-center">{locale.t(empty)}</p>
            } else {
                { data_list(locale, columns, rows) }
            }
        </div>
    }
}

fn data_list(locale: Locale, columns: &[Column], rows: &[Row]) -> Html {
    html! {
        <>
            <table class="hidden md:table w-full text-sm text-left text-neutral-600 dark:text-neutral-200">
                <thead class="text-xs text-neutral-800 dark:text-neutral-200 uppercase border-b border-neutral-300 dark:border-neutral-700">
                    <tr>
                        { for columns.iter().map(|column| html! {
                            <th scope="col" class={classes!("px-4", "py-3", column.class)}>{locale.t(column.label)}</th>
                        }) }
                        <th scope="col" class="px-4 py-3"><span class="sr-only">{locale.t("common.actions")}</span></th>
                    </tr>
                </thead>
                <tbody>
                    { for rows.iter().map(|row| table_row(columns, row)) }
                </tbody>
            </table>
            <ul class="md:hidden divide-y divide-neutral-300 dark:divide-neutral-700 text-sm text-neutral-600 dark:text-neutral-200">
                { for rows.iter().map(|row| card(locale, columns, row)) }
            </ul>
        </>
    }
}

fn table_row(columns: &[Column], row: &Row) -> Html {
    html! {
        <tr key={row.key.clone()} class="border-b last:border-b-0 dark:border-neutral-700">
            { for columns.iter().zip(&row.cells).enumerate().map(|(index, (column, cell))| html! {
                <td class={classes!("px-4", "py-4", column.class, (index == 0).then_some("font-medium text-neutral-900 dark:text-neutral-200"))}>{cell.clone()}</td>
            }) }
            <td class="px-4 py-4 w-0 whitespace-nowrap">
                <div class="flex items-center justify-end gap-5">{row.actions.clone()}</div>
            </td>
        </tr>
    }
}

fn card(locale: Locale, columns: &[Column], row: &Row) -> Html {
    let mut cells = columns.iter().zip(&row.cells);
    let title = cells.next().map(|(_, cell)| cell.clone());
    html! {
        <li key={row.key.clone()} class="px-4 py-3">
            if let Some(title) = title {
                <div class="mb-1 font-medium text-neutral-900 dark:text-neutral-200 break-words">{title}</div>
            }
            <dl>
                { for cells.map(|(column, cell)| html! {
                    <div class="flex items-center justify-between gap-4 py-1">
                        <dt class="shrink-0 text-neutral-500 dark:text-neutral-400">{locale.t(column.label)}</dt>
                        <dd class="min-w-0 text-right break-words">{cell.clone()}</dd>
                    </div>
                }) }
            </dl>
            <div class="flex flex-wrap items-center justify-end gap-x-5">{row.actions.clone()}</div>
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
            <div class="w-9 h-4 bg-neutral-200 dark:bg-neutral-600 peer-focus:outline-none peer-focus:ring-4 peer-focus:ring-blue-300 rounded-full peer peer-checked:after:translate-x-full peer-checked:after:border-white after:content-[''] after:absolute after:top-[2px] after:left-[2px] after:bg-white after:border-neutral-300 after:border after:rounded-full after:h-3 after:w-4 after:transition-all peer-checked:bg-blue-600"></div>
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
