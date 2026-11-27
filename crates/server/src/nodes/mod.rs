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

use axum::extract::State;
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::routing::get;
use axum::{Json, Router};
use gugu_core::config::LOCAL_NODE_NAME;
use serde_json::json;

use crate::state::AppState;

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
