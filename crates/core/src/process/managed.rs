//! `ManagedProcess`：单个被守护进程的生命周期状态机。
//!
//! 平台机制不在此文件：启动命令构造在 [`command`]，停止/强杀在 [`terminate`]，
//! 输出采集在 [`capture`]，Windows 进程树在 [`job`]。

use std::collections::VecDeque;
use std::sync::Arc;

use chrono::{DateTime, Utc};
use tokio::process::{Child, Command};
use tokio::sync::{broadcast, Mutex};
use tokio::task::JoinHandle;

use crate::config::ProcessConfig;
use crate::error::{GuguError, Result};

use super::capture::read_stream;
use super::command::split_command_words;
use super::terminate::{force_kill, run_stop_command, send_default_stop_signal};
use super::types::{ExitRecord, LogEntry, LogStream, ProcessInfo, ProcessStatus};
use super::MAX_EXIT_HISTORY;

#[cfg(windows)]
use super::command::{is_bat_cmd, resolve_program, CREATE_NEW_PROCESS_GROUP, CREATE_NO_WINDOW};
#[cfg(windows)]
use super::job;

const LOG_BROADCAST_CAPACITY: usize = 256;

pub struct ManagedProcess {
    name: String,
    config: ProcessConfig,
    child: Option<Child>,
    #[cfg(windows)]
    job: Option<job::Job>,
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
}

impl ManagedProcess {
    pub fn new(name: String, config: ProcessConfig) -> Self {
        let (log_tx, _) = broadcast::channel(LOG_BROADCAST_CAPACITY);
        Self {
            name,
            config,
            child: None,
            #[cfg(windows)]
            job: None,
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
        }
    }

    #[must_use]
    pub fn name(&self) -> &str {
        &self.name
    }

    #[must_use]
    pub fn config(&self) -> &ProcessConfig {
        &self.config
    }

    pub fn config_mut(&mut self) -> &mut ProcessConfig {
        &mut self.config
    }

    pub fn rename(&mut self, new_name: String) {
        self.name = new_name;
    }

