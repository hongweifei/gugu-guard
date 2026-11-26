//! 操作审计：把每一次会改变守护状态的请求落成一行 JSONL。
//!
//! 审计的对象是「谁在什么时候对哪个进程做了什么、结果如何」，与进程输出日志无关：
//! 前者记录控制面操作，后者记录业务输出。条目先写内存（最近 [`MAX_KEPT`] 条，
//! 供接口快速取用），再追加到 `audit.jsonl`；文件超过体积上限时按保留期压缩。

use std::collections::VecDeque;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::time::{Duration as StdDuration, Instant};

use chrono::{DateTime, Duration, Utc};
use serde::{Deserialize, Serialize};

/// 内存里保留的最近条目数。
pub const MAX_KEPT: usize = 500;
/// 查询默认返回条数。
pub const DEFAULT_LIMIT: usize = 100;
/// 单次查询最多返回条数。
pub const MAX_LIMIT: usize = 500;
/// 审计记录保留天数。
pub const RETENTION_DAYS: i64 = 90;
/// 单个审计文件超过该体积时按保留期压缩一次。
const MAX_FILE_BYTES: u64 = 8 * 1024 * 1024;
/// 低写入量时也要按天执行一次保留期清理，避免「保留 90 天」只在大文件上生效。
const COMPACT_INTERVAL: StdDuration = StdDuration::from_secs(24 * 60 * 60);

/// 请求携带的凭据状态。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AuthKind {
    /// 未配置 API Key，请求无需认证。
    None,
    /// 带有效 API Key。
    Key,
    /// 认证失败（401）。
    Denied,
}

impl AuthKind {
    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            Self::None => "none",
            Self::Key => "key",
            Self::Denied => "denied",
        }
    }
}

/// 一条审计记录。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AuditEntry {
    pub at: DateTime<Utc>,
    /// 稳定的动作码，如 `process.start`，供脚本与筛选使用。
    pub action: String,
    /// 动作的中文名，落盘后直接可读。
    pub label: String,
    /// 操作对象（进程名或分组名）。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub target: Option<String>,
    pub method: String,
    pub path: String,
    pub status: u16,
    pub ok: bool,
    pub duration_ms: u64,
    /// 来源：代理头里的客户端地址，或连接的远端地址。
    pub client: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub user_agent: Option<String>,
    pub auth: AuthKind,
    /// 失败时的错误正文（已截断）。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub detail: Option<String>,
}

/// 查询条件。`limit` 为 0 时按 [`DEFAULT_LIMIT`] 处理。
#[derive(Debug, Clone, Default)]
pub struct AuditFilter {
    pub action: Option<String>,
    pub only_failed: bool,
    pub only_succeeded: bool,
    pub since: Option<DateTime<Utc>>,
    pub query: Option<String>,
    pub limit: usize,
}

impl AuditFilter {
    fn effective_limit(&self) -> usize {
        if self.limit == 0 {
            DEFAULT_LIMIT
        } else {
            self.limit.min(MAX_LIMIT)
        }
    }

    fn matches(&self, entry: &AuditEntry) -> bool {
        if self
            .action
            .as_deref()
            .is_some_and(|action| action != entry.action)
        {
            return false;
        }
        if self.only_failed && entry.ok {
            return false;
        }
        if self.only_succeeded && !entry.ok {
            return false;
        }
        if self.since.is_some_and(|since| entry.at < since) {
            return false;
        }
        if let Some(query) = self.query.as_deref() {
            let needle = query.trim().to_lowercase();
            if !needle.is_empty() {
                let haystack = format!(
                    "{} {} {} {} {} {}",
                    entry.label,
                    entry.action,
                    entry.target.as_deref().unwrap_or_default(),
                    entry.path,
                    entry.client,
                    entry.detail.as_deref().unwrap_or_default()
                )
                .to_lowercase();
                if !haystack.contains(&needle) {
                    return false;
                }
            }
        }
        true
    }
}

/// 审计读写句柄。`dir` 为 None 时只留在内存（测试或未接落盘）。
#[derive(Debug)]
pub struct AuditStore {
    dir: Option<PathBuf>,
    recent: Mutex<VecDeque<AuditEntry>>,
    last_compact: Mutex<Option<Instant>>,
}

