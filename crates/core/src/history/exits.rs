//! 退出日志：每次启动/退出/自动拉起追加一行 JSONL，跨守护重启保留。
//!
//! 内存里的 `recent_exits` 只有最近 5 条且守护一重启就归零，长期可用率、崩溃频次、
//! MTBF 这类数字必须落盘。追加写不回头改文件：自动拉起是随后追加的 `revived` 记录，
//! 读取时把最近一条未标记的退出配对即可。

use std::collections::hash_map::DefaultHasher;
use std::hash::{Hash, Hasher};
use std::io::Write;
use std::path::{Path, PathBuf};

use chrono::{DateTime, Duration, Utc};
use serde::{Deserialize, Serialize};

/// 退出日志保留天数。
pub const RETENTION_DAYS: i64 = 30;
/// 单个进程日志超过该体积时按保留期压缩一次。
const MAX_FILE_BYTES: u64 = 4 * 1024 * 1024;

/// 一行日志事件。
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum JournalEntry {
    Start {
        at: DateTime<Utc>,
    },
    Exit {
        at: DateTime<Utc>,
        code: Option<i32>,
        uptime_secs: Option<i64>,
        /// 这次退出是操作者主动停止，而不是进程自己崩掉。
        #[serde(default)]
        stopped: bool,
    },
    Revived {
        at: DateTime<Utc>,
    },
}

impl JournalEntry {
    #[must_use]
    pub fn at(&self) -> DateTime<Utc> {
        match self {
            Self::Start { at } | Self::Exit { at, .. } | Self::Revived { at } => *at,
        }
    }
}

/// 一次退出的对外视图（已把随后的自动拉起配对进来）。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ExitEvent {
    pub at: DateTime<Utc>,
    pub code: Option<i32>,
    /// 这次运行存活了多久（秒），拿不到启动时间时为 None。
    pub uptime_secs: Option<i64>,
    pub auto_restarted: bool,
    /// 操作者主动停止（不计入崩溃次数）。
    pub stopped: bool,
}

/// 一个时间窗口内的退出统计。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ExitStats {
    pub window_minutes: u64,
    pub exits: u64,
    /// 非主动停止且退出码非 0（或未知）的次数。
    pub crashes: u64,
    /// 其中被操作者主动停止的次数。
    pub manual_stops: u64,
    pub auto_restarted: u64,
    /// 窗口内的累计运行秒数。
    pub uptime_secs: u64,
    /// 窗口内可用率（0~1）；完全没有记录时为 None。
    pub availability: Option<f32>,
    pub last_exit: Option<ExitEvent>,
}

/// 退出日志的读写句柄。`dir` 为 None 时不落盘（测试或未启用历史）。
#[derive(Debug, Clone)]
pub struct ExitLog {
    dir: Option<PathBuf>,
}

impl ExitLog {
    #[must_use]
    pub fn new(dir: PathBuf) -> Self {
        Self { dir: Some(dir) }
    }

    #[must_use]
    pub fn disabled() -> Self {
        Self { dir: None }
    }

    fn path(&self, process: &str) -> Option<PathBuf> {
        let dir = self.dir.as_ref()?;
        Some(dir.join(format!("{}.jsonl", file_stem(process))))
    }

