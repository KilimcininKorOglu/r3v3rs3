//! Data types and parser for Docker Compose service dependency graphs and architecture topology.

use super::container::ContainerHealth;
use super::platform::ComposeServiceStatus;
use serde_derive::{Deserialize, Serialize};
use std::collections::BTreeSet;
use utoipa::ToSchema;

/// The kind of node in a Compose architecture topology graph.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "snake_case")]
pub enum ComposeNodeKind {
    /// A Docker Compose service container.
    Service,
    /// A shared Docker network.
    Network,
    /// A persistent Docker volume or host bind-mount.
    Volume,
}

/// Detailed metadata for a service container node.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, ToSchema, Default)]
pub struct ComposeServiceDetails {
    /// The container image, e.g. "postgres:17-alpine" or "Dockerfile (build)".
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub image: Option<String>,
    /// Port mappings exposed by the service, e.g. ["80:80", "5432:5432"].
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub ports: Vec<String>,
    /// Whether this service is the primary entrypoint routed by r3v3rs3.
    #[serde(default)]
    pub is_entrypoint: bool,
    /// Container runtime state, e.g. "running", "exited", "restarting".
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub state: Option<String>,
    /// Container healthcheck status, if available.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub health: Option<ContainerHealth>,
    /// Container name if the service is deployed and running.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub container: Option<String>,
    /// Count of configured environment variables.
    #[serde(default)]
    pub env_count: usize,
    /// Restart policy, e.g. "unless-stopped", "always".
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub restart: Option<String>,
}

/// Detailed metadata for a volume node.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, ToSchema, Default)]
pub struct ComposeVolumeDetails {
    /// Custom volume driver name if configured.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub driver: Option<String>,
    /// Whether this volume is declared external.
    #[serde(default)]
    pub external: bool,
    /// Whether this represents a host bind-mount rather than a named Docker volume.
    #[serde(default)]
    pub is_bind_mount: bool,
    /// Host source path or volume name.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub source: Option<String>,
}

/// Detailed metadata for a network node.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, ToSchema, Default)]
pub struct ComposeNetworkDetails {
    /// Network driver, e.g. "bridge", "overlay".
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub driver: Option<String>,
    /// Whether this network is declared external.
    #[serde(default)]
    pub external: bool,
}

/// One node in the architecture topology graph.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
pub struct ComposeNode {
    /// Unique identifier for the node (e.g. "service:web", "network:frontend", "volume:db_data").
    pub id: String,
    /// Human-readable label / name (e.g. "web", "frontend", "db_data").
    pub name: String,
    /// The node kind.
    pub kind: ComposeNodeKind,
    /// Service details if this node is a service.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub service: Option<ComposeServiceDetails>,
    /// Volume details if this node is a volume.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub volume: Option<ComposeVolumeDetails>,
    /// Network details if this node is a network.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub network: Option<ComposeNetworkDetails>,
}

/// The kind of dependency or relationship between two topology nodes.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum ComposeEdgeKind {
    /// Service depends on another service, optionally with a health condition.
    DependsOn {
        #[serde(default, skip_serializing_if = "Option::is_none")]
        condition: Option<String>,
    },
    /// Service is attached to a network.
    Network,
    /// Service mounts a volume.
    Volume {
        #[serde(default, skip_serializing_if = "Option::is_none")]
        target_path: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        mode: Option<String>,
    },
    /// Legacy container link.
    Link,
}

/// A directed edge connecting two topology nodes.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
pub struct ComposeEdge {
    /// The source node ID (usually the dependent service).
    pub source: String,
    /// The target node ID (e.g. required service, attached network, or mounted volume).
    pub target: String,
    /// The relationship kind.
    pub kind: ComposeEdgeKind,
    /// Optional human-readable annotation label.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub label: Option<String>,
}

/// Summary metrics for the Compose topology graph.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
pub struct ComposeTopologySummary {
    /// Total number of services.
    pub services_count: usize,
    /// Total number of networks.
    pub networks_count: usize,
    /// Total number of volumes.
    pub volumes_count: usize,
    /// Number of services currently in running state.
    pub running_services: usize,
    /// Number of services currently reporting healthy status.
    pub healthy_services: usize,
}

