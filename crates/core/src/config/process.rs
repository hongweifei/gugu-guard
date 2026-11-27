//! 进程配置：运行参数、健康检查、日志告警与资源硬限。

use std::collections::HashMap;
use std::path::PathBuf;

use serde::{Deserialize, Serialize};

use crate::config::paths::path_to_forward_slashes;

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
    /// 连续崩溃重启计数在进程稳定运行多少秒后归零 (缺陷4：计数不衰减)。
    #[serde(default = "default_restart_reset_secs")]
    pub restart_reset_secs: u64,
    /// true 时 command/args 拼接后经 shell (cmd /C 或 sh -c) 执行；
    /// 默认 false，直接启动可执行文件，参数不被 shell 拆解或注入。
    #[serde(default)]
    pub shell: bool,
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
    pub group: Option<String>,
    /// 进程树内存上限 (MB)：超过则按自动重启路径重启（防泄漏）。
    #[serde(default)]
    pub max_memory_mb: Option<u64>,
    /// 资源硬限：CPU 带宽，`50%` / `0.5`（核数）/ `50000 100000`（quota period）。
    /// Linux 写进 cgroup 配额，Windows 按整机占比折算成节流上限。
    #[serde(default)]
    pub cpu_max: Option<String>,
    /// 资源硬限：内存，`512M` / `2G` / 字节数。
    /// Linux 越界由内核整组 OOM，Windows 限制 Job 的提交内存。
    #[serde(default)]
    pub memory_max: Option<String>,
    /// 资源硬限：进程/任务数（防 fork 炸弹），如 `128`。Windows 只统计进程。
    #[serde(default)]
    pub pids_max: Option<String>,
    #[serde(default)]
    pub max_log_size_mb: Option<u64>,
    #[serde(default)]
    pub stdout_log: Option<PathBuf>,
    #[serde(default)]
    pub stderr_log: Option<PathBuf>,
    /// 日志行的结构化格式；默认 auto：逐行按 JSON → logfmt → 文本 逐档回退。
    #[serde(default)]
    pub log_format: crate::logs::LogFormat,
    /// 日志文本编码；默认 auto：UTF-8 严格解码失败时按 GBK 解码。
    /// 只影响面板、广播与检索，落盘文件仍是程序输出的原始字节。
    #[serde(default)]
    pub encoding: crate::logs::LogEncoding,
    /// 服务监听的端口：启动前做占用预检，运行期统计连接数，也作为就绪判定依据。
    #[serde(default)]
    pub listen_ports: Vec<u16>,
    /// 启动前检查 [`Self::listen_ports`] 是否已被别的进程占用。
    #[serde(default = "default_true")]
    pub port_check: bool,
    /// 等待被依赖进程就绪的超时（秒）；超时则本进程不启动。
    #[serde(default = "default_ready_timeout")]
    pub ready_timeout_secs: u64,
    /// 日志关键字与刷屏告警；未配置则不做日志分析。
    #[serde(default)]
    pub log_alert: Option<LogAlertConfig>,
}

/// 日志告警：关键字命中推一条通知，单位时间行数暴涨再推一条。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LogAlertConfig {
    /// 关键字（按子串匹配，ASCII 忽略大小写）。
    #[serde(default)]
    pub patterns: Vec<String>,
    /// 刷屏检测的统计窗口（秒）。
    #[serde(default = "default_alert_window")]
    pub window_secs: u64,
    /// 一个窗口内行数达到该值判定为刷屏；0 表示关闭刷屏检测。
    #[serde(default)]
    pub burst_lines: u32,
    /// 同类告警的冷却时间（秒），避免持续异常刷屏通知。
    #[serde(default = "default_alert_cooldown")]
    pub cooldown_secs: u64,
}

