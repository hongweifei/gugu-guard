//! REST API：路由注册与共享辅助。子模块按资源划分 handler，
//! 类型集中在 [`types`]，认证在 [`auth`]；对外仅暴露 [`routes`] 与 [`auth_middleware`]。

mod auth;
mod fs;
mod groups;
mod logs;
mod misc;
mod processes;
mod types;

use axum::{
    routing::{get, post},
    Router,
};

use crate::state::AppState;

pub use auth::auth_middleware;

// ── 路由注册 ──────────────────────────────────────────────

pub fn routes() -> Router<AppState> {
    Router::new()
        .route("/api/v1/processes", get(processes::list_processes))
        .route(
            "/api/v1/processes/{name}",
            get(processes::get_process)
                .post(processes::create_process)
                .put(processes::update_process)
                .delete(processes::delete_process),
        )
        .route("/api/v1/processes/{name}/config", get(processes::get_process_config))
        .route("/api/v1/processes/{name}/start", post(processes::start_process))
        .route("/api/v1/processes/{name}/stop", post(processes::stop_process))
        .route("/api/v1/processes/{name}/restart", post(processes::restart_process))
        .route(
            "/api/v1/processes/{name}/logs",
            get(logs::get_logs).delete(logs::clear_logs),
        )
        .route("/api/v1/processes/{name}/logs/download", get(logs::download_logs))
        .route("/api/v1/processes/{name}/health", post(processes::check_health))
        .route("/api/v1/stats", get(misc::get_stats))
        .route("/api/v1/groups", get(groups::list_groups))
        .route("/api/v1/groups/{group}", get(groups::get_group))
        .route("/api/v1/groups/{group}/start", post(groups::start_group))
        .route("/api/v1/groups/{group}/stop", post(groups::stop_group))
        .route("/api/v1/groups/{group}/restart", post(groups::restart_group))
        .route("/api/v1/fs/browse", get(fs::browse_fs))
        .route("/api/v1/reload", post(misc::reload_config))
}

// ── 共享辅助 ──────────────────────────────────────────────

pub(super) fn api_res(
    result: gugu_core::Result<()>,
    success_msg: &str,
    error_status: axum::http::StatusCode,
) -> axum::response::Response {
    use axum::{http::StatusCode, response::IntoResponse, Json};
    match result {
        Ok(()) => (
            StatusCode::OK,
            Json(types::ApiSuccess {
                message: success_msg.to_string(),
            }),
        )
            .into_response(),
        Err(e) => (error_status, Json(types::ApiError::new(e.to_string()))).into_response(),
    }
}

pub(super) fn clean_path(path: &std::path::Path) -> String {
    gugu_core::config::path_to_forward_slashes(&gugu_core::config::strip_unc_prefix(path))
}