/// Complete architectural topology graph for a Docker Compose application.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
pub struct ComposeTopology {
    /// All nodes (services, networks, volumes) in the topology.
    pub nodes: Vec<ComposeNode>,
    /// All directed edges (dependencies, network memberships, volume mounts).
    pub edges: Vec<ComposeEdge>,
    /// High-level topology summary statistics.
    pub summary: ComposeTopologySummary,
}

/// Parses Docker Compose YAML and constructs an architecture topology graph.
pub fn parse_compose_topology(
    yaml: &str,
    statuses: Option<&[ComposeServiceStatus]>,
    entrypoint_service: Option<&str>,
) -> Result<ComposeTopology, String> {
    if yaml.trim().is_empty() {
        return Ok(ComposeTopology::default());
    }

    let val: serde_yaml::Value = serde_yaml::from_str(yaml).map_err(|err| {
        let line = err.location().map(|l| l.line()).unwrap_or(1);
        format!("Line {line}: {err}")
    })?;

    let Some(map) = val.as_mapping() else {
        return Err("compose YAML must be a mapping of top-level keys".into());
    };

    let mut nodes = Vec::new();
    let mut edges = Vec::new();
    let mut declared_networks = BTreeSet::new();
    let mut declared_volumes = BTreeSet::new();

    parse_top_level_networks(map, &mut nodes, &mut declared_networks);
    parse_top_level_volumes(map, &mut nodes, &mut declared_volumes);

    if let Some(services_map) = map
        .get(serde_yaml::Value::String("services".into()))
        .and_then(|s| s.as_mapping())
    {
        for (k, v) in services_map {
            let Some(svc_name) = k.as_str() else { continue };
            let svc_map = v.as_mapping();
            let details = build_service_details(svc_name, svc_map, statuses, entrypoint_service);
            nodes.push(ComposeNode {
                id: format!("service:{svc_name}"),
                name: svc_name.to_string(),
                kind: ComposeNodeKind::Service,
                service: Some(details),
                volume: None,
                network: None,
            });

            if let Some(smap) = svc_map {
                collect_service_edges(
                    svc_name,
                    smap,
                    &mut nodes,
                    &mut edges,
                    &mut declared_networks,
                    &mut declared_volumes,
                );
            }
        }
    }

    nodes.sort_by(|a, b| a.kind.cmp(&b.kind).then_with(|| a.name.cmp(&b.name)));
    edges.sort_by(|a, b| {
        a.source
            .cmp(&b.source)
            .then_with(|| a.target.cmp(&b.target))
    });

    let summary = calculate_summary(&nodes);

    Ok(ComposeTopology {
        nodes,
        edges,
        summary,
    })
}

fn parse_top_level_networks(
    map: &serde_yaml::Mapping,
    nodes: &mut Vec<ComposeNode>,
    declared: &mut BTreeSet<String>,
) {
    let Some(nets) = map
        .get(serde_yaml::Value::String("networks".into()))
        .and_then(|n| n.as_mapping())
    else {
        return;
    };
    for (k, v) in nets {
        let Some(net_name) = k.as_str() else { continue };
        let net_id = format!("network:{net_name}");
        declared.insert(net_id.clone());
        let net_map = v.as_mapping();
        let driver = net_map
            .and_then(|m| m.get(serde_yaml::Value::String("driver".into())))
            .and_then(|d| d.as_str())
            .map(|s| s.to_string());
        let external = net_map
            .and_then(|m| m.get(serde_yaml::Value::String("external".into())))
            .and_then(|e| e.as_bool())
            .unwrap_or(false);

        nodes.push(ComposeNode {
            id: net_id,
            name: net_name.to_string(),
            kind: ComposeNodeKind::Network,
            service: None,
            volume: None,
            network: Some(ComposeNetworkDetails { driver, external }),
        });
    }
}

