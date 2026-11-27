//! 节点注册表：本机身份、从节点列表与最近一次探测结果。
//!
//! 探测（5 秒一轮）与代理各用一个 HTTP 客户端：探测要求快，超时短；
//! 代理要能连稍慢的机器、下载大日志，只约束建连与空闲读取。

use std::collections::HashMap;
use std::sync::{Arc, RwLock};
use std::time::{Duration, Instant};

use chrono::{DateTime, Utc};
use gugu_core::config::{NodeConfig, LOCAL_NODE_NAME};
use gugu_core::manager::SharedManager;
use gugu_core::net::encode_path_segment;
use gugu_core::process::ProcessStatus;
use serde::{Deserialize, Serialize};

/// 从节点健康探测间隔。
const POLL_INTERVAL: Duration = Duration::from_secs(5);
/// 单次探测超时：从节点掉线时不能拖住下一轮。
const POLL_TIMEOUT: Duration = Duration::from_secs(3);
/// 代理建连超时：给慢机器留余量。
const PROXY_CONNECT_TIMEOUT: Duration = Duration::from_secs(10);
/// 代理空闲读取超时：大文件下载只要还在出数据就不会被打断。
const PROXY_READ_TIMEOUT: Duration = Duration::from_secs(30);

/// 节点自身信息，`GET /api/v1/node` 的响应，也是中心节点探测从节点用的口径。
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct NodeInfo {
    pub name: String,
    pub hostname: String,
    pub platform: String,
    pub version: String,
    pub uptime_secs: u64,
    pub processes: usize,
    pub running: usize,
    pub terminal: bool,
    /// 该节点自己纳管的从节点数。
    pub nodes: usize,
}

/// 节点表里的一行：本机与从节点同形，前端只按 `local` 区分。
#[derive(Debug, Clone, Serialize)]
pub struct NodeView {
    /// 节点标识：本机固定 `local`，其余为配置里的 `name`。
    pub id: String,
    /// 界面显示名。
    pub name: String,
    /// 从节点地址；本机为 null。
    pub url: Option<String>,
    pub local: bool,
    pub online: bool,
    pub latency_ms: Option<u64>,
    pub version: Option<String>,
    pub hostname: Option<String>,
    pub platform: Option<String>,
    pub processes: usize,
    pub running: usize,
    pub last_error: Option<String>,
    pub checked_at: Option<DateTime<Utc>>,
    /// 是否开放内置终端（只对本机有意义）。
    pub terminal: bool,
}

/// 最近一次探测结果；`info` 有值即代表探测成功。
#[derive(Debug, Clone, Default)]
struct NodeStatus {
    online: bool,
    latency_ms: Option<u64>,
    checked_at: Option<DateTime<Utc>>,
    last_error: Option<String>,
    info: Option<NodeInfo>,
}

/// 一台从节点：持有一份规范化后的配置，地址去掉末尾斜杠后单独缓存。
/// Key 只在这里与探测/代理请求里出现。
#[derive(Debug, Clone)]
pub(crate) struct UpstreamNode {
    config: NodeConfig,
    base: String,
}

impl UpstreamNode {
    fn new(node: &NodeConfig) -> Self {
        let mut config = node.clone();
        config.name = config.name.trim().to_string();
        config.url = config.url.trim().to_string();
        config.label = config
            .label
            .map(|label| label.trim().to_string())
            .filter(|label| !label.is_empty());
        config.api_key = config
            .api_key
            .map(|key| key.trim().to_string())
            .filter(|key| !key.is_empty());
        Self {
            base: config.base_url(),
            config,
        }
    }

    pub(crate) fn name(&self) -> &str {
        &self.config.name
    }

    pub(crate) fn display_name(&self) -> &str {
        self.config.display_name()
    }

    pub(crate) fn base(&self) -> &str {
        &self.base
    }

    pub(crate) fn api_key(&self) -> Option<&str> {
        self.config.api_key.as_deref()
    }

    /// 连从节点 WebSocket 的地址：scheme 换成 ws(s)，Key 走 query，与浏览器同一条认证路径。
    pub(crate) fn ws_url(&self) -> String {
        let base = if let Some(rest) = self.base.strip_prefix("https://") {
            format!("wss://{rest}")
        } else if let Some(rest) = self.base.strip_prefix("http://") {
            format!("ws://{rest}")
        } else {
            self.base.clone()
        };
        match self.api_key() {
            Some(key) => format!("{base}/api/v1/ws?token={}", encode_path_segment(key)),
            None => format!("{base}/api/v1/ws"),
        }
    }
}

