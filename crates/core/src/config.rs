use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::path::PathBuf;
use toml_edit::{DocumentMut, Item, Table};

/// Web/API 默认监听地址（配置缺省时的服务端语义）。
pub const DEFAULT_BIND_ADDR: &str = "0.0.0.0";
/// 本机回环地址（客户端连接与 addr 为监听通配时的替身）。
pub const LOCALHOST: &str = "127.0.0.1";
/// 默认监听端口。
pub const DEFAULT_PORT: u16 = 9090;
/// 默认 PID 文件名（生成在配置文件同目录）。
pub const PID_FILE_NAME: &str = "gugu.pid";

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
        let base: Option<DocumentMut> = std::fs::read_to_string(path).ok()
            .and_then(|s| s.parse::<DocumentMut>().ok());
        let doc: DocumentMut = match base {
            Some(mut base_doc) => {
                let fresh_doc = fresh.parse::<DocumentMut>()
                    .map_err(|e| crate::error::GuguError::ConfigError(format!("解析配置失败: {e}")))?;
                merge_tables(base_doc.as_table_mut(), fresh_doc.as_table());
                base_doc
            }
            None => fresh.parse::<DocumentMut>()
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
            addr: Some(DEFAULT_BIND_ADDR.into()),
            port: Some(DEFAULT_PORT),
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
pub fn default_restart_reset_secs() -> u64 {
    60
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
            && self.shell == other.shell
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
    let exe_dir = std::env::current_exe().ok()
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
    let stale: Vec<String> = dst.iter()
        .filter(|(k, _)| src.get(k).is_none())
        .map(|(k, _)| k.to_string())
        .collect();
    for k in &stale {
        dst.remove(k);
    }
    for (k, sv) in src.iter() {
        if sv.is_table() && dst.get(k).is_some_and(Item::is_table) {
            if let (Some(dt), Some(st)) = (dst.get_mut(k).and_then(|i| i.as_table_mut()), sv.as_table()) {
                merge_tables(dt, st);
            }
            continue;
        }
        let old_key_decor = dst.key(k).map(|key| key.leaf_decor().clone());
        let old_val_decor = dst.get(k).and_then(|i| i.as_value())
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
