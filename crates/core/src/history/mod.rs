//! 历史数据：进程指标时序（内存）、整机带宽（内存）、退出日志（磁盘）。
//!
//! 子模块分工：
//! - [`metrics`]：1 分钟降采样桶，进程与机器两条序列
//! - [`traffic`]：整机网卡计数器 → 速率
//! - [`exits`]：启动/退出/自动拉起的 JSONL 日志与统计
//!
//! 这个门面（[`History`]）是唯一对外的入口：写入集中在监控循环，读取由 API/CLI 直接调用，
//! 不经过 actor 命令通道，所以读接口再频繁也不会拖慢进程操作。

pub mod exits;
pub mod metrics;
pub mod traffic;

use std::path::PathBuf;
use std::sync::Mutex;

use chrono::{DateTime, Utc};

use crate::process::UsageSample;

pub use exits::{ExitEvent, ExitLog, ExitStats, JournalEntry};
pub use metrics::{MetricPoint, TrafficPoint, BUCKET_SECS, WINDOW_BUCKETS};
pub use traffic::{NetRate, TrafficSampler};

#[derive(Default)]
struct Inner {
    metrics: metrics::MetricsStore,
    sampler: TrafficSampler,
}

/// 历史仓库句柄；克隆都发生在内部锁之后，调用方只需持有 `Arc<History>`。
pub struct History {
    inner: Mutex<Inner>,
    exits: ExitLog,
}

impl History {
    /// 在 `dir` 下落盘（退出日志放 `dir/exits/`），指标只驻内存。
    #[must_use]
    pub fn new(dir: PathBuf) -> Self {
        Self {
            inner: Mutex::new(Inner::default()),
            exits: ExitLog::new(dir.join("exits")),
        }
    }

    /// 不落盘、不记录的形态（测试与未启用场景）。
    #[must_use]
    pub fn disabled() -> Self {
        Self {
            inner: Mutex::new(Inner::default()),
            exits: ExitLog::disabled(),
        }
    }

    /// 记录一批进程的瞬时采样。
    pub fn record_usage(&self, now: DateTime<Utc>, samples: &[(&str, UsageSample, u32)]) {
        let Ok(mut inner) = self.inner.lock() else {
            return;
        };
        for (name, usage, connections) in samples {
            inner.metrics.record_process(name, usage, *connections, now);
        }
    }

    /// 采一次整机带宽并入库；返回本次速率（首次采样为 None）。
    pub fn sample_traffic(&self, now: DateTime<Utc>) -> Option<NetRate> {
        let Ok(mut inner) = self.inner.lock() else {
            return None;
        };
        let rate = inner.sampler.sample()?;
        inner.metrics.record_traffic(&rate, now);
        Some(rate)
    }

    #[must_use]
    pub fn latest_traffic(&self) -> Option<TrafficPoint> {
        let Ok(inner) = self.inner.lock() else {
            return None;
        };
        inner.metrics.latest_traffic()
    }

    #[must_use]
    pub fn process_points(&self, name: &str, minutes: u64) -> Vec<MetricPoint> {
        let Ok(inner) = self.inner.lock() else {
            return Vec::new();
        };
        inner.metrics.process_points(name, minutes)
    }

    #[must_use]
    pub fn traffic_points(&self, minutes: u64) -> Vec<TrafficPoint> {
        let Ok(inner) = self.inner.lock() else {
            return Vec::new();
        };
        inner.metrics.traffic_points(minutes)
    }

    pub fn mark_start(&self, name: &str, at: DateTime<Utc>) {
        self.exits.append(name, &JournalEntry::Start { at });
    }

    pub fn mark_exit(
        &self,
        name: &str,
        at: DateTime<Utc>,
        code: Option<i32>,
        uptime_secs: Option<i64>,
        stopped: bool,
    ) {
        self.exits.append(
            name,
            &JournalEntry::Exit {
                at,
                code,
                uptime_secs,
                stopped,
            },
        );
    }

    pub fn mark_revived(&self, name: &str, at: DateTime<Utc>) {
        self.exits.append(name, &JournalEntry::Revived { at });
    }

    #[must_use]
    pub fn events(&self, name: &str, limit: usize) -> Vec<ExitEvent> {
        self.exits.events(name, limit)
    }

    #[must_use]
    pub fn stats(
        &self,
        name: &str,
        window_minutes: u64,
        now: DateTime<Utc>,
        running_since: Option<DateTime<Utc>>,
    ) -> ExitStats {
        self.exits.stats(name, window_minutes, now, running_since)
    }

    /// 进程被移除时丢掉内存里的指标序列（退出日志保留，便于事后追查）。
    pub fn forget(&self, name: &str) {
        if let Ok(mut inner) = self.inner.lock() {
            inner.metrics.forget(name);
        }
    }

    /// 进程改名时把指标序列与退出日志一并接力过去，避免改名后统计断档。
    pub fn rename(&self, old: &str, new: &str) {
        if let Ok(mut inner) = self.inner.lock() {
            inner.metrics.rename(old, new);
        }
        self.exits.rename(old, new);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tests::support::tempdir;

    fn usage(cpu: f32, mem: u64) -> UsageSample {
        UsageSample {
            cpu_percent: cpu,
            memory_bytes: mem,
            process_count: 1,
        }
    }

    #[test]
    fn records_usage_per_process() {
        let dir = tempdir();
        let history = History::new(dir.path().join("history"));
        history.record_usage(
            Utc::now(),
            &[("web", usage(12.0, 1024), 3), ("db", usage(4.0, 512), 0)],
        );
        assert_eq!(history.process_points("web", 60).len(), 1);
        let point = &history.process_points("web", 60)[0];
        assert_eq!(point.connections, 3);
        assert_eq!(point.mem_peak, 1024);
        assert_eq!(history.process_points("db", 60)[0].connections, 0);
    }

    #[test]
    fn exit_journal_round_trips_through_facade() {
        let dir = tempdir();
        let history = History::new(dir.path().join("history"));
        let now = Utc::now();
        history.mark_start("web", now);
        history.mark_exit("web", now, Some(137), Some(5), false);
        let events = history.events("web", 5);
        assert_eq!(events.len(), 1);
        assert_eq!(events[0].code, Some(137));
        assert!(history.stats("web", 60, now, None).availability.is_some());
    }

    #[test]
    fn disabled_history_records_nothing_but_stays_usable() {
        let history = History::disabled();
        history.mark_start("web", Utc::now());
        history.record_usage(Utc::now(), &[("web", usage(1.0, 1), 1)]);
        assert!(history.events("web", 5).is_empty());
        assert!(history
            .stats("web", 60, Utc::now(), None)
            .availability
            .is_none());
    }
}
