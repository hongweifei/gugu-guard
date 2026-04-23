use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::path::PathBuf;

#[derive(Debug, Default, Clone, Serialize, Deserialize)]
pub struct AppConfig {
    #[serde(default)]
    pub daemon: DaemonConfig,
    #[serde(default)]
    pub processes: HashMap<String, ProcessConfig>,
}

impl AppConfig {
    /// 加载配置文件，文件不存在时返回默认配置。
    ///
    /// # Errors
    /// 文件存在但读取失败或解析失败时返回 `GuguError::ConfigError`。
    pub fn load(path: &std::path::Path) -> crate::error::Result<Self> {
        if !path.exists() {
            return Ok(Self::default());
        }
        let content = std::fs::read_to_string(path)
            .map_err(|e| crate::error::GuguError::ConfigError(format!("读取配置文件失败: {e}")))?;
        let mut config: Self = toml::from_str(&content)
            .map_err(|e| crate::error::GuguError::ConfigError(format!("解析配置文件失败: {e}")))?;
        // 统一路径分隔符，确保 Windows 上 \ 和 / 混用时不影响匹配
        config.normalize_paths();
        Ok(config)
    }

    /// 将配置保存到文件。
    ///
    /// # Errors
    /// 创建目录、序列化或写入失败时返回 `GuguError::ConfigError`。
    pub fn save(&self, path: &std::path::Path) -> crate::error::Result<()> {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)
                .map_err(|e| crate::error::GuguError::ConfigError(format!("创建目录失败: {e}")))?;
        }
        let mut normalized = self.clone();
        normalized.normalize_paths();
        let content = toml::to_string_pretty(&normalized)
            .map_err(|e| crate::error::GuguError::ConfigError(format!("序列化配置失败: {e}")))?;
        std::fs::write(path, content)
            .map_err(|e| crate::error::GuguError::ConfigError(format!("写入配置文件失败: {e}")))?;
        Ok(())
    }

    pub fn normalize_paths(&mut self) {
        for proc in self.processes.values_mut() {
            proc.normalize_paths();
        }
    }

    #[must_use]
    pub fn server_addr(&self) -> String {
        let addr = self.daemon.web.addr.as_deref().unwrap_or("127.0.0.1");
        let port = self.daemon.web.port.unwrap_or(9090);
        format!("{addr}:{port}")
    }
}

#[derive(Debug, Default, Clone, Serialize, Deserialize)]
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
    #[serde(default)]
    pub stop_command: Option<String>,
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
    #[must_use]
    pub fn full_command(&self) -> String {
        if self.args.is_empty() {
            self.command.clone()
        } else {
            format!("{} {}", self.command, self.args.join(" "))
        }
    }

    #[must_use]
    pub fn runtime_fields_eq(&self, other: &Self) -> bool {
        self.command == other.command
            && self.args == other.args
            && self.working_dir == other.working_dir
            && self.env == other.env
            && self.stop_command == other.stop_command
    }

    /// 校验进程配置合法性。
    ///
    /// # Errors
    /// command 为空或健康检查配置不合法时返回 `GuguError::ConfigError`。
    pub fn validate(&self) -> crate::error::Result<()> {
        if self.command.trim().is_empty() {
            return Err(crate::error::GuguError::ConfigError(
                "command 不能为空".into(),
            ));
        }
        if let Some(ref hc) = self.health_check {
            hc.validate()?;
        }
        Ok(())
    }

    pub fn normalize_paths(&mut self) {
        if let Some(ref p) = self.working_dir {
            self.working_dir = Some(PathBuf::from(path_to_forward_slashes(p)));
        }
        if let Some(ref p) = self.stdout_log {
            self.stdout_log = Some(PathBuf::from(path_to_forward_slashes(p)));
        }
        if let Some(ref p) = self.stderr_log {
            self.stderr_log = Some(PathBuf::from(path_to_forward_slashes(p)));
        }
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

impl HealthCheckConfig {
    /// 校验健康检查配置合法性。
    ///
    /// # Errors
    /// 端口为 0、URL 为空、间隔或超时为 0 时返回 `GuguError::ConfigError`。
    pub fn validate(&self) -> crate::error::Result<()> {
        match &self.check_type {
            HealthCheckType::Tcp { host: _, port } => {
                if *port == 0 {
                    return Err(crate::error::GuguError::ConfigError(
                        "健康检查端口不能为 0".into(),
                    ));
                }
            }
            HealthCheckType::Http { url } => {
                if url.trim().is_empty() {
                    return Err(crate::error::GuguError::ConfigError(
                        "HTTP 健康检查 URL 不能为空".into(),
                    ));
                }
            }
        }
        if self.interval_secs == 0 {
            return Err(crate::error::GuguError::ConfigError(
                "健康检查间隔不能为 0 秒".into(),
            ));
        }
        if self.timeout_secs == 0 {
            return Err(crate::error::GuguError::ConfigError(
                "健康检查超时不能为 0 秒".into(),
            ));
        }
        Ok(())
    }
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
    Tcp { host: Option<String>, port: u16 },
    Http { url: String },
}

#[must_use]
pub fn canonicalize_clean(path: &std::path::Path) -> PathBuf {
    std::fs::canonicalize(path)
        .map(|p| strip_unc_prefix(&p))
        .unwrap_or_else(|_| path.to_path_buf())
}

#[must_use]
pub fn strip_unc_prefix(path: &std::path::Path) -> PathBuf {
    let s = path.to_string_lossy();
    match s.strip_prefix(r"\\?\UNC\") {
        Some(rest) => PathBuf::from(format!("\\\\{rest}")),
        None => match s.strip_prefix(r"\\?\") {
            Some(rest) => PathBuf::from(rest),
            None => path.to_path_buf(),
        },
    }
}

#[must_use]
pub fn resolve_relative_path(path: &std::path::Path, base: &std::path::Path) -> PathBuf {
    if path.is_absolute() {
        path.to_path_buf()
    } else {
        base.join(path)
    }
}

#[must_use]
pub fn path_to_forward_slashes(path: &std::path::Path) -> String {
    path.to_string_lossy().replace('\\', "/")
}
