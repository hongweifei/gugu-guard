use serde::{Deserialize, Serialize};
use std::collections::{HashMap, HashSet};
use std::path::PathBuf;
use toml_edit::{DocumentMut, Item, Table};

use crate::net::{host_is_loopback, url_host};

/// Web/API 默认监听地址（配置缺省时的服务端语义）。
pub const DEFAULT_BIND_ADDR: &str = "0.0.0.0";
/// 本机回环地址（客户端连接与 addr 为监听通配时的替身）。
pub const LOCALHOST: &str = "127.0.0.1";
/// 默认监听端口。
pub const DEFAULT_PORT: u16 = 9090;
/// 默认 PID 文件名（生成在配置文件同目录）。
pub const PID_FILE_NAME: &str = "gugu.pid";
/// 本机在节点表里的固定标识；从节点不能占用这个名字。
pub const LOCAL_NODE_NAME: &str = "local";

#[derive(Debug, Default, Clone, Serialize, Deserialize)]
pub struct AppConfig {
    #[serde(default)]
    pub daemon: DaemonConfig,
    #[serde(default)]
    pub processes: HashMap<String, ProcessConfig>,
    /// 中心节点纳管的从节点；空表示只有本机。
    #[serde(default)]
    pub nodes: Vec<NodeConfig>,
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
    /// 若目标文件已存在且可解析，则以原文件为底稿按字段覆盖值，
    /// 保留注释与未变动条目（绿色部署：手写说明不因一次 `add` 而丢失）。
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
        let fresh = toml::to_string_pretty(&normalized)
            .map_err(|e| crate::error::GuguError::ConfigError(format!("序列化配置失败: {e}")))?;
        let base: Option<DocumentMut> = std::fs::read_to_string(path)
            .ok()
            .and_then(|s| s.parse::<DocumentMut>().ok());
        let doc: DocumentMut = match base {
            Some(mut base_doc) => {
                let fresh_doc = fresh.parse::<DocumentMut>().map_err(|e| {
                    crate::error::GuguError::ConfigError(format!("解析配置失败: {e}"))
                })?;
                merge_tables(base_doc.as_table_mut(), fresh_doc.as_table());
                base_doc
            }
            None => fresh
                .parse::<DocumentMut>()
                .map_err(|e| crate::error::GuguError::ConfigError(format!("解析配置失败: {e}")))?,
        };
        std::fs::write(path, doc.to_string())
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
        let addr = self.daemon.web.addr.as_deref().unwrap_or(LOCALHOST);
        let port = self.daemon.web.port.unwrap_or(DEFAULT_PORT);
        format!("{addr}:{port}")
    }

    /// 节点表的问题列表（空表示通过）。
    #[must_use]
    pub fn node_problems(&self) -> Vec<String> {
        validate_nodes(&self.nodes)
    }

    /// 本机在节点表里的显示名：`daemon.node_name` 去空白后的值；
    /// `None` 表示没有配置，由节点注册表退回主机名。
    #[must_use]
    pub fn local_node_name(&self) -> Option<String> {
        self.daemon
            .node_name
            .as_deref()
            .map(str::trim)
            .filter(|name| !name.is_empty())
            .map(str::to_string)
    }
}

/// webhook 载荷格式。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "lowercase")]
pub enum WebhookFormat {
    /// 钉钉 / 企业微信自定义机器人：`{"msgtype":"text","text":{"content": …}}`
    #[default]
    Dingtalk,
    /// 飞书自定义机器人：`{"msg_type":"text","content":{"text": …}}`
    Feishu,
    /// 原样发送事件 JSON（自建接收端用）
    Raw,
}

#[derive(Debug, Default, Clone, Serialize, Deserialize)]
pub struct DaemonConfig {
    #[serde(default)]
    pub pid_file: Option<PathBuf>,
    #[serde(default)]
    pub log_dir: Option<PathBuf>,
    /// 本机在节点列表里的显示名；缺省用主机名。只影响展示，节点标识始终是 `local`。
    #[serde(default)]
    pub node_name: Option<String>,
    #[serde(default)]
    pub api_key: Option<String>,
    /// 崩溃 / 不健康 / 恢复 通知的 webhook 地址（钉钉、企微、飞书自定义机器人或自建端）。
    #[serde(default)]
    pub webhook_url: Option<String>,
    /// webhook 载荷格式，默认 dingtalk（钉钉与企微同形）。
    #[serde(default)]
    pub webhook_format: WebhookFormat,
    /// cgroup v2 硬限的落地目录；默认 `/sys/fs/cgroup`。
    /// 容器或 systemd 委派环境里指向自己有写权限的子树。
    #[serde(default)]
    pub cgroup_root: Option<PathBuf>,
    #[serde(default)]
    pub web: WebConfig,
}

impl DaemonConfig {
    /// 需要发通知时返回 `(url, 载荷格式)`；未配置或空白视为关闭。
    #[must_use]
    pub fn notify_target(&self) -> Option<(&str, WebhookFormat)> {
        let url = self.webhook_url.as_deref()?;
        if url.trim().is_empty() {
            return None;
        }
        Some((url, self.webhook_format))
    }
}

