use crate::{
    auth::{test_token, LoginQuery},
    components::{language_menu::LanguageMenu, theme_menu::ThemeMenu},
    i18n::use_locale,
    pages::Route,
    API_ENDPOINT,
};
use gloo_events::EventListener;
use gloo_net::http::Request;
use r3v3rs3_api::{
    auth::{LoginMethod, LoginRequest, LoginResponse},
    error::ErrorMessage,
};
use serde_derive::Deserialize;
use wasm_bindgen::{prelude::wasm_bindgen, JsCast, UnwrapThrowExt};
use web_sys::HtmlInputElement;
use yew::prelude::*;
use yew_router::prelude::*;

#[derive(Deserialize)]
#[serde(untagged)]
enum ApiResult<T> {
    Ok(T),
    Err(ErrorMessage),
}

#[wasm_bindgen(module = "/js/logout.js")]
extern "C" {
    fn logout();
}

const MENU_BUTTON_CLASS: [&str; 6] = [
    "p-2",
    "rounded-md",
    "text-neutral-600",
    "dark:text-neutral-300",
    "hover:bg-neutral-200",
    "dark:hover:bg-neutral-700",
];

#[function_component(Login)]
pub fn login() -> Html {
    let navigator = use_navigator().unwrap();
    let locale = use_locale();

    use_effect_with((), move |_| {
        EventListener::new(&gloo_utils::document(), "visibilitychange", move |_event| {
            wasm_bindgen_futures::spawn_local(async move {
                if !test_token().await {
                    logout();
                }
            });
        })
        .forget();
    });

    let location = use_location().unwrap();
    let query = location.query::<LoginQuery>().unwrap_or_default();

    let username = use_state(String::new);
    let password = use_state(String::new);
    let totp = use_state(|| Option::<String>::None);
    let error: UseStateHandle<Option<ErrorMessage>> = use_state(|| Option::<ErrorMessage>::None);

    let oninput_username = Callback::from({
        let username = username.clone();
        move |input_event: InputEvent| {
            let target: HtmlInputElement = input_event
                .target()
                .unwrap_throw()
                .dyn_into()
                .unwrap_throw();
            username.set(target.value());
        }
    });

    let oninput_password = Callback::from({
        let password = password.clone();
        move |input_event: InputEvent| {
            let target: HtmlInputElement = input_event
                .target()
                .unwrap_throw()
                .dyn_into()
                .unwrap_throw();
            password.set(target.value());
        }
    });

    let totp_cloned = totp.clone();
    let oninput_totp = Callback::from({
        let totp = totp_cloned;
        move |input_event: InputEvent| {
            let target: HtmlInputElement = input_event
                .target()
                .unwrap_throw()
                .dyn_into()
                .unwrap_throw();
            totp.set(Some(target.value()));
        }
    });

    let totp_cloned = totp.clone();
    let error_cloned = error.clone();
    let username_cloned = username.clone();
    let password_cloned = password.clone();
    let onsubmit = Callback::from(move |event: SubmitEvent| {
        event.prevent_default();

        let navigator = navigator.clone();
        let username = username_cloned.clone();
        let password = password_cloned.clone();
        let totp = totp_cloned.clone();
        let query = query.clone();
        let error = error_cloned.clone();

        let method = if let Some(totp) = &*totp {
            LoginMethod::Totp {
                token: totp.to_string(),
            }
        } else {
            LoginMethod::Password {
                password: password.to_string(),
            }
        };

        wasm_bindgen_futures::spawn_local(async move {
            let insecure = web_sys::window()
                .and_then(|window| window.location().protocol().ok())
                .unwrap_or_default()
                != "https:";
            let login: ApiResult<LoginResponse> = Request::post(&format!("{API_ENDPOINT}/login"))
                .json(&LoginRequest {
                    username: username.to_string(),
                    method,
                    insecure,
                })
                .unwrap()
                .send()
                .await
                .unwrap()
                .json()
                .await
                .unwrap();
            match login {
                ApiResult::Ok(LoginResponse::Success) => {
                    if let Some(redirect) = query.redirect {
                        navigator.replace(&redirect);
                    } else {
                        navigator.push(&Route::Home);
                    }
                }
                ApiResult::Ok(LoginResponse::TotpRequired) => totp.set(Some(String::new())),
                ApiResult::Err(err) => {
                    error.set(Some(err));
                }
            }
        });
    });

    let error_text = (*error).as_ref().map(|err| match &err.error {
        Some(api_error) => locale.error_message(api_error),
        None => err.message.clone(),
    });

    html! {
        <>
        <div class="flex justify-end gap-1 px-4">
            <LanguageMenu class={classes!(MENU_BUTTON_CLASS.to_vec())} />
            <ThemeMenu class={classes!(MENU_BUTTON_CLASS.to_vec())} />
        </div>
        <form class="mx-auto max-w-sm mt-4 px-4" {onsubmit}>
            <div class="mx-auto flex w-full justify-center items-center mb-2">
                <img class="w-8 h-8 dark:invert" src="/assets/logo.svg" />
            </div>
            <div class="mx-auto flex w-full justify-center items-center mb-5">
                <h1 class="font-semibold text-2xl text-neutral-700 dark:text-neutral-200">{locale.t("login.heading")}</h1>
            </div>

            if let Some(message) = error_text {
                <div class="bg-red-100 border border-red-400 text-red-700 dark:bg-red-950 dark:border-red-800 dark:text-red-300 px-4 py-3 rounded relative mb-4" role="alert">
                    <span class="block sm:inline">{message}</span>
                </div>
            }

            if let Some(totp) = &*totp {
                <label class="mr-4 text-neutral-700 dark:text-neutral-200 font-bold inline-block mb-2" for="name">{locale.t("login.one_time_password")}</label>
                <input type="number" class="border bg-white dark:bg-neutral-800 dark:border-neutral-600 py-2 px-4 w-full outline-none focus:ring-2 focus:ring-neutral-400 rounded" oninput={oninput_totp} />
                <input type="submit" class="w-full mt-4 text-neutral-50 font-bold bg-neutral-800 dark:bg-neutral-900 py-3 rounded-md hover:bg-neutral-600 transition duration-300" value={locale.t("login.continue")} disabled={totp.is_empty()} />
            } else {
                <div class="mb-4">
                    <label class="mr-4 text-neutral-700 dark:text-neutral-200 font-bold inline-block mb-2" for="name">{locale.t("login.username")}</label>
                    <input type="text" class="border bg-white dark:bg-neutral-800 dark:border-neutral-600 py-2 px-4 w-full outline-none focus:ring-2 focus:ring-neutral-400 rounded" autocapitalize="off" autofocus={true} oninput={oninput_username} />
                </div>
                <label class="mr-4 text-neutral-700 dark:text-neutral-200 font-bold inline-block mb-2" for="name">{locale.t("login.password")}</label>
                <input type="password" class="border bg-white dark:bg-neutral-800 dark:border-neutral-600 py-2 px-4 w-full outline-none focus:ring-2 focus:ring-neutral-400 rounded" oninput={oninput_password} />
                <input type="submit" class="w-full mt-4 text-neutral-50 font-bold bg-neutral-800 dark:bg-neutral-900 py-3 rounded-md hover:bg-neutral-600 transition duration-300" value={locale.t("login.submit")} disabled={username.is_empty() || password.is_empty()} />
            }
        </form>
        </>
    }
}
