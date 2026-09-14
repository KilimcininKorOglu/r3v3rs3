//! A client of the Consul HTTP API. It sends the ACL token, reads the index of each response and
//! sends blocking queries.

use super::http::{read_json, ApiClient, RESPONSE_TIMEOUT};
use anyhow::Context as _;
use bytes::Bytes;
use http_body_util::Full;
use hyper::body::Incoming;
use hyper::{Method, Request, Response, StatusCode};
use percent_encoding::{utf8_percent_encode, AsciiSet, NON_ALPHANUMERIC};
use serde_derive::Deserialize;
use std::time::Duration;

const TOKEN_HEADER: &str = "X-Consul-Token";
const INDEX_HEADER: &str = "X-Consul-Index";
/// The longest time that Consul holds a blocking query.
pub const WAIT: &str = "5m";
/// The wait time plus the jitter of up to 1/16 that Consul adds.
const BLOCKING_TIMEOUT: Duration = Duration::from_secs(6 * 60);
/// The characters that stay unencoded in a path segment or a query value.
const UNRESERVED: &AsciiSet = &NON_ALPHANUMERIC
    .remove(b'-')
    .remove(b'_')
    .remove(b'.')
    .remove(b'~');

#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "PascalCase")]
pub struct KeyValue {
    pub key: String,
    /// Base64 text. A folder has no value.
    #[serde(default)]
    pub value: Option<String>,
}

/// The path and the query of an API request. A datacenter that is not empty is added to the query.
pub fn path(datacenter: &str, segments: &[&str], query: &[(&str, &str)]) -> String {
    let mut path = String::new();
    for segment in segments {
        path.push('/');
        path.push_str(&utf8_percent_encode(segment, UNRESERVED).to_string());
    }
    let datacenter = Some(("dc", datacenter)).filter(|(_, dc)| !dc.is_empty());
    let pairs = query
        .iter()
        .copied()
        .chain(datacenter)
        .map(|(key, value)| format!("{key}={}", utf8_percent_encode(value, UNRESERVED)))
        .collect::<Vec<_>>();
    if !pairs.is_empty() {
        path.push('?');
        path.push_str(&pairs.join("&"));
    }
    path
}

/// The index of a response. Consul requires an index above zero in a blocking query.
fn consul_index(response: &Response<Incoming>) -> anyhow::Result<u64> {
    let index = response
        .headers()
        .get(INDEX_HEADER)
        .context("the response has no X-Consul-Index header")?
        .to_str()?
        .parse::<u64>()
        .context("invalid X-Consul-Index header")?;
    Ok(index.max(1))
}

pub struct ConsulClient {
    client: ApiClient,
    token: Option<String>,
}

impl ConsulClient {
    /// An empty token sends no token.
    pub fn new(client: ApiClient, token: Option<&str>) -> Self {
        Self {
            client,
            token: token.filter(|token| !token.is_empty()).map(str::to_string),
        }
    }

    /// Sends a GET request, and returns the response and its index. 404 is not an error.
    pub async fn get(&self, path: &str) -> anyhow::Result<(Response<Incoming>, u64)> {
        self.request(path, RESPONSE_TIMEOUT).await
    }

    /// Sends a blocking query and returns the new index. `path` has the `index` and the `wait`
    /// parameters.
    pub async fn block(&self, path: &str) -> anyhow::Result<u64> {
        let (_, index) = self.request(path, BLOCKING_TIMEOUT).await?;
        Ok(index)
    }

    /// Reads the keys of a recursive key-value path, and returns them with the index.
    pub async fn list(&self, path: &str) -> anyhow::Result<(Vec<KeyValue>, u64)> {
        let (response, index) = self.get(path).await?;
        // Consul answers 404 when no key has the prefix.
        if response.status() == StatusCode::NOT_FOUND {
            return Ok((Vec::new(), index));
        }
        Ok((read_json(response).await?, index))
    }

    async fn request(
        &self,
        path: &str,
        timeout: Duration,
    ) -> anyhow::Result<(Response<Incoming>, u64)> {
        let mut builder = self.client.builder(Method::GET, path);
        if let Some(token) = &self.token {
            builder = builder.header(TOKEN_HEADER, token);
        }
        let request: Request<Full<Bytes>> = builder.body(Full::new(Bytes::new()))?;
        let allowed = [StatusCode::NOT_FOUND];
        let response = self.client.send_with(request, timeout, &allowed).await?;
        let index = consul_index(&response)?;
        Ok((response, index))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn paths_encode_names_and_add_the_datacenter() {
        assert_eq!(
            path(
                "eu west",
                &["v1", "health", "service", "a/b"],
                &[("passing", "true")]
            ),
            "/v1/health/service/a%2Fb?passing=true&dc=eu%20west"
        );
        assert_eq!(
            path("", &["v1", "catalog", "services"], &[]),
            "/v1/catalog/services"
        );
    }
}
