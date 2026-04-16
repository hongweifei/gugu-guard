use crate::config::ProcessConfig;
use crate::error::{GuguError, Result};
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use std::collections::VecDeque;
use std::path::PathBuf;
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::process::{Child, Command};
use tokio::sync::Mutex;
use tokio::task::JoinHandle;
use std::sync::Arc;

const MAX_LOG_LINES: usize = 1000;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "lowercase")]
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
    pub started_at: Option<DateTime<Utc>>,
    pub uptime_secs: Option<i64>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LogEntry {
    pub timestamp: DateTime<Utc>,
    pub stream: LogStream,
    pub line: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
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
    started_at: Option<DateTime<Utc>>,
    stdout_lines: Arc<Mutex<VecDeque<LogEntry>>>,
    stderr_lines: Arc<Mutex<VecDeque<LogEntry>>>,
    log_tasks: Vec<JoinHandle<()>>,
}

impl ManagedProcess {
    pub fn new(name: String, config: ProcessConfig) -> Self {
        Self {
            name,
            config,
            child: None,
            status: ProcessStatus::Stopped,
            restart_count: 0,
            started_at: None,
            stdout_lines: Arc::new(Mutex::new(VecDeque::new())),
            stderr_lines: Arc::new(Mutex::new(VecDeque::new())),
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
        self.config.auto_restart && self.restart_count < self.config.max_restarts
    }

    pub fn restart_delay(&self) -> std::time::Duration {
        std::time::Duration::from_secs(self.config.restart_delay_secs)
    }

    pub fn reset_restart_count(&mut self) {
        self.restart_count = 0;
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

                if let Some(stdout) = stdout {
                    let name = self.name.clone();
                    let log_file = self.config.stdout_log.clone();
                    let buffer = self.stdout_lines.clone();
                    let handle = tokio::spawn(async move {
                        read_stream(stdout, log_file, buffer, LogStream::Stdout, &name).await;
                    });
                    self.log_tasks.push(handle);
                }

                if let Some(stderr) = stderr {
                    let name = self.name.clone();
                    let log_file = self.config.stderr_log.clone();
                    let buffer = self.stderr_lines.clone();
                    let handle = tokio::spawn(async move {
                        read_stream(stderr, log_file, buffer, LogStream::Stderr, &name).await;
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
        self.restart_count = 0;
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
            restart_count: self.restart_count,
            auto_start: self.config.auto_start,
            auto_restart: self.config.auto_restart,
            started_at: self.started_at,
            uptime_secs,
        }
    }

    pub async fn logs(&self, lines: usize) -> Vec<LogEntry> {
        let stdout = self.stdout_lines.lock().await;
        let stderr = self.stderr_lines.lock().await;
        let mut all: Vec<LogEntry> = stdout.iter().chain(stderr.iter()).cloned().collect();
        all.sort_by_key(|e| e.timestamp);
        let start = all.len().saturating_sub(lines);
        all[start..].to_vec()
    }

    pub fn set_status(&mut self, status: ProcessStatus) {
        self.status = status;
    }
}

async fn read_stream<R: tokio::io::AsyncRead + Unpin>(
    reader: R,
    log_path: Option<PathBuf>,
    buffer: Arc<Mutex<VecDeque<LogEntry>>>,
    stream: LogStream,
    name: &str,
) {
    let mut buf_reader = BufReader::new(reader);
    let mut line = String::new();
    let mut file = open_log_file(log_path.as_deref()).await;

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
                };

                if let Some(ref mut f) = file {
                    let _ = f.write_all(format!("{trimmed}\n").as_bytes()).await;
                }

                let mut buf = buffer.lock().await;
                if buf.len() >= MAX_LOG_LINES {
                    buf.pop_front();
                }
                buf.push_back(entry);
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
                let _ = tokio::fs::create_dir_all(parent).await;
            }
            tokio::fs::OpenOptions::new()
                .create(true)
                .append(true)
                .open(p)
                .await
                .ok()
        }
        None => None,
    }
}

fn stream_type(stream: &LogStream) -> &'static str {
    match stream {
        LogStream::Stdout => "stdout",
        LogStream::Stderr => "stderr",
    }
}
