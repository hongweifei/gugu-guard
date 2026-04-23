use std::collections::VecDeque;
use std::path::PathBuf;
use std::sync::Arc;

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::process::{Child, Command};
use tokio::sync::{broadcast, Mutex};
use tokio::task::JoinHandle;

use crate::config::ProcessConfig;
use crate::error::{GuguError, Result};

const MAX_LOG_LINES: usize = 1000;
const LOG_BROADCAST_CAPACITY: usize = 256;
// CREATE_NO_WINDOW | CREATE_NEW_PROCESS_GROUP
const CREATE_NO_WINDOW: u32 = 0x0800_0000;
const CREATE_NEW_PROCESS_GROUP: u32 = 0x0000_0200;

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

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
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
    #[cfg(windows)]
    job: Option<windows_sys_job::Job>,
    status: ProcessStatus,
    crash_restart_count: u32,
    healthy: Option<bool>,
    started_at: Option<DateTime<Utc>>,
    stdout_lines: Arc<Mutex<VecDeque<LogEntry>>>,
    stderr_lines: Arc<Mutex<VecDeque<LogEntry>>>,
    log_tx: broadcast::Sender<LogEntry>,
    log_tasks: Vec<JoinHandle<()>>,
    last_health_check: Option<std::time::Instant>,
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
            healthy: None,
            started_at: None,
            stdout_lines: Arc::new(Mutex::new(VecDeque::new())),
            stderr_lines: Arc::new(Mutex::new(VecDeque::new())),
            log_tx,
            log_tasks: Vec::new(),
            last_health_check: None,
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
    }

    pub fn mark_crash_restart(&mut self) {
        self.crash_restart_count += 1;
    }

    pub async fn start(&mut self) -> Result<()> {
        if self.is_running() {
            return Err(GuguError::AlreadyRunning(self.name.clone()));
        }

        self.status = ProcessStatus::Starting;
        self.abort_log_tasks();

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
            cmd.creation_flags(CREATE_NO_WINDOW | CREATE_NEW_PROCESS_GROUP);
        }
        #[cfg(unix)]
        {
            cmd = Command::new("sh");
            cmd.arg("-c").arg(&full_cmd);
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

                // Windows: 将子进程分配到 Job Object，确保进程树可控终止
                #[cfg(windows)]
                {
                    match windows_sys_job::create_kill_on_close_job() {
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
                    let terminated_via_job = self.job.as_ref().is_some_and(windows_sys_job::Job::terminate);
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
                tracing::info!(
                    "[{}] 进程已停止 (退出码: {:?})",
                    name,
                    status.code()
                );
            } else {
                // Windows: Job Object 已经终止了进程树，force_kill 作为兜底
                force_kill(child).await;
                let _ = child.wait().await;
                tracing::warn!("[{}] 进程等待超时，已强制终止", name);
            }
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

async fn run_stop_command(name: &str, stop_cmd: &str, working_dir: Option<&std::path::Path>, env: &std::collections::HashMap<String, String>) {
    let mut cmd;
    #[cfg(windows)]
    {
        cmd = tokio::process::Command::new("cmd");
        cmd.arg("/C").arg(stop_cmd);
        cmd.creation_flags(CREATE_NO_WINDOW);
    }
    #[cfg(unix)]
    {
        cmd = tokio::process::Command::new("sh");
        cmd.arg("-c").arg(stop_cmd);
    }

    if let Some(dir) = working_dir {
        cmd.current_dir(dir);
    }

    cmd.envs(env)
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null());

    tracing::info!("[{}] 执行自定义停止命令: {}", name, stop_cmd);
    match cmd.status().await {
        Ok(s) => tracing::debug!("[{}] 停止命令退出码: {:?}", name, s.code()),
        Err(e) => tracing::warn!("[{}] 停止命令执行失败: {}", name, e),
    }
}

async fn send_default_stop_signal(pid: Option<u32>) {
    if let Some(pid) = pid {
        #[cfg(windows)]
        {
            let _ = tokio::process::Command::new("taskkill")
                .args(["/PID", &pid.to_string(), "/T"])
                .creation_flags(CREATE_NO_WINDOW)
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
}

async fn force_kill(child: &mut Child) {
    #[cfg(unix)]
    if let Some(pid) = child.id() {
        unsafe {
            libc::kill(-(pid as i32), libc::SIGKILL);
        }
    }
    #[cfg(windows)]
    {
        // force_kill 是兜底，直接用 taskkill /F 确保进程终止
        let _ = tokio::process::Command::new("taskkill")
            .args(["/PID", &child.id().unwrap_or_default().to_string(), "/T", "/F"])
            .creation_flags(CREATE_NO_WINDOW)
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .status()
            .await;
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
    let p = path?;

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

async fn rotate_log_file(path: Option<&std::path::Path>) -> Option<tokio::fs::File> {
    let p = path?;

    for i in (1..=5).rev() {
        let old = append_suffix(p, i);
        let next = append_suffix(p, i + 1);
        let _ = tokio::fs::rename(&old, &next).await;
    }
    let _ = tokio::fs::rename(p, append_suffix(p, 1)).await;

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

fn append_suffix(path: &std::path::Path, n: u32) -> PathBuf {
    let stem = path.file_stem().map(|s| s.to_string_lossy().to_string()).unwrap_or_default();
    let ext = path.extension().map(|e| format!(".{}", e.to_string_lossy()));
    let parent = path.parent().unwrap_or(std::path::Path::new("."));
    match ext {
        Some(ext) => parent.join(format!("{stem}.{n}{ext}")),
        None => parent.join(format!("{stem}.{n}")),
    }
}

/// Windows Job Object 封装，用于可靠管理进程树生命周期。
///
/// 核心机制：
/// - `JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE`: 当 Job 句柄关闭（守护进程退出/崩溃）时
///   自动终止所有关联进程，防止孤儿进程
/// - `TerminateJobObject`: 主动终止整棵进程树，比 `taskkill /T` 更可靠
///   因为 Job Object 跟踪所有后代进程，包括 cmd /C 产生的脱离进程树的子进程
#[cfg(windows)]
mod windows_sys_job {
    use std::io;
    use windows_sys::Win32::Foundation::CloseHandle;
    use windows_sys::Win32::System::JobObjects::{
        AssignProcessToJobObject, SetInformationJobObject, TerminateJobObject,
        JOBOBJECT_BASIC_LIMIT_INFORMATION, JOBOBJECT_EXTENDED_LIMIT_INFORMATION,
        JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE, JobObjectExtendedLimitInformation,
    };
    use windows_sys::Win32::System::Threading::{OpenProcess, PROCESS_SET_QUOTA, PROCESS_TERMINATE};

    /// 以 usize 存储 HANDLE 原始值，使 Job 满足 Send（*mut c_void 不满足 Send）。
    /// SAFE: HANDLE 本质上是内核对象的整数标识符，可在线程间安全传递。
    pub struct Job {
        handle: usize,
    }

    // SAFETY: Windows HANDLE 是内核对象标识符，可安全跨线程使用。
    unsafe impl Send for Job {}

    extern "system" {
        fn CreateJobObjectW(lpjobattributes: *const std::ffi::c_void, lpname: *const u16) -> *mut std::ffi::c_void;
    }

    impl Job {
        /// 将进程分配到此 Job Object。
        pub fn assign_process(&self, pid: u32) -> io::Result<()> {
            let proc_handle = unsafe {
                OpenProcess(PROCESS_SET_QUOTA | PROCESS_TERMINATE, 0, pid)
            };
            if proc_handle.is_null() {
                return Err(io::Error::last_os_error());
            }
            let result = unsafe {
                AssignProcessToJobObject(self.as_handle(), proc_handle)
            };
            unsafe { CloseHandle(proc_handle) };
            if result == 0 {
                Err(io::Error::last_os_error())
            } else {
                Ok(())
            }
        }

        /// 终止 Job 中所有进程。成功返回 true。
        pub fn terminate(&self) -> bool {
            unsafe { TerminateJobObject(self.as_handle(), 1) != 0 }
        }

        fn as_handle(&self) -> windows_sys::Win32::Foundation::HANDLE {
            self.handle as *mut _
        }
    }

    impl Drop for Job {
        fn drop(&mut self) {
            if self.handle != 0 {
                unsafe { CloseHandle(self.as_handle()) };
            }
        }
    }

    /// 创建匿名 Job Object，设置 KILL_ON_JOB_CLOSE 以确保进程树在句柄关闭时被清理。
    pub fn create_kill_on_close_job() -> io::Result<Job> {
        let handle = unsafe { CreateJobObjectW(std::ptr::null(), std::ptr::null()) };
        if handle.is_null() {
            return Err(io::Error::last_os_error());
        }

        let info = JOBOBJECT_EXTENDED_LIMIT_INFORMATION {
            BasicLimitInformation: JOBOBJECT_BASIC_LIMIT_INFORMATION {
                LimitFlags: JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE,
                ..unsafe { std::mem::zeroed() }
            },
            ..unsafe { std::mem::zeroed() }
        };

        let h: windows_sys::Win32::Foundation::HANDLE = handle;
        let result = unsafe {
            SetInformationJobObject(
                h,
                JobObjectExtendedLimitInformation,
                &info as *const _ as *const _,
                std::mem::size_of::<JOBOBJECT_EXTENDED_LIMIT_INFORMATION>() as u32,
            )
        };
        if result == 0 {
            unsafe { CloseHandle(h) };
            return Err(io::Error::last_os_error());
        }

        Ok(Job { handle: handle as usize })
    }
}
