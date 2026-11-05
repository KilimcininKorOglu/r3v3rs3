use super::cache_share::{CacheShare, SharedCacheStore, SharedResponse};
use super::compression::{header_items, header_number};
use super::pool::Upstream;
use crate::clock::unix_ms;
use crate::proxy::registry::WeakRegistry;
use bytes::{Bytes, BytesMut};
use http_body_util::{BodyExt, Full, combinators::BoxBody};
use hyper::{
    HeaderMap, Method, Request, Response, StatusCode,
    body::{Body, Frame, SizeHint},
    header::{
        ACCEPT_ENCODING, AGE, CACHE_CONTROL, CONTENT_ENCODING, CONTENT_LANGUAGE, CONTENT_LENGTH,
        CONTENT_RANGE, CONTENT_TYPE, DATE, ETAG, EXPIRES, HeaderName, IF_MODIFIED_SINCE,
        IF_NONE_MATCH, LAST_MODIFIED, PRAGMA, RANGE, SET_COOKIE, TRANSFER_ENCODING, UPGRADE, VARY,
    },
    http::HeaderValue,
};
use moka::{Expiry, sync::Cache};
use pin_project_lite::pin_project;
use r3v3rs3_api::{cache::CacheConfig, id::ShortId};
use std::{
    collections::{HashMap, HashSet},
    fmt,
    pin::Pin,
    sync::{Arc, Mutex, MutexGuard, OnceLock, PoisonError},
    task::{Context, Poll, ready},
    time::{Duration, Instant, SystemTime},
};
use tokio::sync::Notify;
use tracing::{debug, warn};

type ProxyBody = BoxBody<Bytes, anyhow::Error>;

const X_CACHE: HeaderName = HeaderName::from_static("x-cache");

/// A stale response with a validator stays stored this long, so a later request can revalidate it.
const REVALIDATION_WINDOW: Duration = Duration::from_secs(3600);

/// Status codes that are cacheable by default (RFC 9110 15.1).
const CACHEABLE_STATUSES: [u16; 11] = [200, 203, 204, 300, 301, 308, 404, 405, 410, 414, 501];

/// The stored responses of one proxy.
pub struct HttpCache {
    config: CacheConfig,
    entries: Cache<String, Arc<CachedResponse>>,
    /// The responses that the nodes of a cluster share.
    share: Option<CacheShare>,
    revalidating: Mutex<HashSet<String>>,
    in_flight: Mutex<HashMap<String, Arc<Notify>>>,
}

impl fmt::Debug for HttpCache {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("HttpCache")
            .field("config", &self.config)
            .field("entries", &self.entries.entry_count())
            .field("share", &self.share)
            .finish()
    }
}

/// Extracts the URI path and query from a cache key (`"{proto} {host} {path}"` or `"{host} {path}"`).
fn extract_key_path(key: &str) -> &str {
    let mut parts = key.splitn(3, ' ');
    match (parts.next(), parts.next(), parts.next()) {
        (Some(_proto), Some(_host), Some(path)) => path,
        (Some(_host), Some(path), None) => path,
        _ => key,
    }
}

/// Extracts the host from a cache key (`"{proto} {host} {path}"` or `"{host} {path}"`).
fn extract_key_host(key: &str) -> &str {
    let mut parts = key.splitn(3, ' ');
    match (parts.next(), parts.next(), parts.next()) {
        (Some(_proto), Some(host), Some(_path)) => host,
        (Some(host), Some(_path), None) => host,
        _ => "",
    }
}

impl HttpCache {
    fn new(config: CacheConfig, share: Option<CacheShare>) -> Self {
        let default_swr = config.stale_while_revalidate;
        let entries = Cache::builder()
            .weigher(|key: &String, entry: &Arc<CachedResponse>| {
                u32::try_from(key.len() + entry.weight()).unwrap_or(u32::MAX)
            })
            .max_capacity(config.max_size)
            .expire_after(EntryExpiry { default_swr })
            .build();
        Self {
            config,
            entries,
            share,
            revalidating: Mutex::new(HashSet::new()),
            in_flight: Mutex::new(HashMap::new()),
        }
    }

    fn start_flight(self: &Arc<Self>, key: &str) -> Flight {
        let mut flights = lock(&self.in_flight);
        if let Some(notify) = flights.get(key) {
            Flight::Follower(notify.clone())
        } else {
            let notify = Arc::new(Notify::new());
            flights.insert(key.to_string(), notify.clone());
            Flight::Leader(FlightGuard {
                cache: self.clone(),
                key: key.to_string(),
                notify,
                completed: false,
            })
        }
    }

    /// Removes every stored response whose URI path equals or starts with `path`.
    pub fn purge_path(&self, path: &str) -> usize {
        let trimmed = path.trim();
        let mut count = 0;
        for (key, _) in self.entries.iter() {
            let key_path = extract_key_path(&key);
            if key_path == trimmed || key_path.starts_with(trimmed) {
                self.entries.invalidate(&*key);
                count += 1;
            }
        }
        count
    }

    /// Invalidates stored responses matching the given request URI.
    pub fn invalidate_target(&self, key: &str, host: &str, path: &str) {
        self.invalidate_key(key);
        if !host.is_empty() && !path.is_empty() {
            self.invalidate_key(&format!("http {host} {path}"));
            self.invalidate_key(&format!("https {host} {path}"));
            self.invalidate_key(&format!("{host} {path}"));
        }
    }

    /// Invalidates a single cache key locally and in the shared store.
    pub fn invalidate_key(&self, key: &str) {
        self.entries.invalidate(key);
        if let Some(share) = &self.share {
            share.invalidate(key);
        }
    }

    fn mark_revalidating(&self, key: &str) -> bool {
        lock(&self.revalidating).insert(key.to_string())
    }

    fn finish_revalidating(&self, key: &str) {
        lock(&self.revalidating).remove(key);
    }

    fn refresh_entry(
        &self,
        key: &str,
        entry: &CachedResponse,
        headers: &HeaderMap,
    ) -> Arc<CachedResponse> {
        let refreshed = Arc::new(merge_304_headers(
            entry,
            headers,
            self.config.default_ttl,
            true,
        ));
        self.entries.insert(key.to_string(), refreshed.clone());
        refreshed
    }

    fn store_entry(&self, key: &str, head: StoredHead, body: Bytes) -> Arc<CachedResponse> {
        let entry = Arc::new(CachedResponse {
            head,
            body,
            stored_at: Instant::now(),
        });
        self.entries.insert(key.to_string(), entry.clone());
        entry
    }
}

