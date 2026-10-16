//! 进程状态与数据模型（可序列化，供 API / WebUI / 持久层共用）。

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

use super::usage::UsageSample;

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
    pub group: Option<String>,
    pub started_at: Option<DateTime<Utc>>,
    pub uptime_secs: Option<i64>,
    /// 最近几次退出的历史（新的在后，最多 [`crate::process::MAX_EXIT_HISTORY`] 条）。
    #[serde(default)]
    pub recent_exits: Vec<ExitRecord>,
    /// 进程树资源占用（CPU 占单核百分比 + 内存），未运行时为 None。
    #[serde(default)]
    pub usage: Option<UsageSample>,
}

/// 一次进程退出的记录：用于排障（「昨晚崩了 4 次，每次退出码 137」）。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ExitRecord {
    pub at: DateTime<Utc>,
    /// 退出码；被信号杀死等场景为 None。
    pub code: Option<i32>,
    /// 退出后是否由守护进程自动拉起（手动停止或未配置自动重启时为 false）。
    pub auto_restarted: bool,
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
