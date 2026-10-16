//! 进程管理器：单一 actor 任务串行化全部写操作，读操作走 watch 快照。
//!
//! 子模块分工：
//! - [`state`]：actor 内部状态（进程表、配置基路径、回写）
//! - [`actor`]：命令分发与具体操作
//! - [`monitor`]：2 秒监控循环（探活、自动重启、健康检查）
//! - [`deps`]：依赖拓扑排序

mod actor;
mod deps;
mod monitor;
mod state;

use crate::config::{AppConfig, ProcessConfig};
use crate::error::{GuguError, Result};
use crate::process::{LogEntry, ProcessInfo};
use std::path::PathBuf;
use tokio::sync::{broadcast, mpsc, watch};

pub use deps::topological_sort;

// ── Actor 命令 ──────────────────────────────────────────────

enum Command {
    StartProcess { name: String, reply: ReplyTx<()> },
    StopProcess { name: String, reply: ReplyTx<()> },
    RestartProcess { name: String, reply: ReplyTx<()> },
    CheckHealth { name: String, reply: ReplyTx<bool> },
    AddProcess { name: String, config: ProcessConfig, start_now: bool, reply: ReplyTx<()> },
    UpdateProcess { name: String, config: ProcessConfig, new_name: Option<String>, force_restart: bool, reply: ReplyTx<()> },
    RemoveProcess { name: String, reply: ReplyTx<()> },
    ClearLogs { name: String, reply: ReplyTx<()> },
    ReloadConfig { config: AppConfig, reply: ReplyTx<()> },
    ReloadFromFile { reply: ReplyTx<()> },
    GetProcessConfig { name: String, reply: ReplyTx<Option<ProcessConfig>> },
    GetProcessLogs { name: String, lines: usize, reply: ReplyTx<Vec<LogEntry>> },
    SubscribeLogs { name: String, reply: ReplyTx<broadcast::Receiver<LogEntry>> },
    Shutdown,
}

type ReplyTx<T> = tokio::sync::oneshot::Sender<Result<T>>;

// ── 状态快照 ────────────────────────────────────────────────

/// 进程状态快照，通过 watch 通道无锁读取。
#[derive(Debug, Clone, Default)]
pub struct ProcessSnapshot {
    pub processes: Vec<ProcessInfo>,
}

impl ProcessSnapshot {
    pub fn find(&self, name: &str) -> Option<&ProcessInfo> {
        self.processes.iter().find(|p| p.name == name)
    }

    pub fn names(&self) -> Vec<String> {
        self.processes.iter().map(|p| p.name.clone()).collect()
    }
}

// ── SharedManager（公开句柄） ────────────────────────────────

/// 进程管理器的共享句柄。
///
/// 读操作通过 watch 通道无锁获取快照；
/// 写操作通过 mpsc 命令通道序列化到单一 actor 任务执行。
#[derive(Clone)]
pub struct SharedManager {
    cmd_tx: mpsc::Sender<Command>,
    snapshot: watch::Receiver<ProcessSnapshot>,
}

impl SharedManager {
    // ── 读操作（无锁） ──

    pub fn list_processes(&self) -> Vec<ProcessInfo> {
        self.snapshot.borrow().processes.clone()
    }

    pub fn get_process_info(&self, name: &str) -> Option<ProcessInfo> {
        self.snapshot.borrow().find(name).cloned()
    }

    pub fn all_process_names(&self) -> Vec<String> {
        self.snapshot.borrow().names()
    }

    // ── 写操作（命令通道） ──

    pub async fn start_process(&self, name: &str) -> Result<()> {
        self.call(|r| Command::StartProcess { name: name.to_string(), reply: r }).await
    }

    pub async fn stop_process(&self, name: &str) -> Result<()> {
        self.call(|r| Command::StopProcess { name: name.to_string(), reply: r }).await
    }

    pub async fn restart_process(&self, name: &str) -> Result<()> {
        self.call(|r| Command::RestartProcess { name: name.to_string(), reply: r }).await
    }