fn parse_top_level_volumes(
    map: &serde_yaml::Mapping,
    nodes: &mut Vec<ComposeNode>,
    declared: &mut BTreeSet<String>,
) {
    let Some(vols) = map
        .get(serde_yaml::Value::String("volumes".into()))
        .and_then(|v| v.as_mapping())
    else {
        return;
    };
    for (k, v) in vols {
        let Some(vol_name) = k.as_str() else { continue };
        let vol_id = format!("volume:{vol_name}");
        declared.insert(vol_id.clone());
        let vol_map = v.as_mapping();
        let driver = vol_map
            .and_then(|m| m.get(serde_yaml::Value::String("driver".into())))
            .and_then(|d| d.as_str())
            .map(|s| s.to_string());
        let external = vol_map
            .and_then(|m| m.get(serde_yaml::Value::String("external".into())))
            .and_then(|e| e.as_bool())
            .unwrap_or(false);

        nodes.push(ComposeNode {
            id: vol_id,
            name: vol_name.to_string(),
            kind: ComposeNodeKind::Volume,
            service: None,
            volume: Some(ComposeVolumeDetails {
                driver,
                external,
                is_bind_mount: false,
                source: Some(vol_name.to_string()),
            }),
            network: None,
        });
    }
}

fn build_service_details(
    name: &str,
    map: Option<&serde_yaml::Mapping>,
    statuses: Option<&[ComposeServiceStatus]>,
    entrypoint_service: Option<&str>,
) -> ComposeServiceDetails {
    let mut details = ComposeServiceDetails::default();
    if let Some(m) = map {
        extract_image_and_build(m, &mut details);
        extract_ports(m, &mut details);
        extract_env_and_restart(m, &mut details);
    }

    if let Some(entry) = entrypoint_service {
        details.is_entrypoint = entry == name;
    }

    if let Some(st_list) = statuses
        && let Some(st) = st_list.iter().find(|s| s.service == name)
    {
        details.state = Some(st.state.clone());
        details.health = st.health;
        details.container = Some(st.container.clone());
    }

    details
}

fn extract_image_and_build(map: &serde_yaml::Mapping, details: &mut ComposeServiceDetails) {
    if let Some(img) = map
        .get(serde_yaml::Value::String("image".into()))
        .and_then(|i| i.as_str())
    {
        details.image = Some(img.to_string());
    } else if let Some(b) = map.get(serde_yaml::Value::String("build".into())) {
        if let Some(b_str) = b.as_str() {
            details.image = Some(format!("build: {b_str}"));
        } else if let Some(b_map) = b.as_mapping() {
            let ctx = b_map
                .get(serde_yaml::Value::String("context".into()))
                .and_then(|c| c.as_str())
                .unwrap_or(".");
            details.image = Some(format!("build: {ctx}"));
        }
    }
}

fn extract_ports(map: &serde_yaml::Mapping, details: &mut ComposeServiceDetails) {
    let Some(ports) = map
        .get(serde_yaml::Value::String("ports".into()))
        .and_then(|p| p.as_sequence())
    else {
        return;
    };
    for p in ports {
        if let Some(p_str) = p.as_str() {
            details.ports.push(p_str.to_string());
        } else if let Some(p_num) = p.as_u64() {
            details.ports.push(p_num.to_string());
        } else if let Some(p_map) = p.as_mapping()
            && let Some(target) = p_map
                .get(serde_yaml::Value::String("target".into()))
                .and_then(|t| t.as_u64())
        {
            let pub_port = p_map
                .get(serde_yaml::Value::String("published".into()))
                .and_then(|pb| pb.as_u64());
            if let Some(published) = pub_port {
                details.ports.push(format!("{published}:{target}"));
            } else {
                details.ports.push(target.to_string());
            }
        }
    }
}

fn extract_env_and_restart(map: &serde_yaml::Mapping, details: &mut ComposeServiceDetails) {
    if let Some(env) = map.get(serde_yaml::Value::String("environment".into())) {
        if let Some(seq) = env.as_sequence() {
            details.env_count = seq.len();
        } else if let Some(m) = env.as_mapping() {
            details.env_count = m.len();
        }
    }
    if let Some(r) = map
        .get(serde_yaml::Value::String("restart".into()))
        .and_then(|r| r.as_str())
    {
        details.restart = Some(r.to_string());
    }
}