impl AuditStore {
    #[must_use]
    pub fn new(dir: PathBuf) -> Self {
        Self {
            dir: Some(dir),
            recent: Mutex::new(VecDeque::with_capacity(MAX_KEPT)),
            last_compact: Mutex::new(None),
        }
    }

    #[must_use]
    pub fn disabled() -> Self {
        Self {
            dir: None,
            recent: Mutex::new(VecDeque::with_capacity(MAX_KEPT)),
            last_compact: Mutex::new(None),
        }
    }

    /// 审计文件的完整路径；未落盘时为 None。
    #[must_use]
    pub fn file_path(&self) -> Option<PathBuf> {
        self.dir.as_ref().map(|dir| dir.join("audit.jsonl"))
    }

    /// 追加一条记录：先入内存，再落盘。
    pub fn record(&self, entry: AuditEntry) {
        if let Ok(mut recent) = self.recent.lock() {
            recent.push_back(entry.clone());
            while recent.len() > MAX_KEPT {
                recent.pop_front();
            }
        }
        self.append(&entry);
    }

    fn append(&self, entry: &AuditEntry) {
        let Some(path) = self.file_path() else {
            return;
        };
        if let Some(parent) = path.parent() {
            if let Err(e) = std::fs::create_dir_all(parent) {
                tracing::warn!("创建审计目录失败 {}: {}", parent.display(), e);
                return;
            }
        }
        if std::fs::metadata(&path).is_ok_and(|m| m.len() > MAX_FILE_BYTES) || self.compact_due() {
            compact(&path);
        }
        let Ok(line) = serde_json::to_string(entry) else {
            return;
        };
        match std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(&path)
        {
            Ok(mut file) => {
                if let Err(e) = writeln!(file, "{line}") {
                    tracing::warn!("写审计日志失败 {}: {}", path.display(), e);
                }
            }
            Err(e) => tracing::warn!("打开审计日志失败 {}: {}", path.display(), e),
        }
    }

    /// 是否到了每日清理时间；到点就记下本次时间，避免每条记录都判断。
    fn compact_due(&self) -> bool {
        let Ok(mut last) = self.last_compact.lock() else {
            return false;
        };
        let due = last.is_none_or(|time| time.elapsed() >= COMPACT_INTERVAL);
        if due {
            *last = Some(Instant::now());
        }
        due
    }

    /// 最近 `limit` 条，新的在前（只读内存）。
    #[must_use]
    pub fn recent(&self, limit: usize) -> Vec<AuditEntry> {
        let limit = if limit == 0 {
            DEFAULT_LIMIT
        } else {
            limit.min(MAX_LIMIT)
        };
        let Ok(recent) = self.recent.lock() else {
            return Vec::new();
        };
        recent.iter().rev().take(limit).cloned().collect()
    }

    /// 按条件查询，新的在前；有落盘文件时以文件为准，文件读不到才退回内存。
    #[must_use]
    pub fn query(&self, filter: &AuditFilter) -> Vec<AuditEntry> {
        let mut entries = self.load_file().unwrap_or_else(|| {
            self.recent(MAX_KEPT)
                .into_iter()
                .rev()
                .collect::<Vec<AuditEntry>>()
        });
        entries.retain(|entry| filter.matches(entry));
        let limit = filter.effective_limit();
        let skip = entries.len().saturating_sub(limit);
        let mut page = entries.split_off(skip);
        page.reverse();
        page
    }

    /// 读全部落盘记录；文件不存在时返回 None，让调用方退回内存。
    fn load_file(&self) -> Option<Vec<AuditEntry>> {
        let path = self.file_path()?;
        let text = std::fs::read_to_string(&path).ok()?;
        Some(
            text.lines()
                .filter_map(|line| serde_json::from_str::<AuditEntry>(line).ok())
                .collect(),
        )
    }
}

