//! 进程状态与数据模型（可序列化，供 API / WebUI / 持久层共用）。

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

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