fn collect_service_edges(
    svc_name: &str,
    map: &serde_yaml::Mapping,
    nodes: &mut Vec<ComposeNode>,
    edges: &mut Vec<ComposeEdge>,
    declared_networks: &mut BTreeSet<String>,
    declared_volumes: &mut BTreeSet<String>,
) {
    let svc_id = format!("service:{svc_name}");
    collect_depends_on(&svc_id, map, edges);
    collect_service_networks(&svc_id, map, nodes, edges, declared_networks);
    collect_service_volumes(&svc_id, map, nodes, edges, declared_volumes);
    collect_service_links(&svc_id, map, edges);
}

fn collect_depends_on(svc_id: &str, map: &serde_yaml::Mapping, edges: &mut Vec<ComposeEdge>) {
    let Some(deps) = map.get(serde_yaml::Value::String("depends_on".into())) else {
        return;
    };
    if let Some(seq) = deps.as_sequence() {
        for item in seq {
            if let Some(target_svc) = item.as_str() {
                edges.push(ComposeEdge {
                    source: svc_id.to_string(),
                    target: format!("service:{target_svc}"),
                    kind: ComposeEdgeKind::DependsOn { condition: None },
                    label: None,
                });
            }
        }
    } else if let Some(dep_map) = deps.as_mapping() {
        for (k, v) in dep_map {
            let Some(target_svc) = k.as_str() else {
                continue;
            };
            let condition = v
                .as_mapping()
                .and_then(|m| m.get(serde_yaml::Value::String("condition".into())))
                .and_then(|c| c.as_str())
                .map(|s| s.to_string());

            let label = condition
                .as_deref()
                .map(|c| match c {
                    "service_healthy" => "healthy",
                    "service_started" => "started",
                    "service_completed_successfully" => "completed",
                    other => other,
                })
                .map(|s| s.to_string());

            edges.push(ComposeEdge {
                source: svc_id.to_string(),
                target: format!("service:{target_svc}"),
                kind: ComposeEdgeKind::DependsOn { condition },
                label,
            });
        }
    }
}

fn collect_service_networks(
    svc_id: &str,
    map: &serde_yaml::Mapping,
    nodes: &mut Vec<ComposeNode>,
    edges: &mut Vec<ComposeEdge>,
    declared_networks: &mut BTreeSet<String>,
) {
    let Some(nets) = map.get(serde_yaml::Value::String("networks".into())) else {
        return;
    };
    let mut names = Vec::new();
    if let Some(seq) = nets.as_sequence() {
        for n in seq {
            if let Some(name) = n.as_str() {
                names.push(name.to_string());
            }
        }
    } else if let Some(m) = nets.as_mapping() {
        for (k, _) in m {
            if let Some(name) = k.as_str() {
                names.push(name.to_string());
            }
        }
    }

    for net_name in names {
        let net_id = format!("network:{net_name}");
        if declared_networks.insert(net_id.clone()) {
            nodes.push(ComposeNode {
                id: net_id.clone(),
                name: net_name.clone(),
                kind: ComposeNodeKind::Network,
                service: None,
                volume: None,
                network: Some(ComposeNetworkDetails::default()),
            });
        }
        edges.push(ComposeEdge {
            source: svc_id.to_string(),
            target: net_id,
            kind: ComposeEdgeKind::Network,
            label: None,
        });
    }
}

fn collect_service_volumes(
    svc_id: &str,
    map: &serde_yaml::Mapping,
    nodes: &mut Vec<ComposeNode>,
    edges: &mut Vec<ComposeEdge>,
    declared_volumes: &mut BTreeSet<String>,
) {
    let Some(vols) = map
        .get(serde_yaml::Value::String("volumes".into()))
        .and_then(|v| v.as_sequence())
    else {
        return;
    };
    for item in vols {
        if let Some(vol_str) = item.as_str() {
            parse_string_volume(svc_id, vol_str, nodes, edges, declared_volumes);
        } else if let Some(vol_map) = item.as_mapping() {
            parse_mapping_volume(svc_id, vol_map, nodes, edges, declared_volumes);
        }
    }
}