/// The HTTP caches of one server.
#[derive(Default)]
pub struct CacheRegistry {
    caches: WeakRegistry<ShortId, HttpCache>,
    store: OnceLock<Arc<dyn SharedCacheStore>>,
    /// The Unix time in milliseconds of the last purge of each proxy.
    purges: Mutex<HashMap<ShortId, u64>>,
}

impl fmt::Debug for CacheRegistry {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("CacheRegistry")
            .field("caches", &self.caches)
            .field("shared", &self.store.get().is_some())
            .finish()
    }
}

impl CacheRegistry {
    /// Returns the cache of the proxy, or `None` when the cache is disabled. The existing cache is
    /// reused while its configuration is unchanged, so a configuration reload keeps the responses.
    pub fn cache_for(&self, id: ShortId, config: &CacheConfig) -> Option<Arc<HttpCache>> {
        if config.is_disabled() {
            return None;
        }
        let store = self.store.get();
        Some(self.caches.get_or_create(
            id,
            |existing| existing.config == *config && existing.share.is_some() == store.is_some(),
            || {
                let share = store.map(|store| {
                    CacheShare::new(id, store.clone(), self.purged_at(id), config.max_size)
                });
                Arc::new(HttpCache::new(config.clone(), share))
            },
        ))
    }

    /// Shares the stored responses with the other nodes of a cluster. Call it before the first
    /// cache is created, because a cache keeps the mode that it started with.
    pub fn start_sharing(&self, store: Arc<dyn SharedCacheStore>) {
        if self.store.set(store).is_err() {
            debug!("the HTTP caches already share their responses");
        }
    }

    /// Removes every stored response of the proxy. Returns the purge time in Unix milliseconds.
    pub fn purge(&self, id: ShortId) -> u64 {
        let at = unix_ms();
        self.purge_at(id, at);
        at
    }

    /// Removes stored responses of the proxy that match or begin with the path. Returns the purge time in Unix milliseconds.
    pub fn purge_path(&self, id: ShortId, path: &str) -> u64 {
        if let Some(cache) = self.caches.get(&id) {
            cache.purge_path(path);
        }
        unix_ms()
    }

    /// Purges the cache of each proxy with a later purge than the last purge on this server.
    pub fn apply_purges(&self, purges: HashMap<ShortId, u64>) {
        for (id, at) in purges {
            if self.purged_at(id) < at {
                self.purge_at(id, at);
            }
        }
    }

    fn purge_at(&self, id: ShortId, at: u64) {
        {
            let mut purges = lock(&self.purges);
            let last = purges.entry(id).or_default();
            *last = (*last).max(at);
        }
        if let Some(cache) = self.caches.get(&id) {
            cache.entries.invalidate_all();
            if let Some(share) = &cache.share {
                share.purge(at);
            }
        }
    }

    fn purged_at(&self, id: ShortId) -> u64 {
        lock(&self.purges).get(&id).copied().unwrap_or_default()
    }
}

fn lock<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex.lock().unwrap_or_else(PoisonError::into_inner)
}

#[derive(Debug, Clone)]
struct StoredHead {
    status: StatusCode,
    headers: HeaderMap,
    /// Request header values of the header names in `Vary`.
    vary: Vec<(HeaderName, Option<HeaderValue>)>,
    /// Age of the response when r3v3rs3 received it.
    initial_age: Duration,
    freshness: Duration,
}

#[derive(Debug)]
struct CachedResponse {
    head: StoredHead,
    body: Bytes,
    stored_at: Instant,
}

impl CachedResponse {
    fn weight(&self) -> usize {
        let headers: usize = self
            .head
            .headers
            .iter()
            .map(|(name, value)| name.as_str().len() + value.len())
            .sum();
        self.body.len() + headers
    }

    fn age(&self) -> Duration {
        self.head.initial_age + self.stored_at.elapsed()
    }

    fn is_fresh(&self) -> bool {
        self.age() < self.head.freshness
    }

    fn is_stale_revalidatable(&self, config_swr: Duration) -> bool {
        let swr =
            directive_seconds(&self.head.headers, "stale-while-revalidate").unwrap_or(config_swr);
        if swr.is_zero() {
            return false;
        }
        let age = self.age();
        age >= self.head.freshness && age < self.head.freshness + swr
    }

    fn matches(&self, request: &HeaderMap) -> bool {
        self.head
            .vary
            .iter()
            .all(|(name, value)| request.get(name) == value.as_ref())
    }

    /// The response in the form that the store keeps. `None` when a header value is not text.
    fn to_shared(&self, stored_at: u64) -> Option<SharedResponse> {
        let headers = self
            .head
            .headers
            .iter()
            .map(|(name, value)| Some((name.to_string(), value.to_str().ok()?.to_string())))
            .collect::<Option<_>>()?;
        let vary = self
            .head
            .vary
            .iter()
            .map(|(name, value)| {
                let value = value.as_ref().map(HeaderValue::to_str).transpose().ok()?;
                Some((name.to_string(), value.map(str::to_string)))
            })
            .collect::<Option<_>>()?;
        Some(SharedResponse {
            status: self.head.status.as_u16(),
            headers,
            vary,
            initial_age: self.head.initial_age.as_secs(),
            freshness: self.head.freshness.as_secs(),
            stored_at,
            body: SharedResponse::encode_body(&self.body),
        })
    }

    /// A response that another node stored. Its age includes the time since that node stored it.
    fn from_shared(shared: &SharedResponse, now_ms: u64) -> anyhow::Result<Self> {
        let mut headers = HeaderMap::new();
        for (name, value) in &shared.headers {
            headers.append(
                HeaderName::from_bytes(name.as_bytes())?,
                HeaderValue::from_str(value)?,
            );
        }
        let vary = shared
            .vary
            .iter()
            .map(|(name, value)| -> anyhow::Result<_> {
                let value = value.as_deref().map(HeaderValue::from_str).transpose()?;
                Ok((HeaderName::from_bytes(name.as_bytes())?, value))
            })
            .collect::<anyhow::Result<_>>()?;
        let elapsed = Duration::from_millis(now_ms.saturating_sub(shared.stored_at));
        Ok(Self {
            head: StoredHead {
                status: StatusCode::from_u16(shared.status)?,
                headers,
                vary,
                initial_age: Duration::from_secs(shared.initial_age) + elapsed,
                freshness: Duration::from_secs(shared.freshness),
            },
            body: Bytes::from(shared.decode_body()?),
            stored_at: Instant::now(),
        })
    }
}

struct EntryExpiry {
    default_swr: Duration,
}

impl EntryExpiry {
    fn lifetime(&self, entry: &CachedResponse) -> Duration {
        let swr = directive_seconds(&entry.head.headers, "stale-while-revalidate")
            .unwrap_or(self.default_swr);
        let window = if has_validator(&entry.head.headers) {
            REVALIDATION_WINDOW
        } else {
            Duration::ZERO
        };
        entry.head.freshness.saturating_sub(entry.head.initial_age) + swr + window
    }
}

