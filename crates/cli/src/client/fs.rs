//! `gugu fs`：浏览、预览、下载、上传与增删改。
//!
//! 只负责把子命令翻成请求并把结果交给输出层；路径编码、错误口径与其它子命令一致。

use anyhow::{Context, Result};
use futures::TryStreamExt;
use gugu_core::fs::{Entry, EntryKind, Listing, TextFile};
use gugu_core::units::format_bytes;
use std::io::{IsTerminal, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use tokio::io::AsyncWriteExt;
use tokio_util::io::ReaderStream;

use super::{get_json, print_api_response, ApiHelper};
use crate::FsAction;

pub(super) async fn dispatch(api: &ApiHelper<'_>, action: &FsAction) -> Result<()> {
    match action {
        FsAction::Ls { path, json } => {
            let query = path
                .as_deref()
                .map(|path| format!("?path={}", encode_query(path)))
                .unwrap_or_default();
            let listing: Listing = get_json(api, &format!("/api/v1/fs/list{query}")).await?;
            if *json {
                println!("{}", serde_json::to_string_pretty(&listing)?);
            } else {
                super::output::print_fs_listing(&listing);
            }
        }

        FsAction::Cat { path, limit } => {
            let url = format!("/api/v1/fs/read?path={}&limit={limit}", encode_query(path));
            let file: TextFile = get_json(api, &url).await?;
            if file.binary {
                anyhow::bail!(
                    "{} 是二进制文件（{} 字节），用 gugu fs get 下载",
                    file.path,
                    file.size
                );
            }
            print!("{}", file.content);
            if !file.content.ends_with('\n') {
                println!();
            }
            if file.truncated {
                eprintln!(
                    "（只显示开头一段，共 {} 字节；完整内容用 gugu fs get）",
                    file.size
                );
            }
        }

        FsAction::Get {
            path,
            output,
            recursive,
        } => {
            if *recursive {
                download_tree(api, path, output.as_deref()).await?;
            } else {
                let target = output
                    .clone()
                    .unwrap_or_else(|| PathBuf::from(remote_name(path)));
                let written = download_file(api, path, &target).await?;
                println!("已下载 {path} → {}（{written} 字节）", target.display());
            }
        }

        FsAction::Put {
            local,
            remote,
            overwrite,
            recursive,
        } => {
            if *recursive {
                upload_tree(api, local, remote, *overwrite).await?;
            } else {
                let written = upload_file(api, local, remote, *overwrite).await?;
                println!("已上传 {} → {remote}（{written} 字节）", local.display());
            }
        }

        FsAction::Extract {
            path,
            dest,
            overwrite,
            no_collapse,
        } => {
            // 路径同时放进查询串：审计中间件在读完请求体之前就要记下对象。
            let url = format!("/api/v1/fs/extract?path={}", encode_query(path));
            let resp = api
                .post(&url)
                .json(&serde_json::json!({
                    "path": path,
                    "dest": dest,
                    "overwrite": overwrite,
                    "collapse": !no_collapse,
                }))
                .send()
                .await
                .context("无法连接到守护进程")?;
            print_api_response(resp).await?;
        }

        FsAction::Mkdir { path, parents } => {
            // 路径同时放进查询串：审计中间件在读完请求体之前就要记下对象。
            let url = format!("/api/v1/fs/mkdir?path={}", encode_query(path));
            let resp = api
                .post(&url)
                .json(&serde_json::json!({ "path": path, "parents": parents }))
                .send()
                .await
                .context("无法连接到守护进程")?;
            print_api_response(resp).await?;
        }

        FsAction::Touch { path } => {
            let url = format!("/api/v1/fs/touch?path={}", encode_query(path));
            let resp = api
                .post(&url)
                .json(&serde_json::json!({ "path": path }))
                .send()
                .await
                .context("无法连接到守护进程")?;
            print_api_response(resp).await?;
        }

        FsAction::Mv {
            from,
            to,
            overwrite,
            from_node,
        } => {
            let target_node = api
                .node_name()
                .unwrap_or(gugu_core::config::LOCAL_NODE_NAME);
            let source_node = from_node
                .as_deref()
                .map(str::trim)
                .filter(|name| !name.is_empty())
                .unwrap_or(target_node);
            if source_node == target_node {
                let url = format!("/api/v1/fs/rename?from={}", encode_query(from));
                let resp = api
                    .post(&url)
                    .json(&serde_json::json!({ "from": from, "to": to, "overwrite": overwrite }))
                    .send()
                    .await
                    .context("无法连接到守护进程")?;
                print_api_response(resp).await?;
            } else {
                // 跨节点搬运必须直连中心节点：两端节点在 body 里点名。
                let url = format!("/api/v1/fs/transfer?from={}", encode_query(from));
                let resp = api
                    .direct(reqwest::Method::POST, &url)
                    .json(&serde_json::json!({
                        "from": from,
                        "to": to,
                        "from_node": source_node,
                        "to_node": target_node,
                        "overwrite": overwrite,
                    }))
                    .send()
                    .await
                    .context("无法连接到守护进程")?;
                print_api_response(resp).await?;
            }
        }

        FsAction::Cp {
            from,
            to,
            overwrite,
        } => {
            let url = format!("/api/v1/fs/copy?from={}", encode_query(from));
            let resp = api
                .post(&url)
                .json(&serde_json::json!({ "from": from, "to": to, "overwrite": overwrite }))
                .send()
                .await
                .context("无法连接到守护进程")?;
            print_api_response(resp).await?;
        }

        FsAction::Rm { path, recursive } => {
            let url = format!(
                "/api/v1/fs?path={}&recursive={recursive}",
                encode_query(path)
            );
            let resp = api
                .delete(&url)
                .send()
                .await
                .context("无法连接到守护进程")?;
            print_api_response(resp).await?;
        }
    }
    Ok(())
}

// ── 传输：进度、单文件与递归 ─────────────────────────────

/// 一行进度：只在终端上刷新；输出被重定向或接管道时保持安静。
struct Progress {
    label: &'static str,
    total: u64,
    done: AtomicU64,
    shown: AtomicU64,
    live: bool,
}

impl Progress {
    fn new(label: &'static str, total: u64) -> Arc<Self> {
        let live = total > 0 && std::io::stdout().is_terminal();
        Arc::new(Self {
            label,
            total,
            done: AtomicU64::new(0),
            // u64::MAX 保证第一次一定刷新。
            shown: AtomicU64::new(u64::MAX),
            live,
        })
    }

    fn add(&self, bytes: u64) {
        if !self.live {
            return;
        }
        let done = self.done.fetch_add(bytes, Ordering::Relaxed) + bytes;
        let percent = done.saturating_mul(100) / self.total;
        if percent == self.shown.swap(percent, Ordering::Relaxed) {
            return;
        }
        print!(
            "\r  {} {percent}%（{} / {}）",
            self.label,
            format_bytes(done),
            format_bytes(self.total)
        );
        let _ = std::io::stdout().flush();
    }

    fn finish(&self) {
        if !self.live {
            return;
        }
        print!("\r\x1b[2K");
        let _ = std::io::stdout().flush();
    }
}

/// 下载单个文件，返回落地的字节数。
async fn download_file(api: &ApiHelper<'_>, remote: &str, target: &Path) -> Result<u64> {
    let url = format!("/api/v1/fs/download?path={}", encode_query(remote));
    let mut resp = api.get(&url).send().await.context("无法连接到守护进程")?;
    let status = resp.status();
    if !status.is_success() {
        let text = resp.text().await.unwrap_or_default();
        return Err(super::api_error(status, &text));
    }
    let progress = Progress::new("下载", resp.content_length().unwrap_or(0));
    if let Some(parent) = target
        .parent()
        .filter(|parent| !parent.as_os_str().is_empty())
    {
        tokio::fs::create_dir_all(parent)
            .await
            .with_context(|| format!("创建目录失败: {}", parent.display()))?;
    }
    let mut file = tokio::fs::File::create(target)
        .await
        .with_context(|| format!("创建本地文件失败: {}", target.display()))?;
    let mut written: u64 = 0;
    while let Some(chunk) = resp.chunk().await.context("读取下载内容失败")? {
        file.write_all(&chunk).await.context("写入本地文件失败")?;
        written += chunk.len() as u64;
        progress.add(chunk.len() as u64);
    }
    file.flush().await.context("写入本地文件失败")?;
    progress.finish();
    Ok(written)
}

/// 上传单个文件，返回上传的字节数。
async fn upload_file(
    api: &ApiHelper<'_>,
    local: &Path,
    remote: &str,
    overwrite: bool,
) -> Result<u64> {
    let meta = tokio::fs::metadata(local)
        .await
        .with_context(|| format!("读取本地文件失败: {}", local.display()))?;
    if !meta.is_file() {
        anyhow::bail!("{} 不是普通文件", local.display());
    }
    let total = meta.len();
    let file = tokio::fs::File::open(local)
        .await
        .with_context(|| format!("打开本地文件失败: {}", local.display()))?;
    let progress = Progress::new("上传", total);
    let sink = Arc::clone(&progress);
    let stream = ReaderStream::new(file).map_ok(move |chunk| {
        sink.add(chunk.len() as u64);
        chunk
    });
    let url = format!(
        "/api/v1/fs/upload?path={}&overwrite={overwrite}",
        encode_query(remote)
    );
    let resp = api
        .post(&url)
        .body(reqwest::Body::wrap_stream(stream))
        .send()
        .await
        .context("上传失败")?;
    let status = resp.status();
    let text = resp.text().await.unwrap_or_default();
    progress.finish();
    if !status.is_success() {
        return Err(super::api_error(status, &text));
    }
    Ok(total)
}

/// 递归下载目录：远端目录结构与本地一一对应。
async fn download_tree(api: &ApiHelper<'_>, remote: &str, output: Option<&Path>) -> Result<()> {
    let entry: Entry = get_json(
        api,
        &format!("/api/v1/fs/stat?path={}", encode_query(remote)),
    )
    .await?;
    if !entry.is_dir {
        anyhow::bail!("{remote} 不是目录；下载单个文件请去掉 -r");
    }
    let root = output
        .map(Path::to_path_buf)
        .unwrap_or_else(|| PathBuf::from(remote_name(remote)));

    let mut files: u64 = 0;
    let mut bytes: u64 = 0;
    let mut stack = vec![(remote.to_string(), root.clone())];
    while let Some((remote_dir, local_dir)) = stack.pop() {
        tokio::fs::create_dir_all(&local_dir)
            .await
            .with_context(|| format!("创建目录失败: {}", local_dir.display()))?;
        let listing: Listing = get_json(
            api,
            &format!("/api/v1/fs/list?path={}", encode_query(&remote_dir)),
        )
        .await?;
        for item in listing.entries {
            let target = local_dir.join(&item.name);
            match item.kind {
                EntryKind::Dir => stack.push((item.path, target)),
                EntryKind::File => {
                    let written = download_file(api, &item.path, &target).await?;
                    files += 1;
                    bytes += written;
                    println!("  ← {}（{}）", item.path, format_bytes(written));
                }
                EntryKind::Link => eprintln!("  跳过符号链接：{}", item.path),
            }
        }
    }
    println!(
        "已下载 {files} 个文件（{}）→ {}",
        format_bytes(bytes),
        root.display()
    );
    Ok(())
}

/// 递归上传目录：远端逐级建目录，再按相对结构传文件。
async fn upload_tree(
    api: &ApiHelper<'_>,
    local: &Path,
    remote: &str,
    overwrite: bool,
) -> Result<()> {
    let meta = tokio::fs::metadata(local)
        .await
        .with_context(|| format!("读取本地目录失败: {}", local.display()))?;
    if !meta.is_dir() {
        anyhow::bail!("{} 不是目录；上传单个文件请去掉 -r", local.display());
    }
    mkdir_remote(api, remote).await?;

    let mut files: u64 = 0;
    let mut bytes: u64 = 0;
    let mut stack = vec![(local.to_path_buf(), remote.to_string())];
    while let Some((local_dir, remote_dir)) = stack.pop() {
        let mut reader = tokio::fs::read_dir(&local_dir)
            .await
            .with_context(|| format!("读取目录失败: {}", local_dir.display()))?;
        while let Some(item) = reader.next_entry().await? {
            let file_type = item.file_type().await?;
            if file_type.is_symlink() {
                eprintln!("  跳过符号链接：{}", item.path().display());
                continue;
            }
            let name = item.file_name().to_string_lossy().to_string();
            let target = join_remote(&remote_dir, &name);
            if file_type.is_dir() {
                mkdir_remote(api, &target).await?;
                stack.push((item.path(), target));
            } else if file_type.is_file() {
                let size = upload_file(api, &item.path(), &target, overwrite).await?;
                files += 1;
                bytes += size;
                println!("  → {target}（{}）", format_bytes(size));
            }
        }
    }
    println!("已上传 {files} 个文件（{}）→ {remote}", format_bytes(bytes));
    Ok(())
}

/// 逐级建远端目录；已存在时服务端按 parents 语义当作成功。
async fn mkdir_remote(api: &ApiHelper<'_>, remote: &str) -> Result<()> {
    let url = format!("/api/v1/fs/mkdir?path={}", encode_query(remote));
    let resp = api
        .post(&url)
        .json(&serde_json::json!({ "path": remote, "parents": true }))
        .send()
        .await
        .context("无法连接到守护进程")?;
    let status = resp.status();
    if status.is_success() {
        return Ok(());
    }
    let text = resp.text().await.unwrap_or_default();
    Err(super::api_error(status, &text))
}

fn join_remote(dir: &str, name: &str) -> String {
    format!("{}/{}", dir.trim_end_matches(['/', '\\']), name)
}

/// 远端路径的最后一段，用作下载的缺省文件名。
fn remote_name(path: &str) -> String {
    path.trim()
        .rsplit(['/', '\\'])
        .find(|part| !part.is_empty())
        .unwrap_or("download")
        .to_string()
}

/// 查询串里的路径编码：只保留 RFC 3986 的非保留字符，`/` 也编码。
fn encode_query(value: &str) -> String {
    let mut out = String::with_capacity(value.len() * 3);
    for byte in value.trim().as_bytes() {
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
    fn encodes_paths_for_the_query_string() {
        assert_eq!(encode_query("/srv/a b.txt"), "%2Fsrv%2Fa%20b.txt");
        assert_eq!(
            encode_query("D:\\data\\日志.log"),
            "D%3A%5Cdata%5C%E6%97%A5%E5%BF%97.log"
        );
    }

    #[test]
    fn takes_the_last_component_as_the_default_name() {
        assert_eq!(remote_name("/srv/logs/app.log"), "app.log");
        assert_eq!(remote_name("D:\\data\\报表.csv"), "报表.csv");
        assert_eq!(remote_name("/"), "download");
    }
}
