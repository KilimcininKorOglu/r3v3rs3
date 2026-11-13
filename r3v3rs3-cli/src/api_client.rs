use anyhow::{Context, bail};
use rand::distr::SampleString;
use reqwest::Method;
use serde_json::Value;

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

fn apply_prune_args(cur: &mut r3v3rs3_api::platform::PrunePolicy, args: &SetPrunePolicyArgs) {
    if let Some(en) = args.enabled {
        cur.enabled = en;
    }
    if let Some(ref intv) = args.interval {
        cur.interval = intv.clone();
    }
    if let Some(dp) = args.disk_pressure {
        cur.disk_pressure_threshold = Some(dp);
    }
    apply_prune_flags(cur, args);
    apply_prune_limits(cur, args);
}

fn apply_prune_flags(cur: &mut r3v3rs3_api::platform::PrunePolicy, args: &SetPrunePolicyArgs) {
    if let Some(pi) = args.prune_images {
        cur.prune_images = pi;
    }
    if let Some(pv) = args.prune_volumes {
        cur.prune_volumes = pv;
    }
    if let Some(pb) = args.prune_build_cache {
        cur.prune_build_cache = pb;
    }
    if let Some(pc) = args.prune_containers {
        cur.prune_containers = pc;
    }
    if let Some(pl) = args.prune_logs {
        cur.prune_logs = pl;
    }
}

fn apply_prune_limits(cur: &mut r3v3rs3_api::platform::PrunePolicy, args: &SetPrunePolicyArgs) {
    if let Some(lt) = args.log_threshold_mb {
        cur.log_vacuum_threshold_mb = Some(lt);
    }
    if let Some(kc) = args.keep_cache_mb {
        cur.build_cache_keep_storage_mb = Some(kc);
    }
    if let Some(kb) = args.kept_builds {
        cur.kept_builds_count = Some(kb);
    }
    if let Some(dl) = args.dangling_layers_only {
        cur.dangling_layers_only = dl;
    }
}

pub fn load_typed_payload<T: serde::de::DeserializeOwned>(input: &str) -> anyhow::Result<T> {
    let val = load_payload(input)?;
    serde_json::from_value(val).context("failed to parse payload into domain type")
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
) -> anyhow::Result<r3v3rs3_api::platform::RegistryRequest> {
    if let Some(p) = payload {
        load_typed_payload(&p)
    } else {
        let s = server
            .ok_or_else(|| anyhow::anyhow!("--server is required when payload is omitted"))?;
        let u = username
            .ok_or_else(|| anyhow::anyhow!("--username is required when payload is omitted"))?;
        Ok(r3v3rs3_api::platform::RegistryRequest {
            server: s,
            username: u,
            password,
        })
    }
}

fn parse_git_provider(
    provider_str: &str,
) -> anyhow::Result<r3v3rs3_api::git_connection::GitProvider> {
    match provider_str.to_ascii_lowercase().as_str() {
        "github" => Ok(r3v3rs3_api::git_connection::GitProvider::Github),
        "gitlab" => Ok(r3v3rs3_api::git_connection::GitProvider::Gitlab),
        "gitea" => Ok(r3v3rs3_api::git_connection::GitProvider::Gitea),
        other => anyhow::bail!("unknown provider: {other}. Expected github, gitlab, or gitea"),
    }
}

fn parse_git_url(
    url: Option<String>,
) -> anyhow::Result<Option<r3v3rs3_api::git_connection::ProviderUrl>> {
    match url {
        Some(u) => Ok(Some(
            u.parse().map_err(|e| anyhow::anyhow!("invalid url: {e}"))?,
        )),
        None => Ok(None),
    }
}

fn parse_git_name(name: Option<String>) -> anyhow::Result<r3v3rs3_api::container::AppName> {
    name.ok_or_else(|| anyhow::anyhow!("--name is required when payload is omitted"))?
        .parse()
        .map_err(|e| anyhow::anyhow!("invalid name: {e}"))
}

fn build_git_connection_payload(
    name: Option<String>,
    provider: Option<String>,
    url: Option<String>,
    client_id: Option<String>,
    client_secret: Option<String>,
    payload: Option<String>,
) -> anyhow::Result<r3v3rs3_api::git_connection::GitConnectionRequest> {
    if let Some(p) = payload {
        return load_typed_payload(&p);
    }
    let n = parse_git_name(name)?;
    let p_str = provider
        .ok_or_else(|| anyhow::anyhow!("--provider is required when payload is omitted"))?;
    let p = parse_git_provider(&p_str)?;
    let cid = client_id
        .ok_or_else(|| anyhow::anyhow!("--client-id is required when payload is omitted"))?;
    let parsed_url = parse_git_url(url)?;

    Ok(r3v3rs3_api::git_connection::GitConnectionRequest {
        name: n,
        provider: p,
        url: parsed_url,
        client_id: cid,
        client_secret,
    })
}

fn build_account_create_payload(
    username: Option<String>,
    password: Option<String>,
    role: &str,
    payload: Option<String>,
) -> anyhow::Result<r3v3rs3_api::auth::NewAccount> {
    if let Some(p) = payload {
        load_typed_payload(&p)
    } else {
        let u = username
            .ok_or_else(|| anyhow::anyhow!("--username is required when payload is omitted"))?;
        let p = password
            .ok_or_else(|| anyhow::anyhow!("--password is required when payload is omitted"))?;
        let r = match role.to_ascii_lowercase().as_str() {
            "admin" => r3v3rs3_api::auth::Role::Admin,
            "editor" => r3v3rs3_api::auth::Role::Editor,
            "viewer" => r3v3rs3_api::auth::Role::Viewer,
            other => anyhow::bail!("unknown role: {other}. Expected admin, editor, or viewer"),
        };
        Ok(r3v3rs3_api::auth::NewAccount {
            username: u,
            password: p,
            role: r,
            proxies: None,
            totp: false,
        })
    }
}

fn build_account_update_payload(
    password: Option<String>,
    role: Option<String>,
    payload: Option<String>,
) -> anyhow::Result<r3v3rs3_api::auth::AccountUpdate> {
    if let Some(p) = payload {
        load_typed_payload(&p)
    } else {
        let r = match role.as_deref() {
            Some("admin") => r3v3rs3_api::auth::Role::Admin,
            Some("editor") => r3v3rs3_api::auth::Role::Editor,
            Some("viewer") => r3v3rs3_api::auth::Role::Viewer,
            Some(other) => {
                anyhow::bail!("unknown role: {other}. Expected admin, editor, or viewer")
            }
            None => r3v3rs3_api::auth::Role::Editor,
        };
        Ok(r3v3rs3_api::auth::AccountUpdate {
            password,
            role: r,
            proxies: None,
        })
    }
}

