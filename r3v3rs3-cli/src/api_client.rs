use anyhow::{Context, bail};
use rand::distr::SampleString;
use reqwest::Method;
use serde_json::{Value, json};

pub struct ApiClient {
    pub client: reqwest::Client,
    pub server: String,
    pub token: String,
}

pub struct ManifestItems {
    pub ports: Vec<Value>,
    pub proxies: Vec<Value>,
    pub apps: Vec<Value>,
}

#[derive(Default)]
pub struct SetCtConfigArgs<'a> {
    pub enable: Option<bool>,
    pub domains: Option<&'a str>,
    pub auto_discover: Option<bool>,
    pub allowed_issuers: Option<&'a str>,
    pub alert_unknown: Option<bool>,
    pub interval: Option<u64>,
    pub api_url: Option<&'a str>,
}

#[derive(Default)]
pub struct SetPrunePolicyArgs {
    pub enabled: Option<bool>,
    pub interval: Option<String>,
    pub disk_pressure: Option<u8>,
    pub prune_images: Option<bool>,
    pub prune_volumes: Option<bool>,
    pub prune_build_cache: Option<bool>,
    pub prune_containers: Option<bool>,
    pub prune_logs: Option<bool>,
    pub log_threshold_mb: Option<u64>,
    pub keep_cache_mb: Option<u64>,
    pub kept_builds: Option<u32>,
    pub dangling_layers_only: Option<bool>,
}

fn apply_prune_args(cur: &mut Value, args: &SetPrunePolicyArgs) {
    if let Some(en) = args.enabled {
        cur["enabled"] = json!(en);
    }
    if let Some(ref intv) = args.interval {
        cur["interval"] = json!(intv);
    }
    if let Some(dp) = args.disk_pressure {
        cur["disk_pressure_threshold"] = json!(dp);
    }
    apply_prune_flags(cur, args);
    apply_prune_limits(cur, args);
}

fn apply_prune_flags(cur: &mut Value, args: &SetPrunePolicyArgs) {
    if let Some(pi) = args.prune_images {
        cur["prune_images"] = json!(pi);
    }
    if let Some(pv) = args.prune_volumes {
        cur["prune_volumes"] = json!(pv);
    }
    if let Some(pb) = args.prune_build_cache {
        cur["prune_build_cache"] = json!(pb);
    }
    if let Some(pc) = args.prune_containers {
        cur["prune_containers"] = json!(pc);
    }
    if let Some(pl) = args.prune_logs {
        cur["prune_logs"] = json!(pl);
    }
}

fn apply_prune_limits(cur: &mut Value, args: &SetPrunePolicyArgs) {
    if let Some(lt) = args.log_threshold_mb {
        cur["log_vacuum_threshold_mb"] = json!(lt);
    }
    if let Some(kc) = args.keep_cache_mb {
        cur["build_cache_keep_storage_mb"] = json!(kc);
    }
    if let Some(kb) = args.kept_builds {
        cur["kept_builds_count"] = json!(kb);
    }
    if let Some(dl) = args.dangling_layers_only {
        cur["dangling_layers_only"] = json!(dl);
    }
}

pub fn load_payload(input: &str) -> anyhow::Result<Value> {
    let trimmed = input.trim();
    if trimmed.is_empty() {
        anyhow::bail!("payload is empty");
    }
    if trimmed == "-" {
        use std::io::Read;
        let mut buf = String::new();
        std::io::stdin().read_to_string(&mut buf)?;
        if buf.trim().is_empty() {
            anyhow::bail!("stdin payload is empty");
        }
        return parse_json_or_yaml(&buf, "stdin");
    }
    if let Ok(content) = std::fs::read_to_string(trimmed) {
        if content.trim().is_empty() {
            anyhow::bail!("file {trimmed} is empty");
        }
        return parse_json_or_yaml(&content, trimmed);
    }
    parse_json_or_yaml(trimmed, "payload")
}

fn parse_json_or_yaml(raw: &str, source: &str) -> anyhow::Result<Value> {
    let val: Value = serde_json::from_str(raw).or_else(|_| {
        serde_yaml::from_str(raw).map_err(|e| anyhow::anyhow!("failed to parse {source}: {e}"))
    })?;
    if val.is_null() {
        anyhow::bail!("empty payload in {source}");
    }
    Ok(val)
}

fn build_registry_payload(
    server: Option<String>,
    username: Option<String>,
    password: Option<String>,
    payload: Option<String>,
) -> anyhow::Result<Value> {
    if let Some(p) = payload {
        load_payload(&p)
    } else {
        let s = server
            .ok_or_else(|| anyhow::anyhow!("--server is required when payload is omitted"))?;
        let u = username
            .ok_or_else(|| anyhow::anyhow!("--username is required when payload is omitted"))?;
        Ok(json!({
            "server": s,
            "username": u,
            "password": password,
        }))
    }
}

fn build_git_connection_payload(
    name: Option<String>,
    provider: Option<String>,
    url: Option<String>,
    client_id: Option<String>,
    client_secret: Option<String>,
    payload: Option<String>,
) -> anyhow::Result<Value> {
    if let Some(p) = payload {
        load_payload(&p)
    } else {
        let n =
            name.ok_or_else(|| anyhow::anyhow!("--name is required when payload is omitted"))?;
        let p = provider
            .ok_or_else(|| anyhow::anyhow!("--provider is required when payload is omitted"))?;
        let cid = client_id
            .ok_or_else(|| anyhow::anyhow!("--client-id is required when payload is omitted"))?;
        Ok(json!({
            "name": n,
            "provider": p,
            "url": url,
            "client_id": cid,
            "client_secret": client_secret,
        }))
    }
}

fn build_account_create_payload(
    username: Option<String>,
    password: Option<String>,
    role: &str,
    payload: Option<String>,
) -> anyhow::Result<Value> {
    if let Some(p) = payload {
        load_payload(&p)
    } else {
        let u = username
            .ok_or_else(|| anyhow::anyhow!("--username is required when payload is omitted"))?;
        let p = password
            .ok_or_else(|| anyhow::anyhow!("--password is required when payload is omitted"))?;
        Ok(json!({
            "username": u,
            "password": p,
            "role": role,
        }))
    }
}

fn build_account_update_payload(
    password: Option<String>,
    role: Option<String>,
    payload: Option<String>,
) -> anyhow::Result<Value> {
    if let Some(p) = payload {
        load_payload(&p)
    } else {
        let mut map = serde_json::Map::new();
        if let Some(pwd) = password {
            map.insert("password".to_string(), json!(pwd));
        }
        if let Some(r) = role {
            map.insert("role".to_string(), json!(r));
        }
        Ok(Value::Object(map))
    }
}

fn build_token_create_payload(
    name: Option<String>,
    scope: &str,
    payload: Option<String>,
) -> anyhow::Result<Value> {
    if let Some(p) = payload {
        load_payload(&p)
    } else {
        let n =
            name.ok_or_else(|| anyhow::anyhow!("--name is required when payload is omitted"))?;
        Ok(json!({
            "name": n,
            "scope": scope,
        }))
    }
}

impl ApiClient {
    pub fn new(client: reqwest::Client, server: String, token: String) -> Self {
        Self {
            client,
            server,
            token,
        }
    }

    pub async fn request(
        &self,
        method: Method,
        path: &str,
        body: Option<&Value>,
    ) -> anyhow::Result<Value> {
        let url = format!("{}{path}", self.server);
        let mut req = self
            .client
            .request(method, &url)
            .header("Authorization", format!("Bearer {}", self.token));
        if let Some(body) = body {
            req = req.json(body);
        }
        let res = req
            .send()
            .await
            .context("failed to communicate with server")?;
        let status = res.status();
        let text = res.text().await.unwrap_or_default();
        if !status.is_success() {
            bail!("API error {status}: {text}");
        }
        if text.is_empty() {
            return Ok(Value::Null);
        }
        serde_json::from_str(&text).or_else(|_| Ok(Value::String(text)))
    }

    // --- Port operations ---

    pub async fn list_ports(&self) -> anyhow::Result<()> {
        let res = self.request(Method::GET, "/api/ports", None).await?;
        if let Some(arr) = res.as_array() {
            println!("{:<26} {:<32} STATUS", "ID", "LISTEN ADDRESS");
            println!("{:-<26} {:-<32} {:-<10}", "", "", "");
            for item in arr {
                let id = item.get("id").and_then(|v| v.as_str()).unwrap_or("-");
                let listen = item
                    .get("port")
                    .and_then(|p| p.get("listen"))
                    .and_then(|v| v.as_str())
                    .unwrap_or("-");
                let active = item
                    .get("port")
                    .and_then(|p| p.get("active"))
                    .and_then(|v| v.as_bool())
                    .unwrap_or(true);
                let status = if active { "active" } else { "inactive" };
                println!("{:<26} {:<32} {}", id, listen, status);
            }
        } else {
            print_json(&res)?;
        }
        Ok(())
    }

    pub async fn get_port(&self, id: &str) -> anyhow::Result<()> {
        let res = self
            .request(Method::GET, &format!("/api/ports/{id}"), None)
            .await?;
        print_json(&res)
    }

    pub async fn add_port(&self, payload: &str) -> anyhow::Result<()> {
        let val = load_payload(payload)?;
        let res = self.request(Method::POST, "/api/ports", Some(&val)).await?;
        println!("port added");
        print_json(&res)
    }

    pub async fn update_port(&self, id: &str, payload: &str) -> anyhow::Result<()> {
        let val = load_payload(payload)?;
        let res = self
            .request(Method::PUT, &format!("/api/ports/{id}"), Some(&val))
            .await?;
        println!("port/{id} updated");
        print_json(&res)
    }

    pub async fn delete_port(&self, id: &str) -> anyhow::Result<()> {
        self.request(Method::DELETE, &format!("/api/ports/{id}"), None)
            .await?;
        println!("port/{id} deleted");
        Ok(())
    }

    pub async fn reset_port(&self, id: &str) -> anyhow::Result<()> {
        self.request(Method::GET, &format!("/api/ports/{id}/reset"), None)
            .await?;
        println!("port/{id} listener reset");
        Ok(())
    }

    pub async fn set_port_early_data(&self, id: &str, enable: bool) -> anyhow::Result<()> {
        let entry = self
            .request(Method::GET, &format!("/api/ports/{id}"), None)
            .await?;
        let mut port = entry.get("port").cloned().unwrap_or(entry);
        if let Some(opts) = port.get_mut("opts") {
            if let Some(tls) = opts.get_mut("tls_termination") {
                tls["early_data"] = serde_json::json!(enable);
            } else if enable {
                anyhow::bail!("port/{id} does not have TLS termination configured");
            }
        }
        let res = self
            .request(Method::PUT, &format!("/api/ports/{id}"), Some(&port))
            .await?;
        println!("port/{id} early_data set to {enable}");
        print_json(&res)
    }

    // --- Proxy operations ---

    pub async fn list_proxies(&self) -> anyhow::Result<()> {
        let res = self.request(Method::GET, "/api/proxies", None).await?;
        if let Some(arr) = res.as_array() {
            println!("{:<26} {:<24} {:<12} PORTS", "ID", "NAME", "PROTOCOL");
            println!("{:-<26} {:-<24} {:-<12} {:-<16}", "", "", "", "");
            for item in arr {
                let id = item.get("id").and_then(|v| v.as_str()).unwrap_or("-");
                let name = item.get("name").and_then(|v| v.as_str()).unwrap_or("-");
                let protocol = item
                    .get("protocol")
                    .and_then(|v| v.as_str())
                    .unwrap_or("http");
                let ports = item
                    .get("ports")
                    .and_then(|v| v.as_array())
                    .map(|arr| {
                        arr.iter()
                            .filter_map(|p| p.as_str())
                            .collect::<Vec<_>>()
                            .join(", ")
                    })
                    .unwrap_or_default();
                println!("{:<26} {:<24} {:<12} {}", id, name, protocol, ports);
            }
        } else {
            print_json(&res)?;
        }
        Ok(())
    }

    pub async fn get_proxy(&self, id: &str) -> anyhow::Result<()> {
        let res = self
            .request(Method::GET, &format!("/api/proxies/{id}"), None)
            .await?;
        print_json(&res)
    }

    pub async fn add_proxy(&self, payload: &str) -> anyhow::Result<()> {
        let val = load_payload(payload)?;
        let res = self
            .request(Method::POST, "/api/proxies", Some(&val))
            .await?;
        println!("proxy added");
        print_json(&res)
    }

    pub async fn update_proxy(&self, id: &str, payload: &str) -> anyhow::Result<()> {
        let val = load_payload(payload)?;
        let res = self
            .request(Method::PUT, &format!("/api/proxies/{id}"), Some(&val))
            .await?;
        println!("proxy/{id} updated");
        print_json(&res)
    }

    pub async fn proxy_status(&self, id: &str) -> anyhow::Result<()> {
        let res = self
            .request(Method::GET, &format!("/api/proxies/{id}/status"), None)
            .await?;
        print_json(&res)
    }

