use super::openapi::{ErrorResponses, NotFoundResponse};
use super::{AppError, AppState};
use crate::accounts::Caller;
use crate::accounts::Permission;
use crate::proxy::http::filter::{FilterResult, MatchRank, RequestFilter};
use crate::proxy::http::rewrite::Rewrite;
use crate::server::rpc::proxies::{
    AddProxy, DeleteProxy, GetProxy, GetProxyList, GetProxyStatus, PurgeProxyCache, UpdateProxy,
};
use axum::{
    Extension, Json,
    extract::{Path, Query, State},
};
use hyper::{Request, Uri};
use r3v3rs3_api::cache::CachePurge;
use r3v3rs3_api::fixed_response::FixedResponse;
use r3v3rs3_api::policy::AuthPolicy;
use r3v3rs3_api::redirect::RedirectRule;
use r3v3rs3_api::{
    id::ShortId,
    proxy::{
        HttpProxy, Proxy, ProxyDryRunRequest, ProxyDryRunResult, ProxyEntry, ProxyStatus, Route,
    },
};

/// Lists the proxies.
#[utoipa::path(
    get,
    path = "/",
    tag = "proxies",
    operation_id = "list_proxies",
    responses((status = 200, description = "The proxies.", body = Vec<ProxyEntry>), ErrorResponses)
)]
pub async fn list(
    State(state): State<AppState>,
    Extension(caller): Extension<Caller>,
) -> Result<Json<Box<Vec<ProxyEntry>>>, AppError> {
    Ok(Json(state.call(&caller, GetProxyList).await?))
}

/// Returns one proxy.
#[utoipa::path(
    get,
    path = "/{id}",
    tag = "proxies",
    operation_id = "get_proxy",
    params(("id" = ShortId, Path, description = "Proxy id.")),
    responses((status = 200, description = "The proxy.", body = ProxyEntry), NotFoundResponse, ErrorResponses)
)]
pub async fn get(
    State(state): State<AppState>,
    Extension(caller): Extension<Caller>,
    Path(id): Path<ShortId>,
) -> Result<Json<Box<ProxyEntry>>, AppError> {
    Ok(Json(state.call(&caller, GetProxy { id }).await?))
}

/// Returns the state of a proxy.
#[utoipa::path(
    get,
    path = "/{id}/status",
    tag = "proxies",
    operation_id = "get_proxy_status",
    params(("id" = ShortId, Path, description = "Proxy id.")),
    responses((status = 200, description = "The proxy status.", body = ProxyStatus), NotFoundResponse, ErrorResponses)
)]
pub async fn status(
    State(state): State<AppState>,
    Extension(caller): Extension<Caller>,
    Path(id): Path<ShortId>,
) -> Result<Json<Box<ProxyStatus>>, AppError> {
    Ok(Json(state.call(&caller, GetProxyStatus { id }).await?))
}

/// Deletes a proxy.
#[utoipa::path(
    delete,
    path = "/{id}",
    tag = "proxies",
    operation_id = "delete_proxy",
    params(("id" = ShortId, Path, description = "Proxy id.")),
    responses((status = 200, description = "The proxy is deleted."), NotFoundResponse, ErrorResponses)
)]
pub async fn delete(
    State(state): State<AppState>,
    Extension(caller): Extension<Caller>,
    Path(id): Path<ShortId>,
) -> Result<Json<Box<()>>, AppError> {
    Ok(Json(state.call(&caller, DeleteProxy { id }).await?))
}

#[derive(Debug, serde::Deserialize, utoipa::IntoParams)]
pub struct CachePurgeQuery {
    /// Optional URI path or prefix to purge.
    pub path: Option<String>,
}

/// Removes the stored responses from the HTTP cache of a proxy.
#[utoipa::path(
    delete,
    path = "/{id}/cache",
    tag = "proxies",
    operation_id = "purge_proxy_cache",
    params(
        ("id" = ShortId, Path, description = "Proxy id."),
        CachePurgeQuery,
    ),
    responses((status = 200, description = "The cache is empty."), NotFoundResponse, ErrorResponses)
)]
pub async fn purge_cache(
    State(state): State<AppState>,
    Extension(caller): Extension<Caller>,
    Path(id): Path<ShortId>,
    Query(query): Query<CachePurgeQuery>,
) -> Result<Json<Box<()>>, AppError> {
    Ok(Json(
        state
            .call(
                &caller,
                PurgeProxyCache {
                    id,
                    path: query.path,
                },
            )
            .await?,
    ))
}

