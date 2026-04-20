use axum::{
    extract::{Path, Query, State},
    http::StatusCode,
    middleware::Next,
    response::IntoResponse,
    routing::{get, post},
    extract::Request,
    Json, Router,
};
use gugu_core::config::{HealthCheckConfig, ProcessConfig};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::path::PathBuf;

use crate::state::AppState;

#[derive(Serialize)]
struct ApiSuccess {
    message: String,
}

impl ApiSuccess {
    fn new(msg: &str) -> Self {
        Self { message: msg.to_string() }
    }
}

#[derive(Serialize)]
struct ApiError {
    error: String,
}

impl ApiError {
    fn new(msg: impl Into<String>) -> Self {
        Self { error: msg.into() }
    }
}

#[derive(Serialize)]
struct StatsResponse {
    total: usize,
    running: usize,
    stopped: usize,
    failed: usize,
}

#[derive(Serialize)]
struct BrowseResponse {
    path: String,
    parent: Option<String>,
    entries: Vec<FsEntry>,
}

#[derive(Serialize)]
struct FsEntry {
    name: String,
    path: String,
    is_dir: bool,
}

pub fn routes() -> Router<AppState> {
    Router::new()
        .route("/api/v1/processes", get(list_processes))
        .route("/api/v1/processes/:name", get(get_process).post(create_process).put(update_process).delete(delete_process))
        .route("/api/v1/processes/:name/config", get(get_process_config))
        .route("/api/v1/processes/:name/start", post(start_process))
        .route("/api/v1/processes/:name/stop", post(stop_process))
        .route("/api/v1/processes/:name/restart", post(restart_process))
        .route("/api/v1/processes/:name/logs", get(get_logs).delete(clear_logs))
        .route("/api/v1/processes/:name/logs/download", get(download_logs))
        .route("/api/v1/processes/:name/health", post(check_health))
        .route("/api/v1/stats", get(get_stats))
        .route("/api/v1/fs/browse", get(browse_fs))
        .route("/api/v1/reload", post(reload_config))
}

pub async fn auth_middleware(
    State(state): State<AppState>,
    req: Request,
    next: Next,
) -> axum::response::Response {
    if let Some(ref expected_key) = state.api_key {
        if !expected_key.is_empty() {
            let auth_header = req
                .headers()
                .get("Authorization")
                .and_then(|v| v.to_str().ok())
                .and_then(|v| v.strip_prefix("Bearer "));

            if let Some(key) = auth_header {
                if constant_time_eq(key, expected_key) {
                    return next.run(req).await;
                }
            }

            if let Some(query) = req.uri().query() {
                for pair in query.split('&') {
                    if let Some(token) = pair.strip_prefix("token=") {
                        if constant_time_eq(token, expected_key) {
                            return next.run(req).await;
                        }
                    }
                }
            }

            return StatusCode::UNAUTHORIZED.into_response();
        }
    }
    next.run(req).await
}

fn constant_time_eq(a: &str, b: &str) -> bool {
    let a_bytes = a.as_bytes();
    let b_bytes = b.as_bytes();
    if a_bytes.len() != b_bytes.len() {
        return false;
    }
    let mut result: u8 = 0;
    for (x, y) in a_bytes.iter().zip(b_bytes.iter()) {
        result |= x ^ y;
    }
    result == 0
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
    #[serde(default = "gugu_core::config::default_true")]
    auto_start: bool,
    #[serde(default = "gugu_core::config::default_true")]
    auto_restart: bool,
    #[serde(default = "gugu_core::config::default_max_restarts")]
    max_restarts: u32,
    #[serde(default = "gugu_core::config::default_restart_delay")]
    restart_delay_secs: u64,
    #[serde(default = "gugu_core::config::default_stop_timeout")]
    stop_timeout_secs: u64,
    #[serde(default)]
    health_check: Option<HealthCheckConfig>,
    #[serde(default)]
    unhealthy_restart: bool,
    #[serde(default)]
    depends_on: Vec<String>,
    #[serde(default)]
    max_log_size_mb: Option<u64>,
    stdout_log: Option<std::path::PathBuf>,
    stderr_log: Option<std::path::PathBuf>,
    #[serde(default)]
    start_now: bool,
    new_name: Option<String>,
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
            health_check: req.health_check,
            unhealthy_restart: req.unhealthy_restart,
            depends_on: req.depends_on,
            max_log_size_mb: req.max_log_size_mb,
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
        None => (StatusCode::NOT_FOUND, Json(ApiError::new("进程未找到"))).into_response(),
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
        Ok(()) => (StatusCode::CREATED, Json(ApiSuccess::new("进程已添加"))).into_response(),
        Err(e) => (StatusCode::BAD_REQUEST, Json(ApiError::new(e.to_string()))).into_response(),
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
    match mgr.update_process(&name, config, new_name, false).await {
        Ok(()) => Json(ApiSuccess::new("进程已更新")).into_response(),
        Err(e) => (StatusCode::BAD_REQUEST, Json(ApiError::new(e.to_string()))).into_response(),
    }
}