    #[must_use]
    pub fn is_running(&self) -> bool {
        matches!(self.status, ProcessStatus::Running | ProcessStatus::Starting)
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

    /// 记录一次进程退出（保留最近 MAX_EXIT_HISTORY 条，新的在后）。
    pub(crate) fn record_exit(&mut self, code: Option<i32>) {
        self.exit_history.push_back(ExitRecord {
            at: Utc::now(),
            code,
            auto_restarted: false,
        });
        while self.exit_history.len() > MAX_EXIT_HISTORY {
            self.exit_history.pop_front();
        }
    }

    /// 标记最近一次退出是被守护进程自动拉起的（在自动重启真正成功后调用）。
    pub fn mark_last_exit_auto_restarted(&mut self) {
        if let Some(last) = self.exit_history.back_mut() {
            last.auto_restarted = true;
        }
    }

    #[must_use]
    pub fn recent_exits(&self) -> Vec<ExitRecord> {
        self.exit_history.iter().cloned().collect()
    }

    /// 记录一次健康检查结果，返回 (当前连续失败次数, 是否达到判定阈值)。
    /// 达到阈值才允许触发 unhealthy_restart —— 单次抖动不再误伤进程。
    pub fn record_health(&mut self, ok: bool) -> (u32, bool) {
        if ok {
            self.health_failures = 0;
            return (0, false);
        }
        self.health_failures += 1;
        let threshold = self.config.health_check.as_ref().map_or(1, |hc| hc.fail_threshold);
        (self.health_failures, self.health_failures >= threshold)
    }

    /// 进程已运行且存活时长达到 `restart_reset_secs` 时，崩溃重启计数归零，
    /// 让守护长期有效（慢速崩溃循环也能一直被拉起，而非一次事故后永久放弃）。
    #[must_use]
    pub fn maybe_decay_crash_count(&mut self, uptime_secs: i64) -> bool {
        if self.crash_restart_count > 0
            && self.is_running()
            && uptime_secs >= self.config.restart_reset_secs.max(1) as i64
        {
            self.crash_restart_count = 0;
            return true;
        }
        false
    }

    pub async fn start(&mut self) -> Result<()> {
        if self.is_running() {
            return Err(GuguError::AlreadyRunning(self.name.clone()));
        }

        self.status = ProcessStatus::Starting;
        self.abort_log_tasks();

        // shell=true 时保持旧行为：command+args 拼接后交给 shell (支持管道等)。
        // 默认 (shell=false) 直接启动可执行文件，参数以数组传递，
        // 空格路径不再被拆碎，参数中的 & | 等也不会被当作命令执行。
        let mut cmd;
        if self.config.shell {
            let full_cmd = if self.config.args.is_empty() {
                self.config.command.clone()
            } else {
                let args_str = self.config.args.join(" ");
                format!("{} {args_str}", self.config.command)
            };
            #[cfg(windows)]
            {
                cmd = Command::new("cmd");
                cmd.arg("/C").arg(&full_cmd);
                cmd.creation_flags(CREATE_NO_WINDOW | CREATE_NEW_PROCESS_GROUP);
            }
            #[cfg(unix)]
            {
                cmd = Command::new("sh");
                cmd.arg("-c").arg(&full_cmd);
                cmd.process_group(0);
            }
        } else {
            let (exe, mut argv) = split_command_words(&self.config.command);
            if exe.is_empty() {
                self.status = ProcessStatus::Failed("command 为空，无法启动".into());
                return Err(GuguError::StartFailed(self.name.clone(), "command 为空".into()));
            }
            argv.extend(self.config.args.iter().cloned());
            #[cfg(windows)]
            {
                match resolve_program(&exe) {
                    Some(target) if is_bat_cmd(&target) => {
                        if argv.iter().any(|a| a.contains('"')) {
                            tracing::warn!(
                                "[{}] .bat/.cmd 包装执行时，参数中的双引号可能破坏 cmd 解析，建议改用 shell = true",
                                self.name
                            );
                        }
                        cmd = Command::new("cmd");
                        cmd.arg("/D").arg("/C").arg("call").arg(&target);
                    }
                    Some(target) => cmd = Command::new(target),
                    None => cmd = Command::new(&exe),
                }
                cmd.args(&argv);
                cmd.creation_flags(CREATE_NO_WINDOW | CREATE_NEW_PROCESS_GROUP);
            }
            #[cfg(unix)]
            {
                cmd = Command::new(&exe);
                cmd.args(&argv);
                cmd.process_group(0);
            }
        }

        cmd.envs(&self.config.env)
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::piped());

        if let Some(ref dir) = self.config.working_dir {
            cmd.current_dir(dir);
        }

        match cmd.spawn() {
            Ok(mut child) => {
                let pid = child.id();

                // Windows: 将子进程分配到 Job Object，确保进程树可控终止
                #[cfg(windows)]
                {
                    match job::create_kill_on_close_job() {
                        Ok(job) => {
                            if let Some(pid) = pid {
                                if let Err(e) = job.assign_process(pid) {
                                    tracing::warn!(
                                        "[{}] 无法将进程 {} 分配到 Job Object: {}",
                                        self.name, pid, e
                                    );
                                }
                            }
                            self.job = Some(job);
                        }
                        Err(e) => {
                            tracing::warn!("[{}] 创建 Job Object 失败: {}", self.name, e);
                        }
                    }
                }

                let stdout = child.stdout.take();
                let stderr = child.stderr.take();
                let max_log_size = self.config.max_log_size_mb;

                if let Some(stdout) = stdout {
                    let name = self.name.clone();
                    let log_file = self.config.stdout_log.clone();
                    let buffer = self.stdout_lines.clone();
                    let log_tx = self.log_tx.clone();
                    let handle = tokio::spawn(async move {
                        read_stream(stdout, log_file, buffer, log_tx, LogStream::Stdout, &name, max_log_size).await;
                    });
                    self.log_tasks.push(handle);
                }

                if let Some(stderr) = stderr {
                    let name = self.name.clone();
                    let log_file = self.config.stderr_log.clone();
                    let buffer = self.stderr_lines.clone();
                    let log_tx = self.log_tx.clone();
                    let handle = tokio::spawn(async move {
                        read_stream(stderr, log_file, buffer, log_tx, LogStream::Stderr, &name, max_log_size).await;
                    });
                    self.log_tasks.push(handle);
                }

                self.child = Some(child);
                self.started_at = Some(Utc::now());
                self.status = ProcessStatus::Running;
                self.last_health_check = None;
                self.health_failures = 0;

                tracing::info!("[{}] 进程已启动 (PID: {:?})", self.name, pid);
                Ok(())
            }
            Err(e) => {
                self.status = ProcessStatus::Failed(e.to_string());
                tracing::error!("[{}] 启动失败: {}", self.name, e);
                Err(GuguError::StartFailed(self.name.clone(), e.to_string()))
            }
        }
    }

