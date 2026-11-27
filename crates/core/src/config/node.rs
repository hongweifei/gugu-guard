//! 从节点配置与节点表校验。

use std::collections::HashSet;

use serde::{Deserialize, Serialize};

use crate::config::LOCAL_NODE_NAME;
use crate::net::{host_is_loopback, url_host};

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
