//! 进程日志：查询、清空、下载（文本导出）。

use axum::{
    extract::{Path, Query, State},
    http::StatusCode,
    response::{IntoResponse, Response},
    Json,
};
use gugu_core::process::LogStream;

use crate::state::AppState;

use super::api_res;
use super::types::{ApiError, LogQuery};

pub(super) async fn get_logs(
    State(state): State<AppState>,
    Path(name): Path<String>,
    Query(query): Query<LogQuery>,
) -> Response {
    match state.manager.get_process_logs(&name, query.lines).await {
        Ok(logs) => Json(logs).into_response(),
        Err(e) => (StatusCode::NOT_FOUND, Json(ApiError::new(e.to_string()))).into_response(),
    }
}

pub(super) async fn clear_logs(
    State(state): State<AppState>,
    Path(name): Path<String>,
) -> Response {
    let result = state.manager.clear_process_logs(&name).await;
    api_res(result, "日志已清空", StatusCode::NOT_FOUND)
}

pub(super) async fn download_logs(
    State(state): State<AppState>,
    Path(name): Path<String>,
    Query(query): Query<LogQuery>,
) -> Response {
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
        Err(e) => (StatusCode::NOT_FOUND, Json(ApiError::new(e.to_string()))).into_response(),
    }
}
