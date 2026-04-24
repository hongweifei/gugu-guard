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
use gugu_core::process::LogStream;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::path::PathBuf;

use crate::state::AppState;

// ── 响应类型 ──────────────────────────────────────────────

#[derive(Serialize)]
struct ApiSuccess {
    message: String,
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

#[derive(Serialize)]
struct GroupInfo {
    name: String,
    processes: Vec<String>,
}

#[derive(Serialize)]
struct BatchResponse {
    message: String,
    total: usize,
    succeeded: usize,
    failed: usize,
    results: Vec<BatchResult>,
}

#[derive(Serialize)]
struct BatchResult {
    process: String,
    success: bool,
    error: Option<String>,
}

// ── 路由注册 ──────────────────────────────────────────────

pub fn routes() -> Router<AppState> {
    Router::new()
        .route("/api/v1/processes", get(list_processes))
        .route(
            "/api/v1/processes/:name",
            get(get_process)
                .post(create_process)
                .put(update_process)
                .delete(delete_process),
        )
        .route("/api/v1/processes/:name/config", get(get_process_config))
        .route("/api/v1/processes/:name/start", post(start_process))
        .route("/api/v1/processes/:name/stop", post(stop_process))
        .route("/api/v1/processes/:name/restart", post(restart_process))
        .route(
            "/api/v1/processes/:name/logs",
            get(get_logs).delete(clear_logs),
        )
        .route("/api/v1/processes/:name/logs/download", get(download_logs))
        .route("/api/v1/processes/:name/health", post(check_health))
        .route("/api/v1/stats", get(get_stats))
        .route("/api/v1/groups", get(list_groups))
        .route("/api/v1/groups/:group", get(get_group))
        .route("/api/v1/groups/:group/start", post(start_group))
        .route("/api/v1/groups/:group/stop", post(stop_group))
        .route("/api/v1/groups/:group/restart", post(restart_group))
        .route("/api/v1/fs/browse", get(browse_fs))
        .route("/api/v1/reload", post(reload_config))
}

// ── 认证中间件 ──────────────────────────────────────────────

pub async fn auth_middleware(
    State(state): State<AppState>,
    req: Request,
    next: Next,
) -> axum::response::Response {
    let Some(ref expected_key) = state.api_key else {
        return next.run(req).await;
    };
    if expected_key.is_empty() {
        return next.run(req).await;
    }

    // Bearer token
    if let Some(key) = req
        .headers()
        .get("Authorization")
        .and_then(|v| v.to_str().ok())
        .and_then(|v| v.strip_prefix("Bearer "))
    {
        if constant_time_eq(key, expected_key) {
            return next.run(req).await;
        }
    }

    // Query token
    if let Some(query) = req.uri().query() {
        for pair in query.split('&') {
            if let Some(token) = pair.strip_prefix("token=") {
                if constant_time_eq(token, expected_key) {
                    return next.run(req).await;
                }
            }
        }
    }

    StatusCode::UNAUTHORIZED.into_response()
}

/// Constant-time 字符串比较，防止 timing attack。
fn constant_time_eq(a: &str, b: &str) -> bool {
    let a_bytes = a.as_bytes();
    let b_bytes = b.as_bytes();
    let mut result: u8 = if a_bytes.len() == b_bytes.len() { 0 } else { 0xff };
    let max_len = a_bytes.len().max(b_bytes.len());
    for i in 0..max_len {
        let x = a_bytes.get(i).copied().unwrap_or(0);
        let y = b_bytes.get(i).copied().unwrap_or(0);
        result |= x ^ y;
    }
    result == 0
}

// ── 请求体 ──────────────────────────────────────────────

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
    working_dir: Option<PathBuf>,
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
    #[serde(default)]
    stop_command: Option<String>,
    #[serde(default = "gugu_core::config::default_stop_timeout")]
    stop_timeout_secs: u64,
    #[serde(default)]
    health_check: Option<HealthCheckConfig>,
    #[serde(default)]
    unhealthy_restart: bool,
    #[serde(default)]
    depends_on: Vec<String>,
    #[serde(default)]
    group: Option<String>,
    #[serde(default)]
    max_log_size_mb: Option<u64>,
    stdout_log: Option<PathBuf>,
    stderr_log: Option<PathBuf>,
    #[serde(default)]
    start_now: bool,
    new_name: Option<String>,
}

