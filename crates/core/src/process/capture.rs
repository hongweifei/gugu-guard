//! stdout/stderr 逐行采集：内存环形缓冲 + 广播 + 文件落盘与轮转。

use std::collections::VecDeque;
use std::path::PathBuf;
use std::sync::Arc;

use chrono::Utc;
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::sync::{broadcast, Mutex};

use super::logwatch::LogWatcher;
use super::types::{LogEntry, LogStream};
use crate::logs::{clean, parse_line, LogFormat};

const MAX_LOG_LINES: usize = 1000;

/// 一条输出流的采集目标：内存缓冲、广播通道、日志文件与告警分析器。
/// 打包成一个结构，`read_stream` 只关心"从哪条流读"和"读到的东西去哪"。
pub(crate) struct StreamTarget<'a> {
    pub(crate) log_path: Option<PathBuf>,
    pub(crate) buffer: Arc<Mutex<VecDeque<LogEntry>>>,
    pub(crate) log_tx: broadcast::Sender<LogEntry>,
    pub(crate) process: &'a str,
    pub(crate) max_log_size_mb: Option<u64>,
    pub(crate) watcher: Option<Arc<LogWatcher>>,
    pub(crate) format: LogFormat,
}

pub(crate) async fn read_stream<R: tokio::io::AsyncRead + Unpin>(
    reader: R,
    stream: LogStream,
    target: StreamTarget<'_>,
) {
    let StreamTarget {
        log_path,
        buffer,
        log_tx,
        process: name,
        max_log_size_mb,
        watcher,
        format,
    } = target;
    let mut buf_reader = BufReader::new(reader);
    let mut raw: Vec<u8> = Vec::with_capacity(1024);
    let mut file = open_log_file(log_path.as_deref()).await;
    let max_bytes = max_log_size_mb.unwrap_or(0) * 1024 * 1024;
    let mut line_count: u32 = 0;

    loop {
        raw.clear();
        match buf_reader.read_until(b'\n', &mut raw).await {
            Ok(0) => break,
            Ok(_) => {
                // 按字节切行，容忍非 UTF-8（GBK 等）输出：内存/广播走 lossy 转换，
                // 文件保留原始字节，不丢任何一行。
                let mut body: &[u8] = &raw;
                if body.last() == Some(&b'\n') {
                    body = &body[..body.len() - 1];
                }
                if body.last() == Some(&b'\r') {
                    body = &body[..body.len() - 1];
                }
                // 内存与广播里的这一份先洗掉 ANSI 转义与不可见控制符，
                // 落盘字节仍是原始内容，导出的文件可以对得上原样。
                let line = clean(&String::from_utf8_lossy(body));
                let parsed = parse_line(&line, format);
                let parsed = (!parsed.is_empty()).then_some(parsed);
                if let Some(ref watcher) = watcher {
                    watcher.on_line(&line);
                }
                let entry = LogEntry {
                    timestamp: Utc::now(),
                    stream: stream.clone(),
                    line,
                    process_name: None,
                    parsed,
                };

                if let Some(ref mut f) = file {
                    let mut bytes = raw.clone();
                    if bytes.last() != Some(&b'\n') {
                        bytes.push(b'\n');
                    }
                    if let Err(e) = f.write_all(&bytes).await {
                        tracing::debug!("[{}] 写入日志文件失败: {}", name, e);
                    }
                }

                let mut buf = buffer.lock().await;
                if buf.len() >= MAX_LOG_LINES {
                    buf.pop_front();
                }
                buf.push_back(entry.clone());
                drop(buf);

                let _ = log_tx.send(entry);

                line_count += 1;
                if max_bytes > 0 && line_count.is_multiple_of(256) {
                    let should_rotate = match file.as_ref() {
                        Some(f) => f
                            .metadata()
                            .await
                            .map(|m| m.len() > max_bytes)
                            .unwrap_or(false),
                        None => false,
                    };
                    if should_rotate {
                        file = rotate_log_file(log_path.as_deref()).await;
                    }
                }
            }
            Err(e) => {
                tracing::debug!("[{}] 读取 {} 流错误: {}", name, stream_type(&stream), e);
                break;
            }
        }
    }
}

async fn open_log_file(path: Option<&std::path::Path>) -> Option<tokio::fs::File> {
    let p = path?;

    if let Some(parent) = p.parent() {
        if let Err(e) = tokio::fs::create_dir_all(parent).await {
            tracing::debug!("创建日志目录失败 {}: {}", parent.display(), e);
        }
    }
    match tokio::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(p)
        .await
    {
        Ok(f) => Some(f),
        Err(e) => {
            tracing::debug!("打开日志文件失败 {}: {}", p.display(), e);
            None
        }
    }
}

async fn rotate_log_file(path: Option<&std::path::Path>) -> Option<tokio::fs::File> {
    let p = path?;

    for i in (1..=5).rev() {
        let old = append_suffix(p, i);
        let next = append_suffix(p, i + 1);
        let _ = tokio::fs::rename(&old, &next).await;
    }
    let _ = tokio::fs::rename(p, append_suffix(p, 1)).await;

    tokio::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(p)
        .await
        .ok()
}

fn stream_type(stream: &LogStream) -> &'static str {
    match stream {
        LogStream::Stdout => "stdout",
        LogStream::Stderr => "stderr",
    }
}

fn append_suffix(path: &std::path::Path, n: u32) -> PathBuf {
    let stem = path
        .file_stem()
        .map(|s| s.to_string_lossy().to_string())
        .unwrap_or_default();
    let ext = path
        .extension()
        .map(|e| format!(".{}", e.to_string_lossy()));
    let parent = path.parent().unwrap_or(std::path::Path::new("."));
    match ext {
        Some(ext) => parent.join(format!("{stem}.{n}{ext}")),
        None => parent.join(format!("{stem}.{n}")),
    }
}
