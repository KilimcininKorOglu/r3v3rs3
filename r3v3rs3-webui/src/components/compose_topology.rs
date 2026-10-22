//! Interactive SVG architecture visualizer and service dependency graph for Docker Compose.

use r3v3rs3_api::container::ContainerHealth;
use r3v3rs3_api::i18n::Locale;
use r3v3rs3_api::platform::{
    ComposeEdge, ComposeEdgeKind, ComposeNode, ComposeNodeKind, ComposeTopology,
};
use std::collections::{BTreeMap, HashMap};
use yew::prelude::*;

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum TopologyFilter {
    All,
    Services,
    Networks,
    Volumes,
}

#[derive(Clone, PartialEq)]
struct NodePosition {
    x: f64,
    y: f64,
    width: f64,
    height: f64,
}

#[derive(Properties, PartialEq)]
pub struct ComposeTopologyGraphProps {
    pub topology: ComposeTopology,
    pub locale: Locale,
}

#[function_component(ComposeTopologyGraph)]
pub fn compose_topology_graph(props: &ComposeTopologyGraphProps) -> Html {
    let locale = props.locale;
    let topology = &props.topology;
    let filter = use_state(|| TopologyFilter::All);
    let selected_node_id = use_state(|| Option::<String>::None);
    let zoom_level = use_state(|| 1.0f64);

    let filtered_nodes: Vec<&ComposeNode> = topology
        .nodes
        .iter()
        .filter(|n| match *filter {
            TopologyFilter::All => true,
            TopologyFilter::Services => n.kind == ComposeNodeKind::Service,
            TopologyFilter::Networks => n.kind == ComposeNodeKind::Network,
            TopologyFilter::Volumes => n.kind == ComposeNodeKind::Volume,
        })
        .collect();

    let positions = calculate_node_positions(&filtered_nodes, &topology.edges);
    let selected_node = (*selected_node_id)
        .as_ref()
        .and_then(|id| topology.nodes.iter().find(|n| &n.id == id));

    html! {
        <div class="flex flex-col gap-4">
            { render_summary_bar(locale, &topology.summary) }
            { render_controls(locale, &filter, &zoom_level, topology.nodes.len()) }

            <div class="relative bg-neutral-900 border border-neutral-700/80 rounded-xl overflow-hidden shadow-inner flex flex-col">
                { render_svg_canvas(
                    locale,
                    &filtered_nodes,
                    &topology.edges,
                    &positions,
                    &selected_node_id,
                    *zoom_level,
                ) }

                if let Some(node) = selected_node {
                    { render_inspector_flyout(locale, node, &topology.edges, &selected_node_id) }
                }
            </div>
        </div>
    }
}

fn render_summary_bar(
    locale: Locale,
    summary: &r3v3rs3_api::platform::ComposeTopologySummary,
) -> Html {
    html! {
        <div class="grid grid-cols-2 sm:grid-cols-5 gap-3">
            <div class="bg-neutral-800/80 border border-neutral-700/60 rounded-lg p-3 text-center">
                <div class="text-xs font-medium text-neutral-400">{locale.t("compose.summary_services")}</div>
                <div class="text-lg font-bold text-neutral-100 mt-0.5">{summary.services_count}</div>
            </div>
            <div class="bg-neutral-800/80 border border-neutral-700/60 rounded-lg p-3 text-center">
                <div class="text-xs font-medium text-emerald-400">{locale.t("compose.summary_running")}</div>
                <div class="text-lg font-bold text-emerald-300 mt-0.5">{summary.running_services}</div>
            </div>
            <div class="bg-neutral-800/80 border border-neutral-700/60 rounded-lg p-3 text-center">
                <div class="text-xs font-medium text-teal-400">{locale.t("compose.summary_healthy")}</div>
                <div class="text-lg font-bold text-teal-300 mt-0.5">{summary.healthy_services}</div>
            </div>
            <div class="bg-neutral-800/80 border border-neutral-700/60 rounded-lg p-3 text-center">
                <div class="text-xs font-medium text-cyan-400">{locale.t("compose.summary_networks")}</div>
                <div class="text-lg font-bold text-cyan-300 mt-0.5">{summary.networks_count}</div>
            </div>
            <div class="bg-neutral-800/80 border border-neutral-700/60 rounded-lg p-3 text-center col-span-2 sm:col-span-1">
                <div class="text-xs font-medium text-purple-400">{locale.t("compose.summary_volumes")}</div>
                <div class="text-lg font-bold text-purple-300 mt-0.5">{summary.volumes_count}</div>
            </div>
        </div>
    }
}