    pub async fn check_proxy_grpc_health(
        &self,
        id: &str,
        service: Option<&str>,
        target_url: Option<&str>,
    ) -> anyhow::Result<()> {
        let mut map = serde_json::Map::new();
        if let Some(t) = target_url {
            map.insert(
                "target_url".to_string(),
                serde_json::Value::String(t.to_string()),
            );
        }
        if let Some(s) = service {
            map.insert(
                "service".to_string(),
                serde_json::Value::String(s.to_string()),
            );
        }
        let body = serde_json::Value::Object(map);
        let res = self
            .request(
                Method::POST,
                &format!("/api/proxies/{id}/grpc/health"),
                Some(&body),
            )
            .await?;
        let healthy = res
            .get("healthy")
            .and_then(|v| v.as_bool())
            .unwrap_or(false);
        let status = res
            .get("status")
            .and_then(|v| v.as_str())
            .unwrap_or("UNKNOWN");
        let status_code = res.get("status_code").and_then(|v| v.as_u64()).unwrap_or(0);
        let grpc_status = res.get("grpc_status").and_then(|v| v.as_u64()).unwrap_or(0);
        let grpc_name = res
            .get("grpc_status_name")
            .and_then(|v| v.as_str())
            .unwrap_or("UNKNOWN");
        let latency_ms = res.get("latency_ms").and_then(|v| v.as_u64()).unwrap_or(0);

        let status_colored = if healthy {
            format!("\x1b[32m{status}\x1b[0m")
        } else {
            format!("\x1b[31m{status}\x1b[0m")
        };
        println!("Proxy ID:       {id}");
        println!("Healthy:        {}", if healthy { "yes" } else { "no" });
        println!("Serving Status: {status_colored} (code {status_code})");
        println!("gRPC Status:    {grpc_status} ({grpc_name})");
        if let Some(msg) = res.get("grpc_message").and_then(|v| v.as_str()) {
            println!("gRPC Message:   {msg}");
        }
        println!("Latency:        {latency_ms} ms");
        Ok(())
    }

    pub async fn delete_proxy(&self, id: &str) -> anyhow::Result<()> {
        self.request(Method::DELETE, &format!("/api/proxies/{id}"), None)
            .await?;
        println!("proxy/{id} deleted");
        Ok(())
    }

    pub async fn purge_cache(&self, id: &str, path: Option<&str>) -> anyhow::Result<()> {
        let url = match path {
            Some(p) => {
                let encoded: String = url::form_urlencoded::byte_serialize(p.as_bytes()).collect();
                format!("/api/proxies/{id}/cache?path={encoded}")
            }
            None => format!("/api/proxies/{id}/cache"),
        };
        self.request(Method::DELETE, &url, None).await?;
        println!("proxy/{id} cache purged");
        Ok(())
    }

    pub async fn list_proxy_dictionaries(&self, proxy_id: &str) -> anyhow::Result<()> {
        let res = self
            .request(Method::GET, &format!("/api/proxies/{proxy_id}"), None)
            .await?;
        let dicts = res
            .pointer("/proxy/compression/dictionaries")
            .and_then(|v| v.as_array());
        let Some(arr) = dicts else {
            println!("No compression dictionaries found for proxy {proxy_id}");
            return Ok(());
        };
        if arr.is_empty() {
            println!("No compression dictionaries configured for proxy {proxy_id}");
            return Ok(());
        }
        println!(
            "{:<26} {:<20} {:<15} {:<30} HASH",
            "ID", "NAME", "MATCH", "PATH"
        );
        println!(
            "{:-<26} {:-<20} {:-<15} {:-<30} {:-<30}",
            "", "", "", "", ""
        );
        for item in arr {
            let id = item.get("id").and_then(|v| v.as_str()).unwrap_or("-");
            let name = item.get("name").and_then(|v| v.as_str()).unwrap_or("-");
            let match_pat = item
                .get("match_pattern")
                .and_then(|v| v.as_str())
                .unwrap_or("-");
            let path = item.get("path").and_then(|v| v.as_str()).unwrap_or("-");
            let hash = item
                .get("sha256_sf")
                .and_then(|v| v.as_str())
                .unwrap_or("-");
            println!(
                "{:<26} {:<20} {:<15} {:<30} {}",
                id, name, match_pat, path, hash
            );
        }
        Ok(())
    }

    pub async fn get_proxy_dictionary(&self, proxy_id: &str, dict_id: &str) -> anyhow::Result<()> {
        let res = self
            .request(Method::GET, &format!("/api/proxies/{proxy_id}"), None)
            .await?;
        let dict = res
            .pointer("/proxy/compression/dictionaries")
            .and_then(|v| v.as_array())
            .and_then(|arr| {
                arr.iter()
                    .find(|d| d.get("id").and_then(|id| id.as_str()) == Some(dict_id))
            });
        let Some(d) = dict else {
            bail!("Dictionary {dict_id} not found in proxy {proxy_id}");
        };
        print_json(d)
    }

    pub async fn add_proxy_dictionary(
        &self,
        proxy_id: &str,
        name: &str,
        match_pattern: &str,
        path: &str,
        data: &str,
    ) -> anyhow::Result<()> {
        let raw_data = if let Some(file_path) = data.strip_prefix('@') {
            std::fs::read_to_string(file_path)
                .with_context(|| format!("failed to read dictionary file from {file_path}"))?
        } else {
            data.to_string()
        };
        let mut res = self
            .request(Method::GET, &format!("/api/proxies/{proxy_id}"), None)
            .await?;
        let dict_id = rand::distr::Alphanumeric
            .sample_string(&mut rand::rng(), 24)
            .to_ascii_lowercase();
        let dict = json!({
            "id": dict_id,
            "name": name,
            "match_pattern": match_pattern,
            "path": path,
            "raw_data": raw_data,
        });
        let Some(proxy) = res.get_mut("proxy") else {
            bail!("Invalid proxy response from server");
        };
        if proxy.get("compression").is_none() {
            proxy["compression"] = json!({});
        }
        if proxy["compression"].get("dictionaries").is_none() {
            proxy["compression"]["dictionaries"] = json!([]);
        }
        if let Some(arr) = proxy["compression"]["dictionaries"].as_array_mut() {
            arr.push(dict);
        }
        let updated = self
            .request(Method::PUT, &format!("/api/proxies/{proxy_id}"), Some(&res))
            .await?;
        println!("Compression dictionary added to proxy {proxy_id}");
        print_json(&updated)
    }

    pub async fn delete_proxy_dictionary(
        &self,
        proxy_id: &str,
        dict_id: &str,
    ) -> anyhow::Result<()> {
        let mut res = self
            .request(Method::GET, &format!("/api/proxies/{proxy_id}"), None)
            .await?;
        let Some(arr) = res
            .pointer_mut("/proxy/compression/dictionaries")
            .and_then(|v| v.as_array_mut())
        else {
            bail!("No compression dictionaries found in proxy {proxy_id}");
        };
        let initial_len = arr.len();
        arr.retain(|d| d.get("id").and_then(|id| id.as_str()) != Some(dict_id));
        if arr.len() == initial_len {
            bail!("Dictionary {dict_id} not found in proxy {proxy_id}");
        }
        let updated = self
            .request(Method::PUT, &format!("/api/proxies/{proxy_id}"), Some(&res))
            .await?;
        println!("Compression dictionary {dict_id} deleted from proxy {proxy_id}");
        print_json(&updated)
    }

    // --- Certificate operations ---

    pub async fn list_certs(&self) -> anyhow::Result<()> {
        let res = self.request(Method::GET, "/api/certs", None).await?;
        if let Some(arr) = res.as_array() {
            println!("{:<26} {:<30} {:<12} ISSUER", "ID", "DOMAINS", "EXPIRES");
            println!("{:-<26} {:-<30} {:-<12} {:-<20}", "", "", "", "");
            for item in arr {
                let id = item.get("id").and_then(|v| v.as_str()).unwrap_or("-");
                let domains = item
                    .get("domains")
                    .and_then(|v| v.as_array())
                    .map(|d| {
                        d.iter()
                            .filter_map(|s| s.as_str())
                            .collect::<Vec<_>>()
                            .join(", ")
                    })
                    .unwrap_or_default();
                let expires = item.get("expiration").and_then(|v| v.as_u64()).unwrap_or(0);
                let issuer = item.get("issuer").and_then(|v| v.as_str()).unwrap_or("-");
                println!("{:<26} {:<30} {:<12} {}", id, domains, expires, issuer);
            }
        } else {
            print_json(&res)?;
        }
        Ok(())
    }

    pub async fn get_cert(&self, id: &str) -> anyhow::Result<()> {
        let res = self
            .request(Method::GET, &format!("/api/certs/{id}"), None)
            .await?;
        print_json(&res)
    }

    pub async fn delete_cert(&self, id: &str) -> anyhow::Result<()> {
        self.request(Method::DELETE, &format!("/api/certs/{id}"), None)
            .await?;
        println!("cert/{id} deleted");
        Ok(())
    }

    pub async fn cert_coverage(&self) -> anyhow::Result<()> {
        let res = self
            .request(Method::POST, "/api/certs/coverage", None)
            .await?;
        print_json(&res)
    }

    pub async fn upload_cert(
        &self,
        cert_path: &str,
        key_path: &str,
        kind: Option<&str>,
    ) -> anyhow::Result<()> {
        let cert_bytes =
            std::fs::read(cert_path).context("failed to read certificate chain file")?;
        let key_bytes = std::fs::read(key_path).context("failed to read private key file")?;
        let kind = kind.unwrap_or("server");

        let part_chain = reqwest::multipart::Part::bytes(cert_bytes)
            .file_name("chain.pem")
            .mime_str("application/x-pem-file")?;
        let part_key = reqwest::multipart::Part::bytes(key_bytes)
            .file_name("key.pem")
            .mime_str("application/x-pem-file")?;

        let form = reqwest::multipart::Form::new()
            .part("chain", part_chain)
            .part("key", part_key);

        let url = format!("{}/api/certs/upload?kind={kind}", self.server);
        let res = self
            .client
            .post(&url)
            .header("Authorization", format!("Bearer {}", self.token))
            .multipart(form)
            .send()
            .await?;
        if !res.status().is_success() {
            let text = res.text().await.unwrap_or_default();
            bail!("failed to upload certificate: {text}");
        }
        println!("certificate uploaded successfully");
        Ok(())
    }

    // --- ACME operations ---

    pub async fn list_acme(&self) -> anyhow::Result<()> {
        let res = self.request(Method::GET, "/api/acme", None).await?;
        if let Some(arr) = res.as_array() {
            println!("{:<26} {:<32} IDENTIFIERS", "ID", "ACME SERVER");
            println!("{:-<26} {:-<32} {:-<30}", "", "", "");
            for item in arr {
                let id = item.get("id").and_then(|v| v.as_str()).unwrap_or("-");
                let server = item.get("server").and_then(|v| v.as_str()).unwrap_or("-");
                let identifiers = item
                    .get("identifiers")
                    .and_then(|v| v.as_array())
                    .map(|d| {
                        d.iter()
                            .filter_map(|s| s.as_str())
                            .collect::<Vec<_>>()
                            .join(", ")
                    })
                    .unwrap_or_default();
                println!("{:<26} {:<32} {}", id, server, identifiers);
            }
        } else {
            print_json(&res)?;
        }
        Ok(())
    }

    pub async fn get_acme(&self, id: &str) -> anyhow::Result<()> {
        let res = self
            .request(Method::GET, &format!("/api/acme/{id}"), None)
            .await?;
        print_json(&res)
    }

    pub async fn add_acme(&self, payload: &str) -> anyhow::Result<()> {
        let val = load_payload(payload)?;
        let res = self.request(Method::POST, "/api/acme", Some(&val)).await?;
        println!("acme order added");
        print_json(&res)
    }

    pub async fn update_acme(&self, id: &str, payload: &str) -> anyhow::Result<()> {
        let val = load_payload(payload)?;
        let res = self
            .request(Method::PUT, &format!("/api/acme/{id}"), Some(&val))
            .await?;
        println!("acme/{id} updated");
        print_json(&res)
    }

    pub async fn delete_acme(&self, id: &str) -> anyhow::Result<()> {
        self.request(Method::DELETE, &format!("/api/acme/{id}"), None)
            .await?;
        println!("acme/{id} deleted");
        Ok(())
    }

    pub async fn order_acme(&self, id: &str) -> anyhow::Result<()> {
        let res = self
            .request(Method::POST, &format!("/api/acme/{id}/order"), None)
            .await?;
        println!("acme/{id} order triggered");
        print_json(&res)
    }

    pub async fn get_acme_cname_instructions(&self, id: &str) -> anyhow::Result<()> {
        let res = self
            .request(
                Method::GET,
                &format!("/api/acme/{id}/cname_instructions"),
                None,
            )
            .await?;
        if let Some(arr) = res.as_array() {
            println!(
                "{:<24} {:<40} {:<40} VERIFIED",
                "DOMAIN", "SOURCE (CUSTOMER CNAME)", "TARGET (ALIAS)"
            );
            println!("{:-<24} {:-<40} {:-<40} {:-<8}", "", "", "", "");
            for item in arr {
                let domain = item.get("domain").and_then(|v| v.as_str()).unwrap_or("-");
                let source = item
                    .get("source_fqdn")
                    .and_then(|v| v.as_str())
                    .unwrap_or("-");
                let target = item
                    .get("target_fqdn")
                    .and_then(|v| v.as_str())
                    .unwrap_or("-");
                let verified = item
                    .get("cname_verified")
                    .and_then(|v| v.as_bool())
                    .unwrap_or(false);
                let mark = if verified { "✓ YES" } else { "✗ NO" };
                println!("{:<24} {:<40} {:<40} {}", domain, source, target, mark);
            }
        } else {
            print_json(&res)?;
        }
        Ok(())
    }

    // --- Access List operations ---

    pub async fn list_acls(&self) -> anyhow::Result<()> {
        let res = self.request(Method::GET, "/api/access-lists", None).await?;
        if let Some(arr) = res.as_array() {
            println!("{:<26} {:<30} RULES", "ID", "NAME");
            println!("{:-<26} {:-<30} {:-<10}", "", "", "");
            for item in arr {
                let id = item.get("id").and_then(|v| v.as_str()).unwrap_or("-");
                let name = item
                    .get("entry")
                    .and_then(|e| e.get("name"))
                    .and_then(|v| v.as_str())
                    .unwrap_or("-");
                let rules = item
                    .get("entry")
                    .and_then(|e| e.get("rules"))
                    .and_then(|r| r.as_array())
                    .map(|a| a.len())
                    .unwrap_or(0);
                println!("{:<26} {:<30} {}", id, name, rules);
            }
        } else {
            print_json(&res)?;
        }
        Ok(())
    }

