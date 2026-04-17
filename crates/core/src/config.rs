use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::path::PathBuf;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AppConfig {
    #[serde(default)]
    pub daemon: DaemonConfig,
    #[serde(default)]
    pub processes: HashMap<String, ProcessConfig>,
}

impl AppConfig {
    pub fn load(path: &std::path::Path) -> crate::error::Result<Self> {
        if !path.exists() {
            return Ok(Self::default());
        }
        let content = std::fs::read_to_string(path)
            .map_err(|e| crate::error::GuguError::ConfigError(format!("读取配置文件失败: {e}")))?;
        let config: Self = toml::from_str(&content)
            .map_err(|e| crate::error::GuguError::ConfigError(format!("解析配置文件失败: {e}")))?;
        Ok(config)
    }

    pub fn save(&self, path: &std::path::Path) -> crate::error::Result<()> {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)
                .map_err(|e| crate::error::GuguError::ConfigError(format!("创建目录失败: {e}")))?;
        }
        let content = toml::to_string_pretty(self)
            .map_err(|e| crate::error::GuguError::ConfigError(format!("序列化配置失败: {e}")))?;
        std::fs::write(path, content)
            .map_err(|e| crate::error::GuguError::ConfigError(format!("写入配置文件失败: {e}")))?;
        Ok(())
    }

    pub fn server_addr(&self) -> String {
        let addr = self.daemon.web.addr.as_deref().unwrap_or("127.0.0.1");
        let port = self.daemon.web.port.unwrap_or(9090);
        format!("{addr}:{port}")
    }
}

impl Default for AppConfig {
    fn default() -> Self {
        Self {
            daemon: DaemonConfig::default(),
            processes: HashMap::new(),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DaemonConfig {
    #[serde(default)]
    pub pid_file: Option<PathBuf>,
    #[serde(default)]
    pub log_dir: Option<PathBuf>,
    #[serde(default)]
    pub api_key: Option<String>,
    #[serde(default)]
    pub web: WebConfig,
}

impl Default for DaemonConfig {
    fn default() -> Self {
        Self {
            pid_file: None,
            log_dir: None,
            api_key: None,
            web: WebConfig::default(),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WebConfig {
    pub addr: Option<String>,
    pub port: Option<u16>,
    #[serde(default)]
    pub cors_origins: Vec<String>,
}

impl Default for WebConfig {
    fn default() -> Self {
        Self {
            addr: Some("0.0.0.0".into()),
            port: Some(9090),
            cors_origins: Vec::new(),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ProcessConfig {
    pub command: String,
    #[serde(default)]
    pub args: Vec<String>,
    #[serde(default)]
    pub working_dir: Option<PathBuf>,
    #[serde(default)]
    pub env: HashMap<String, String>,
    #[serde(default = "default_true")]
    pub auto_start: bool,
    #[serde(default = "default_true")]
    pub auto_restart: bool,
    #[serde(default = "default_max_restarts")]
    pub max_restarts: u32,
    #[serde(default = "default_restart_delay")]
    pub restart_delay_secs: u64,
    #[serde(default = "default_stop_timeout")]
    pub stop_timeout_secs: u64,
    #[serde(default)]
    pub health_check: Option<HealthCheckConfig>,
    #[serde(default)]
    pub unhealthy_restart: bool,
    #[serde(default)]
    pub depends_on: Vec<String>,
    #[serde(default)]
    pub max_log_size_mb: Option<u64>,
    #[serde(default)]
    pub stdout_log: Option<PathBuf>,
    #[serde(default)]
    pub stderr_log: Option<PathBuf>,
}

pub fn default_true() -> bool {
    true
}
pub fn default_max_restarts() -> u32 {
    3
}
pub fn default_restart_delay() -> u64 {
    5
}
pub fn default_stop_timeout() -> u64 {
    10
}

impl ProcessConfig {
    pub fn full_command(&self) -> String {
        if self.args.is_empty() {
            self.command.clone()
        } else {
            format!("{} {}", self.command, self.args.join(" "))
        }
    }

    pub fn runtime_fields_eq(&self, other: &Self) -> bool {
        self.command == other.command
            && self.args == other.args
            && self.working_dir == other.working_dir
            && self.env == other.env
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HealthCheckConfig {
    #[serde(flatten)]
    pub check_type: HealthCheckType,
    #[serde(default = "default_health_interval")]
    pub interval_secs: u64,
    #[serde(default = "default_health_timeout")]
    pub timeout_secs: u64,
}

fn default_health_interval() -> u64 {
    30
}
fn default_health_timeout() -> u64 {
    5
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "lowercase")]
pub enum HealthCheckType {
    Tcp { port: u16 },
    Http { url: String },
}
