//! 文件管理接口：列出、预览、下载、上传、保存、新建、改名、复制与删除。
//!
//! 路径统一由 `gugu_core::fs::Scope` 解析与约束：越界与只读返回 403，不存在返回 404，
//! 参数不合法返回 400。列表与写操作放到阻塞线程池，避免大目录或递归复制占住运行时。
//! 写操作由审计中间件统一记录。

mod read;
mod transfer;
mod types;
mod write;

use axum::extract::{Query, State};
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::routing::{delete, get, post, put};
use axum::{Json, Router};
use gugu_core::error::GuguError;
use gugu_core::fs::Scope;
use std::net::IpAddr;

use super::types::ApiError;
use crate::state::AppState;

use read::{download_fs, read_fs};
use transfer::upload_fs;
use types::{FsRoots, FsSuccess, PathQuery};
use write::{copy_fs, mkdir_fs, remove_fs, rename_fs, touch_fs, write_fs};

pub(super) fn routes() -> Router<AppState> {
    Router::new()
        .route("/api/v1/fs/roots", get(roots_fs))
        .route("/api/v1/fs/list", get(list_fs))
        .route("/api/v1/fs/stat", get(stat_fs))
        .route("/api/v1/fs/read", get(read_fs))
        .route("/api/v1/fs/download", get(download_fs))
        .route("/api/v1/fs/upload", post(upload_fs))
        .route("/api/v1/fs/write", put(write_fs))
        .route("/api/v1/fs/mkdir", post(mkdir_fs))
        .route("/api/v1/fs/touch", post(touch_fs))
        .route("/api/v1/fs/rename", post(rename_fs))
        .route("/api/v1/fs/copy", post(copy_fs))
        .route("/api/v1/fs", delete(remove_fs))
}

/// 根目录清单：前端据此渲染根切换；空列表表示没有配置白名单。
async fn roots_fs(State(state): State<AppState>) -> Response {
    let roots = state.fs.roots().iter().map(|root| forward(root)).collect();
    Json(FsRoots {
        roots,
        writable: state.fs.writable(),
    })
    .into_response()
}

async fn list_fs(State(state): State<AppState>, Query(query): Query<PathQuery>) -> Response {
    let scope = state.fs.clone();
    let target = query
        .path
        .map(|path| path.trim().to_string())
        .filter(|path| !path.is_empty())
        .unwrap_or_else(|| forward(&scope.default_dir()));
    match run(scope, move |scope| gugu_core::fs::list(scope, &target)).await {
        Ok(listing) => Json(listing).into_response(),
        Err(error) => failure(error),
    }
}

async fn stat_fs(State(state): State<AppState>, Query(query): Query<PathQuery>) -> Response {
    let Some(target) = query.path.filter(|path| !path.trim().is_empty()) else {
        return failure(GuguError::InvalidPath("缺少 path 参数".to_string()));
    };
    match run(state.fs.clone(), move |scope| {
        gugu_core::fs::stat(scope, &target)
    })
    .await
    {
        Ok(entry) => Json(entry).into_response(),
        Err(error) => failure(error),
    }
}

/// 把同步文件操作移到阻塞线程池。
pub(super) async fn run<T, F>(scope: Scope, job: F) -> gugu_core::Result<T>
where
    T: Send + 'static,
    F: FnOnce(&Scope) -> gugu_core::Result<T> + Send + 'static,
{
    tokio::task::spawn_blocking(move || job(&scope))
        .await
        .map_err(|error| GuguError::InvalidPath(format!("文件操作被中断: {error}")))?
}

/// 启动门槛：开启写权限、监听非回环地址、又没有 API Key 时拒绝启动。
///
/// # Errors
/// 返回人类可读的拒绝原因。
pub fn check_exposure(addr: IpAddr, api_key: Option<&str>, writable: bool) -> Result<(), String> {
    let has_key = api_key.is_some_and(|key| !key.trim().is_empty());
    if !writable || addr.is_loopback() || has_key {
        return Ok(());
    }
    Err(format!(
        "已开启文件管理写权限（daemon.web.fs_write = true），但监听地址 {addr} 不是回环地址且未配置 API Key。\
         写权限等于把文件系统交给任何能访问面板的人：请配置 daemon.api_key（或 GUGU_API_KEY），\
         或只监听 127.0.0.1，或把 daemon.web.fs_write 设为 false。"
    ))
}

/// 统一的错误出口：状态码按错误类别给，正文保持一句话。
pub(super) fn failure(error: GuguError) -> Response {
    let status = match &error {
        GuguError::ReadOnly | GuguError::PathDenied(_) => StatusCode::FORBIDDEN,
        GuguError::NotFound(_) => StatusCode::NOT_FOUND,
        GuguError::InvalidPath(_) => StatusCode::BAD_REQUEST,
        _ => StatusCode::INTERNAL_SERVER_ERROR,
    };
    (status, Json(ApiError::new(error.to_string()))).into_response()
}

pub(super) fn success(message: impl Into<String>, path: impl Into<String>) -> Response {
    Json(FsSuccess::new(message, path)).into_response()
}

pub(super) fn forward(path: &std::path::Path) -> String {
    gugu_core::config::path_to_forward_slashes(path)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn loopback_or_keyed_binds_are_allowed() {
        let loopback: IpAddr = "127.0.0.1".parse().expect("回环地址");
        assert!(check_exposure(loopback, None, true).is_ok());
        let public: IpAddr = "0.0.0.0".parse().expect("通配地址");
        assert!(check_exposure(public, Some("secret"), true).is_ok());
        assert!(
            check_exposure(public, None, false).is_ok(),
            "只读模式不做限制"
        );
    }

    #[test]
    fn public_writable_bind_without_a_key_is_rejected() {
        let public: IpAddr = "192.168.1.10".parse().expect("内网地址");
        let error = check_exposure(public, None, true).expect_err("应拒绝");
        assert!(error.contains("fs_write"), "{error}");
        let error = check_exposure(public, Some("  "), true).expect_err("空白 Key 不算配置");
        assert!(error.contains("api_key"), "{error}");
    }

    #[test]
    fn errors_map_to_distinct_status_codes() {
        assert_eq!(failure(GuguError::ReadOnly).status(), StatusCode::FORBIDDEN);
        assert_eq!(
            failure(GuguError::NotFound("x".into())).status(),
            StatusCode::NOT_FOUND
        );
        assert_eq!(
            failure(GuguError::InvalidPath("x".into())).status(),
            StatusCode::BAD_REQUEST
        );
        assert_eq!(
            failure(GuguError::Io(std::io::Error::other("x"))).status(),
            StatusCode::INTERNAL_SERVER_ERROR
        );
    }
}