async fn delete_process(
    State(state): State<AppState>,
    Path(name): Path<String>,
) -> impl IntoResponse {
    let mut mgr = state.manager.write().await;
    match mgr.remove_process(&name).await {
        Ok(()) => Json(ApiSuccess::new("进程已移除")).into_response(),
        Err(e) => (StatusCode::NOT_FOUND, Json(ApiError::new(e.to_string()))).into_response(),
    }
}

async fn get_process_config(
    State(state): State<AppState>,
    Path(name): Path<String>,
) -> impl IntoResponse {
    let mgr = state.manager.read().await;
    match mgr.get_process_config(&name) {
        Some(config) => Json(config).into_response(),
        None => (StatusCode::NOT_FOUND, Json(ApiError::new("进程未找到"))).into_response(),
    }
}

async fn start_process(
    State(state): State<AppState>,
    Path(name): Path<String>,
) -> impl IntoResponse {
    let mut mgr = state.manager.write().await;
    match mgr.start_process(&name).await {
        Ok(()) => Json(ApiSuccess::new("进程已启动")).into_response(),
        Err(e) => (StatusCode::BAD_REQUEST, Json(ApiError::new(e.to_string()))).into_response(),
    }
}

async fn stop_process(
    State(state): State<AppState>,
    Path(name): Path<String>,
) -> impl IntoResponse {
    let mut mgr = state.manager.write().await;
    match mgr.stop_process(&name).await {
        Ok(()) => Json(ApiSuccess::new("进程已停止")).into_response(),
        Err(e) => (StatusCode::BAD_REQUEST, Json(ApiError::new(e.to_string()))).into_response(),
    }
}

async fn restart_process(
    State(state): State<AppState>,
    Path(name): Path<String>,
) -> impl IntoResponse {
    let mut mgr = state.manager.write().await;
    match mgr.restart_process(&name).await {
        Ok(()) => Json(ApiSuccess::new("进程已重启")).into_response(),
        Err(e) => (StatusCode::BAD_REQUEST, Json(ApiError::new(e.to_string()))).into_response(),
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
        Err(e) => (StatusCode::NOT_FOUND, Json(ApiError::new(e.to_string()))).into_response(),
    }
}

async fn clear_logs(
    State(state): State<AppState>,
    Path(name): Path<String>,
) -> impl IntoResponse {
    let mgr = state.manager.read().await;
    match mgr.clear_process_logs(&name).await {
        Ok(()) => Json(ApiSuccess::new("日志已清空")).into_response(),
        Err(e) => (StatusCode::NOT_FOUND, Json(ApiError::new(e.to_string()))).into_response(),
    }
}