fn render_controls(
    locale: Locale,
    filter: &UseStateHandle<TopologyFilter>,
    zoom: &UseStateHandle<f64>,
    total_nodes: usize,
) -> Html {
    let current_filter = **filter;
    let current_zoom = **zoom;

    let set_filter =
        |f: TopologyFilter, h: UseStateHandle<TopologyFilter>| Callback::from(move |_| h.set(f));

    let zoom_in = {
        let zoom = zoom.clone();
        Callback::from(move |_| zoom.set((current_zoom + 0.15).min(2.0)))
    };
    let zoom_out = {
        let zoom = zoom.clone();
        Callback::from(move |_| zoom.set((current_zoom - 0.15).max(0.6)))
    };
    let zoom_reset = {
        let zoom = zoom.clone();
        Callback::from(move |_| zoom.set(1.0))
    };

    html! {
        <div class="flex flex-wrap items-center justify-between gap-3 bg-neutral-800/50 p-2.5 rounded-lg border border-neutral-700/50">
            <div class="flex items-center gap-1.5">
                <button
                    type="button"
                    onclick={set_filter(TopologyFilter::All, filter.clone())}
                    class={filter_button_class(current_filter == TopologyFilter::All)}
                >
                    {locale.t("compose.filter_all")}{format!(" ({total_nodes})")}
                </button>
                <button
                    type="button"
                    onclick={set_filter(TopologyFilter::Services, filter.clone())}
                    class={filter_button_class(current_filter == TopologyFilter::Services)}
                >
                    {locale.t("compose.filter_services")}
                </button>
                <button
                    type="button"
                    onclick={set_filter(TopologyFilter::Networks, filter.clone())}
                    class={filter_button_class(current_filter == TopologyFilter::Networks)}
                >
                    {locale.t("compose.filter_networks")}
                </button>
                <button
                    type="button"
                    onclick={set_filter(TopologyFilter::Volumes, filter.clone())}
                    class={filter_button_class(current_filter == TopologyFilter::Volumes)}
                >
                    {locale.t("compose.filter_volumes")}
                </button>
            </div>

            <div class="flex items-center gap-2">
                <span class="text-xs text-neutral-400 font-mono">{format!("{:.0}%", current_zoom * 100.0)}</span>
                <div class="inline-flex rounded-md shadow-sm border border-neutral-700 overflow-hidden">
                    <button
                        type="button"
                        onclick={zoom_out}
                        title={locale.t("compose.zoom_out")}
                        class="px-2.5 py-1 text-xs bg-neutral-800 hover:bg-neutral-700 text-neutral-200"
                    >
                        {"-"}
                    </button>
                    <button
                        type="button"
                        onclick={zoom_reset}
                        title={locale.t("compose.zoom_reset")}
                        class="px-2.5 py-1 text-xs bg-neutral-800 hover:bg-neutral-700 text-neutral-200 border-x border-neutral-700"
                    >
                        {"100%"}
                    </button>
                    <button
                        type="button"
                        onclick={zoom_in}
                        title={locale.t("compose.zoom_in")}
                        class="px-2.5 py-1 text-xs bg-neutral-800 hover:bg-neutral-700 text-neutral-200"
                    >
                        {"+"}
                    </button>
                </div>
            </div>
        </div>
    }
}

fn filter_button_class(active: bool) -> &'static str {
    if active {
        "px-2.5 py-1 text-xs font-semibold rounded-md bg-blue-600 text-white shadow-sm"
    } else {
        "px-2.5 py-1 text-xs font-medium rounded-md text-neutral-300 hover:bg-neutral-700/60"
    }
}

fn calculate_node_positions(
    nodes: &[&ComposeNode],
    edges: &[ComposeEdge],
) -> HashMap<String, NodePosition> {
    let mut positions = HashMap::new();
    let card_w = 230.0;
    let card_h = 86.0;
    let spacing_x = 70.0;
    let spacing_y = 35.0;

    let layers = assign_node_layers(nodes, edges);

    for (layer_idx, layer_nodes) in layers.into_iter().enumerate() {
        let x = 40.0 + (layer_idx as f64) * (card_w + spacing_x);
        for (row_idx, node_id) in layer_nodes.into_iter().enumerate() {
            let y = 40.0 + (row_idx as f64) * (card_h + spacing_y);
            positions.insert(
                node_id,
                NodePosition {
                    x,
                    y,
                    width: card_w,
                    height: card_h,
                },
            );
        }
    }

    positions
}

