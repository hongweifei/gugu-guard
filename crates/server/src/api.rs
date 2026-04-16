use axum::{
    extract::{Path, Query, State},
    http::StatusCode,
    response::IntoResponse,
    routing::{get, post},
    Json, Router,
};
use gugu_core::config::ProcessConfig;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::path::PathBuf;

use crate::state::AppState;

pub fn routes() -> Router<AppState> {
    Router::new()
        .route("/api/v1/processes", get(list_processes))
        .route("/api/v1/processes/:name", get(get_process).post(create_process).put(update_process).delete(delete_process))
        .route("/api/v1/processes/:name/config", get(get_process_config))
        .route("/api/v1/processes/:name/start", post(start_process))
        .route("/api/v1/processes/:name/stop", post(stop_process))
        .route("/api/v1/processes/:name/restart", post(restart_process))
        .route("/api/v1/processes/:name/logs", get(get_logs))
        .route("/api/v1/stats", get(get_stats))
        .route("/api/v1/fs/browse", get(browse_fs))
}

#[derive(Deserialize)]
struct LogQuery {
    #[serde(default = "default_lines")]
    lines: usize,
}

fn default_lines() -> usize {
    100
}

#[derive(Deserialize)]
struct CreateProcessRequest {
    command: String,
    #[serde(default)]
    args: Vec<String>,
    working_dir: Option<std::path::PathBuf>,
    #[serde(default)]
    env: HashMap<String, String>,
    #[serde(default = "default_true")]
    auto_start: bool,
    #[serde(default = "default_true")]
    auto_restart: bool,
    #[serde(default = "default_max_restarts")]
    max_restarts: u32,
    #[serde(default = "default_restart_delay")]
    restart_delay_secs: u64,
    #[serde(default = "default_stop_timeout")]
    stop_timeout_secs: u64,
    stdout_log: Option<std::path::PathBuf>,
    stderr_log: Option<std::path::PathBuf>,
    #[serde(default)]
    start_now: bool,
    new_name: Option<String>,
}

fn default_true() -> bool {
    true
}
fn default_max_restarts() -> u32 {
    3
}
fn default_restart_delay() -> u64 {
    5
}
fn default_stop_timeout() -> u64 {
    10
}

impl From<CreateProcessRequest> for ProcessConfig {
    fn from(req: CreateProcessRequest) -> Self {
        ProcessConfig {
            command: req.command,
            args: req.args,
            working_dir: req.working_dir,
            env: req.env,
            auto_start: req.auto_start,
            auto_restart: req.auto_restart,
            max_restarts: req.max_restarts,
            restart_delay_secs: req.restart_delay_secs,
            stop_timeout_secs: req.stop_timeout_secs,
            health_check: None,
            stdout_log: req.stdout_log,
            stderr_log: req.stderr_log,
        }
    }
}

async fn list_processes(State(state): State<AppState>) -> impl IntoResponse {
    let mgr = state.manager.read().await;
    Json(mgr.list_processes())
}

async fn get_process(
    State(state): State<AppState>,
    Path(name): Path<String>,
) -> impl IntoResponse {
    let mgr = state.manager.read().await;
    match mgr.get_process_info(&name) {
        Some(info) => Json(Some(info)).into_response(),
        None => (StatusCode::NOT_FOUND, Json(serde_json::json!({"error": "进程未找到"}))).into_response(),
    }
}

async fn create_process(
    State(state): State<AppState>,
    Path(name): Path<String>,
    Json(body): Json<CreateProcessRequest>,
) -> impl IntoResponse {
    let start_now = body.start_now;
    let config: ProcessConfig = body.into();
    let mut mgr = state.manager.write().await;
    match mgr.add_process(name, config, start_now).await {
        Ok(()) => (StatusCode::CREATED, Json(serde_json::json!({"message": "进程已添加"}))).into_response(),
        Err(e) => (StatusCode::BAD_REQUEST, Json(serde_json::json!({"error": e.to_string()}))).into_response(),
    }
}

async fn update_process(
    State(state): State<AppState>,
    Path(name): Path<String>,
    Json(body): Json<CreateProcessRequest>,
) -> impl IntoResponse {
    let new_name = body.new_name.clone();
    let config: ProcessConfig = body.into();
    let mut mgr = state.manager.write().await;
    match mgr.update_process(&name, config, new_name, true).await {
        Ok(()) => Json(serde_json::json!({"message": "进程已更新"})).into_response(),
        Err(e) => (StatusCode::BAD_REQUEST, Json(serde_json::json!({"error": e.to_string()}))).into_response(),
    }
}