    pub async fn get_acl(&self, id: &str) -> anyhow::Result<()> {
        let res = self
            .request(Method::GET, &format!("/api/access-lists/{id}"), None)
            .await?;
        print_json(&res)
    }

    pub async fn add_acl(&self, payload: &str) -> anyhow::Result<()> {
        let val = load_payload(payload)?;
        let res = self
            .request(Method::POST, "/api/access-lists", Some(&val))
            .await?;
        println!("access-list added");
        print_json(&res)
    }

    pub async fn update_acl(&self, id: &str, payload: &str) -> anyhow::Result<()> {
        let val = load_payload(payload)?;
        let res = self
            .request(Method::PUT, &format!("/api/access-lists/{id}"), Some(&val))
            .await?;
        println!("access-list/{id} updated");
        print_json(&res)
    }

    pub async fn delete_acl(&self, id: &str) -> anyhow::Result<()> {
        self.request(Method::DELETE, &format!("/api/access-lists/{id}"), None)
            .await?;
        println!("access-list/{id} deleted");
        Ok(())
    }

    // --- Error Page Themes operations ---

    pub async fn list_error_page_themes(&self) -> anyhow::Result<()> {
        let res = self
            .request(Method::GET, "/api/error-page-themes", None)
            .await?;
        if let Some(arr) = res.as_array() {
            println!("{:<26} {:<28} {:<10} DESCRIPTION", "ID", "NAME", "PAGES");
            println!("{:-<26} {:-<28} {:-<10} {:-<30}", "", "", "", "");
            for item in arr {
                let id = item.get("id").and_then(|v| v.as_str()).unwrap_or("-");
                let name = item.get("name").and_then(|v| v.as_str()).unwrap_or("-");
                let pages = item
                    .get("pages")
                    .and_then(|p| p.as_array())
                    .map(|a| a.len())
                    .unwrap_or(0);
                let desc = item
                    .get("description")
                    .and_then(|v| v.as_str())
                    .unwrap_or("-");
                println!("{:<26} {:<28} {:<10} {}", id, name, pages, desc);
            }
        } else {
            print_json(&res)?;
        }
        Ok(())
    }

    pub async fn get_error_page_theme(&self, id: &str) -> anyhow::Result<()> {
        let res = self
            .request(Method::GET, &format!("/api/error-page-themes/{id}"), None)
            .await?;
        print_json(&res)
    }

    pub async fn add_error_page_theme(&self, payload: &str) -> anyhow::Result<()> {
        let val = load_payload(payload)?;
        let res = self
            .request(Method::POST, "/api/error-page-themes", Some(&val))
            .await?;
        println!("error-page-theme added");
        print_json(&res)
    }

    pub async fn update_error_page_theme(&self, id: &str, payload: &str) -> anyhow::Result<()> {
        let val = load_payload(payload)?;
        let res = self
            .request(
                Method::PUT,
                &format!("/api/error-page-themes/{id}"),
                Some(&val),
            )
            .await?;
        println!("error-page-theme/{id} updated");
        print_json(&res)
    }

    pub async fn delete_error_page_theme(&self, id: &str) -> anyhow::Result<()> {
        self.request(
            Method::DELETE,
            &format!("/api/error-page-themes/{id}"),
            None,
        )
        .await?;
        println!("error-page-theme/{id} deleted");
        Ok(())
    }

    pub async fn reset_error_page_themes(&self) -> anyhow::Result<()> {
        let res = self
            .request(Method::POST, "/api/error-page-themes/reset-defaults", None)
            .await?;
        println!("error-page-themes reset to defaults");
        print_json(&res)
    }

    // --- DNS Credentials operations ---

    pub async fn list_dns(&self) -> anyhow::Result<()> {
        let res = self
            .request(Method::GET, "/api/dns-credentials", None)
            .await?;
        if let Some(arr) = res.as_array() {
            println!("{:<26} {:<30} PROVIDER", "ID", "NAME");
            println!("{:-<26} {:-<30} {:-<20}", "", "", "");
            for item in arr {
                let id = item.get("id").and_then(|v| v.as_str()).unwrap_or("-");
                let name = item.get("name").and_then(|v| v.as_str()).unwrap_or("-");
                let provider = item.get("provider").and_then(|v| v.as_str()).unwrap_or("-");
                println!("{:<26} {:<30} {}", id, name, provider);
            }
        } else {
            print_json(&res)?;
        }
        Ok(())
    }

    pub async fn get_dns(&self, id: &str) -> anyhow::Result<()> {
        let res = self
            .request(Method::GET, &format!("/api/dns-credentials/{id}"), None)
            .await?;
        print_json(&res)
    }

    pub async fn add_dns(&self, payload: &str) -> anyhow::Result<()> {
        let val = load_payload(payload)?;
        let res = self
            .request(Method::POST, "/api/dns-credentials", Some(&val))
            .await?;
        println!("dns credential added");
        print_json(&res)
    }

    pub async fn update_dns(&self, id: &str, payload: &str) -> anyhow::Result<()> {
        let val = load_payload(payload)?;
        let res = self
            .request(
                Method::PUT,
                &format!("/api/dns-credentials/{id}"),
                Some(&val),
            )
            .await?;
        println!("dns-credential/{id} updated");
        print_json(&res)
    }

    pub async fn delete_dns(&self, id: &str) -> anyhow::Result<()> {
        self.request(Method::DELETE, &format!("/api/dns-credentials/{id}"), None)
            .await?;
        println!("dns-credential/{id} deleted");
        Ok(())
    }

    // --- App operations ---

    pub async fn list_apps(&self) -> anyhow::Result<()> {
        let res = self.request(Method::GET, "/api/apps", None).await?;
        if let Some(arr) = res.as_array() {
            println!("{:<26} {:<20} {:<14} DOMAINS", "ID", "NAME", "STATUS");
            println!("{:-<26} {:-<20} {:-<14} {:-<20}", "", "", "", "");
            for item in arr {
                let id = item.get("id").and_then(|v| v.as_str()).unwrap_or("-");
                let spec = item.get("spec");
                let name = spec
                    .and_then(|s| s.get("name"))
                    .and_then(|v| v.as_str())
                    .unwrap_or("-");
                let status = item.get("status").and_then(|v| v.as_str()).unwrap_or("-");
                let domains = spec
                    .and_then(|s| s.get("domains"))
                    .and_then(|v| v.as_array())
                    .map(|d| {
                        d.iter()
                            .filter_map(|dom| dom.as_str())
                            .collect::<Vec<_>>()
                            .join(", ")
                    })
                    .unwrap_or_default();
                println!("{:<26} {:<20} {:<14} {}", id, name, status, domains);
            }
        } else {
            print_json(&res)?;
        }
        Ok(())
    }

    pub async fn get_app(&self, id: &str) -> anyhow::Result<()> {
        let res = self
            .request(Method::GET, &format!("/api/apps/{id}"), None)
            .await?;
        print_json(&res)
    }

    pub async fn deploy_app(&self, id: &str) -> anyhow::Result<()> {
        self.request(Method::POST, &format!("/api/apps/{id}/deploy"), None)
            .await?;
        println!("app/{id} deployment triggered");
        Ok(())
    }

    pub async fn rollback_app(&self, id: &str) -> anyhow::Result<()> {
        let dep_id = self.resolve_rollback_deployment_id(id).await?;
        self.request(
            Method::POST,
            &format!("/api/deployments/{dep_id}/rollback"),
            None,
        )
        .await?;
        println!("deployment/{dep_id} rollback queued");
        Ok(())
    }

    async fn resolve_rollback_deployment_id(&self, id: &str) -> anyhow::Result<String> {
        let deployments_url = format!("/api/apps/{id}/deployments");
        if let Ok(res) = self.request(Method::GET, &deployments_url, None).await {
            let target = res
                .as_array()
                .and_then(|arr| arr.get(1))
                .and_then(|d| d.get("id"))
                .and_then(|v| v.as_str());
            if let Some(target_dep) = target {
                return Ok(target_dep.to_string());
            }
        }
        Ok(id.to_string())
    }

    pub async fn start_app(&self, id: &str) -> anyhow::Result<()> {
        self.request(Method::POST, &format!("/api/apps/{id}/start"), None)
            .await?;
        println!("app/{id} started");
        Ok(())
    }

    pub async fn stop_app(&self, id: &str) -> anyhow::Result<()> {
        self.request(Method::POST, &format!("/api/apps/{id}/stop"), None)
            .await?;
        println!("app/{id} stopped");
        Ok(())
    }

    pub async fn restart_app(&self, id: &str) -> anyhow::Result<()> {
        self.request(Method::POST, &format!("/api/apps/{id}/restart"), None)
            .await?;
        println!("app/{id} restarted");
        Ok(())
    }

    pub async fn delete_app(&self, id: &str) -> anyhow::Result<()> {
        self.request(Method::DELETE, &format!("/api/apps/{id}"), None)
            .await?;
        println!("app/{id} deleted");
        Ok(())
    }

    pub async fn add_app(&self, payload: &str) -> anyhow::Result<()> {
        let val = load_payload(payload)?;
        let res = self.request(Method::POST, "/api/apps", Some(&val)).await?;
        println!("app created");
        print_json(&res)
    }

    pub async fn update_app(&self, id: &str, payload: &str) -> anyhow::Result<()> {
        let val = load_payload(payload)?;
        let res = self
            .request(Method::PUT, &format!("/api/apps/{id}"), Some(&val))
            .await?;
        println!("app/{id} updated");
        print_json(&res)
    }

    pub async fn app_logs(&self, id: &str, tail: usize) -> anyhow::Result<()> {
        let res = self
            .request(
                Method::GET,
                &format!("/api/apps/{id}/logs?tail={tail}"),
                None,
            )
            .await?;
        if let Some(arr) = res.as_array() {
            for line in arr {
                if let Some(text) = line.get("message").and_then(|m| m.as_str()) {
                    println!("{}", text);
                } else if let Some(text) = line.as_str() {
                    println!("{}", text);
                }
            }
        } else if let Some(text) = res.as_str() {
            println!("{}", text);
        } else {
            print_json(&res)?;
        }
        Ok(())
    }

    pub async fn app_dashboard(&self, id: &str) -> anyhow::Result<()> {
        let res = self
            .request(Method::GET, &format!("/api/apps/{id}/dashboard"), None)
            .await?;
        print_json(&res)
    }

    pub async fn app_diagnostics(&self, id: &str) -> anyhow::Result<()> {
        let res = self
            .request(Method::GET, &format!("/api/apps/{id}/diagnostics"), None)
            .await?;
        print_json(&res)
    }

    pub async fn app_log_size(&self, id: &str) -> anyhow::Result<()> {
        let res = self
            .request(Method::GET, &format!("/api/apps/{id}/logs/size"), None)
            .await?;
        print_json(&res)
    }

    pub async fn vacuum_app_logs(&self, id: &str) -> anyhow::Result<()> {
        let res = self
            .request(Method::POST, &format!("/api/apps/{id}/logs/vacuum"), None)
            .await?;
        println!("app/{id} container logs vacuumed");
        print_json(&res)
    }

    pub async fn get_app_env(&self, id: &str) -> anyhow::Result<()> {
        let res = self
            .request(Method::GET, &format!("/api/apps/{id}/env"), None)
            .await?;
        print_json(&res)
    }

    pub async fn set_app_env(&self, id: &str, pairs: &[String]) -> anyhow::Result<()> {
        let mut entries = Vec::new();
        for pair in pairs {
            if let Some((k, v)) = pair.split_once('=') {
                let is_secret = is_likely_secret_key(k);
                entries.push(json!({
                    "key": k,
                    "value": v,
                    "secret": is_secret,
                }));
            } else {
                bail!("invalid env pair '{pair}', expected KEY=VALUE format");
            }
        }
        let body = Value::Array(entries);
        self.request(Method::PUT, &format!("/api/apps/{id}/env"), Some(&body))
            .await?;
        println!("app/{id} environment variables updated");
        Ok(())
    }

    pub async fn generate_app_env(
        &self,
        id: &str,
        save: bool,
        overwrite: bool,
    ) -> anyhow::Result<()> {
        let body = json!({
            "save": save,
            "overwrite": overwrite,
        });
        let res = self
            .request(
                Method::POST,
                &format!("/api/apps/{id}/env/generate"),
                Some(&body),
            )
            .await?;
        print_json(&res)
    }

    pub async fn resolve_env_template(
        &self,
        template: Option<&str>,
        file: Option<&std::path::Path>,
        overwrite: bool,
    ) -> anyhow::Result<()> {
        let template_content = if let Some(path) = file {
            Some(std::fs::read_to_string(path)?)
        } else {
            template.map(|t| t.to_string())
        };
        let body = json!({
            "template": template_content,
            "overwrite": overwrite,
        });
        let res = self
            .request(Method::POST, "/api/compose/env/generate", Some(&body))
            .await?;
        print_json(&res)
    }
}

fn is_likely_secret_key(key: &str) -> bool {
    let upper = key.to_ascii_uppercase();
    upper.ends_with("_PASSWORD")
        || upper.ends_with("_SECRET")
        || upper.ends_with("_KEY")
        || upper.ends_with("_TOKEN")
        || upper.contains("PASSWORD")
        || upper.contains("SECRET")
}