impl Expiry<String, Arc<CachedResponse>> for EntryExpiry {
    fn expire_after_create(
        &self,
        _key: &String,
        entry: &Arc<CachedResponse>,
        _created_at: Instant,
    ) -> Option<Duration> {
        Some(self.lifetime(entry))
    }

    fn expire_after_update(
        &self,
        _key: &String,
        entry: &Arc<CachedResponse>,
        _updated_at: Instant,
        _duration_until_expiry: Option<Duration>,
    ) -> Option<Duration> {
        Some(self.lifetime(entry))
    }
}

/// The cache state of one request.
#[derive(Debug)]
pub struct CacheRequest {
    cache: Arc<HttpCache>,
    key: String,
    head: bool,
    /// Headers of the upstream request, for `Vary` and conditional requests.
    headers: HeaderMap,
    /// False when the client asks for a response that does not come from the cache.
    lookup: bool,
    /// True when the client request had an `Authorization` header.
    authorized: bool,
    unsafe_method: bool,
    host: String,
    path: String,
}

impl CacheRequest {
    /// Returns `None` when the request cannot use or affect the cache. For cacheable requests,
    /// removes `Accept-Encoding`, so the upstream server sends an unencoded response that every
    /// client can receive. `key` identifies the stored response.
    pub fn new<B>(
        cache: &Arc<HttpCache>,
        req: &mut Request<B>,
        key: String,
        authorized: bool,
    ) -> Option<Self> {
        let unsafe_method = is_unsafe_method(req.method());
        if !unsafe_method && !is_cacheable_request(req) {
            return None;
        }
        if !unsafe_method {
            req.headers_mut().remove(ACCEPT_ENCODING);
        }
        let headers = req.headers().clone();
        let lookup = !unsafe_method && is_lookup_allowed(&headers);
        let host = extract_key_host(&key).to_string();
        let path = extract_key_path(&key).to_string();
        Some(Self {
            cache: cache.clone(),
            key,
            head: req.method() == Method::HEAD,
            headers,
            lookup,
            authorized,
            unsafe_method,
            host,
            path,
        })
    }

    pub fn is_unsafe(&self) -> bool {
        self.unsafe_method
    }

    pub fn invalidate_target(&self) {
        self.cache
            .invalidate_target(&self.key, &self.host, &self.path);
    }

    fn stored(&self) -> Option<Arc<CachedResponse>> {
        if !self.lookup {
            return None;
        }
        self.cache
            .entries
            .get(&self.key)
            .filter(|entry| entry.matches(&self.headers))
    }

    /// The response of another node after a local miss. This node stores the response too.
    async fn shared(&self) -> Option<Arc<CachedResponse>> {
        if !self.lookup {
            return None;
        }
        let shared = self.cache.share.as_ref()?.lookup(&self.key).await?;
        let entry = match CachedResponse::from_shared(&shared, unix_ms()) {
            Ok(entry) => Arc::new(entry),
            Err(err) => {
                warn!("invalid shared response: {err:#}");
                return None;
            }
        };
        if !entry.matches(&self.headers) {
            return None;
        }
        self.cache.entries.insert(self.key.clone(), entry.clone());
        Some(entry)
    }

    /// Sends a stored response. A client with a matching validator receives 304 Not Modified.
    fn respond(&self, entry: &CachedResponse) -> Response<ProxyBody> {
        let not_modified = is_not_modified(&self.headers, &entry.head.headers);
        let body = if self.head || not_modified {
            Bytes::new()
        } else {
            entry.body.clone()
        };
        let mut res = Response::new(BoxBody::new(Full::new(body).map_err(Into::into)));
        *res.status_mut() = if not_modified {
            StatusCode::NOT_MODIFIED
        } else {
            entry.head.status
        };
        *res.headers_mut() = entry.head.headers.clone();
        if not_modified {
            Self::strip_304_headers(res.headers_mut());
        }
        res.headers_mut()
            .insert(AGE, HeaderValue::from(entry.age().as_secs()));
        res.headers_mut()
            .insert(X_CACHE, HeaderValue::from_static("HIT"));
        super::super::stats::count_cache_hit();
        res
    }

    /// Strips representation headers from a 304 Not Modified response (RFC 9110 15.4.5).
    fn strip_304_headers(headers: &mut HeaderMap) {
        headers.remove(CONTENT_LENGTH);
        headers.remove(CONTENT_TYPE);
        headers.remove(CONTENT_ENCODING);
        headers.remove(CONTENT_LANGUAGE);
        headers.remove(CONTENT_RANGE);
    }

    /// Sends a stale stored response while background revalidation is running.
    fn respond_stale(&self, entry: &CachedResponse) -> Response<ProxyBody> {
        let mut res = self.respond(entry);
        res.headers_mut()
            .insert(X_CACHE, HeaderValue::from_static("STALE"));
        res
    }

    /// Updates the stored response with the headers of a 304 Not Modified response.
    fn refresh(&self, entry: &CachedResponse, headers: &HeaderMap) -> Arc<CachedResponse> {
        self.cache.refresh_entry(&self.key, entry, headers)
    }

    fn revalidate_background(
        &self,
        upstream: &Upstream,
        entry: &CachedResponse,
        original_req: &Request<ProxyBody>,
    ) {
        if !self.cache.mark_revalidating(&self.key) {
            return;
        }
        let upstream = upstream.clone();
        let cache = self.cache.clone();
        let key = self.key.clone();
        let stored_entry = Arc::new(CachedResponse {
            head: entry.head.clone(),
            body: entry.body.clone(),
            stored_at: entry.stored_at,
        });
        let mut reval_req = Request::new(BoxBody::new(Full::new(Bytes::new()).map_err(Into::into)));
        *reval_req.method_mut() = original_req.method().clone();
        *reval_req.uri_mut() = original_req.uri().clone();
        *reval_req.headers_mut() = original_req.headers().clone();
        *reval_req.extensions_mut() = original_req.extensions().clone();
        set_validators(reval_req.headers_mut(), &entry.head.headers);
        let authorized = self.authorized;
        let head = self.head;
        let req_headers = self.headers.clone();
        tokio::spawn(async move {
            let _guard = RevalidationGuard {
                cache: cache.clone(),
                key: key.clone(),
            };
            let task = RevalidationTask {
                upstream,
                reval_req,
                cache,
                key,
                stored_entry,
                authorized,
                head,
                req_headers,
            };
            run_background_revalidation(task).await;
        });
    }