    pub async fn stop(&mut self) -> Result<()> {
        if !self.is_running() {
            return Err(GuguError::NotRunning(self.name.clone()));
        }

        let mut stopped_code: Option<Option<i32>> = None;
        if let Some(ref mut child) = self.child {
            let pid = child.id();
            let stop_cmd = self.config.stop_command.clone();
            let working_dir = self.config.working_dir.clone();
            let env = self.config.env.clone();
            let name = self.name.clone();
            let timeout_secs = self.config.stop_timeout_secs;

            if let Some(ref cmd) = stop_cmd {
                run_stop_command(&name, cmd, working_dir.as_deref(), &env).await;
            } else {
                // Windows: 优先通过 Job Object 终止整棵进程树
                #[cfg(windows)]
                {
                    let terminated_via_job = self.job.as_ref().is_some_and(job::Job::terminate);
                    if terminated_via_job {
                        tracing::debug!("[{}] 已通过 Job Object 终止进程树", name);
                    } else {
                        send_default_stop_signal(pid).await;
                    }
                }
                #[cfg(not(windows))]
                {
                    send_default_stop_signal(pid).await;
                }
            }

            let timeout = std::time::Duration::from_secs(timeout_secs);
            if let Ok(Ok(status)) = tokio::time::timeout(timeout, child.wait()).await {
                stopped_code = Some(status.code());
                tracing::info!(
                    "[{}] 进程已停止 (退出码: {:?})",
                    name,
                    status.code()
                );
            } else {
                // Windows: Job Object 已经终止了进程树，force_kill 作为兜底
                force_kill(child).await;
                if let Ok(status) = child.wait().await {
                    stopped_code = Some(status.code());
                }
                tracing::warn!("[{}] 进程等待超时，已强制终止", name);
            }
        }

        if let Some(code) = stopped_code {
            self.record_exit(code);
        }
        self.abort_log_tasks();
        #[cfg(windows)]
        {
            self.job = None;
        }

        self.child = None;
        self.status = ProcessStatus::Stopped;
        Ok(())
    }

    pub async fn restart(&mut self) -> Result<()> {
        if self.is_running() {
            self.stop().await?;
            tokio::time::sleep(self.restart_delay()).await;
        }
        self.crash_restart_count = 0;
        self.start().await
    }

    pub fn check_alive(&mut self) -> bool {
        if let Some(ref mut child) = self.child {
            match child.try_wait() {
                Ok(Some(status)) => {
                    tracing::info!(
                        "[{}] 进程已退出 (退出码: {:?})",
                        self.name,
                        status.code()
                    );
                    self.record_exit(status.code());
                    self.abort_log_tasks();
                    self.child = None;
                    #[cfg(windows)]
                    {
                        self.job = None;
                    }
                    self.status = ProcessStatus::Stopped;
                    false
                }
                Ok(None) => true,
                Err(_) => {
                    self.abort_log_tasks();
                    self.child = None;
                    #[cfg(windows)]
                    {
                        self.job = None;
                    }
                    self.status = ProcessStatus::Failed("检查进程状态失败".into());
                    false
                }
            }
        } else {
            false
        }
    }

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
        }
    }

    pub async fn logs(&self, lines: usize) -> Vec<LogEntry> {
        let stdout = self.stdout_lines.lock().await;
        let stderr = self.stderr_lines.lock().await;

        let total = stdout.len() + stderr.len();
        if total == 0 {
            return Vec::new();
        }

        let take = lines.min(total);
        let skip = total.saturating_sub(take);

        let mut result = Vec::with_capacity(take);
        let mut si = 0;
        let mut ei = 0;

        let mut skipped = 0;
        while si < stdout.len() || ei < stderr.len() {
            let entry = match (stdout.get(si), stderr.get(ei)) {
                (Some(s), Some(e)) => {
                    if s.timestamp <= e.timestamp {
                        si += 1;
                        s.clone()
                    } else {
                        ei += 1;
                        e.clone()
                    }
                }
                (Some(s), None) => {
                    si += 1;
                    s.clone()
                }
                (None, Some(e)) => {
                    ei += 1;
                    e.clone()
                }
                (None, None) => break,
            };

            skipped += 1;
            if skipped > skip {
                result.push(entry);
            }
            if result.len() >= take {
                break;
            }
        }

        result
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
