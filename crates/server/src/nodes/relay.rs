//! WebSocket 中继：浏览器连中心节点，中心节点连从节点，双向搬运帧。

use axum::extract::ws::{CloseFrame, Message, WebSocket};
use axum::http::Request;
use futures::{SinkExt, StreamExt};
use tokio_tungstenite::tungstenite::client::IntoClientRequest;
use tokio_tungstenite::tungstenite::Message as UpstreamMessage;

use super::registry::UpstreamNode;

/// 把浏览器连接与从节点的 `/api/v1/ws` 接起来：实时状态与日志跟着所选节点走。
///
/// `forwarded` 是中心节点认定的客户端地址，随升级请求带给从节点，
/// 从节点的终端会话与审计据此记下来源，而不是只看到中心节点。
pub(crate) async fn relay(
    remote: UpstreamNode,
    socket: WebSocket,
    node: String,
    forwarded: Option<String>,
) {
    let url = remote.ws_url();
    let request = match upgrade_request(&url, forwarded.as_deref()) {
        Ok(request) => request,
        Err(reason) => return fail(socket, &node, reason).await,
    };
    let upstream = match tokio_tungstenite::connect_async(request).await {
        Ok((stream, _)) => stream,
        Err(e) => return fail(socket, &node, e.to_string()).await,
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

/// 构造发往从节点的升级请求：Key 仍在 URL 上，客户端地址放进 `x-forwarded-for`。
fn upgrade_request(url: &str, forwarded: Option<&str>) -> Result<Request<()>, String> {
    let mut request = url.into_client_request().map_err(|e| e.to_string())?;
    if let Some(forwarded) = forwarded {
        let value = forwarded
            .parse()
            .map_err(|e| format!("x-forwarded-for 不合法: {e}"))?;
        request.headers_mut().insert("x-forwarded-for", value);
    }
    Ok(request)
}

/// 连不上从节点时给浏览器发一帧关闭说明，面板据此显示红字。
async fn fail(socket: WebSocket, node: &str, reason: String) {
    tracing::debug!("节点 {node} 的 WebSocket 连接失败: {reason}");
    let mut socket = socket;
    let _ = socket
        .send(Message::Close(Some(CloseFrame {
            code: 1011,
            reason: format!("节点 {node} 不可达: {reason}").into(),
        })))
        .await;
}