    /// Marks the upstream response as a miss and stores it while the client receives the body.
    fn store(
        &self,
        mut res: Response<ProxyBody>,
        mut guard: Option<FlightGuard>,
    ) -> Response<ProxyBody> {
        let head = self.storable_head(&res);
        res.headers_mut()
            .insert(X_CACHE, HeaderValue::from_static("MISS"));
        super::super::stats::count_cache_miss();
        let Some(head) = head else {
            if let Some(guard) = guard.take() {
                guard.finish();
            }
            return res;
        };
        let recorder = Recorder {
            head: Some(head),
            buffer: BytesMut::new(),
            expected: content_length(res.headers()),
            max_size: self.cache.config.max_entry_size,
            cache: self.cache.clone(),
            key: self.key.clone(),
            guard,
            chunked: is_chunked(res.headers()),
        };
        res.map(|inner| BoxBody::new(RecordingBody { inner, recorder }))
    }

    fn storable_head(&self, res: &Response<ProxyBody>) -> Option<StoredHead> {
        let headers = res.headers();
        if self.head || !is_storable(res.status(), headers, self.authorized) {
            return None;
        }
        if content_length(headers).is_none() && !is_chunked(headers) {
            return None;
        }
        let freshness = freshness(headers, self.cache.config.default_ttl);
        let too_large =
            content_length(headers).is_some_and(|length| length > self.cache.config.max_entry_size);
        if too_large || (freshness.is_zero() && !has_validator(headers)) {
            return None;
        }
        Some(StoredHead {
            status: res.status(),
            headers: headers.clone(),
            vary: vary_values(headers, &self.headers)?,
            initial_age: age(headers),
            freshness,
        })
    }
}

enum Flight {
    Leader(FlightGuard),
    Follower(Arc<Notify>),
}

struct FlightGuard {
    cache: Arc<HttpCache>,
    key: String,
    notify: Arc<Notify>,
    completed: bool,
}

impl FlightGuard {
    fn finish(mut self) {
        self.completed = true;
        self.cleanup();
    }

    fn cleanup(&mut self) {
        let mut flights = lock(&self.cache.in_flight);
        flights.remove(&self.key);
        self.notify.notify_waiters();
    }
}

impl Drop for FlightGuard {
    fn drop(&mut self) {
        if !self.completed {
            self.cleanup();
        }
    }
}

struct RevalidationGuard {
    cache: Arc<HttpCache>,
    key: String,
}

impl Drop for RevalidationGuard {
    fn drop(&mut self) {
        self.cache.finish_revalidating(&self.key);
    }
}

struct RevalidationTask {
    upstream: Upstream,
    reval_req: Request<ProxyBody>,
    cache: Arc<HttpCache>,
    key: String,
    stored_entry: Arc<CachedResponse>,
    authorized: bool,
    head: bool,
    req_headers: HeaderMap,
}

fn merge_304_headers(
    entry: &CachedResponse,
    headers: &HeaderMap,
    default_ttl: Duration,
    filter_cookie: bool,
) -> CachedResponse {
    let mut head = entry.head.clone();
    for name in headers
        .keys()
        .filter(|name| **name != CONTENT_LENGTH && (!filter_cookie || **name != SET_COOKIE))
    {
        head.headers.remove(name);
        for value in headers.get_all(name) {
            head.headers.append(name, value.clone());
        }
    }
    head.initial_age = age(headers);
    head.freshness = freshness(&head.headers, default_ttl);
    CachedResponse {
        head,
        body: entry.body.clone(),
        stored_at: Instant::now(),
    }
}

fn is_storable_304(headers: &HeaderMap, authorized: bool) -> bool {
    if has_directive(headers, CACHE_CONTROL, "no-store")
        || has_directive(headers, CACHE_CONTROL, "private")
        || headers.contains_key(SET_COOKIE)
    {
        return false;
    }
    !authorized || allows_authorized(headers)
}

fn handle_304(
    cache: &CacheRequest,
    entry: &CachedResponse,
    headers: &HeaderMap,
) -> Response<ProxyBody> {
    if is_storable_304(headers, cache.authorized) {
        let refreshed = cache.refresh(entry, headers);
        cache.respond(&refreshed)
    } else {
        let transient = merge_304_headers(entry, headers, cache.cache.config.default_ttl, false);
        cache.respond(&transient)
    }
}

async fn run_background_revalidation(task: RevalidationTask) {
    let Ok(res) = task.upstream.request(task.reval_req).await else {
        return;
    };
    if res.status() == StatusCode::NOT_MODIFIED {
        if is_storable_304(res.headers(), task.authorized) {
            task.cache
                .refresh_entry(&task.key, &task.stored_entry, res.headers());
        }
        return;
    }
    if !is_storable(res.status(), res.headers(), task.authorized) || task.head {
        return;
    }
    let (parts, body) = res.into_parts();
    let Ok(collected) = body.collect().await else {
        return;
    };
    let bytes = collected.to_bytes();
    if (bytes.len() as u64) > task.cache.config.max_entry_size {
        return;
    }
    let freshness = freshness(&parts.headers, task.cache.config.default_ttl);
    if let Some(vary) = vary_values(&parts.headers, &task.req_headers) {
        let head = StoredHead {
            status: parts.status,
            headers: parts.headers,
            vary,
            initial_age: Duration::ZERO,
            freshness,
        };
        task.cache.store_entry(&task.key, head, bytes);
    }
}

fn cached_response(
    cache: &CacheRequest,
    stored: Option<&Arc<CachedResponse>>,
) -> Option<Response<ProxyBody>> {
    let entry = stored?;
    if entry.is_fresh() {
        Some(cache.respond(entry))
    } else {
        None
    }
}

async fn find_stored(cache: &CacheRequest) -> Option<Arc<CachedResponse>> {
    match cache.stored() {
        Some(entry) => Some(entry),
        None => cache.shared().await,
    }
}

async fn wait_for_flight(cache: &CacheRequest, notify: Arc<Notify>) -> Option<Response<ProxyBody>> {
    if let Some(stored) = cache.stored().filter(|entry| entry.is_fresh()) {
        return Some(cache.respond(&stored));
    }
    let _ = tokio::time::timeout(Duration::from_secs(10), notify.notified()).await;
    let stored = cache.stored()?;
    if stored.is_fresh() {
        Some(cache.respond(&stored))
    } else {
        None
    }
}