impl From<CreateProcessRequest> for ProcessConfig {
    fn from(req: CreateProcessRequest) -> Self {
        Self {
            command: req.command,
            args: req.args,
            working_dir: req.working_dir,
            env: req.env,
            auto_start: req.auto_start,
            auto_restart: req.auto_restart,
            max_restarts: req.max_restarts,
            restart_delay_secs: req.restart_delay_secs,
            stop_command: req.stop_command,
            stop_timeout_secs: req.stop_timeout_secs,
            health_check: req.health_check,
            unhealthy_restart: req.unhealthy_restart,
            depends_on: req.depends_on,
            group: req.group,
            max_log_size_mb: req.max_log_size_mb,
            stdout_log: req.stdout_log,
            stderr_log: req.stderr_log,
        }
    }
}

// ── 辅助函数 ──────────────────────────────────────────────

fn api_res(
    result: gugu_core::Result<()>,
    success_msg: &str,
    error_status: StatusCode,
) -> axum::response::Response {
    match result {
        Ok(()) => (
            StatusCode::OK,
            Json(ApiSuccess {
                message: success_msg.to_string(),
            }),
        )
            .into_response(),
        Err(e) => (error_status, Json(ApiError::new(e.to_string()))).into_response(),
    }
}

fn clean_path(path: &std::path::Path) -> String {
    gugu_core::config::path_to_forward_slashes(&gugu_core::config::strip_unc_prefix(path))
}

// ── 处理函数 ──────────────────────────────────────────────

async fn list_processes(State(state): State<AppState>) -> impl IntoResponse {
    Json(state.manager.list_processes())
}

async fn get_process(
    State(state): State<AppState>,
    Path(name): Path<String>,
) -> impl IntoResponse {
    match state.manager.get_process_info(&name) {
        Some(info) => Json(Some(info)).into_response(),
        None => (
            StatusCode::NOT_FOUND,
            Json(ApiError::new("进程未找到")),
        )
            .into_response(),
    }
}

async fn create_process(
    State(state): State<AppState>,
    Path(name): Path<String>,
    Json(body): Json<CreateProcessRequest>,
) -> impl IntoResponse {
    let start_now = body.start_now;
    let config: ProcessConfig = body.into();
    match state.manager.add_process(name, config, start_now).await {
        Ok(()) => (
            StatusCode::CREATED,
            Json(ApiSuccess {
                message: "进程已添加".to_string(),
            }),
        )
            .into_response(),
        Err(e) => (
            StatusCode::BAD_REQUEST,
            Json(ApiError::new(e.to_string())),
        )
            .into_response(),
    }
}

async fn update_process(
    State(state): State<AppState>,
    Path(name): Path<String>,
    Json(body): Json<CreateProcessRequest>,
) -> impl IntoResponse {
    let new_name = body.new_name.clone();
    let config: ProcessConfig = body.into();
    let result = state.manager.update_process(&name, config, new_name, false).await;
    api_res(result, "进程已更新", StatusCode::BAD_REQUEST)
}

async fn delete_process(
    State(state): State<AppState>,
    Path(name): Path<String>,
) -> impl IntoResponse {
    let result = state.manager.remove_process(&name).await;
    api_res(result, "进程已移除", StatusCode::NOT_FOUND)
}

async fn get_process_config(
    State(state): State<AppState>,
    Path(name): Path<String>,
) -> impl IntoResponse {
    match state.manager.get_process_config(&name).await {
        Ok(Some(config)) => Json(config).into_response(),
        Ok(None) => (
            StatusCode::NOT_FOUND,
            Json(ApiError::new("进程未找到")),
        )
            .into_response(),
        Err(e) => (
            StatusCode::NOT_FOUND,
            Json(ApiError::new(e.to_string())),
        )
            .into_response(),
    }
}

async fn start_process(
    State(state): State<AppState>,
    Path(name): Path<String>,
) -> impl IntoResponse {
    let result = state.manager.start_process(&name).await;
    api_res(result, "进程已启动", StatusCode::BAD_REQUEST)
}

async fn stop_process(
    State(state): State<AppState>,
    Path(name): Path<String>,
) -> impl IntoResponse {
    let result = state.manager.stop_process(&name).await;
    api_res(result, "进程已停止", StatusCode::BAD_REQUEST)
}

async fn restart_process(
    State(state): State<AppState>,
    Path(name): Path<String>,
) -> impl IntoResponse {
    let result = state.manager.restart_process(&name).await;
    api_res(result, "进程已重启", StatusCode::BAD_REQUEST)
}

async fn get_logs(
    State(state): State<AppState>,
    Path(name): Path<String>,
    Query(query): Query<LogQuery>,
) -> impl IntoResponse {
    match state.manager.get_process_logs(&name, query.lines).await {
        Ok(logs) => Json(logs).into_response(),
        Err(e) => (
            StatusCode::NOT_FOUND,
            Json(ApiError::new(e.to_string())),
        )
            .into_response(),
    }
}