/// Web 服务配置：监听地址、端口、跨域来源与内置终端开关。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WebConfig {
    pub addr: Option<String>,
    pub port: Option<u16>,
    #[serde(default)]
    pub cors_origins: Vec<String>,
    /// 是否开放 WebUI 内置终端。默认关闭；开启后等于把 shell 交给能访问面板的人。
    /// 监听非回环地址时必须配置 `api_key`，否则守护进程拒绝启动。
    #[serde(default)]
    pub terminal: bool,
}

impl Default for WebConfig {
    fn default() -> Self {
        Self {
            addr: Some(DEFAULT_BIND_ADDR.into()),
            port: Some(DEFAULT_PORT),
            cors_origins: Vec::new(),
            terminal: false,
        }
    }
}

/// 中心节点纳管的一台从节点。本机不写在这里，始终以 [`LOCAL_NODE_NAME`] 存在。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct NodeConfig {
    /// 节点标识：用作 `/api/v1/n/{name}/...` 的路径段与 `--node` 参数。
    pub name: String,
    /// 界面上显示的名字，缺省用 `name`。
    #[serde(default)]
    pub label: Option<String>,
    /// 从节点的服务地址，如 `http://10.0.0.12:9090`。
    pub url: String,
    /// 访问从节点的 API Key；从节点未开认证时留空。
    #[serde(default)]
    pub api_key: Option<String>,
}

impl NodeConfig {
    /// 去掉末尾斜杠的地址，拼接 `/api/v1/...` 时不会出现双斜杠。
    #[must_use]
    pub fn base_url(&self) -> String {
        self.url.trim().trim_end_matches('/').to_string()
    }

    /// 界面显示名：`label` 优先，缺省回到 `name`。
    #[must_use]
    pub fn display_name(&self) -> &str {
        self.label
            .as_deref()
            .map(str::trim)
            .filter(|label| !label.is_empty())
            .unwrap_or(&self.name)
    }

    /// 校验单个节点：名称能安全放进 URL 路径，地址是 http(s)，对外地址必须带 Key。
    ///
    /// # Errors
    /// 名称、地址或 Key 不满足要求时返回 `GuguError::ConfigError`。
    pub fn validate(&self) -> crate::error::Result<()> {
        let name = self.name.trim();
        if name.is_empty() {
            return Err(crate::error::GuguError::ConfigError(
                "节点名称不能为空".into(),
            ));
        }
        if !name
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '_' | '-'))
        {
            return Err(crate::error::GuguError::ConfigError(format!(
                "节点名称 '{name}' 只能用字母、数字、'.'、'_'、'-'"
            )));
        }
        if name.eq_ignore_ascii_case(LOCAL_NODE_NAME) {
            return Err(crate::error::GuguError::ConfigError(format!(
                "节点名称 '{name}' 保留给本机"
            )));
        }
        if name == "." || name == ".." {
            return Err(crate::error::GuguError::ConfigError(format!(
                "节点名称 '{name}' 会被 URL 当成目录段，换一个名字"
            )));
        }
        let base = self.base_url();
        if !(base.starts_with("http://") || base.starts_with("https://")) {
            return Err(crate::error::GuguError::ConfigError(format!(
                "节点 '{name}' 的地址必须以 http:// 或 https:// 开头"
            )));
        }
        let Some(host) = url_host(&base) else {
            return Err(crate::error::GuguError::ConfigError(format!(
                "节点 '{name}' 的地址缺少主机名"
            )));
        };
        let has_key = self
            .api_key
            .as_deref()
            .is_some_and(|key| !key.trim().is_empty());
        if !host_is_loopback(&host) && !has_key {
            return Err(crate::error::GuguError::ConfigError(format!(
                "节点 '{name}' 指向非回环地址 {host}，必须配置 api_key：代理会用这把 Key 操作从节点"
            )));
        }
        Ok(())
    }
}

/// 校验节点表，返回人类可读的问题列表（空表示通过）。
#[must_use]
pub fn validate_nodes(nodes: &[NodeConfig]) -> Vec<String> {
    let mut problems: Vec<String> = nodes
        .iter()
        .filter_map(|node| node.validate().err())
        .map(|e| e.to_string())
        .collect();
    problems.extend(
        duplicate_node_names(nodes)
            .into_iter()
            .map(|name| format!("节点名称重复: {name}")),
    );
    problems
}

