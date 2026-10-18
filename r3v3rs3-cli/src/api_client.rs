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

    pub async fn purge_cache(&self, id: &str) -> anyhow::Result<()> {
        self.request(Method::DELETE, &format!("/api/proxies/{id}/cache"), None)
            .await?;
        println!("proxy/{id} cache purged");
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
}
