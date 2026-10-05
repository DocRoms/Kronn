//! GET/POST /api/config/desktop-port — the loopback port the desktop app binds
//! at launch (`desktop-port.json`). The shell reads the file before any config
//! load, so a change applies at the next launch.

use axum::{extract::State, Json};
use serde::{Deserialize, Serialize};

use crate::core::config::config_dir;
use crate::core::desktop_port::{
    port_file_path, read_persisted, validate_port, write_persisted, MAX_PORT, MIN_PORT,
};
use crate::models::ApiResponse;
use crate::AppState;

#[derive(Serialize)]
pub struct DesktopPortInfo {
    /// The port saved for the next launch, if any.
    pub saved: Option<u16>,
    /// The port this process listens on now.
    pub current: u16,
    pub min: u16,
    pub max: u16,
}

#[derive(Deserialize)]
pub struct SetDesktopPortRequest {
    pub port: i64,
}

async fn info(state: &AppState) -> Result<DesktopPortInfo, String> {
    let dir = config_dir().map_err(|e| e.to_string())?;
    Ok(DesktopPortInfo {
        saved: read_persisted(&port_file_path(&dir)).port(),
        current: state.config.read().await.server.listening_port(),
        min: MIN_PORT,
        max: MAX_PORT,
    })
}

pub async fn get(State(state): State<AppState>) -> Json<ApiResponse<DesktopPortInfo>> {
    match info(&state).await {
        Ok(info) => Json(ApiResponse::ok(info)),
        Err(error) => Json(ApiResponse::err(error)),
    }
}

pub async fn set(
    State(state): State<AppState>,
    Json(req): Json<SetDesktopPortRequest>,
) -> Json<ApiResponse<DesktopPortInfo>> {
    let port = match validate_port(req.port) {
        Ok(port) => port,
        Err(message) => return Json(ApiResponse::err(message)),
    };
    let dir = match config_dir() {
        Ok(dir) => dir,
        Err(error) => return Json(ApiResponse::err(error.to_string())),
    };
    let path = port_file_path(&dir);
    if let Err(error) = tokio::task::spawn_blocking(move || write_persisted(&path, port))
        .await
        .map_err(|e| e.to_string())
        .and_then(|result| result.map_err(|e| e.to_string()))
    {
        return Json(ApiResponse::err(format!(
            "Failed to save the desktop port: {error}"
        )));
    }
    match info(&state).await {
        Ok(info) => Json(ApiResponse::ok(info)),
        Err(error) => Json(ApiResponse::err(error)),
    }
}