fn assign_node_layers(nodes: &[&ComposeNode], edges: &[ComposeEdge]) -> Vec<Vec<String>> {
    let service_depths = calculate_service_depths(nodes, edges);
    group_nodes_by_layers(nodes, &service_depths)
}

fn calculate_service_depths(
    nodes: &[&ComposeNode],
    edges: &[ComposeEdge],
) -> HashMap<String, usize> {
    let mut depths: HashMap<String, usize> = HashMap::new();

    for node in nodes.iter().filter(|n| n.kind == ComposeNodeKind::Service) {
        let is_entrypoint = node
            .service
            .as_ref()
            .map(|s| s.is_entrypoint)
            .unwrap_or(false);
        if is_entrypoint {
            depths.insert(node.id.clone(), 0);
        }
    }

    for _ in 0..4 {
        for edge in edges {
            if let ComposeEdgeKind::DependsOn { .. } = &edge.kind {
                let src_depth = depths.get(&edge.source).copied().unwrap_or(0);
                let target_entry = depths.entry(edge.target.clone()).or_insert(0);
                if *target_entry <= src_depth {
                    *target_entry = src_depth + 1;
                }
            }
        }
    }

    depths
}

fn group_nodes_by_layers(
    nodes: &[&ComposeNode],
    service_depths: &HashMap<String, usize>,
) -> Vec<Vec<String>> {
    let mut layer_map: BTreeMap<usize, Vec<String>> = BTreeMap::new();
    let mut max_svc_depth = 0;

    for node in nodes.iter().filter(|n| n.kind == ComposeNodeKind::Service) {
        let d = service_depths.get(&node.id).copied().unwrap_or(0);
        if d > max_svc_depth {
            max_svc_depth = d;
        }
        layer_map.entry(d).or_default().push(node.id.clone());
    }

    let vol_layer = max_svc_depth + 1;
    for node in nodes.iter().filter(|n| n.kind == ComposeNodeKind::Volume) {
        layer_map
            .entry(vol_layer)
            .or_default()
            .push(node.id.clone());
    }

    let net_layer = vol_layer + 1;
    for node in nodes.iter().filter(|n| n.kind == ComposeNodeKind::Network) {
        layer_map
            .entry(net_layer)
            .or_default()
            .push(node.id.clone());
    }

    layer_map.into_values().collect()
}

fn render_svg_canvas(
    locale: Locale,
    nodes: &[&ComposeNode],
    edges: &[ComposeEdge],
    positions: &HashMap<String, NodePosition>,
    selected_id: &UseStateHandle<Option<String>>,
    zoom: f64,
) -> Html {
    let (max_w, max_h) = calculate_canvas_bounds(positions);

    html! {
        <div class="w-full overflow-auto max-h-[560px] p-2 select-none">
            <svg
                width={format!("{:.0}px", max_w * zoom)}
                height={format!("{:.0}px", max_h * zoom)}
                viewBox={format!("0 0 {max_w} {max_h}")}
                class="min-w-full font-sans transition-all duration-200"
            >
                { render_svg_defs() }

                // Draw connecting edge lines
                <g class="edges">
                    { for edges.iter().filter_map(|e| render_edge(e, positions)) }
                </g>

                // Draw nodes
                <g class="nodes">
                    { for nodes.iter().filter_map(|n| {
                        positions.get(&n.id).map(|pos| render_node_element(locale, n, pos, selected_id))
                    }) }
                </g>
            </svg>
        </div>
    }
}

fn calculate_canvas_bounds(positions: &HashMap<String, NodePosition>) -> (f64, f64) {
    let mut max_x = 780.0;
    let mut max_y = 420.0;
    for pos in positions.values() {
        if pos.x + pos.width + 60.0 > max_x {
            max_x = pos.x + pos.width + 60.0;
        }
        if pos.y + pos.height + 60.0 > max_y {
            max_y = pos.y + pos.height + 60.0;
        }
    }
    (max_x, max_y)
}

fn render_svg_defs() -> Html {
    html! {
        <defs>
            <marker id="marker-arrow-blue" viewBox="0 0 10 10" refX="9" refY="5" markerWidth="6" markerHeight="6" orient="auto-start-reverse">
                <path d="M 0 1.5 L 8 5 L 0 8.5 z" fill="#3b82f6" />
            </marker>
            <marker id="marker-arrow-cyan" viewBox="0 0 10 10" refX="9" refY="5" markerWidth="6" markerHeight="6" orient="auto-start-reverse">
                <path d="M 0 1.5 L 8 5 L 0 8.5 z" fill="#06b6d4" />
            </marker>
            <marker id="marker-arrow-purple" viewBox="0 0 10 10" refX="9" refY="5" markerWidth="6" markerHeight="6" orient="auto-start-reverse">
                <path d="M 0 1.5 L 8 5 L 0 8.5 z" fill="#a855f7" />
            </marker>
        </defs>
    }
}

