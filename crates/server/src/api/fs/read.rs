//! 文本预览与原始下载。
//!
//! 下载走流式：文件不整块读进内存，大文件的开销只跟缓冲区有关。

use axum::body::Body;
use axum::extract::{Query, State};
use axum::http::{header, StatusCode};
use axum::response::{IntoResponse, Response};
use gugu_core::error::GuguError;
use gugu_core::fs;
use tokio_util::io::ReaderStream;

use super::types::PathQuery;
use super::{disposition, failure, run};
use crate::state::AppState;

pub(super) async fn read_fs(
    State(state): State<AppState>,
    Query(query): Query<PathQuery>,
) -> Response {
    let Some(target) = query.path.filter(|path| !path.trim().is_empty()) else {
        return failure(GuguError::InvalidPath("缺少 path 参数".to_string()));
    };
    let limit = query.limit.unwrap_or(0);
    match run(state.fs.clone(), move |scope| {
        fs::read(scope, &target, limit)
    })
    .await
    {
        Ok(file) => json_body(file),
        Err(error) => failure(error),
    }
}

pub(super) async fn download_fs(
    State(state): State<AppState>,
    Query(query): Query<PathQuery>,
) -> Response {
    let Some(target) = query.path.filter(|path| !path.trim().is_empty()) else {
        return failure(GuguError::InvalidPath("缺少 path 参数".to_string()));
    };
    let path = match state.fs.resolve(&target) {
        Ok(path) => path,
        Err(error) => return failure(error),
    };
    let meta = match tokio::fs::metadata(&path).await {
        Ok(meta) => meta,
        Err(error) => return failure(GuguError::Io(error)),
    };
    if meta.is_dir() {
        return failure(GuguError::InvalidPath(format!(
            "目录不能直接下载: {}",
            path.display()
        )));
    }
    let file = match tokio::fs::File::open(&path).await {
        Ok(file) => file,
        Err(error) => return failure(GuguError::Io(error)),
    };

    let name = path
        .file_name()
        .map(|name| name.to_string_lossy().to_string())
        .unwrap_or_else(|| "download".to_string());
    let mime = mime_guess::from_path(&path).first_or_octet_stream();
    Response::builder()
        .header(header::CONTENT_TYPE, mime.as_ref())
        .header(header::CONTENT_LENGTH, meta.len())
        .header(header::CONTENT_DISPOSITION, disposition(&name))
        .body(Body::from_stream(ReaderStream::new(file)))
        .unwrap_or_else(|_| (StatusCode::INTERNAL_SERVER_ERROR, "构造下载响应失败").into_response())
}

fn json_body(file: fs::TextFile) -> Response {
    (StatusCode::OK, axum::Json(file)).into_response()
}