impl ApiClient {
    pub async fn app_hooks(
        &self,
        id: &str,
        pre_deploy: Option<String>,
        post_deploy: Option<String>,
        clear: bool,
    ) -> anyhow::Result<()> {
        let mut app_val = self
            .request(Method::GET, &format!("/api/apps/{id}"), None)
            .await?;
        if clear {
            app_val["spec"]["lifecycle_hooks"] = serde_json::Value::Null;
            self.request(
                Method::PUT,
                &format!("/api/apps/{id}"),
                Some(&json!({
                    "name": app_val["name"],
                    "target": app_val["target"],
                    "spec": app_val["spec"],
                })),
            )
            .await?;
            println!("app/{id} lifecycle hooks cleared");
            return Ok(());
        }
        if pre_deploy.is_some() || post_deploy.is_some() {
            let mut hooks = app_val["spec"]["lifecycle_hooks"]
                .as_object()
                .cloned()
                .unwrap_or_default();
            if let Some(cmd) = pre_deploy {
                hooks.insert("pre_deploy".to_string(), json!(cmd));
            }
            if let Some(cmd) = post_deploy {
                hooks.insert("post_deploy".to_string(), json!(cmd));
            }
            app_val["spec"]["lifecycle_hooks"] = json!(hooks);
            self.request(
                Method::PUT,
                &format!("/api/apps/{id}"),
                Some(&json!({
                    "name": app_val["name"],
                    "target": app_val["target"],
                    "spec": app_val["spec"],
                })),
            )
            .await?;
            println!("app/{id} lifecycle hooks updated");
            return Ok(());
        }
        let hooks = &app_val["spec"]["lifecycle_hooks"];
        print_json(hooks)
    }

    // --- Backup operations ---

    pub async fn list_backups(&self, database_id: &str) -> anyhow::Result<()> {
        let res = self
            .request(
                Method::GET,
                &format!("/api/databases/{database_id}/backups"),
                None,
            )
            .await?;
        if let Some(arr) = res.as_array() {
            println!(
                "{:<26} {:<10} {:<10} CREATED",
                "BACKUP ID", "SIZE", "STATUS"
            );
            println!("{:-<26} {:-<10} {:-<10} {:-<20}", "", "", "", "");
            for item in arr {
                let id = item.get("id").and_then(|v| v.as_str()).unwrap_or("-");
                let size = item.get("size").and_then(|v| v.as_u64()).unwrap_or(0);
                let status = item.get("status").and_then(|v| v.as_str()).unwrap_or("-");
                let time = item.get("time").and_then(|v| v.as_u64()).unwrap_or(0);
                println!(
                    "{:<26} {:<10} {:<10} {}",
                    id,
                    format!("{size}B"),
                    status,
                    time
                );
            }
        } else {
            print_json(&res)?;
        }
        Ok(())
    }

    pub async fn create_backup(&self, database_id: &str) -> anyhow::Result<()> {
        self.request(
            Method::POST,
            &format!("/api/databases/{database_id}/backups"),
            None,
        )
        .await?;
        println!("backup for database/{database_id} initiated");
        Ok(())
    }

    pub async fn restore_backup(&self, database_id: &str, backup_id: &str) -> anyhow::Result<()> {
        self.request(
            Method::POST,
            &format!("/api/databases/{database_id}/backups/{backup_id}/restore"),
            None,
        )
        .await?;
        println!("database/{database_id} restore initiated from backup/{backup_id}");
        Ok(())
    }

    pub async fn backup_config(&self) -> anyhow::Result<()> {
        let res = self
            .request(Method::GET, "/api/backup-config", None)
            .await?;
        print_json(&res)
    }

    pub async fn delete_backup(&self, database_id: &str, backup_id: &str) -> anyhow::Result<()> {
        self.request(
            Method::DELETE,
            &format!("/api/databases/{database_id}/backups/{backup_id}"),
            None,
        )
        .await?;
        println!("backup/{backup_id} of database/{database_id} deleted");
        Ok(())
    }

    // --- Disaster Recovery operations ---

    pub async fn list_dr_backups(&self) -> anyhow::Result<()> {
        let res = self
            .request(Method::GET, "/api/disaster-recovery/backups", None)
            .await?;
        if let Some(arr) = res.as_array() {
            println!(
                "{:<26} {:<10} {:<8} {:<15} CREATED",
                "SNAPSHOT ID", "SIZE", "FILES", "S3 REPLICATED"
            );
            println!("{:-<26} {:-<10} {:-<8} {:-<15} {:-<20}", "", "", "", "", "");
            for item in arr {
                let id = item.get("id").and_then(|v| v.as_str()).unwrap_or("-");
                let size = item.get("size").and_then(|v| v.as_u64()).unwrap_or(0);
                let files = item
                    .get("files_count")
                    .and_then(|v| v.as_u64())
                    .unwrap_or(0);
                let s3 = item
                    .get("s3_replicated")
                    .and_then(|v| v.as_bool())
                    .unwrap_or(false);
                let time = item.get("created_at").and_then(|v| v.as_u64()).unwrap_or(0);
                println!(
                    "{:<26} {:<10} {:<8} {:<15} {}",
                    id,
                    format!("{size}B"),
                    files,
                    if s3 { "yes" } else { "no" },
                    time
                );
            }
        } else {
            print_json(&res)?;
        }
        Ok(())
    }

    pub async fn create_dr_backup(
        &self,
        passphrase: Option<&str>,
        upload_s3: bool,
    ) -> anyhow::Result<()> {
        let payload = json!({
            "passphrase": passphrase,
            "upload_s3": upload_s3
        });
        let res = self
            .request(
                Method::POST,
                "/api/disaster-recovery/backups",
                Some(&payload),
            )
            .await?;
        println!("Disaster recovery snapshot created successfully:");
        print_json(&res)
    }

    pub async fn restore_dr_backup(
        &self,
        backup_id: &str,
        passphrase: Option<&str>,
    ) -> anyhow::Result<()> {
        let payload = json!({
            "passphrase": passphrase
        });
        self.request(
            Method::POST,
            &format!("/api/disaster-recovery/backups/{backup_id}/restore"),
            Some(&payload),
        )
        .await?;
        println!(
            "Disaster recovery snapshot {backup_id} restored successfully. Restart r3v3rs3 to apply changes."
        );
        Ok(())
    }

    pub async fn delete_dr_backup(&self, backup_id: &str) -> anyhow::Result<()> {
        self.request(
            Method::DELETE,
            &format!("/api/disaster-recovery/backups/{backup_id}"),
            None,
        )
        .await?;
        println!("Disaster recovery snapshot {backup_id} deleted.");
        Ok(())
    }

    pub async fn download_dr_backup(
        &self,
        backup_id: &str,
        output: Option<&str>,
    ) -> anyhow::Result<()> {
        let target_path = output
            .map(|s| s.to_string())
            .unwrap_or_else(|| format!("{backup_id}.r3bundle"));
        let url = format!(
            "{}/api/disaster-recovery/backups/{backup_id}/download",
            self.server
        );
        let resp = self
            .client
            .get(&url)
            .header("Authorization", format!("Bearer {}", self.token))
            .send()
            .await
            .context("failed to request disaster recovery bundle download")?;
        if !resp.status().is_success() {
            let status = resp.status();
            let text = resp.text().await.unwrap_or_default();
            bail!("download failed ({status}): {text}");
        }
        let bytes = resp
            .bytes()
            .await
            .context("failed to read response bytes")?;
        std::fs::write(&target_path, &bytes)
            .with_context(|| format!("failed to write bundle to {target_path}"))?;
        println!(
            "Disaster recovery bundle saved to {target_path} ({} bytes)",
            bytes.len()
        );
        Ok(())
    }

    pub async fn restore_dr_file(
        &self,
        file: &str,
        passphrase: Option<&str>,
    ) -> anyhow::Result<()> {
        let bytes =
            std::fs::read(file).with_context(|| format!("failed to read bundle file: {file}"))?;
        let url = format!("{}/api/disaster-recovery/restore-upload", self.server);
        let mut req = self
            .client
            .post(&url)
            .header("Authorization", format!("Bearer {}", self.token))
            .header("content-type", "application/octet-stream");
        if let Some(pass) = passphrase {
            req = req.header("x-passphrase", pass);
        }
        let resp = req
            .body(bytes)
            .send()
            .await
            .context("failed to upload disaster recovery bundle")?;
        if !resp.status().is_success() {
            let status = resp.status();
            let text = resp.text().await.unwrap_or_default();
            bail!("restore-upload failed ({status}): {text}");
        }
        println!(
            "Disaster recovery bundle restored successfully from {file}. Restart r3v3rs3 to apply changes."
        );
        Ok(())
    }

    // --- Database operations ---

    pub async fn list_dbs(&self) -> anyhow::Result<()> {
        let res = self.request(Method::GET, "/api/databases", None).await?;
        if let Some(arr) = res.as_array() {
            println!("{:<26} {:<20} {:<12} STATUS", "ID", "NAME", "ENGINE");
            println!("{:-<26} {:-<20} {:-<12} {:-<10}", "", "", "", "");
            for item in arr {
                let id = item.get("id").and_then(|v| v.as_str()).unwrap_or("-");
                let name = item.get("name").and_then(|v| v.as_str()).unwrap_or("-");
                let engine = item.get("engine").and_then(|v| v.as_str()).unwrap_or("-");
                let status = item.get("status").and_then(|v| v.as_str()).unwrap_or("-");
                println!("{:<26} {:<20} {:<12} {}", id, name, engine, status);
            }
        } else {
            print_json(&res)?;
        }
        Ok(())
    }

    pub async fn get_db(&self, id: &str) -> anyhow::Result<()> {
        let res = self
            .request(Method::GET, &format!("/api/databases/{id}"), None)
            .await?;
        print_json(&res)
    }

    pub async fn create_db(
        &self,
        name: &str,
        engine: &str,
        version: Option<&str>,
        password: Option<&str>,
    ) -> anyhow::Result<()> {
        let default_ver = match engine.to_ascii_lowercase().as_str() {
            "postgres" | "postgresql" => "17",
            "mysql" => "8.4",
            "mariadb" => "11.4",
            "redis" => "7.4",
            "mongo" | "mongodb" => "7.0",
            _ => "latest",
        };
        let body = serde_json::json!({
            "name": name,
            "engine": engine,
            "version": version.unwrap_or(default_ver),
            "password": password,
        });
        let res = self
            .request(Method::POST, "/api/databases", Some(&body))
            .await?;
        println!("database created");
        print_json(&res)
    }

    pub async fn delete_db(&self, id: &str) -> anyhow::Result<()> {
        self.request(Method::DELETE, &format!("/api/databases/{id}"), None)
            .await?;
        println!("database/{id} deleted");
        Ok(())
    }

    pub async fn clone_db(&self, id: &str, name: &str) -> anyhow::Result<()> {
        let body = serde_json::json!({ "name": name });
        let res = self
            .request(
                Method::POST,
                &format!("/api/databases/{id}/clone"),
                Some(&body),
            )
            .await?;
        println!("database/{id} clone triggered");
        print_json(&res)
    }

    pub async fn replica_db(&self, id: &str, name: &str) -> anyhow::Result<()> {
        let body = serde_json::json!({ "name": name });
        let res = self
            .request(
                Method::POST,
                &format!("/api/databases/{id}/replica"),
                Some(&body),
            )
            .await?;
        println!("database/{id} replica created");
        print_json(&res)
    }

    pub async fn db_logs(&self, id: &str, tail: usize) -> anyhow::Result<()> {
        let res = self
            .request(
                Method::GET,
                &format!("/api/databases/{id}/logs?tail={tail}"),
                None,
            )
            .await?;
        print_json(&res)
    }

    // --- Target operations ---

    pub async fn list_targets(&self) -> anyhow::Result<()> {
        let res = self.request(Method::GET, "/api/targets", None).await?;
        if let Some(arr) = res.as_array() {
            println!("{:<26} {:<20} {:<24} STATUS", "ID", "NAME", "ADDRESS");
            println!("{:-<26} {:-<20} {:-<24} {:-<10}", "", "", "", "");
            for item in arr {
                let id = item.get("id").and_then(|v| v.as_str()).unwrap_or("-");
                let name = item.get("name").and_then(|v| v.as_str()).unwrap_or("-");
                let addr = item.get("address").and_then(|v| v.as_str()).unwrap_or("-");
                let status = item.get("status").and_then(|v| v.as_str()).unwrap_or("-");
                println!("{:<26} {:<20} {:<24} {}", id, name, addr, status);
            }
        } else {
            print_json(&res)?;
        }
        Ok(())
    }

    pub async fn get_target(&self, id: &str) -> anyhow::Result<()> {
        let res = self
            .request(Method::GET, &format!("/api/targets/{id}"), None)
            .await?;
        print_json(&res)
    }

    pub async fn add_target(&self, name: &str, host: &str, port: u16) -> anyhow::Result<()> {
        let body = serde_json::json!({
            "name": name,
            "host": host,
            "port": port,
        });
        let res = self
            .request(Method::POST, "/api/targets", Some(&body))
            .await?;
        println!("target added");
        print_json(&res)
    }

    pub async fn delete_target(&self, id: &str) -> anyhow::Result<()> {
        self.request(Method::DELETE, &format!("/api/targets/{id}"), None)
            .await?;
        println!("target/{id} deleted");
        Ok(())
    }

    pub async fn drain_target(&self, id: &str) -> anyhow::Result<()> {
        self.request(Method::POST, &format!("/api/targets/{id}/drain"), None)
            .await?;
        println!("target/{id} drained for maintenance");
        Ok(())
    }

    pub async fn undrain_target(&self, id: &str) -> anyhow::Result<()> {
        self.request(Method::POST, &format!("/api/targets/{id}/undrain"), None)
            .await?;
        println!("target/{id} restored to traffic pool");
        Ok(())
    }

    pub async fn set_target_region(&self, id: &str, region: Option<&str>) -> anyhow::Result<()> {
        let payload = serde_json::json!({ "region": region });
        let updated: serde_json::Value = self
            .request(
                Method::PUT,
                &format!("/api/targets/{id}/region"),
                Some(&payload),
            )
            .await?;
        println!(
            "target/{id} region set to {}",
            region.unwrap_or("none (global fallback)")
        );
        print_json(&updated)
    }

    pub async fn new_target_token(&self) -> anyhow::Result<()> {
        let res = self
            .request(Method::POST, "/api/targets/token", None)
            .await?;
        print_json(&res)
    }

    // --- Project operations ---