/// Purges the HTTP cache of a proxy, either completely or for a specific path.
#[utoipa::path(
    post,
    path = "/{id}/cache/purge",
    tag = "proxies",
    operation_id = "purge_proxy_cache_post",
    params(("id" = ShortId, Path, description = "Proxy id.")),
    request_body(content = Option<CachePurge>, description = "Optional path to purge"),
    responses((status = 200, description = "The cache was purged."), NotFoundResponse, ErrorResponses)
)]
pub async fn purge_cache_post(
    State(state): State<AppState>,
    Extension(caller): Extension<Caller>,
    Path(id): Path<ShortId>,
    payload: Option<Json<CachePurge>>,
) -> Result<Json<Box<()>>, AppError> {
    let path = payload.and_then(|Json(p)| p.path);
    Ok(Json(
        state.call(&caller, PurgeProxyCache { id, path }).await?,
    ))
}

/// Adds a proxy.
#[utoipa::path(
    post,
    path = "/",
    tag = "proxies",
    operation_id = "add_proxy",
    request_body = Proxy,
    responses((status = 200, description = "The proxy is added."), ErrorResponses)
)]
pub async fn add(
    State(state): State<AppState>,
    Extension(caller): Extension<Caller>,
    Json(entry): Json<Proxy>,
) -> Result<Json<Box<()>>, AppError> {
    // An account with a proxy list sees the new proxy through its list.
    let owner = caller.proxies.is_some().then(|| caller.username.clone());
    Ok(Json(state.call(&caller, AddProxy { entry, owner }).await?))
}

/// Replaces the config of a proxy.
#[utoipa::path(
    put,
    path = "/{id}",
    tag = "proxies",
    operation_id = "update_proxy",
    params(("id" = ShortId, Path, description = "Proxy id.")),
    request_body = Proxy,
    responses((status = 200, description = "The proxy is updated."), NotFoundResponse, ErrorResponses)
)]
pub async fn put(
    State(state): State<AppState>,
    Extension(caller): Extension<Caller>,
    Path(id): Path<ShortId>,
    Json(entry): Json<Proxy>,
) -> Result<Json<Box<()>>, AppError> {
    let entry = (id, entry).into();
    Ok(Json(state.call(&caller, UpdateProxy { entry }).await?))
}

/// Returns the stream priority configuration of an HTTP proxy.
#[utoipa::path(
    get,
    path = "/{id}/priority",
    tag = "proxies",
    operation_id = "get_proxy_stream_priority",
    params(("id" = ShortId, Path, description = "Proxy id.")),
    responses((status = 200, description = "The stream priority configuration.", body = r3v3rs3_api::priority::StreamPriorityConfig), NotFoundResponse, ErrorResponses)
)]
pub async fn get_priority(
    State(state): State<AppState>,
    Extension(caller): Extension<Caller>,
    Path(id): Path<ShortId>,
) -> Result<Json<r3v3rs3_api::priority::StreamPriorityConfig>, AppError> {
    let entry: ProxyEntry = *state.call(&caller, GetProxy { id }).await?;
    match entry.proxy.kind {
        r3v3rs3_api::proxy::ProxyKind::Http(http) => Ok(Json(http.stream_priority)),
        _ => Err(r3v3rs3_api::error::Error::NotHttpProxy { id }.into()),
    }
}

/// Updates the stream priority configuration of an HTTP proxy.
#[utoipa::path(
    put,
    path = "/{id}/priority",
    tag = "proxies",
    operation_id = "set_proxy_stream_priority",
    params(("id" = ShortId, Path, description = "Proxy id.")),
    request_body = r3v3rs3_api::priority::SetStreamPriorityRequest,
    responses((status = 200, description = "Stream priority updated."), NotFoundResponse, ErrorResponses)
)]
pub async fn put_priority(
    State(state): State<AppState>,
    Extension(caller): Extension<Caller>,
    Path(id): Path<ShortId>,
    Json(payload): Json<r3v3rs3_api::priority::SetStreamPriorityRequest>,
) -> Result<Json<()>, AppError> {
    let mut entry: ProxyEntry = *state.call(&caller, GetProxy { id }).await?;
    match &mut entry.proxy.kind {
        r3v3rs3_api::proxy::ProxyKind::Http(http) => {
            http.stream_priority = payload.config;
        }
        _ => return Err(r3v3rs3_api::error::Error::NotHttpProxy { id }.into()),
    }
    state.call(&caller, UpdateProxy { entry }).await?;
    Ok(Json(()))
}