fn render_edge(edge: &ComposeEdge, positions: &HashMap<String, NodePosition>) -> Option<Html> {
    let src = positions.get(&edge.source)?;
    let tgt = positions.get(&edge.target)?;

    let (stroke, marker, stroke_dash) = match &edge.kind {
        ComposeEdgeKind::DependsOn { .. } => ("#3b82f6", "url(#marker-arrow-blue)", ""),
        ComposeEdgeKind::Network => ("#06b6d4", "url(#marker-arrow-cyan)", "4,4"),
        ComposeEdgeKind::Volume { .. } => ("#a855f7", "url(#marker-arrow-purple)", "3,3"),
        ComposeEdgeKind::Link => ("#64748b", "url(#marker-arrow-blue)", "2,2"),
    };

    let sx = src.x + src.width;
    let sy = src.y + src.height / 2.0;
    let tx = tgt.x;
    let ty = tgt.y + tgt.height / 2.0;

    let c1x = sx + (tx - sx).max(30.0) * 0.5;
    let c2x = tx - (tx - sx).max(30.0) * 0.5;
    let path_data =
        format!("M {sx:.1} {sy:.1} C {c1x:.1} {sy:.1}, {c2x:.1} {ty:.1}, {tx:.1} {ty:.1}");

    let mx = (sx + tx) / 2.0;
    let my = (sy + ty) / 2.0 - 6.0;

    Some(html! {
        <g class="edge-group">
            <path
                d={path_data}
                fill="none"
                stroke={stroke}
                stroke-width="1.8"
                stroke-dasharray={stroke_dash}
                marker-end={marker}
                opacity="0.85"
            />
            if let Some(lbl) = &edge.label {
                <text
                    x={format!("{mx:.1}")}
                    y={format!("{my:.1}")}
                    fill={stroke}
                    font-size="9"
                    font-family="monospace"
                    text-anchor="middle"
                    class="font-medium bg-neutral-900 px-1"
                >
                    {lbl}
                </text>
            }
        </g>
    })
}

fn render_node_element(
    locale: Locale,
    node: &ComposeNode,
    pos: &NodePosition,
    selected_id: &UseStateHandle<Option<String>>,
) -> Html {
    let node_id = node.id.clone();
    let is_selected = selected_id.as_deref() == Some(&node_id);
    let select_cb = {
        let selected_id = selected_id.clone();
        let nid = node_id.clone();
        Callback::from(move |_| {
            if selected_id.as_deref() == Some(&nid) {
                selected_id.set(None);
            } else {
                selected_id.set(Some(nid.clone()));
            }
        })
    };

    let (border_color, status_color, badge_text) = node_colors_and_badge(node);

    html! {
        <g
            class="node-card cursor-pointer transition-transform hover:scale-[1.02]"
            onclick={select_cb}
            transform={format!("translate({:.1}, {:.1})", pos.x, pos.y)}
        >
            // Card background
            <rect
                width={pos.width.to_string()}
                height={pos.height.to_string()}
                rx="8"
                ry="8"
                fill="#171717"
                stroke={if is_selected { "#3b82f6" } else { border_color }}
                stroke-width={if is_selected { "2.5" } else { "1.2" }}
                class="shadow-lg transition-all"
            />

            // Status light dot / halo
            <circle cx="16" cy="20" r="4.5" fill={status_color} />

            // Node Name / Title
            <text
                x="28"
                y="24"
                fill="#f5f5f5"
                font-size="12"
                font-weight="600"
                class="truncate"
            >
                {truncate_string(&node.name, 18)}
            </text>

            // Subtitle / Image / Driver
            <text
                x="14"
                y="46"
                fill="#a3a3a3"
                font-size="10"
                font-family="monospace"
            >
                {node_subtitle(node)}
            </text>

            // Bottom Badges Row
            { render_node_bottom_badges(locale, node, badge_text) }
        </g>
    }
}

