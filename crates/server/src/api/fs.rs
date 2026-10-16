//! 文件系统浏览：供 WebUI 表单快速选择工作目录/可执行文件。

use axum::{
    extract::Query,
    http::StatusCode,
    response::{IntoResponse, Response},
    Json,
};
use std::path::PathBuf;

use super::clean_path;
use super::types::{ApiError, BrowseQuery, BrowseResponse, FsEntry};

pub(super) async fn browse_fs(Query(query): Query<BrowseQuery>) -> Response {
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
        return (StatusCode::BAD_REQUEST, Json(ApiError::new("路径不是目录"))).into_response();
    }

    let path_str = clean_path(&dir);
    let parent = dir.parent().map(clean_path);

    let read_dir = match std::fs::read_dir(&dir) {
        Ok(rd) => rd,
        Err(e) => {
            return (StatusCode::BAD_REQUEST, Json(ApiError::new(e.to_string()))).into_response()
        }
    };

    let mut entries: Vec<FsEntry> = Vec::with_capacity(256);
    for entry in read_dir.flatten() {
        let is_dir = entry.file_type().map(|t| t.is_dir()).unwrap_or(false);
        let name = entry.file_name().to_string_lossy().to_string();
        let path = clean_path(&dir.join(&name));
        entries.push(FsEntry::new(name, path, is_dir));
    }

    entries.sort_by(|a, b| {
        b.is_dir
            .cmp(&a.is_dir)
            .then(a.name.to_lowercase().cmp(&b.name.to_lowercase()))
    });

    Json(BrowseResponse::new(path_str, parent, entries)).into_response()
}