/// Simulates route matching and rewriting against unsaved proxy rules.
#[utoipa::path(
    post,
    path = "/dry-run",
    tag = "proxies",
    operation_id = "dry_run_proxy",
    request_body = ProxyDryRunRequest,
    responses((status = 200, description = "The dry-run result.", body = ProxyDryRunResult), ErrorResponses)
)]
pub async fn dry_run(
    Extension(caller): Extension<Caller>,
    Json(req): Json<ProxyDryRunRequest>,
) -> Result<Json<ProxyDryRunResult>, AppError> {
    caller.authorize(Permission::Read)?;
    Ok(Json(simulate_dry_run(req)))
}

pub(crate) fn simulate_dry_run(req: ProxyDryRunRequest) -> ProxyDryRunResult {
    let (host, path, query) = parse_test_url(&req.url);
    let dummy_req = build_dummy_request(&path, query.as_deref(), req.method.as_deref());
    if let Some(redirect_res) = check_redirects(&req.proxy.redirects, host.as_deref(), &dummy_req) {
        return redirect_res;
    }
    if let Some((index, route, filter_result)) =
        find_best_route(&req.proxy, &dummy_req, host.as_deref())
    {
        return build_route_dry_run_result(
            &req.proxy,
            index,
            route,
            filter_result,
            query.as_deref(),
            req.client_ip,
        );
    }
    ProxyDryRunResult {
        matched: false,
        route_index: None,
        route_path: None,
        upstream_url: None,
        upstream_servers: Vec::new(),
        redirect_url: None,
        redirect_status: None,
        disallowed_by_ip: false,
        auth_required: None,
        explanation: no_match_explanation(&req.proxy, host.as_deref(), &path),
    }
}

fn parse_test_url(url_str: &str) -> (Option<String>, String, Option<String>) {
    let url_str = url_str.trim();
    let parsed = if url_str.starts_with("http://") || url_str.starts_with("https://") {
        url::Url::parse(url_str).ok()
    } else if url_str.starts_with('/') {
        url::Url::parse(&format!("http://localhost{}", url_str)).ok()
    } else {
        url::Url::parse(&format!("http://{}", url_str)).ok()
    };
    if let Some(u) = parsed {
        let is_implicit = url_str.starts_with('/') && u.host_str() == Some("localhost");
        let host = if is_implicit {
            None
        } else {
            u.host_str().map(str::to_string)
        };
        (host, u.path().to_string(), u.query().map(str::to_string))
    } else {
        (None, url_str.to_string(), None)
    }
}

fn build_dummy_request(path: &str, query: Option<&str>, method: Option<&str>) -> Request<()> {
    let path_and_query = match query {
        Some(q) => format!("{path}?{q}"),
        None => path.to_string(),
    };
    let uri: Uri = path_and_query
        .parse()
        .unwrap_or_else(|_| Uri::from_static("/"));
    let mut builder = Request::builder().uri(uri);
    if let Some(m) = method
        && let Ok(method) = hyper::Method::from_bytes(m.as_bytes())
    {
        builder = builder.method(method);
    }
    builder
        .body(())
        .unwrap_or_else(|_| Request::get("/").body(()).unwrap())
}

fn check_redirects(
    redirects: &[RedirectRule],
    host: Option<&str>,
    req: &Request<()>,
) -> Option<ProxyDryRunResult> {
    let res = crate::proxy::http::redirect::redirect_rule(redirects, host, req)?;
    let loc = res
        .headers()
        .get(hyper::header::LOCATION)
        .and_then(|v| v.to_str().ok())
        .unwrap_or_default()
        .to_string();
    let status = res.status().as_u16();
    Some(ProxyDryRunResult {
        matched: true,
        route_index: None,
        route_path: None,
        upstream_url: None,
        upstream_servers: Vec::new(),
        redirect_url: Some(loc.clone()),
        redirect_status: Some(status),
        disallowed_by_ip: false,
        auth_required: None,
        explanation: format!("Matched redirect rule ({status} -> {loc})"),
    })
}