    pub async fn list_projects(&self) -> anyhow::Result<()> {
        let res = self.request(Method::GET, "/api/projects", None).await?;
        if let Some(arr) = res.as_array() {
            println!("{:<26} {:<30} ENVIRONMENTS", "ID", "NAME");
            println!("{:-<26} {:-<30} {:-<20}", "", "", "");
            for item in arr {
                let id = item.get("id").and_then(|v| v.as_str()).unwrap_or("-");
                let name = item.get("name").and_then(|v| v.as_str()).unwrap_or("-");
                let envs = item
                    .get("environments")
                    .and_then(|e| e.as_array())
                    .map(|arr| {
                        arr.iter()
                            .filter_map(|x| x.get("name").and_then(|n| n.as_str()))
                            .collect::<Vec<_>>()
                            .join(", ")
                    })
                    .unwrap_or_default();
                println!("{:<26} {:<30} {}", id, name, envs);
            }
        } else {
            print_json(&res)?;
        }
        Ok(())
    }

    pub async fn get_project(&self, id: &str) -> anyhow::Result<()> {
        let res = self
            .request(Method::GET, &format!("/api/projects/{id}"), None)
            .await?;
        print_json(&res)
    }

    pub async fn create_project(
        &self,
        name: &str,
        description: Option<&str>,
    ) -> anyhow::Result<()> {
        let body = serde_json::json!({
            "name": name,
            "description": description.unwrap_or_default(),
        });
        let res = self
            .request(Method::POST, "/api/projects", Some(&body))
            .await?;
        println!("project created");
        print_json(&res)
    }

    pub async fn delete_project(&self, id: &str) -> anyhow::Result<()> {
        self.request(Method::DELETE, &format!("/api/projects/{id}"), None)
            .await?;
        println!("project/{id} deleted");
        Ok(())
    }

    pub async fn sync_project_env(
        &self,
        project_id: &str,
        from: &str,
        to: &str,
    ) -> anyhow::Result<()> {
        let body = serde_json::json!({
            "source_env": from,
            "target_env": to,
        });
        let res = self
            .request(
                Method::POST,
                &format!("/api/projects/{project_id}/environments/sync"),
                Some(&body),
            )
            .await?;
        println!("project/{project_id} environment variables synchronized from {from} to {to}");
        print_json(&res)
    }

    // --- App Store operations ---

    pub async fn list_store(&self) -> anyhow::Result<()> {
        let res = self.request(Method::GET, "/api/store", None).await?;
        if let Some(arr) = res.as_array() {
            println!("{:<24} {:<24} {:<16} TYPE", "ID", "NAME", "CATEGORY");
            println!("{:-<24} {:-<24} {:-<16} {:-<10}", "", "", "", "");
            for item in arr {
                let id = item.get("id").and_then(|v| v.as_str()).unwrap_or("-");
                let name = item.get("name").and_then(|v| v.as_str()).unwrap_or("-");
                let cat = item.get("category").and_then(|v| v.as_str()).unwrap_or("-");
                let typ = if item.get("compose").is_some() {
                    "compose"
                } else {
                    "image"
                };
                println!("{:<24} {:<24} {:<16} {}", id, name, cat, typ);
            }
        } else {
            print_json(&res)?;
        }
        Ok(())
    }

    pub async fn refresh_store(&self) -> anyhow::Result<()> {
        self.request(Method::POST, "/api/store/refresh", None)
            .await?;
        println!("app store catalog refreshed");
        Ok(())
    }

    pub async fn install_store(
        &self,
        app_id: &str,
        name: &str,
        domain: Option<&str>,
    ) -> anyhow::Result<()> {
        let mut domains = Vec::new();
        if let Some(d) = domain {
            domains.push(d.to_string());
        }
        let body = serde_json::json!({
            "name": name,
            "store_app_id": app_id,
            "domains": domains,
        });
        let res = self.request(Method::POST, "/api/apps", Some(&body)).await?;
        println!("app store application '{app_id}' installed as '{name}'");
        print_json(&res)
    }

    // --- Security operations ---

    pub async fn list_banned_ips(&self) -> anyhow::Result<()> {
        let res = self.request(Method::GET, "/api/ban/ips", None).await?;
        if let Some(arr) = res.as_array() {
            println!("{:<20} {:<30} BANNED AT", "IP ADDRESS", "REASON");
            println!("{:-<20} {:-<30} {:-<20}", "", "", "");
            for item in arr {
                let ip = item.get("ip").and_then(|v| v.as_str()).unwrap_or("-");
                let reason = item
                    .get("reason")
                    .and_then(|v| v.as_str())
                    .unwrap_or("brute force");
                let time = item.get("banned_at").and_then(|v| v.as_u64()).unwrap_or(0);
                println!("{:<20} {:<30} {}", ip, reason, time);
            }
        } else {
            print_json(&res)?;
        }
        Ok(())
    }

    pub async fn unban_ip(&self, ip: &str) -> anyhow::Result<()> {
        self.request(Method::DELETE, &format!("/api/ban/ips/{ip}"), None)
            .await?;
        println!("ip/{ip} unbanned");
        Ok(())
    }

    pub async fn list_quarantined_ips(&self) -> anyhow::Result<()> {
        let res = self
            .request(Method::GET, "/api/security/quarantine", None)
            .await?;
        if let Some(arr) = res.as_array() {
            println!(
                "{:<18} {:<32} {:<12} {:<10}",
                "IP ADDRESS", "REASON", "VIOLATIONS", "REMAINING"
            );
            println!("{:-<18} {:-<32} {:-<12} {:-<10}", "", "", "", "");
            for item in arr {
                let ip = item.get("ip").and_then(|v| v.as_str()).unwrap_or("-");
                let reason = item.get("reason").and_then(|v| v.as_str()).unwrap_or("-");
                let violations = item.get("violations").and_then(|v| v.as_u64()).unwrap_or(0);
                let remaining = item
                    .get("remaining_seconds")
                    .and_then(|v| v.as_u64())
                    .unwrap_or(0);
                println!(
                    "{:<18} {:<32} {:<12} {}s",
                    ip, reason, violations, remaining
                );
            }
        } else {
            print_json(&res)?;
        }
        Ok(())
    }

    pub async fn quarantine_ip(
        &self,
        ip: &str,
        reason: Option<&str>,
        duration_seconds: Option<u64>,
    ) -> anyhow::Result<()> {
        let payload = serde_json::json!({
            "ip": ip,
            "reason": reason,
            "duration_seconds": duration_seconds,
        });
        let res = self
            .request(Method::POST, "/api/security/quarantine", Some(&payload))
            .await?;
        println!("ip/{ip} quarantined successfully");
        print_json(&res)?;
        Ok(())
    }

    pub async fn unquarantine_ip(&self, ip: &str) -> anyhow::Result<()> {
        self.request(
            Method::DELETE,
            &format!("/api/security/quarantine/{ip}"),
            None,
        )
        .await?;
        println!("ip/{ip} released from quarantine");
        Ok(())
    }

    pub async fn get_quarantine_config(&self) -> anyhow::Result<()> {
        let res = self
            .request(Method::GET, "/api/security/quarantine/config", None)
            .await?;
        print_json(&res)?;
        Ok(())
    }

    pub async fn set_quarantine_config(
        &self,
        enabled: Option<bool>,
        max_violations: Option<u32>,
        window_seconds: Option<u64>,
        ban_duration_seconds: Option<u64>,
    ) -> anyhow::Result<()> {
        let current = self
            .request(Method::GET, "/api/security/quarantine/config", None)
            .await?;
        let mut config = current;
        if let Some(obj) = config.as_object_mut() {
            if let Some(enabled) = enabled {
                obj.insert("enabled".to_string(), serde_json::json!(enabled));
            }
            if let Some(max_violations) = max_violations {
                obj.insert(
                    "max_violations".to_string(),
                    serde_json::json!(max_violations),
                );
            }
            if let Some(window_seconds) = window_seconds {
                obj.insert(
                    "window_seconds".to_string(),
                    serde_json::json!(window_seconds),
                );
            }
            if let Some(ban_duration_seconds) = ban_duration_seconds {
                obj.insert(
                    "ban_duration_seconds".to_string(),
                    serde_json::json!(ban_duration_seconds),
                );
            }
        }
        self.request(
            Method::PUT,
            "/api/security/quarantine/config",
            Some(&config),
        )
        .await?;
        println!("auto-quarantine configuration updated");
        Ok(())
    }

    pub async fn list_active_connections(&self, proxy_id: Option<&str>) -> anyhow::Result<()> {
        let path = match proxy_id {
            Some(id) => format!("/api/traffic/connections?proxy_id={id}"),
            None => "/api/traffic/connections".to_string(),
        };
        let res = self.request(Method::GET, &path, None).await?;
        let conns = res.as_array().context("expected connection array")?;
        if conns.is_empty() {
            println!("No active long-lived connections found.");
            return Ok(());
        }

        println!(
            "{:<10} {:<12} {:<20} {:<24} {:<12} {:<14} {:<14} SPEED",
            "ID", "PROTO", "PROXY", "CLIENT", "UPTIME", "SENT", "RECV"
        );
        println!(
            "{:-<10} {:-<12} {:-<20} {:-<24} {:-<12} {:-<14} {:-<14} {:-<12}",
            "", "", "", "", "", "", "", ""
        );

        for c in conns {
            let id = c["id"].as_str().unwrap_or("-");
            let proto = c["protocol"].as_str().unwrap_or("-");
            let proxy = c["proxy_name"].as_str().unwrap_or("-");
            let client = c["client_addr"].as_str().unwrap_or("-");
            let uptime_secs = c["uptime_secs"].as_u64().unwrap_or(0);
            let uptime = format_duration_secs(uptime_secs);
            let sent = format_cli_bytes(c["bytes_sent"].as_u64().unwrap_or(0));
            let recv = format_cli_bytes(c["bytes_received"].as_u64().unwrap_or(0));
            let speed = format!(
                "{}/s",
                format_cli_bytes(c["current_rate_bps"].as_u64().unwrap_or(0))
            );

            println!(
                "{:<10} {:<12} {:<20} {:<24} {:<12} {:<14} {:<14} {}",
                id, proto, proxy, client, uptime, sent, recv, speed
            );
        }
        Ok(())
    }

    pub async fn terminate_active_connection(&self, id: &str) -> anyhow::Result<()> {
        let path = format!("/api/traffic/connections/{id}");
        self.request(Method::DELETE, &path, None).await?;
        println!("connection/{id} terminated successfully");
        Ok(())
    }

    pub async fn get_geoip_status(&self) -> anyhow::Result<()> {
        let res = self
            .request(Method::GET, "/api/security/geoip/status", None)
            .await?;
        if let Some(obj) = res.as_object() {
            let loaded = obj.get("loaded").and_then(|v| v.as_bool()).unwrap_or(false);
            let path = obj
                .get("database_path")
                .and_then(|v| v.as_str())
                .unwrap_or("-");
            let size = obj
                .get("file_size_bytes")
                .and_then(|v| v.as_u64())
                .map(|b| format!("{:.2} MB", b as f64 / (1024.0 * 1024.0)))
                .unwrap_or_else(|| "-".to_string());
            let build = obj
                .get("build_epoch")
                .and_then(|v| v.as_i64())
                .map(|e| e.to_string())
                .unwrap_or_else(|| "-".to_string());
            let auto_update = obj
                .get("auto_update")
                .and_then(|v| v.as_bool())
                .unwrap_or(false);
            let interval = obj
                .get("update_interval_days")
                .and_then(|v| v.as_u64())
                .unwrap_or(7);
            let license_set = obj
                .get("license_key_configured")
                .and_then(|v| v.as_bool())
                .unwrap_or(false);
            let account_id = obj
                .get("account_id")
                .and_then(|v| v.as_str())
                .unwrap_or("-");

            println!(
                "{:<22} {:<10}",
                "GeoIP Status:",
                if loaded { "Loaded" } else { "Not Loaded" }
            );
            println!("{:<22} {}", "Database Path:", path);
            println!("{:<22} {}", "Database Size:", size);
            println!("{:<22} {}", "Build Epoch:", build);
            println!(
                "{:<22} {}",
                "Auto Update:",
                if auto_update { "Enabled" } else { "Disabled" }
            );
            println!("{:<22} {} days", "Update Interval:", interval);
            println!(
                "{:<22} {}",
                "License Key:",
                if license_set {
                    "Configured"
                } else {
                    "Not Configured"
                }
            );
            println!("{:<22} {}", "Account ID:", account_id);
        } else {
            print_json(&res)?;
        }
        Ok(())
    }

    pub async fn update_geoip_database(&self) -> anyhow::Result<()> {
        println!("Downloading and updating GeoIP database from MaxMind...");
        let res = self
            .request(Method::POST, "/api/security/geoip/update", None)
            .await?;
        println!("GeoIP database downloaded and hot-reloaded successfully.");
        print_json(&res)?;
        Ok(())
    }

    pub async fn get_geoip_config(&self) -> anyhow::Result<()> {
        let res = self
            .request(Method::GET, "/api/security/geoip/status", None)
            .await?;
        print_json(&res)?;
        Ok(())
    }

    pub async fn set_geoip_config(
        &self,
        account_id: Option<String>,
        license_key: Option<String>,
        database: Option<String>,
        auto_update: Option<bool>,
        interval_days: Option<u32>,
    ) -> anyhow::Result<()> {
        let current = self
            .request(Method::GET, "/api/security/geoip/status", None)
            .await?;
        let curr_auto = current
            .get("auto_update")
            .and_then(|v| v.as_bool())
            .unwrap_or(false);
        let curr_interval = current
            .get("update_interval_days")
            .and_then(|v| v.as_u64())
            .unwrap_or(7) as u32;

        let payload = serde_json::json!({
            "account_id": account_id.or_else(|| current.get("account_id").and_then(|v| v.as_str()).map(String::from)),
            "license_key": license_key,
            "database": database.or_else(|| current.get("database_path").and_then(|v| v.as_str()).map(String::from)),
            "auto_update": auto_update.unwrap_or(curr_auto),
            "update_interval_days": interval_days.unwrap_or(curr_interval),
        });

        let res = self
            .request(Method::PUT, "/api/security/geoip/config", Some(&payload))
            .await?;
        println!("GeoIP configuration updated successfully.");
        print_json(&res)?;
        Ok(())
    }

