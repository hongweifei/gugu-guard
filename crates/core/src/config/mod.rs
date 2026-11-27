//! 配置模型与读写。
//!
//! 子模块分工（对外路径由本文件 re-export 保持稳定）：
//! - [`daemon`]：守护进程与 Web 服务配置
//! - [`node`]：从节点配置与节点表校验
//! - [`process`]：进程配置、健康检查、日志告警与资源硬限
//! - [`paths`]：配置文件查找与路径规范化
//! - [`unknown`]：未知配置键的检出与拼写建议
//! - [`write`]：写回时与原文件底稿的合并

use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use toml_edit::DocumentMut;

mod daemon;
mod node;
mod paths;
mod process;
mod unknown;
mod write;

pub use daemon::{DaemonConfig, WebConfig, WebhookFormat};
pub use node::{duplicate_node_names, validate_nodes, NodeConfig};
pub use paths::{
    canonicalize_clean, default_config_path, path_to_forward_slashes, relativize_path,
    resolve_relative_path, select_config_path, strip_unc_prefix, CONFIG_FILE_NAME,
};
pub use process::{
    default_alert_cooldown, default_alert_window, default_fail_threshold, default_max_restarts,
    default_ready_timeout, default_restart_delay, default_restart_reset_secs, default_stop_timeout,
    default_true, port_from_url, HealthCheckConfig, HealthCheckType, LogAlertConfig, ProcessConfig,
};

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
    /// 配置文件里存在、但结构体不认识的键，路径形如 `processes.web.memory_maxx`。
    /// 由 [`AppConfig::load`] 填充，供启动告警与 `gugu check` 提示；不参与序列化。
    #[serde(skip)]
    pub unknown_keys: Vec<String>,
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
        config.unknown_keys = unknown::collect(&content, &config);
        // 统一路径分隔符，确保 Windows 上 \ 和 / 混用时不影响匹配
        config.normalize_paths();
        Ok(config)
    }

    /// 把加载时收集到的未知键逐条记进日志。
    ///
    /// 必须等日志初始化之后再调用：`load` 发生在解析 `log_dir` 之前，那时打日志会被丢弃。
    /// 未知键不阻止启动，但拼写或层级写错通常意味着某项设置没有生效。
    pub fn warn_unknown_keys(&self) {
        for key in &self.unknown_keys {
            tracing::warn!("配置里有不认识的键 {key}：该项被忽略，请检查拼写与层级");
        }
    }

    /// 文件浏览没有白名单、监听的又不是回环地址时，把暴露面说清楚。
    ///
    /// 与未知键一样必须在日志初始化之后调用，否则这条提示会被丢弃。
    pub fn warn_open_browse(&self) {
        let web = &self.daemon.web;
        if !web.fs_roots.is_empty() {
            return;
        }
        let addr = web.addr.as_deref().unwrap_or(DEFAULT_BIND_ADDR);
        if crate::net::host_is_loopback(addr) {
            return;
        }
        tracing::warn!(
            "文件浏览未设白名单（daemon.web.fs_roots），能访问面板的人可以浏览任意目录；对外提供服务时请限定根目录"
        );
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
                write::merge_tables(base_doc.as_table_mut(), fresh_doc.as_table());
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