    pub fn append(&self, process: &str, entry: &JournalEntry) {
        let Some(path) = self.path(process) else {
            return;
        };
        if let Some(parent) = path.parent() {
            if let Err(e) = std::fs::create_dir_all(parent) {
                tracing::debug!("创建退出日志目录失败 {}: {}", parent.display(), e);
                return;
            }
        }
        if std::fs::metadata(&path).is_ok_and(|m| m.len() > MAX_FILE_BYTES) {
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
                    tracing::debug!("写退出日志失败 {}: {}", path.display(), e);
                }
            }
            Err(e) => tracing::debug!("打开退出日志失败 {}: {}", path.display(), e),
        }
    }

    /// 进程改名时接续日志文件：目标已存在则追加上去，否则直接改名。
    pub fn rename(&self, old: &str, new: &str) {
        let (Some(old_path), Some(new_path)) = (self.path(old), self.path(new)) else {
            return;
        };
        if !old_path.exists() {
            return;
        }
        if let Some(parent) = new_path.parent() {
            let _ = std::fs::create_dir_all(parent);
        }
        if new_path.exists() {
            let text = std::fs::read_to_string(&old_path).ok();
            let appended = text.is_some_and(|text| {
                let mut body = text;
                if !body.ends_with('\n') {
                    body.push('\n');
                }
                std::fs::OpenOptions::new()
                    .append(true)
                    .create(true)
                    .open(&new_path)
                    .and_then(|mut file| file.write_all(body.as_bytes()))
                    .is_ok()
            });
            if appended {
                let _ = std::fs::remove_file(&old_path);
            }
        } else {
            let _ = std::fs::rename(&old_path, &new_path);
        }
    }

    /// 读出全部记录；文件不存在或含坏行时返回已解析的部分。
    #[must_use]
    pub fn load(&self, process: &str) -> Vec<JournalEntry> {
        let Some(path) = self.path(process) else {
            return Vec::new();
        };
        let Ok(text) = std::fs::read_to_string(&path) else {
            return Vec::new();
        };
        text.lines()
            .filter_map(|line| serde_json::from_str::<JournalEntry>(line).ok())
            .collect()
    }

    /// 最近 `limit` 次退出（新的在后）。
    #[must_use]
    pub fn events(&self, process: &str, limit: usize) -> Vec<ExitEvent> {
        let entries = self.load(process);
        let mut events: Vec<ExitEvent> = Vec::new();
        for entry in entries {
            match entry {
                JournalEntry::Exit {
                    at,
                    code,
                    uptime_secs,
                    stopped,
                } => events.push(ExitEvent {
                    at,
                    code,
                    uptime_secs,
                    auto_restarted: false,
                    stopped,
                }),
                JournalEntry::Revived { .. } => {
                    if let Some(last) = events.last_mut() {
                        last.auto_restarted = true;
                    }
                }
                JournalEntry::Start { .. } => {}
            }
        }
        let skip = events.len().saturating_sub(limit);
        events.split_off(skip)
    }

    /// 统计最近 `window_minutes` 分钟；`running_since` 是当前这段运行的起点，
    /// 用来把还没结束的运行区间计入可用率。
    #[must_use]
    pub fn stats(
        &self,
        process: &str,
        window_minutes: u64,
        now: DateTime<Utc>,
        running_since: Option<DateTime<Utc>>,
    ) -> ExitStats {
        let entries = self.load(process);
        let window_start = now - Duration::minutes(window_minutes as i64);

        let mut exits = 0u64;
        let mut crashes = 0u64;
        let mut manual_stops = 0u64;
        let mut auto_restarted = 0u64;
        let mut covered_secs: i64 = 0;
        let mut open: Option<DateTime<Utc>> = None;
        let mut last_exit: Option<ExitEvent> = None;

        for entry in entries {
            match entry {
                JournalEntry::Start { at } => {
                    if open.is_none() {
                        open = Some(at.max(window_start));
                    }
                }
                JournalEntry::Exit {
                    at,
                    code,
                    uptime_secs,
                    stopped,
                } => {
                    if let Some(start) = open.take() {
                        let end = at.clamp(window_start, now);
                        if end > start {
                            covered_secs += (end - start).num_seconds();
                        }
                    }
                    if at >= window_start {
                        exits += 1;
                        if stopped {
                            manual_stops += 1;
                        } else if code != Some(0) {
                            crashes += 1;
                        }
                        last_exit = Some(ExitEvent {
                            at,
                            code,
                            uptime_secs,
                            auto_restarted: false,
                            stopped,
                        });
                    }
                }
                JournalEntry::Revived { at } => {
                    if let Some(last) = last_exit.as_mut() {
                        if !last.auto_restarted && at >= window_start {
                            last.auto_restarted = true;
                            auto_restarted += 1;
                        }
                    }
                }
            }
        }

        if let Some(start) = open {
            if let Some(running) = running_since {
                let start = start.max(running.min(now));
                if now > start {
                    covered_secs += (now - start).num_seconds();
                }
            }
        }

        let window_secs = (window_minutes as i64) * 60;
        let has_data = exits > 0 || running_since.is_some();
        ExitStats {
            window_minutes,
            exits,
            crashes,
            manual_stops,
            auto_restarted,
            uptime_secs: covered_secs.max(0) as u64,
            availability: has_data.then(|| {
                let ratio = covered_secs as f32 / window_secs.max(1) as f32;
                ratio.clamp(0.0, 1.0)
            }),
            last_exit,
        }
    }
}

/// 文件名：可读前缀 + 名字哈希，避免同名前缀或路径分隔符带来的碰撞。
fn file_stem(process: &str) -> String {
    let mut hasher = DefaultHasher::new();
    process.hash(&mut hasher);
    let readable: String = process
        .chars()
        .map(|c| {
            if c.is_alphanumeric() || c == '-' || c == '_' {
                c
            } else {
                '_'
            }
        })
        .take(40)
        .collect();
    format!("{readable}-{:08x}", hasher.finish() as u32)
}

