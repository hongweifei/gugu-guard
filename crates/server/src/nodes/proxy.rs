//! HTTP 代理：把中心节点收到的 `/api/v1/nodes/{node}/...` 原样转给从节点。
//!
//! 认证换成从节点自己的 Key；响应边收边转，不把大文件读进内存。

use axum::body::{to_bytes, Body};
use axum::extract::{Path, Request, State};
use axum::http::{header, StatusCode};
use axum::response::Response;

use super::forwarded_for;
use crate::peer::PeerAddr;
use crate::state::AppState;

/// 代理请求体上限；写配置的 JSON 远小于此值。
const MAX_PROXY_BODY: usize = 4 * 1024 * 1024;

/// 转发到从节点：方法、查询串、请求体与业务头都带上，认证换成从节点自己的 Key。
pub(crate) async fn handle(
    State(state): State<AppState>,
    Path((node, rest)): Path<(String, String)>,
    peer: PeerAddr,
    req: Request,
) -> Response {
    let remote = match super::upstream_of(&state, &node) {
        Ok(remote) => remote,
        Err(e) => return e.into_response(),
    };

    let method = req.method().clone();
    let query = req.uri().query().map(str::to_string);
    let headers = req.headers().clone();
    let forwarded = forwarded_for(&headers, peer.0);
    let Ok(body) = to_bytes(req.into_body(), MAX_PROXY_BODY).await else {
        return super::failure(
            StatusCode::PAYLOAD_TOO_LARGE,
            format!("请求体超过 {} MB", MAX_PROXY_BODY / 1024 / 1024),
        );
    };

    let url = match query {
        Some(query) => format!("{}/api/v1/{rest}?{query}", remote.base()),
        None => format!("{}/api/v1/{rest}", remote.base()),
    };
    let mut builder = state.nodes.http_client().request(method, &url);
    for (name, value) in &headers {
        if forward_request_header(name.as_str()) {
            builder = builder.header(name, value);
        }
    }
    if let Some(forwarded) = forwarded {
        builder = builder.header("x-forwarded-for", forwarded);
    }
    if let Some(key) = remote.api_key() {
        builder = builder.bearer_auth(key);
    }

    let upstream = match builder.body(body).send().await {
        Ok(response) => response,
        Err(e) => {
            return super::failure(StatusCode::BAD_GATEWAY, format!("节点 {node} 不可达: {e}"))
        }
    };
    let status = upstream.status();
    let upstream_headers = upstream.headers().clone();

    let mut response = Response::builder()
        .status(status)
        .header("x-gugu-node", node.as_str());
    for name in [
        header::CONTENT_TYPE,
        header::CONTENT_DISPOSITION,
        header::CACHE_CONTROL,
        header::ETAG,
        header::LAST_MODIFIED,
    ] {
        if let Some(value) = upstream_headers.get(&name) {
            response = response.header(name, value);
        }
    }
    response
        .body(Body::from_stream(upstream.bytes_stream()))
        .unwrap_or_else(|_| super::failure(StatusCode::BAD_GATEWAY, "构造代理响应失败".to_string()))
}

/// 逐跳头与认证头不转发：认证由中心节点按从节点的 Key 重新签发；
/// `x-forwarded-for` 也不透传，由中心节点按「原链路 + 直连对端」重写一份。
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
            | "x-forwarded-for"
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn auth_and_hop_by_hop_headers_are_not_forwarded() {
        assert!(!forward_request_header("authorization"));
        assert!(!forward_request_header("host"));
        assert!(!forward_request_header("content-length"));
        assert!(!forward_request_header("accept-encoding"));
        assert!(!forward_request_header("x-forwarded-for"));
        assert!(forward_request_header("content-type"));
        assert!(forward_request_header("accept"));
        assert!(forward_request_header("user-agent"));
    }
}
