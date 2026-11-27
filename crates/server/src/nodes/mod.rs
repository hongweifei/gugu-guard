//! 多节点：一台中心节点统一管理多台跑着 gugu 的机器。
//!
//! 本机始终以 `local` 存在，`[[nodes]]` 里声明的是从节点。中心节点做三件事：
//! - `GET /api/v1/nodes` 给出带在线状态的节点表，5 秒一轮探测各从节点的 `GET /api/v1/node`；
//! - `/api/v1/nodes/{node}/{*rest}` 把请求原样转发到从节点的 `/api/v1/{rest}`，带上传入的
//!   Body、查询串与从节点自己的 API Key，响应状态码、类型与下载头原样返回；
//! - `/api/v1/ws?node=` 连上从节点的 `/api/v1/ws` 做双向中继，实时状态与日志跟着所选节点走。
//!
//! 从节点的 Key 只存在于中心节点的配置里，`GET /api/v1/nodes` 不会返回它。
//!
//! 三层分工：`registry` 管节点表与探测，`proxy` 管 HTTP 转发，`relay` 管 WebSocket 搬运。

mod proxy;
mod registry;
mod relay;

use std::net::SocketAddr;

use axum::extract::State;
use axum::http::{HeaderMap, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::routing::get;
use axum::{Json, Router};
use gugu_core::config::LOCAL_NODE_NAME;
use serde_json::json;

use crate::state::AppState;

/// 传给从节点的 `x-forwarded-for`：请求自带代理链就追加直连对端，否则只写直连对端。
///
/// 从节点的审计取第一段，看到的是原始客户端而不是中心节点的地址；中心节点前面还有
/// 反向代理时，原来那一段链原样保留下来，诊断时能看出请求经过了几层。
pub(crate) fn forwarded_for(headers: &HeaderMap, peer: Option<SocketAddr>) -> Option<String> {
    let incoming = headers
        .get("x-forwarded-for")
        .and_then(|value| value.to_str().ok())
        .map(str::trim)
        .filter(|value| !value.is_empty());
    let peer = peer.map(|addr| addr.ip().to_string());
    match (incoming, peer) {
        (Some(chain), Some(peer)) => Some(format!("{chain}, {peer}")),
        (Some(chain), None) => Some(chain.to_string()),
        (None, peer) => peer,
    }
}

pub(crate) use registry::UpstreamNode;
pub use registry::{NodeInfo, NodeRegistry, NodeView};
pub(crate) use relay::relay;
pub fn routes() -> Router<AppState> {
    Router::new()
        .route("/api/v1/nodes", get(list_nodes))
        .route("/api/v1/node", get(self_node))
        .route(
            "/api/v1/nodes/{node}/{*rest}",
            get(proxy::handle)
                .post(proxy::handle)
                .put(proxy::handle)
                .delete(proxy::handle)
                .patch(proxy::handle),
        )
}

/// 解析代理目标的失败原因：本机不需要代理，其余说明配置里没有这个名字。
pub(crate) enum LookupError {
    Local,
    Unknown(String),
}

impl LookupError {
    pub(crate) fn into_response(self) -> Response {
        match self {
            Self::Local => failure(
                StatusCode::BAD_REQUEST,
                "本机节点直接用 /api/v1/...，不需要代理".to_string(),
            ),
            Self::Unknown(node) => failure(StatusCode::NOT_FOUND, format!("未知节点: {node}")),
        }
    }
}

/// 解析代理目标：`local` 与未知标识各给一条可读错误，HTTP 代理与 WebSocket 共用这套口径。
pub(crate) fn upstream_of(state: &AppState, node: &str) -> Result<UpstreamNode, LookupError> {
    if node == LOCAL_NODE_NAME {
        return Err(LookupError::Local);
    }
    state
        .nodes
        .remote(node)
        .ok_or_else(|| LookupError::Unknown(node.to_string()))
}

fn failure(status: StatusCode, message: String) -> Response {
    (status, Json(json!({ "error": message }))).into_response()
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

#[cfg(test)]
mod tests {
    use super::*;
    use axum::http::HeaderValue;

    fn headers(value: Option<&str>) -> HeaderMap {
        let mut headers = HeaderMap::new();
        if let Some(value) = value {
            headers.insert("x-forwarded-for", HeaderValue::from_str(value).unwrap());
        }
        headers
    }

    fn peer(ip: &str) -> Option<SocketAddr> {
        Some(format!("{ip}:5000").parse().unwrap())
    }

    #[test]
    fn appends_the_peer_to_an_existing_chain() {
        assert_eq!(
            forwarded_for(&headers(Some("10.9.9.9")), peer("127.0.0.1")).as_deref(),
            Some("10.9.9.9, 127.0.0.1")
        );
        assert_eq!(
            forwarded_for(&headers(Some("10.9.9.9, 10.9.9.10")), peer("127.0.0.1")).as_deref(),
            Some("10.9.9.9, 10.9.9.10, 127.0.0.1")
        );
    }

    #[test]
    fn writes_the_peer_when_the_request_carries_no_chain() {
        assert_eq!(
            forwarded_for(&headers(None), peer("127.0.0.1")).as_deref(),
            Some("127.0.0.1")
        );
        assert_eq!(
            forwarded_for(&headers(Some("   ")), peer("192.168.1.7")).as_deref(),
            Some("192.168.1.7")
        );
    }

    #[test]
    fn keeps_the_chain_when_the_peer_is_unknown() {
        assert_eq!(
            forwarded_for(&headers(Some("10.9.9.9")), None).as_deref(),
            Some("10.9.9.9")
        );
        assert_eq!(forwarded_for(&headers(None), None), None);
    }
}
