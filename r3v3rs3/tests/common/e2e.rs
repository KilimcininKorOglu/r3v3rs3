//! The services of `tests/discovery/docker-compose.yml` for the ignored end-to-end tests.

use serde_json::{json, Value};

pub const CONSUL: &str = "http://127.0.0.1:8480";
/// The management token of the Consul agent.
pub const CONSUL_TOKEN: &str = "e2e-consul-token";
pub const ETCD: &str = "http://127.0.0.1:8481";
/// The password of the etcd `root` user.
pub const ETCD_PASSWORD: &str = "e2e-etcd-password";

pub async fn etcd_post(path: &str, body: Value, token: Option<&str>) -> anyhow::Result<Value> {
    let mut request = reqwest::Client::new()
        .post(format!("{ETCD}{path}"))
        .json(&body);
    if let Some(token) = token {
        request = request.header("Authorization", token);
    }
    let response = request.send().await?;
    let status = response.status();
    let body = response.json::<Value>().await?;
    anyhow::ensure!(status.is_success(), "etcd {path}: {status} {body}");
    Ok(body)
}

/// Adds the `root` user, enables authentication and returns a token. A cluster whose
/// authentication is already on only returns the token.
pub async fn etcd_token() -> anyhow::Result<String> {
    let credentials = json!({"name": "root", "password": ETCD_PASSWORD});
    let status = etcd_post("/v3/auth/status", json!({}), None).await?;
    if status["enabled"] != json!(true) {
        etcd_post("/v3/auth/user/add", credentials.clone(), None).await?;
        let grant = json!({"user": "root", "role": "root"});
        etcd_post("/v3/auth/user/grant", grant, None).await?;
        etcd_post("/v3/auth/enable", json!({}), None).await?;
    }
    let body = etcd_post("/v3/auth/authenticate", credentials, None).await?;
    body["token"]
        .as_str()
        .map(String::from)
        .ok_or_else(|| anyhow::anyhow!("etcd returned no token: {body}"))
}

/// Sends a request with the management token to the Consul HTTP API.
pub async fn consul_put(path: &str, body: String) -> anyhow::Result<Value> {
    let response = reqwest::Client::new()
        .put(format!("{CONSUL}{path}"))
        .header("X-Consul-Token", CONSUL_TOKEN)
        .body(body)
        .send()
        .await?
        .error_for_status()?;
    let text = response.text().await?;
    Ok(serde_json::from_str(&text).unwrap_or(Value::Null))
}
