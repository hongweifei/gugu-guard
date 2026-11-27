//! 停止、重启、存活检查与退出记录。

use chrono::Utc;

use crate::error::{GuguError, Result};
use crate::process::terminate::{force_kill, run_stop_command, send_default_stop_signal};
use crate::process::types::{ExitRecord, ProcessStatus};
use crate::process::MAX_EXIT_HISTORY;

#[cfg(windows)]
use crate::cgroup::Limiter;

use super::ManagedProcess;

impl ManagedProcess {
    /// 记录一次进程退出（保留最近 MAX_EXIT_HISTORY 条，新的在后），并写进退出日志。
    ///
    /// `stopped` 标记操作者主动停止：这类退出不计入崩溃次数，但照常计入运行时长。
    pub(crate) fn record_exit(&mut self, code: Option<i32>, stopped: bool) {
        let at = Utc::now();
        let uptime_secs = self.started_at.map(|t| (at - t).num_seconds());
        self.exit_history.push_back(ExitRecord {
            at,
            code,
            auto_restarted: false,
        });
        while self.exit_history.len() > MAX_EXIT_HISTORY {
            self.exit_history.pop_front();
        }
        self.history
            .mark_exit(&self.name, at, code, uptime_secs, stopped);
    }

    /// 标记最近一次退出是被守护进程自动拉起的（在自动重启真正成功后调用）。
    pub fn mark_last_exit_auto_restarted(&mut self) {
        if let Some(last) = self.exit_history.back_mut() {
            last.auto_restarted = true;
        }
        self.history.mark_revived(&self.name, Utc::now());
    }

    #[must_use]
    pub fn recent_exits(&self) -> Vec<ExitRecord> {
        self.exit_history.iter().cloned().collect()
    }

    /// 释放硬限载体：Linux 杀掉还留在 cgroup 里的子进程并删除目录，
    /// Windows 终止 Job 里的进程树。进程退出后也要调用。
    fn drop_limiter(&mut self) {
        if let Some(limiter) = self.limiter.take() {
            limiter.kill_and_remove();
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
                    let terminated_via_job = self.limiter.as_ref().is_some_and(Limiter::terminate);
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
                tracing::info!("[{}] 进程已停止 (退出码: {:?})", name, status.code());
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
            self.record_exit(code, true);
        }
        // 主进程停了，硬限载体里可能还有它拉起的子进程：一并结束
        self.drop_limiter();
        self.usage = None;
        self.connections = 0;
        self.abort_log_tasks();

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
                    tracing::info!("[{}] 进程已退出 (退出码: {:?})", self.name, status.code());
                    self.record_exit(status.code(), false);
                    self.drop_limiter();
                    self.usage = None;
                    self.connections = 0;
                    self.abort_log_tasks();
                    self.child = None;
                    self.status = ProcessStatus::Stopped;
                    false
                }
                Ok(None) => true,
                Err(_) => {
                    self.abort_log_tasks();
                    self.drop_limiter();
                    self.child = None;
                    self.status = ProcessStatus::Failed("检查进程状态失败".into());
                    false
                }
            }
        } else {
            false
        }
    }
}