    // --- Webhook operations ---

    pub async fn list_webhooks(&self) -> anyhow::Result<()> {
        let res = self.request(Method::GET, "/api/webhooks", None).await?;
        if let Some(arr) = res.as_array() {
            println!("{:<26} {:<40} ACTIVE", "ID", "URL");
            println!("{:-<26} {:-<40} {:-<10}", "", "", "");
            for item in arr {
                let id = item.get("id").and_then(|v| v.as_str()).unwrap_or("-");
                let url = item.get("url").and_then(|v| v.as_str()).unwrap_or("-");
                let active = item.get("active").and_then(|v| v.as_bool()).unwrap_or(true);
                println!("{:<26} {:<40} {}", id, url, active);
            }
        } else {
            print_json(&res)?;
        }
        Ok(())
    }

    pub async fn get_webhook(&self, id: &str) -> anyhow::Result<()> {
        let res = self
            .request(Method::GET, &format!("/api/webhooks/{id}"), None)
            .await?;
        print_json(&res)
    }

    pub async fn add_webhook(&self, payload: &str) -> anyhow::Result<()> {
        let val = load_payload(payload)?;
        let res = self
            .request(Method::POST, "/api/webhooks", Some(&val))
            .await?;
        println!("webhook added");
        print_json(&res)
    }

    pub async fn update_webhook(&self, id: &str, payload: &str) -> anyhow::Result<()> {
        let val = load_payload(payload)?;
        let res = self
            .request(Method::PUT, &format!("/api/webhooks/{id}"), Some(&val))
            .await?;
        println!("webhook/{id} updated");
        print_json(&res)
    }

    pub async fn delete_webhook(&self, id: &str) -> anyhow::Result<()> {
        self.request(Method::DELETE, &format!("/api/webhooks/{id}"), None)
            .await?;
        println!("webhook/{id} deleted");
        Ok(())
    }

    pub async fn test_webhook(&self, id: &str) -> anyhow::Result<()> {
        let res = self
            .request(Method::POST, &format!("/api/webhooks/{id}/test"), None)
            .await?;
        println!("webhook/{id} test ping sent");
        print_json(&res)
    }

    // --- Container Registry operations ---

    pub async fn list_registries(&self) -> anyhow::Result<()> {
        let res = self.request(Method::GET, "/api/registries", None).await?;
        if let Some(arr) = res.as_array() {
            println!("{:<26} {:<32} USERNAME", "ID", "SERVER");
            println!("{:-<26} {:-<32} {:-<20}", "", "", "");
            for item in arr {
                let id = item.get("id").and_then(|v| v.as_str()).unwrap_or("-");
                let server = item.get("server").and_then(|v| v.as_str()).unwrap_or("-");
                let username = item.get("username").and_then(|v| v.as_str()).unwrap_or("-");
                println!("{:<26} {:<32} {}", id, server, username);
            }
        } else {
            print_json(&res)?;
        }
        Ok(())
    }

    pub async fn add_registry(
        &self,
        server: Option<String>,
        username: Option<String>,
        password: Option<String>,
        payload: Option<String>,
    ) -> anyhow::Result<()> {
        let val = build_registry_payload(server, username, password, payload)?;
        let res = self
            .request(Method::POST, "/api/registries", Some(&val))
            .await?;
        println!("registry added");
        print_json(&res)
    }

    pub async fn update_registry(
        &self,
        id: &str,
        server: Option<String>,
        username: Option<String>,
        password: Option<String>,
        payload: Option<String>,
    ) -> anyhow::Result<()> {
        let val = build_registry_payload(server, username, password, payload)?;
        let res = self
            .request(Method::PUT, &format!("/api/registries/{id}"), Some(&val))
            .await?;
        println!("registry/{id} updated");
        print_json(&res)
    }

    pub async fn delete_registry(&self, id: &str) -> anyhow::Result<()> {
        self.request(Method::DELETE, &format!("/api/registries/{id}"), None)
            .await?;
        println!("registry/{id} deleted");
        Ok(())
    }

    // --- Git Provider Connection operations ---

    pub async fn list_git_connections(&self) -> anyhow::Result<()> {
        let res = self
            .request(Method::GET, "/api/git/connections", None)
            .await?;
        if let Some(arr) = res.as_array() {
            println!(
                "{:<26} {:<20} {:<12} {:<14} ACCOUNT",
                "ID", "NAME", "PROVIDER", "STATUS"
            );
            println!(
                "{:-<26} {:-<20} {:-<12} {:-<14} {:-<20}",
                "", "", "", "", ""
            );
            for item in arr {
                let id = item.get("id").and_then(|v| v.as_str()).unwrap_or("-");
                let name = item.get("name").and_then(|v| v.as_str()).unwrap_or("-");
                let provider = item.get("provider").and_then(|v| v.as_str()).unwrap_or("-");
                let status = item.get("status").and_then(|v| v.as_str()).unwrap_or("-");
                let account = item.get("account").and_then(|v| v.as_str()).unwrap_or("-");
                println!(
                    "{:<26} {:<20} {:<12} {:<14} {}",
                    id, name, provider, status, account
                );
            }
        } else {
            print_json(&res)?;
        }
        Ok(())
    }

    pub async fn get_git_connection(&self, id: &str) -> anyhow::Result<()> {
        let res = self
            .request(Method::GET, &format!("/api/git/connections/{id}"), None)
            .await?;
        print_json(&res)
    }

    pub async fn add_git_connection(
        &self,
        name: Option<String>,
        provider: Option<String>,
        url: Option<String>,
        client_id: Option<String>,
        client_secret: Option<String>,
        payload: Option<String>,
    ) -> anyhow::Result<()> {
        let val =
            build_git_connection_payload(name, provider, url, client_id, client_secret, payload)?;
        let res = self
            .request(Method::POST, "/api/git/connections", Some(&val))
            .await?;
        println!("git connection added");
        print_json(&res)
    }

    pub async fn delete_git_connection(&self, id: &str) -> anyhow::Result<()> {
        self.request(Method::DELETE, &format!("/api/git/connections/{id}"), None)
            .await?;
        println!("git connection/{id} deleted");
        Ok(())
    }

    pub async fn list_git_repos(
        &self,
        id: &str,
        search: Option<&str>,
        page: Option<u32>,
    ) -> anyhow::Result<()> {
        let mut path = format!("/api/git/connections/{id}/repositories?");
        if let Some(s) = search {
            path.push_str(&format!("search={s}&"));
        }
        if let Some(p) = page {
            path.push_str(&format!("page={p}&"));
        }
        let res = self.request(Method::GET, &path, None).await?;
        if let Some(arr) = res.as_array() {
            println!("{:<40} {:<10} CLONE_URL", "REPOSITORY", "PRIVATE");
            println!("{:-<40} {:-<10} {:-<40}", "", "", "");
            for item in arr {
                let full_name = item
                    .get("full_name")
                    .and_then(|v| v.as_str())
                    .unwrap_or("-");
                let private = item
                    .get("private")
                    .and_then(|v| v.as_bool())
                    .unwrap_or(false);
                let clone_url = item
                    .get("clone_url")
                    .and_then(|v| v.as_str())
                    .unwrap_or("-");
                println!("{:<40} {:<10} {}", full_name, private, clone_url);
            }
        } else {
            print_json(&res)?;
        }
        Ok(())
    }

    pub async fn list_git_branches(&self, id: &str, repo: &str) -> anyhow::Result<()> {
        let path = format!("/api/git/connections/{id}/branches?repository={repo}");
        let res = self.request(Method::GET, &path, None).await?;
        if let Some(arr) = res.as_array() {
            println!("BRANCH");
            println!("{:-<30}", "");
            for item in arr {
                let b = item.as_str().unwrap_or("-");
                println!("{}", b);
            }
        } else {
            print_json(&res)?;
        }
        Ok(())
    }

    // --- Account operations ---

    pub async fn list_accounts(&self) -> anyhow::Result<()> {
        let res = self.request(Method::GET, "/api/accounts", None).await?;
        if let Some(arr) = res.as_array() {
            println!("{:<24} {:<12} PROXIES", "USERNAME", "ROLE");
            println!("{:-<24} {:-<12} {:-<20}", "", "", "");
            for item in arr {
                let username = item.get("username").and_then(|v| v.as_str()).unwrap_or("-");
                let role = item.get("role").and_then(|v| v.as_str()).unwrap_or("-");
                let proxies = item
                    .get("proxies")
                    .and_then(|v| v.as_array())
                    .map(|arr| format!("{} proxies", arr.len()))
                    .unwrap_or_else(|| "all".to_string());
                println!("{:<24} {:<12} {}", username, role, proxies);
            }
        } else {
            print_json(&res)?;
        }
        Ok(())
    }

    pub async fn create_account(
        &self,
        username: Option<String>,
        password: Option<String>,
        role: &str,
        payload: Option<String>,
    ) -> anyhow::Result<()> {
        let val = build_account_create_payload(username, password, role, payload)?;
        let res = self
            .request(Method::POST, "/api/accounts", Some(&val))
            .await?;
        println!("account created");
        print_json(&res)
    }

    pub async fn update_account(
        &self,
        username: &str,
        password: Option<String>,
        role: Option<String>,
        payload: Option<String>,
    ) -> anyhow::Result<()> {
        let val = build_account_update_payload(password, role, payload)?;
        let res = self
            .request(
                Method::PUT,
                &format!("/api/accounts/{username}"),
                Some(&val),
            )
            .await?;
        println!("account/{username} updated");
        print_json(&res)
    }

    pub async fn delete_account(&self, username: &str) -> anyhow::Result<()> {
        self.request(Method::DELETE, &format!("/api/accounts/{username}"), None)
            .await?;
        println!("account/{username} deleted");
        Ok(())
    }

    // --- Token operations ---

    pub async fn list_tokens(&self) -> anyhow::Result<()> {
        let res = self.request(Method::GET, "/api/auth/tokens", None).await?;
        if let Some(arr) = res.as_array() {
            println!("{:<26} {:<24} SCOPE", "ID", "NAME");
            println!("{:-<26} {:-<24} {:-<10}", "", "", "");
            for item in arr {
                let id = item.get("id").and_then(|v| v.as_str()).unwrap_or("-");
                let name = item.get("name").and_then(|v| v.as_str()).unwrap_or("-");
                let scope = item.get("scope").and_then(|v| v.as_str()).unwrap_or("-");
                println!("{:<26} {:<24} {}", id, name, scope);
            }
        } else {
            print_json(&res)?;
        }
        Ok(())
    }

    pub async fn create_token(
        &self,
        name: Option<String>,
        scope: &str,
        payload: Option<String>,
    ) -> anyhow::Result<()> {
        let val = build_token_create_payload(name, scope, payload)?;
        let res = self
            .request(Method::POST, "/api/auth/tokens", Some(&val))
            .await?;
        println!("token created");
        print_json(&res)
    }

    pub async fn delete_token(&self, id: &str) -> anyhow::Result<()> {
        self.request(Method::DELETE, &format!("/api/auth/tokens/{id}"), None)
            .await?;
        println!("token/{id} deleted");
        Ok(())
    }

    // --- Cron operations ---

    pub async fn list_cron(&self, app_id: &str) -> anyhow::Result<()> {
        let res = self
            .request(Method::GET, &format!("/api/apps/{app_id}/commands"), None)
            .await?;
        if let Some(arr) = res.as_array() {
            println!("{:<26} {:<18} {:<16} COMMAND", "ID", "NAME", "CRON");
            println!("{:-<26} {:-<18} {:-<16} {:-<30}", "", "", "", "");
            for item in arr {
                let id = item.get("id").and_then(|v| v.as_str()).unwrap_or("-");
                let name = item.get("name").and_then(|v| v.as_str()).unwrap_or("-");
                let cron = item.get("cron").and_then(|v| v.as_str()).unwrap_or("-");
                let cmd = item.get("command").and_then(|v| v.as_str()).unwrap_or("-");
                println!("{:<26} {:<18} {:<16} {}", id, name, cron, cmd);
            }
        } else {
            print_json(&res)?;
        }
        Ok(())
    }

    pub async fn add_cron(
        &self,
        app_id: &str,
        cron: &str,
        command: &str,
        name: Option<&str>,
    ) -> anyhow::Result<()> {
        let body = serde_json::json!({
            "schedule": cron,
            "cron": cron,
            "command": command,
            "name": name.unwrap_or(command),
            "enabled": true,
        });
        let res = self
            .request(
                Method::POST,
                &format!("/api/apps/{app_id}/commands"),
                Some(&body),
            )
            .await?;
        println!("scheduled command added to app/{app_id}");
        print_json(&res)
    }

    pub async fn update_cron(
        &self,
        app_id: &str,
        id: &str,
        cron: Option<&str>,
        command: Option<&str>,
        name: Option<&str>,
        payload: Option<&str>,
    ) -> anyhow::Result<()> {
        let existing = if payload.is_none() {
            let list = self
                .request(Method::GET, &format!("/api/apps/{app_id}/commands"), None)
                .await?;
            list.as_array().and_then(|arr| {
                arr.iter()
                    .find(|it| it.get("id").and_then(|v| v.as_str()) == Some(id))
                    .cloned()
            })
        } else {
            None
        };
        let body = build_cron_update_payload(existing.as_ref(), cron, command, name, payload)?;
        let res = self
            .request(
                Method::PUT,
                &format!("/api/apps/{app_id}/commands/{id}"),
                Some(&body),
            )
            .await?;
        println!("scheduled command/{id} updated on app/{app_id}");
        print_json(&res)
    }

