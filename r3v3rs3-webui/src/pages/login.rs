use crate::{
    API_ENDPOINT,
    auth::{LoginQuery, test_token},
    components::{language_menu::LanguageMenu, theme_menu::ThemeMenu},
    i18n::use_locale,
    pages::Route,
};
use gloo_events::EventListener;
use gloo_net::http::Request;
use r3v3rs3_api::{
    auth::{LoginMethod, LoginRequest, LoginResponse},
    error::ErrorMessage,
};
use serde_derive::Deserialize;
use wasm_bindgen::{JsCast, UnwrapThrowExt, prelude::wasm_bindgen};
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

    use_logout_on_hide();

    let location = use_location().unwrap();
    let query = location.query::<LoginQuery>().unwrap_or_default();

    let username = use_state(String::new);
    let password = use_state(String::new);
    let totp = use_state(|| Option::<String>::None);
    let error: UseStateHandle<Option<ErrorMessage>> = use_state(|| Option::<ErrorMessage>::None);

    let oninput_username = text_oninput(username.clone());
    let oninput_password = text_oninput(password.clone());
    let oninput_totp = totp_oninput(totp.clone());

    let onsubmit = submit_callback(LoginForm {
        navigator,
        query,
        username: username.clone(),
        password: password.clone(),
        totp: totp.clone(),
        error: error.clone(),
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

/// Signs the account out when the tab becomes visible again without a valid token.
#[hook]
fn use_logout_on_hide() {
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
}

fn input_value(event: &InputEvent) -> String {
    let target: HtmlInputElement = event.target().unwrap_throw().dyn_into().unwrap_throw();
    target.value()
}

fn text_oninput(state: UseStateHandle<String>) -> Callback<InputEvent> {
    Callback::from(move |event: InputEvent| state.set(input_value(&event)))
}

fn totp_oninput(state: UseStateHandle<Option<String>>) -> Callback<InputEvent> {
    Callback::from(move |event: InputEvent| state.set(Some(input_value(&event))))
}

/// The state of the sign-in form.
struct LoginForm {
    navigator: Navigator,
    query: LoginQuery,
    username: UseStateHandle<String>,
    password: UseStateHandle<String>,
    totp: UseStateHandle<Option<String>>,
    error: UseStateHandle<Option<ErrorMessage>>,
}

impl LoginForm {
    fn clone_state(&self) -> Self {
        Self {
            navigator: self.navigator.clone(),
            query: self.query.clone(),
            username: self.username.clone(),
            password: self.password.clone(),
            totp: self.totp.clone(),
            error: self.error.clone(),
        }
    }

    /// The TOTP token when the server asked for one, the password otherwise.
    fn method(&self) -> LoginMethod {
        match &*self.totp {
            Some(totp) => LoginMethod::Totp {
                token: totp.to_string(),
            },
            None => LoginMethod::Password {
                password: self.password.to_string(),
            },
        }
    }

    fn apply(self, login: ApiResult<LoginResponse>) {
        match login {
            ApiResult::Ok(LoginResponse::Success) => match self.query.redirect {
                Some(redirect) => self.navigator.replace(&redirect),
                None => self.navigator.push(&Route::Home),
            },
            ApiResult::Ok(LoginResponse::TotpRequired) => self.totp.set(Some(String::new())),
            ApiResult::Err(err) => self.error.set(Some(err)),
        }
    }
}

fn submit_callback(form: LoginForm) -> Callback<SubmitEvent> {
    Callback::from(move |event: SubmitEvent| {
        event.prevent_default();
        let form = form.clone_state();
        let method = form.method();
        let username = form.username.to_string();
        wasm_bindgen_futures::spawn_local(async move {
            form.apply(send_login(username, method).await);
        });
    })
}

/// Sends the sign-in request. The connection is insecure outside HTTPS.
async fn send_login(username: String, method: LoginMethod) -> ApiResult<LoginResponse> {
    let insecure = web_sys::window()
        .and_then(|window| window.location().protocol().ok())
        .unwrap_or_default()
        != "https:";
    Request::post(&format!("{API_ENDPOINT}/login"))
        .json(&LoginRequest {
            username,
            method,
            insecure,
        })
        .unwrap()
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap()
}
