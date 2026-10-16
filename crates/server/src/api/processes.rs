//! 进程资源：增删改查、启动/停止/重启、手动健康检查。

use axum::{
    extract::{Path, State},
    http::StatusCode,
    response::{IntoResponse, Response},
    Json,
};

use crate::state::AppState;

use super::api_res;
use super::types::{ApiError, ApiSuccess, CreateProcessRequest};

pub(super) async fn list_processes(State(state): State<AppState>) -> impl IntoResponse {
    Json(state.manager.list_processes())
}

pub(super) async fn get_process(
    State(state): State<AppState>,
    Path(name): Path<String>,
) -> Response {
    match state.manager.get_process_info(&name) {
        Some(info) => Json(Some(info)).into_response(),
        None => (StatusCode::NOT_FOUND, Json(ApiError::new("进程未找到"))).into_response(),
    }
}

pub(super) async fn create_process(
    State(state): State<AppState>,
    Path(name): Path<String>,
    Json(body): Json<CreateProcessRequest>,
) -> Response {
    let (config, start_now, _new_name) = body.into_config();
    match state.manager.add_process(name, config, start_now).await {
        Ok(()) => (
            StatusCode::CREATED,
            Json(ApiSuccess {
                message: "进程已添加".to_string(),
            }),
        )
            .into_response(),
        Err(e) => (StatusCode::BAD_REQUEST, Json(ApiError::new(e.to_string()))).into_response(),
    }
}

pub(super) async fn update_process(
    State(state): State<AppState>,
    Path(name): Path<String>,
    Json(body): Json<CreateProcessRequest>,
) -> Response {
    let (config, _start_now, new_name) = body.into_config();
    let result = state
        .manager
        .update_process(&name, config, new_name, false)
        .await;
    api_res(result, "进程已更新", StatusCode::BAD_REQUEST)
}

pub(super) async fn delete_process(
    State(state): State<AppState>,
    Path(name): Path<String>,
) -> Response {
    let result = state.manager.remove_process(&name).await;
    api_res(result, "进程已移除", StatusCode::NOT_FOUND)
}

pub(super) async fn get_process_config(
    State(state): State<AppState>,
    Path(name): Path<String>,
) -> Response {
    match state.manager.get_process_config(&name).await {
        Ok(Some(config)) => Json(config).into_response(),
        Ok(None) => (StatusCode::NOT_FOUND, Json(ApiError::new("进程未找到"))).into_response(),
        Err(e) => (StatusCode::NOT_FOUND, Json(ApiError::new(e.to_string()))).into_response(),
    }
}

pub(super) async fn start_process(
    State(state): State<AppState>,
    Path(name): Path<String>,
) -> Response {
    let result = state.manager.start_process(&name).await;
    api_res(result, "进程已启动", StatusCode::BAD_REQUEST)
}

pub(super) async fn stop_process(
    State(state): State<AppState>,
    Path(name): Path<String>,
) -> Response {
    let result = state.manager.stop_process(&name).await;
    api_res(result, "进程已停止", StatusCode::BAD_REQUEST)
}

pub(super) async fn restart_process(
    State(state): State<AppState>,
    Path(name): Path<String>,
) -> Response {
    let result = state.manager.restart_process(&name).await;
    api_res(result, "进程已重启", StatusCode::BAD_REQUEST)
}

pub(super) async fn check_health(
    Path(name): Path<String>,
    State(state): State<AppState>,
) -> Response {
    match state.manager.check_process_health(&name).await {
        Ok(healthy) => Json(serde_json::json!({
            "healthy": healthy,
            "message": if healthy { "健康检查通过" } else { "健康检查失败" }
        }))
        .into_response(),
        Err(e) => (StatusCode::BAD_REQUEST, Json(ApiError::new(e.to_string()))).into_response(),
    }
}
