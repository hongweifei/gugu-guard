//! 多节点：一台中心节点统一管理多台跑着 gugu 的机器。
//!
//! 本机始终以 `local` 存在，`[[nodes]]` 里声明的是从节点。中心节点做三件事：
//! - `GET /api/v1/nodes` 给出带在线状态的节点表，5 秒一轮探测各从节点的 `GET /api/v1/node`；
//! - `/api/v1/n/{node}/{*rest}` 把请求原样转发到从节点的 `/api/v1/{rest}`，带上传入的
//!   Body、查询串与从节点自己的 API Key，响应状态码、类型与下载头原样返回；
//! - `/api/v1/ws?node=` 连上从节点的 `/api/v1/ws` 做双向中继，实时状态与日志跟着所选节点走。
//!
//! 从节点的 Key 只存在于中心节点的配置里，`GET /api/v1/nodes` 不会返回它。

use std::collections::HashMap;
use std::sync::{Arc, RwLock};
use std::time::{Duration, Instant};

use axum::body::{to_bytes, Body};
use axum::extract::ws::{CloseFrame, Message, WebSocket};
use axum::extract::{Path, Request, State};
use axum::http::{header, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::routing::get;
use axum::{Json, Router};
use chrono::{DateTime, Utc};
use futures::{SinkExt, StreamExt};
use gugu_core::config::{NodeConfig, LOCAL_NODE_NAME};
use gugu_core::manager::SharedManager;
use gugu_core::process::ProcessStatus;
use serde::{Deserialize, Serialize};
use serde_json::json;
use tokio_tungstenite::tungstenite::Message as UpstreamMessage;

use crate::state::AppState;

/// 从节点健康探测间隔。
const POLL_INTERVAL: Duration = Duration::from_secs(5);
/// 单次探测超时：从节点掉线时不能拖住下一轮。
const POLL_TIMEOUT: Duration = Duration::from_secs(3);
/// 代理请求超时：启停、拉日志都在这个量级内。
const PROXY_TIMEOUT: Duration = Duration::from_secs(30);
/// 代理请求体上限；写配置的 JSON 远小于此值。
const MAX_PROXY_BODY: usize = 4 * 1024 * 1024;

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

#[derive(Debug, Clone, Default, Serialize)]
struct RemoteStatus {
    online: bool,
    latency_ms: Option<u64>,
    version: Option<String>,
    hostname: Option<String>,
    platform: Option<String>,
    processes: usize,
    running: usize,
    last_error: Option<String>,
    checked_at: Option<DateTime<Utc>>,
}

/// 一台从节点。Key 只在这里与探测/代理请求里出现。
#[derive(Debug, Clone)]
pub struct RemoteNode {
    name: String,
    label: Option<String>,
    base: String,
    api_key: Option<String>,
}

impl RemoteNode {
    fn display_name(&self) -> &str {
        self.label.as_deref().unwrap_or(&self.name)
    }

    /// 连从节点 WebSocket 的地址：scheme 换成 ws(s)，Key 走 query，与浏览器同一条认证路径。
    fn ws_url(&self) -> String {
        let base = if let Some(rest) = self.base.strip_prefix("https://") {
            format!("wss://{rest}")
        } else if let Some(rest) = self.base.strip_prefix("http://") {
            format!("ws://{rest}")
        } else {
            self.base.clone()
        };
        match &self.api_key {
            Some(key) => format!("{base}/api/v1/ws?token={}", encode_query(key)),
            None => format!("{base}/api/v1/ws"),
        }
    }
}

/// 节点注册表：本机身份 + 从节点列表 + 最近一次探测结果。
pub struct NodeRegistry {
    local_name: String,
    hostname: String,
    started: Instant,
    remotes: Vec<RemoteNode>,
    client: reqwest::Client,
    status: RwLock<HashMap<String, RemoteStatus>>,
}

impl NodeRegistry {
    #[must_use]
    pub fn new(nodes: &[NodeConfig], local_name: Option<String>) -> Self {
        let hostname = hostname();
        let local_name = local_name
            .map(|name| name.trim().to_string())
            .filter(|name| !name.is_empty())
            .unwrap_or_else(|| hostname.clone());
        let remotes = nodes
            .iter()
            .map(|node| RemoteNode {
                name: node.name.trim().to_string(),
                label: node
                    .label
                    .as_deref()
                    .map(str::trim)
                    .filter(|label| !label.is_empty())
                    .map(str::to_string),
                base: node.base_url(),
                api_key: node
                    .api_key
                    .as_deref()
                    .map(str::trim)
                    .filter(|key| !key.is_empty())
                    .map(str::to_string),
            })
            .collect();
        let client = reqwest::Client::builder()
            .user_agent(concat!("gugu-hub/", env!("CARGO_PKG_VERSION")))
            .connect_timeout(POLL_TIMEOUT)
            .build()
            .unwrap_or_default();
        Self {
            local_name,
            hostname,
            started: Instant::now(),
            remotes,
            client,
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

    #[must_use]
    pub fn remote(&self, name: &str) -> Option<RemoteNode> {
        self.remotes.iter().find(|node| node.name == name).cloned()
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
        for remote in &self.remotes {
            let current = status.get(&remote.name).cloned().unwrap_or_default();
            views.push(NodeView {
                id: remote.name.clone(),
                name: remote.display_name().to_string(),
                url: Some(remote.base.clone()),
                local: false,
                online: current.online,
                latency_ms: current.latency_ms,
                version: current.version,
                hostname: current.hostname,
                platform: current.platform,
                processes: current.processes,
                running: current.running,
                last_error: current.last_error,
                checked_at: current.checked_at,
                terminal: false,
            });
        }
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

    /// 起一个后台任务，每 5 秒探测一轮从节点；没有从节点时立即返回空任务。
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

    async fn probe(&self, node: &RemoteNode) -> (String, RemoteStatus) {
        let started = Instant::now();
        let mut request = self
            .client
            .get(format!("{}/api/v1/node", node.base))
            .timeout(POLL_TIMEOUT);
        if let Some(key) = &node.api_key {
            request = request.bearer_auth(key);
        }
        let status = match request.send().await {
            Ok(response) if response.status().is_success() => {
                let latency_ms = started.elapsed().as_millis() as u64;
                match response.json::<NodeInfo>().await {
                    Ok(info) => RemoteStatus {
                        online: true,
                        latency_ms: Some(latency_ms),
                        version: Some(info.version),
                        hostname: Some(info.hostname),
                        platform: Some(info.platform),
                        processes: info.processes,
                        running: info.running,
                        last_error: None,
                        checked_at: Some(Utc::now()),
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
        (node.name.clone(), status)
    }
}

fn offline(error: String) -> RemoteStatus {
    RemoteStatus {
        online: false,
        last_error: Some(error),
        checked_at: Some(Utc::now()),
        ..RemoteStatus::default()
    }
}

fn hostname() -> String {
    std::env::var("COMPUTERNAME")
        .or_else(|_| std::env::var("HOSTNAME"))
        .unwrap_or_else(|_| "unknown".to_string())
}

pub fn routes() -> Router<AppState> {
    Router::new()
        .route("/api/v1/nodes", get(list_nodes))
        .route("/api/v1/node", get(self_node))
        .route(
            "/api/v1/n/{node}/{*rest}",
            get(proxy).post(proxy).put(proxy).delete(proxy).patch(proxy),
        )
}

async fn list_nodes(State(state): State<AppState>) -> Json<serde_json::Value> {
    Json(json!({
        "local": LOCAL_NODE_NAME,
        "local_name": state.nodes.local_name(),
        "nodes": state.nodes.views(&state.manager, state.terminal_enabled),
    }))
}

async fn self_node(State(state): State<AppState>) -> Json<NodeInfo> {
    Json(
        state
            .nodes
            .self_info(&state.manager, state.terminal_enabled),
    )
}

/// 转发到从节点：方法、查询串、请求体与业务头都带上，认证换成从节点自己的 Key。
async fn proxy(
    State(state): State<AppState>,
    Path((node, rest)): Path<(String, String)>,
    req: Request,
) -> Response {
    let Some(remote) = state.nodes.remote(&node) else {
        return if node == LOCAL_NODE_NAME {
            failure(
                StatusCode::BAD_REQUEST,
                "本机节点直接用 /api/v1/...，不需要代理".to_string(),
            )
        } else {
            failure(StatusCode::NOT_FOUND, format!("未知节点: {node}"))
        };
    };

    let method = req.method().clone();
    let query = req.uri().query().map(str::to_string);
    let headers = req.headers().clone();
    let Ok(body) = to_bytes(req.into_body(), MAX_PROXY_BODY).await else {
        return failure(
            StatusCode::PAYLOAD_TOO_LARGE,
            format!("请求体超过 {} MB", MAX_PROXY_BODY / 1024 / 1024),
        );
    };

    let url = match query {
        Some(query) => format!("{}/api/v1/{rest}?{query}", remote.base),
        None => format!("{}/api/v1/{rest}", remote.base),
    };
    let mut builder = state
        .nodes
        .client
        .request(method, &url)
        .timeout(PROXY_TIMEOUT);
    for (name, value) in &headers {
        if forward_request_header(name.as_str()) {
            builder = builder.header(name, value);
        }
    }
    if let Some(key) = &remote.api_key {
        builder = builder.bearer_auth(key);
    }

    let upstream = match builder.body(body).send().await {
        Ok(response) => response,
        Err(e) => return failure(StatusCode::BAD_GATEWAY, format!("节点 {node} 不可达: {e}")),
    };
    let status = upstream.status();
    let upstream_headers = upstream.headers().clone();
    let bytes = match upstream.bytes().await {
        Ok(bytes) => bytes,
        Err(e) => {
            return failure(
                StatusCode::BAD_GATEWAY,
                format!("读取节点 {node} 的响应失败: {e}"),
            )
        }
    };

    let mut response = Response::builder()
        .status(status)
        .header("x-gugu-node", node.as_str());
    for name in [
        header::CONTENT_TYPE,
        header::CONTENT_DISPOSITION,
        header::CACHE_CONTROL,
    ] {
        if let Some(value) = upstream_headers.get(&name) {
            response = response.header(name, value);
        }
    }
    response
        .body(Body::from(bytes))
        .unwrap_or_else(|_| failure(StatusCode::BAD_GATEWAY, "构造代理响应失败".to_string()))
}

/// 逐跳头与认证头不转发：认证由中心节点按从节点的 Key 重新签发。
fn forward_request_header(name: &str) -> bool {
    !matches!(
        name,
        "host"
            | "authorization"
            | "content-length"
            | "connection"
            | "transfer-encoding"
            | "accept-encoding"
            | "upgrade"
            | "keep-alive"
            | "proxy-connection"
    )
}

fn failure(status: StatusCode, message: String) -> Response {
    (status, Json(json!({ "error": message }))).into_response()
}

/// 把浏览器连接与从节点的 `/api/v1/ws` 接起来：实时状态与日志跟着所选节点走。
pub async fn relay_ws(remote: RemoteNode, socket: WebSocket, node: String) {
    let url = remote.ws_url();
    let upstream = match tokio_tungstenite::connect_async(&url).await {
        Ok((stream, _)) => stream,
        Err(e) => {
            tracing::debug!("节点 {node} 的 WebSocket 连接失败: {e}");
            let mut socket = socket;
            let reason = format!("节点 {node} 不可达: {e}");
            let _ = socket
                .send(Message::Close(Some(CloseFrame {
                    code: 1011,
                    reason: reason.into(),
                })))
                .await;
            return;
        }
    };

    let (mut upstream_tx, mut upstream_rx) = upstream.split();
    let (mut socket_tx, mut socket_rx) = socket.split();

    loop {
        tokio::select! {
            message = upstream_rx.next() => {
                let Some(Ok(message)) = message else { break };
                let forwarded = match message {
                    UpstreamMessage::Text(text) => Message::Text(text.to_string().into()),
                    UpstreamMessage::Binary(bytes) => Message::Binary(bytes),
                    UpstreamMessage::Ping(bytes) => Message::Ping(bytes),
                    UpstreamMessage::Pong(bytes) => Message::Pong(bytes),
                    UpstreamMessage::Close(_) => break,
                    UpstreamMessage::Frame(_) => continue,
                };
                if socket_tx.send(forwarded).await.is_err() {
                    break;
                }
            }
            message = socket_rx.next() => {
                let Some(Ok(message)) = message else { break };
                let forwarded = match message {
                    Message::Text(text) => UpstreamMessage::Text(text.to_string().into()),
                    Message::Binary(bytes) => UpstreamMessage::Binary(bytes),
                    Message::Ping(bytes) => UpstreamMessage::Ping(bytes),
                    Message::Pong(bytes) => UpstreamMessage::Pong(bytes),
                    Message::Close(_) => break,
                };
                if upstream_tx.send(forwarded).await.is_err() {
                    break;
                }
            }
        }
    }

    let _ = socket_tx.send(Message::Close(None)).await;
    let _ = upstream_tx.close().await;
}

/// API Key 放进查询串时要编码：Key 里可能有 `+ / =`。
fn encode_query(raw: &str) -> String {
    let mut out = String::with_capacity(raw.len());
    for byte in raw.bytes() {
        match byte {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'.' | b'_' | b'~' => {
                out.push(byte as char);
            }
            _ => out.push_str(&format!("%{byte:02X}")),
        }
    }
    out
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
    async fn local_node_id_is_reserved_and_first() {
        let manager = gugu_core::manager::start(&gugu_core::config::AppConfig::default(), None);
        let registry = NodeRegistry::new(
            &[node("web1", "http://127.0.0.1:9092", None)],
            Some("中心机".into()),
        );
        let views = registry.views(&manager, true);
        assert_eq!(views.len(), 2);
        assert_eq!(views[0].id, LOCAL_NODE_NAME);
        assert_eq!(views[0].name, "中心机");
        assert!(views[0].local);
        assert!(views[0].online);
        assert_eq!(views[1].id, "web1");
        assert!(!views[1].online, "首次探测前从节点应为未知/离线");
        assert_eq!(views[1].url.as_deref(), Some("http://127.0.0.1:9092"));
    }

    #[test]
    fn ws_url_switches_scheme_and_encodes_key() {
        let remote = RemoteNode {
            name: "web1".into(),
            label: None,
            base: "https://gugu.example.com".into(),
            api_key: Some("a b+c".into()),
        };
        assert_eq!(
            remote.ws_url(),
            "wss://gugu.example.com/api/v1/ws?token=a%20b%2Bc"
        );
        let plain = RemoteNode {
            name: "web1".into(),
            label: None,
            base: "http://127.0.0.1:9092".into(),
            api_key: None,
        };
        assert_eq!(plain.ws_url(), "ws://127.0.0.1:9092/api/v1/ws");
    }

    #[test]
    fn auth_headers_are_not_forwarded() {
        assert!(!forward_request_header("authorization"));
        assert!(!forward_request_header("host"));
        assert!(!forward_request_header("content-length"));
        assert!(forward_request_header("content-type"));
        assert!(forward_request_header("accept"));
    }
}
