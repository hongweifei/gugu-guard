//! 守护进程与 Web 服务的配置模型。

use serde::{Deserialize, Serialize};

use crate::config::{DEFAULT_BIND_ADDR, DEFAULT_PORT};

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
    pub pid_file: Option<std::path::PathBuf>,
    #[serde(default)]
    pub log_dir: Option<std::path::PathBuf>,
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
    /// Linux 上 cgroup v2 硬限的落地目录；默认 `/sys/fs/cgroup`。
    /// 容器或 systemd 委派环境里指向自己有写权限的子树。
    #[serde(default)]
    pub cgroup_root: Option<std::path::PathBuf>,
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

/// Web 服务配置：监听地址、端口、跨域来源、文件浏览白名单与内置终端开关。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WebConfig {
    pub addr: Option<String>,
    pub port: Option<u16>,
    #[serde(default)]
    pub cors_origins: Vec<String>,
    /// 文件浏览接口允许的根目录；为空表示不限制。
    /// 相对路径以配置文件所在目录为基准，加载后由调用方解析成绝对路径。
    #[serde(default)]
    pub fs_roots: Vec<std::path::PathBuf>,
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
            fs_roots: Vec::new(),
            terminal: false,
        }
    }
}

impl WebConfig {
    /// 校验文件浏览白名单：每个条目都指向一个具体目录，空条目会把整份配置目录放出去。
    ///
    /// # Errors
    /// 条目为空或只是空白时返回 `GuguError::ConfigError`。
    pub fn validate(&self) -> crate::error::Result<()> {
        for (index, root) in self.fs_roots.iter().enumerate() {
            if root.as_os_str().to_string_lossy().trim().is_empty() {
                return Err(crate::error::GuguError::ConfigError(format!(
                    "daemon.web.fs_roots 第 {} 项为空，写具体目录",
                    index + 1
                )));
            }
        }
        Ok(())
    }
}
