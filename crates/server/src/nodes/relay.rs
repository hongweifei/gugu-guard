//! WebSocket 中继：浏览器连中心节点，中心节点连从节点，双向搬运帧。

use axum::extract::ws::{CloseFrame, Message, WebSocket};
use futures::{SinkExt, StreamExt};
use tokio_tungstenite::tungstenite::Message as UpstreamMessage;

use super::registry::UpstreamNode;

/// 把浏览器连接与从节点的 `/api/v1/ws` 接起来：实时状态与日志跟着所选节点走。
pub(crate) async fn relay(remote: UpstreamNode, socket: WebSocket, node: String) {
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
