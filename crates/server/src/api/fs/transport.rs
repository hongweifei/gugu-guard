//! 跨节点搬运：中心节点把条目从源节点搬到目标节点。
//!
//! 走向是「源侧取内容 → 中心节点中转 → 目标侧写入 → 删源」，中心节点只做编排：
//! 目录走打包 + 解压（内容不套顶层目录，原样落地），单文件直接走下载 + 上传，
//! 文件内容从流里过，不整块读进内存。
//!
//! 两端在同一节点时不做处理——那是 [`super::write::rename_fs`] 的活，这里直接拒绝，
//! 避免同一件事有两条实现。

use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use axum::extract::{Query, State};
use axum::http::{Method, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::Json;
use futures::StreamExt;
use gugu_core::config::{path_to_forward_slashes, LOCAL_NODE_NAME};
use gugu_core::error::{GuguError, Result};
use gugu_core::fs::{self, Entry, TEMP_PREFIX};
use gugu_core::net::encode_path_segment;
use tokio::io::AsyncWriteExt;
use tokio_util::io::ReaderStream;

use super::types::{FsSuccess, TransferBody, TransferQuery};
use super::{archive, failure, run};
use crate::nodes::UpstreamNode;
use crate::state::AppState;

/// 搬运涉及的两端：本机直接走作用域，另一台机器走 HTTP。
#[derive(Clone)]
enum Side {
    Local,
    Remote(UpstreamNode),
}

pub(super) async fn transfer_fs(
    State(state): State<AppState>,
    Query(query): Query<TransferQuery>,
    Json(body): Json<TransferBody>,
) -> Response {
    let from = query.from.unwrap_or(body.from).trim().to_string();
    let to = body.to.trim().to_string();
    if from.is_empty() || to.is_empty() {
        return failure(GuguError::InvalidPath("缺少 from 或 to".to_string()));
    }
    let from_node = node_id(&body.from_node);
    let to_node = node_id(&body.to_node);
    if from_node == to_node {
        return failure(GuguError::InvalidPath(
            "源与目标在同一节点，请用 /api/v1/fs/rename".to_string(),
        ));
    }

    match transfer(&state, &from_node, &from, &to_node, &to, body.overwrite).await {
        Ok((message, path)) => Json(FsSuccess::new(message, path)).into_response(),
        Err(error) => failure(error),
    }
}

async fn transfer(
    state: &AppState,
    from_node: &str,
    from: &str,
    to_node: &str,
    to: &str,
    overwrite: bool,
) -> Result<(String, String)> {
    let source = side_of(state, from_node)?;
    let target = side_of(state, to_node)?;

    let entry = stat_one(state, &source, from)
        .await?
        .ok_or_else(|| GuguError::NotFound(format!("源不存在: {from}（{from_node}）")))?;
    if entry.kind == gugu_core::fs::EntryKind::Link {
        return Err(GuguError::InvalidPath(format!(
            "符号链接不搬运: {from}（链接目标可能在白名单之外）"
        )));
    }

    // 目标父目录必须在，目标本身按覆盖意图处理：先查一遍，免得搬完一大包才失败。
    let parent = parent_of(to)?;
    match stat_one(state, &target, &parent).await? {
        Some(found) if found.is_dir => {}
        Some(_) => {
            return Err(GuguError::InvalidPath(format!(
                "目标父目录不是目录: {parent}"
            )))
        }
        None => {
            return Err(GuguError::InvalidPath(format!(
                "目标父目录不存在: {parent}"
            )))
        }
    }
    if stat_one(state, &target, to).await?.is_some() && !overwrite {
        return Err(GuguError::InvalidPath(format!(
            "目标已存在: {to}（需要覆盖请显式声明）"
        )));
    }

    let summary = if entry.is_dir {
        // 目录：源侧平铺打包，目标侧原样解压，归档在两侧各留一个临时名。
        let center = center_temp();
        if let Err(error) = pack(state, &source, from, &center).await {
            let _ = std::fs::remove_file(&center);
            return Err(error);
        }
        let target_temp = join_parent(Path::new(&parent), &temp_name());
        let handed = hand_over(state, &target, &center, &target_temp).await;
        let _ = std::fs::remove_file(&center);
        handed?;
        let unpacked = unpack_at(state, &target, &target_temp, to, overwrite).await;
        let _ = drop_temp(state, &target, &target_temp).await;
        unpacked?
    } else {
        copy_file(state, &source, from, &target, to, entry.size, overwrite).await?
    };

    remove_source(state, &source, from).await.map_err(|error| {
        GuguError::Io(std::io::Error::other(format!(
            "内容已到达 {to}（{to_node}），但删除源失败: {error}"
        )))
    })?;

    Ok((
        format!("已搬运 {from}（{from_node}）→ {to}（{to_node}）；{summary}"),
        to.to_string(),
    ))
}

fn parent_of(to: &str) -> Result<String> {
    Path::new(to)
        .parent()
        .map(path_to_forward_slashes)
        .ok_or_else(|| GuguError::InvalidPath(format!("目标没有父目录: {to}")))
}

fn node_id(raw: &Option<String>) -> String {
    raw.as_deref()
        .map(str::trim)
        .filter(|name| !name.is_empty())
        .unwrap_or(LOCAL_NODE_NAME)
        .to_string()
}

fn side_of(state: &AppState, node: &str) -> Result<Side> {
    if node == LOCAL_NODE_NAME {
        return Ok(Side::Local);
    }
    state
        .nodes
        .remote(node)
        .map(Side::Remote)
        .ok_or_else(|| GuguError::NotFound(format!("未知节点: {node}")))
}

fn call(
    state: &AppState,
    node: &UpstreamNode,
    method: Method,
    url: &str,
) -> reqwest::RequestBuilder {
    let builder = state.nodes.http_client().request(method, url);
    match node.api_key() {
        Some(key) => builder.bearer_auth(key),
        None => builder,
    }
}

fn unreachable(error: reqwest::Error) -> GuguError {
    GuguError::Io(std::io::Error::other(format!("节点不可达: {error}")))
}

fn remote_error(status: StatusCode, text: &str) -> GuguError {
    let message = serde_json::from_str::<serde_json::Value>(text)
        .ok()
        .and_then(|value| {
            value
                .get("error")
                .and_then(|field| field.as_str().map(str::to_string))
        })
        .unwrap_or_else(|| text.trim().to_string());
    match status.as_u16() {
        403 => GuguError::PathRefused(message),
        404 => GuguError::NotFound(message),
        400 => GuguError::InvalidPath(message),
        _ => GuguError::Io(std::io::Error::other(message)),
    }
}

async fn stat_one(state: &AppState, side: &Side, path: &str) -> Result<Option<Entry>> {
    match side {
        Side::Local => {
            let scope = state.fs.clone();
            let path = path.to_string();
            match run(scope, move |scope| fs::stat(scope, &path)).await {
                Ok(entry) => Ok(Some(entry)),
                Err(GuguError::NotFound(_)) => Ok(None),
                Err(error) => Err(error),
            }
        }
        Side::Remote(node) => {
            let url = format!(
                "{}/api/v1/fs/stat?path={}",
                node.base(),
                encode_path_segment(path)
            );
            let response = call(state, node, Method::GET, &url)
                .send()
                .await
                .map_err(unreachable)?;
            let status = response.status();
            if status == StatusCode::NOT_FOUND {
                return Ok(None);
            }
            if !status.is_success() {
                return Err(remote_error(
                    status,
                    &response.text().await.unwrap_or_default(),
                ));
            }
            response.json::<Entry>().await.map(Some).map_err(|error| {
                GuguError::InvalidPath(format!("节点 {0} 的回应无法解析: {error}", node.base()))
            })
        }
    }
}

/// 把源打包到中心节点的临时文件：本机直接打包，远端下载归档流。
async fn pack(state: &AppState, side: &Side, path: &str, into: &Path) -> Result<()> {
    match side {
        Side::Local => {
            let scope = state.fs.clone();
            let path = path.to_string();
            let into = into.to_path_buf();
            run(scope, move |scope| {
                let source = scope.resolve(&path)?;
                archive::bundle(&source, true, &into).map(|_| ())
            })
            .await
        }
        Side::Remote(node) => {
            let url = format!(
                "{}/api/v1/fs/archive?path={}&flat=true",
                node.base(),
                encode_path_segment(path)
            );
            let response = call(state, node, Method::GET, &url)
                .send()
                .await
                .map_err(unreachable)?;
            let status = response.status();
            if !status.is_success() {
                return Err(remote_error(
                    status,
                    &response.text().await.unwrap_or_default(),
                ));
            }
            let mut file = tokio::fs::File::create(into).await?;
            let mut stream = response.bytes_stream();
            while let Some(chunk) = stream.next().await {
                let chunk = chunk.map_err(unreachable)?;
                file.write_all(&chunk).await?;
            }
            file.flush().await?;
            Ok(())
        }
    }
}

/// 把中心节点上的归档交到目标侧：本机复制，远端上传。
async fn hand_over(state: &AppState, side: &Side, center: &Path, target: &str) -> Result<()> {
    match side {
        Side::Local => {
            tokio::fs::copy(center, target).await?;
            Ok(())
        }
        Side::Remote(node) => {
            let url = format!(
                "{}/api/v1/fs/upload?path={}&overwrite=true",
                node.base(),
                encode_path_segment(target)
            );
            let file = tokio::fs::File::open(center).await?;
            let body = reqwest::Body::wrap_stream(ReaderStream::new(file));
            let response = call(state, node, Method::POST, &url)
                .header("content-type", "application/octet-stream")
                .body(body)
                .send()
                .await
                .map_err(unreachable)?;
            let status = response.status();
            if !status.is_success() {
                return Err(remote_error(
                    status,
                    &response.text().await.unwrap_or_default(),
                ));
            }
            Ok(())
        }
    }
}

/// 单文件搬运：把内容从源侧直接搬到目标侧，不经过归档。
///
/// 目标在本机时先写同目录的临时文件，再整文件改名就位，中途失败不会留下半个目标文件；
/// 目标是远端时把源侧的内容流当作上传体，一次写完并显式声明覆盖意图。
async fn copy_file(
    state: &AppState,
    source: &Side,
    from: &str,
    target: &Side,
    to: &str,
    size: u64,
    overwrite: bool,
) -> Result<String> {
    match target {
        Side::Local => {
            let Side::Remote(node) = source else {
                return Err(GuguError::InvalidPath(
                    "源与目标在同一节点，请用 /api/v1/fs/rename".to_string(),
                ));
            };
            let parent = parent_of(to)?;
            let staging = join_parent(Path::new(&parent), &part_name());
            let url = format!(
                "{}/api/v1/fs/download?path={}",
                node.base(),
                encode_path_segment(from)
            );
            let response = call(state, node, Method::GET, &url)
                .send()
                .await
                .map_err(unreachable)?;
            let status = response.status();
            if !status.is_success() {
                return Err(remote_error(
                    status,
                    &response.text().await.unwrap_or_default(),
                ));
            }
            let mut file = tokio::fs::File::create(&staging).await?;
            let mut stream = response.bytes_stream();
            while let Some(chunk) = stream.next().await {
                let chunk = chunk.map_err(unreachable)?;
                file.write_all(&chunk).await?;
            }
            file.flush().await?;
            drop(file);

            let scope = state.fs.clone();
            let target_path = to.to_string();
            let moved_to = target_path.clone();
            run(scope, move |scope| {
                fs::relocate(scope, &staging, &moved_to, overwrite)
            })
            .await?;
            Ok(written(size, target_path))
        }
        Side::Remote(node) => {
            let body = match source {
                Side::Local => {
                    let scope = state.fs.clone();
                    let path = from.to_string();
                    let resolved = run(scope, move |scope| scope.resolve(&path)).await?;
                    let file = tokio::fs::File::open(resolved).await?;
                    reqwest::Body::wrap_stream(ReaderStream::new(file))
                }
                Side::Remote(from_node) => {
                    let url = format!(
                        "{}/api/v1/fs/download?path={}",
                        from_node.base(),
                        encode_path_segment(from)
                    );
                    let response = call(state, from_node, Method::GET, &url)
                        .send()
                        .await
                        .map_err(unreachable)?;
                    let status = response.status();
                    if !status.is_success() {
                        return Err(remote_error(
                            status,
                            &response.text().await.unwrap_or_default(),
                        ));
                    }
                    reqwest::Body::wrap_stream(response.bytes_stream())
                }
            };
            let url = format!(
                "{}/api/v1/fs/upload?path={}&overwrite={overwrite}",
                node.base(),
                encode_path_segment(to)
            );
            let response = call(state, node, Method::POST, &url)
                .header("content-type", "application/octet-stream")
                .body(body)
                .send()
                .await
                .map_err(unreachable)?;
            let status = response.status();
            if !status.is_success() {
                return Err(remote_error(
                    status,
                    &response.text().await.unwrap_or_default(),
                ));
            }
            Ok(written(size, to.to_string()))
        }
    }
}

/// 单文件搬运的结果摘要，与解压摘要同样带上字节数与落点。
fn written(size: u64, to: String) -> String {
    format!("已写入 1 个文件（{size} 字节）→ {to}")
}

/// 目标侧解压：搬运要求原样落地，因此关掉同名顶层目录的展开。
async fn unpack_at(
    state: &AppState,
    side: &Side,
    archive: &str,
    to: &str,
    overwrite: bool,
) -> Result<String> {
    match side {
        Side::Local => {
            let scope = state.fs.clone();
            let archive = archive.to_string();
            let to = to.to_string();
            run(scope, move |scope| {
                fs::extract(scope, &archive, Some(&to), overwrite, false)
            })
            .await
            .map(|done| done.summary())
        }
        Side::Remote(node) => {
            let url = format!(
                "{}/api/v1/fs/extract?path={}",
                node.base(),
                encode_path_segment(archive)
            );
            let response = call(state, node, Method::POST, &url)
                .json(&serde_json::json!({
                    "path": archive,
                    "dest": to,
                    "overwrite": overwrite,
                    "collapse": false,
                }))
                .send()
                .await
                .map_err(unreachable)?;
            let status = response.status();
            if !status.is_success() {
                return Err(remote_error(
                    status,
                    &response.text().await.unwrap_or_default(),
                ));
            }
            let body: serde_json::Value = response.json().await.map_err(|error| {
                GuguError::InvalidPath(format!("节点 {0} 的回应无法解析: {error}", node.base()))
            })?;
            body.get("message")
                .and_then(|field| field.as_str())
                .map(str::to_string)
                .ok_or_else(|| {
                    GuguError::InvalidPath(format!("节点 {0} 没有返回结果", node.base()))
                })
        }
    }
}

async fn remove_source(state: &AppState, side: &Side, path: &str) -> Result<()> {
    match side {
        Side::Local => {
            let scope = state.fs.clone();
            let path = path.to_string();
            run(scope, move |scope| fs::remove(scope, &path, true)).await
        }
        Side::Remote(node) => {
            let url = format!(
                "{}/api/v1/fs?path={}&recursive=true",
                node.base(),
                encode_path_segment(path)
            );
            let response = call(state, node, Method::DELETE, &url)
                .send()
                .await
                .map_err(unreachable)?;
            let status = response.status();
            if !status.is_success() {
                return Err(remote_error(
                    status,
                    &response.text().await.unwrap_or_default(),
                ));
            }
            Ok(())
        }
    }
}

/// 目标侧的中转归档用完就删；删不掉不影响搬运结果。
async fn drop_temp(state: &AppState, side: &Side, path: &str) -> Result<()> {
    match side {
        Side::Local => {
            let scope = state.fs.clone();
            let path = path.to_string();
            let _ = run(scope, move |scope| fs::remove(scope, &path, true)).await;
            Ok(())
        }
        Side::Remote(node) => {
            let url = format!(
                "{}/api/v1/fs?path={}&recursive=true",
                node.base(),
                encode_path_segment(path)
            );
            let _ = call(state, node, Method::DELETE, &url).send().await;
            Ok(())
        }
    }
}

/// 中心节点的中转归档放在系统临时目录，与打包下载同一套命名。
fn center_temp() -> PathBuf {
    std::env::temp_dir().join(format!("{TEMP_PREFIX}transfer-{}.zip", stamp()))
}

/// 目标侧的中转归档放在目标的父目录里，解压时才落在作用域内。
fn temp_name() -> String {
    format!("{TEMP_PREFIX}move-{}.zip", stamp())
}

/// 单文件的暂存名：内容写完再整体改名就位，中途失败只留一个临时文件。
fn part_name() -> String {
    format!("{TEMP_PREFIX}part-{}.bin", stamp())
}

fn stamp() -> String {
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_nanos())
        .unwrap_or(0);
    format!("{}-{nanos}", std::process::id())
}