async fn delete_process(
    State(state): State<AppState>,
    Path(name): Path<String>,
) -> impl IntoResponse {
    let mut mgr = state.manager.write().await;
    match mgr.remove_process(&name).await {
        Ok(()) => Json(serde_json::json!({"message": "进程已移除"})).into_response(),
        Err(e) => (StatusCode::NOT_FOUND, Json(serde_json::json!({"error": e.to_string()}))).into_response(),
    }
}

async fn get_process_config(
    State(state): State<AppState>,
    Path(name): Path<String>,
) -> impl IntoResponse {
    let mgr = state.manager.read().await;
    match mgr.get_process_config(&name) {
        Some(config) => Json(config).into_response(),
        None => (StatusCode::NOT_FOUND, Json(serde_json::json!({"error": "进程未找到"}))).into_response(),
    }
}

async fn start_process(
    State(state): State<AppState>,
    Path(name): Path<String>,
) -> impl IntoResponse {
    let mut mgr = state.manager.write().await;
    match mgr.start_process(&name).await {
        Ok(()) => Json(serde_json::json!({"message": "进程已启动"})).into_response(),
        Err(e) => (StatusCode::BAD_REQUEST, Json(serde_json::json!({"error": e.to_string()}))).into_response(),
    }
}

async fn stop_process(
    State(state): State<AppState>,
    Path(name): Path<String>,
) -> impl IntoResponse {
    let mut mgr = state.manager.write().await;
    match mgr.stop_process(&name).await {
        Ok(()) => Json(serde_json::json!({"message": "进程已停止"})).into_response(),
        Err(e) => (StatusCode::BAD_REQUEST, Json(serde_json::json!({"error": e.to_string()}))).into_response(),
    }
}

async fn restart_process(
    State(state): State<AppState>,
    Path(name): Path<String>,
) -> impl IntoResponse {
    let mut mgr = state.manager.write().await;
    match mgr.restart_process(&name).await {
        Ok(()) => Json(serde_json::json!({"message": "进程已重启"})).into_response(),
        Err(e) => (StatusCode::BAD_REQUEST, Json(serde_json::json!({"error": e.to_string()}))).into_response(),
    }
}

async fn get_logs(
    State(state): State<AppState>,
    Path(name): Path<String>,
    Query(query): Query<LogQuery>,
) -> impl IntoResponse {
    let mgr = state.manager.read().await;
    match mgr.get_process_logs(&name, query.lines).await {
        Ok(logs) => Json(logs).into_response(),
        Err(e) => (StatusCode::NOT_FOUND, Json(serde_json::json!({"error": e.to_string()}))).into_response(),
    }
}

async fn get_stats(State(state): State<AppState>) -> impl IntoResponse {
    let mgr = state.manager.read().await;
    let processes = mgr.list_processes();
    let total = processes.len();
    let running = processes.iter().filter(|p| matches!(p.status, gugu_core::process::ProcessStatus::Running)).count();
    let stopped = processes.iter().filter(|p| matches!(p.status, gugu_core::process::ProcessStatus::Stopped)).count();
    let failed = processes.iter().filter(|p| matches!(p.status, gugu_core::process::ProcessStatus::Failed(_))).count();

    Json(serde_json::json!({
        "total": total,
        "running": running,
        "stopped": stopped,
        "failed": failed,
    }))
}

#[derive(Deserialize)]
struct BrowseQuery {
    path: Option<String>,
}

#[derive(Serialize)]
struct FsEntry {
    name: String,
    path: String,
    is_dir: bool,
}

async fn browse_fs(Query(query): Query<BrowseQuery>) -> impl IntoResponse {
    let raw = query.path.unwrap_or_else(|| ".".into());
    let dir = PathBuf::from(&raw);

    let dir = if dir.is_relative() {
        std::env::current_dir().unwrap_or_default().join(&dir)
    } else {
        dir
    };

    let parent = dir.parent().map(|p| p.to_string_lossy().to_string());

    let read_dir = match std::fs::read_dir(&dir) {
        Ok(rd) => rd,
        Err(e) => return (StatusCode::BAD_REQUEST, Json(serde_json::json!({"error": e.to_string()}))).into_response(),
    };

    let mut entries: Vec<FsEntry> = Vec::new();
    for entry in read_dir.flatten() {
        let name = entry.file_name().to_string_lossy().to_string();
        let path = entry.path().to_string_lossy().to_string();
        let is_dir = entry.file_type().map(|t| t.is_dir()).unwrap_or(false);
        entries.push(FsEntry { name, path, is_dir });
    }

    entries.sort_by(|a, b| {
        b.is_dir.cmp(&a.is_dir).then(a.name.to_lowercase().cmp(&b.name.to_lowercase()))
    });

    Json(serde_json::json!({
        "path": dir.to_string_lossy(),
        "parent": parent,
        "entries": entries,
    })).into_response()
}
