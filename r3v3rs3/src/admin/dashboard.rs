use super::openapi::ErrorResponses;
use super::platform::platform_error;
use super::{AppError, AppState};
use crate::accounts::{Caller, Permission};
use crate::platform::Platform;
use crate::server::rpc::ports::GetPortList;
use crate::server::rpc::proxies::GetProxyList;
use axum::{Extension, Json, extract::State};
use r3v3rs3_api::dashboard::{
    AppsSummary, DashboardSummary, DatabasesSummary, DeploymentSummary, DiskSpace, PortsSummary,
    ProxiesSummary, TargetsSummary,
};
use r3v3rs3_api::id::ShortId;
use r3v3rs3_api::platform::{AppEntry, DeploymentEntry, TargetEntry};
use r3v3rs3_api::port::PortEntry;
use r3v3rs3_api::proxy::ProxyEntry;
use std::collections::{HashMap, HashSet};
use std::path::Path;

/// Returns the system dashboard overview.
#[utoipa::path(
    get,
    path = "/",
    tag = "dashboard",
    operation_id = "get_dashboard",
    responses((status = 200, description = "The system dashboard overview.", body = DashboardSummary), ErrorResponses)
)]
pub async fn get(
    State(state): State<AppState>,
    Extension(caller): Extension<Caller>,
) -> Result<Json<DashboardSummary>, AppError> {
    caller.authorize(Permission::Read)?;

    let ports_list = state.call(&caller, GetPortList).await?;
    let proxies_list = state.call(&caller, GetProxyList).await?;
    let ports = summarize_ports(&ports_list);
    let proxies = summarize_proxies(&proxies_list);

    let (apps, databases, targets, recent_deployments) =
        match state.platform(&caller, Permission::Read).await {
            Ok(platform) => fetch_platform_info(&platform).await?,
            Err(_) => (None, None, None, Vec::new()),
        };

    let data = state.data.lock().await;
    let config_path = data.app_info.config_path.clone();
    let version = data.app_info.version.to_string();
    drop(data);

    let disk = get_disk_space(&config_path);
    let update_available = *state
        .call_system(crate::server::rpc::auth::GetUpdateAvailable)
        .await?;

    Ok(Json(DashboardSummary {
        ports,
        proxies,
        apps,
        databases,
        targets,
        disk,
        version,
        update_available,
        recent_deployments,
    }))
}

fn summarize_ports(ports: &[PortEntry]) -> PortsSummary {
    let active = ports.iter().filter(|p| p.port.active).count();
    PortsSummary {
        total: ports.len(),
        active,
        error: 0,
    }
}

fn summarize_proxies(proxies: &[ProxyEntry]) -> ProxiesSummary {
    let active = proxies.iter().filter(|p| p.proxy.active).count();
    ProxiesSummary {
        total: proxies.len(),
        active,
        error: 0,
    }
}

type PlatformOverview = (
    Option<AppsSummary>,
    Option<DatabasesSummary>,
    Option<TargetsSummary>,
    Vec<DeploymentSummary>,
);

async fn fetch_platform_info(platform: &Platform) -> Result<PlatformOverview, AppError> {
    let app_list = platform.apps().await.map_err(platform_error)?;
    let target_list = platform.targets().await.map_err(platform_error)?;
    let running = platform
        .running_deployments()
        .await
        .map_err(platform_error)?;
    let recent = platform
        .recent_deployments(5)
        .await
        .map_err(platform_error)?;

    let running_set: HashSet<ShortId> = running.into_iter().map(|d| d.app).collect();
    let (apps, databases, app_names) = summarize_apps(&app_list, &running_set);
    let targets = summarize_targets(&target_list);
    let recent_deployments = summarize_recent_deployments(recent, &app_names);

    Ok((
        Some(apps),
        Some(databases),
        Some(targets),
        recent_deployments,
    ))
}

fn summarize_apps(
    apps: &[AppEntry],
    running: &HashSet<ShortId>,
) -> (AppsSummary, DatabasesSummary, HashMap<ShortId, String>) {
    let catalog = crate::appstore::state().catalog.clone();
    let mut app_summary = AppsSummary::default();
    let mut db_summary = DatabasesSummary::default();
    let mut names = HashMap::new();

    for app in apps {
        names.insert(app.id, app.name.to_string());
        let is_running = running.contains(&app.id);
        if app.spec.database.is_some() {
            db_summary.total += 1;
            if is_running {
                db_summary.running += 1;
            }
        } else {
            app_summary.total += 1;
            if is_running {
                app_summary.running += 1;
            } else {
                app_summary.stopped += 1;
            }
            if check_has_update(app, &catalog) {
                app_summary.update_available += 1;
            }
        }
    }
    (app_summary, db_summary, names)
}

fn check_has_update(app: &AppEntry, catalog: &r3v3rs3_api::store::StoreCatalog) -> bool {
    super::platform::check_catalog_update(app, catalog).is_some()
        || app.update.as_ref().is_some_and(|u| u.update_available)
}

fn summarize_targets(targets: &[TargetEntry]) -> TargetsSummary {
    let total = targets.len();
    let online = targets.iter().filter(|t| t.online).count();
    let offline = total.saturating_sub(online);
    TargetsSummary {
        total,
        online,
        offline,
    }
}

fn summarize_recent_deployments(
    deployments: Vec<DeploymentEntry>,
    names: &HashMap<ShortId, String>,
) -> Vec<DeploymentSummary> {
    deployments
        .into_iter()
        .map(|d| {
            let app_name = names
                .get(&d.app)
                .cloned()
                .unwrap_or_else(|| d.app.to_string());
            DeploymentSummary {
                id: d.id,
                app_id: d.app,
                app_name,
                status: d.status,
                trigger: d.trigger,
                started_at: d.started_at,
                finished_at: d.finished_at,
            }
        })
        .collect()
}

fn get_disk_space(path: &Path) -> Option<DiskSpace> {
    use std::ffi::CString;
    use std::os::unix::ffi::OsStrExt;

    let c_path = CString::new(path.as_os_str().as_bytes()).ok()?;
    let mut stat: libc::statvfs = unsafe { std::mem::zeroed() };
    if unsafe { libc::statvfs(c_path.as_ptr(), &mut stat) } != 0 {
        return None;
    }

    let frsize = stat.f_frsize as u64;
    let total_bytes = stat.f_blocks as u64 * frsize;
    let available_bytes = stat.f_bavail as u64 * frsize;
    let used_bytes = total_bytes.saturating_sub(available_bytes);
    let used_percent = if total_bytes > 0 {
        ((used_bytes as f64 / total_bytes as f64) * 100.0).round() as u8
    } else {
        0
    };

    Some(DiskSpace {
        total_bytes,
        available_bytes,
        used_bytes,
        used_percent,
    })
}
