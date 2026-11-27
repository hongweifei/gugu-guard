//! 文件系统浏览：供 WebUI 表单快速选择工作目录/可执行文件。
//!
//! 配置了 `daemon.web.fs_roots` 时，只放行这些根目录及其子目录；
//! 判定发生在 `canonicalize` 之后，`..` 与符号链接都已被解析成真实路径。

use axum::{
    extract::{Query, State},
    http::StatusCode,
    response::{IntoResponse, Response},
    Json,
};
use std::path::{Path, PathBuf};

use super::clean_path;
use super::types::{ApiError, BrowseQuery, BrowseResponse, FsEntry};
use crate::state::AppState;

pub(super) async fn browse_fs(
    State(state): State<AppState>,
    Query(query): Query<BrowseQuery>,
) -> Response {
    // 白名单生效时缺省路径取第一个根：工作目录未必在允许范围内。
    let requested = query
        .path
        .as_deref()
        .map(str::trim)
        .filter(|path| !path.is_empty())
        .map(PathBuf::from);
    let start = requested
        .or_else(|| state.fs_roots.first().cloned())
        .unwrap_or_else(|| PathBuf::from("."));

    let dir = {
        let resolved = if start.is_relative() {
            std::env::current_dir().unwrap_or_default().join(&start)
        } else {
            start
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

    if !allowed(&dir, &state.fs_roots) {
        return (
            StatusCode::FORBIDDEN,
            Json(ApiError::new(format!(
                "路径不在 daemon.web.fs_roots 允许的范围内: {}",
                dir.display()
            ))),
        )
            .into_response();
    }

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

/// 目标路径是否落在白名单内；白名单为空表示不限制。
///
/// 根目录解析失败（尚未创建、盘未挂载）时只跳过这一项，
/// 不让一个失效的根把其它根一起否掉。
fn allowed(path: &Path, roots: &[PathBuf]) -> bool {
    if roots.is_empty() {
        return true;
    }
    roots.iter().any(|root| {
        let Ok(root) = std::fs::canonicalize(root) else {
            return false;
        };
        is_under(path, &gugu_core::config::strip_unc_prefix(&root))
    })
}

/// 按路径分量比较，`/data` 不会把 `/database` 算作子目录。
fn is_under(path: &Path, root: &Path) -> bool {
    if path.starts_with(root) {
        return true;
    }
    // Windows 的文件系统不区分大小写，`D:\Data` 与 `d:\data` 是同一个目录。
    #[cfg(windows)]
    {
        let lower = |p: &Path| p.to_string_lossy().to_lowercase().replace('/', "\\");
        let path = lower(path);
        let root = lower(root);
        let root = root.trim_end_matches('\\');
        path == root
            || path
                .strip_prefix(root)
                .is_some_and(|rest| rest.starts_with('\\'))
    }
    #[cfg(not(windows))]
    {
        false
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn component_wise_prefix_does_not_leak_into_siblings() {
        assert!(is_under(Path::new("/data/app"), Path::new("/data")));
        assert!(!is_under(Path::new("/database"), Path::new("/data")));
        assert!(is_under(Path::new("/data"), Path::new("/data")));
    }

    #[test]
    fn empty_whitelist_allows_everything() {
        assert!(allowed(Path::new("/anywhere"), &[]));
    }

    #[cfg(windows)]
    #[test]
    fn windows_paths_compare_case_insensitively() {
        assert!(is_under(Path::new(r"D:\Data\App"), Path::new(r"d:\data")));
        assert!(!is_under(Path::new(r"D:\Data2"), Path::new(r"d:\data")));
    }
}