fn find_best_route<'a>(
    proxy: &'a HttpProxy,
    req: &Request<()>,
    host: Option<&str>,
) -> Option<(usize, &'a Route, FilterResult)> {
    let mut best: Option<(usize, MatchRank, &'a Route)> = None;
    for (index, route) in proxy.routes.iter().enumerate() {
        let filter = RequestFilter::new(&proxy.vhosts, route);
        if let Some(rank) = filter.rank(req, host)
            && best
                .as_ref()
                .is_none_or(|(_, best_rank, _)| rank > *best_rank)
        {
            best = Some((index, rank, route));
        }
    }
    let (index, _, route) = best?;
    let filter = RequestFilter::new(&proxy.vhosts, route);
    Some((index, route, filter.result(req)))
}

fn auth_description(auth: &AuthPolicy) -> Option<String> {
    match auth {
        AuthPolicy::None => None,
        AuthPolicy::Basic(_) => Some("HTTP Basic Auth".to_string()),
        AuthPolicy::Bearer(_) => Some("Bearer Token".to_string()),
        AuthPolicy::Forward(f) => Some(format!("Forward Auth ({})", f.url)),
        AuthPolicy::Session => Some("Panel Session".to_string()),
        AuthPolicy::Oidc(o) => Some(format!(
            "OpenID Connect ({})",
            if !o.issuer_url.is_empty() {
                &o.issuer_url
            } else {
                &o.client_id
            }
        )),
    }
}

fn resolve_upstream_targets(
    route: &Route,
    filter_result: &FilterResult,
    query: Option<&str>,
) -> (Option<String>, Vec<String>) {
    let base_path: String = route
        .path
        .split('/')
        .filter(|seg| !seg.is_empty())
        .map(|seg| format!("/{seg}"))
        .collect();
    let rewrite = Rewrite::new(&route.rewrite, &base_path);
    let segments = rewrite.apply(filter_result.path_segments.clone());
    let mut servers = Vec::new();
    for server in &route.servers {
        let u = crate::proxy::http::pool::upstream_url(&server.url.0, &segments, query);
        servers.push(u.to_string());
    }
    let first = servers.first().cloned();
    (first, servers)
}

fn format_route_explanation(
    index: usize,
    route: &Route,
    upstream_url: &Option<String>,
    redirect_url: &Option<String>,
    disallowed_by_ip: bool,
    auth_required: &Option<String>,
) -> String {
    let mut exp = format!("Matched route #{} with pattern '{}'", index + 1, route.path);
    if let Some(target) = upstream_url {
        exp.push_str(&format!(" -> Upstream: {target}"));
    } else if let Some(loc) = redirect_url {
        exp.push_str(&format!(" -> Fixed redirect: {loc}"));
    } else if let Some(FixedResponse::Status(s)) = &route.response {
        exp.push_str(&format!(" -> Fixed status: {}", s.status));
    }
    if disallowed_by_ip {
        exp.push_str(" [BLOCKED by IP filter]");
    }
    if let Some(auth_name) = auth_required {
        exp.push_str(&format!(" [Requires {auth_name}]"));
    }
    exp
}

fn build_route_dry_run_result(
    proxy: &HttpProxy,
    index: usize,
    route: &Route,
    filter_result: FilterResult,
    query: Option<&str>,
    client_ip: Option<std::net::IpAddr>,
) -> ProxyDryRunResult {
    let ip_filter = route.ip_filter.as_ref().unwrap_or(&proxy.ip_filter);
    let disallowed_by_ip = client_ip.is_some_and(|ip| !ip_filter.allows(ip));
    let auth = route.auth.as_ref().unwrap_or(&proxy.auth);
    let auth_required = auth_description(auth);
    let (upstream_url, upstream_servers) = resolve_upstream_targets(route, &filter_result, query);
    let (redirect_url, redirect_status) = match &route.response {
        Some(FixedResponse::Redirect(r)) => (Some(r.target.clone()), Some(r.status.code())),
        _ => (None, None),
    };
    let explanation = format_route_explanation(
        index,
        route,
        &upstream_url,
        &redirect_url,
        disallowed_by_ip,
        &auth_required,
    );
    ProxyDryRunResult {
        matched: true,
        route_index: Some(index),
        route_path: Some(route.path.clone()),
        upstream_url,
        upstream_servers,
        redirect_url,
        redirect_status,
        disallowed_by_ip,
        auth_required,
        explanation,
    }
}

fn no_match_explanation(proxy: &HttpProxy, host: Option<&str>, path: &str) -> String {
    if proxy.routes.is_empty() {
        "Proxy has no routes configured.".to_string()
    } else if let Some(h) = host {
        format!(
            "No route matched host '{h}' and path '{path}'. Check virtual hosts or route paths."
        )
    } else {
        format!("No route matched path '{path}'. Check virtual hosts or route paths.")
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use r3v3rs3_api::proxy::Server;

    #[test]
    fn dry_run_matches_route_and_rewrites_path() {
        let proxy = HttpProxy {
            vhosts: vec!["app.example.com".parse().unwrap()],
            routes: vec![
                Route {
                    path: "/api".to_string(),
                    servers: vec![Server::new("http://backend:8080/v1".parse().unwrap())],
                    ..Default::default()
                },
                Route {
                    path: "/".to_string(),
                    servers: vec![Server::new("http://frontend:3000".parse().unwrap())],
                    ..Default::default()
                },
            ],
            ..Default::default()
        };

        let req = ProxyDryRunRequest {
            proxy: proxy.clone(),
            url: "https://app.example.com/api/users?page=2".to_string(),
            method: Some("GET".to_string()),
            client_ip: None,
        };
        let res = simulate_dry_run(req);
        assert!(res.matched);
        assert_eq!(res.route_index, Some(0));
        assert_eq!(res.route_path.as_deref(), Some("/api"));
        assert_eq!(
            res.upstream_url.as_deref(),
            Some("http://backend:8080/v1/users?page=2")
        );
        assert!(!res.disallowed_by_ip);

        // Root path matches route #1
        let req2 = ProxyDryRunRequest {
            proxy: proxy.clone(),
            url: "https://app.example.com/about".to_string(),
            method: Some("GET".to_string()),
            client_ip: None,
        };
        let res2 = simulate_dry_run(req2);
        assert!(res2.matched);
        assert_eq!(res2.route_index, Some(1));
        assert_eq!(
            res2.upstream_url.as_deref(),
            Some("http://frontend:3000/about")
        );

        // Unmatched vhost
        let req3 = ProxyDryRunRequest {
            proxy,
            url: "https://other.example.com/api".to_string(),
            method: Some("GET".to_string()),
            client_ip: None,
        };
        let res3 = simulate_dry_run(req3);
        assert!(!res3.matched);
    }

    #[test]
    fn dry_run_detects_ip_disallow() {
        use r3v3rs3_api::policy::IpFilter;
        let proxy = HttpProxy {
            ip_filter: IpFilter {
                allow: vec!["10.0.0.0/8".parse().unwrap()],
                ..Default::default()
            },
            routes: vec![Route {
                path: "/".to_string(),
                servers: vec![Server::new("http://backend:8080".parse().unwrap())],
                ..Default::default()
            }],
            ..Default::default()
        };
        let req = ProxyDryRunRequest {
            proxy,
            url: "http://localhost/".to_string(),
            method: Some("GET".to_string()),
            client_ip: Some("192.168.1.1".parse().unwrap()),
        };
        let res = simulate_dry_run(req);
        assert!(res.matched);
        assert!(res.disallowed_by_ip);
    }

    #[test]
    fn dry_run_matches_redirects_before_routes() {
        let rule =
            RedirectRule::parse_line("301 ^example\\.com/old/(.*)$ https://example.com/new/${1}")
                .unwrap();
        let proxy = HttpProxy {
            redirects: vec![rule],
            routes: vec![Route {
                path: "/".to_string(),
                servers: vec![Server::new("http://backend:8080".parse().unwrap())],
                ..Default::default()
            }],
            ..Default::default()
        };
        let req = ProxyDryRunRequest {
            proxy,
            url: "https://example.com/old/docs".to_string(),
            method: Some("GET".to_string()),
            client_ip: None,
        };
        let res = simulate_dry_run(req);
        assert!(res.matched);
        assert_eq!(res.redirect_status, Some(301));
        assert_eq!(
            res.redirect_url.as_deref(),
            Some("https://example.com/new/docs")
        );
    }
}
