//! 分片上传：创建会话、追加分片、查询偏移、完成就位与放弃。
//!
//! 分片按顺序追加，偏移由服务端记录并校验：客户端断线后先查偏移，从断点继续，
//! 重发上一片会被明确拒绝，不会把重复字节拼进文件。
//! 写权限在每次请求上重新检查，运行中把配置改成只读也会立刻生效。

use std::path::Path;

use axum::body::Body;
use axum::extract::{Path as UrlPath, Query, State};
use axum::response::{IntoResponse, Response};
use axum::Json;
use futures::StreamExt;
use gugu_core::error::GuguError;
use tokio::io::AsyncWriteExt;

use super::types::{BeginBody, ChunkQuery, ChunkResult, PathQuery, UploadSession};
use super::{failure, forward, success};
use crate::state::AppState;
use crate::uploads::SESSION_TTL;

/// 创建会话：确认目标可写、父目录存在、同名文件的处理方式。
pub(super) async fn begin_fs(
    State(state): State<AppState>,
    Query(query): Query<PathQuery>,
    Json(body): Json<BeginBody>,
) -> Response {
    if let Err(error) = state.fs.ensure_writable() {
        return failure(error);
    }
    let raw = if body.path.trim().is_empty() {
        query.path.unwrap_or_default()
    } else {
        body.path.clone()
    };
    let target = match state.fs.resolve_new(&raw) {
        Ok(target) => target,
        Err(error) => return failure(error),
    };
    if target.is_dir() {
        return failure(GuguError::InvalidPath(format!(
            "目标是目录: {}",
            target.display()
        )));
    }
    if target.exists() && !body.overwrite {
        return failure(GuguError::InvalidPath(format!(
            "目标已存在: {}（需要覆盖请显式声明）",
            target.display()
        )));
    }

    // 顺手回收过期会话，临时文件不会无限堆积。
    for temp in state.uploads.sweep(SESSION_TTL) {
        let _ = tokio::fs::remove_file(temp).await;
    }

    let started = state
        .uploads
        .begin(target.clone(), body.size, body.overwrite);
    if let Err(error) = tokio::fs::File::create(&started.temp).await {
        state.uploads.take(&started.id);
        return failure(GuguError::Io(error));
    }
    Json(UploadSession {
        id: started.id,
        path: forward(&target),
        size: body.size,
        offset: 0,
    })
    .into_response()
}

/// 查询会话：续传前用它拿服务端已有的偏移。
pub(super) async fn session_fs(
    State(state): State<AppState>,
    UrlPath(id): UrlPath<String>,
) -> Response {
    let Some(session) = state.uploads.get(&id) else {
        return failure(GuguError::NotFound("上传会话不存在或已过期".to_string()));
    };
    Json(UploadSession {
        id,
        path: forward(&session.path),
        size: session.size,
        offset: session.offset(),
    })
    .into_response()
}

/// 追加一片：偏移必须与已收字节数一致。
pub(super) async fn chunk_fs(
    State(state): State<AppState>,
    UrlPath(id): UrlPath<String>,
    Query(query): Query<ChunkQuery>,
    body: Body,
) -> Response {
    if let Err(error) = state.fs.ensure_writable() {
        return failure(error);
    }
    let Some(session) = state.uploads.get(&id) else {
        return failure(GuguError::NotFound("上传会话不存在或已过期".to_string()));
    };
    let _guard = session.write.lock().await;
    let before = session.offset();
    if query.offset != before {
        return failure(GuguError::InvalidPath(format!(
            "分片偏移不匹配：服务端已有 {before} 字节，本次从 {} 开始",
            query.offset
        )));
    }

    let mut file = match tokio::fs::OpenOptions::new()
        .append(true)
        .open(&session.temp)
        .await
    {
        Ok(file) => file,
        Err(error) => {
            state.uploads.take(&id);
            return failure(GuguError::Io(error));
        }
    };

    let mut written: u64 = 0;
    let mut stream = body.into_data_stream();
    while let Some(chunk) = stream.next().await {
        let chunk = match chunk {
            Ok(chunk) => chunk,
            Err(error) => {
                rollback(&session.temp, before).await;
                return failure(GuguError::InvalidPath(format!("读取分片内容失败: {error}")));
            }
        };
        if session.size > 0 && before + written + chunk.len() as u64 > session.size {
            rollback(&session.temp, before).await;
            return failure(GuguError::InvalidPath(format!(
                "分片超过声明的总字节数 {}",
                session.size
            )));
        }
        if let Err(error) = file.write_all(&chunk).await {
            rollback(&session.temp, before).await;
            return failure(GuguError::Io(error));
        }
        written += chunk.len() as u64;
    }
    if let Err(error) = file.flush().await {
        rollback(&session.temp, before).await;
        return failure(GuguError::Io(error));
    }
    drop(file);

    let offset = before + written;
    session.set_offset(offset);
    Json(ChunkResult { offset }).into_response()
}

/// 完成：校验总量后把临时文件改名就位。
pub(super) async fn finish_fs(
    State(state): State<AppState>,
    UrlPath(id): UrlPath<String>,
) -> Response {
    if let Err(error) = state.fs.ensure_writable() {
        return failure(error);
    }
    let Some(session) = state.uploads.get(&id) else {
        return failure(GuguError::NotFound("上传会话不存在或已过期".to_string()));
    };
    let _guard = session.write.lock().await;
    let offset = session.offset();
    if session.size > 0 && offset != session.size {
        return failure(GuguError::InvalidPath(format!(
            "还没传完：已收到 {offset} / {} 字节",
            session.size
        )));
    }
    if session.path.exists() && !session.overwrite {
        return failure(GuguError::InvalidPath(format!(
            "目标已存在: {}（需要覆盖请显式声明）",
            session.path.display()
        )));
    }

    // 只读句柄在 Windows 上不能 sync_all，这里用写句柄落盘后再改名。
    let file = match tokio::fs::OpenOptions::new()
        .write(true)
        .open(&session.temp)
        .await
    {
        Ok(file) => file,
        Err(error) => {
            state.uploads.take(&id);
            return failure(GuguError::Io(error));
        }
    };
    if let Err(error) = file.sync_all().await {
        return failure(GuguError::Io(error));
    }
    drop(file);
    if let Err(error) = tokio::fs::rename(&session.temp, &session.path).await {
        return failure(GuguError::Io(error));
    }
    state.uploads.take(&id);

    let name = session
        .path
        .file_name()
        .map(|name| name.to_string_lossy().to_string())
        .unwrap_or_else(|| forward(&session.path));
    success(
        format!("已上传 {name}（{offset} 字节）"),
        forward(&session.path),
    )
}

/// 放弃：删掉临时文件，目标不受影响。
pub(super) async fn abort_fs(
    State(state): State<AppState>,
    UrlPath(id): UrlPath<String>,
) -> Response {
    let Some(session) = state.uploads.take(&id) else {
        return failure(GuguError::NotFound("上传会话不存在或已过期".to_string()));
    };
    let _ = tokio::fs::remove_file(&session.temp).await;
    success("已放弃这次上传", forward(&session.path))
}

/// 分片写到一半失败时把文件截回上一次成功的偏移，重试这一片不会重复写入。
async fn rollback(path: &Path, offset: u64) {
    if let Ok(file) = tokio::fs::OpenOptions::new().write(true).open(path).await {
        let _ = file.set_len(offset).await;
    }
}