/// 节点注册表：本机身份 + 从节点列表 + 最近一次探测结果。
pub struct NodeRegistry {
    local_name: String,
    hostname: String,
    started: Instant,
    remotes: Vec<UpstreamNode>,
    probe: reqwest::Client,
    http: reqwest::Client,
    status: RwLock<HashMap<String, NodeStatus>>,
}

impl NodeRegistry {
    /// `local_name` 由调用方规范化（去空白、空串变 None），这里只补主机名兜底。
    #[must_use]
    pub fn new(nodes: &[NodeConfig], local_name: Option<String>) -> Self {
        let hostname = hostname();
        let local_name = local_name.unwrap_or_else(|| hostname.clone());
        let remotes: Vec<UpstreamNode> = nodes.iter().map(UpstreamNode::new).collect();
        let user_agent = concat!("gugu-hub/", env!("CARGO_PKG_VERSION"));
        let probe = reqwest::Client::builder()
            .user_agent(user_agent)
            .connect_timeout(POLL_TIMEOUT)
            .timeout(POLL_TIMEOUT)
            .build()
            .unwrap_or_default();
        let http = reqwest::Client::builder()
            .user_agent(user_agent)
            .connect_timeout(PROXY_CONNECT_TIMEOUT)
            .read_timeout(PROXY_READ_TIMEOUT)
            .build()
            .unwrap_or_default();
        Self {
            local_name,
            hostname,
            started: Instant::now(),
            remotes,
            probe,
            http,
            status: RwLock::new(HashMap::new()),
        }
    }

    /// 单机形态：没有任何从节点。
    #[must_use]
    pub fn local_only() -> Self {
        Self::new(&[], None)
    }

    #[must_use]
    pub fn local_name(&self) -> &str {
        &self.local_name
    }

    /// 按标识找从节点；命中即返回可移交的副本，代理与中继共用。
    #[must_use]
    pub(crate) fn remote(&self, name: &str) -> Option<UpstreamNode> {
        self.remotes
            .iter()
            .find(|node| node.name() == name)
            .cloned()
    }

    /// 代理请求用的 HTTP 客户端：复用一个连接池，超时口径见文件头。
    pub(crate) fn http_client(&self) -> &reqwest::Client {
        &self.http
    }

    /// 本机信息，供 `GET /api/v1/node` 与探测共用。
    #[must_use]
    pub fn self_info(&self, manager: &SharedManager, terminal: bool) -> NodeInfo {
        let view = self.local_view(manager, terminal);
        NodeInfo {
            name: self.local_name.clone(),
            hostname: self.hostname.clone(),
            platform: std::env::consts::OS.to_string(),
            version: env!("CARGO_PKG_VERSION").to_string(),
            uptime_secs: self.started.elapsed().as_secs(),
            processes: view.processes,
            running: view.running,
            terminal,
            nodes: self.remotes.len(),
        }
    }

    /// 节点表：本机在前，从节点按配置顺序。
    #[must_use]
    pub fn views(&self, manager: &SharedManager, terminal: bool) -> Vec<NodeView> {
        let mut views = Vec::with_capacity(self.remotes.len() + 1);
        views.push(self.local_view(manager, terminal));
        let status = self
            .status
            .read()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        views.extend(self.remotes.iter().map(|remote| {
            let current = status.get(remote.name()).cloned().unwrap_or_default();
            NodeView::remote(remote, &current)
        }));
        views
    }

    fn local_view(&self, manager: &SharedManager, terminal: bool) -> NodeView {
        let processes = manager.list_processes();
        let running = processes
            .iter()
            .filter(|p| matches!(p.status, ProcessStatus::Running))
            .count();
        NodeView {
            id: LOCAL_NODE_NAME.to_string(),
            name: self.local_name.clone(),
            url: None,
            local: true,
            online: true,
            latency_ms: None,
            version: Some(env!("CARGO_PKG_VERSION").to_string()),
            hostname: Some(self.hostname.clone()),
            platform: Some(std::env::consts::OS.to_string()),
            processes: processes.len(),
            running,
            last_error: None,
            checked_at: None,
            terminal,
        }
    }