/// 按保留期压缩；压缩后仍然过大时只保留较新的半份，避免每条记录都触发重写。
fn compact(path: &Path) {
    let Ok(text) = std::fs::read_to_string(path) else {
        return;
    };
    let cutoff = Utc::now() - Duration::days(RETENTION_DAYS);
    let mut kept: Vec<&str> = text
        .lines()
        .filter(|line| serde_json::from_str::<AuditEntry>(line).is_ok_and(|e| e.at >= cutoff))
        .collect();
    let total: usize = kept.iter().map(|line| line.len() + 1).sum();
    if total > (MAX_FILE_BYTES / 2) as usize && kept.len() > 1 {
        kept.drain(..kept.len() / 2);
    }
    let body = if kept.is_empty() {
        String::new()
    } else {
        format!("{}\n", kept.join("\n"))
    };
    match std::fs::write(path, body) {
        Ok(()) => tracing::debug!("审计日志已压缩: {}", path.display()),
        Err(e) => tracing::warn!("压缩审计日志失败 {}: {}", path.display(), e),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tests::support::tempdir;

    fn entry(action: &str, target: &str, ok: bool) -> AuditEntry {
        AuditEntry {
            at: Utc::now(),
            action: action.to_string(),
            label: action.to_string(),
            target: Some(target.to_string()),
            method: "POST".to_string(),
            path: format!("/api/v1/processes/{target}/start"),
            status: if ok { 200 } else { 400 },
            ok,
            duration_ms: 12,
            client: "127.0.0.1:5000".to_string(),
            user_agent: None,
            auth: AuthKind::None,
            detail: (!ok).then(|| "启动失败".to_string()),
        }
    }

    #[test]
    fn records_keep_newest_first_and_cap_memory() {
        let store = AuditStore::disabled();
        for index in 0..(MAX_KEPT + 10) {
            store.record(entry("process.start", &format!("p{index}"), true));
        }

        let recent = store.recent(2);
        assert_eq!(recent.len(), 2);
        assert_eq!(recent[0].target.as_deref(), Some("p509"), "新的在前");
        assert_eq!(recent[1].target.as_deref(), Some("p508"));
        assert_eq!(store.recent(MAX_LIMIT).len(), MAX_LIMIT, "查询条数封顶");
        assert_eq!(
            store.recent(MAX_KEPT + 100).len(),
            MAX_KEPT,
            "内存只留最近 500 条"
        );
    }

    #[test]
    fn filters_by_action_result_keyword_and_limit() {
        let store = AuditStore::disabled();
        let now = Utc::now();
        for (action, ok, target) in [
            ("process.start", true, "web"),
            ("process.stop", true, "web"),
            ("process.start", false, "api"),
            ("process.delete", true, "api"),
        ] {
            let mut item = entry(action, target, ok);
            item.at = now - Duration::seconds(3);
            store.record(item);
        }

        let filter = AuditFilter {
            action: Some("process.start".to_string()),
            only_failed: true,
            query: Some("api".to_string()),
            limit: 10,
            ..AuditFilter::default()
        };
        let hits = store.query(&filter);
        assert_eq!(hits.len(), 1);
        assert_eq!(hits[0].target.as_deref(), Some("api"));
        assert_eq!(hits[0].status, 400);

        let all = store.query(&AuditFilter {
            limit: 10,
            ..AuditFilter::default()
        });
        assert_eq!(all.len(), 4);
        assert_eq!(all[0].action, "process.delete", "新的在前");

        let limited = store.query(&AuditFilter {
            limit: 2,
            ..AuditFilter::default()
        });
        assert_eq!(limited.len(), 2);

        let old = store.query(&AuditFilter {
            since: Some(now),
            ..AuditFilter::default()
        });
        assert!(old.is_empty(), "早于 since 的记录不返回");
    }

    #[test]
    fn entries_survive_a_restart() {
        let dir = tempdir();
        let path = dir.path().join("audit");
        let store = AuditStore::new(path.clone());
        store.record(entry("process.restart", "web", true));

        let reopened = AuditStore::new(path);
        let hits = reopened.query(&AuditFilter::default());
        assert_eq!(hits.len(), 1);
        assert_eq!(hits[0].target.as_deref(), Some("web"));
    }

    #[test]
    fn compact_drops_expired_entries() {
        let dir = tempdir();
        let path = dir.path().join("audit");
        let store = AuditStore::new(path);
        let old = Utc::now() - Duration::days(RETENTION_DAYS + 1);
        let mut expired = entry("process.start", "old", true);
        expired.at = old;
        store.record(expired);
        store.record(entry("process.stop", "fresh", true));

        compact(&store.file_path().expect("应有审计文件"));

        let hits = store.query(&AuditFilter::default());
        assert_eq!(hits.len(), 1, "过期记录应被压缩掉");
        assert_eq!(hits[0].target.as_deref(), Some("fresh"));
    }
}
