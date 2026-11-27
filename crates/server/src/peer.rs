//! 直连对端地址：HTTP 代理与 WebSocket 升级都要把客户端地址带给从节点。

use std::convert::Infallible;
use std::net::SocketAddr;

use axum::extract::{ConnectInfo, FromRequestParts};
use axum::http::request::Parts;

/// 本次连接的对端地址；服务没启用 `connect_info` 时（测试里的 `oneshot`、
/// 裸 `serve`）为 None，处理器照常工作，来源链路只保留请求里已带的部分。
pub(crate) struct PeerAddr(pub(crate) Option<SocketAddr>);

impl<S> FromRequestParts<S> for PeerAddr
where
    S: Send + Sync,
{
    type Rejection = Infallible;

    async fn from_request_parts(parts: &mut Parts, _state: &S) -> Result<Self, Self::Rejection> {
        Ok(Self(
            parts
                .extensions
                .get::<ConnectInfo<SocketAddr>>()
                .map(|info| info.0),
        ))
    }
}
