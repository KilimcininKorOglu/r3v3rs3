use super::cache_share::{CacheShare, SharedCacheStore, SharedResponse};
use super::compression::{header_items, header_number};
use super::pool::Upstream;
use crate::clock::unix_ms;
use crate::proxy::registry::WeakRegistry;
use bytes::{Bytes, BytesMut};
use http_body_util::{combinators::BoxBody, BodyExt, Full};
use hyper::{
    body::{Body, Frame, SizeHint},
    header::{
        HeaderName, ACCEPT_ENCODING, AGE, CACHE_CONTROL, CONTENT_LENGTH, DATE, ETAG, EXPIRES,
        IF_MODIFIED_SINCE, IF_NONE_MATCH, LAST_MODIFIED, PRAGMA, RANGE, SET_COOKIE, UPGRADE, VARY,
    },
    http::HeaderValue,
    HeaderMap, Method, Request, Response, StatusCode,
};
use moka::{sync::Cache, Expiry};
use pin_project_lite::pin_project;
use r3v3rs3_api::{cache::CacheConfig, id::ShortId};
use std::{
    collections::HashMap,
    fmt,
    pin::Pin,
    sync::{Arc, Mutex, MutexGuard, OnceLock, PoisonError},
    task::{ready, Context, Poll},
    time::{Duration, Instant, SystemTime},
};
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

impl HttpCache {
    fn new(config: CacheConfig, share: Option<CacheShare>) -> Self {
        let entries = Cache::builder()
            .weigher(|key: &String, entry: &Arc<CachedResponse>| {
                u32::try_from(key.len() + entry.weight()).unwrap_or(u32::MAX)
            })
            .max_capacity(config.max_size)
            .expire_after(EntryExpiry)
            .build();
        Self {
            config,
            entries,
            share,
        }
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
                let share =
                    store.map(|store| CacheShare::new(id, store.clone(), self.purged_at(id)));
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

struct EntryExpiry;

impl EntryExpiry {
    fn lifetime(entry: &CachedResponse) -> Duration {
        let window = if has_validator(&entry.head.headers) {
            REVALIDATION_WINDOW
        } else {
            Duration::ZERO
        };
        entry.head.freshness.saturating_sub(entry.head.initial_age) + window
    }
}

impl Expiry<String, Arc<CachedResponse>> for EntryExpiry {
    fn expire_after_create(
        &self,
        _key: &String,
        entry: &Arc<CachedResponse>,
        _created_at: Instant,
    ) -> Option<Duration> {
        Some(Self::lifetime(entry))
    }

    fn expire_after_update(
        &self,
        _key: &String,
        entry: &Arc<CachedResponse>,
        _updated_at: Instant,
        _duration_until_expiry: Option<Duration>,
    ) -> Option<Duration> {
        Some(Self::lifetime(entry))
    }
}

/// The cache state of one GET or HEAD request.
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
}

impl CacheRequest {
    /// Returns `None` when the request cannot use the cache. Removes `Accept-Encoding`, so the
    /// upstream server sends an unencoded response that every client can receive. `key`
    /// identifies the stored response.
    pub fn new<B>(
        cache: &Arc<HttpCache>,
        req: &mut Request<B>,
        key: String,
        authorized: bool,
    ) -> Option<Self> {
        if !is_cacheable_request(req) {
            return None;
        }
        req.headers_mut().remove(ACCEPT_ENCODING);
        let headers = req.headers().clone();
        let lookup = !has_directive(&headers, CACHE_CONTROL, "no-cache")
            && !has_directive(&headers, PRAGMA, "no-cache");
        Some(Self {
            cache: cache.clone(),
            key,
            head: req.method() == Method::HEAD,
            headers,
            lookup,
            authorized,
        })
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
            res.headers_mut().remove(CONTENT_LENGTH);
        }
        res.headers_mut()
            .insert(AGE, HeaderValue::from(entry.age().as_secs()));
        res.headers_mut()
            .insert(X_CACHE, HeaderValue::from_static("HIT"));
        res
    }