fn join_parent(parent: &Path, name: &str) -> String {
    path_to_forward_slashes(&parent.join(name))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn unknown_nodes_map_to_not_found() {
        let state = AppState::new(gugu_core::manager::start(
            &gugu_core::config::AppConfig::default(),
            None,
        ));
        let error = side_of(&state, "web9").err().expect("未知节点应报错");
        assert!(matches!(error, GuguError::NotFound(_)), "{error}");
        assert!(matches!(side_of(&state, LOCAL_NODE_NAME), Ok(Side::Local)));
    }

    #[test]
    fn remote_errors_keep_their_status_class() {
        let denied = remote_error(
            StatusCode::FORBIDDEN,
            r#"{"error":"路径不在 daemon.web.fs_roots 允许的范围内: D:/x"}"#,
        );
        assert!(matches!(denied, GuguError::PathRefused(_)), "{denied}");
        assert_eq!(
            denied.to_string(),
            "路径不在 daemon.web.fs_roots 允许的范围内: D:/x",
            "远端文案原样转发，不再套第二层前缀"
        );
        assert!(matches!(
            remote_error(StatusCode::NOT_FOUND, r#"{"error":"没有"}"#),
            GuguError::NotFound(_)
        ));
        assert!(matches!(
            remote_error(StatusCode::BAD_REQUEST, r#"{"error":"参数"}"#),
            GuguError::InvalidPath(_)
        ));
        assert!(matches!(
            remote_error(StatusCode::BAD_GATEWAY, "网关"),
            GuguError::Io(_)
        ));
    }

    #[test]
    fn transfer_names_stay_hidden_from_listings() {
        assert!(center_temp()
            .file_name()
            .expect("有文件名")
            .to_string_lossy()
            .starts_with(TEMP_PREFIX));
        assert!(temp_name().starts_with(TEMP_PREFIX));
        assert!(part_name().starts_with(TEMP_PREFIX));
        assert_ne!(temp_name(), temp_name());
        assert_ne!(part_name(), part_name());
        let joined = join_parent(Path::new("D:/data"), ".gugu-move-1.zip");
        assert_eq!(joined, "D:/data/.gugu-move-1.zip");
    }
}
