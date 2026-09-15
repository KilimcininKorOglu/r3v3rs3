//! OpenAPI document of the admin API. Every handler has a `#[utoipa::path]` attribute, and the
//! `routes!` macro adds each handler to the router and to the document together.

use r3v3rs3_api::error::ErrorMessage;
use std::collections::BTreeMap;
use utoipa::openapi::response::{Response, ResponseBuilder};
use utoipa::openapi::security::{ApiKey, ApiKeyValue, SecurityScheme};
use utoipa::openapi::{Components, ContentBuilder, Ref, RefOr};
use utoipa::{IntoResponses, Modify, OpenApi};

/// Path of the OpenAPI document.
pub const OPENAPI_PATH: &str = "/api/openapi.json";

/// Path of the Swagger UI.
pub const DOCS_PATH: &str = "/api/docs";

/// Name of the security scheme. The `security` attribute of [`ApiDoc`] repeats it.
const SESSION_SCHEME: &str = "session";

/// Name of the cookie that `POST /api/login` sets.
const SESSION_COOKIE: &str = "token";

#[derive(OpenApi)]
#[openapi(
    info(
        title = "r3v3rs3 admin API",
        description = "The API that the r3v3rs3 WebUI uses. Sign in with `POST /api/login`. The response sets the `token` session cookie, and every other endpoint requires it.",
        contact(name = "r3v3rs3", url = "https://github.com/KilimcininKorOglu/r3v3rs3")
    ),
    modifiers(&SessionCookie),
    security(("session" = [])),
    components(schemas(ErrorMessage)),
    tags(
        (name = "auth", description = "Sign in and sign out."),
        (name = "events", description = "Server events."),
        (name = "config", description = "Application settings."),
        (name = "ports", description = "Listening ports."),
        (name = "proxies", description = "Proxies that route traffic from ports to upstream servers."),
        (name = "certs", description = "Server and root certificates."),
        (name = "acme", description = "ACME certificate requests."),
        (name = "logs", description = "Logs of ports, proxies and certificates."),
        (name = "app_info", description = "Build and path information of the server."),
        (name = "cdn", description = "CDN IP ranges for the client IP resolution."),
        (name = "discovery", description = "Service discovery providers."),
        (name = "cluster", description = "The cluster store that the nodes share."),
    )
)]
pub struct ApiDoc;

struct SessionCookie;

impl Modify for SessionCookie {
    fn modify(&self, openapi: &mut utoipa::openapi::OpenApi) {
        openapi
            .components
            .get_or_insert_with(Components::new)
            .add_security_scheme(
                SESSION_SCHEME,
                SecurityScheme::ApiKey(ApiKey::Cookie(ApiKeyValue::new(SESSION_COOKIE))),
            );
    }
}

/// Error responses that every endpoint can return.
pub struct ErrorResponses;

impl IntoResponses for ErrorResponses {
    fn responses() -> BTreeMap<String, RefOr<Response>> {
        error_responses(&[
            ("400", "The request is not valid."),
            ("401", "The session cookie is missing or expired."),
            ("500", "The server failed to handle the request."),
        ])
    }
}

/// Error response of the endpoints that address one resource by its id.
pub struct NotFoundResponse;

impl IntoResponses for NotFoundResponse {
    fn responses() -> BTreeMap<String, RefOr<Response>> {
        error_responses(&[("404", "No resource has this id.")])
    }
}

fn error_responses(entries: &[(&str, &str)]) -> BTreeMap<String, RefOr<Response>> {
    entries
        .iter()
        .map(|(status, description)| {
            let content = ContentBuilder::new()
                .schema(Some(Ref::from_schema_name("ErrorMessage")))
                .build();
            let response = ResponseBuilder::new()
                .description(*description)
                .content("application/json", content)
                .build();
            (status.to_string(), response.into())
        })
        .collect()
}
