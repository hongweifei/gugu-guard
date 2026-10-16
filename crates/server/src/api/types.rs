//! API 请求/响应类型（仅 api 树内可见）。

use gugu_core::config::{HealthCheckConfig, ProcessConfig};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::path::PathBuf;

// ── 响应类型 ──────────────────────────────────────────────

#[derive(Serialize)]
pub(super) struct ApiSuccess {
    pub(super) message: String,
}

#[derive(Serialize)]
pub(super) struct ApiError {
    error: String,
}

impl ApiError {
    pub(super) fn new(msg: impl Into<String>) -> Self {
        Self { error: msg.into() }
    }
}

#[derive(Serialize)]
pub(super) struct StatsResponse {
    total: usize,
    running: usize,
    stopped: usize,
    failed: usize,
}

impl StatsResponse {
    pub(super) fn new(total: usize, running: usize, stopped: usize, failed: usize) -> Self {
        Self {
            total,
            running,
            stopped,
            failed,
        }
    }
}

#[derive(Serialize)]
pub(super) struct BrowseResponse {
    path: String,
    parent: Option<String>,
    entries: Vec<FsEntry>,
}

impl BrowseResponse {
    pub(super) fn new(path: String, parent: Option<String>, entries: Vec<FsEntry>) -> Self {
        Self {
            path,
            parent,
            entries,
        }
    }
}

#[derive(Serialize)]
pub(super) struct FsEntry {
    pub(super) name: String,
    pub(super) path: String,
    pub(super) is_dir: bool,
}

impl FsEntry {
    pub(super) fn new(name: String, path: String, is_dir: bool) -> Self {
        Self { name, path, is_dir }
    }
}

#[derive(Serialize)]
pub(super) struct GroupInfo {
    name: String,
    processes: Vec<String>,
}

impl GroupInfo {
    pub(super) fn new(name: String, processes: Vec<String>) -> Self {
        Self { name, processes }
    }
}

#[derive(Serialize)]
pub(super) struct BatchResponse {
    message: String,
    total: usize,
    succeeded: usize,
    failed: usize,
    results: Vec<BatchResult>,
}

impl BatchResponse {
    pub(super) fn new(message: String, results: Vec<BatchResult>) -> Self {
        let succeeded = results.iter().filter(|r| r.success).count();
        Self {
            message,
            total: results.len(),
            succeeded,
            failed: results.len() - succeeded,
            results,
        }
    }
}

#[derive(Serialize)]
pub(super) struct BatchResult {
    process: String,
    success: bool,
    error: Option<String>,
}

impl BatchResult {
    pub(super) fn new(process: String, outcome: gugu_core::Result<()>) -> Self {
        Self {
            process,
            success: outcome.is_ok(),
            error: outcome.err().map(|e| e.to_string()),
        }
    }
}

// ── 请求体 ──────────────────────────────────────────────

#[derive(Deserialize)]
pub(super) struct LogQuery {
    #[serde(default = "default_lines")]
    pub(super) lines: usize,
}

fn default_lines() -> usize {
    100
}

#[derive(Deserialize)]
pub(super) struct BrowseQuery {
    pub(super) path: Option<String>,
}

#[derive(Deserialize)]
pub(super) struct CreateProcessRequest {
    command: String,
    #[serde(default)]
    args: Vec<String>,
    working_dir: Option<PathBuf>,
    #[serde(default)]
    env: HashMap<String, String>,
    #[serde(default = "gugu_core::config::default_true")]
    auto_start: bool,
    #[serde(default = "gugu_core::config::default_true")]
    auto_restart: bool,
    #[serde(default = "gugu_core::config::default_max_restarts")]
    max_restarts: u32,
    #[serde(default = "gugu_core::config::default_restart_delay")]
    restart_delay_secs: u64,
    #[serde(default = "gugu_core::config::default_restart_reset_secs")]
    restart_reset_secs: u64,
    #[serde(default)]
    shell: bool,
    #[serde(default)]
    stop_command: Option<String>,
    #[serde(default = "gugu_core::config::default_stop_timeout")]
    stop_timeout_secs: u64,
    #[serde(default)]
    health_check: Option<HealthCheckConfig>,
    #[serde(default)]
    unhealthy_restart: bool,
    #[serde(default)]
    depends_on: Vec<String>,
    #[serde(default)]
    group: Option<String>,
    #[serde(default)]
    max_memory_mb: Option<u64>,
    #[serde(default)]
    max_log_size_mb: Option<u64>,
    stdout_log: Option<PathBuf>,
    stderr_log: Option<PathBuf>,
    #[serde(default)]
    start_now: bool,
    new_name: Option<String>,
}

impl CreateProcessRequest {
    pub(super) fn into_config(self) -> (ProcessConfig, bool, Option<String>) {
        let start_now = self.start_now;
        let new_name = self.new_name;
        let config = ProcessConfig {
            command: self.command,
            args: self.args,
            working_dir: self.working_dir,
            env: self.env,
            auto_start: self.auto_start,
            auto_restart: self.auto_restart,
            max_restarts: self.max_restarts,
            restart_delay_secs: self.restart_delay_secs,
            restart_reset_secs: self.restart_reset_secs,
            shell: self.shell,
            stop_command: self.stop_command,
            stop_timeout_secs: self.stop_timeout_secs,
            health_check: self.health_check,
            unhealthy_restart: self.unhealthy_restart,
            depends_on: self.depends_on,
            group: self.group,
            max_memory_mb: self.max_memory_mb,
            max_log_size_mb: self.max_log_size_mb,
            stdout_log: self.stdout_log,
            stderr_log: self.stderr_log,
        };
        (config, start_now, new_name)
    }
}