async fn fetch_upstream(
    upstream: &Upstream,
    mut req: Request<ProxyBody>,
    cache: &CacheRequest,
    stored: Option<Arc<CachedResponse>>,
    mut guard: Option<FlightGuard>,
) -> Result<Response<ProxyBody>, anyhow::Error> {
    if let Some(entry) = &stored {
        set_validators(req.headers_mut(), &entry.head.headers);
    }
    let res = upstream.request(req).await?;
    match stored {
        Some(entry) if res.status() == StatusCode::NOT_MODIFIED => {
            if let Some(guard) = guard.take() {
                guard.finish();
            }
            Ok(handle_304(cache, &entry, res.headers()))
        }
        _ => Ok(cache.store(res, guard)),
    }
}

/// Sends the request to the upstream server, or answers it from the cache.
pub async fn fetch(
    upstream: &Upstream,
    req: Request<ProxyBody>,
    cache: Option<CacheRequest>,
) -> Result<Response<ProxyBody>, anyhow::Error> {
    let Some(cache) = cache else {
        return upstream.request(req).await;
    };
    if cache.is_unsafe() {
        return fetch_unsafe(upstream, req, &cache).await;
    }
    let stored = find_stored(&cache).await;
    if let Some(res) = cached_response(&cache, stored.as_ref()) {
        return Ok(res);
    }
    if let Some(entry) = stored
        .as_ref()
        .filter(|entry| entry.is_stale_revalidatable(cache.cache.config.stale_while_revalidate))
    {
        cache.revalidate_background(upstream, entry, &req);
        return Ok(cache.respond_stale(entry));
    }
    match cache.cache.start_flight(&cache.key) {
        Flight::Follower(notify) => {
            if let Some(res) = wait_for_flight(&cache, notify).await {
                return Ok(res);
            }
            fetch_upstream(upstream, req, &cache, stored, None).await
        }
        Flight::Leader(guard) => fetch_upstream(upstream, req, &cache, stored, Some(guard)).await,
    }
}

async fn fetch_unsafe(
    upstream: &Upstream,
    req: Request<ProxyBody>,
    cache: &CacheRequest,
) -> Result<Response<ProxyBody>, anyhow::Error> {
    let res = upstream.request(req).await?;
    if res.status().is_success() || res.status().is_redirection() {
        cache.invalidate_target();
    }
    Ok(res)
}

/// Replaces the client validators with the validators of the stored response.
fn set_validators(headers: &mut HeaderMap, stored: &HeaderMap) {
    headers.remove(IF_NONE_MATCH);
    headers.remove(IF_MODIFIED_SINCE);
    if let Some(etag) = stored.get(ETAG) {
        headers.insert(IF_NONE_MATCH, etag.clone());
    }
    if let Some(modified) = stored.get(LAST_MODIFIED) {
        headers.insert(IF_MODIFIED_SINCE, modified.clone());
    }
}

fn is_unsafe_method(method: &Method) -> bool {
    !matches!(
        *method,
        Method::GET | Method::HEAD | Method::OPTIONS | Method::TRACE
    )
}

fn is_lookup_allowed(headers: &HeaderMap) -> bool {
    !has_directive(headers, CACHE_CONTROL, "no-cache")
        && !has_directive(headers, PRAGMA, "no-cache")
}

fn is_cacheable_request<B>(req: &Request<B>) -> bool {
    let headers = req.headers();
    matches!(*req.method(), Method::GET | Method::HEAD)
        && !headers.contains_key(RANGE)
        && !headers.contains_key(UPGRADE)
        && !has_directive(headers, CACHE_CONTROL, "no-store")
}

fn is_chunked(headers: &HeaderMap) -> bool {
    headers.get_all(TRANSFER_ENCODING).iter().any(|v| {
        v.to_str()
            .map(|s| s.eq_ignore_ascii_case("chunked"))
            .unwrap_or(false)
    })
}

/// Returns true when a comma-separated header has the directive, with or without a value.
fn has_directive(headers: &HeaderMap, name: HeaderName, directive: &str) -> bool {
    header_items(headers, name).any(|item| {
        item.split('=')
            .next()
            .unwrap_or_default()
            .trim()
            .eq_ignore_ascii_case(directive)
    })
}

fn has_validator(headers: &HeaderMap) -> bool {
    headers.contains_key(ETAG) || headers.contains_key(LAST_MODIFIED)
}

fn is_storable(status: StatusCode, headers: &HeaderMap, authorized: bool) -> bool {
    CACHEABLE_STATUSES.contains(&status.as_u16())
        && !has_directive(headers, CACHE_CONTROL, "no-store")
        && !has_directive(headers, CACHE_CONTROL, "private")
        && !headers.contains_key(SET_COOKIE)
        && (!authorized || allows_authorized(headers))
}

/// A shared cache stores the response to a request with `Authorization` only when the response
/// allows it (RFC 9111 3.5).
fn allows_authorized(headers: &HeaderMap) -> bool {
    ["public", "s-maxage", "must-revalidate"]
        .into_iter()
        .any(|directive| has_directive(headers, CACHE_CONTROL, directive))
}

/// Returns how long the response stays fresh after the upstream server created it (RFC 9111 4.2.1).
fn freshness(headers: &HeaderMap, default_ttl: Duration) -> Duration {
    if has_directive(headers, CACHE_CONTROL, "no-cache") {
        return Duration::ZERO;
    }
    directive_seconds(headers, "s-maxage")
        .or_else(|| directive_seconds(headers, "max-age"))
        .or_else(|| expires_lifetime(headers))
        .unwrap_or(default_ttl)
}

fn directive_seconds(headers: &HeaderMap, directive: &str) -> Option<Duration> {
    header_items(headers, CACHE_CONTROL).find_map(|item| {
        let (name, value) = item.split_once('=')?;
        if !name.trim().eq_ignore_ascii_case(directive) {
            return None;
        }
        value
            .trim()
            .trim_matches('"')
            .parse()
            .ok()
            .map(Duration::from_secs)
    })
}

/// An invalid `Expires` date means that the response is already stale.
fn expires_lifetime(headers: &HeaderMap) -> Option<Duration> {
    let expires = headers.get(EXPIRES)?;
    let date = header_time(headers.get(DATE)).unwrap_or_else(SystemTime::now);
    Some(
        header_time(Some(expires))
            .and_then(|expires| expires.duration_since(date).ok())
            .unwrap_or_default(),
    )
}

fn header_time(value: Option<&HeaderValue>) -> Option<SystemTime> {
    httpdate::parse_http_date(value?.to_str().ok()?).ok()
}

fn age(headers: &HeaderMap) -> Duration {
    header_number(headers, AGE)
        .map(Duration::from_secs)
        .unwrap_or_default()
}

fn content_length(headers: &HeaderMap) -> Option<u64> {
    header_number(headers, CONTENT_LENGTH)
}

