//! API 请求/响应类型（仅 api 树内可见）。

use gugu_core::config::{HealthCheckConfig, ProcessConfig};
use gugu_core::logs::{LogLevel, LogQuery};
use gugu_core::process::LogStream;
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

/// 日志接口的查询参数：实时读取与历史文件检索共用一套过滤维度。
#[derive(Deserialize)]
pub(super) struct LogQueryParams {
    /// 实时读取的条数上限。
    #[serde(default = "default_lines")]
    pub(super) lines: usize,
    /// 历史文件检索的命中条数上限。
    #[serde(default = "default_search_limit")]
    pub(super) limit: usize,
    /// 最低级别：trace / debug / info / warn / error / fatal；`all` 或留空表示不限。
    pub(super) level: Option<String>,
    /// 关键字，空白分隔，全部命中才算匹配。
    pub(super) q: Option<String>,
    /// `stdout` / `stderr`；留空或 `all` 表示不限。
    pub(super) stream: Option<String>,
    /// 历史检索时每个文件最多倒读的 MB 数。
    #[serde(default = "default_budget_mb")]
    pub(super) budget_mb: u64,
}

impl LogQueryParams {
    /// 实时读取条件。
    ///
    /// # Errors
    /// 级别或输出流写错时返回可直接展示给用户的错误说明。
    pub(super) fn live_query(&self) -> Result<LogQuery, String> {
        Ok(self
            .filters()?
            .into_query(self.lines.clamp(1, MAX_LIVE_LINES)))
    }

    /// 历史文件检索条件。
    ///
    /// # Errors
    /// 级别或输出流写错时返回可直接展示给用户的错误说明。
    pub(super) fn search_query(&self) -> Result<LogQuery, String> {
        Ok(self
            .filters()?
            .into_query(self.limit.clamp(1, MAX_SEARCH_LIMIT)))
    }

    /// 每个文件的最大倒读字节数。
    pub(super) fn budget_bytes(&self) -> u64 {
        self.budget_mb
            .clamp(1, MAX_BUDGET_MB)
            .saturating_mul(1024 * 1024)
    }

    fn filters(&self) -> Result<LogFilters, String> {
        let stream = match self.stream.as_deref().map(str::trim) {
            None | Some("" | "all") => None,
            Some("stdout") => Some(LogStream::Stdout),
            Some("stderr") => Some(LogStream::Stderr),
            Some(other) => return Err(format!("未知输出流 {other}，可选 stdout / stderr / all")),
        };
        let min_level = match self.level.as_deref().map(str::trim) {
            None | Some("" | "all") => None,
            Some(raw) => Some(LogLevel::parse(raw).ok_or_else(|| {
                format!("未知日志级别 {raw}，可选 trace / debug / info / warn / error / fatal")
            })?),
        };
        Ok(LogFilters {
            stream,
            min_level,
            keywords: LogQuery::keywords_from(self.q.as_deref()),
        })
    }
}

/// 查询参数里解析出的过滤维度，行为上等价于不含时间窗与条数上限的 [`LogQuery`]。
struct LogFilters {
    stream: Option<LogStream>,
    min_level: Option<LogLevel>,
    keywords: Vec<String>,
}

impl LogFilters {
    fn into_query(self, limit: usize) -> LogQuery {
        LogQuery {
            stream: self.stream,
            min_level: self.min_level,
            keywords: self.keywords,
            since: None,
            until: None,
            limit,
        }
    }
}

const MAX_LIVE_LINES: usize = 1000;
const MAX_SEARCH_LIMIT: usize = 1000;
const MAX_BUDGET_MB: u64 = 32;

fn default_lines() -> usize {
    100
}

fn default_search_limit() -> usize {
    200
}

fn default_budget_mb() -> u64 {
    4
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
    /// cgroup v2 硬限：CPU（`50%` / `0.5` / `50000 100000`）。
    #[serde(default)]
    cpu_max: Option<String>,
    /// cgroup v2 硬限：内存（`512M` / `2G` / 字节数）。
    #[serde(default)]
    memory_max: Option<String>,
    /// cgroup v2 硬限：任务数。
    #[serde(default)]
    pids_max: Option<String>,
    #[serde(default)]
    max_log_size_mb: Option<u64>,
    stdout_log: Option<PathBuf>,
    stderr_log: Option<PathBuf>,
    #[serde(default)]
    log_format: Option<gugu_core::logs::LogFormat>,
    #[serde(default)]
    listen_ports: Vec<u16>,
    #[serde(default = "gugu_core::config::default_true")]
    port_check: bool,
    #[serde(default = "gugu_core::config::default_ready_timeout")]
    ready_timeout_secs: u64,
    #[serde(default)]
    log_alert: Option<gugu_core::config::LogAlertConfig>,
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
            cpu_max: self.cpu_max,
            memory_max: self.memory_max,
            pids_max: self.pids_max,
            max_log_size_mb: self.max_log_size_mb,
            stdout_log: self.stdout_log,
            stderr_log: self.stderr_log,
            log_format: self.log_format.unwrap_or_default(),
            listen_ports: self.listen_ports,
            port_check: self.port_check,
            ready_timeout_secs: self.ready_timeout_secs,
            log_alert: self.log_alert,
        };
        (config, start_now, new_name)
    }
}
