//! 杂项端点：统计概览与配置热重载。

use axum::{
    extract::State,
    http::StatusCode,
    response::IntoResponse,
    Json,
};

use crate::state::AppState;

use super::api_res;
use super::types::StatsResponse;

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
