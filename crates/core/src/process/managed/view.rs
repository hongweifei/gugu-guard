//! 对外视图与日志读取。

use std::path::PathBuf;

use chrono::Utc;
use tokio::process::Child;
use tokio::sync::broadcast;

use crate::logs::LogQuery;
use crate::process::types::{LogEntry, LogStream, ProcessInfo, ProcessStatus};

use super::ManagedProcess;

impl ManagedProcess {
    #[must_use]
    pub fn info(&self) -> ProcessInfo {
        let uptime_secs = self.started_at.map(|t| (Utc::now() - t).num_seconds());
        ProcessInfo {
            name: self.name.clone(),
            command: self.config.command.clone(),
            args: self.config.args.clone(),
            status: self.status.clone(),
            pid: self.child.as_ref().and_then(Child::id),
            restart_count: self.crash_restart_count,
            auto_start: self.config.auto_start,
            auto_restart: self.config.auto_restart,
            has_health_check: self.config.health_check.is_some(),
            unhealthy_restart: self.config.unhealthy_restart,
            healthy: self.healthy,
            group: self.config.group.clone(),
            started_at: self.started_at,
            uptime_secs,
            recent_exits: self.recent_exits(),
            usage: self.usage,
            listen_ports: self.watched_ports(),
            connections: self.connections,
        }
    }

    /// 实时日志：stdout 与 stderr 按时间合并后，先按检索条件过滤，再取最新的 `limit` 条。
    pub async fn logs(&self, query: &LogQuery) -> Vec<LogEntry> {
        let stdout = self.stdout_lines.lock().await;
        let stderr = self.stderr_lines.lock().await;
        if stdout.is_empty() && stderr.is_empty() {
            return Vec::new();
        }

        let mut merged: Vec<LogEntry> = Vec::with_capacity(stdout.len() + stderr.len());
        merged.extend(stdout.iter().cloned());
        merged.extend(stderr.iter().cloned());
        drop(stdout);
        drop(stderr);
        // 稳定排序：时间相同的行里 stdout 在前，与合并游走的顺序一致。
        merged.sort_by_key(|entry| entry.timestamp);

        let mut hits: Vec<LogEntry> = merged
            .into_iter()
            .filter(|entry| query.matches(entry))
            .collect();
        if hits.len() > query.limit {
            hits.drain(..hits.len() - query.limit);
        }
        hits
    }

    /// 本进程配置的输出日志文件（stdout / stderr 各一条），供历史检索。
    #[must_use]
    pub fn log_files(&self) -> Vec<(LogStream, PathBuf)> {
        let mut files = Vec::new();
        if let Some(path) = &self.config.stdout_log {
            files.push((LogStream::Stdout, path.clone()));
        }
        if let Some(path) = &self.config.stderr_log {
            files.push((LogStream::Stderr, path.clone()));
        }
        files
    }

    pub fn set_status(&mut self, status: ProcessStatus) {
        self.status = status;
    }

    pub fn set_healthy(&mut self, healthy: Option<bool>) {
        self.healthy = healthy;
    }

    pub fn last_health_check(&self) -> Option<std::time::Instant> {
        self.last_health_check
    }

    pub fn set_last_health_check(&mut self, instant: Option<std::time::Instant>) {
        self.last_health_check = instant;
    }

    pub fn subscribe_logs(&self) -> broadcast::Receiver<LogEntry> {
        self.log_tx.subscribe()
    }

    pub async fn clear_logs(&self) {
        self.stdout_lines.lock().await.clear();
        self.stderr_lines.lock().await.clear();
    }
}