    pub async fn check_process_health(&self, name: &str) -> Result<bool> {
        self.call_val(|r| Command::CheckHealth { name: name.to_string(), reply: r }).await
    }

    pub async fn add_process(&self, name: String, config: ProcessConfig, start_now: bool) -> Result<()> {
        self.call(|r| Command::AddProcess { name, config, start_now, reply: r }).await
    }

    pub async fn update_process(&self, name: &str, config: ProcessConfig, new_name: Option<String>, force_restart: bool) -> Result<()> {
        self.call(|r| Command::UpdateProcess { name: name.to_string(), config, new_name, force_restart, reply: r }).await
    }

    pub async fn remove_process(&self, name: &str) -> Result<()> {
        self.call(|r| Command::RemoveProcess { name: name.to_string(), reply: r }).await
    }

    pub async fn get_process_config(&self, name: &str) -> Result<Option<ProcessConfig>> {
        self.call_val(|r| Command::GetProcessConfig { name: name.to_string(), reply: r }).await
    }

    pub async fn get_process_logs(&self, name: &str, lines: usize) -> Result<Vec<LogEntry>> {
        self.call_val(|r| Command::GetProcessLogs { name: name.to_string(), lines, reply: r }).await
    }

    pub async fn clear_process_logs(&self, name: &str) -> Result<()> {
        self.call(|r| Command::ClearLogs { name: name.to_string(), reply: r }).await
    }

    pub async fn subscribe_process_logs(&self, name: &str) -> Result<broadcast::Receiver<LogEntry>> {
        self.call_val(|r| Command::SubscribeLogs { name: name.to_string(), reply: r }).await
    }

    pub async fn reload_config(&self, new_config: &AppConfig) -> Result<()> {
        self.call(|r| Command::ReloadConfig { config: new_config.clone(), reply: r }).await
    }

    pub async fn reload_from_file(&self) -> Result<()> {
        self.call(|r| Command::ReloadFromFile { reply: r }).await
    }

    /// 请求 actor 关闭，停止所有进程后退出循环。
    pub fn shutdown(&self) {
        // Shutdown 不需要回复，fire-and-forget。
        // 用 try_send 避免在 channel 满时阻塞。
        let _ = self.cmd_tx.try_send(Command::Shutdown);
    }

    // ── 内部 ──

    async fn call<F>(&self, f: F) -> Result<()>
    where F: FnOnce(ReplyTx<()>) -> Command {
        let (tx, rx) = tokio::sync::oneshot::channel();
        self.cmd_tx.send(f(tx)).await
            .map_err(|_| GuguError::ConfigError("进程管理器已关闭".into()))?;
        rx.await
            .map_err(|_| GuguError::ConfigError("进程管理器未响应".into()))?
    }

    async fn call_val<T, F>(&self, f: F) -> Result<T>
    where F: FnOnce(ReplyTx<T>) -> Command {
        let (tx, rx) = tokio::sync::oneshot::channel();
        self.cmd_tx.send(f(tx)).await
            .map_err(|_| GuguError::ConfigError("进程管理器已关闭".into()))?;
        rx.await
            .map_err(|_| GuguError::ConfigError("进程管理器未响应".into()))?
    }
}

/// 创建并启动进程管理器 actor，返回共享句柄。
///
/// 内部会自动启动所有 `auto_start` 进程和监控循环。
pub fn start(config: &AppConfig, config_path: Option<PathBuf>) -> SharedManager {
    let (cmd_tx, cmd_rx) = mpsc::channel(256);
    let mut state = state::State::new(config, config_path);
    let snapshot = state.snapshot();
    let (snap_tx, snap_rx) = watch::channel(snapshot);

    tokio::spawn(async move {
        state.start_all().await;
        actor::actor_loop(state, cmd_rx, snap_tx).await;
    });

    SharedManager { cmd_tx, snapshot: snap_rx }
}