fn node_colors_and_badge(node: &ComposeNode) -> (&'static str, &'static str, Option<&'static str>) {
    match node.kind {
        ComposeNodeKind::Service => {
            if let Some(svc) = &node.service {
                if svc.is_entrypoint {
                    return ("#2563eb", "#3b82f6", Some("ENTRYPOINT"));
                }
                match svc.state.as_deref() {
                    Some("running") => {
                        if svc.health == Some(ContainerHealth::Healthy) {
                            ("#10b981", "#10b981", Some("HEALTHY"))
                        } else {
                            ("#22c55e", "#22c55e", Some("RUNNING"))
                        }
                    }
                    Some("exited") => ("#ef4444", "#ef4444", Some("EXITED")),
                    Some("restarting") => ("#f59e0b", "#f59e0b", Some("RESTART")),
                    _ => ("#525252", "#737373", None),
                }
            } else {
                ("#525252", "#737373", None)
            }
        }
        ComposeNodeKind::Network => ("#0891b2", "#06b6d4", Some("NETWORK")),
        ComposeNodeKind::Volume => ("#9333ea", "#a855f7", Some("VOLUME")),
    }
}

fn node_subtitle(node: &ComposeNode) -> String {
    match node.kind {
        ComposeNodeKind::Service => node
            .service
            .as_ref()
            .and_then(|s| s.image.as_ref())
            .map(|i| truncate_string(i, 24))
            .unwrap_or_else(|| "service".to_string()),
        ComposeNodeKind::Network => node
            .network
            .as_ref()
            .and_then(|n| n.driver.as_ref())
            .map(|d| format!("driver: {d}"))
            .unwrap_or_else(|| "bridge network".to_string()),
        ComposeNodeKind::Volume => node
            .volume
            .as_ref()
            .and_then(|v| v.source.as_ref())
            .map(|s| truncate_string(s, 24))
            .unwrap_or_else(|| "named volume".to_string()),
    }
}

fn render_node_bottom_badges(
    _locale: Locale,
    node: &ComposeNode,
    badge_text: Option<&'static str>,
) -> Html {
    html! {
        <g transform="translate(14, 62)">
            if let Some(badge) = badge_text {
                <rect x="0" y="0" width="70" height="15" rx="3" fill="#262626" />
                <text x="35" y="11" fill="#e5e5e5" font-size="8.5" font-weight="600" text-anchor="middle">
                    {badge}
                </text>
            }
            if let Some(svc) = &node.service {
                if let Some(port) = svc.ports.first() {
                    <rect x="76" y="0" width="60" height="15" rx="3" fill="#1e293b" />
                    <text x="106" y="11" fill="#38bdf8" font-size="8.5" font-family="monospace" text-anchor="middle">
                        {truncate_string(port, 9)}
                    </text>
                }
            }
        </g>
    }
}

fn render_inspector_flyout(
    locale: Locale,
    node: &ComposeNode,
    edges: &[ComposeEdge],
    selected_id: &UseStateHandle<Option<String>>,
) -> Html {
    let close = {
        let selected_id = selected_id.clone();
        Callback::from(move |_| selected_id.set(None))
    };

    let upstream_deps: Vec<&ComposeEdge> = edges.iter().filter(|e| e.source == node.id).collect();
    let downstream_deps: Vec<&ComposeEdge> = edges.iter().filter(|e| e.target == node.id).collect();

    html! {
        <div class="absolute bottom-3 right-3 w-80 bg-neutral-900/95 backdrop-blur-md border border-neutral-700 rounded-xl p-4 shadow-2xl text-xs text-neutral-200 z-20 flex flex-col gap-2.5 max-h-[460px] overflow-y-auto">
            <div class="flex items-center justify-between border-b border-neutral-800 pb-2">
                <div class="flex items-center gap-2">
                    <span class="font-bold text-neutral-100 text-sm">{&node.name}</span>
                    <span class="px-1.5 py-0.5 rounded text-[10px] font-semibold bg-neutral-800 text-neutral-300">
                        {node_kind_label(locale, node.kind)}
                    </span>
                </div>
                <button
                    type="button"
                    onclick={close}
                    class="text-neutral-400 hover:text-neutral-100 text-base font-bold px-1"
                >
                    {"×"}
                </button>
            </div>

            if let Some(svc) = &node.service {
                { render_service_inspector_details(locale, svc) }
            }
            if let Some(vol) = &node.volume {
                { render_volume_inspector_details(locale, vol) }
            }
            if let Some(net) = &node.network {
                { render_network_inspector_details(locale, net) }
            }

            { render_dependency_lists(locale, &upstream_deps, &downstream_deps) }
        </div>
    }
}

