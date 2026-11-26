//! 历史日志文件检索：从文件尾部按字节预算倒读，用同一解析器逐行结构化后过滤。
//!
//! 只读盘上的 `stdout_log` / `stderr_log`（含轮转前还在原路径的那一段），
//! 不读 `.1`..`.5` 备份：预算内取最近的尾部，够用且不会把磁盘读穿。

use std::collections::VecDeque;
use std::io::{Read, Seek, SeekFrom};
use std::path::{Path, PathBuf};

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

use crate::process::LogStream;

use super::parse::{clean, parse_line, LogFormat, ParsedLine};
use super::query::LogQuery;

/// 默认只读每个文件最后 4 MB。
pub const DEFAULT_BUDGET_BYTES: u64 = 4 * 1024 * 1024;
/// 单次检索最多读 32 MB，避免把守护进程内存当成日志仓库。
pub const MAX_BUDGET_BYTES: u64 = 32 * 1024 * 1024;
/// 默认返回条数。
pub const DEFAULT_LIMIT: usize = 200;
const MAX_LIMIT: usize = 1000;

/// 检索的物理预算。
#[derive(Debug, Clone, Copy)]
pub struct SearchOptions {
    pub budget_bytes: u64,
    pub limit: usize,
}

impl SearchOptions {
    #[must_use]
    pub fn new(budget_bytes: u64, limit: usize) -> Self {
        Self {
            budget_bytes: budget_bytes.clamp(1, MAX_BUDGET_BYTES),
            limit: limit.clamp(1, MAX_LIMIT),
        }
    }
}

impl Default for SearchOptions {
    fn default() -> Self {
        Self {
            budget_bytes: DEFAULT_BUDGET_BYTES,
            limit: DEFAULT_LIMIT,
        }
    }
}

/// 一条命中记录，字段与实时 [`crate::process::LogEntry`] 一一对应；
/// 时间取行内时间戳（历史行没有采集时刻可用）。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SearchHit {
    pub stream: LogStream,
    pub line: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub parsed: Option<ParsedLine>,
}

impl SearchHit {
    #[must_use]
    pub fn at(&self) -> Option<DateTime<Utc>> {
        self.parsed.as_ref().and_then(|parsed| parsed.at)
    }
}

/// 一次历史检索的结果与覆盖面：命中条数与扫描量一起返回，用户能判断结果够不够全。
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct LogSearchReport {
    pub hits: Vec<SearchHit>,
    pub scanned_lines: u64,
    pub scanned_bytes: u64,
    /// 过滤前的命中总数（可能大于返回条数）。
    pub matched: u64,
    /// 命中被条数上限截断，或读到的范围被字节预算截断。
    pub truncated: bool,
    pub files: Vec<String>,
}

/// 扫描若干日志文件（stdout/stderr 各自一条）：命中都带行内时间时按时间合并，
/// 否则保留扫描顺序（先 stdout 后 stderr，各自按文件原顺序）。
#[must_use]
pub fn search_files(
    files: &[(LogStream, PathBuf)],
    format: LogFormat,
    query: &LogQuery,
    options: SearchOptions,
) -> LogSearchReport {
    let mut report = LogSearchReport::default();
    let mut hits: Vec<SearchHit> = Vec::new();

    for (stream, path) in files {
        let Some(scan) = scan_file(
            path,
            stream,
            format,
            query,
            options.budget_bytes,
            options.limit,
        ) else {
            continue;
        };
        report.scanned_lines += scan.scanned_lines;
        report.scanned_bytes += scan.scanned_bytes;
        report.matched += scan.matched;
        report.truncated |= scan.truncated;
        report.files.push(path.display().to_string());
        hits.extend(scan.hits);
    }

    // 所有命中都带行内时间时才按时间排序（stdout/stderr 交错才读得顺）；
    // 只要有行缺时间，就保留扫描顺序（先 stdout 后 stderr、各自按文件原顺序），
    // 不做半真半假的排序。
    if hits.iter().all(|hit| hit.at().is_some()) {
        hits.sort_by_key(SearchHit::at);
    }
    if hits.len() > options.limit {
        let extra = hits.len() - options.limit;
        hits.drain(..extra);
        report.truncated = true;
    }
    report.hits = hits;
    report
}

struct FileScan {
    hits: Vec<SearchHit>,
    scanned_lines: u64,
    scanned_bytes: u64,
    matched: u64,
    truncated: bool,
}

fn scan_file(
    path: &Path,
    stream: &LogStream,
    format: LogFormat,
    query: &LogQuery,
    budget_bytes: u64,
    limit: usize,
) -> Option<FileScan> {
    let mut file = std::fs::File::open(path).ok()?;
    let len = file.metadata().ok()?.len();
    let start = len.saturating_sub(budget_bytes);
    file.seek(SeekFrom::Start(start)).ok()?;

    let mut buffer = Vec::with_capacity(usize::try_from(len - start).unwrap_or(0) + 1);
    file.read_to_end(&mut buffer).ok()?;
    let mut text = String::from_utf8_lossy(&buffer).into_owned();
    if start > 0 {
        // 从文件中段开始读，第一行可能被截断：丢掉这半行，别当成完整日志。
        match text.find('\n') {
            Some(pos) => {
                text.drain(..=pos);
            }
            None => text.clear(),
        }
    }

    let mut hits: VecDeque<SearchHit> = VecDeque::new();
    let mut matched = 0_u64;
    let mut scanned_lines = 0_u64;

    for raw in text.split('\n') {
        if raw.is_empty() {
            continue;
        }
        scanned_lines += 1;
        let line = clean(raw);
        if line.is_empty() {
            continue;
        }
        let parsed = parse_line(&line, format);
        let parsed = (!parsed.is_empty()).then_some(parsed);
        let at = parsed.as_ref().and_then(|parsed| parsed.at);
        if !query.matches_parts(stream, &line, parsed.as_ref(), at.as_ref()) {
            continue;
        }
        matched += 1;
        hits.push_back(SearchHit {
            stream: stream.clone(),
            line,
            parsed,
        });
        if hits.len() > limit {
            hits.pop_front();
        }
    }

    Some(FileScan {
        hits: hits.into(),
        scanned_lines,
        scanned_bytes: buffer.len() as u64,
        matched,
        truncated: matched > limit as u64 || start > 0,
    })
}
