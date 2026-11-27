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
use super::{failure, run};
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

/// `Content-Disposition`：ASCII 兜底名 + RFC 5987 的 UTF-8 名，中文文件名不乱码。
fn disposition(name: &str) -> String {
    let fallback: String = name
        .chars()
        .map(|ch| {
            if ch.is_ascii_graphic() || ch == ' ' {
                ch
            } else {
                '_'
            }
        })
        .collect();
    format!(
        "attachment; filename=\"{}\"; filename*=UTF-8''{}",
        fallback.replace('"', "_"),
        encode_component(name)
    )
}

fn encode_component(value: &str) -> String {
    let mut out = String::with_capacity(value.len() * 3);
    for byte in value.as_bytes() {
        match byte {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'.' | b'_' | b'~' => {
                out.push(*byte as char);
            }
            other => out.push_str(&format!("%{other:02X}")),
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn disposition_keeps_the_chinese_name_in_the_extended_field() {
        let value = disposition("日志 2026.log");
        assert!(value.starts_with("attachment; filename=\""));
        assert!(value.contains("filename*=UTF-8''"));
        assert!(value.contains("%E6%97%A5%E5%BF%97"), "{value}");

        // ASCII 兜底名只保留 ASCII 图形字符，中文在这里退化成下划线。
        let fallback = value
            .split("filename=\"")
            .nth(1)
            .and_then(|rest| rest.split('"').next())
            .expect("应带 ASCII 兜底名");
        assert!(fallback.is_ascii(), "{fallback}");
        assert!(fallback.contains('_'), "{fallback}");

        // 文件名里的引号会破坏头部语法，兜底名要把它替换掉。
        let quoted = disposition("a\"b.txt");
        assert!(
            quoted.contains("filename=\"a_b.txt\""),
            "兜底名里的引号应替换成下划线: {quoted}"
        );
    }
}
