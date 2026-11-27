//! `gugu fs`：浏览、预览、下载、上传与增删改。
//!
//! 只负责把子命令翻成请求并把结果交给输出层；路径编码、错误口径与其它子命令一致。

use anyhow::{Context, Result};
use gugu_core::fs::{Listing, TextFile};
use std::path::PathBuf;
use tokio::io::AsyncWriteExt;

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

        FsAction::Get { path, output } => {
            let url = format!("/api/v1/fs/download?path={}", encode_query(path));
            let mut resp = api.get(&url).send().await.context("无法连接到守护进程")?;
            let status = resp.status();
            if !status.is_success() {
                let text = resp.text().await.unwrap_or_default();
                return Err(super::api_error(status, &text));
            }
            let target = output
                .clone()
                .unwrap_or_else(|| PathBuf::from(remote_name(path)));
            let mut file = tokio::fs::File::create(&target)
                .await
                .with_context(|| format!("创建本地文件失败: {}", target.display()))?;
            let mut written: u64 = 0;
            while let Some(chunk) = resp.chunk().await.context("读取下载内容失败")? {
                file.write_all(&chunk).await.context("写入本地文件失败")?;
                written += chunk.len() as u64;
            }
            file.flush().await.context("写入本地文件失败")?;
            println!("已下载 {} → {}（{written} 字节）", path, target.display());
        }

        FsAction::Put {
            local,
            remote,
            overwrite,
        } => {
            let meta = tokio::fs::metadata(local)
                .await
                .with_context(|| format!("读取本地文件失败: {}", local.display()))?;
            if !meta.is_file() {
                anyhow::bail!("{} 不是普通文件", local.display());
            }
            let file = tokio::fs::File::open(local)
                .await
                .with_context(|| format!("打开本地文件失败: {}", local.display()))?;
            let url = format!(
                "/api/v1/fs/upload?path={}&overwrite={overwrite}",
                encode_query(remote)
            );
            let resp = api
                .post(&url)
                .body(reqwest::Body::from(file))
                .send()
                .await
                .context("上传失败")?;
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
        } => {
            let url = format!("/api/v1/fs/rename?from={}", encode_query(from));
            let resp = api
                .post(&url)
                .json(&serde_json::json!({ "from": from, "to": to, "overwrite": overwrite }))
                .send()
                .await
                .context("无法连接到守护进程")?;
            print_api_response(resp).await?;
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