impl LogAlertConfig {
    /// # Errors
    /// 窗口为 0、关键字全为空且未启用刷屏检测时返回 `GuguError::ConfigError`。
    pub fn validate(&self) -> crate::error::Result<()> {
        if self.window_secs == 0 {
            return Err(crate::error::GuguError::ConfigError(
                "log_alert.window_secs 不能为 0".into(),
            ));
        }
        let has_pattern = self.patterns.iter().any(|p| !p.trim().is_empty());
        if !has_pattern && self.burst_lines == 0 {
            return Err(crate::error::GuguError::ConfigError(
                "log_alert 至少要配置 patterns 或 burst_lines".into(),
            ));
        }
        Ok(())
    }
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
pub fn default_restart_reset_secs() -> u64 {
    60
}
pub fn default_stop_timeout() -> u64 {
    10
}
pub fn default_ready_timeout() -> u64 {
    60
}
pub fn default_alert_window() -> u64 {
    10
}
pub fn default_alert_cooldown() -> u64 {
    300
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
            && self.shell == other.shell
            // 硬限只在启动时生效：改了限值就要重启，否则用户会以为已经生效
            && self.cpu_max == other.cpu_max
            && self.memory_max == other.memory_max
            && self.pids_max == other.pids_max
    }

    /// 解析出的 cgroup 硬限（三个字段都为空时返回不启用的空规格）。
    ///
    /// # Errors
    /// 任一限值写法无法解析时返回 `GuguError::ConfigError`。
    pub fn cgroup_spec(&self) -> crate::error::Result<crate::cgroup::CgroupSpec> {
        crate::cgroup::CgroupSpec::parse(
            self.cpu_max.as_deref(),
            self.memory_max.as_deref(),
            self.pids_max.as_deref(),
        )
        .map_err(crate::error::GuguError::ConfigError)
    }

    /// 校验进程配置合法性。
    ///
    /// # Errors
    /// command 为空、端口为 0、健康检查或日志告警配置不合法时返回 `GuguError::ConfigError`。
    pub fn validate(&self) -> crate::error::Result<()> {
        if self.command.trim().is_empty() {
            return Err(crate::error::GuguError::ConfigError(
                "command 不能为空".into(),
            ));
        }
        if self.listen_ports.contains(&0) {
            return Err(crate::error::GuguError::ConfigError(
                "listen_ports 不能包含 0".into(),
            ));
        }
        if let Some(ref hc) = self.health_check {
            hc.validate()?;
        }
        if let Some(ref la) = self.log_alert {
            la.validate()?;
        }
        // 限值写错在保存配置时就拦住，而不是等启动才发现
        self.cgroup_spec()?;
        if self.ready_timeout_secs == 0 {
            return Err(crate::error::GuguError::ConfigError(
                "ready_timeout_secs 不能为 0".into(),
            ));
        }
        Ok(())
    }

    /// 需要关注的本机端口：显式声明的监听端口 + 健康检查/就绪检查用的端口。
    ///
    /// 端口预检、连接数统计、依赖就绪判定都基于这一份端口集合。
    #[must_use]
    pub fn watched_ports(&self) -> Vec<u16> {
        let mut ports = self.listen_ports.clone();
        if let Some(ref hc) = self.health_check {
            let port = match &hc.check_type {
                HealthCheckType::Tcp { port, .. } => Some(*port),
                HealthCheckType::Http { url } => port_from_url(url),
            };
            if let Some(p) = port {
                ports.push(p);
            }
        }
        ports.retain(|p| *p != 0);
        ports.sort_unstable();
        ports.dedup();
        ports
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
    /// 连续失败达到该次数才判定为不健康 (缺陷5：单次抖动即重启)。
    #[serde(default = "default_fail_threshold")]
    pub fail_threshold: u32,
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
        if self.fail_threshold == 0 {
            return Err(crate::error::GuguError::ConfigError(
                "健康检查失败判定阈值不能为 0".into(),
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
pub fn default_fail_threshold() -> u32 {
    3
}

/// 从 URL 里取出端口；未写端口时按 scheme 取默认值（http 80 / https 443）。
///
/// 只服务于「哪些端口归这个服务」这一个用途，因此不引入完整 URL 解析库。
#[must_use]
pub fn port_from_url(url: &str) -> Option<u16> {
    let (scheme, _) = url.split_once("://")?;
    if let Some(port) = crate::net::split_authority(url).and_then(|(_, port)| port) {
        return Some(port);
    }
    match scheme.to_ascii_lowercase().as_str() {
        "https" => Some(443),
        "http" => Some(80),
        _ => None,
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "lowercase")]
pub enum HealthCheckType {
    Tcp { host: Option<String>, port: u16 },
    Http { url: String },
}