fn render_service_inspector_details(
    locale: Locale,
    svc: &r3v3rs3_api::platform::ComposeServiceDetails,
) -> Html {
    html! {
        <div class="flex flex-col gap-1 text-[11px]">
            if let Some(img) = &svc.image {
                <div><span class="text-neutral-400">{locale.t("compose.inspect_image")}{": "}</span><span class="font-mono text-neutral-200">{img}</span></div>
            }
            if !svc.ports.is_empty() {
                <div><span class="text-neutral-400">{locale.t("compose.inspect_ports")}{": "}</span><span class="font-mono text-cyan-400">{svc.ports.join(", ")}</span></div>
            }
            if let Some(st) = &svc.state {
                <div><span class="text-neutral-400">{locale.t("apps.service_state")}{": "}</span><span class="font-semibold text-emerald-400">{st}</span></div>
            }
            if let Some(h) = svc.health {
                <div><span class="text-neutral-400">{locale.t("apps.service_health")}{": "}</span><span class="font-semibold text-teal-400">{format!("{h:?}")}</span></div>
            }
            if svc.env_count > 0 {
                <div><span class="text-neutral-400">{locale.t("compose.inspect_env_count")}{": "}</span><span>{svc.env_count}</span></div>
            }
        </div>
    }
}

fn render_volume_inspector_details(
    locale: Locale,
    vol: &r3v3rs3_api::platform::ComposeVolumeDetails,
) -> Html {
    html! {
        <div class="flex flex-col gap-1 text-[11px]">
            if let Some(src) = &vol.source {
                <div><span class="text-neutral-400">{locale.t("compose.inspect_source")}{": "}</span><span class="font-mono">{src}</span></div>
            }
            if let Some(drv) = &vol.driver {
                <div><span class="text-neutral-400">{locale.t("compose.inspect_driver")}{": "}</span><span>{drv}</span></div>
            }
        </div>
    }
}

fn render_network_inspector_details(
    locale: Locale,
    net: &r3v3rs3_api::platform::ComposeNetworkDetails,
) -> Html {
    html! {
        <div class="flex flex-col gap-1 text-[11px]">
            if let Some(drv) = &net.driver {
                <div><span class="text-neutral-400">{locale.t("compose.inspect_driver")}{": "}</span><span>{drv}</span></div>
            }
        </div>
    }
}

fn render_dependency_lists(
    locale: Locale,
    upstream: &[&ComposeEdge],
    downstream: &[&ComposeEdge],
) -> Html {
    html! {
        <div class="flex flex-col gap-2 pt-1 border-t border-neutral-800 text-[11px]">
            if !upstream.is_empty() {
                <div>
                    <span class="font-medium text-neutral-400 block mb-1">{locale.t("compose.inspect_dependencies")}{":"}</span>
                    <div class="flex flex-wrap gap-1">
                        { for upstream.iter().map(|e| {
                            let target_name = e.target.split(':').nth(1).unwrap_or(&e.target);
                            html! {
                                <span class="px-1.5 py-0.5 rounded bg-blue-950 border border-blue-800 text-blue-300 font-mono text-[10px]">
                                    {target_name}
                                    if let Some(lbl) = &e.label { {format!(" ({lbl})")} }
                                </span>
                            }
                        }) }
                    </div>
                </div>
            }
            if !downstream.is_empty() {
                <div>
                    <span class="font-medium text-neutral-400 block mb-1">{locale.t("compose.inspect_connected_services")}{":"}</span>
                    <div class="flex flex-wrap gap-1">
                        { for downstream.iter().map(|e| {
                            let src_name = e.source.split(':').nth(1).unwrap_or(&e.source);
                            html! {
                                <span class="px-1.5 py-0.5 rounded bg-neutral-800 text-neutral-300 font-mono text-[10px]">
                                    {src_name}
                                </span>
                            }
                        }) }
                    </div>
                </div>
            }
        </div>
    }
}

fn node_kind_label(locale: Locale, kind: ComposeNodeKind) -> &'static str {
    match kind {
        ComposeNodeKind::Service => locale.t("compose.node_kind_service"),
        ComposeNodeKind::Network => locale.t("compose.node_kind_network"),
        ComposeNodeKind::Volume => locale.t("compose.node_kind_volume"),
    }
}

fn truncate_string(s: &str, max_len: usize) -> String {
    if s.len() > max_len {
        format!("{}…", &s[..max_len.saturating_sub(1)])
    } else {
        s.to_string()
    }
}