/// Returns `None` for `Vary: *`, because no later request can match that response.
fn vary_values(
    response: &HeaderMap,
    request: &HeaderMap,
) -> Option<Vec<(HeaderName, Option<HeaderValue>)>> {
    header_items(response, VARY)
        .filter(|name| !name.is_empty())
        .map(|name| {
            if name == "*" {
                return None;
            }
            let name = HeaderName::from_bytes(name.as_bytes()).ok()?;
            let value = request.get(&name).cloned();
            Some((name, value))
        })
        .collect()
}

fn is_not_modified(request: &HeaderMap, stored: &HeaderMap) -> bool {
    if let Some(condition) = request.get(IF_NONE_MATCH) {
        return stored
            .get(ETAG)
            .is_some_and(|etag| etag_matches(condition, etag));
    }
    match (
        header_time(request.get(IF_MODIFIED_SINCE)),
        header_time(stored.get(LAST_MODIFIED)),
    ) {
        (Some(since), Some(modified)) => modified <= since,
        _ => false,
    }
}

/// Weak comparison of entity tags (RFC 9110 8.8.3.2).
fn etag_matches(condition: &HeaderValue, etag: &HeaderValue) -> bool {
    let etag = weak_tag(etag.as_bytes());
    condition.to_str().is_ok_and(|condition| {
        condition
            .split(',')
            .map(str::trim)
            .any(|tag| tag == "*" || weak_tag(tag.as_bytes()) == etag)
    })
}

fn weak_tag(tag: &[u8]) -> &[u8] {
    tag.strip_prefix(b"W/").unwrap_or(tag)
}

/// Copies a response body into the cache while the client receives it.
struct Recorder {
    /// `None` when the response cannot be stored.
    head: Option<StoredHead>,
    buffer: BytesMut,
    expected: Option<u64>,
    max_size: u64,
    cache: Arc<HttpCache>,
    key: String,
    guard: Option<FlightGuard>,
    chunked: bool,
}

impl Recorder {
    fn abort(&mut self) {
        self.head = None;
        self.buffer = BytesMut::new();
        if let Some(guard) = self.guard.take() {
            guard.finish();
        }
    }

    fn record(&mut self, frame: &Frame<Bytes>) {
        let Some(data) = frame.data_ref() else {
            // Trailers are not stored, so a response with trailers is not stored.
            self.abort();
            return;
        };
        if self.head.is_none() {
            return;
        }
        if (self.buffer.len() + data.len()) as u64 > self.max_size {
            self.abort();
            return;
        }
        self.buffer.extend_from_slice(data);
        if self.expected == Some(self.buffer.len() as u64) {
            self.finish(true);
        }
    }

    fn finish(&mut self, completed_cleanly: bool) {
        let guard = self.guard.take();
        let Some(head) = self.head.take() else {
            self.buffer = BytesMut::new();
            return;
        };
        if self
            .expected
            .is_some_and(|length| length != self.buffer.len() as u64)
            || (self.expected.is_none() && (!self.chunked || !completed_cleanly))
        {
            self.buffer = BytesMut::new();
            return;
        }
        let entry = Arc::new(CachedResponse {
            head,
            body: std::mem::take(&mut self.buffer).freeze(),
            stored_at: Instant::now(),
        });
        let key = std::mem::take(&mut self.key);
        if let Some(share) = &self.cache.share {
            share_response(share, &key, &entry);
        }
        self.cache.entries.insert(key, entry);
        if let Some(guard) = guard {
            guard.finish();
        }
    }
}

/// Queues a stored response for the other nodes of a cluster.
fn share_response(share: &CacheShare, key: &str, entry: &CachedResponse) {
    let remaining = entry.head.freshness.saturating_sub(entry.head.initial_age);
    if !share.accepts(remaining, entry.body.len()) {
        return;
    }
    if let Some(shared) = entry.to_shared(unix_ms()) {
        share.offer(key, shared);
    }
}

pin_project! {
    struct RecordingBody {
        #[pin]
        inner: ProxyBody,
        recorder: Recorder,
    }
}

impl Body for RecordingBody {
    type Data = Bytes;
    type Error = anyhow::Error;

    fn poll_frame(
        self: Pin<&mut Self>,
        cx: &mut Context<'_>,
    ) -> Poll<Option<Result<Frame<Bytes>, anyhow::Error>>> {
        let mut this = self.project();
        let frame = ready!(this.inner.as_mut().poll_frame(cx));
        match &frame {
            Some(Ok(frame)) => this.recorder.record(frame),
            Some(Err(_)) => this.recorder.abort(),
            None => {
                let clean = this.inner.is_end_stream();
                this.recorder.finish(clean);
            }
        }
        Poll::Ready(frame)
    }