async fn clear_logs(
    State(state): State<AppState>,
    Path(name): Path<String>,
) -> impl IntoResponse {
    let result = state.manager.clear_process_logs(&name).await;
    api_res(result, "日志已清空", StatusCode::NOT_FOUND)
}

async fn download_logs(
    State(state): State<AppState>,
    Path(name): Path<String>,
    Query(query): Query<LogQuery>,
) -> impl IntoResponse {
    match state.manager.get_process_logs(&name, query.lines).await {
        Ok(logs) => {
            let mut content = String::with_capacity(logs.len() * 128);
            for entry in &logs {
                let time = entry.timestamp.format("%Y-%m-%d %H:%M:%S%.3f");
                let prefix = match &entry.stream {
                    LogStream::Stdout => "OUT",
                    LogStream::Stderr => "ERR",
                    _ => "???",
                };
                content.push_str(&format!("[{time}] [{prefix}] {}\n", entry.line));
            }
            (
                StatusCode::OK,
                [
                    ("content-type", "text/plain; charset=utf-8".to_string()),
                    (
                        "content-disposition",
                        format!("attachment; filename=\"{name}.log\""),
                    ),
                ],
                content,
            )
                .into_response()
        }
        Err(e) => (
            StatusCode::NOT_FOUND,
            Json(ApiError::new(e.to_string())),
        )
            .into_response(),
    }
}

async fn check_health(
    Path(name): Path<String>,
    State(state): State<AppState>,
) -> impl IntoResponse {
    match state.manager.check_process_health(&name).await {
        Ok(healthy) => Json(serde_json::json!({
            "healthy": healthy,
            "message": if healthy { "健康检查通过" } else { "健康检查失败" }
        }))
        .into_response(),
        Err(e) => (
            StatusCode::BAD_REQUEST,
            Json(ApiError::new(e.to_string())),
        )
            .into_response(),
    }
}

async fn get_stats(State(state): State<AppState>) -> impl IntoResponse {
    use gugu_core::process::ProcessStatus;

    let processes = state.manager.list_processes();
    let total = processes.len();
    let running = processes
        .iter()
        .filter(|p| matches!(p.status, ProcessStatus::Running))
        .count();
    let stopped = processes
        .iter()
        .filter(|p| matches!(p.status, ProcessStatus::Stopped))
        .count();
    let failed = processes
        .iter()
        .filter(|p| matches!(p.status, ProcessStatus::Failed(_)))
        .count();
    Json(StatsResponse {
        total,
        running,
        stopped,
        failed,
    })
}

#[derive(Deserialize)]
struct BrowseQuery {
    path: Option<String>,
}

async fn browse_fs(Query(query): Query<BrowseQuery>) -> impl IntoResponse {
    let raw = query.path.unwrap_or_else(|| ".".into());
    let dir = PathBuf::from(&raw);

    let dir = {
        let resolved = if dir.is_relative() {
            std::env::current_dir().unwrap_or_default().join(&dir)
        } else {
            dir
        };
        match std::fs::canonicalize(&resolved) {
            Ok(p) => gugu_core::config::strip_unc_prefix(&p),
            Err(e) => {
                return (
                    StatusCode::BAD_REQUEST,
                    Json(ApiError::new(format!("路径不存在: {e}"))),
                )
                    .into_response()
            }
        }
    };

    if !dir.is_dir() {
        return (
            StatusCode::BAD_REQUEST,
            Json(ApiError::new("路径不是目录")),
        )
            .into_response();
    }

    let path_str = clean_path(&dir);
    let parent = dir.parent().map(clean_path);

    let read_dir = match std::fs::read_dir(&dir) {
        Ok(rd) => rd,
        Err(e) => {
            return (
                StatusCode::BAD_REQUEST,
                Json(ApiError::new(e.to_string())),
            )
                .into_response()
        }
    };

    let mut entries: Vec<FsEntry> = Vec::with_capacity(256);
    for entry in read_dir.flatten() {
        let is_dir = entry.file_type().map(|t| t.is_dir()).unwrap_or(false);
        let name = entry.file_name().to_string_lossy().to_string();
        let path = clean_path(&dir.join(&name));
        entries.push(FsEntry { name, path, is_dir });
    }

    entries.sort_by(|a, b| {
        b.is_dir
            .cmp(&a.is_dir)
            .then(a.name.to_lowercase().cmp(&b.name.to_lowercase()))
    });

    Json(BrowseResponse {
        path: path_str,
        parent,
        entries,
    })
    .into_response()
}

