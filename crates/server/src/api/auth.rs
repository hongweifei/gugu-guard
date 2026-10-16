//! 认证中间件：Bearer 头或 ?token= 查询参数，constant-time 比对。

use axum::{
    extract::{Request, State},
    http::StatusCode,
    middleware::Next,
    response::{IntoResponse, Response},
};

use crate::state::AppState;

pub async fn auth_middleware(State(state): State<AppState>, req: Request, next: Next) -> Response {
    let Some(ref expected_key) = state.api_key else {
        return next.run(req).await;
    };
    if expected_key.is_empty() {
        return next.run(req).await;
    }

    // Bearer token
    if let Some(key) = req
        .headers()
        .get("Authorization")
        .and_then(|v| v.to_str().ok())
        .and_then(|v| v.strip_prefix("Bearer "))
    {
        if constant_time_eq(key, expected_key) {
            return next.run(req).await;
        }
    }

    // Query token
    if let Some(query) = req.uri().query() {
        for pair in query.split('&') {
            if let Some(token) = pair.strip_prefix("token=") {
                if constant_time_eq(token, expected_key) {
                    return next.run(req).await;
                }
            }
        }
    }

    StatusCode::UNAUTHORIZED.into_response()
}

/// Constant-time 字符串比较，防止 timing attack。
fn constant_time_eq(a: &str, b: &str) -> bool {
    let a_bytes = a.as_bytes();
    let b_bytes = b.as_bytes();
    let mut result: u8 = if a_bytes.len() == b_bytes.len() {
        0
    } else {
        0xff
    };
    let max_len = a_bytes.len().max(b_bytes.len());
    for i in 0..max_len {
        let x = a_bytes.get(i).copied().unwrap_or(0);
        let y = b_bytes.get(i).copied().unwrap_or(0);
        result |= x ^ y;
    }
    result == 0
}