    pub async fn delete_cron(&self, app_id: &str, id: &str) -> anyhow::Result<()> {
        self.request(
            Method::DELETE,
            &format!("/api/apps/{app_id}/commands/{id}"),
            None,
        )
        .await?;
        println!("scheduled command/{id} deleted from app/{app_id}");
        Ok(())
    }

    pub async fn run_cron(&self, app_id: &str, id: &str) -> anyhow::Result<()> {
        let res = self
            .request(
                Method::POST,
                &format!("/api/apps/{app_id}/commands/{id}/run"),
                None,
            )
            .await?;
        println!("scheduled command/{id} executed on app/{app_id}");
        print_json(&res)
    }

    // --- Platform & System operations ---

    pub async fn platform_status(&self) -> anyhow::Result<()> {
        let res = self.request(Method::GET, "/api/status", None).await?;
        print_json(&res)
    }

    pub async fn platform_metrics(&self) -> anyhow::Result<()> {
        let res = self.request(Method::GET, "/api/metrics", None).await?;
        print_json(&res)
    }

    pub async fn platform_prune(&self) -> anyhow::Result<()> {
        let res = self
            .request(Method::POST, "/api/platform/prune/run", None)
            .await?;
        println!("platform disk hygiene prune completed");
        print_json(&res)
    }

    pub async fn get_prune_policy(&self) -> anyhow::Result<()> {
        let res = self
            .request(Method::GET, "/api/platform/prune", None)
            .await?;
        print_json(&res)
    }

    pub async fn set_prune_policy(&self, args: &SetPrunePolicyArgs) -> anyhow::Result<()> {
        let mut cur = self
            .request(Method::GET, "/api/platform/prune", None)
            .await
            .unwrap_or(serde_json::json!({}));
        apply_prune_args(&mut cur, args);
        let res = self
            .request(Method::PUT, "/api/platform/prune", Some(&cur))
            .await?;
        println!("Platform prune policy updated successfully.");
        print_json(&res)
    }

    pub async fn vacuum_platform_logs(&self, threshold_mb: Option<u64>) -> anyhow::Result<()> {
        let url = match threshold_mb {
            Some(mb) => format!("/api/platform/logs/vacuum?threshold_mb={mb}"),
            None => "/api/platform/logs/vacuum".to_string(),
        };
        let res = self.request(Method::POST, &url, None).await?;
        println!("platform container logs vacuum completed");
        print_json(&res)
    }

    // --- Platform operations ---

    pub async fn rotate_platform_key(&self) -> anyhow::Result<()> {
        let res = self
            .request(Method::POST, "/api/platform/key/rotate", None)
            .await?;
        println!("Platform encryption key rotated successfully.");
        print_json(&res)
    }

    pub async fn get_node_alerts(&self) -> anyhow::Result<()> {
        let res = self
            .request(Method::GET, "/api/platform/node_alerts", None)
            .await?;
        print_json(&res)
    }

    pub async fn set_node_alerts(
        &self,
        enabled: bool,
        min_disk_percent: Option<u8>,
        bandwidth_spike: Option<u64>,
        max_fd_percent: Option<u8>,
        max_time_wait: Option<u32>,
    ) -> anyhow::Result<()> {
        let body = serde_json::json!({
            "enabled": enabled,
            "min_free_disk_percent": min_disk_percent,
            "bandwidth_spike_bytes_per_sec": bandwidth_spike,
            "max_fd_ratio_percent": max_fd_percent,
            "max_time_wait_sockets": max_time_wait,
        });
        let res = self
            .request(Method::PUT, "/api/platform/node_alerts", Some(&body))
            .await?;
        println!("Node threshold alert rules updated successfully.");
        print_json(&res)
    }

    // --- Apply operation ---

    // --- Apply operation ---

    pub async fn apply_manifest(&self, content: &str) -> anyhow::Result<()> {
        let val = parse_manifest(content)?;
        let items = extract_manifest_items(val);

        self.apply_ports(&items.ports).await?;
        self.apply_proxies(&items.proxies).await?;
        self.apply_apps(&items.apps).await?;

        Ok(())
    }

    async fn apply_ports(&self, ports: &[Value]) -> anyhow::Result<()> {
        if ports.is_empty() {
            return Ok(());
        }
        let existing = self.request(Method::GET, "/api/ports", None).await?;
        let existing_list = existing.as_array().cloned().unwrap_or_default();
        for item in ports {
            self.apply_single_port(item, &existing_list).await?;
        }
        Ok(())
    }

    async fn apply_single_port(&self, item: &Value, existing: &[Value]) -> anyhow::Result<()> {
        let target_id = item.get("id").and_then(|v| v.as_str());
        let target_listen = item
            .get("listen")
            .or_else(|| item.get("port").and_then(|p| p.get("listen")))
            .and_then(|v| v.as_str());

        let matched = existing.iter().find(|e| {
            let e_id = e.get("id").and_then(|v| v.as_str());
            let e_listen = e
                .get("port")
                .and_then(|p| p.get("listen"))
                .and_then(|v| v.as_str());
            (target_id.is_some() && e_id == target_id)
                || (target_listen.is_some() && e_listen == target_listen)
        });

        if let Some(matched_entry) = matched {
            let id = matched_entry
                .get("id")
                .and_then(|v| v.as_str())
                .unwrap_or_default();
            self.request(Method::PUT, &format!("/api/ports/{id}"), Some(item))
                .await?;
            println!("port/{id} configured");
        } else {
            self.request(Method::POST, "/api/ports", Some(item)).await?;
            let name = target_id.or(target_listen).unwrap_or("unknown");
            println!("port/{name} created");
        }
        Ok(())
    }

    async fn apply_proxies(&self, proxies: &[Value]) -> anyhow::Result<()> {
        if proxies.is_empty() {
            return Ok(());
        }
        let existing = self.request(Method::GET, "/api/proxies", None).await?;
        let existing_list = existing.as_array().cloned().unwrap_or_default();
        for item in proxies {
            self.apply_single_proxy(item, &existing_list).await?;
        }
        Ok(())
    }

    async fn apply_single_proxy(&self, item: &Value, existing: &[Value]) -> anyhow::Result<()> {
        let target_id = item.get("id").and_then(|v| v.as_str());
        let target_name = item.get("name").and_then(|v| v.as_str());

        let matched = existing.iter().find(|e| {
            let e_id = e.get("id").and_then(|v| v.as_str());
            let e_name = e.get("name").and_then(|v| v.as_str());
            (target_id.is_some() && e_id == target_id)
                || (target_name.is_some() && e_name == target_name)
        });

        if let Some(matched_entry) = matched {
            let id = matched_entry
                .get("id")
                .and_then(|v| v.as_str())
                .unwrap_or_default();
            self.request(Method::PUT, &format!("/api/proxies/{id}"), Some(item))
                .await?;
            println!("proxy/{id} configured");
        } else {
            self.request(Method::POST, "/api/proxies", Some(item))
                .await?;
            let name = target_name.or(target_id).unwrap_or("unknown");
            println!("proxy/{name} created");
        }
        Ok(())
    }

    async fn apply_apps(&self, apps: &[Value]) -> anyhow::Result<()> {
        if apps.is_empty() {
            return Ok(());
        }
        let existing = self.request(Method::GET, "/api/apps", None).await?;
        let existing_list = existing.as_array().cloned().unwrap_or_default();
        for item in apps {
            self.apply_single_app(item, &existing_list).await?;
        }
        Ok(())
    }

    async fn apply_single_app(&self, item: &Value, existing: &[Value]) -> anyhow::Result<()> {
        let target_id = item.get("id").and_then(|v| v.as_str());
        let target_name = item
            .get("name")
            .or_else(|| item.get("spec").and_then(|s| s.get("name")))
            .and_then(|v| v.as_str());

        let matched = existing.iter().find(|e| {
            let e_id = e.get("id").and_then(|v| v.as_str());
            let e_name = e
                .get("spec")
                .and_then(|s| s.get("name"))
                .and_then(|v| v.as_str());
            (target_id.is_some() && e_id == target_id)
                || (target_name.is_some() && e_name == target_name)
        });

        if let Some(matched_entry) = matched {
            let id = matched_entry
                .get("id")
                .and_then(|v| v.as_str())
                .unwrap_or_default();
            self.request(Method::PUT, &format!("/api/apps/{id}"), Some(item))
                .await?;
            println!("app/{id} configured");
        } else {
            self.request(Method::POST, "/api/apps", Some(item)).await?;
            let name = target_name.or(target_id).unwrap_or("unknown");
            println!("app/{name} created");
        }
        Ok(())
    }

    pub async fn get_config(&self) -> anyhow::Result<()> {
        let res = self.request(Method::GET, "/api/config", None).await?;
        print_json(&res)
    }

    pub async fn export_config_backup(&self, output: Option<&str>) -> anyhow::Result<()> {
        let res = self
            .request(Method::GET, "/api/config/backup", None)
            .await?;
        let json_text = serde_json::to_string_pretty(&res)?;
        if let Some(path) = output {
            std::fs::write(path, &json_text)
                .with_context(|| format!("failed to write backup to {path}"))?;
            println!("Configuration backup saved to {path}");
        } else {
            println!("{json_text}");
        }
        Ok(())
    }

    pub async fn restore_config_backup(&self, file: &str) -> anyhow::Result<()> {
        let content = if file == "-" {
            let mut buf = String::new();
            std::io::Read::read_to_string(&mut std::io::stdin(), &mut buf)
                .context("failed to read backup from stdin")?;
            buf
        } else {
            std::fs::read_to_string(file)
                .with_context(|| format!("failed to read backup file: {file}"))?
        };
        let body: Value = serde_json::from_str(&content).context("failed to parse backup JSON")?;
        self.request(Method::POST, "/api/config/restore", Some(&body))
            .await?;
        println!("Configuration successfully restored.");
        Ok(())
    }

    pub async fn list_security_profiles(&self) -> anyhow::Result<()> {
        let res = self
            .request(Method::GET, "/api/security/headers/profiles", None)
            .await?;
        println!("{:<16} {:<24} DESCRIPTION", "ID", "TITLE");
        println!("{}", "-".repeat(80));
        if let Some(list) = res.as_array() {
            for item in list {
                let id = item.get("id").and_then(|v| v.as_str()).unwrap_or("");
                let title = item.get("title").and_then(|v| v.as_str()).unwrap_or("");
                let desc = item
                    .get("description")
                    .and_then(|v| v.as_str())
                    .unwrap_or("");
                println!("{:<16} {:<24} {}", id, title, desc);
            }
        }
        Ok(())
    }

    pub async fn audit_proxy_security_headers(&self, id: &str) -> anyhow::Result<()> {
        let res = self
            .request(
                Method::GET,
                &format!("/api/proxies/{id}/security-headers"),
                None,
            )
            .await?;
        let score = res.get("score").and_then(|v| v.as_u64()).unwrap_or(0);
        let grade = res.get("grade").and_then(|v| v.as_str()).unwrap_or("F");
        let detected = res
            .get("detected_profile")
            .and_then(|v| v.as_str())
            .unwrap_or("custom");

        let grade_colored = match grade {
            "a_plus" | "a" => format!(
                "\x1b[32m{}\x1b[0m",
                grade.to_ascii_uppercase().replace('_', "+")
            ),
            "b" => "\x1b[33mB\x1b[0m".to_string(),
            "c" | "d" => format!("\x1b[33m{}\x1b[0m", grade.to_ascii_uppercase()),
            _ => "\x1b[31mF\x1b[0m".to_string(),
        };

        println!("Proxy Security Audit: {}", id);
        println!(
            "Score: {}/100 | Grade: {} | Matching Profile: {}",
            score, grade_colored, detected
        );
        println!("{}", "-".repeat(85));
        println!(
            "{:<28} {:<12} {:<6} RISK / CURRENT VALUE",
            "HEADER", "STATUS", "PTS"
        );
        println!("{}", "-".repeat(85));

        if let Some(items) = res.get("items").and_then(|v| v.as_array()) {
            for item in items {
                print_security_audit_item(item);
            }
        }
        Ok(())
    }

    pub async fn apply_proxy_security_profile(
        &self,
        id: &str,
        profile_raw: &str,
    ) -> anyhow::Result<()> {
        let normalized_profile = match profile_raw.to_ascii_lowercase().as_str() {
            "strict" => "strict",
            "balanced" => "balanced",
            "api-only" | "api_only" | "api" => "api_only",
            "spa" | "spa-frontend" | "spa_frontend" => "spa_frontend",
            other => anyhow::bail!(
                "unknown profile: {other}. Expected strict, balanced, api-only, or spa"
            ),
        };

        let body = serde_json::json!({ "profile": normalized_profile });
        let res = self
            .request(
                Method::POST,
                &format!("/api/proxies/{id}/security-headers/apply-profile"),
                Some(&body),
            )
            .await?;

        let score = res.get("score").and_then(|v| v.as_u64()).unwrap_or(0);
        let grade = res.get("grade").and_then(|v| v.as_str()).unwrap_or("F");
        println!(
            "\x1b[32mSuccessfully applied security profile '{}' to proxy {}.\x1b[0m",
            normalized_profile, id
        );
        println!(
            "New Score: {}/100 (Grade: {})",
            score,
            grade.to_ascii_uppercase().replace('_', "+")
        );
        Ok(())
    }

    pub async fn get_stream_priority(&self, id: &str) -> anyhow::Result<()> {
        let res = self
            .request(Method::GET, &format!("/api/proxies/{id}/priority"), None)
            .await?;
        println!("{}", serde_json::to_string_pretty(&res)?);
        Ok(())
    }

    pub async fn set_stream_priority(
        &self,
        id: &str,
        enable: bool,
        inject_header: Option<bool>,
        adaptive_concurrency: Option<bool>,
    ) -> anyhow::Result<()> {
        let mut cur = self
            .request(Method::GET, &format!("/api/proxies/{id}/priority"), None)
            .await
            .unwrap_or(serde_json::json!({}));

        cur["enabled"] = serde_json::json!(enable);
        if let Some(inject) = inject_header {
            cur["inject_header"] = serde_json::json!(inject);
        }
        if let Some(adaptive) = adaptive_concurrency {
            cur["adaptive_concurrency"] = serde_json::json!(adaptive);
        }

        let body = serde_json::json!({ "config": cur });
        self.request(
            Method::PUT,
            &format!("/api/proxies/{id}/priority"),
            Some(&body),
        )
        .await?;
        println!("proxy/{id} stream_priority enabled set to {enable}");
        Ok(())
    }

