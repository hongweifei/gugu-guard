//! 写操作：保存文本、新建目录与空文件、改名/移动、复制、删除。
//!
//! 每个入口只做「取参数 → 交给 core → 把结果翻成一句话」，路径约束与覆盖策略都在 core 里。

use axum::extract::{Query, State};
use axum::response::Response;
use axum::Json;
use gugu_core::fs;

use super::types::{MkdirBody, PairBody, PathBody, RemoveQuery, WriteBody};
use super::{failure, run, success};
use crate::state::AppState;

pub(super) async fn write_fs(
    State(state): State<AppState>,
    Json(body): Json<WriteBody>,
) -> Response {
    let WriteBody {
        path,
        content,
        create,
    } = body;
    let outcome = run(state.fs.clone(), move |scope| {
        fs::write(scope, &path, &content, create)
    })
    .await;
    match outcome {
        Ok(entry) => success(
            format!("已保存 {}（{} 字节）", entry.name, entry.size),
            entry.path,
        ),
        Err(error) => failure(error),
    }
}

pub(super) async fn mkdir_fs(
    State(state): State<AppState>,
    Json(body): Json<MkdirBody>,
) -> Response {
    let MkdirBody { path, parents } = body;
    let shown = slashed(&path);
    match run(state.fs.clone(), move |scope| {
        fs::create_dir(scope, &path, parents)
    })
    .await
    {
        Ok(()) => success(
            if parents {
                "目录已就绪"
            } else {
                "目录已创建"
            },
            shown,
        ),
        Err(error) => failure(error),
    }
}

pub(super) async fn touch_fs(
    State(state): State<AppState>,
    Json(body): Json<PathBody>,
) -> Response {
    let path = body.path;
    let shown = slashed(&path);
    match run(state.fs.clone(), move |scope| fs::create_file(scope, &path)).await {
        Ok(()) => success("文件已创建", shown),
        Err(error) => failure(error),
    }
}

pub(super) async fn rename_fs(
    State(state): State<AppState>,
    Json(body): Json<PairBody>,
) -> Response {
    let PairBody {
        from,
        to,
        overwrite,
    } = body;
    let shown = slashed(&to);
    match run(state.fs.clone(), move |scope| {
        fs::relocate(scope, &from, &to, overwrite)
    })
    .await
    {
        Ok(()) => success("已移动", shown),
        Err(error) => failure(error),
    }
}

pub(super) async fn copy_fs(State(state): State<AppState>, Json(body): Json<PairBody>) -> Response {
    let PairBody {
        from,
        to,
        overwrite,
    } = body;
    let shown = slashed(&to);
    match run(state.fs.clone(), move |scope| {
        fs::copy(scope, &from, &to, overwrite)
    })
    .await
    {
        Ok(()) => success("已复制", shown),
        Err(error) => failure(error),
    }
}

pub(super) async fn remove_fs(
    State(state): State<AppState>,
    Query(query): Query<RemoveQuery>,
) -> Response {
    let RemoveQuery { path, recursive } = query;
    let shown = slashed(&path);
    match run(state.fs.clone(), move |scope| {
        fs::remove(scope, &path, recursive)
    })
    .await
    {
        Ok(()) => success("已删除", shown),
        Err(error) => failure(error),
    }
}

/// 回显统一用正斜杠，与列表返回的路径形式一致。
fn slashed(path: &str) -> String {
    path.trim().replace('\\', "/")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn echoed_paths_use_forward_slashes() {
        assert_eq!(slashed("  D:\\data\\a.txt "), "D:/data/a.txt");
        assert_eq!(slashed("/srv/app"), "/srv/app");
    }
}
