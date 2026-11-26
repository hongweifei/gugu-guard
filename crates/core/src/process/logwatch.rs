//! 日志告警：关键字命中与刷屏检测。
//!
//! 采集任务逐行调用 [`LogWatcher::on_line`]，监控循环每 2 秒取走
//! [`LogWatcher::take_alerts`] 产生的告警并走既有通知通道。这样采集侧只加一次加锁，
//! 不需要把通知通道穿进采集任务。
//!
//! 崩溃前通常先刷屏，因此「单位时间行数暴涨」比事后退出码更早给出信号。

use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use crate::config::LogAlertConfig;

/// 告警类型。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AlertKind {
    /// 关键字命中。
    Pattern,
    /// 窗口内行数达到阈值。
    Burst,
}

impl AlertKind {
    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Pattern => "log_pattern",
            Self::Burst => "log_burst",
        }
    }
}

/// 一条待投递的日志告警。
#[derive(Debug, Clone)]
pub struct LogAlert {
    pub kind: AlertKind,
    /// 关键字原文或行数描述。
    pub detail: String,
    /// 触发告警的那一行（截断到 160 字符）。
    pub sample: String,
}

struct Inner {
    window_start: Instant,
    window_lines: u32,
    pending: Vec<LogAlert>,
    /// 每个关键字的最近一次告警时间，用来做冷却。
    last_pattern_alert: HashMap<usize, Instant>,
    last_burst_alert: Option<Instant>,
}

/// 单个进程的日志分析器。
pub struct LogWatcher {
    patterns: Vec<String>,
    lowered: Vec<String>,
    window: Duration,
    burst_lines: u32,
    cooldown: Duration,
    inner: Mutex<Inner>,
}

impl LogWatcher {
    /// 配置为空（既无关键字也无刷屏阈值）时返回 None，调用方无需判断细节。
    #[must_use]
    pub fn from_config(config: &LogAlertConfig) -> Option<Arc<Self>> {
        let patterns: Vec<String> = config
            .patterns
            .iter()
            .map(|p| p.trim().to_string())
            .filter(|p| !p.is_empty())
            .collect();
        if patterns.is_empty() && config.burst_lines == 0 {
            return None;
        }
        let lowered = patterns.iter().map(|p| p.to_lowercase()).collect();
        Some(Arc::new(Self {
            patterns,
            lowered,
            window: Duration::from_secs(config.window_secs.max(1)),
            burst_lines: config.burst_lines,
            cooldown: Duration::from_secs(config.cooldown_secs),
            inner: Mutex::new(Inner {
                window_start: Instant::now(),
                window_lines: 0,
                pending: Vec::new(),
                last_pattern_alert: HashMap::new(),
                last_burst_alert: None,
            }),
        }))
    }

    /// 采集侧逐行调用；命中时把告警放进待取队列。
    pub fn on_line(&self, line: &str) {
        let now = Instant::now();
        let lowered = line.to_lowercase();
        let Ok(mut inner) = self.inner.lock() else {
            return;
        };

        // 刷屏窗口：到点就重开一个窗口，阈值只在本窗口内比较
        if now.duration_since(inner.window_start) >= self.window {
            inner.window_start = now;
            inner.window_lines = 0;
        }
        inner.window_lines += 1;
        if self.burst_lines > 0 && inner.window_lines == self.burst_lines {
            let cooled = inner
                .last_burst_alert
                .is_none_or(|last| now.duration_since(last) >= self.cooldown);
            if cooled {
                inner.last_burst_alert = Some(now);
                inner.pending.push(LogAlert {
                    kind: AlertKind::Burst,
                    detail: format!("{} 秒内输出 {} 行", self.window.as_secs(), self.burst_lines),
                    sample: truncate(line),
                });
            }
        }

        for (idx, pattern) in self.lowered.iter().enumerate() {
            if !lowered.contains(pattern) {
                continue;
            }
            let cooled = inner
                .last_pattern_alert
                .get(&idx)
                .is_none_or(|last| now.duration_since(*last) >= self.cooldown);
            if !cooled {
                continue;
            }
            inner.last_pattern_alert.insert(idx, now);
            inner.pending.push(LogAlert {
                kind: AlertKind::Pattern,
                detail: self.patterns[idx].clone(),
                sample: truncate(line),
            });
        }
    }

    /// 取走待投递的告警（监控循环每秒级调用一次）。
    pub fn take_alerts(&self) -> Vec<LogAlert> {
        let Ok(mut inner) = self.inner.lock() else {
            return Vec::new();
        };
        std::mem::take(&mut inner.pending)
    }
}

fn truncate(line: &str) -> String {
    const MAX: usize = 160;
    if line.chars().count() <= MAX {
        return line.to_string();
    }
    let mut out: String = line.chars().take(MAX).collect();
    out.push('…');
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn config(patterns: &[&str], burst: u32) -> LogAlertConfig {
        LogAlertConfig {
            patterns: patterns.iter().map(|p| (*p).to_string()).collect(),
            window_secs: 10,
            burst_lines: burst,
            cooldown_secs: 300,
        }
    }

    #[test]
    fn empty_config_disables_watcher() {
        assert!(LogWatcher::from_config(&config(&[], 0)).is_none());
        assert!(LogWatcher::from_config(&config(&["  "], 0)).is_none());
        assert!(LogWatcher::from_config(&config(&["panic"], 0)).is_some());
        assert!(LogWatcher::from_config(&config(&[], 100)).is_some());
    }

    #[test]
    fn matches_keywords_case_insensitively_and_cools_down() {
        let watcher = LogWatcher::from_config(&config(&["panic", "OOM"], 0)).expect("应启用");
        watcher.on_line("normal line");
        assert!(watcher.take_alerts().is_empty());

        watcher.on_line("thread 'main' panicked at src/main.rs");
        let alerts = watcher.take_alerts();
        assert_eq!(alerts.len(), 1);
        assert_eq!(alerts[0].kind, AlertKind::Pattern);
        assert_eq!(alerts[0].detail, "panic");

        // 冷却期内不再重复通知
        watcher.on_line("panicked again");
        assert!(watcher.take_alerts().is_empty());

        // 不同关键字各自独立冷却；匹配不分大小写，详情保留配置里的原文
        watcher.on_line("kernel: oom-killer invoked, Out of memory: Killed process 4242");
        let alerts = watcher.take_alerts();
        assert_eq!(alerts.len(), 1);
        assert_eq!(alerts[0].detail, "OOM");
    }

    #[test]
    fn burst_fires_once_when_threshold_reached() {
        let watcher = LogWatcher::from_config(&config(&[], 5)).expect("应启用");
        for _ in 0..4 {
            watcher.on_line("x");
        }
        assert!(watcher.take_alerts().is_empty(), "未达阈值不应告警");
        watcher.on_line("x");
        let alerts = watcher.take_alerts();
        assert_eq!(alerts.len(), 1);
        assert_eq!(alerts[0].kind, AlertKind::Burst);
        assert!(alerts[0].detail.contains('5'), "{:?}", alerts[0].detail);

        // 超过阈值后同一窗口内不再重复（且冷却仍生效）
        watcher.on_line("x");
        assert!(watcher.take_alerts().is_empty());
    }

    #[test]
    fn long_lines_are_truncated_for_the_notice() {
        let watcher = LogWatcher::from_config(&config(&["error"], 0)).expect("应启用");
        let long = format!("error {}", "x".repeat(300));
        watcher.on_line(&long);
        let alerts = watcher.take_alerts();
        assert_eq!(alerts.len(), 1);
        assert!(alerts[0].sample.chars().count() <= 161);
    }
}
