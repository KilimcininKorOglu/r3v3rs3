use anyhow::{Context, bail};
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

    pub async fn proxy_status(&self, id: &str) -> anyhow::Result<()> {
        let res = self
            .request(Method::GET, &format!("/api/proxies/{id}/status"), None)
            .await?;
        print_json(&res)
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

    pub async fn delete_acl(&self, id: &str) -> anyhow::Result<()> {
        self.request(Method::DELETE, &format!("/api/access-lists/{id}"), None)
            .await?;
        println!("access-list/{id} deleted");
        Ok(())
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
        let mut map = serde_json::Map::new();
        for pair in pairs {
            if let Some((k, v)) = pair.split_once('=') {
                map.insert(k.to_string(), Value::String(v.to_string()));
            } else {
                bail!("invalid env pair '{pair}', expected KEY=VALUE format");
            }
        }
        let body = Value::Object(map);
        self.request(Method::PUT, &format!("/api/apps/{id}/env"), Some(&body))
            .await?;
        println!("app/{id} environment variables updated");
        Ok(())
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
            "cron": cron,
            "command": command,
            "name": name.unwrap_or(command),
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
            .request(Method::POST, "/api/platform/prune", None)
            .await?;
        println!("platform disk hygiene prune completed");
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
}