    pub async fn get_traffic_priority_stats(&self) -> anyhow::Result<()> {
        let res = self
            .request(Method::GET, "/api/traffic/priority", None)
            .await?;
        println!("{}", serde_json::to_string_pretty(&res)?);
        Ok(())
    }

    pub async fn get_ct_status(&self) -> anyhow::Result<()> {
        let res = self
            .request(Method::GET, "/api/certs/ct/status", None)
            .await?;
        println!("{}", serde_json::to_string_pretty(&res)?);
        Ok(())
    }

    pub async fn get_ct_config(&self) -> anyhow::Result<()> {
        let res = self
            .request(Method::GET, "/api/certs/ct/config", None)
            .await?;
        println!("{}", serde_json::to_string_pretty(&res)?);
        Ok(())
    }

    pub async fn set_ct_config(&self, args: SetCtConfigArgs<'_>) -> anyhow::Result<()> {
        let mut cur = self
            .request(Method::GET, "/api/certs/ct/config", None)
            .await
            .unwrap_or(serde_json::json!({}));

        if let Some(en) = args.enable {
            cur["enabled"] = serde_json::json!(en);
        }
        if let Some(doms) = args.domains {
            let list: Vec<&str> = doms
                .split(',')
                .map(|s| s.trim())
                .filter(|s| !s.is_empty())
                .collect();
            cur["monitored_domains"] = serde_json::json!(list);
        }
        if let Some(auto) = args.auto_discover {
            cur["auto_discover_domains"] = serde_json::json!(auto);
        }
        if let Some(issuers) = args.allowed_issuers {
            let list: Vec<&str> = issuers
                .split(',')
                .map(|s| s.trim())
                .filter(|s| !s.is_empty())
                .collect();
            cur["allowed_issuers"] = serde_json::json!(list);
        }
        if let Some(alert) = args.alert_unknown {
            cur["alert_on_unknown_issuer"] = serde_json::json!(alert);
        }
        if let Some(int) = args.interval {
            cur["check_interval_secs"] = serde_json::json!(int);
        }
        if let Some(url) = args.api_url {
            cur["ct_api_url"] = serde_json::json!(url);
        }

        let body = serde_json::json!({ "config": cur });
        let res = self
            .request(Method::PUT, "/api/certs/ct/config", Some(&body))
            .await?;
        println!("{}", serde_json::to_string_pretty(&res)?);
        Ok(())
    }

    pub async fn check_ct_logs(&self, domain: Option<&str>) -> anyhow::Result<()> {
        let path = match domain {
            Some(d) => format!("/api/certs/ct/check?domain={d}"),
            None => "/api/certs/ct/check".to_string(),
        };
        let res = self.request(Method::POST, &path, None).await?;
        println!("Certificate Transparency check completed. New rogue certificates: {res}");
        Ok(())
    }

    pub async fn list_ct_entries(
        &self,
        domain: Option<&str>,
        rogue_only: bool,
        limit: Option<usize>,
    ) -> anyhow::Result<()> {
        let mut params = Vec::new();
        if let Some(d) = domain {
            params.push(format!("domain={d}"));
        }
        if rogue_only {
            params.push("rogue_only=true".to_string());
        }
        if let Some(l) = limit {
            params.push(format!("limit={l}"));
        }
        let query = if params.is_empty() {
            String::new()
        } else {
            format!("?{}", params.join("&"))
        };
        let res = self
            .request(Method::GET, &format!("/api/certs/ct/entries{query}"), None)
            .await?;
        println!("{}", serde_json::to_string_pretty(&res)?);
        Ok(())
    }

    pub async fn dismiss_ct_alert(&self, id: &str, status: Option<&str>) -> anyhow::Result<()> {
        let body = serde_json::json!({
            "id": id,
            "status": status,
        });
        self.request(Method::POST, "/api/certs/ct/dismiss", Some(&body))
            .await?;
        println!("Certificate Transparency alert {id} dismissed.");
        Ok(())
    }
}

pub fn parse_manifest(content: &str) -> anyhow::Result<Value> {
    if let Ok(val) = serde_json::from_str::<Value>(content) {
        return Ok(val);
    }
    serde_yaml::from_str::<Value>(content).context("failed to parse manifest as JSON or YAML")
}

pub fn extract_manifest_items(val: Value) -> ManifestItems {
    let mut items = ManifestItems {
        ports: Vec::new(),
        proxies: Vec::new(),
        apps: Vec::new(),
    };
    if let Some(map) = val.as_object() {
        if map.contains_key("ports") || map.contains_key("proxies") || map.contains_key("apps") {
            if let Some(ports) = map.get("ports").and_then(|v| v.as_array()) {
                items.ports.extend(ports.clone());
            }
            if let Some(proxies) = map.get("proxies").and_then(|v| v.as_array()) {
                items.proxies.extend(proxies.clone());
            }
            if let Some(apps) = map.get("apps").and_then(|v| v.as_array()) {
                items.apps.extend(apps.clone());
            }
            return items;
        }
        add_item_by_kind(&mut items, val);
        return items;
    }
    if let Some(list) = val.as_array() {
        for elem in list {
            add_item_by_kind(&mut items, elem.clone());
        }
    }
    items
}

fn add_item_by_kind(items: &mut ManifestItems, item: Value) {
    let kind = item
        .get("kind")
        .and_then(|k| k.as_str())
        .map(|s| s.to_ascii_lowercase())
        .unwrap_or_default();
    match kind.as_str() {
        "port" => items.ports.push(item),
        "proxy" => items.proxies.push(item),
        "app" => items.apps.push(item),
        _ => {}
    }
}

fn print_json(val: &Value) -> anyhow::Result<()> {
    let text = serde_json::to_string_pretty(val)?;
    println!("{text}");
    Ok(())
}

fn print_security_audit_item(item: &Value) {
    let header = item
        .get("header_name")
        .and_then(|v| v.as_str())
        .unwrap_or("");
    let status = item.get("status").and_then(|v| v.as_str()).unwrap_or("");
    let earned = item
        .get("earned_score")
        .and_then(|v| v.as_u64())
        .unwrap_or(0);
    let risk = item.get("risk").and_then(|v| v.as_str()).unwrap_or("");

    let status_tag = match status {
        "present" => "\x1b[32m[SECURED]\x1b[0m",
        "weak" => "\x1b[33m[ WEAK  ]\x1b[0m",
        _ => "\x1b[31m[MISSING]\x1b[0m",
    };

    println!("{:<28} {:<21} {:<6} {}", header, status_tag, earned, risk);
}

fn format_cli_bytes(bytes: u64) -> String {
    if bytes >= 1024 * 1024 * 1024 {
        format!("{:.2} GiB", bytes as f64 / (1024.0 * 1024.0 * 1024.0))
    } else if bytes >= 1024 * 1024 {
        format!("{:.2} MiB", bytes as f64 / (1024.0 * 1024.0))
    } else if bytes >= 1024 {
        format!("{:.1} KiB", bytes as f64 / 1024.0)
    } else {
        format!("{bytes} B")
    }
}

fn format_duration_secs(secs: u64) -> String {
    if secs >= 86400 {
        format!("{}d {}h", secs / 86400, (secs % 86400) / 3600)
    } else if secs >= 3600 {
        format!("{}h {}m", secs / 3600, (secs % 3600) / 60)
    } else if secs >= 60 {
        format!("{}m {}s", secs / 60, secs % 60)
    } else {
        format!("{secs}s")
    }
}

pub fn build_cron_update_payload(
    existing: Option<&Value>,
    cron: Option<&str>,
    command: Option<&str>,
    name: Option<&str>,
    payload: Option<&str>,
) -> anyhow::Result<Value> {
    if let Some(p) = payload {
        return load_payload(p);
    }
    let cur = existing.context("scheduled command not found on application")?;
    let current_name = cur.get("name").and_then(|v| v.as_str()).unwrap_or("");
    let current_schedule = cur
        .get("schedule")
        .or_else(|| cur.get("cron"))
        .and_then(|v| v.as_str())
        .unwrap_or("");
    let current_command = cur.get("command").and_then(|v| v.as_str()).unwrap_or("");
    let current_enabled = cur.get("enabled").and_then(|v| v.as_bool()).unwrap_or(true);

    Ok(serde_json::json!({
        "name": name.unwrap_or(current_name),
        "schedule": cron.unwrap_or(current_schedule),
        "cron": cron.unwrap_or(current_schedule),
        "command": command.unwrap_or(current_command),
        "enabled": current_enabled,
    }))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_map_manifest_json() {
        let json = r#"{
            "ports": [{"listen": "/ip4/0.0.0.0/tcp/80"}],
            "proxies": [{"name": "web", "protocol": "http"}],
            "apps": [{"spec": {"name": "myapp"}}]
        }"#;
        let val = parse_manifest(json).unwrap();
        let items = extract_manifest_items(val);
        assert_eq!(items.ports.len(), 1);
        assert_eq!(items.proxies.len(), 1);
        assert_eq!(items.apps.len(), 1);
    }

    #[test]
    fn parses_list_manifest_yaml() {
        let yaml = r#"
- kind: port
  listen: /ip4/0.0.0.0/tcp/443
- kind: proxy
  name: api
- kind: app
  name: backend
"#;
        let val = parse_manifest(yaml).unwrap();
        let items = extract_manifest_items(val);
        assert_eq!(items.ports.len(), 1);
        assert_eq!(items.proxies.len(), 1);
        assert_eq!(items.apps.len(), 1);
    }

    #[test]
    fn parses_single_resource_yaml() {
        let yaml = r#"
kind: proxy
name: single-proxy
protocol: http
"#;
        let val = parse_manifest(yaml).unwrap();
        let items = extract_manifest_items(val);
        assert_eq!(items.proxies.len(), 1);
        assert_eq!(items.ports.len(), 0);
        assert_eq!(items.apps.len(), 0);
    }

    #[test]
    fn load_payload_parses_json_and_yaml() {
        let json_str = r#"{"name": "test-proxy", "protocol": "http"}"#;
        let val = load_payload(json_str).unwrap();
        assert_eq!(val["name"], "test-proxy");

        let yaml_str = "name: test-proxy\nprotocol: http\n";
        let val2 = load_payload(yaml_str).unwrap();
        assert_eq!(val2["name"], "test-proxy");
    }

    #[test]
    fn load_payload_rejects_garbage() {
        assert!(load_payload("").is_err());
    }

    #[test]
    fn build_registry_payload_constructs_valid_json() {
        let val = build_registry_payload(
            Some("ghcr.io".to_string()),
            Some("octocat".to_string()),
            Some("token123".to_string()),
            None,
        )
        .unwrap();
        assert_eq!(val["server"], "ghcr.io");
        assert_eq!(val["username"], "octocat");
        assert_eq!(val["password"], "token123");

        let custom = build_registry_payload(
            None,
            None,
            None,
            Some(r#"{"server": "docker.io", "username": "user"}"#.to_string()),
        )
        .unwrap();
        assert_eq!(custom["server"], "docker.io");
        assert_eq!(custom["username"], "user");

        assert!(build_registry_payload(None, Some("u".to_string()), None, None).is_err());
        assert!(build_registry_payload(Some("s".to_string()), None, None, None).is_err());
    }

    #[test]
    fn build_git_connection_payload_constructs_valid_json() {
        let val = build_git_connection_payload(
            Some("my-gitlab".to_string()),
            Some("gitlab".to_string()),
            Some("https://gitlab.com".to_string()),
            Some("client123".to_string()),
            Some("secret456".to_string()),
            None,
        )
        .unwrap();
        assert_eq!(val["name"], "my-gitlab");
        assert_eq!(val["provider"], "gitlab");
        assert_eq!(val["client_id"], "client123");
        assert_eq!(val["client_secret"], "secret456");

        assert!(build_git_connection_payload(None, None, None, None, None, None).is_err());
    }

    #[test]
    fn build_account_and_token_payloads_construct_valid_json() {
        let acc = build_account_create_payload(
            Some("alice".to_string()),
            Some("pw123".to_string()),
            "admin",
            None,
        )
        .unwrap();
        assert_eq!(acc["username"], "alice");
        assert_eq!(acc["password"], "pw123");
        assert_eq!(acc["role"], "admin");

        let update = build_account_update_payload(
            Some("newpw".to_string()),
            Some("viewer".to_string()),
            None,
        )
        .unwrap();
        assert_eq!(update["password"], "newpw");
        assert_eq!(update["role"], "viewer");

        let tok = build_token_create_payload(Some("my-token".to_string()), "edit", None).unwrap();
        assert_eq!(tok["name"], "my-token");
        assert_eq!(tok["scope"], "edit");

        assert!(build_account_create_payload(None, None, "editor", None).is_err());
        assert!(build_token_create_payload(None, "read", None).is_err());
    }

    #[test]
    fn build_cron_update_payload_merges_existing() {
        let existing = serde_json::json!({
            "id": "cmd-1",
            "name": "backup",
            "schedule": "0 0 * * *",
            "command": "run-backup.sh",
            "enabled": true
        });

        let updated = build_cron_update_payload(
            Some(&existing),
            Some("*/15 * * * *"),
            None,
            Some("quick backup"),
            None,
        )
        .unwrap();

        assert_eq!(updated["name"], "quick backup");
        assert_eq!(updated["schedule"], "*/15 * * * *");
        assert_eq!(updated["command"], "run-backup.sh");
        assert_eq!(updated["enabled"], true);

        assert!(build_cron_update_payload(None, None, None, None, None).is_err());
    }
}
