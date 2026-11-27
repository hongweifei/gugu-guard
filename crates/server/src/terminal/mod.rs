//! WebUI 内置终端：把浏览器的 WebSocket 接到一个真实 PTY。
//!
//! 这里不做逐行命令执行，而是让 shell 自己跑在伪终端里，因此 `cd`、分页器、
//! 彩色输出、交互式程序都能像本地终端一样工作。每个连接一个会话，彼此独立。
//!
//! 安全边界：终端默认关闭；路由挂在 `protected` 下，自动继承 API Key 认证；
//! 监听非回环地址且未配置 API Key 时，CLI 会在启动阶段拒绝运行。
//!
//! 分工：本文件只放路由、常量与会话名额；会话循环在 [`session`]，PTY 与 shell
//! 探测在 [`pty`]，同源校验在 [`origin`]，启动门槛在 [`exposure`]。

mod exposure;
mod origin;
mod pty;
mod session;

use std::path::Path;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::Duration;

use axum::extract::ws::WebSocketUpgrade;
use axum::extract::State;
use axum::http::{HeaderMap, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::routing::get;
use axum::{Json, Router};
use serde_json::{json, Value};

use crate::state::AppState;

pub use exposure::check_exposure;

/// 同时打开的终端会话上限。
pub const MAX_SESSIONS: usize = 4;
/// 无输入也无输出多久后自动结束会话。
pub const IDLE_TIMEOUT: Duration = Duration::from_secs(30 * 60);

static ACTIVE_SESSIONS: AtomicUsize = AtomicUsize::new(0);

pub fn routes() -> Router<AppState> {
    Router::new()
        .route("/api/v1/terminal", get(info_handler))
        .route("/api/v1/terminal/ws", get(ws_handler))
}

/// 终端是否可用；关闭时返回 `enabled:false`，前端据此提示而不是静默隐藏。
async fn info_handler(State(state): State<AppState>) -> Json<Value> {
    if !state.terminal_enabled {
        return Json(json!({ "enabled": false }));
    }
    Json(json!({
        "enabled": true,
        "shell": pty::shell_program(),
        "cwd": state.terminal_cwd.as_deref().unwrap_or_else(|| Path::new(".")).display().to_string(),
        "max_sessions": MAX_SESSIONS,
        "idle_timeout_secs": IDLE_TIMEOUT.as_secs(),
    }))
}

/// 未启用时在升级前直接 403，避免浏览器先建立连接再收到一屏错误。
async fn ws_handler(
    ws: WebSocketUpgrade,
    State(state): State<AppState>,
    headers: HeaderMap,
) -> Response {
    if !state.terminal_enabled {
        return (
            StatusCode::FORBIDDEN,
            Json(json!({ "error": "终端未启用" })),
        )
            .into_response();
    }
    // WebSocket 不受浏览器同源策略约束：没有这道检查，任何网页都能连本机终端。
    let has_key = state.api_key.as_deref().is_some_and(|key| !key.is_empty());
    if !origin::origin_allowed(&headers, has_key) {
        tracing::warn!("拒绝跨站终端 WebSocket 请求");
        return (
            StatusCode::FORBIDDEN,
            Json(json!({ "error": "终端只接受同源页面连接" })),
        )
            .into_response();
    }
    let Some(guard) = SessionGuard::acquire() else {
        return (
            StatusCode::TOO_MANY_REQUESTS,
            Json(json!({ "error": format!("终端会话已达上限（{MAX_SESSIONS} 个）") })),
        )
            .into_response();
    };
    ws.on_upgrade(move |socket| session::handle_socket(socket, state, headers, guard))
}

/// 会话名额，Drop 时归还，保证异常路径也不会泄漏名额。
pub(crate) struct SessionGuard;

impl SessionGuard {
    fn acquire() -> Option<Self> {
        let mut current = ACTIVE_SESSIONS.load(Ordering::SeqCst);
        loop {
            if current >= MAX_SESSIONS {
                return None;
            }
            match ACTIVE_SESSIONS.compare_exchange(
                current,
                current + 1,
                Ordering::SeqCst,
                Ordering::SeqCst,
            ) {
                Ok(_) => return Some(Self),
                Err(actual) => current = actual,
            }
        }
    }
}

impl Drop for SessionGuard {
    fn drop(&mut self) {
        ACTIVE_SESSIONS.fetch_sub(1, Ordering::SeqCst);
    }
}