async fn reload_config(State(state): State<AppState>) -> impl IntoResponse {
    let result = state.manager.reload_from_file().await;
    api_res(result, "配置已重新加载", StatusCode::INTERNAL_SERVER_ERROR)
}

// ── 进程组 ──────────────────────────────────────────────────

fn group_members<'a>(processes: &'a [gugu_core::process::ProcessInfo], group: &str) -> Vec<&'a gugu_core::process::ProcessInfo> {
    processes.iter().filter(|p| p.group.as_deref() == Some(group)).collect()
}

async fn list_groups(State(state): State<AppState>) -> impl IntoResponse {
    let processes = state.manager.list_processes();
    let mut groups: std::collections::BTreeMap<String, Vec<String>> = std::collections::BTreeMap::new();
    for p in &processes {
        if let Some(ref g) = p.group {
            groups.entry(g.clone()).or_default().push(p.name.clone());
        }
    }
    let result: Vec<GroupInfo> = groups
        .into_iter()
        .map(|(name, processes)| GroupInfo { name, processes })
        .collect();
    Json(result)
}

async fn get_group(
    State(state): State<AppState>,
    Path(group): Path<String>,
) -> impl IntoResponse {
    let processes = state.manager.list_processes();
    let members = group_members(&processes, &group);
    if members.is_empty() {
        return (
            StatusCode::NOT_FOUND,
            Json(ApiError::new("进程组未找到")),
        )
            .into_response();
    }
    let names: Vec<String> = members.iter().map(|p| p.name.clone()).collect();
    Json(GroupInfo {
        name: group,
        processes: names,
    })
    .into_response()
}

async fn start_group(
    State(state): State<AppState>,
    Path(group): Path<String>,
) -> impl IntoResponse {
    let processes = state.manager.list_processes();
    let members = group_members(&processes, &group);
    if members.is_empty() {
        return (
            StatusCode::NOT_FOUND,
            Json(ApiError::new("进程组未找到")),
        )
            .into_response();
    }
    let mut results = Vec::with_capacity(members.len());
    for p in &members {
        let result = state.manager.start_process(&p.name).await;
        results.push(BatchResult {
            process: p.name.clone(),
            success: result.is_ok(),
            error: result.err().map(|e| e.to_string()),
        });
    }
    let succeeded = results.iter().filter(|r| r.success).count();
    let failed = results.len() - succeeded;
    Json(BatchResponse {
        message: format!("进程组 '{group}' 启动完成"),
        total: results.len(),
        succeeded,
        failed,
        results,
    })
    .into_response()
}

async fn stop_group(
    State(state): State<AppState>,
    Path(group): Path<String>,
) -> impl IntoResponse {
    let processes = state.manager.list_processes();
    let members = group_members(&processes, &group);
    if members.is_empty() {
        return (
            StatusCode::NOT_FOUND,
            Json(ApiError::new("进程组未找到")),
        )
            .into_response();
    }
    let mut results = Vec::with_capacity(members.len());
    for p in members.iter().rev() {
        let result = state.manager.stop_process(&p.name).await;
        results.push(BatchResult {
            process: p.name.clone(),
            success: result.is_ok(),
            error: result.err().map(|e| e.to_string()),
        });
    }
    let succeeded = results.iter().filter(|r| r.success).count();
    let failed = results.len() - succeeded;
    Json(BatchResponse {
        message: format!("进程组 '{group}' 停止完成"),
        total: results.len(),
        succeeded,
        failed,
        results,
    })
    .into_response()
}

async fn restart_group(
    State(state): State<AppState>,
    Path(group): Path<String>,
) -> impl IntoResponse {
    let processes = state.manager.list_processes();
    let members = group_members(&processes, &group);
    if members.is_empty() {
        return (
            StatusCode::NOT_FOUND,
            Json(ApiError::new("进程组未找到")),
        )
            .into_response();
    }
    let mut results = Vec::with_capacity(members.len());
    for p in &members {
        let result = state.manager.restart_process(&p.name).await;
        results.push(BatchResult {
            process: p.name.clone(),
            success: result.is_ok(),
            error: result.err().map(|e| e.to_string()),
        });
    }
    let succeeded = results.iter().filter(|r| r.success).count();
    let failed = results.len() - succeeded;
    Json(BatchResponse {
        message: format!("进程组 '{group}' 重启完成"),
        total: results.len(),
        succeeded,
        failed,
        results,
    })
    .into_response()
}