    /// Updates the stored response with the headers of a 304 Not Modified response.
    fn refresh(&self, entry: &CachedResponse, headers: &HeaderMap) -> Arc<CachedResponse> {
        let mut head = entry.head.clone();
        for name in headers.keys().filter(|name| **name != CONTENT_LENGTH) {
            head.headers.remove(name);
            for value in headers.get_all(name) {
                head.headers.append(name, value.clone());
            }
        }
        head.initial_age = age(headers);
        head.freshness = freshness(&head.headers, self.cache.config.default_ttl);
        let refreshed = Arc::new(CachedResponse {
            head,
            body: entry.body.clone(),
            stored_at: Instant::now(),
        });
        self.cache
            .entries
            .insert(self.key.clone(), refreshed.clone());
        refreshed
    }

    /// Marks the upstream response as a miss and stores it while the client receives the body.
    fn store(&self, mut res: Response<ProxyBody>) -> Response<ProxyBody> {
        let head = self.storable_head(&res);
        res.headers_mut()
            .insert(X_CACHE, HeaderValue::from_static("MISS"));
        let Some(head) = head else {
            return res;
        };
        let recorder = Recorder {
            head: Some(head),
            buffer: BytesMut::new(),
            expected: content_length(res.headers()),
            max_size: self.cache.config.max_entry_size,
            cache: self.cache.clone(),
            key: self.key.clone(),
        };
        res.map(|inner| BoxBody::new(RecordingBody { inner, recorder }))
    }

    fn storable_head(&self, res: &Response<ProxyBody>) -> Option<StoredHead> {
        let headers = res.headers();
        if self.head || !is_storable(res.status(), headers, self.authorized) {
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

/// Sends the request to the upstream server, or answers it from the cache.
pub async fn fetch(
    upstream: &Upstream,
    mut req: Request<ProxyBody>,
    cache: Option<CacheRequest>,
) -> Result<Response<ProxyBody>, anyhow::Error> {
    let Some(cache) = cache else {
        return upstream.request(req).await;
    };
    let stored = match cache.stored() {
        Some(entry) => Some(entry),
        None => cache.shared().await,
    };
    if let Some(entry) = stored.as_ref().filter(|entry| entry.is_fresh()) {
        return Ok(cache.respond(entry));
    }
    if let Some(entry) = &stored {
        set_validators(req.headers_mut(), &entry.head.headers);
    }
    let res = upstream.request(req).await?;
    match stored {
        Some(entry) if res.status() == StatusCode::NOT_MODIFIED => {
            let entry = cache.refresh(&entry, res.headers());
            Ok(cache.respond(&entry))
        }
        _ => Ok(cache.store(res)),
    }
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

fn is_cacheable_request<B>(req: &Request<B>) -> bool {
    let headers = req.headers();
    matches!(*req.method(), Method::GET | Method::HEAD)
        && !headers.contains_key(RANGE)
        && !headers.contains_key(UPGRADE)
        && !has_directive(headers, CACHE_CONTROL, "no-store")
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
}

impl Recorder {
    fn record(&mut self, frame: &Frame<Bytes>) {
        let Some(data) = frame.data_ref() else {
            // Trailers are not stored, so a response with trailers is not stored.
            self.head = None;
            return;
        };
        if self.head.is_none() {
            return;
        }
        if (self.buffer.len() + data.len()) as u64 > self.max_size {
            self.head = None;
            return;
        }
        self.buffer.extend_from_slice(data);
        if self.expected == Some(self.buffer.len() as u64) {
            self.finish();
        }
    }

    fn finish(&mut self) {
        let Some(head) = self.head.take() else {
            return;
        };
        if self
            .expected
            .is_some_and(|length| length != self.buffer.len() as u64)
        {
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
        let this = self.project();
        let frame = ready!(this.inner.poll_frame(cx));
        match &frame {
            Some(Ok(frame)) => this.recorder.record(frame),
            Some(Err(_)) => this.recorder.head = None,
            None => this.recorder.finish(),
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
}