    /// 起一个后台任务，每 5 秒探测一轮从节点；没有从节点时立即结束。
    pub fn spawn_poller(self: &Arc<Self>) -> tokio::task::JoinHandle<()> {
        let registry = Arc::clone(self);
        tokio::spawn(async move {
            if registry.remotes.is_empty() {
                return;
            }
            let mut ticker = tokio::time::interval(POLL_INTERVAL);
            ticker.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
            loop {
                ticker.tick().await;
                registry.refresh().await;
            }
        })
    }

    /// 立刻探测一轮所有从节点（启动后首次填充，测试也走这里）。
    pub async fn refresh(&self) {
        if self.remotes.is_empty() {
            return;
        }
        let results =
            futures::future::join_all(self.remotes.iter().map(|node| self.probe(node))).await;
        let mut status = self
            .status
            .write()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        for (name, current) in results {
            status.insert(name, current);
        }
    }

    async fn probe(&self, node: &UpstreamNode) -> (String, NodeStatus) {
        let started = Instant::now();
        let mut request = self.probe.get(format!("{}/api/v1/node", node.base()));
        if let Some(key) = node.api_key() {
            request = request.bearer_auth(key);
        }
        let status = match request.send().await {
            Ok(response) if response.status().is_success() => {
                let latency_ms = started.elapsed().as_millis() as u64;
                match response.json::<NodeInfo>().await {
                    Ok(info) => NodeStatus {
                        online: true,
                        latency_ms: Some(latency_ms),
                        checked_at: Some(Utc::now()),
                        last_error: None,
                        info: Some(info),
                    },
                    Err(e) => offline(format!("返回内容无法解析: {e}")),
                }
            }
            Ok(response) => offline(format!("HTTP {}", response.status())),
            Err(e) => offline(if e.is_timeout() {
                "连接超时".to_string()
            } else if e.is_connect() {
                "连接失败".to_string()
            } else {
                e.to_string()
            }),
        };
        (node.name().to_string(), status)
    }
}

impl NodeView {
    fn remote(remote: &UpstreamNode, status: &NodeStatus) -> Self {
        let info = status.info.as_ref();
        Self {
            id: remote.name().to_string(),
            name: remote.display_name().to_string(),
            url: Some(remote.base().to_string()),
            local: false,
            online: status.online,
            latency_ms: status.latency_ms,
            version: info.map(|info| info.version.clone()),
            hostname: info.map(|info| info.hostname.clone()),
            platform: info.map(|info| info.platform.clone()),
            processes: info.map_or(0, |info| info.processes),
            running: info.map_or(0, |info| info.running),
            last_error: status.last_error.clone(),
            checked_at: status.checked_at,
            terminal: false,
        }
    }
}

fn offline(error: String) -> NodeStatus {
    NodeStatus {
        last_error: Some(error),
        checked_at: Some(Utc::now()),
        ..NodeStatus::default()
    }
}

fn hostname() -> String {
    std::env::var("COMPUTERNAME")
        .or_else(|_| std::env::var("HOSTNAME"))
        .unwrap_or_else(|_| "unknown".to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn node(name: &str, url: &str, key: Option<&str>) -> NodeConfig {
        NodeConfig {
            name: name.to_string(),
            label: None,
            url: url.to_string(),
            api_key: key.map(str::to_string),
        }
    }

    #[tokio::test]
    async fn local_node_row_is_first_and_unnamed_url() {
        let manager = gugu_core::manager::start(&gugu_core::config::AppConfig::default(), None);
        let registry = NodeRegistry::new(
            &[node("web1", "http://127.0.0.1:9092", None)],
            Some("中心机".into()),
        );
        let views = registry.views(&manager, true);
        assert_eq!(views.len(), 2);
        assert_eq!(views[0].id, LOCAL_NODE_NAME);
        assert_eq!(views[0].name, "中心机");
        assert!(views[0].local && views[0].online);
        assert!(views[0].url.is_none());
        assert_eq!(views[1].id, "web1");
        assert!(!views[1].online, "首次探测前从节点应为未知/离线");
        assert_eq!(views[1].url.as_deref(), Some("http://127.0.0.1:9092"));
    }

    #[test]
    fn ws_url_switches_scheme_and_escapes_key() {
        let remote = UpstreamNode::new(&node("web1", "https://gugu.example.com", Some("a b+c")));
        assert_eq!(
            remote.ws_url(),
            "wss://gugu.example.com/api/v1/ws?token=a%20b%2Bc"
        );
        let plain = UpstreamNode::new(&node("web1", "http://127.0.0.1:9092", None));
        assert_eq!(plain.ws_url(), "ws://127.0.0.1:9092/api/v1/ws");
    }
}
