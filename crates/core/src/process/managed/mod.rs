//! `ManagedProcess`：单个被守护进程的生命周期状态机。
//!
//! 平台机制不在此文件：启动命令构造在 [`command`]，停止/强杀在 [`terminate`]，
//! 输出采集在 [`capture`]，资源硬限在 [`crate::cgroup`]。
//!
//! 子模块按动作分块实现同一类型：
//! - [`spawn`]：启动（命令构造、硬限落地、日志采集接线）
//! - [`lifecycle`]：停止 / 重启 / 存活检查 / 退出记录
//! - [`observe`]：资源用量、连接数、健康检查与日志告警的节拍采样
//! - [`view`]：对外视图（[`ProcessInfo`]）与日志读取

mod lifecycle;
mod observe;
mod spawn;
mod view;

use std::collections::VecDeque;
use std::path::PathBuf;
use std::sync::Arc;

use chrono::{DateTime, Utc};
use tokio::process::Child;
use tokio::sync::{broadcast, Mutex};
use tokio::task::JoinHandle;

use crate::cgroup::{Limiter, DEFAULT_BASE};
use crate::config::ProcessConfig;
use crate::history::History;

use super::logwatch::LogWatcher;
use super::types::{ExitRecord, LogEntry, ProcessStatus};
use super::{UsageSample, UsageSampler};

const LOG_BROADCAST_CAPACITY: usize = 256;

pub struct ManagedProcess {
    name: String,
    config: ProcessConfig,
    child: Option<Child>,
    status: ProcessStatus,
    crash_restart_count: u32,
    health_failures: u32,
    healthy: Option<bool>,
    started_at: Option<DateTime<Utc>>,
    stdout_lines: Arc<Mutex<VecDeque<LogEntry>>>,
    stderr_lines: Arc<Mutex<VecDeque<LogEntry>>>,
    log_tx: broadcast::Sender<LogEntry>,
    log_tasks: Vec<JoinHandle<()>>,
    last_health_check: Option<std::time::Instant>,
    /// 最近退出历史（新的在后，上限 MAX_EXIT_HISTORY）。
    exit_history: VecDeque<ExitRecord>,
    /// 进程树资源占用（由监控循环按节拍刷新）。
    usage: Option<UsageSample>,
    usage_sampler: UsageSampler,
    /// 指标与退出日志；测试等场景可用 `History::disabled()`。
    history: Arc<History>,
    /// 日志关键字/刷屏分析器，未配置时为 None。
    log_watcher: Option<Arc<LogWatcher>>,
    /// 监听端口上的已建立连接数（由监控循环刷新）。
    connections: u32,
    /// 本进程当前所在的硬限载体（Linux 的 cgroup、Windows 的 Job Object）。
    limiter: Option<Limiter>,
    /// 硬限落地目录，来自 `daemon.cgroup_root`；仅 Linux 使用。
    cgroup_base: PathBuf,
}

impl ManagedProcess {
    pub fn new(name: String, config: ProcessConfig) -> Self {
        let log_watcher = config.log_alert.as_ref().and_then(LogWatcher::from_config);
        let (log_tx, _) = broadcast::channel(LOG_BROADCAST_CAPACITY);
        Self {
            name,
            config,
            child: None,
            status: ProcessStatus::Stopped,
            crash_restart_count: 0,
            health_failures: 0,
            healthy: None,
            started_at: None,
            stdout_lines: Arc::new(Mutex::new(VecDeque::new())),
            stderr_lines: Arc::new(Mutex::new(VecDeque::new())),
            log_tx,
            log_tasks: Vec::new(),
            last_health_check: None,
            exit_history: VecDeque::new(),
            usage: None,
            usage_sampler: UsageSampler::new(),
            history: Arc::new(History::disabled()),
            log_watcher,
            connections: 0,
            limiter: None,
            cgroup_base: PathBuf::from(DEFAULT_BASE),
        }
    }

    /// 指定 cgroup v2 的落地目录（`manager::State` 按 `daemon.cgroup_root` 传入）。
    #[must_use]
    pub fn with_cgroup_base(mut self, base: PathBuf) -> Self {
        self.cgroup_base = base;
        self
    }

    /// 接入历史仓库（生产路径由 `manager::State` 注入）。
    #[must_use]
    pub fn with_history(mut self, history: Arc<History>) -> Self {
        self.history = history;
        self
    }

    #[must_use]
    pub fn name(&self) -> &str {
        &self.name
    }

    #[must_use]
    pub fn config(&self) -> &ProcessConfig {
        &self.config
    }

    /// 换一份配置。日志告警分析器在进程运行期间保持原样：采集任务已经持有旧句柄，
    /// 重建只会让新配置收不到日志，因此运行中修改的 `log_alert` 在下一次启动生效。
    pub fn set_config(&mut self, config: ProcessConfig) {
        if !self.is_running() {
            self.log_watcher = config.log_alert.as_ref().and_then(LogWatcher::from_config);
        }
        self.config = config;
    }

    /// 本进程关注的端口（显式声明 + 健康检查端口）。
    #[must_use]
    pub fn watched_ports(&self) -> Vec<u16> {
        self.config.watched_ports()
    }

    pub fn rename(&mut self, new_name: String) {
        self.name = new_name;
    }

    #[must_use]
    pub fn is_running(&self) -> bool {
        matches!(
            self.status,
            ProcessStatus::Running | ProcessStatus::Starting
        )
    }

    pub fn abort_log_tasks(&mut self) {
        for handle in &self.log_tasks {
            handle.abort();
        }
        self.log_tasks.clear();
    }

    #[must_use]
    pub fn should_auto_restart(&self) -> bool {
        self.config.auto_restart && self.crash_restart_count < self.config.max_restarts
    }

    #[must_use]
    pub fn restart_delay(&self) -> std::time::Duration {
        std::time::Duration::from_secs(self.config.restart_delay_secs)
    }

    pub fn reset_crash_restart_count(&mut self) {
        self.crash_restart_count = 0;
        self.health_failures = 0;
    }

    pub fn mark_crash_restart(&mut self) {
        self.crash_restart_count += 1;
    }
}
