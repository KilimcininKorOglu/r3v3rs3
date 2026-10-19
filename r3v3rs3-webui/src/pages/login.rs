use crate::{
    API_ENDPOINT,
    auth::{LoginQuery, test_token},
    components::{language_menu::LanguageMenu, theme_menu::ThemeMenu},
    i18n::use_locale,
};
use gloo_events::EventListener;
use gloo_net::http::Request;
use r3v3rs3_api::{
    auth::{LoginMethod, LoginRequest, LoginResponse},
    error::ErrorMessage,
    i18n::Locale,
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

#[cfg(target_arch = "wasm32")]
#[wasm_bindgen(module = "/js/webauthn.js")]
extern "C" {
    #[wasm_bindgen(js_name = isWebAuthnSupported)]
    fn is_web_authn_supported() -> bool;
    #[wasm_bindgen(js_name = getPasskey)]
    fn get_passkey(challenge_b64: &str, rp_id: &str) -> js_sys::Promise;
}

#[cfg(not(target_arch = "wasm32"))]
fn is_web_authn_supported() -> bool {
    false
}

const MENU_BUTTON_CLASS: [&str; 6] = [
    "p-2",
    "rounded-md",
    "text-neutral-600",
    "dark:text-neutral-300",
    "hover:bg-neutral-200",
    "dark:hover:bg-neutral-700",
];

fn login_error_view(message: Option<String>) -> Html {
    let Some(message) = message else {
        return html!();
    };
    html! {
        <div class="bg-red-100 border border-red-400 text-red-700 dark:bg-red-950 dark:border-red-800 dark:text-red-300 px-4 py-3 rounded relative mb-4" role="alert">
            <span class="block sm:inline">{message}</span>
        </div>
    }
}

fn passkey_login_button(locale: Locale, on_click: Callback<MouseEvent>) -> Html {
    if !is_web_authn_supported() {
        return html!();
    }
    html! {
        <>
            <div class="relative my-4 flex items-center justify-center">
                <div class="border-t border-neutral-300 dark:border-neutral-700 w-full"></div>
                <span class="bg-white dark:bg-neutral-900 px-2 text-xs text-neutral-400 absolute">{locale.t("login.or")}</span>
            </div>
            <button type="button" class="w-full text-neutral-800 dark:text-neutral-100 font-semibold border border-neutral-300 dark:border-neutral-700 py-2.5 rounded-md hover:bg-neutral-100 dark:hover:bg-neutral-800 transition duration-300 flex items-center justify-center gap-2 cursor-pointer" onclick={on_click}>
                <svg class="w-5 h-5 text-neutral-600 dark:text-neutral-400" fill="none" stroke="currentColor" viewBox="0 0 24 24"><path stroke-linecap="round" stroke-linejoin="round" stroke-width="2" d="M15 7a2 2 0 012 2m4 0a6 6 0 01-7.743 5.743L11 17H9v2H7v2H4a1 1 0 01-1-1v-2.586a1 1 0 01.293-.707l5.964-5.964A6 6 0 1121 9z"></path></svg>
                {locale.t("login.sign_in_with_passkey")}
            </button>
        </>
    }
}

#[function_component(Login)]
pub fn login() -> Html {
    let Some(navigator) = use_navigator() else {
        return html!();
    };
    let locale = use_locale();

    use_logout_on_hide();

    let Some(location) = use_location() else {
        return html!();
    };
    let query = location.query::<LoginQuery>().unwrap_or_default();

    let username = use_state(String::new);
    let password = use_state(String::new);
    let totp = use_state(|| Option::<String>::None);
    let error: UseStateHandle<Option<ErrorMessage>> = use_state(|| Option::<ErrorMessage>::None);

    let oninput_username = text_oninput(username.clone());
    let oninput_password = text_oninput(password.clone());
    let oninput_totp = totp_oninput(totp.clone());

    let onsubmit = submit_callback(LoginForm {
        navigator: navigator.clone(),
        query: query.clone(),
        username: username.clone(),
        password: password.clone(),
        totp: totp.clone(),
        error: error.clone(),
    });

    let on_passkey_login = {
        let form = LoginForm {
            navigator,
            query,
            username: username.clone(),
            password: password.clone(),
            totp: totp.clone(),
            error: error.clone(),
        };
        Callback::from(move |_| {
            let form = form.clone_state();
            let user = form.username.to_string();
            wasm_bindgen_futures::spawn_local(async move {
                match passkey_login(locale, user).await {
                    Ok(resp) => form.apply(resp),
                    Err(err) => form.error.set(Some(ErrorMessage {
                        message: err,
                        error: None,
                    })),
                }
            });
        })
    };

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

            { login_error_view(error_text) }

            if let Some(totp) = &*totp {
                <label class="mr-4 text-neutral-700 dark:text-neutral-200 font-bold inline-block mb-2" for="name">{locale.t("login.one_time_password")}</label>
                <input type="number" class="border bg-white dark:bg-neutral-800 dark:border-neutral-600 py-2 px-4 w-full outline-none focus:ring-2 focus:ring-neutral-400 rounded" autocomplete="one-time-code" oninput={oninput_totp} />
                <input type="submit" class="w-full mt-4 text-neutral-50 font-bold bg-neutral-800 dark:bg-neutral-900 py-3 rounded-md hover:bg-neutral-600 transition duration-300" value={locale.t("login.continue")} disabled={totp.is_empty()} />
            } else {
                <div class="mb-4">
                    <label class="mr-4 text-neutral-700 dark:text-neutral-200 font-bold inline-block mb-2" for="name">{locale.t("login.username")}</label>
                    <input type="text" class="border bg-white dark:bg-neutral-800 dark:border-neutral-600 py-2 px-4 w-full outline-none focus:ring-2 focus:ring-neutral-400 rounded" autocapitalize="off" autocomplete="username" autofocus={true} oninput={oninput_username} />
                </div>
                <label class="mr-4 text-neutral-700 dark:text-neutral-200 font-bold inline-block mb-2" for="name">{locale.t("login.password")}</label>
                <input type="password" class="border bg-white dark:bg-neutral-800 dark:border-neutral-600 py-2 px-4 w-full outline-none focus:ring-2 focus:ring-neutral-400 rounded" autocomplete="current-password" oninput={oninput_password} />
                <input type="submit" class="w-full mt-4 text-neutral-50 font-bold bg-neutral-800 dark:bg-neutral-900 py-3 rounded-md hover:bg-neutral-600 transition duration-300" value={locale.t("login.submit")} disabled={username.is_empty() || password.is_empty()} />
                { passkey_login_button(locale, on_passkey_login) }
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
            ApiResult::Ok(LoginResponse::Success) => {
                if self.query.is_oauth_redirect() {
                    #[cfg(target_arch = "wasm32")]
                    if let Some(target) = &self.query.redirect
                        && let Some(window) = web_sys::window()
                    {
                        let _ = window.location().assign(target);
                        return;
                    }
                }
                self.navigator.replace(&self.query.route());
            }
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
    let request = Request::post(&format!("{API_ENDPOINT}/login"));
    let request = match request.json(&LoginRequest {
        username,
        method,
        insecure,
    }) {
        Ok(request) => request,
        Err(err) => {
            return ApiResult::Err(ErrorMessage {
                message: err.to_string(),
                error: None,
            });
        }
    };
    let response = match request.send().await {
        Ok(response) => response,
        Err(err) => {
            return ApiResult::Err(ErrorMessage {
                message: err.to_string(),
                error: None,
            });
        }
    };
    match response.json::<LoginResponse>().await {
        Ok(response) => ApiResult::Ok(response),
        Err(err) => ApiResult::Err(ErrorMessage {
            message: err.to_string(),
            error: None,
        }),
    }
}

#[derive(Deserialize)]
struct PasskeyAssertion {
    credential_id: String,
    authenticator_data: String,
    client_data_json: String,
    signature: String,
}

#[cfg(target_arch = "wasm32")]
fn js_error_message(err: wasm_bindgen::JsValue) -> String {
    if let Some(s) = err.as_string() {
        return s;
    }
    if let Ok(msg) = js_sys::Reflect::get(&err, &wasm_bindgen::JsValue::from_str("message"))
        && let Some(s) = msg.as_string()
    {
        return s;
    }
    "Passkey authentication failed".to_string()
}

#[cfg(target_arch = "wasm32")]
fn format_passkey_error(locale: Locale, err: &str) -> String {
    if err.contains("PASSKEY_ORIGIN_IP")
        || err.contains("127.0.0.1")
        || err.contains("cannot be used with the current origin")
        || err.contains("relying party ID")
    {
        locale.t("accounts.passkey_origin_error").to_string()
    } else if err.contains("PASSKEY_ORIGIN_IP_OTHER") {
        locale.t("accounts.passkey_ip_error").to_string()
    } else {
        err.to_string()
    }
}

async fn fetch_passkey_challenge() -> Result<r3v3rs3_api::auth::PasskeyChallenge, String> {
    let response = Request::get(&format!("{API_ENDPOINT}/auth/passkey/challenge"))
        .send()
        .await
        .map_err(|e| e.to_string())?;
    if !response.ok() {
        return Err("failed to get passkey challenge".into());
    }
    response
        .json::<r3v3rs3_api::auth::PasskeyChallenge>()
        .await
        .map_err(|e| e.to_string())
}

async fn passkey_login(
    locale: Locale,
    username: String,
) -> Result<ApiResult<LoginResponse>, String> {
    let challenge = fetch_passkey_challenge().await?;
    #[cfg(target_arch = "wasm32")]
    let assertion = {
        let promise = get_passkey(&challenge.challenge, &challenge.rp_id);
        let js_val = wasm_bindgen_futures::JsFuture::from(promise)
            .await
            .map_err(|err| format_passkey_error(locale, &js_error_message(err)))?;
        let json_str = js_val
            .as_string()
            .ok_or_else(|| "Invalid passkey response".to_string())?;
        serde_json::from_str::<PasskeyAssertion>(&json_str).map_err(|e| e.to_string())?
    };
    #[cfg(not(target_arch = "wasm32"))]
    let assertion = {
        let _ = (&challenge, &locale);
        PasskeyAssertion {
            credential_id: String::new(),
            authenticator_data: String::new(),
            client_data_json: String::new(),
            signature: String::new(),
        }
    };

    let method = LoginMethod::Passkey {
        credential_id: assertion.credential_id,
        authenticator_data: assertion.authenticator_data,
        client_data_json: assertion.client_data_json,
        signature: assertion.signature,
    };
    Ok(send_login(username, method).await)
}