/// 重复的节点名称（忽略大小写，返回首次出现时的写法）。
#[must_use]
pub fn duplicate_node_names(nodes: &[NodeConfig]) -> Vec<String> {
    let mut seen = HashSet::with_capacity(nodes.len());
    let mut duplicates: Vec<String> = Vec::new();
    for node in nodes {
        let name = node.name.trim();
        if !seen.insert(name.to_ascii_lowercase())
            && !duplicates.iter().any(|d| d.eq_ignore_ascii_case(name))
        {
            duplicates.push(name.to_string());
        }
    }
    duplicates
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
    /// cgroup v2 硬限：CPU 带宽，`50%` / `0.5`（核数）/ `50000 100000`（quota period）。
    #[serde(default)]
    pub cpu_max: Option<String>,
    /// cgroup v2 硬限：内存，`512M` / `2G` / 字节数；越界由内核整组 OOM。
    #[serde(default)]
    pub memory_max: Option<String>,
    /// cgroup v2 硬限：任务数（防 fork 炸弹），如 `128`。
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
    let (scheme, rest) = url.split_once("://")?;
    let authority = rest.split(['/', '?', '#']).next()?;
    if let Some((_, port)) = authority.rsplit_once(':') {
        if let Ok(port) = port.parse::<u16>() {
            return Some(port);
        }
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

/// 默认配置文件名。
pub const CONFIG_FILE_NAME: &str = "gugu.toml";

/// 按绿色软件的查找顺序选择默认配置文件位置（纯函数，便于测试）：
/// 1. 程序同目录下的 gugu.toml —— 配置跟着程序走；
/// 2. 当前工作目录下的 gugu.toml —— 兼容在项目目录里直接运行；
/// 3. 两者都不存在时，返回程序同目录路径作为“应当放置/生成配置”的位置；
///    若拿不到程序目录，退回当前工作目录。
#[must_use]
pub fn select_config_path(
    exe_dir: Option<&std::path::Path>,
    cwd: &std::path::Path,
    file_exists: &dyn Fn(&std::path::Path) -> bool,
) -> PathBuf {
    if let Some(dir) = exe_dir {
        let candidate = dir.join(CONFIG_FILE_NAME);
        if file_exists(&candidate) {
            return candidate;
        }
    }
    let cwd_candidate = cwd.join(CONFIG_FILE_NAME);
    if file_exists(&cwd_candidate) {
        return cwd_candidate;
    }
    exe_dir.unwrap_or(cwd).join(CONFIG_FILE_NAME)
}

/// 解析默认配置文件路径（绝对路径）。
#[must_use]
pub fn default_config_path() -> PathBuf {
    let exe_dir = std::env::current_exe()
        .ok()
        .and_then(|p| p.parent().map(std::path::Path::to_path_buf));
    let cwd = std::env::current_dir().unwrap_or_else(|_| PathBuf::from("."));
    select_config_path(exe_dir.as_deref(), &cwd, &|p| p.is_file())
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

/// 若 `path` 位于 `base` 目录下，返回其相对路径 (正斜杠形式)；否则原样返回。
/// 用于把内存中已解析的绝对路径回写成配置里的相对路径 (绿色部署可搬移)。
#[must_use]
pub fn relativize_path(path: &std::path::Path, base: &std::path::Path) -> PathBuf {
    let ps = path_to_forward_slashes(path);
    let bs = path_to_forward_slashes(base);
    let bs = bs.trim_end_matches('/');
    if bs.is_empty() {
        return path.to_path_buf();
    }
    let prefix_len = bs.len() + 1; // base + '/'
    if ps.len() > prefix_len
        && ps.as_bytes()[prefix_len - 1] == b'/'
        && ps[..bs.len()].eq_ignore_ascii_case(bs)
    {
        PathBuf::from(&ps[prefix_len..])
    } else {
        path.to_path_buf()
    }
}

/// 把 `src` (新配置序列化结果) 的值覆盖进 `dst` (原文件底稿)：
/// - 双方都是表：递归合并
/// - 已存在的标量键：替换值，保留其行前/行尾注释与空白 (decor)
/// - 其余情况 (新增/类型变化)：整体替换
/// - `dst` 中 `src` 不再包含的键：移除 (配置模型是唯一事实，删除的进程/字段不残留)
fn merge_tables(dst: &mut Table, src: &Table) {
    let stale: Vec<String> = dst
        .iter()
        .filter(|(k, _)| src.get(k).is_none())
        .map(|(k, _)| k.to_string())
        .collect();
    for k in &stale {
        dst.remove(k);
    }
    for (k, sv) in src.iter() {
        if sv.is_table() && dst.get(k).is_some_and(Item::is_table) {
            if let (Some(dt), Some(st)) =
                (dst.get_mut(k).and_then(|i| i.as_table_mut()), sv.as_table())
            {
                merge_tables(dt, st);
            }
            continue;
        }
        let old_key_decor = dst.key(k).map(|key| key.leaf_decor().clone());
        let old_val_decor = dst
            .get(k)
            .and_then(|i| i.as_value())
            .map(|v| (v.decor().prefix().cloned(), v.decor().suffix().cloned()));
        let mut new_item = sv.clone();
        if let (Some((p, s)), Some(nv)) = (old_val_decor, new_item.as_value_mut()) {
            let mut decor = toml_edit::Decor::default();
            if let Some(p) = p {
                decor.set_prefix(p);
            }
            if let Some(s) = s {
                decor.set_suffix(s);
            }
            *nv.decor_mut() = decor;
        }
        dst.insert(k, new_item);
        if let Some(kd) = old_key_decor {
            if let Some(mut key) = dst.key_mut(k) {
                *key.leaf_decor_mut() = kd;
            }
        }
    }
}
