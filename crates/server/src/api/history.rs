//! 历史读数：进程指标时序、退出日志与统计、整机带宽。
//!
//! 这些接口直接读 [`gugu_core::history::History`]，不经过 actor 命令通道，
//! 因此轮询再频繁也不会阻塞启停操作。

use axum::{
    extract::{Path, Query, State},
    http::StatusCode,
    response::{IntoResponse, Response},
    Json,
};
use chrono::Utc;
use serde::Deserialize;

use crate::state::AppState;

#[derive(Debug, Deserialize)]
pub(super) struct RangeQuery {
    /// 时间窗口（分钟），默认 1 小时。
    #[serde(default = "default_minutes")]
    minutes: u64,
}

fn default_minutes() -> u64 {
    60
}

#[derive(Debug, Deserialize)]
pub(super) struct ExitQuery {
    #[serde(default = "default_minutes")]
    minutes: u64,
    #[serde(default = "default_limit")]
    limit: usize,
}

fn default_limit() -> usize {
    50
}

/// 指标序列最长只保留 24 小时，超出窗口的请求按上限截断。
fn clamp_minutes(minutes: u64) -> u64 {
    minutes.clamp(1, 24 * 60)
}

pub(super) async fn process_history(
    State(state): State<AppState>,
    Path(name): Path<String>,
    Query(query): Query<RangeQuery>,
) -> Response {
    if state.manager.get_process_info(&name).is_none() {
        return (
            StatusCode::NOT_FOUND,
            Json(super::types::ApiError::new("进程未找到")),
        )
            .into_response();
    }
    let minutes = clamp_minutes(query.minutes);
    let points = state.manager.history().process_points(&name, minutes);
    Json(serde_json::json!({
        "process": name,
        "minutes": minutes,
        "points": points,
    }))
    .into_response()
}

pub(super) async fn process_exits(
    State(state): State<AppState>,
    Path(name): Path<String>,
    Query(query): Query<ExitQuery>,
) -> Response {
    let Some(info) = state.manager.get_process_info(&name) else {
        return (
            StatusCode::NOT_FOUND,
            Json(super::types::ApiError::new("进程未找到")),
        )
            .into_response();
    };
    let minutes = clamp_minutes(query.minutes);
    let limit = query.limit.clamp(1, 200);
    let history = state.manager.history();
    let events = history.events(&name, limit);
    let stats = history.stats(&name, minutes, Utc::now(), info.started_at);
    Json(serde_json::json!({
        "process": name,
        "minutes": minutes,
        "events": events,
        "stats": stats,
    }))
    .into_response()
}

pub(super) async fn system_history(
    State(state): State<AppState>,
    Query(query): Query<RangeQuery>,
) -> Response {
    let minutes = clamp_minutes(query.minutes);
    let history = state.manager.history();
    Json(serde_json::json!({
        "minutes": minutes,
        "latest": history.latest_traffic(),
        "points": history.traffic_points(minutes),
    }))
    .into_response()
}