async fn download_logs(
    State(state): State<AppState>,
    Path(name): Path<String>,
    Query(query): Query<LogQuery>,
) -> impl IntoResponse {
    let mgr = state.manager.read().await;
    match mgr.get_process_logs(&name, query.lines).await {
        Ok(logs) => {
            let mut content = String::new();
            for entry in &logs {
                let time = entry.timestamp.format("%Y-%m-%d %H:%M:%S%.3f");
                let prefix = match &entry.stream {
                    gugu_core::process::LogStream::Stdout => "OUT",
                    gugu_core::process::LogStream::Stderr => "ERR",
                    _ => "???",
                };
                content.push_str(&format!("[{time}] [{prefix}] {}\n", entry.line));
            }
            let filename = format!("{name}.log");
            (
                StatusCode::OK,
                [
                    ("content-type", "text/plain; charset=utf-8".to_string()),
                    ("content-disposition", format!("attachment; filename=\"{filename}\"")),
                ],
                content,
            ).into_response()
        }
        Err(e) => (StatusCode::NOT_FOUND, Json(ApiError::new(e.to_string()))).into_response(),
    }
}

async fn check_health(
    Path(name): Path<String>,
    State(state): State<AppState>,
) -> impl IntoResponse {
    let mut mgr = state.manager.write().await;
    match mgr.check_process_health(&name).await {
        Ok(healthy) => Json(serde_json::json!({
            "healthy": healthy,
            "message": if healthy { "健康检查通过" } else { "健康检查失败" }
        })).into_response(),
        Err(e) => (StatusCode::BAD_REQUEST, Json(ApiError::new(e.to_string()))).into_response(),
    }
}

async fn get_stats(State(state): State<AppState>) -> impl IntoResponse {
    let mgr = state.manager.read().await;
    let processes = mgr.list_processes();
    let total = processes.len();
    let running = processes.iter().filter(|p| matches!(p.status, gugu_core::process::ProcessStatus::Running)).count();
    let stopped = processes.iter().filter(|p| matches!(p.status, gugu_core::process::ProcessStatus::Stopped)).count();
    let failed = processes.iter().filter(|p| matches!(p.status, gugu_core::process::ProcessStatus::Failed(_))).count();

    Json(StatsResponse { total, running, stopped, failed })
}

#[derive(Deserialize)]
struct BrowseQuery {
    path: Option<String>,
}

async fn browse_fs(Query(query): Query<BrowseQuery>) -> impl IntoResponse {
    let raw = query.path.unwrap_or_else(|| ".".into());
    let dir = PathBuf::from(&raw);

    let dir = if dir.is_relative() {
        let joined = std::env::current_dir().unwrap_or_default().join(&dir);
        joined.canonicalize().unwrap_or(joined)
    } else {
        dir.canonicalize().unwrap_or(dir)
    };

    let path_str = clean_path(&dir);
    let parent = dir.parent().map(clean_path);

    let read_dir = match std::fs::read_dir(&dir) {
        Ok(rd) => rd,
        Err(e) => return (StatusCode::BAD_REQUEST, Json(ApiError::new(e.to_string()))).into_response(),
    };

    let mut entries: Vec<FsEntry> = Vec::with_capacity(256);
    let sep = std::path::MAIN_SEPARATOR;
    for entry in read_dir.flatten() {
        let is_dir = entry.file_type().map(|t| t.is_dir()).unwrap_or(false);
        let name = entry.file_name().to_string_lossy().to_string();
        let path = format!("{}{}{}", path_str, sep, name);
        entries.push(FsEntry { name, path, is_dir });
    }

    entries.sort_by(|a, b| {
        b.is_dir.cmp(&a.is_dir).then(a.name.to_lowercase().cmp(&b.name.to_lowercase()))
    });

    Json(BrowseResponse {
        path: path_str,
        parent,
        entries,
    }).into_response()
}

async fn reload_config(State(state): State<AppState>) -> impl IntoResponse {
    let mut mgr = state.manager.write().await;
    match mgr.reload_from_file().await {
        Ok(()) => Json(ApiSuccess::new("配置已重新加载")).into_response(),
        Err(e) => (StatusCode::INTERNAL_SERVER_ERROR, Json(ApiError::new(e.to_string()))).into_response(),
    }
}

fn clean_path(path: &std::path::Path) -> String {
    let s = path.to_string_lossy();
    s.strip_prefix(r"\\?\").unwrap_or(&s).to_string()
}
