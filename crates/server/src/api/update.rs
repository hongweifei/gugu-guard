//! 自更新端点：查看状态、立即检查、一键安装、切换自动更新策略。
//!
//! 所有写操作都落在「本机正在运行的这个二进制」上：检查与安装共用
//! [`gugu_core::update::UpdateService`]，安装的收尾由替身进程完成，
//! 因此接口在把请求交给后台任务之后立刻返回，随后的重启由前端自行重连。

use axum::{
    extract::State,
    http::StatusCode,
    response::{IntoResponse, Response},
    routing::{get, post},
    Json, Router,
};
use serde::Deserialize;

use gugu_core::config::AutoUpdate;

use crate::state::AppState;

use super::types::{ApiError, ApiSuccess};

pub(super) fn routes() -> Router<AppState> {
    Router::new()
        .route("/api/v1/update", get(get_update))
        .route("/api/v1/update/check", post(check_update))
        .route("/api/v1/update/apply", post(apply_update))
        .route("/api/v1/update/settings", post(update_settings))
}

pub(super) async fn get_update(State(state): State<AppState>) -> Response {
    match state.updates.as_ref() {
        Some(service) => Json(service.snapshot()).into_response(),
        None => unavailable(),
    }
}

pub(super) async fn check_update(State(state): State<AppState>) -> Response {
    let Some(service) = state.updates.as_ref() else {
        return unavailable();
    };
    match service.check().await {
        Ok(_) => Json(service.snapshot()).into_response(),
        Err(error) => bad_gateway(error),
    }
}

#[derive(Debug, Deserialize)]
pub(super) struct ApplyRequest {
    /// 期望安装的版本；缺省按最近一次检查的结果安装。
    #[serde(default)]
    version: Option<String>,
}

pub(super) async fn apply_update(
    State(state): State<AppState>,
    Json(request): Json<ApplyRequest>,
) -> Response {
    let Some(service) = state.updates.as_ref() else {
        return unavailable();
    };
    match service.start_apply(request.version) {
        Ok(()) => {
            tracing::info!("已接受更新请求，替换后守护进程将重启");
            (
                StatusCode::ACCEPTED,
                Json(ApiSuccess {
                    message: "更新已开始，替换完成后守护进程会重启".to_string(),
                }),
            )
                .into_response()
        }
        Err(error) => (StatusCode::CONFLICT, Json(ApiError::new(error))).into_response(),
    }
}

#[derive(Debug, Deserialize)]
pub(super) struct SettingsRequest {
    /// 自动更新策略：off / check / install。
    auto: String,
}

pub(super) async fn update_settings(
    State(state): State<AppState>,
    Json(request): Json<SettingsRequest>,
) -> Response {
    let Some(service) = state.updates.as_ref() else {
        return unavailable();
    };
    let Some(auto) = AutoUpdate::parse(&request.auto) else {
        return (
            StatusCode::BAD_REQUEST,
            Json(ApiError::new(format!(
                "自动更新策略只能是 off / check / install，收到 {:?}",
                request.auto
            ))),
        )
            .into_response();
    };
    match service.set_auto(auto) {
        Ok(()) => Json(service.snapshot()).into_response(),
        Err(error) => (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(ApiError::new(error)),
        )
            .into_response(),
    }
}

fn unavailable() -> Response {
    (
        StatusCode::NOT_IMPLEMENTED,
        Json(ApiError::new("当前进程没有启用更新服务")),
    )
        .into_response()
}

fn bad_gateway(error: String) -> Response {
    (
        StatusCode::BAD_GATEWAY,
        Json(ApiError::new(format!("检查更新失败: {error}"))),
    )
        .into_response()
}
