//! 杂项端点：统计概览与配置热重载。

use axum::{
    extract::State,
    http::StatusCode,
    response::{IntoResponse, Response},
    Json,
};

use crate::state::AppState;

use super::api_res;
use super::types::{ApiError, ApiSuccess, StatsResponse};

pub(super) async fn get_stats(State(state): State<AppState>) -> impl IntoResponse {
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
    Json(StatsResponse::new(total, running, stopped, failed))
}

pub(super) async fn reload_config(State(state): State<AppState>) -> impl IntoResponse {
    let result = state.manager.reload_from_file().await;
    api_res(result, "配置已重新加载", StatusCode::INTERNAL_SERVER_ERROR)
}

/// 请求守护进程优雅停止（先停所有子进程，再退出）。
/// 响应先回给调用方，真正的退出由 CLI 侧的信号循环完成。
pub(super) async fn shutdown(State(state): State<AppState>) -> Response {
    match state.shutdown_tx.as_ref() {
        Some(tx) => {
            if tx.try_send(()).is_ok() {
                Json(ApiSuccess {
                    message: "守护进程正在优雅停止".to_string(),
                })
                .into_response()
            } else {
                (
                    StatusCode::SERVICE_UNAVAILABLE,
                    Json(ApiError::new("停止请求未能送达（通道已满或已关闭）")),
                )
                    .into_response()
            }
        }
        None => (
            StatusCode::NOT_IMPLEMENTED,
            Json(ApiError::new("当前进程未启用 API 停止（请直接终止进程或停止服务）")),
        )
            .into_response(),
    }
}