    // `is_end_stream` keeps its default, so the server polls until the end of the body and the
    // recorder sees the end of a body without `Content-Length`.
    fn size_hint(&self) -> SizeHint {
        self.inner.size_hint()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn headers(pairs: &[(&str, &str)]) -> HeaderMap {
        pairs
            .iter()
            .map(|(name, value)| (name.parse().unwrap(), value.parse().unwrap()))
            .collect()
    }

    fn value(value: &str) -> HeaderValue {
        value.parse().unwrap()
    }

    #[test]
    fn freshness_prefers_s_maxage_then_max_age_then_expires() {
        let ttl = Duration::from_secs(7);
        let lifetime = |pairs: &[(&str, &str)]| freshness(&headers(pairs), ttl);
        assert_eq!(
            lifetime(&[("cache-control", "max-age=60, s-maxage=\"30\"")]),
            Duration::from_secs(30)
        );
        assert_eq!(
            lifetime(&[
                ("cache-control", "public, max-age=60"),
                ("expires", "Wed, 21 Oct 2015 07:28:00 GMT")
            ]),
            Duration::from_secs(60)
        );
        assert_eq!(
            lifetime(&[
                ("date", "Wed, 21 Oct 2015 07:28:00 GMT"),
                ("expires", "Wed, 21 Oct 2015 07:38:00 GMT")
            ]),
            Duration::from_secs(600)
        );
        assert_eq!(lifetime(&[("expires", "0")]), Duration::ZERO);
        assert_eq!(
            lifetime(&[("cache-control", "no-cache, max-age=60")]),
            Duration::ZERO
        );
        assert_eq!(lifetime(&[]), ttl);
    }

    #[test]
    fn private_and_personal_responses_are_not_stored() {
        let ok = StatusCode::OK;
        assert!(is_storable(ok, &headers(&[]), false));
        assert!(!is_storable(
            StatusCode::PARTIAL_CONTENT,
            &headers(&[]),
            false
        ));
        assert!(!is_storable(
            ok,
            &headers(&[("cache-control", "private=\"x\"")]),
            false
        ));
        assert!(!is_storable(
            ok,
            &headers(&[("cache-control", "No-Store")]),
            false
        ));
        assert!(!is_storable(ok, &headers(&[("set-cookie", "id=1")]), false));
        assert!(!is_storable(ok, &headers(&[]), true));
        assert!(is_storable(
            ok,
            &headers(&[("cache-control", "s-maxage=5")]),
            true
        ));
    }

    #[test]
    fn validators_are_compared_weakly() {
        let stored = headers(&[
            ("etag", "\"v1\""),
            ("last-modified", "Wed, 21 Oct 2015 07:28:00 GMT"),
        ]);
        let request = |pairs: &[(&str, &str)]| is_not_modified(&headers(pairs), &stored);
        assert!(request(&[("if-none-match", "\"v0\", W/\"v1\"")]));
        assert!(request(&[("if-none-match", "*")]));
        assert!(!request(&[("if-none-match", "\"v2\"")]));
        assert!(!request(&[
            ("if-none-match", "\"v2\""),
            ("if-modified-since", "Wed, 21 Oct 2015 08:00:00 GMT")
        ]));
        assert!(request(&[(
            "if-modified-since",
            "Wed, 21 Oct 2015 08:00:00 GMT"
        )]));
        assert!(!request(&[(
            "if-modified-since",
            "Wed, 21 Oct 2015 07:00:00 GMT"
        )]));
        assert!(etag_matches(&value("\"v1\""), &value("W/\"v1\"")));
    }

    #[test]
    fn vary_records_request_values() {
        let request = headers(&[("x-lang", "en")]);
        let vary = vary_values(&headers(&[("vary", "X-Lang, Origin")]), &request).unwrap();
        assert_eq!(
            vary,
            vec![
                (HeaderName::from_static("x-lang"), Some(value("en"))),
                (HeaderName::from_static("origin"), None),
            ]
        );
        assert!(vary_values(&headers(&[("vary", "Origin, *")]), &request).is_none());
    }

    #[test]
    fn registry_reuses_the_cache_and_purge_removes_entries() {
        let id: ShortId = "cachprg".parse().unwrap();
        let config = CacheConfig {
            enabled: true,
            ..Default::default()
        };
        let registry = CacheRegistry::default();
        assert!(registry.cache_for(id, &CacheConfig::default()).is_none());
        let cache = registry.cache_for(id, &config).unwrap();
        assert!(Arc::ptr_eq(
            &cache,
            &registry.cache_for(id, &config).unwrap()
        ));
        let other = CacheRegistry::default();
        assert!(!Arc::ptr_eq(&cache, &other.cache_for(id, &config).unwrap()));

        cache.entries.insert("key".into(), Arc::new(stored_entry()));
        assert!(cache.entries.get("key").is_some());
        other.purge(id);
        assert!(cache.entries.get("key").is_some());
        let at = registry.purge(id);
        assert!(cache.entries.get("key").is_none());

        // A purge that this server already applied does not purge again.
        cache.entries.insert("key".into(), Arc::new(stored_entry()));
        registry.apply_purges(HashMap::from([(id, at)]));
        assert!(cache.entries.get("key").is_some());
        registry.apply_purges(HashMap::from([(id, at + 1)]));
        assert!(cache.entries.get("key").is_none());
    }

    fn stored_entry() -> CachedResponse {
        CachedResponse {
            head: StoredHead {
                status: StatusCode::OK,
                headers: headers(&[("etag", "\"v1\""), ("cache-control", "max-age=90")]),
                vary: vec![
                    (HeaderName::from_static("x-lang"), Some(value("en"))),
                    (HeaderName::from_static("origin"), None),
                ],
                initial_age: Duration::from_secs(10),
                freshness: Duration::from_secs(90),
            },
            body: Bytes::from_static(b"body"),
            stored_at: Instant::now(),
        }
    }

    #[test]
    fn a_shared_response_keeps_its_head_and_body() {
        let entry = stored_entry();
        let shared = entry.to_shared(1_000).unwrap();
        let restored = CachedResponse::from_shared(&shared, 3_000).unwrap();
        assert_eq!(restored.head.status, StatusCode::OK);
        assert_eq!(restored.head.headers, entry.head.headers);
        assert_eq!(restored.head.vary, entry.head.vary);
        assert_eq!(restored.head.initial_age, Duration::from_secs(12));
        assert_eq!(restored.head.freshness, entry.head.freshness);
        assert_eq!(restored.body, entry.body);

        let invalid = SharedResponse {
            status: 1000,
            ..shared
        };
        assert!(CachedResponse::from_shared(&invalid, 3_000).is_err());
    }

    #[test]
    fn purge_path_removes_matching_keys_across_schemes() {
        let cache = HttpCache::new(CacheConfig::default(), None);
        cache.entries.insert(
            "https example.com /fresh/a".into(),
            Arc::new(stored_entry()),
        );
        cache
            .entries
            .insert("http example.com /fresh/b".into(), Arc::new(stored_entry()));
        cache
            .entries
            .insert("example.com /legacy".into(), Arc::new(stored_entry()));
        assert_eq!(cache.purge_path("/fresh"), 2);
        assert_eq!(cache.purge_path("/legacy"), 1);
    }

    #[test]
    fn extract_key_path_extracts_path_from_both_formats() {
        assert_eq!(extract_key_path("https example.com /test?a=1"), "/test?a=1");
        assert_eq!(extract_key_path("http example.com /test"), "/test");
        assert_eq!(extract_key_path("example.com /legacy/path"), "/legacy/path");
        assert_eq!(extract_key_path("opaque-key"), "opaque-key");
    }

    #[tokio::test]
    async fn concurrent_flights_coalesce_to_leader_and_followers() {
        let cache = Arc::new(HttpCache::new(CacheConfig::default(), None));
        let flight1 = cache.start_flight("stampede-key");
        assert!(matches!(flight1, Flight::Leader(_)));

        let flight2 = cache.start_flight("stampede-key");
        assert!(matches!(flight2, Flight::Follower(_)));

        let cache_clone = cache.clone();
        let handle = tokio::spawn(async move {
            if let Flight::Follower(notify) = flight2 {
                let req = CacheRequest {
                    cache: cache_clone,
                    key: "stampede-key".into(),
                    head: false,
                    headers: HeaderMap::new(),
                    lookup: true,
                    authorized: false,
                    unsafe_method: false,
                    host: String::new(),
                    path: String::new(),
                };
                wait_for_flight(&req, notify).await
            } else {
                None
            }
        });

        tokio::task::yield_now().await;

        if let Flight::Leader(guard) = flight1 {
            let entry = CachedResponse {
                head: StoredHead {
                    status: StatusCode::OK,
                    headers: headers(&[("etag", "\"v1\""), ("cache-control", "max-age=90")]),
                    vary: vec![],
                    initial_age: Duration::from_secs(10),
                    freshness: Duration::from_secs(90),
                },
                body: Bytes::from_static(b"body"),
                stored_at: Instant::now(),
            };
            cache.entries.insert("stampede-key".into(), Arc::new(entry));
            guard.finish();
        }

        let res = handle.await.unwrap();
        assert!(res.is_some());

        let flight3 = cache.start_flight("stampede-key");
        assert!(matches!(flight3, Flight::Leader(_)));
    }

    #[test]
    fn not_modified_response_strips_representation_headers() {
        let cache = Arc::new(HttpCache::new(CacheConfig::default(), None));
        let req_headers = headers(&[("if-none-match", "\"v1\"")]);
        let req = CacheRequest {
            cache,
            key: "test-key".into(),
            head: false,
            headers: req_headers,
            lookup: true,
            authorized: false,
            unsafe_method: false,
            host: String::new(),
            path: String::new(),
        };
        let stored = CachedResponse {
            head: StoredHead {
                status: StatusCode::OK,
                headers: headers(&[
                    ("etag", "\"v1\""),
                    ("content-type", "application/json"),
                    ("content-encoding", "gzip"),
                    ("content-language", "en"),
                    ("content-range", "bytes 0-10/11"),
                    ("content-length", "11"),
                    ("cache-control", "max-age=60"),
                ]),
                vary: vec![],
                initial_age: Duration::from_secs(5),
                freshness: Duration::from_secs(60),
            },
            body: Bytes::from_static(b"hello world"),
            stored_at: Instant::now(),
        };
        let res = req.respond(&stored);
        assert_eq!(res.status(), StatusCode::NOT_MODIFIED);
        assert!(!res.headers().contains_key(CONTENT_TYPE));
        assert!(!res.headers().contains_key(CONTENT_ENCODING));
        assert!(!res.headers().contains_key(CONTENT_LANGUAGE));
        assert!(!res.headers().contains_key(CONTENT_RANGE));
        assert!(!res.headers().contains_key(CONTENT_LENGTH));
        assert_eq!(res.headers().get(ETAG).unwrap(), "\"v1\"");
    }

    #[test]
    fn unchunked_responses_without_content_length_are_not_storable() {
        let cache = Arc::new(HttpCache::new(CacheConfig::default(), None));
        let req = CacheRequest {
            cache,
            key: "test-key".into(),
            head: false,
            headers: HeaderMap::new(),
            lookup: true,
            authorized: false,
            unsafe_method: false,
            host: String::new(),
            path: String::new(),
        };
        let res = Response::builder()
            .status(StatusCode::OK)
            .header(CACHE_CONTROL, "max-age=60")
            .body(BoxBody::new(Full::new(Bytes::new()).map_err(Into::into)))
            .unwrap();
        assert!(req.storable_head(&res).is_none());

        let chunked_res = Response::builder()
            .status(StatusCode::OK)
            .header(CACHE_CONTROL, "max-age=60")
            .header(TRANSFER_ENCODING, "chunked")
            .body(BoxBody::new(Full::new(Bytes::new()).map_err(Into::into)))
            .unwrap();
        assert!(req.storable_head(&chunked_res).is_some());
    }

    #[test]
    fn aborted_or_unclean_chunked_stream_is_not_stored_and_frees_buffer() {
        let cache = Arc::new(HttpCache::new(CacheConfig::default(), None));
        let mut recorder = Recorder {
            head: Some(StoredHead {
                status: StatusCode::OK,
                headers: HeaderMap::new(),
                vary: vec![],
                initial_age: Duration::ZERO,
                freshness: Duration::from_secs(60),
            }),
            buffer: BytesMut::from(&b"partial data"[..]),
            expected: None,
            max_size: 10,
            cache: cache.clone(),
            key: "chunked-key".into(),
            guard: None,
            chunked: true,
        };

        recorder.record(&Frame::data(Bytes::from_static(b"extra overflow")));
        assert!(recorder.head.is_none());
        assert!(recorder.buffer.is_empty());

        let mut unclean_recorder = Recorder {
            head: Some(StoredHead {
                status: StatusCode::OK,
                headers: HeaderMap::new(),
                vary: vec![],
                initial_age: Duration::ZERO,
                freshness: Duration::from_secs(60),
            }),
            buffer: BytesMut::from(&b"truncated"[..]),
            expected: None,
            max_size: 100,
            cache: cache.clone(),
            key: "truncated-key".into(),
            guard: None,
            chunked: true,
        };
        unclean_recorder.finish(false);
        assert!(cache.entries.get("truncated-key").is_none());
        assert!(unclean_recorder.buffer.is_empty());
    }

    #[test]
    fn extract_key_host_extracts_host_from_keys() {
        assert_eq!(
            extract_key_host("https example.com /test?a=1"),
            "example.com"
        );
        assert_eq!(
            extract_key_host("http sub.example.com /test"),
            "sub.example.com"
        );
        assert_eq!(extract_key_host("legacy.com /path"), "legacy.com");
        assert_eq!(extract_key_host("opaque-key"), "");
    }

    #[test]
    fn unsafe_methods_identified_correctly() {
        assert!(is_unsafe_method(&Method::POST));
        assert!(is_unsafe_method(&Method::PUT));
        assert!(is_unsafe_method(&Method::DELETE));
        assert!(is_unsafe_method(&Method::PATCH));
        assert!(!is_unsafe_method(&Method::GET));
        assert!(!is_unsafe_method(&Method::HEAD));
        assert!(!is_unsafe_method(&Method::OPTIONS));
        assert!(!is_unsafe_method(&Method::TRACE));
    }

    #[test]
    fn invalidate_target_removes_both_schemes_and_legacy_key() {
        let cache = HttpCache::new(CacheConfig::default(), None);
        cache.entries.insert(
            "https example.com /resource".into(),
            Arc::new(stored_entry()),
        );
        cache.entries.insert(
            "http example.com /resource".into(),
            Arc::new(stored_entry()),
        );
        cache
            .entries
            .insert("example.com /resource".into(), Arc::new(stored_entry()));
        cache
            .entries
            .insert("https example.com /other".into(), Arc::new(stored_entry()));

        cache.invalidate_target("https example.com /resource", "example.com", "/resource");
        assert!(cache.entries.get("https example.com /resource").is_none());
        assert!(cache.entries.get("http example.com /resource").is_none());
        assert!(cache.entries.get("example.com /resource").is_none());
        assert!(cache.entries.get("https example.com /other").is_some());
    }
}