fn build_token_create_payload(
    name: Option<String>,
    scope: &str,
    payload: Option<String>,
) -> anyhow::Result<r3v3rs3_api::auth::CreateApiTokenRequest> {
    if let Some(p) = payload {
        load_typed_payload(&p)
    } else {
        let n =
            name.ok_or_else(|| anyhow::anyhow!("--name is required when payload is omitted"))?;
        let sc = match scope.to_ascii_lowercase().as_str() {
            "read" => r3v3rs3_api::auth::TokenScope::Read,
            "edit" | "write" => r3v3rs3_api::auth::TokenScope::Edit,
            other => anyhow::bail!("unknown scope: {other}. Expected 'read' or 'edit'"),
        };
        Ok(r3v3rs3_api::auth::CreateApiTokenRequest { name: n, scope: sc })
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

    pub async fn get_json<T: serde::de::DeserializeOwned>(&self, path: &str) -> anyhow::Result<T> {
        let val = self.request(Method::GET, path, None).await?;
        serde_json::from_value(val).context("failed to deserialize response")
    }

    pub async fn post_json<B: serde::Serialize, T: serde::de::DeserializeOwned>(
        &self,
        path: &str,
        body: &B,
    ) -> anyhow::Result<T> {
        let body_val = serde_json::to_value(body)?;
        let res_val = self.request(Method::POST, path, Some(&body_val)).await?;
        serde_json::from_value(res_val).context("failed to deserialize response")
    }

    pub async fn put_json<B: serde::Serialize, T: serde::de::DeserializeOwned>(
        &self,
        path: &str,
        body: &B,
    ) -> anyhow::Result<T> {
        let body_val = serde_json::to_value(body)?;
        let res_val = self.request(Method::PUT, path, Some(&body_val)).await?;
        serde_json::from_value(res_val).context("failed to deserialize response")
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
        let entries: Vec<r3v3rs3_api::port::PortEntry> = self.get_json("/api/ports").await?;
        println!("{:<26} {:<32} STATUS", "ID", "LISTEN ADDRESS");
        println!("{:-<26} {:-<32} {:-<10}", "", "", "");
        for item in &entries {
            let status = if item.port.active {
                "active"
            } else {
                "inactive"
            };
            println!("{:<26} {:<32} {}", item.id, item.port.listen, status);
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
        let port: r3v3rs3_api::port::Port = load_typed_payload(payload)?;
        let res = self
            .request(
                Method::POST,
                "/api/ports",
                Some(&serde_json::to_value(&port)?),
            )
            .await?;
        println!("port added");
        print_json(&res)
    }

    pub async fn update_port(&self, id: &str, payload: &str) -> anyhow::Result<()> {
        let port: r3v3rs3_api::port::Port = load_typed_payload(payload)?;
        let res = self
            .request(
                Method::PUT,
                &format!("/api/ports/{id}"),
                Some(&serde_json::to_value(&port)?),
            )
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
        self.request(Method::POST, &format!("/api/ports/{id}/reset"), None)
            .await?;
        println!("port/{id} listener reset");
        Ok(())
    }

    pub async fn set_port_early_data(&self, id: &str, enable: bool) -> anyhow::Result<()> {
        let entry: r3v3rs3_api::port::PortEntry =
            self.get_json(&format!("/api/ports/{id}")).await?;
        let mut port = entry.port;
        if let Some(ref mut tls) = port.opts.tls_termination {
            tls.early_data = enable;
        } else if enable {
            anyhow::bail!("port/{id} does not have TLS termination configured");
        }
        let res = self
            .request(
                Method::PUT,
                &format!("/api/ports/{id}"),
                Some(&serde_json::to_value(&port)?),
            )
            .await?;
        println!("port/{id} early_data set to {enable}");
        print_json(&res)
    }

    // --- Proxy operations ---

    pub async fn list_proxies(&self) -> anyhow::Result<()> {
        let entries: Vec<r3v3rs3_api::proxy::ProxyEntry> = self.get_json("/api/proxies").await?;
        println!("{:<26} {:<24} {:<12} PORTS", "ID", "NAME", "PROTOCOL");
        println!("{:-<26} {:-<24} {:-<12} {:-<16}", "", "", "", "");
        for item in &entries {
            let protocol = match &item.proxy.kind {
                r3v3rs3_api::proxy::ProxyKind::Http(_) => "http",
                r3v3rs3_api::proxy::ProxyKind::Tcp(_) => "tcp",
                r3v3rs3_api::proxy::ProxyKind::Udp(_) => "udp",
            };
            let ports = item
                .proxy
                .ports
                .iter()
                .map(ToString::to_string)
                .collect::<Vec<_>>()
                .join(", ");
            println!(
                "{:<26} {:<24} {:<12} {}",
                item.id, item.proxy.name, protocol, ports
            );
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
        let proxy: r3v3rs3_api::proxy::Proxy = load_typed_payload(payload)?;
        let res = self
            .request(
                Method::POST,
                "/api/proxies",
                Some(&serde_json::to_value(&proxy)?),
            )
            .await?;
        println!("proxy added");
        print_json(&res)
    }

    pub async fn update_proxy(&self, id: &str, payload: &str) -> anyhow::Result<()> {
        let proxy: r3v3rs3_api::proxy::Proxy = load_typed_payload(payload)?;
        let res = self
            .request(
                Method::PUT,
                &format!("/api/proxies/{id}"),
                Some(&serde_json::to_value(&proxy)?),
            )
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
        let body = r3v3rs3_api::grpc::GrpcHealthCheckRequest {
            service: service.map(str::to_string),
            target_url: target_url.map(str::to_string),
        };
        let res: r3v3rs3_api::grpc::GrpcHealthCheckResponse = self
            .post_json(&format!("/api/proxies/{id}/grpc/health"), &body)
            .await?;
        let healthy = res.healthy;
        let status = res.status;
        let status_code = res.status_code;
        let grpc_status = res.grpc_status;
        let grpc_name = &res.grpc_status_name;
        let latency_ms = res.latency_ms;

        let status_str = status.as_str();
        let status_colored = if healthy {
            format!("\x1b[32m{status_str}\x1b[0m")
        } else {
            format!("\x1b[31m{status_str}\x1b[0m")
        };
        println!("Proxy ID:       {id}");
        println!("Healthy:        {}", if healthy { "yes" } else { "no" });
        println!("Serving Status: {status_colored} (code {status_code})");
        println!("gRPC Status:    {grpc_status} ({grpc_name})");
        if let Some(ref msg) = res.grpc_message {
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
        let entry: r3v3rs3_api::proxy::ProxyEntry =
            self.get_json(&format!("/api/proxies/{proxy_id}")).await?;
        let r3v3rs3_api::proxy::ProxyKind::Http(ref http) = entry.proxy.kind else {
            println!("Proxy {proxy_id} is not an HTTP proxy");
            return Ok(());
        };
        let compression = &http.compression;
        if compression.dictionaries.is_empty() {
            println!("No compression dictionaries configured for proxy {proxy_id}");
            return Ok(());
        }
        println!(
            "{:<26} {:<20} {:<15} {:<30} PATH",
            "ID", "NAME", "MATCH", "PATH"
        );
        println!(
            "{:-<26} {:-<20} {:-<15} {:-<30} {:-<30}",
            "", "", "", "", ""
        );
        for item in &compression.dictionaries {
            println!(
                "{:<26} {:<20} {:<15} {:<30} {}",
                item.id, item.name, item.match_pattern, item.path, item.path
            );
        }
        Ok(())
    }

    pub async fn get_proxy_dictionary(&self, proxy_id: &str, dict_id: &str) -> anyhow::Result<()> {
        let entry: r3v3rs3_api::proxy::ProxyEntry =
            self.get_json(&format!("/api/proxies/{proxy_id}")).await?;
        let r3v3rs3_api::proxy::ProxyKind::Http(ref http) = entry.proxy.kind else {
            bail!("Proxy {proxy_id} is not an HTTP proxy");
        };
        let compression = &http.compression;
        let dict = compression
            .dictionaries
            .iter()
            .find(|d| d.id.to_string() == dict_id);
        let Some(d) = dict else {
            bail!("Dictionary {dict_id} not found in proxy {proxy_id}");
        };
        println!("{}", serde_json::to_string_pretty(d)?);
        Ok(())
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
        let mut entry: r3v3rs3_api::proxy::ProxyEntry =
            self.get_json(&format!("/api/proxies/{proxy_id}")).await?;
        let dict_id = rand::distr::Alphanumeric
            .sample_string(&mut rand::rng(), 24)
            .to_ascii_lowercase();
        let dict = r3v3rs3_api::compression_dictionary::CompressionDictionary {
            id: dict_id
                .parse()
                .map_err(|e| anyhow::anyhow!("invalid dict id: {e}"))?,
            name: name.to_string(),
            match_pattern: match_pattern.to_string(),
            path: path.to_string(),
            raw_data,
            sha256_sf: String::new(),
        };
        let r3v3rs3_api::proxy::ProxyKind::Http(ref mut http) = entry.proxy.kind else {
            bail!("Proxy {proxy_id} is not an HTTP proxy");
        };
        http.compression.dictionaries.push(dict);
        let updated = self
            .request(
                Method::PUT,
                &format!("/api/proxies/{proxy_id}"),
                Some(&serde_json::to_value(&entry.proxy)?),
            )
            .await?;
        println!("Compression dictionary added to proxy {proxy_id}");
        print_json(&updated)
    }

    pub async fn delete_proxy_dictionary(
        &self,
        proxy_id: &str,
        dict_id: &str,
    ) -> anyhow::Result<()> {
        let mut entry: r3v3rs3_api::proxy::ProxyEntry =
            self.get_json(&format!("/api/proxies/{proxy_id}")).await?;
        let r3v3rs3_api::proxy::ProxyKind::Http(ref mut http) = entry.proxy.kind else {
            bail!("Proxy {proxy_id} is not an HTTP proxy");
        };
        let initial_len = http.compression.dictionaries.len();
        http.compression
            .dictionaries
            .retain(|d| d.id.to_string() != dict_id);
        if http.compression.dictionaries.len() == initial_len {
            bail!("Dictionary {dict_id} not found in proxy {proxy_id}");
        }
        let updated = self
            .request(
                Method::PUT,
                &format!("/api/proxies/{proxy_id}"),
                Some(&serde_json::to_value(&entry.proxy)?),
            )
            .await?;
        println!("Compression dictionary {dict_id} deleted from proxy {proxy_id}");
        print_json(&updated)
    }

    // --- Certificate operations ---

    pub async fn list_certs(&self) -> anyhow::Result<()> {
        let entries: Vec<r3v3rs3_api::cert::CertInfo> = self.get_json("/api/certs").await?;
        println!("{:<26} {:<30} {:<12} ISSUER", "ID", "DOMAINS", "EXPIRES");
        println!("{:-<26} {:-<30} {:-<12} {:-<20}", "", "", "", "");
        for item in &entries {
            let domains = item
                .san
                .iter()
                .map(ToString::to_string)
                .collect::<Vec<_>>()
                .join(", ");
            let expires = item.not_after.to_string();
            println!(
                "{:<26} {:<30} {:<12} {}",
                item.id, domains, expires, item.issuer
            );
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
        let entries: Vec<r3v3rs3_api::acme::AcmeInfo> = self.get_json("/api/acme").await?;
        println!("{:<26} {:<32} IDENTIFIERS", "ID", "ACME SERVER");
        println!("{:-<26} {:-<32} {:-<30}", "", "", "");
        for item in &entries {
            let identifiers = item.identifiers.join(", ");
            println!(
                "{:<26} {:<32} {}",
                item.id, item.config.provider, identifiers
            );
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
        let req: r3v3rs3_api::acme::AcmeRequest = load_typed_payload(payload)?;
        let res = self
            .request(
                Method::POST,
                "/api/acme",
                Some(&serde_json::to_value(&req)?),
            )
            .await?;
        println!("acme order added");
        print_json(&res)
    }

    pub async fn update_acme(&self, id: &str, payload: &str) -> anyhow::Result<()> {
        let req: r3v3rs3_api::acme::AcmeRequest = load_typed_payload(payload)?;
        let res = self
            .request(
                Method::PUT,
                &format!("/api/acme/{id}"),
                Some(&serde_json::to_value(&req)?),
            )
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
        let entries: Vec<r3v3rs3_api::access_list::AccessListEntry> =
            self.get_json("/api/access-lists").await?;
        println!("{:<26} {:<30} RULES", "ID", "NAME");
        println!("{:-<26} {:-<30} {:-<10}", "", "", "");
        for item in &entries {
            let rules = item.list.ip_filter.allow.len() + item.list.ip_filter.deny.len();
            println!("{:<26} {:<30} {}", item.id, item.list.name, rules);
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
        let list: r3v3rs3_api::access_list::AccessList = load_typed_payload(payload)?;
        let res = self
            .request(
                Method::POST,
                "/api/access-lists",
                Some(&serde_json::to_value(&list)?),
            )
            .await?;
        println!("access-list added");
        print_json(&res)
    }

    pub async fn update_acl(&self, id: &str, payload: &str) -> anyhow::Result<()> {
        let list: r3v3rs3_api::access_list::AccessList = load_typed_payload(payload)?;
        let res = self
            .request(
                Method::PUT,
                &format!("/api/access-lists/{id}"),
                Some(&serde_json::to_value(&list)?),
            )
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
        let entries: Vec<r3v3rs3_api::error_page_theme::ErrorPageTheme> =
            self.get_json("/api/error-page-themes").await?;
        println!("{:<26} {:<28} {:<10} DESCRIPTION", "ID", "NAME", "PAGES");
        println!("{:-<26} {:-<28} {:-<10} {:-<30}", "", "", "", "");
        for item in &entries {
            println!(
                "{:<26} {:<28} {:<10} {}",
                item.id,
                item.name,
                item.pages.len(),
                item.description
            );
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
        let theme: r3v3rs3_api::error_page_theme::ErrorPageTheme = load_typed_payload(payload)?;
        let res = self
            .request(
                Method::POST,
                "/api/error-page-themes",
                Some(&serde_json::to_value(&theme)?),
            )
            .await?;
        println!("error-page-theme added");
        print_json(&res)
    }

    pub async fn update_error_page_theme(&self, id: &str, payload: &str) -> anyhow::Result<()> {
        let theme: r3v3rs3_api::error_page_theme::ErrorPageTheme = load_typed_payload(payload)?;
        let res = self
            .request(
                Method::PUT,
                &format!("/api/error-page-themes/{id}"),
                Some(&serde_json::to_value(&theme)?),
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
        let entries: Vec<r3v3rs3_api::acme::DnsCredentialInfo> =
            self.get_json("/api/dns-credentials").await?;
        println!("{:<26} {:<30} PROVIDER", "ID", "NAME");
        println!("{:-<26} {:-<30} {:-<20}", "", "", "");
        for item in &entries {
            println!(
                "{:<26} {:<30} {}",
                item.id,
                item.name,
                item.provider.as_str()
            );
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
        let req: r3v3rs3_api::acme::DnsCredentialRequest = load_typed_payload(payload)?;
        let res = self
            .request(
                Method::POST,
                "/api/dns-credentials",
                Some(&serde_json::to_value(&req)?),
            )
            .await?;
        println!("dns credential added");
        print_json(&res)
    }

    pub async fn update_dns(&self, id: &str, payload: &str) -> anyhow::Result<()> {
        let req: r3v3rs3_api::acme::DnsCredentialRequest = load_typed_payload(payload)?;
        let res = self
            .request(
                Method::PUT,
                &format!("/api/dns-credentials/{id}"),
                Some(&serde_json::to_value(&req)?),
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
        let entries: Vec<r3v3rs3_api::platform::AppEntry> = self.get_json("/api/apps").await?;
        println!("{:<26} {:<20} {:<14} DOMAINS", "ID", "NAME", "TARGET");
        println!("{:-<26} {:-<20} {:-<14} {:-<20}", "", "", "", "");
        for item in &entries {
            let domains = item
                .spec
                .domains
                .iter()
                .map(ToString::to_string)
                .collect::<Vec<_>>()
                .join(", ");
            println!(
                "{:<26} {:<20} {:<14} {}",
                item.id, item.name, item.target, domains
            );
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

    pub async fn cancel_deployment(&self, id: &str) -> anyhow::Result<()> {
        let res = self
            .request(Method::POST, &format!("/api/deployments/{id}/cancel"), None)
            .await?;
        print_json(&res)
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
        let req: r3v3rs3_api::platform::AppRequest = load_typed_payload(payload)?;
        let res = self
            .request(
                Method::POST,
                "/api/apps",
                Some(&serde_json::to_value(&req)?),
            )
            .await?;
        println!("app created");
        print_json(&res)
    }

    pub async fn update_app(&self, id: &str, payload: &str) -> anyhow::Result<()> {
        let req: r3v3rs3_api::platform::AppRequest = load_typed_payload(payload)?;
        let res = self
            .request(
                Method::PUT,
                &format!("/api/apps/{id}"),
                Some(&serde_json::to_value(&req)?),
            )
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
        let mut entries = Vec::with_capacity(pairs.len());
        for pair in pairs {
            if let Some((k, v)) = pair.split_once('=') {
                let key: r3v3rs3_api::container::EnvKey = k
                    .parse()
                    .map_err(|e| anyhow::anyhow!("invalid env key '{k}': {e}"))?;
                let is_secret = is_likely_secret_key(k);
                entries.push(r3v3rs3_api::platform::EnvEntry {
                    key,
                    value: Some(v.to_string()),
                    secret: is_secret,
                });
            } else {
                bail!("invalid env pair '{pair}', expected KEY=VALUE format");
            }
        }
        self.request(
            Method::PUT,
            &format!("/api/apps/{id}/env"),
            Some(&serde_json::to_value(&entries)?),
        )
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
        let body = r3v3rs3_api::compose_env::GenerateAppEnvSecretsRequest { save, overwrite };
        let res = self
            .request(
                Method::POST,
                &format!("/api/apps/{id}/env/generate"),
                Some(&serde_json::to_value(&body)?),
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
        let body = r3v3rs3_api::compose_env::ResolveEnvTemplateRequest {
            template: template_content,
            entries: None,
            overwrite,
        };
        let res = self
            .request(
                Method::POST,
                "/api/compose/env/generate",
                Some(&serde_json::to_value(&body)?),
            )
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

fn resolve_store_source(
    store_app: &r3v3rs3_api::store::StoreApp,
) -> anyhow::Result<r3v3rs3_api::platform::AppSource> {
    if let Some(ref image) = store_app.image {
        return Ok(r3v3rs3_api::platform::AppSource::Image {
            image: image
                .parse()
                .map_err(|e| anyhow::anyhow!("invalid image: {e}"))?,
        });
    }
    if let Some(ref repo) = store_app.repository {
        let service_str = store_app.service.as_deref().unwrap_or("web");
        return Ok(r3v3rs3_api::platform::AppSource::Compose {
            repository: Some(
                repo.parse()
                    .map_err(|e| anyhow::anyhow!("invalid repository URL: {e}"))?,
            ),
            branch: store_app
                .branch
                .as_deref()
                .unwrap_or("main")
                .parse()
                .map_err(|e| anyhow::anyhow!("invalid branch: {e}"))?,
            file: store_app.file.as_deref().and_then(|f| f.parse().ok()),
            service: service_str
                .parse()
                .map_err(|e| anyhow::anyhow!("invalid service: {e}"))?,
            connection: None,
            compose_yaml: None,
        });
    }
    anyhow::bail!(
        "store app '{}' has neither image nor repository",
        store_app.id
    )
}

fn build_store_spec(
    store_app: r3v3rs3_api::store::StoreApp,
    domains: Vec<r3v3rs3_api::subject_name::SubjectName>,
) -> anyhow::Result<r3v3rs3_api::platform::AppSpec> {
    let source = resolve_store_source(&store_app)?;
    Ok(r3v3rs3_api::platform::AppSpec {
        source,
        port: store_app.port,
        domains,
        ports: Vec::new(),
        health_check_path: store_app.health_check_path,
        volumes: Vec::new(),
        restart: Default::default(),
        limits: Default::default(),
        host_port: None,
        database: None,
        uses_database: None,
        replica_of: None,
        auto_deploy: false,
        require_approval: false,
        deploy_window: None,
        tag_trigger: None,
        store_app_id: Some(store_app.id),
        targets: Vec::new(),
        load_balancing: None,
        alert_rules: None,
        sticky: None,
        git_config: None,
        env_groups: Vec::new(),
        preview: None,
        preview_of: None,
        buildkit: None,
        bot_blocker: false,
        grpc_web: false,
        frame_protection: true,
        project_id: None,
        environment_id: None,
        lifecycle_hooks: None,
    })
}

impl ApiClient {
    async fn clear_app_hooks(
        &self,
        id: &str,
        mut entry: r3v3rs3_api::platform::AppEntry,
    ) -> anyhow::Result<()> {
        entry.spec.lifecycle_hooks = None;
        let req = r3v3rs3_api::platform::AppRequest {
            name: entry.name,
            target: entry.target,
            spec: entry.spec,
        };
        let _res: r3v3rs3_api::platform::AppEntry =
            self.put_json(&format!("/api/apps/{id}"), &req).await?;
        println!("app/{id} lifecycle hooks cleared");
        Ok(())
    }

    async fn update_app_hooks(
        &self,
        id: &str,
        mut entry: r3v3rs3_api::platform::AppEntry,
        pre_deploy: Option<String>,
        post_deploy: Option<String>,
    ) -> anyhow::Result<()> {
        let mut hooks = entry.spec.lifecycle_hooks.take().unwrap_or_default();
        if let Some(cmd) = pre_deploy {
            hooks.pre_deploy = Some(cmd);
        }
        if let Some(cmd) = post_deploy {
            hooks.post_deploy = Some(cmd);
        }
        entry.spec.lifecycle_hooks = Some(hooks);
        let req = r3v3rs3_api::platform::AppRequest {
            name: entry.name,
            target: entry.target,
            spec: entry.spec,
        };
        let _res: r3v3rs3_api::platform::AppEntry =
            self.put_json(&format!("/api/apps/{id}"), &req).await?;
        println!("app/{id} lifecycle hooks updated");
        Ok(())
    }

    pub async fn app_hooks(
        &self,
        id: &str,
        pre_deploy: Option<String>,
        post_deploy: Option<String>,
        clear: bool,
    ) -> anyhow::Result<()> {
        let entry: r3v3rs3_api::platform::AppEntry =
            self.get_json(&format!("/api/apps/{id}")).await?;
        if clear {
            return self.clear_app_hooks(id, entry).await;
        }
        if pre_deploy.is_some() || post_deploy.is_some() {
            return self
                .update_app_hooks(id, entry, pre_deploy, post_deploy)
                .await;
        }
        if let Some(ref hooks) = entry.spec.lifecycle_hooks {
            print_json(&serde_json::to_value(hooks)?)?;
        } else {
            println!("No lifecycle hooks configured for app/{id}");
        }
        Ok(())
    }

    // --- Backup operations ---

    pub async fn list_backups(&self, database_id: &str) -> anyhow::Result<()> {
        let entries: Vec<r3v3rs3_api::platform::BackupEntry> = self
            .get_json(&format!("/api/databases/{database_id}/backups"))
            .await?;
        println!(
            "{:<26} {:<10} {:<10} CREATED",
            "BACKUP ID", "SIZE", "STATUS"
        );
        println!("{:-<26} {:-<10} {:-<10} {:-<20}", "", "", "", "");
        for item in &entries {
            let status = item.verification_status.as_deref().unwrap_or("-");
            println!(
                "{:<26} {:<10} {:<10} {}",
                item.id,
                format_cli_bytes(item.size),
                status,
                item.created_at
            );
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

    pub async fn test_database_backup(
        &self,
        database_id: &str,
        backup_id: &str,
    ) -> anyhow::Result<()> {
        let res = self
            .request(
                Method::POST,
                &format!("/api/databases/{database_id}/backups/{backup_id}/test"),
                None,
            )
            .await?;
        println!("backup/{backup_id} test verification completed for database/{database_id}");
        print_json(&res)
    }

    pub async fn get_database_pitr_range(&self, database_id: &str) -> anyhow::Result<()> {
        let res = self
            .request(
                Method::GET,
                &format!("/api/databases/{database_id}/pitr"),
                None,
            )
            .await?;
        print_json(&res)
    }

    pub async fn restore_database_pitr(
        &self,
        database_id: &str,
        target_time: u64,
        new_name: Option<&str>,
        host_port: Option<u16>,
    ) -> anyhow::Result<()> {
        let body =
            build_pitr_restore_payload(target_time, new_name.map(str::to_string), host_port)?;
        let res = self
            .request(
                Method::POST,
                &format!("/api/databases/{database_id}/pitr/restore"),
                Some(&serde_json::to_value(&body)?),
            )
            .await?;
        println!("database/{database_id} PITR restore initiated to timestamp {target_time}");
        print_json(&res)
    }

    pub async fn archive_database_wal(&self, database_id: &str) -> anyhow::Result<()> {
        let res = self
            .request(
                Method::POST,
                &format!("/api/databases/{database_id}/wal/archive"),
                None,
            )
            .await?;
        println!("database/{database_id} WAL segment archival triggered");
        print_json(&res)
    }

    pub async fn list_database_wal_segments(&self, database_id: &str) -> anyhow::Result<()> {
        let res = self
            .request(
                Method::GET,
                &format!("/api/databases/{database_id}/wal/segments"),
                None,
            )
            .await?;
        if let Some(arr) = res.as_array() {
            println!("{:<32} {:<14} STATUS", "FILENAME", "BYTES");
            println!("{:-<32} {:-<14} {:-<10}", "", "", "");
            for item in arr {
                let name = item.get("filename").and_then(|v| v.as_str()).unwrap_or("-");
                let bytes = item.get("bytes").and_then(|v| v.as_u64()).unwrap_or(0);
                println!("{:<32} {:<14} archived", name, format_cli_bytes(bytes));
            }
        } else {
            print_json(&res)?;
        }
        Ok(())
    }

    // --- Disaster Recovery operations ---

    pub async fn list_dr_backups(&self) -> anyhow::Result<()> {
        let entries: Vec<r3v3rs3_api::platform::DisasterRecoveryBackupEntry> =
            self.get_json("/api/disaster-recovery/backups").await?;
        println!(
            "{:<26} {:<10} {:<8} {:<15} CREATED",
            "SNAPSHOT ID", "SIZE", "FILES", "S3 REPLICATED"
        );
        println!("{:-<26} {:-<10} {:-<8} {:-<15} {:-<20}", "", "", "", "", "");
        for item in &entries {
            println!(
                "{:<26} {:<10} {:<8} {:<15} {}",
                item.id,
                format_cli_bytes(item.size),
                item.files_count,
                if item.s3_replicated { "yes" } else { "no" },
                item.created_at
            );
        }
        Ok(())
    }

    pub async fn create_dr_backup(
        &self,
        passphrase: Option<&str>,
        upload_s3: bool,
    ) -> anyhow::Result<()> {
        let payload = r3v3rs3_api::platform::CreateDisasterRecoveryRequest {
            passphrase: passphrase.map(str::to_string),
            upload_s3,
        };
        let res = self
            .request(
                Method::POST,
                "/api/disaster-recovery/backups",
                Some(&serde_json::to_value(&payload)?),
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
        let payload = r3v3rs3_api::platform::RestoreDisasterRecoveryRequest {
            passphrase: passphrase.map(str::to_string),
        };
        self.request(
            Method::POST,
            &format!("/api/disaster-recovery/backups/{backup_id}/restore"),
            Some(&serde_json::to_value(&payload)?),
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
        let entries: Vec<r3v3rs3_api::platform::AppEntry> = self.get_json("/api/databases").await?;
        println!("{:<26} {:<20} {:<12} TARGET", "ID", "NAME", "ENGINE");
        println!("{:-<26} {:-<20} {:-<12} {:-<10}", "", "", "", "");
        for item in &entries {
            let engine = item.spec.database.as_deref().unwrap_or("-");
            println!(
                "{:<26} {:<20} {:<12} {}",
                item.id, item.name, engine, item.target
            );
        }
        Ok(())
    }

    /// A database is an app, so the app route returns it.
    pub async fn get_db(&self, id: &str) -> anyhow::Result<()> {
        let res = self
            .request(Method::GET, &format!("/api/apps/{id}"), None)
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
        let _ = password;
        let req = r3v3rs3_api::platform::DatabaseRequest {
            name: name
                .parse()
                .map_err(|e| anyhow::anyhow!("invalid db name: {e}"))?,
            engine: engine.to_string(),
            version: version.unwrap_or(default_ver).to_string(),
            host_port: None,
            limits: Default::default(),
            project_id: None,
            environment_id: None,
        };
        let res = self
            .request(
                Method::POST,
                "/api/databases",
                Some(&serde_json::to_value(&req)?),
            )
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
        let req = r3v3rs3_api::platform::DatabaseCloneRequest {
            name: name
                .parse()
                .map_err(|e| anyhow::anyhow!("invalid clone name: {e}"))?,
            host_port: None,
            limits: Default::default(),
        };
        let res = self
            .request(
                Method::POST,
                &format!("/api/databases/{id}/clone"),
                Some(&serde_json::to_value(&req)?),
            )
            .await?;
        println!("database/{id} clone triggered");
        print_json(&res)
    }

    pub async fn replica_db(&self, id: &str, name: &str) -> anyhow::Result<()> {
        let req = r3v3rs3_api::platform::DatabaseReplicaRequest {
            name: name
                .parse()
                .map_err(|e| anyhow::anyhow!("invalid replica name: {e}"))?,
            host_port: None,
            limits: Default::default(),
            project_id: None,
            environment_id: None,
        };
        let res = self
            .request(
                Method::POST,
                &format!("/api/databases/{id}/replicas"),
                Some(&serde_json::to_value(&req)?),
            )
            .await?;
        println!("database/{id} replica created");
        print_json(&res)
    }

    pub async fn db_logs(&self, id: &str, tail: usize) -> anyhow::Result<()> {
        let res = self
            .request(
                Method::GET,
                &format!("/api/apps/{id}/logs?tail={tail}"),
                None,
            )
            .await?;
        print_json(&res)
    }

    // --- Target operations ---

    pub async fn list_targets(&self) -> anyhow::Result<()> {
        let entries: Vec<r3v3rs3_api::platform::TargetEntry> =
            self.get_json("/api/targets").await?;
        println!("{:<26} {:<20} {:<12} STATUS", "ID", "NAME", "KIND");
        println!("{:-<26} {:-<20} {:-<12} {:-<10}", "", "", "", "");
        for item in &entries {
            let kind = match item.kind {
                r3v3rs3_api::platform::TargetKind::Local => "local",
                r3v3rs3_api::platform::TargetKind::Agent => "agent",
            };
            let status = if item.online { "online" } else { "offline" };
            println!("{:<26} {:<20} {:<12} {}", item.id, item.name, kind, status);
        }
        Ok(())
    }

    pub async fn get_target(&self, id: &str) -> anyhow::Result<()> {
        let res = self
            .request(Method::GET, &format!("/api/targets/{id}"), None)
            .await?;
        print_json(&res)
    }

    pub async fn add_target(&self, name: &str, region: Option<&str>) -> anyhow::Result<()> {
        let req = r3v3rs3_api::platform::TargetRequest {
            name: name
                .parse()
                .map_err(|e| anyhow::anyhow!("invalid target name: {e}"))?,
            region: region.map(str::to_string),
        };
        let res: r3v3rs3_api::platform::TargetToken = self.post_json("/api/targets", &req).await?;
        println!(
            "target added: id={}, enrollment token={}",
            res.target.id, res.token
        );
        Ok(())
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
        let payload = r3v3rs3_api::platform::SetTargetRegionRequest {
            region: region.map(str::to_string),
        };
        let updated: serde_json::Value = self
            .request(
                Method::PUT,
                &format!("/api/targets/{id}/region"),
                Some(&serde_json::to_value(&payload)?),
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

    pub async fn upgrade_target(&self, id: &str) -> anyhow::Result<()> {
        let res = self
            .request(Method::POST, &format!("/api/targets/{id}/upgrade"), None)
            .await?;
        println!("target/{id} binary upgrade initiated");
        print_json(&res)
    }

    pub async fn upgrade_all_targets(&self) -> anyhow::Result<()> {
        let res = self
            .request(Method::POST, "/api/targets/upgrade", None)
            .await?;
        println!("all agent targets binary upgrade initiated");
        print_json(&res)
    }

    // --- Project operations ---

    pub async fn list_projects(&self) -> anyhow::Result<()> {
        let entries: Vec<r3v3rs3_api::platform::ProjectSummary> =
            self.get_json("/api/projects").await?;
        println!("{:<26} {:<30} {:<15} APPS", "ID", "NAME", "ENVIRONMENTS");
        println!("{:-<26} {:-<30} {:-<15} {:-<10}", "", "", "", "");
        for item in &entries {
            println!(
                "{:<26} {:<30} {:<15} {}",
                item.id, item.name, item.environments_count, item.apps_count
            );
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
        let body = r3v3rs3_api::platform::ProjectSpec {
            name: name.to_string(),
            description: description.map(str::to_string),
            default_environments: Vec::new(),
        };
        let res = self
            .request(
                Method::POST,
                "/api/projects",
                Some(&serde_json::to_value(&body)?),
            )
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
        let body = r3v3rs3_api::platform::EnvironmentSyncRequest {
            source_env_id: from
                .parse()
                .map_err(|e| anyhow::anyhow!("invalid source env id: {e}"))?,
            target_env_id: to
                .parse()
                .map_err(|e| anyhow::anyhow!("invalid target env id: {e}"))?,
            mode: r3v3rs3_api::platform::EnvironmentSyncMode::SyncMissing,
            keys: Vec::new(),
        };
        let res = self
            .request(
                Method::POST,
                &format!("/api/projects/{project_id}/environments/sync"),
                Some(&serde_json::to_value(&body)?),
            )
            .await?;
        println!("project/{project_id} environment variables synchronized from {from} to {to}");
        print_json(&res)
    }

    // --- App Store operations ---

    pub async fn list_store(&self) -> anyhow::Result<()> {
        let entries: Vec<r3v3rs3_api::store::StoreApp> = self.get_json("/api/store").await?;
        println!("{:<24} {:<24} {:<16} TYPE", "ID", "NAME", "CATEGORY");
        println!("{:-<24} {:-<24} {:-<16} {:-<10}", "", "", "", "");
        for item in &entries {
            let cat = item.category.as_deref().unwrap_or("-");
            let typ = if item.file.is_some() || item.repository.is_some() {
                "compose"
            } else {
                "image"
            };
            println!("{:<24} {:<24} {:<16} {}", item.id, item.name, cat, typ);
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
            domains.push(
                d.parse()
                    .map_err(|e| anyhow::anyhow!("invalid domain: {e}"))?,
            );
        }
        let catalog: r3v3rs3_api::store::StoreCatalog = self.get_json("/api/store").await?;
        let store_app = catalog
            .apps
            .into_iter()
            .find(|a| a.id == app_id)
            .ok_or_else(|| anyhow::anyhow!("app '{app_id}' not found in store catalog"))?;

        let spec = build_store_spec(store_app, domains)?;
        let req = r3v3rs3_api::platform::AppRequest {
            name: name
                .parse()
                .map_err(|e| anyhow::anyhow!("invalid app name: {e}"))?,
            target: "local"
                .parse()
                .map_err(|e| anyhow::anyhow!("invalid target id: {e}"))?,
            spec,
        };

        let res: r3v3rs3_api::platform::AppEntry = self.post_json("/api/apps", &req).await?;
        println!(
            "app store application '{app_id}' installed as '{name}' (id: {})",
            res.id
        );
        Ok(())
    }

    // --- Security operations ---

    pub async fn list_banned_ips(&self) -> anyhow::Result<()> {
        let entries: Vec<r3v3rs3_api::auth::BannedIpEntry> =
            self.get_json("/api/banned-ips").await?;
        println!(
            "{:<20} {:<10} {:<20} REMAINING",
            "IP ADDRESS", "FAILURES", "BANNED UNTIL"
        );
        println!("{:-<20} {:-<10} {:-<20} {:-<10}", "", "", "", "");
        for item in &entries {
            println!(
                "{:<20} {:<10} {:<20} {}s",
                item.ip, item.failures, item.banned_until, item.remaining_seconds
            );
        }
        Ok(())
    }

    pub async fn unban_ip(&self, ip: &str) -> anyhow::Result<()> {
        self.request(Method::DELETE, &format!("/api/banned-ips/{ip}"), None)
            .await?;
        println!("ip/{ip} unbanned");
        Ok(())
    }

    pub async fn list_quarantined_ips(&self) -> anyhow::Result<()> {
        let entries: Vec<r3v3rs3_api::quarantine::QuarantinedIpEntry> =
            self.get_json("/api/security/quarantine").await?;
        println!(
            "{:<18} {:<32} {:<12} {:<10}",
            "IP ADDRESS", "REASON", "VIOLATIONS", "REMAINING"
        );
        println!("{:-<18} {:-<32} {:-<12} {:-<10}", "", "", "", "");
        for item in &entries {
            println!(
                "{:<18} {:<32} {:<12} {}s",
                item.ip, item.reason, item.violations, item.remaining_seconds
            );
        }
        Ok(())
    }

    pub async fn quarantine_ip(
        &self,
        ip: &str,
        reason: Option<&str>,
        duration_seconds: Option<u64>,
    ) -> anyhow::Result<()> {
        let payload = r3v3rs3_api::quarantine::QuarantineIpRequest {
            ip: ip
                .parse()
                .map_err(|e| anyhow::anyhow!("invalid IP address: {e}"))?,
            reason: reason.map(str::to_string),
            duration_seconds,
        };
        let res = self
            .request(
                Method::POST,
                "/api/security/quarantine",
                Some(&serde_json::to_value(&payload)?),
            )
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
        let config: r3v3rs3_api::quarantine::AutoQuarantineConfig =
            self.get_json("/api/security/quarantine/config").await?;
        print_json(&serde_json::to_value(&config)?)?;
        Ok(())
    }

    pub async fn set_quarantine_config(
        &self,
        enabled: Option<bool>,
        max_violations: Option<u32>,
        window_seconds: Option<u64>,
        ban_duration_seconds: Option<u64>,
    ) -> anyhow::Result<()> {
        let mut config: r3v3rs3_api::quarantine::AutoQuarantineConfig = self
            .get_json("/api/security/quarantine/config")
            .await
            .unwrap_or_default();
        if let Some(en) = enabled {
            config.enabled = en;
        }
        if let Some(mv) = max_violations {
            config.max_violations = mv;
        }
        if let Some(ws) = window_seconds {
            config.window_seconds = ws;
        }
        if let Some(bd) = ban_duration_seconds {
            config.ban_duration_seconds = bd;
        }
        self.request(
            Method::PUT,
            "/api/security/quarantine/config",
            Some(&serde_json::to_value(&config)?),
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
        let conns: Vec<r3v3rs3_api::traffic::ActiveConnectionInfo> = self.get_json(&path).await?;
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

        for c in &conns {
            let proto = c.protocol.as_str();
            let uptime = format_duration_secs(c.uptime_secs);
            let sent = format_cli_bytes(c.bytes_sent);
            let recv = format_cli_bytes(c.bytes_received);
            let speed = format!("{}/s", format_cli_bytes(c.current_rate_bps));

            println!(
                "{:<10} {:<12} {:<20} {:<24} {:<12} {:<14} {:<14} {}",
                c.id, proto, c.proxy_name, c.client_addr, uptime, sent, recv, speed
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
        let status: r3v3rs3_api::geoip::GeoIpStatus =
            self.get_json("/api/security/geoip/status").await?;
        let size = status
            .file_size_bytes
            .map(|b| format!("{:.2} MB", b as f64 / (1024.0 * 1024.0)))
            .unwrap_or_else(|| "-".to_string());
        let build = status
            .build_epoch
            .map(|e| e.to_string())
            .unwrap_or_else(|| "-".to_string());

        println!(
            "{:<22} {:<10}",
            "GeoIP Status:",
            if status.loaded {
                "Loaded"
            } else {
                "Not Loaded"
            }
        );
        println!(
            "{:<22} {}",
            "Database Path:",
            status.database_path.as_deref().unwrap_or("-")
        );
        println!("{:<22} {}", "Database Size:", size);
        println!("{:<22} {}", "Build Epoch:", build);
        println!(
            "{:<22} {}",
            "Auto Update:",
            if status.auto_update {
                "Enabled"
            } else {
                "Disabled"
            }
        );
        println!(
            "{:<22} {} days",
            "Update Interval:", status.update_interval_days
        );
        println!(
            "{:<22} {}",
            "License Key:",
            if status.license_key_configured {
                "Configured"
            } else {
                "Not Configured"
            }
        );
        println!(
            "{:<22} {}",
            "Account ID:",
            status.account_id.as_deref().unwrap_or("-")
        );
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
        let status: r3v3rs3_api::geoip::GeoIpStatus =
            self.get_json("/api/security/geoip/status").await?;
        print_json(&serde_json::to_value(&status)?)?;
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
        let current: r3v3rs3_api::geoip::GeoIpStatus =
            self.get_json("/api/security/geoip/status").await?;

        let payload = r3v3rs3_api::geoip::UpdateGeoIpConfigRequest {
            account_id: account_id.or_else(|| current.account_id.clone()),
            license_key,
            database: database
                .or(current.database_path)
                .map(std::path::PathBuf::from),
            auto_update: auto_update.unwrap_or(current.auto_update),
            update_interval_days: interval_days.unwrap_or(current.update_interval_days),
        };

        let res = self
            .request(
                Method::PUT,
                "/api/security/geoip/config",
                Some(&serde_json::to_value(&payload)?),
            )
            .await?;
        println!("GeoIP configuration updated successfully.");
        print_json(&res)?;
        Ok(())
    }

    // --- Webhook operations ---

    pub async fn list_webhooks(&self) -> anyhow::Result<()> {
        let entries: Vec<r3v3rs3_api::webhook::OutboundWebhook> =
            self.get_json("/api/webhooks").await?;
        println!("{:<26} {:<20} {:<40} ENABLED", "ID", "NAME", "URL");
        println!("{:-<26} {:-<20} {:-<40} {:-<10}", "", "", "", "");
        for item in &entries {
            println!(
                "{:<26} {:<20} {:<40} {}",
                item.id, item.name, item.url, item.enabled
            );
        }
        Ok(())
    }

    pub async fn get_webhook(&self, id: &str) -> anyhow::Result<()> {
        let res: r3v3rs3_api::webhook::OutboundWebhook =
            self.get_json(&format!("/api/webhooks/{id}")).await?;
        let text = serde_json::to_string_pretty(&res)?;
        println!("{text}");
        Ok(())
    }

    pub async fn add_webhook(&self, payload: &str) -> anyhow::Result<()> {
        let req: r3v3rs3_api::webhook::CreateWebhookRequest = load_typed_payload(payload)?;
        let res: r3v3rs3_api::webhook::OutboundWebhook =
            self.post_json("/api/webhooks", &req).await?;
        println!("webhook added: id={}", res.id);
        Ok(())
    }

    pub async fn update_webhook(&self, id: &str, payload: &str) -> anyhow::Result<()> {
        let req: r3v3rs3_api::webhook::UpdateWebhookRequest = load_typed_payload(payload)?;
        let _res: r3v3rs3_api::webhook::OutboundWebhook =
            self.put_json(&format!("/api/webhooks/{id}"), &req).await?;
        println!("webhook/{id} updated");
        Ok(())
    }

    pub async fn delete_webhook(&self, id: &str) -> anyhow::Result<()> {
        self.request(Method::DELETE, &format!("/api/webhooks/{id}"), None)
            .await?;
        println!("webhook/{id} deleted");
        Ok(())
    }

    pub async fn test_webhook(&self, id: &str) -> anyhow::Result<()> {
        let req = r3v3rs3_api::webhook::TestWebhookRequest {
            webhook_id: Some(
                id.parse()
                    .map_err(|e| anyhow::anyhow!("invalid webhook id: {e}"))?,
            ),
            url: None,
            secret: None,
            event: None,
        };
        let res: r3v3rs3_api::webhook::TestWebhookResponse =
            self.post_json("/api/webhooks/test", &req).await?;
        println!(
            "webhook/{id} test: success={}, status={:?}, elapsed={}ms",
            res.success, res.status_code, res.elapsed_ms
        );
        println!("message: {}", res.message);
        Ok(())
    }

    // --- Container Registry operations ---

    pub async fn list_registries(&self) -> anyhow::Result<()> {
        let entries: Vec<r3v3rs3_api::platform::RegistryEntry> =
            self.get_json("/api/registries").await?;
        println!("{:<26} {:<32} USERNAME", "ID", "SERVER");
        println!("{:-<26} {:-<32} {:-<20}", "", "", "");
        for item in &entries {
            println!("{:<26} {:<32} {}", item.id, item.server, item.username);
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
            .request(
                Method::POST,
                "/api/registries",
                Some(&serde_json::to_value(&val)?),
            )
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
            .request(
                Method::PUT,
                &format!("/api/registries/{id}"),
                Some(&serde_json::to_value(&val)?),
            )
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
        let entries: Vec<r3v3rs3_api::git_connection::GitConnectionEntry> =
            self.get_json("/api/git/connections").await?;
        println!(
            "{:<26} {:<20} {:<12} {:<14} ACCOUNT",
            "ID", "NAME", "PROVIDER", "STATUS"
        );
        println!(
            "{:-<26} {:-<20} {:-<12} {:-<14} {:-<20}",
            "", "", "", "", ""
        );
        for item in &entries {
            let status = match item.status {
                r3v3rs3_api::git_connection::ConnectionStatus::NotConnected => "not_connected",
                r3v3rs3_api::git_connection::ConnectionStatus::Connected => "connected",
                r3v3rs3_api::git_connection::ConnectionStatus::Expired => "expired",
            };
            let account = item.account.as_deref().unwrap_or("-");
            println!(
                "{:<26} {:<20} {:<12} {:<14} {}",
                item.id,
                item.name,
                item.provider.as_str(),
                status,
                account
            );
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
            .request(
                Method::POST,
                "/api/git/connections",
                Some(&serde_json::to_value(&val)?),
            )
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
        let entries: Vec<r3v3rs3_api::auth::AccountInfo> = self.get_json("/api/accounts").await?;
        println!("{:<24} {:<12} PROXIES", "USERNAME", "ROLE");
        println!("{:-<24} {:-<12} {:-<20}", "", "", "");
        for item in &entries {
            let role = match item.role {
                r3v3rs3_api::auth::Role::Admin => "admin",
                r3v3rs3_api::auth::Role::Editor => "editor",
                r3v3rs3_api::auth::Role::Viewer => "viewer",
            };
            let proxies = item
                .proxies
                .as_ref()
                .map(|s| format!("{} proxies", s.len()))
                .unwrap_or_else(|| "all".to_string());
            println!("{:<24} {:<12} {}", item.username, role, proxies);
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
            .request(
                Method::POST,
                "/api/accounts",
                Some(&serde_json::to_value(&val)?),
            )
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
                Some(&serde_json::to_value(&val)?),
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
        let entries: Vec<r3v3rs3_api::auth::ApiTokenInfo> =
            self.get_json("/api/auth/tokens").await?;
        println!("{:<26} {:<24} SCOPE", "ID", "NAME");
        println!("{:-<26} {:-<24} {:-<10}", "", "", "");
        for item in &entries {
            let scope = match item.scope {
                r3v3rs3_api::auth::TokenScope::Read => "read",
                r3v3rs3_api::auth::TokenScope::Edit => "edit",
            };
            println!("{:<26} {:<24} {}", item.id, item.name, scope);
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
            .request(
                Method::POST,
                "/api/auth/tokens",
                Some(&serde_json::to_value(&val)?),
            )
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
        let entries: Vec<r3v3rs3_api::platform::AppScheduledCommand> = self
            .get_json(&format!("/api/apps/{app_id}/commands"))
            .await?;
        println!("{:<26} {:<18} {:<16} COMMAND", "ID", "NAME", "CRON");
        println!("{:-<26} {:-<18} {:-<16} {:-<30}", "", "", "", "");
        for item in &entries {
            println!(
                "{:<26} {:<18} {:<16} {}",
                item.id, item.name, item.schedule, item.command
            );
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
        let req = r3v3rs3_api::platform::ScheduledCommandRequest {
            name: name.unwrap_or(command).to_string(),
            schedule: cron.to_string(),
            command: command.to_string(),
            enabled: true,
        };
        let res: r3v3rs3_api::platform::AppScheduledCommand = self
            .post_json(&format!("/api/apps/{app_id}/commands"), &req)
            .await?;
        println!("scheduled command added to app/{app_id}: id={}", res.id);
        Ok(())
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
            let list: Vec<r3v3rs3_api::platform::AppScheduledCommand> = self
                .get_json(&format!("/api/apps/{app_id}/commands"))
                .await?;
            list.into_iter().find(|it| it.id.to_string() == id)
        } else {
            None
        };
        let body = build_cron_update_payload(existing.as_ref(), cron, command, name, payload)?;
        let _res: r3v3rs3_api::platform::AppScheduledCommand = self
            .put_json(&format!("/api/apps/{app_id}/commands/{id}"), &body)
            .await?;
        println!("scheduled command/{id} updated on app/{app_id}");
        Ok(())
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
        let res: r3v3rs3_api::platform::ScheduledCommandRunResult = self
            .post_json(
                &format!("/api/apps/{app_id}/commands/{id}/run"),
                &serde_json::json!({}),
            )
            .await?;
        println!(
            "scheduled command/{id} executed on app/{app_id}: success={}, status={}, duration={}ms",
            res.success, res.status, res.duration_ms
        );
        if !res.output.is_empty() {
            println!("output:\n{}", res.output);
        }
        Ok(())
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

    pub async fn upgrade_server(&self) -> anyhow::Result<()> {
        let res = self.request(Method::POST, "/api/upgrade", None).await?;
        println!("server zero-downtime upgrade initiated");
        print_json(&res)
    }

    pub async fn get_server_upgrade_status(&self) -> anyhow::Result<()> {
        let res = self
            .request(Method::GET, "/api/upgrade/status", None)
            .await?;
        println!("server upgrade status: {}", res);
        Ok(())
    }

    pub async fn platform_prune(&self) -> anyhow::Result<()> {
        let res = self
            .request(Method::POST, "/api/platform/prune/run", None)
            .await?;
        println!("platform disk hygiene prune completed");
        print_json(&res)
    }

    pub async fn get_prune_policy(&self) -> anyhow::Result<()> {
        let res: r3v3rs3_api::platform::PrunePolicy = self.get_json("/api/platform/prune").await?;
        print_json(&serde_json::to_value(&res)?)
    }

    pub async fn set_prune_policy(&self, args: &SetPrunePolicyArgs) -> anyhow::Result<()> {
        let mut cur: r3v3rs3_api::platform::PrunePolicy = self
            .get_json("/api/platform/prune")
            .await
            .unwrap_or_default();
        apply_prune_args(&mut cur, args);
        let res = self
            .request(
                Method::PUT,
                "/api/platform/prune",
                Some(&serde_json::to_value(&cur)?),
            )
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
        let res: r3v3rs3_api::platform::NodeAlertRules =
            self.get_json("/api/platform/node_alerts").await?;
        let text = serde_json::to_string_pretty(&res)?;
        println!("{text}");
        Ok(())
    }

    pub async fn set_node_alerts(
        &self,
        enabled: bool,
        min_disk_percent: Option<u8>,
        bandwidth_spike: Option<u64>,
        max_fd_percent: Option<u8>,
        max_time_wait: Option<u32>,
    ) -> anyhow::Result<()> {
        let rules = r3v3rs3_api::platform::NodeAlertRules {
            enabled,
            min_free_disk_percent: min_disk_percent,
            bandwidth_spike_bytes_per_sec: bandwidth_spike,
            max_fd_ratio_percent: max_fd_percent,
            max_time_wait_sockets: max_time_wait,
        };
        let res: r3v3rs3_api::platform::NodeAlertRules =
            self.put_json("/api/platform/node_alerts", &rules).await?;
        println!(
            "Node threshold alert rules updated successfully (enabled: {}).",
            res.enabled
        );
        Ok(())
    }

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
        let existing_list: Vec<r3v3rs3_api::port::PortEntry> = self.get_json("/api/ports").await?;
        for item in ports {
            self.apply_single_port(item, &existing_list).await?;
        }
        Ok(())
    }

    async fn apply_single_port(
        &self,
        item: &Value,
        existing: &[r3v3rs3_api::port::PortEntry],
    ) -> anyhow::Result<()> {
        let target_id = item.get("id").and_then(|v| v.as_str());
        let target_listen = item
            .get("listen")
            .or_else(|| item.get("port").and_then(|p| p.get("listen")))
            .and_then(|v| v.as_str());

        let matched = existing.iter().find(|e| {
            (target_id.is_some() && e.id.to_string() == target_id.unwrap_or_default())
                || (target_listen.is_some()
                    && e.port.listen.to_string() == target_listen.unwrap_or_default())
        });

        if let Some(matched_entry) = matched {
            let id = matched_entry.id.to_string();
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
        let existing_list: Vec<r3v3rs3_api::proxy::ProxyEntry> =
            self.get_json("/api/proxies").await?;
        for item in proxies {
            self.apply_single_proxy(item, &existing_list).await?;
        }
        Ok(())
    }

    async fn apply_single_proxy(
        &self,
        item: &Value,
        existing: &[r3v3rs3_api::proxy::ProxyEntry],
    ) -> anyhow::Result<()> {
        let target_id = item.get("id").and_then(|v| v.as_str());
        let target_name = item.get("name").and_then(|v| v.as_str());

        let matched = existing.iter().find(|e| {
            (target_id.is_some() && e.id.to_string() == target_id.unwrap_or_default())
                || (target_name.is_some()
                    && e.proxy.name.as_str() == target_name.unwrap_or_default())
        });

        if let Some(matched_entry) = matched {
            let id = matched_entry.id.to_string();
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
        let existing_list: Vec<r3v3rs3_api::platform::AppEntry> =
            self.get_json("/api/apps").await?;
        for item in apps {
            self.apply_single_app(item, &existing_list).await?;
        }
        Ok(())
    }

    async fn apply_single_app(
        &self,
        item: &Value,
        existing: &[r3v3rs3_api::platform::AppEntry],
    ) -> anyhow::Result<()> {
        let target_id = item.get("id").and_then(|v| v.as_str());
        let target_name = item
            .get("name")
            .or_else(|| item.get("spec").and_then(|s| s.get("name")))
            .and_then(|v| v.as_str());

        let matched = existing.iter().find(|e| {
            (target_id.is_some() && e.id.to_string() == target_id.unwrap_or_default())
                || (target_name.is_some() && e.name.as_str() == target_name.unwrap_or_default())
        });

        if let Some(matched_entry) = matched {
            let id = matched_entry.id.to_string();
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
        let list: Vec<r3v3rs3_api::security_headers::SecurityProfileInfo> =
            self.get_json("/api/security/headers/profiles").await?;
        println!("{:<16} {:<24} DESCRIPTION", "ID", "TITLE");
        println!("{}", "-".repeat(80));
        for item in &list {
            println!(
                "{:<16} {:<24} {}",
                item.id.title(),
                item.title,
                item.description
            );
        }
        Ok(())
    }

    pub async fn audit_proxy_security_headers(&self, id: &str) -> anyhow::Result<()> {
        let audit: r3v3rs3_api::security_headers::SecurityHeadersAudit = self
            .get_json(&format!("/api/proxies/{id}/security-headers"))
            .await?;

        let grade_colored = match audit.grade {
            r3v3rs3_api::security_headers::SecurityGrade::APlus
            | r3v3rs3_api::security_headers::SecurityGrade::A => {
                format!("\x1b[32m{}\x1b[0m", audit.grade.as_str())
            }
            r3v3rs3_api::security_headers::SecurityGrade::B => "\x1b[33mB\x1b[0m".to_string(),
            r3v3rs3_api::security_headers::SecurityGrade::C
            | r3v3rs3_api::security_headers::SecurityGrade::D => {
                format!("\x1b[33m{}\x1b[0m", audit.grade.as_str())
            }
            _ => "\x1b[31mF\x1b[0m".to_string(),
        };

        let detected = audit
            .detected_profile
            .map(|p| p.title())
            .unwrap_or("Custom");
        println!("Proxy Security Audit: {}", id);
        println!(
            "Score: {}/100 | Grade: {} | Matching Profile: {}",
            audit.score, grade_colored, detected
        );
        println!("{}", "-".repeat(85));
        println!(
            "{:<28} {:<12} {:<6} RISK / CURRENT VALUE",
            "HEADER", "STATUS", "PTS"
        );
        println!("{}", "-".repeat(85));

        for item in &audit.items {
            print_security_audit_item(item);
        }
        Ok(())
    }

    pub async fn apply_proxy_security_profile(
        &self,
        id: &str,
        profile_raw: &str,
    ) -> anyhow::Result<()> {
        let profile = match profile_raw.to_ascii_lowercase().as_str() {
            "strict" => r3v3rs3_api::security_headers::SecurityProfile::Strict,
            "balanced" => r3v3rs3_api::security_headers::SecurityProfile::Balanced,
            "api-only" | "api_only" | "api" => {
                r3v3rs3_api::security_headers::SecurityProfile::ApiOnly
            }
            "spa" | "spa-frontend" | "spa_frontend" => {
                r3v3rs3_api::security_headers::SecurityProfile::SpaFrontend
            }
            other => anyhow::bail!(
                "unknown profile: {other}. Expected strict, balanced, api-only, or spa"
            ),
        };

        let req = r3v3rs3_api::security_headers::ApplySecurityProfileRequest { profile };
        let res: r3v3rs3_api::security_headers::SecurityHeadersAudit = self
            .post_json(
                &format!("/api/proxies/{id}/security-headers/apply-profile"),
                &req,
            )
            .await?;

        println!(
            "\x1b[32mSuccessfully applied security profile '{}' to proxy {}.\x1b[0m",
            profile.title(),
            id
        );
        println!(
            "New Score: {}/100 (Grade: {})",
            res.score,
            res.grade.as_str()
        );
        Ok(())
    }

    pub async fn get_stream_priority(&self, id: &str) -> anyhow::Result<()> {
        let res: r3v3rs3_api::priority::StreamPriorityConfig = self
            .get_json(&format!("/api/proxies/{id}/priority"))
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
        let current: Option<r3v3rs3_api::priority::StreamPriorityConfig> = self
            .get_json(&format!("/api/proxies/{id}/priority"))
            .await
            .ok();

        let mut config = current.unwrap_or_default();
        config.enabled = enable;
        if let Some(inject) = inject_header {
            config.inject_header = inject;
        }
        if let Some(adaptive) = adaptive_concurrency {
            config.adaptive_concurrency = adaptive;
        }

        let body = r3v3rs3_api::priority::SetStreamPriorityRequest { config };
        let _res: r3v3rs3_api::priority::StreamPriorityConfig = self
            .put_json(&format!("/api/proxies/{id}/priority"), &body)
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

    pub async fn traffic_geo(&self) -> anyhow::Result<()> {
        let res: r3v3rs3_api::traffic::TrafficGeoDistribution =
            self.get_json("/api/traffic/geo").await?;
        println!("{:<10} {:<12} PERCENTAGE", "COUNTRY", "REQUESTS");
        println!("{:-<10} {:-<12} {:-<10}", "", "", "");
        for item in &res.top_countries {
            println!(
                "{:<10} {:<12} {:.1}%",
                item.country, item.count, item.percentage
            );
        }
        Ok(())
    }

    pub async fn traffic_replay(&self, id: u64) -> anyhow::Result<()> {
        let entry: r3v3rs3_api::traffic::AccessLogEntry =
            self.get_json(&format!("/api/traffic/{id}")).await?;
        let url = if entry.uri.starts_with("http://") || entry.uri.starts_with("https://") {
            entry.uri
        } else {
            format!("http://{}{}", entry.host, entry.uri)
        };
        let body = r3v3rs3_api::traffic::RequestReplayRequest {
            method: entry.method,
            url,
            headers: entry.request_headers,
            body: None,
            timeout_ms: Some(10000),
            allow_internal: false,
            insecure_skip_verify: false,
        };
        let res: r3v3rs3_api::traffic::RequestReplayResponse =
            self.post_json("/api/traffic/replay", &body).await?;
        println!(
            "traffic log entry {id} replayed (status: {}, latency: {:.2}ms)",
            res.status, res.latency_ms
        );
        if let Some(err) = res.error {
            println!("replay error: {err}");
        }
        Ok(())
    }

    pub async fn get_ct_status(&self) -> anyhow::Result<()> {
        let res: r3v3rs3_api::ct::CtMonitorStatus = self.get_json("/api/certs/ct/status").await?;
        println!("{}", serde_json::to_string_pretty(&res)?);
        Ok(())
    }

    pub async fn get_ct_config(&self) -> anyhow::Result<()> {
        let res: r3v3rs3_api::ct::CtMonitorConfig = self.get_json("/api/certs/ct/config").await?;
        println!("{}", serde_json::to_string_pretty(&res)?);
        Ok(())
    }

    pub async fn set_ct_config(&self, args: SetCtConfigArgs<'_>) -> anyhow::Result<()> {
        let mut cur: r3v3rs3_api::ct::CtMonitorConfig = self
            .get_json("/api/certs/ct/config")
            .await
            .unwrap_or_default();

        if let Some(en) = args.enable {
            cur.enabled = en;
        }
        if let Some(doms) = args.domains {
            cur.monitored_domains = doms
                .split(',')
                .map(|s| s.trim().to_string())
                .filter(|s| !s.is_empty())
                .collect();
        }
        if let Some(auto) = args.auto_discover {
            cur.auto_discover_domains = auto;
        }
        if let Some(issuers) = args.allowed_issuers {
            cur.allowed_issuers = issuers
                .split(',')
                .map(|s| s.trim().to_string())
                .filter(|s| !s.is_empty())
                .collect();
        }
        if let Some(alert) = args.alert_unknown {
            cur.alert_on_unknown_issuer = alert;
        }
        if let Some(int) = args.interval {
            cur.check_interval_secs = int;
        }
        if let Some(url) = args.api_url {
            cur.ct_api_url = Some(url.to_string());
        }

        let body = r3v3rs3_api::ct::UpdateCtConfigRequest { config: cur };
        let res: r3v3rs3_api::ct::CtMonitorConfig =
            self.put_json("/api/certs/ct/config", &body).await?;
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
        let res: Vec<r3v3rs3_api::ct::CtLogEntry> = self
            .get_json(&format!("/api/certs/ct/entries{query}"))
            .await?;
        println!("{}", serde_json::to_string_pretty(&res)?);
        Ok(())
    }

    pub async fn dismiss_ct_alert(&self, id: &str, status: Option<&str>) -> anyhow::Result<()> {
        let status = match status {
            Some(s) => match s.to_ascii_lowercase().as_str() {
                "investigating" => Some(r3v3rs3_api::ct::CtAlertStatus::Investigating),
                "dismissed" => Some(r3v3rs3_api::ct::CtAlertStatus::Dismissed),
                "confirmed_rogue" | "confirmed-rogue" | "rogue" => {
                    Some(r3v3rs3_api::ct::CtAlertStatus::ConfirmedRogue)
                }
                other => anyhow::bail!("unknown status: {other}"),
            },
            None => None,
        };
        let body = r3v3rs3_api::ct::DismissCtAlertRequest {
            id: id.to_string(),
            status,
        };
        let _res: bool = self.post_json("/api/certs/ct/dismiss", &body).await?;
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

fn print_security_audit_item(item: &r3v3rs3_api::security_headers::HeaderAuditItem) {
    let status_tag = match item.status {
        r3v3rs3_api::security_headers::HeaderAuditStatus::Present => "\x1b[32m[SECURED]\x1b[0m",
        r3v3rs3_api::security_headers::HeaderAuditStatus::Weak => "\x1b[33m[ WEAK  ]\x1b[0m",
        r3v3rs3_api::security_headers::HeaderAuditStatus::Missing => "\x1b[31m[MISSING]\x1b[0m",
    };

    println!(
        "{:<28} {:<21} {:<6} {}",
        item.header_name, status_tag, item.earned_score, item.risk
    );
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
    existing: Option<&r3v3rs3_api::platform::AppScheduledCommand>,
    cron: Option<&str>,
    command: Option<&str>,
    name: Option<&str>,
    payload: Option<&str>,
) -> anyhow::Result<r3v3rs3_api::platform::ScheduledCommandRequest> {
    if let Some(p) = payload {
        return load_typed_payload(p);
    }
    let cur = existing.context("scheduled command not found on application")?;
    Ok(r3v3rs3_api::platform::ScheduledCommandRequest {
        name: name.unwrap_or(&cur.name).to_string(),
        schedule: cron.unwrap_or(&cur.schedule).to_string(),
        command: command.unwrap_or(&cur.command).to_string(),
        enabled: cur.enabled,
    })
}

pub fn build_pitr_restore_payload(
    target_time: u64,
    new_name: Option<String>,
    host_port: Option<u16>,
) -> anyhow::Result<r3v3rs3_api::platform::PitrRestoreRequest> {
    let new_name = match new_name {
        Some(n) => Some(
            n.parse()
                .map_err(|e| anyhow::anyhow!("invalid database name: {e}"))?,
        ),
        None => None,
    };
    Ok(r3v3rs3_api::platform::PitrRestoreRequest {
        target_time,
        new_name,
        host_port,
    })
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
        assert_eq!(val.server, "ghcr.io");
        assert_eq!(val.username, "octocat");
        assert_eq!(val.password.as_deref(), Some("token123"));

        let custom = build_registry_payload(
            None,
            None,
            None,
            Some(r#"{"server": "docker.io", "username": "user"}"#.to_string()),
        )
        .unwrap();
        assert_eq!(custom.server, "docker.io");
        assert_eq!(custom.username, "user");

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
        assert_eq!(val.name.as_str(), "my-gitlab");
        assert_eq!(
            val.provider,
            r3v3rs3_api::git_connection::GitProvider::Gitlab
        );
        assert_eq!(val.client_id, "client123");
        assert_eq!(val.client_secret.as_deref(), Some("secret456"));

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
        assert_eq!(acc.username, "alice");
        assert_eq!(acc.password, "pw123");
        assert_eq!(acc.role, r3v3rs3_api::auth::Role::Admin);

        let update = build_account_update_payload(
            Some("newpw".to_string()),
            Some("viewer".to_string()),
            None,
        )
        .unwrap();
        assert_eq!(update.password.as_deref(), Some("newpw"));
        assert_eq!(update.role, r3v3rs3_api::auth::Role::Viewer);

        let tok = build_token_create_payload(Some("my-token".to_string()), "edit", None).unwrap();
        assert_eq!(tok.name, "my-token");
        assert_eq!(tok.scope, r3v3rs3_api::auth::TokenScope::Edit);

        assert!(build_account_create_payload(None, None, "editor", None).is_err());
        assert!(build_token_create_payload(None, "read", None).is_err());
    }

    #[test]
    fn build_cron_update_payload_merges_existing() {
        let existing = r3v3rs3_api::platform::AppScheduledCommand {
            id: "cmd-1".parse().unwrap(),
            app_id: "app-1".parse().unwrap(),
            name: "backup".to_string(),
            schedule: "0 0 * * *".to_string(),
            command: "run-backup.sh".to_string(),
            enabled: true,
            last_run_at: None,
            last_status: None,
            last_output: None,
            created_at: 0,
            updated_at: 0,
        };

        let updated = build_cron_update_payload(
            Some(&existing),
            Some("*/15 * * * *"),
            None,
            Some("quick backup"),
            None,
        )
        .unwrap();

        assert_eq!(updated.name, "quick backup");
        assert_eq!(updated.schedule, "*/15 * * * *");
        assert_eq!(updated.command, "run-backup.sh");
        assert!(updated.enabled);

        assert!(build_cron_update_payload(None, None, None, None, None).is_err());
    }

    #[test]
    fn build_pitr_restore_payload_constructs_valid_json() {
        let val =
            build_pitr_restore_payload(1700000000000, Some("db-restored".to_string()), Some(5433))
                .unwrap();
        assert_eq!(val.target_time, 1700000000000u64);
        assert_eq!(val.new_name.unwrap().as_str(), "db-restored");
        assert_eq!(val.host_port, Some(5433));

        let val_minimal = build_pitr_restore_payload(1700000000000, None, None).unwrap();
        assert_eq!(val_minimal.target_time, 1700000000000u64);
        assert!(val_minimal.new_name.is_none());
    }
}
