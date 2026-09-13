use super::openapi::{ErrorResponses, NotFoundResponse};
use super::{AppError, AppState};
use crate::server::rpc::ports::{
    AddPort, DeletePort, GetNetworkInterfaceList, GetPort, GetPortList, GetPortStatus, ResetPort,
    UpdatePort,
};
use axum::{
    extract::{Path, State},
    Json,
};
use r3v3rs3_api::{
    id::ShortId,
    port::{NetworkInterface, Port, PortEntry, PortStatus},
};

/// Lists the ports.
#[utoipa::path(
    get,
    path = "/",
    tag = "ports",
    operation_id = "list_ports",
    responses((status = 200, description = "The ports.", body = Vec<PortEntry>), ErrorResponses)
)]
pub async fn list(State(state): State<AppState>) -> Result<Json<Box<Vec<PortEntry>>>, AppError> {
    Ok(Json(state.call(GetPortList).await?))
}

/// Returns one port.
#[utoipa::path(
    get,
    path = "/{id}",
    tag = "ports",
    operation_id = "get_port",
    params(("id" = ShortId, Path, description = "Port id.")),
    responses((status = 200, description = "The port.", body = PortEntry), NotFoundResponse, ErrorResponses)
)]
pub async fn get(
    State(state): State<AppState>,
    Path(id): Path<ShortId>,
) -> Result<Json<Box<PortEntry>>, AppError> {
    Ok(Json(state.call(GetPort { id }).await?))
}

/// Returns the socket and TLS state of a port.
#[utoipa::path(
    get,
    path = "/{id}/status",
    tag = "ports",
    operation_id = "get_port_status",
    params(("id" = ShortId, Path, description = "Port id.")),
    responses((status = 200, description = "The port status.", body = PortStatus), NotFoundResponse, ErrorResponses)
)]
pub async fn status(
    State(state): State<AppState>,
    Path(id): Path<ShortId>,
) -> Result<Json<Box<PortStatus>>, AppError> {
    Ok(Json(state.call(GetPortStatus { id }).await?))
}

/// Deletes a port.
#[utoipa::path(
    delete,
    path = "/{id}",
    tag = "ports",
    operation_id = "delete_port",
    params(("id" = ShortId, Path, description = "Port id.")),
    responses((status = 200, description = "The port is deleted."), NotFoundResponse, ErrorResponses)
)]
pub async fn delete(
    State(state): State<AppState>,
    Path(id): Path<ShortId>,
) -> Result<Json<Box<()>>, AppError> {
    Ok(Json(state.call(DeletePort { id }).await?))
}

/// Adds a port.
#[utoipa::path(
    post,
    path = "/",
    tag = "ports",
    operation_id = "add_port",
    request_body = Port,
    responses((status = 200, description = "The port is added."), ErrorResponses)
)]
pub async fn add(
    State(state): State<AppState>,
    Json(entry): Json<Port>,
) -> Result<Json<Box<()>>, AppError> {
    Ok(Json(state.call(AddPort { entry }).await?))
}

/// Replaces the config of a port.
#[utoipa::path(
    put,
    path = "/{id}",
    tag = "ports",
    operation_id = "update_port",
    params(("id" = ShortId, Path, description = "Port id.")),
    request_body = Port,
    responses((status = 200, description = "The port is updated."), NotFoundResponse, ErrorResponses)
)]
pub async fn put(
    State(state): State<AppState>,
    Path(id): Path<ShortId>,
    Json(entry): Json<Port>,
) -> Result<Json<Box<()>>, AppError> {
    let entry = (id, entry).into();
    Ok(Json(state.call(UpdatePort { entry }).await?))
}

/// Closes and opens the listener of a port again.
#[utoipa::path(
    get,
    path = "/{id}/reset",
    tag = "ports",
    operation_id = "reset_port",
    params(("id" = ShortId, Path, description = "Port id.")),
    responses((status = 200, description = "The port is reset."), NotFoundResponse, ErrorResponses)
)]
pub async fn reset(
    State(state): State<AppState>,
    Path(id): Path<ShortId>,
) -> Result<Json<Box<()>>, AppError> {
    Ok(Json(state.call(ResetPort { id }).await?))
}

/// Lists the network interfaces that a port can listen on.
#[utoipa::path(
    get,
    path = "/interfaces",
    tag = "ports",
    operation_id = "list_network_interfaces",
    responses((status = 200, description = "The network interfaces.", body = Vec<NetworkInterface>), ErrorResponses)
)]
pub async fn interfaces(
    State(state): State<AppState>,
) -> Result<Json<Box<Vec<NetworkInterface>>>, AppError> {
    Ok(Json(state.call(GetNetworkInterfaceList).await?))
}
