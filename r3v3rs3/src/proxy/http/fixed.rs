//! Answers a route with its fixed redirect or status instead of an upstream server.

use bytes::Bytes;
use http_body_util::Full;
use hyper::header::{CONTENT_TYPE, HeaderValue, LOCATION};
use hyper::{Request, Response, StatusCode};
use r3v3rs3_api::fixed_response::{FixedRedirect, FixedResponse, FixedStatus};
use tracing::warn;

/// The response of a route with a fixed response.
pub fn respond<B>(fixed: &FixedResponse, req: &Request<B>) -> Response<Full<Bytes>> {
    match fixed {
        FixedResponse::Redirect(redirect) => redirect_response(redirect, req),
        FixedResponse::Status(status) => status_response(status),
    }
}

/// The redirect to the target. With `preserve_path`, the path and the query of the request follow
/// the target.
fn redirect_response<B>(redirect: &FixedRedirect, req: &Request<B>) -> Response<Full<Bytes>> {
    let location = if redirect.preserve_path {
        let path = req.uri().path_and_query().map_or("/", |path| path.as_str());
        format!("{}{path}", redirect.target.trim_end_matches('/'))
    } else {
        redirect.target.clone()
    };
    let Ok(value) = HeaderValue::from_str(&location) else {
        warn!(
            location,
            "the fixed redirect target is not a valid header value"
        );
        return response(StatusCode::INTERNAL_SERVER_ERROR, Bytes::new());
    };
    let mut res = response(status_code(redirect.status.code()), Bytes::new());
    res.headers_mut().insert(LOCATION, value);
    res
}

fn status_response(fixed: &FixedStatus) -> Response<Full<Bytes>> {
    let mut res = response(status_code(fixed.status), Bytes::from(fixed.body.clone()));
    if !fixed.body.is_empty() {
        let text = HeaderValue::from_static("text/plain; charset=utf-8");
        res.headers_mut().insert(CONTENT_TYPE, text);
    }
    res
}

/// The status code of a validated fixed response. An invalid code logs a warning and gives 500.
fn status_code(code: u16) -> StatusCode {
    StatusCode::from_u16(code).unwrap_or_else(|err| {
        warn!(code, %err, "the fixed response status is not a valid status code");
        StatusCode::INTERNAL_SERVER_ERROR
    })
}

fn response(status: StatusCode, body: Bytes) -> Response<Full<Bytes>> {
    let mut res = Response::new(Full::new(body));
    *res.status_mut() = status;
    res
}

#[cfg(test)]
mod tests {
    use super::*;
    use r3v3rs3_api::redirect::RedirectStatus;

    fn location(target: &str, preserve_path: bool, path: &str) -> (u16, String) {
        let fixed = FixedResponse::Redirect(FixedRedirect {
            target: target.into(),
            status: RedirectStatus::PermanentRedirect,
            preserve_path,
        });
        let req = Request::get(path).body(()).unwrap();
        let res = respond(&fixed, &req);
        let location = res.headers()[LOCATION].to_str().unwrap().to_string();
        (res.status().as_u16(), location)
    }

    #[test]
    fn a_redirect_keeps_the_request_path_only_when_asked() {
        assert_eq!(
            location("https://new.test/", true, "/a/b?c=1"),
            (308, "https://new.test/a/b?c=1".to_string())
        );
        assert_eq!(
            location("https://new.test/landing", false, "/a"),
            (308, "https://new.test/landing".to_string())
        );
    }

    #[test]
    fn a_status_response_sends_its_body_as_plain_text() {
        let req = Request::get("/").body(()).unwrap();
        let body = FixedResponse::Status(FixedStatus {
            status: 404,
            body: "not here".into(),
        });
        let res = respond(&body, &req);
        assert_eq!(res.status(), StatusCode::NOT_FOUND);
        assert_eq!(res.headers()[CONTENT_TYPE], "text/plain; charset=utf-8");

        let empty = FixedResponse::Status(FixedStatus {
            status: 503,
            body: String::new(),
        });
        let res = respond(&empty, &req);
        assert_eq!(res.status(), StatusCode::SERVICE_UNAVAILABLE);
        assert!(!res.headers().contains_key(CONTENT_TYPE));
    }
}
