//! 解压归档：zip、tar 与 tar.gz / tgz。
//!
//! 解压是整块阻塞操作，放到阻塞线程池；目标目录缺省为归档旁的同名目录，
//! 已存在时必须显式声明覆盖，覆盖是整目录替换。

use axum::extract::{Query, State};
use axum::response::{IntoResponse, Response};
use axum::Json;
use gugu_core::error::GuguError;

use super::types::{ExtractBody, FsSuccess, PathQuery};
use super::{failure, run};
use crate::state::AppState;

pub(super) async fn extract_fs(
    State(state): State<AppState>,
    Query(query): Query<PathQuery>,
    Json(body): Json<ExtractBody>,
) -> Response {
    let path = query
        .path
        .map(|path| path.trim().to_string())
        .filter(|path| !path.is_empty())
        .unwrap_or(body.path);
    if path.trim().is_empty() {
        return failure(GuguError::InvalidPath("缺少 path 参数".to_string()));
    }
    let dest = body
        .dest
        .map(|dest| dest.trim().to_string())
        .filter(|dest| !dest.is_empty());
    let overwrite = body.overwrite;
    let collapse = body.collapse;
    let done = run(state.fs.clone(), move |scope| {
        gugu_core::fs::extract(scope, &path, dest.as_deref(), overwrite, collapse)
    })
    .await;
    match done {
        Ok(done) => Json(FsSuccess::new(done.summary(), done.path)).into_response(),
        Err(error) => failure(error),
    }
}
