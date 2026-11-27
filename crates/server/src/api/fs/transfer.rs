//! 上传：请求体就是文件内容，边收边写。
//!
//! 先写到同目录临时文件，全部收完再改名就位；中途失败不会留下半个目标文件，
//! 也不会覆盖原来的同名文件。

use axum::body::Body;
use axum::extract::{Query, State};
use axum::response::Response;
use futures::StreamExt;
use gugu_core::error::GuguError;
use std::path::{Path, PathBuf};
use tokio::io::AsyncWriteExt;

use super::types::UploadQuery;
use super::{failure, forward, success};
use crate::state::AppState;

pub(super) async fn upload_fs(
    State(state): State<AppState>,
    Query(query): Query<UploadQuery>,
    body: Body,
) -> Response {
    if let Err(error) = state.fs.ensure_writable() {
        return failure(error);
    }
    let target = match state.fs.resolve_new(&query.path) {
        Ok(target) => target,
        Err(error) => return failure(error),
    };
    if target.is_dir() {
        return failure(GuguError::InvalidPath(format!(
            "目标是目录: {}",
            target.display()
        )));
    }
    if target.exists() && !query.overwrite {
        return failure(GuguError::InvalidPath(format!(
            "目标已存在: {}（需要覆盖请显式声明）",
            target.display()
        )));
    }
    let Some(parent) = target.parent().map(Path::to_path_buf) else {
        return failure(GuguError::InvalidPath(format!(
            "路径没有父目录: {}",
            target.display()
        )));
    };

    let temp = parent.join(temp_name());
    let mut file = match tokio::fs::File::create(&temp).await {
        Ok(file) => file,
        Err(error) => return failure(GuguError::Io(error)),
    };
    let mut written: u64 = 0;
    let mut stream = body.into_data_stream();
    while let Some(chunk) = stream.next().await {
        let chunk = match chunk {
            Ok(chunk) => chunk,
            Err(error) => {
                abandon(&temp).await;
                return failure(GuguError::InvalidPath(format!("读取上传内容失败: {error}")));
            }
        };
        if let Err(error) = file.write_all(&chunk).await {
            abandon(&temp).await;
            return failure(GuguError::Io(error));
        }
        written += chunk.len() as u64;
    }
    if let Err(error) = file.flush().await {
        abandon(&temp).await;
        return failure(GuguError::Io(error));
    }
    if let Err(error) = file.sync_all().await {
        abandon(&temp).await;
        return failure(GuguError::Io(error));
    }
    drop(file);

    if let Err(error) = tokio::fs::rename(&temp, &target).await {
        abandon(&temp).await;
        return failure(GuguError::Io(error));
    }

    let name = target
        .file_name()
        .map(|name| name.to_string_lossy().to_string())
        .unwrap_or_else(|| forward(&target));
    success(format!("已上传 {name}（{written} 字节）"), forward(&target))
}

async fn abandon(temp: &Path) {
    let _ = tokio::fs::remove_file(temp).await;
}

fn temp_name() -> PathBuf {
    PathBuf::from(format!(
        ".gugu-upload-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_nanos())
            .unwrap_or(0)
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn temp_names_are_hidden_and_unique() {
        let first = temp_name();
        let second = temp_name();
        assert!(first.to_string_lossy().starts_with(".gugu-upload-"));
        assert_ne!(first, second);
    }
}