/// 按保留期重写文件，丢弃过期行。
fn compact(path: &Path) {
    let Ok(text) = std::fs::read_to_string(path) else {
        return;
    };
    let cutoff = Utc::now() - Duration::days(RETENTION_DAYS);
    let kept: Vec<&str> = text
        .lines()
        .filter(|line| serde_json::from_str::<JournalEntry>(line).is_ok_and(|e| e.at() >= cutoff))
        .collect();
    match std::fs::write(path, format!("{}\n", kept.join("\n"))) {
        Ok(()) => tracing::debug!("退出日志已压缩: {}", path.display()),
        Err(e) => tracing::debug!("压缩退出日志失败 {}: {}", path.display(), e),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tests::support::tempdir;

    #[test]
    fn file_stem_keeps_readable_prefix_and_disambiguates() {
        assert_eq!(file_stem("web"), file_stem("web"));
        assert_ne!(file_stem("web"), file_stem("web2"));
        // 路径分隔符被替换，不会写到别的目录
        assert!(!file_stem("a/b").contains('/'));
        assert!(file_stem("a/b").starts_with("a_b-"));
    }

    #[test]
    fn disabled_log_never_touches_disk() {
        let log = ExitLog::disabled();
        log.append("web", &JournalEntry::Start { at: Utc::now() });
        assert!(log.load("web").is_empty());
    }

    #[test]
    fn round_trips_entries_and_pairs_auto_restart() {
        let dir = tempdir();
        let log = ExitLog::new(dir.path().join("exits"));
        let now = Utc::now();
        log.append("web", &JournalEntry::Start { at: now });
        log.append(
            "web",
            &JournalEntry::Exit {
                at: now + Duration::seconds(10),
                code: Some(1),
                uptime_secs: Some(10),
                stopped: false,
            },
        );
        log.append(
            "web",
            &JournalEntry::Revived {
                at: now + Duration::seconds(12),
            },
        );
        log.append(
            "web",
            &JournalEntry::Start {
                at: now + Duration::seconds(12),
            },
        );
        log.append(
            "web",
            &JournalEntry::Exit {
                at: now + Duration::seconds(30),
                code: None,
                uptime_secs: Some(18),
                stopped: true,
            },
        );

        let events = log.events("web", 10);
        assert_eq!(events.len(), 2);
        assert!(
            events[0].auto_restarted,
            "退出后追加 revived 应标记为已自愈"
        );
        assert!(!events[1].auto_restarted);
        assert!(events[1].stopped, "主动停止应被标记");
        assert_eq!(events[0].code, Some(1));
        assert_eq!(events[1].code, None);

        // limit 只保留最近 N 条
        assert_eq!(log.events("web", 1).len(), 1);
    }

    #[test]
    fn stats_count_crashes_and_cover_running_interval() {
        let dir = tempdir();
        let log = ExitLog::new(dir.path().join("exits"));
        let now = Utc::now();
        log.append(
            "web",
            &JournalEntry::Start {
                at: now - Duration::minutes(30),
            },
        );
        log.append(
            "web",
            &JournalEntry::Exit {
                at: now - Duration::minutes(20),
                code: Some(0),
                uptime_secs: Some(600),
                stopped: false,
            },
        );
        log.append(
            "web",
            &JournalEntry::Revived {
                at: now - Duration::minutes(19),
            },
        );
        log.append(
            "web",
            &JournalEntry::Start {
                at: now - Duration::minutes(19),
            },
        );

        let stats = log.stats("web", 60, now, Some(now - Duration::minutes(19)));
        assert_eq!(stats.exits, 1);
        assert_eq!(stats.crashes, 0, "退出码 0 不算崩溃");
        assert_eq!(stats.manual_stops, 0);
        assert_eq!(stats.auto_restarted, 1);
        // 10 分钟（30→20）+ 19 分钟（19→now）
        assert_eq!(stats.uptime_secs, 29 * 60);
        let availability = stats.availability.expect("有记录应给出可用率");
        assert!((availability - 29.0 / 60.0).abs() < 0.01, "{availability}");
    }

    #[test]
    fn manual_stops_are_not_counted_as_crashes() {
        let dir = tempdir();
        let log = ExitLog::new(dir.path().join("exits"));
        let now = Utc::now();
        log.append(
            "web",
            &JournalEntry::Start {
                at: now - Duration::minutes(5),
            },
        );
        // Windows 上被 Job Object 终止时退出码是 1，不能当成崩溃
        log.append(
            "web",
            &JournalEntry::Exit {
                at: now - Duration::minutes(3),
                code: Some(1),
                uptime_secs: Some(120),
                stopped: true,
            },
        );

        let stats = log.stats("web", 60, now, None);
        assert_eq!(stats.exits, 1);
        assert_eq!(stats.crashes, 0, "主动停止不计入崩溃");
        assert_eq!(stats.manual_stops, 1);
        assert_eq!(stats.uptime_secs, 120, "主动停止之前的运行时长照常计入");
        let last = stats.last_exit.expect("应带最近一次退出");
        assert!(last.stopped);
    }

    #[test]
    fn stats_without_any_record_reports_no_availability() {
        let dir = tempdir();
        let log = ExitLog::new(dir.path().join("exits"));
        let stats = log.stats("idle", 60, Utc::now(), None);
        assert_eq!(stats.exits, 0);
        assert!(stats.availability.is_none());
    }

    #[test]
    fn compact_drops_expired_lines() {
        let dir = tempdir();
        let path = dir.path().join("exits");
        std::fs::create_dir_all(&path).expect("建目录");
        let log = ExitLog::new(path);
        let old = Utc::now() - Duration::days(RETENTION_DAYS + 1);
        log.append("web", &JournalEntry::Start { at: old });
        compact(&log.path("web").expect("应有路径"));
        assert!(log.events("web", 10).is_empty(), "过期记录应被压缩掉");
    }
}