fn parse_string_volume(
    svc_id: &str,
    vol_str: &str,
    nodes: &mut Vec<ComposeNode>,
    edges: &mut Vec<ComposeEdge>,
    declared_volumes: &mut BTreeSet<String>,
) {
    let parts: Vec<&str> = vol_str.split(':').collect();
    let source = parts[0].trim();
    if source.is_empty() {
        return;
    }
    let target = parts.get(1).map(|t| t.trim().to_string());
    let mode = parts.get(2).map(|m| m.trim().to_string());

    let is_bind = source.starts_with('.') || source.starts_with('/') || source.starts_with('~');
    let vol_id = if is_bind {
        format!("volume:bind:{source}")
    } else {
        format!("volume:{source}")
    };

    if declared_volumes.insert(vol_id.clone()) {
        nodes.push(ComposeNode {
            id: vol_id.clone(),
            name: source.to_string(),
            kind: ComposeNodeKind::Volume,
            service: None,
            volume: Some(ComposeVolumeDetails {
                driver: None,
                external: false,
                is_bind_mount: is_bind,
                source: Some(source.to_string()),
            }),
            network: None,
        });
    }

    edges.push(ComposeEdge {
        source: svc_id.to_string(),
        target: vol_id,
        kind: ComposeEdgeKind::Volume {
            target_path: target,
            mode: mode.clone(),
        },
        label: mode,
    });
}

fn parse_mapping_volume(
    svc_id: &str,
    vol_map: &serde_yaml::Mapping,
    nodes: &mut Vec<ComposeNode>,
    edges: &mut Vec<ComposeEdge>,
    declared_volumes: &mut BTreeSet<String>,
) {
    let source = vol_map
        .get(serde_yaml::Value::String("source".into()))
        .and_then(|s| s.as_str())
        .unwrap_or_default();
    if source.is_empty() {
        return;
    }
    let target = vol_map
        .get(serde_yaml::Value::String("target".into()))
        .and_then(|t| t.as_str())
        .map(|s| s.to_string());
    let read_only = vol_map
        .get(serde_yaml::Value::String("read_only".into()))
        .and_then(|ro| ro.as_bool())
        .unwrap_or(false);
    let mode = if read_only {
        Some("ro".to_string())
    } else {
        None
    };

    let is_bind = source.starts_with('.') || source.starts_with('/') || source.starts_with('~');
    let vol_id = if is_bind {
        format!("volume:bind:{source}")
    } else {
        format!("volume:{source}")
    };

    if declared_volumes.insert(vol_id.clone()) {
        nodes.push(ComposeNode {
            id: vol_id.clone(),
            name: source.to_string(),
            kind: ComposeNodeKind::Volume,
            service: None,
            volume: Some(ComposeVolumeDetails {
                driver: None,
                external: false,
                is_bind_mount: is_bind,
                source: Some(source.to_string()),
            }),
            network: None,
        });
    }

    edges.push(ComposeEdge {
        source: svc_id.to_string(),
        target: vol_id,
        kind: ComposeEdgeKind::Volume {
            target_path: target,
            mode: mode.clone(),
        },
        label: mode,
    });
}

fn collect_service_links(svc_id: &str, map: &serde_yaml::Mapping, edges: &mut Vec<ComposeEdge>) {
    let Some(links) = map
        .get(serde_yaml::Value::String("links".into()))
        .and_then(|l| l.as_sequence())
    else {
        return;
    };
    for l in links {
        if let Some(link_str) = l.as_str() {
            let target_svc = link_str.split(':').next().unwrap_or(link_str).trim();
            if !target_svc.is_empty() {
                edges.push(ComposeEdge {
                    source: svc_id.to_string(),
                    target: format!("service:{target_svc}"),
                    kind: ComposeEdgeKind::Link,
                    label: Some("link".to_string()),
                });
            }
        }
    }
}

fn calculate_summary(nodes: &[ComposeNode]) -> ComposeTopologySummary {
    let mut summary = ComposeTopologySummary::default();
    for node in nodes {
        match node.kind {
            ComposeNodeKind::Service => {
                summary.services_count += 1;
                if let Some(svc) = &node.service {
                    if svc.state.as_deref() == Some("running") {
                        summary.running_services += 1;
                    }
                    if svc.health == Some(ContainerHealth::Healthy) {
                        summary.healthy_services += 1;
                    }
                }
            }
            ComposeNodeKind::Network => {
                summary.networks_count += 1;
            }
            ComposeNodeKind::Volume => {
                summary.volumes_count += 1;
            }
        }
    }
    summary
}

