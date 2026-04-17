use crate::config::ProcessConfig;
use crate::error::{GuguError, Result};
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use std::collections::VecDeque;
use std::path::PathBuf;
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::process::{Child, Command};
use tokio::sync::{broadcast, Mutex};
use tokio::task::JoinHandle;
use std::sync::Arc;

const MAX_LOG_LINES: usize = 1000;
const LOG_BROADCAST_CAPACITY: usize = 256;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "lowercase")]
#[non_exhaustive]
pub enum ProcessStatus {
    Stopped,
    Running,
    Starting,
    Failed(String),
    Restarting,
}

impl std::fmt::Display for ProcessStatus {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Stopped => write!(f, "已停止"),
            Self::Running => write!(f, "运行中"),
            Self::Starting => write!(f, "启动中"),
            Self::Failed(e) => write!(f, "失败: {e}"),
            Self::Restarting => write!(f, "重启中"),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ProcessInfo {
    pub name: String,
    pub command: String,
    pub args: Vec<String>,
    pub status: ProcessStatus,
    pub pid: Option<u32>,
    pub restart_count: u32,
    pub auto_start: bool,
    pub auto_restart: bool,
    pub has_health_check: bool,
    pub unhealthy_restart: bool,
    pub healthy: Option<bool>,
    pub started_at: Option<DateTime<Utc>>,
    pub uptime_secs: Option<i64>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LogEntry {
    pub timestamp: DateTime<Utc>,
    pub stream: LogStream,
    pub line: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub process_name: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
#[non_exhaustive]
pub enum LogStream {
    Stdout,
    Stderr,
}

pub struct ManagedProcess {
    name: String,
    config: ProcessConfig,
    child: Option<Child>,
    status: ProcessStatus,
    restart_count: u32,
    crash_restart_count: u32,
    healthy: Option<bool>,
    started_at: Option<DateTime<Utc>>,
    stdout_lines: Arc<Mutex<VecDeque<LogEntry>>>,
    stderr_lines: Arc<Mutex<VecDeque<LogEntry>>>,
    log_tx: broadcast::Sender<LogEntry>,
    log_tasks: Vec<JoinHandle<()>>,
}

impl ManagedProcess {
    pub fn new(name: String, config: ProcessConfig) -> Self {
        let (log_tx, _) = broadcast::channel(LOG_BROADCAST_CAPACITY);
        Self {
            name,
            config,
            child: None,
            status: ProcessStatus::Stopped,
            restart_count: 0,
            crash_restart_count: 0,
            healthy: None,
            started_at: None,
            stdout_lines: Arc::new(Mutex::new(VecDeque::new())),
            stderr_lines: Arc::new(Mutex::new(VecDeque::new())),
            log_tx,
            log_tasks: Vec::new(),
        }
    }

    pub fn name(&self) -> &str {
        &self.name
    }

    pub fn config(&self) -> &ProcessConfig {
        &self.config
    }

    pub fn config_mut(&mut self) -> &mut ProcessConfig {
        &mut self.config
    }

    pub fn rename(&mut self, new_name: String) {
        self.name = new_name;
    }

    pub fn is_running(&self) -> bool {
        matches!(self.status, ProcessStatus::Running | ProcessStatus::Starting)
    }

    pub fn should_auto_restart(&self) -> bool {
        self.config.auto_restart && self.crash_restart_count < self.config.max_restarts
    }

    pub fn restart_delay(&self) -> std::time::Duration {
        std::time::Duration::from_secs(self.config.restart_delay_secs)
    }

    pub fn reset_crash_restart_count(&mut self) {
        self.crash_restart_count = 0;
    }

    pub fn mark_crash_restart(&mut self) {
        self.crash_restart_count += 1;
    }

    pub async fn start(&mut self) -> Result<()> {
        if self.is_running() {
            return Err(GuguError::AlreadyRunning(self.name.clone()));
        }

        self.status = ProcessStatus::Starting;
        self.log_tasks.clear();

        let full_cmd = if self.config.args.is_empty() {
            self.config.command.clone()
        } else {
            let args_str = self.config.args.join(" ");
            format!("{} {args_str}", self.config.command)
        };

        let mut cmd;
        #[cfg(windows)]
        {
            cmd = Command::new("cmd");
            cmd.arg("/C").arg(&full_cmd);
            #[allow(unused_imports)]
            use std::os::windows::process::CommandExt;
            cmd.creation_flags(0x08000000);
        }
        #[cfg(unix)]
        {
            cmd = Command::new("sh");
            cmd.arg("-c").arg(&full_cmd);
            use std::os::unix::process::CommandExt;
            cmd.process_group(0);
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
                self.restart_count += 1;

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

        if let Some(ref mut child) = self.child {
            if let Some(pid) = child.id() {
                #[cfg(windows)]
                {
                    #[allow(unused_imports)]
                    use std::os::windows::process::CommandExt;
                    let _ = tokio::process::Command::new("taskkill")
                        .args(["/PID", &pid.to_string(), "/T", "/F"])
                        .creation_flags(0x08000000)
                        .stdout(std::process::Stdio::null())
                        .stderr(std::process::Stdio::null())
                        .status()
                        .await;
                }

                #[cfg(unix)]
                {
                    unsafe {
                        libc::kill(-(pid as i32), libc::SIGTERM);
                    }
                }
            }

            let timeout = std::time::Duration::from_secs(self.config.stop_timeout_secs);
            match tokio::time::timeout(timeout, child.wait()).await {
                Ok(Ok(status)) => {
                    tracing::info!(
                        "[{}] 进程已停止 (退出码: {:?})",
                        self.name,
                        status.code()
                    );
                }
                _ => {
                    #[cfg(unix)]
                    if let Some(pid) = child.id() {
                        unsafe {
                            libc::kill(-(pid as i32), libc::SIGKILL);
                        }
                    }
                    #[cfg(windows)]
                    {
                        #[allow(unused_imports)]
                        use std::os::windows::process::CommandExt;
                        let _ = tokio::process::Command::new("taskkill")
                            .args(["/PID", &child.id().unwrap_or_default().to_string(), "/T", "/F"])
                            .creation_flags(0x08000000)
                            .stdout(std::process::Stdio::null())
                            .stderr(std::process::Stdio::null())
                            .status()
                            .await;
                    }
                    let _ = child.wait().await;
                    tracing::warn!("[{}] 进程等待超时，已强制终止", self.name);
                }
            }
        }

        for handle in &self.log_tasks {
            handle.abort();
        }
        self.log_tasks.clear();

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
                    self.child = None;
                    self.status = ProcessStatus::Stopped;
                    false
                }
                Ok(None) => true,
                Err(_) => {
                    self.child = None;
                    self.status = ProcessStatus::Failed("检查进程状态失败".into());
                    false
                }
            }
        } else {
            false
        }
    }

    pub fn info(&self) -> ProcessInfo {
        let uptime_secs = self.started_at.map(|t| (Utc::now() - t).num_seconds());
        ProcessInfo {
            name: self.name.clone(),
            command: self.config.command.clone(),
            args: self.config.args.clone(),
            status: self.status.clone(),
            pid: self.child.as_ref().and_then(|c| c.id()),
            restart_count: self.crash_restart_count,
            auto_start: self.config.auto_start,
            auto_restart: self.config.auto_restart,
            has_health_check: self.config.health_check.is_some(),
            unhealthy_restart: self.config.unhealthy_restart,
            healthy: self.healthy,
            started_at: self.started_at,
            uptime_secs,
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

        let mut result = Vec::with_capacity(total);
        let mut si = 0;
        let mut ei = 0;

        while si < stdout.len() || ei < stderr.len() {
            match (stdout.get(si), stderr.get(ei)) {
                (Some(s), Some(e)) => {
                    if s.timestamp <= e.timestamp {
                        result.push(s.clone());
                        si += 1;
                    } else {
                        result.push(e.clone());
                        ei += 1;
                    }
                }
                (Some(s), None) => {
                    result.push(s.clone());
                    si += 1;
                }
                (None, Some(e)) => {
                    result.push(e.clone());
                    ei += 1;
                }
                (None, None) => break,
            }
        }

        if skip > 0 {
            result[skip..].to_vec()
        } else {
            result
        }
    }

    pub fn set_status(&mut self, status: ProcessStatus) {
        self.status = status;
    }

    pub fn set_healthy(&mut self, healthy: Option<bool>) {
        self.healthy = healthy;
    }

    pub fn subscribe_logs(&self) -> broadcast::Receiver<LogEntry> {
        self.log_tx.subscribe()
    }

    pub async fn clear_logs(&self) {
        self.stdout_lines.lock().await.clear();
        self.stderr_lines.lock().await.clear();
    }
}

async fn read_stream<R: tokio::io::AsyncRead + Unpin>(
    reader: R,
    log_path: Option<PathBuf>,
    buffer: Arc<Mutex<VecDeque<LogEntry>>>,
    log_tx: broadcast::Sender<LogEntry>,
    stream: LogStream,
    name: &str,
    max_log_size_mb: Option<u64>,
) {
    let mut buf_reader = BufReader::new(reader);
    let mut line = String::new();
    let mut file = open_log_file(log_path.as_deref()).await;
    let max_bytes = max_log_size_mb.unwrap_or(0) * 1024 * 1024;
    let mut line_count: u32 = 0;

    loop {
        line.clear();
        match buf_reader.read_line(&mut line).await {
            Ok(0) => break,
            Ok(_) => {
                let trimmed = line.trim_end_matches('\n').trim_end_matches('\r');
                let entry = LogEntry {
                    timestamp: Utc::now(),
                    stream: stream.clone(),
                    line: trimmed.to_string(),
                    process_name: None,
                };

                if let Some(ref mut f) = file {
                    if let Err(e) = f.write_all(format!("{trimmed}\n").as_bytes()).await {
                        tracing::debug!("[{}] 写入日志文件失败: {}", name, e);
                    }
                }

                let mut buf = buffer.lock().await;
                if buf.len() >= MAX_LOG_LINES {
                    buf.pop_front();
                }
                buf.push_back(entry.clone());
                drop(buf);

                let _ = log_tx.send(entry);

                line_count += 1;
                if max_bytes > 0 && line_count.is_multiple_of(256) {
                    let should_rotate = match file.as_ref() {
                        Some(f) => f.metadata().await.map(|m| m.len() > max_bytes).unwrap_or(false),
                        None => false,
                    };
                    if should_rotate {
                        file = rotate_log_file(log_path.as_deref()).await;
                    }
                }
            }
            Err(e) => {
                tracing::debug!("[{}] 读取 {} 流错误: {}", name, stream_type(&stream), e);
                break;
            }
        }
    }
}

async fn open_log_file(path: Option<&std::path::Path>) -> Option<tokio::fs::File> {
    match path {
        Some(p) => {
            if let Some(parent) = p.parent() {
                if let Err(e) = tokio::fs::create_dir_all(parent).await {
                    tracing::debug!("创建日志目录失败 {}: {}", parent.display(), e);
                }
            }
            match tokio::fs::OpenOptions::new()
                .create(true)
                .append(true)
                .open(p)
                .await
            {
                Ok(f) => Some(f),
                Err(e) => {
                    tracing::debug!("打开日志文件失败 {}: {}", p.display(), e);
                    None
                }
            }
        }
        None => None,
    }
}

async fn rotate_log_file(path: Option<&std::path::Path>) -> Option<tokio::fs::File> {
    let p = match path {
        Some(p) => p,
        None => return None,
    };

    for i in (1..=5).rev() {
        let old = format!("{}.{}", p.display(), i);
        let old_path = std::path::Path::new(&old);
        if old_path.exists() {
            let next = format!("{}.{}", p.display(), i + 1);
            let _ = tokio::fs::rename(old_path, &next).await;
        }
    }
    let _ = tokio::fs::rename(p, format!("{}.1", p.display())).await;

    tokio::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(p)
        .await
        .ok()
}

fn stream_type(stream: &LogStream) -> &'static str {
    match stream {
        LogStream::Stdout => "stdout",
        LogStream::Stderr => "stderr",
    }
}