#[cfg(test)]
mod tests {
    use super::*;

    const TEST_YAML: &str = r#"
services:
  web:
    image: nginx:alpine
    ports:
      - "80:80"
    depends_on:
      api:
        condition: service_healthy
    networks:
      - frontend
      - backend
    volumes:
      - ./nginx.conf:/etc/nginx/nginx.conf:ro

  api:
    image: my-app:latest
    ports:
      - "3000:3000"
    environment:
      - DATABASE_URL=postgres://db:5432/app
      - REDIS_URL=redis://cache:6379
    depends_on:
      - db
      - cache
    networks:
      - backend
    volumes:
      - app_uploads:/app/uploads

  db:
    image: postgres:17-alpine
    environment:
      - POSTGRES_PASSWORD=secret
    volumes:
      - db_data:/var/lib/postgresql/data
    networks:
      - backend

  cache:
    image: redis:alpine
    networks:
      - backend

networks:
  frontend:
    driver: bridge
  backend:
    driver: bridge

volumes:
  db_data:
  app_uploads:
"#;

    #[test]
    fn parses_full_compose_topology() {
        let statuses = vec![
            ComposeServiceStatus {
                service: "web".to_string(),
                container: "project_web_1".to_string(),
                state: "running".to_string(),
                health: Some(ContainerHealth::Healthy),
                exit_code: None,
            },
            ComposeServiceStatus {
                service: "api".to_string(),
                container: "project_api_1".to_string(),
                state: "running".to_string(),
                health: Some(ContainerHealth::Healthy),
                exit_code: None,
            },
            ComposeServiceStatus {
                service: "db".to_string(),
                container: "project_db_1".to_string(),
                state: "running".to_string(),
                health: None,
                exit_code: None,
            },
            ComposeServiceStatus {
                service: "cache".to_string(),
                container: "project_cache_1".to_string(),
                state: "exited".to_string(),
                health: None,
                exit_code: Some(0),
            },
        ];

        let topo = parse_compose_topology(TEST_YAML, Some(&statuses), Some("web")).unwrap();

        assert_eq!(topo.summary.services_count, 4);
        assert_eq!(topo.summary.networks_count, 2);
        assert_eq!(topo.summary.volumes_count, 3);
        assert_eq!(topo.summary.running_services, 3);
        assert_eq!(topo.summary.healthy_services, 2);

        // Find web service
        let web = topo.nodes.iter().find(|n| n.name == "web").unwrap();
        let svc = web.service.as_ref().unwrap();
        assert!(svc.is_entrypoint);
        assert_eq!(svc.state.as_deref(), Some("running"));
        assert_eq!(svc.health, Some(ContainerHealth::Healthy));
        assert_eq!(svc.ports, vec!["80:80"]);

        // Check depends_on edge with condition
        let web_to_api = topo
            .edges
            .iter()
            .find(|e| e.source == "service:web" && e.target == "service:api")
            .unwrap();
        assert_eq!(
            web_to_api.kind,
            ComposeEdgeKind::DependsOn {
                condition: Some("service_healthy".to_string())
            }
        );
        assert_eq!(web_to_api.label.as_deref(), Some("healthy"));

        // Check volumes
        assert!(topo.nodes.iter().any(|n| n.name == "db_data"));
        assert!(topo.nodes.iter().any(|n| n.name == "app_uploads"));
        assert!(topo.nodes.iter().any(|n| n.name == "./nginx.conf"));

        // Check networks
        assert!(topo.nodes.iter().any(|n| n.name == "frontend"));
        assert!(topo.nodes.iter().any(|n| n.name == "backend"));
    }

    #[test]
    fn parses_empty_compose_yaml() {
        let topo = parse_compose_topology("", None, None).unwrap();
        assert!(topo.nodes.is_empty());
        assert!(topo.edges.is_empty());
        assert_eq!(topo.summary.services_count, 0);
    }
}
