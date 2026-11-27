//! 单个终端会话：浏览器 WebSocket 与 PTY 之间的双向搬运，以及打开/关闭的审计。

use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use axum::extract::ws::{Message, WebSocket};
use axum::http::{HeaderMap, StatusCode};
use chrono::Utc;
use gugu_core::audit::AuditEntry;
use portable_pty::PtySize;
use serde_json::{json, Value};

use super::pty::{spawn_pty, take_utf8_prefix, Pty};
use super::{SessionGuard, IDLE_TIMEOUT, MAX_SESSIONS};
use crate::audit::{auth_kind, client_of_headers, header_value};
use crate::state::AppState;

/// 打开与关闭审计里记录的 user-agent 长度上限。
const MAX_UA_CHARS: usize = 200;
/// 心跳间隔：同时用作探测客户端是否还在。
const HEARTBEAT: Duration = Duration::from_secs(30);

struct Session {
    socket: WebSocket,
    pty: Pty,
    /// 会话起点，用于关闭审计里记时长。
    started: Instant,
}

pub(super) async fn handle_socket(
    mut socket: WebSocket,
    state: AppState,
    headers: HeaderMap,
    // 名额在会话全程持有，函数结束（含提前返回）时由 Drop 归还。
    _guard: SessionGuard,
) {
    let cwd = state
        .terminal_cwd
        .clone()
        .unwrap_or_else(|| PathBuf::from("."));
    let (shell, args) = super::pty::shell_command();
    let client = client_of_headers(&headers);
    let user_agent = header_value(&headers, "user-agent", MAX_UA_CHARS);

    let mut session = match spawn_pty(&shell, &args, &cwd) {
        Ok(pty) => {
            record_audit(
                &state,
                "terminal.open",
                "打开终端",
                StatusCode::SWITCHING_PROTOCOLS.as_u16(),
                true,
                client,
                user_agent,
                Some(format!("{} @ {}", pty.shell, cwd.display())),
            );
            Session {
                socket,
                pty,
                started: Instant::now(),
            }
        }
        Err(error) => {
            tracing::warn!("创建终端会话失败: {error:#}");
            let shell_name = shell_name_of(&shell);
            record_audit(
                &state,
                "terminal.open",
                "打开终端",
                StatusCode::INTERNAL_SERVER_ERROR.as_u16(),
                false,
                client,
                user_agent,
                Some(format!("{shell_name} 启动失败: {error}")),
            );
            let _ = send_json(
                &mut socket,
                json!({ "type": "error", "message": format!("无法启动 shell: {error}") }),
            )
            .await;
            let _ = socket.send(Message::Close(None)).await;
            return;
        }
    };

    let _ = send_json(
        &mut session.socket,
        json!({
            "type": "ready",
            "shell": session.pty.shell,
            "cwd": cwd.display().to_string(),
            "max_sessions": MAX_SESSIONS,
            "idle_timeout_secs": IDLE_TIMEOUT.as_secs(),
        }),
    )
    .await;

    let outcome = run_session(&mut session, &state).await;
    let elapsed = session.started.elapsed();
    let shell = session.pty.shell.clone();

    record_audit(
        &state,
        "terminal.close",
        "关闭终端",
        StatusCode::OK.as_u16(),
        true,
        client_of_headers(&headers),
        header_value(&headers, "user-agent", MAX_UA_CHARS),
        Some(format!(
            "{outcome}，时长 {} 秒，shell {shell}",
            elapsed.as_secs()
        )),
    );

    let _ = session.socket.send(Message::Close(None)).await;
}

/// 会话主循环：浏览器输入 → PTY，PTY 输出 → 浏览器，直到任一侧结束或空闲超时。
async fn run_session(session: &mut Session, state: &AppState) -> &'static str {
    let idle = tokio::time::sleep(IDLE_TIMEOUT);
    tokio::pin!(idle);
    let mut heartbeat = tokio::time::interval(HEARTBEAT);
    // PTY 按字节流返回，UTF-8 多字节字符可能被切到两个块里；保留未完成的后缀。
    let mut pending_output = Vec::new();

    loop {
        tokio::select! {
            message = session.socket.recv() => {
                match message {
                    Some(Ok(Message::Text(text))) => {
                        idle.as_mut().reset(tokio::time::Instant::now() + IDLE_TIMEOUT);
                        if handle_client_message(session, text.as_str()).await.is_err() {
                            return "客户端断开";
                        }
                    }
                    Some(Ok(Message::Binary(bytes))) => {
                        idle.as_mut().reset(tokio::time::Instant::now() + IDLE_TIMEOUT);
                        if let Err(error) = session.pty.writer.write_all(&bytes) {
                            tracing::debug!("终端写入失败: {error}");
                            return "PTY 写入失败";
                        }
                    }
                    Some(Ok(Message::Close(_)) | Err(_)) | None => return "客户端断开",
                    Some(Ok(_)) => {}
                }
            }
            chunk = session.pty.output_rx.recv() => {
                let Some(chunk) = chunk else {
                    // 收尾时把不完整的尾巴按有损方式送出，至少不吞掉最后的提示符。
                    let rest = String::from_utf8_lossy(&pending_output).to_string();
                    if !rest.is_empty() {
                        let _ = send_json(
                            &mut session.socket,
                            json!({ "type": "data", "data": rest }),
                        ).await;
                    }
                    let code = session
                        .pty
                        .child
                        .try_wait()
                        .ok()
                        .flatten()
                        .map(|status| status.exit_code());
                    let _ = send_json(
                        &mut session.socket,
                        json!({ "type": "exit", "code": code, "reason": "shell 已退出" }),
                    ).await;
                    return "shell 已退出";
                };
                idle.as_mut().reset(tokio::time::Instant::now() + IDLE_TIMEOUT);
                pending_output.extend_from_slice(&chunk);
                let text = take_utf8_prefix(&mut pending_output);
                if text.is_empty() {
                    continue;
                }
                if send_json(&mut session.socket, json!({ "type": "data", "data": text }))
                    .await
                    .is_err()
                {
                    return "客户端断开";
                }
            }
            () = &mut idle => {
                let _ = send_json(
                    &mut session.socket,
                    json!({ "type": "exit", "code": null, "reason": "空闲超时" }),
                ).await;
                tracing::info!("终端会话空闲超时，已结束（{}）", state.terminal_cwd.as_deref().unwrap_or_else(|| Path::new(".")).display());
                return "空闲超时";
            }
            _ = heartbeat.tick() => {
                if session.socket.send(Message::Ping(Vec::new().into())).await.is_err() {
                    return "客户端断开";
                }
            }
        }
    }
}

/// 处理客户端 JSON 消息；返回 Err 表示应结束会话。
async fn handle_client_message(session: &mut Session, text: &str) -> Result<(), ()> {
    let Ok(message) = serde_json::from_str::<Value>(text) else {
        return Ok(());
    };
    match message.get("type").and_then(Value::as_str) {
        Some("input") => {
            let data = message
                .get("data")
                .and_then(Value::as_str)
                .unwrap_or_default();
            if data.is_empty() {
                return Ok(());
            }
            session
                .pty
                .writer
                .write_all(data.as_bytes())
                .map_err(|error| {
                    tracing::debug!("终端写入失败: {error}");
                })
        }
        Some("resize") => {
            let cols = message
                .get("cols")
                .and_then(Value::as_u64)
                .unwrap_or(80)
                .clamp(1, 1000) as u16;
            let rows = message
                .get("rows")
                .and_then(Value::as_u64)
                .unwrap_or(24)
                .clamp(1, 1000) as u16;
            // 尺寸只影响排版，失败不必断开会话。
            if let Err(error) = session.pty.master.resize(PtySize {
                rows,
                cols,
                pixel_width: 0,
                pixel_height: 0,
            }) {
                tracing::debug!("调整终端尺寸失败: {error}");
            }
            Ok(())
        }
        _ => Ok(()),
    }
}

async fn send_json(socket: &mut WebSocket, value: Value) -> Result<(), ()> {
    let text = serde_json::to_string(&value).map_err(|_| ())?;
    socket
        .send(Message::Text(text.into()))
        .await
        .map_err(|_| ())
}

fn shell_name_of(shell: &str) -> String {
    Path::new(shell).file_name().map_or_else(
        || shell.to_string(),
        |name| name.to_string_lossy().to_string(),
    )
}

/// 终端自己的开关事件：审计中间件只拦写请求，WS 升级由 handler 记录。
#[allow(clippy::too_many_arguments)]
fn record_audit(
    state: &AppState,
    action: &str,
    label: &str,
    status: u16,
    ok: bool,
    client: Option<String>,
    user_agent: Option<String>,
    detail: Option<String>,
) {
    state.audit.record(AuditEntry {
        at: Utc::now(),
        action: action.to_string(),
        label: label.to_string(),
        target: None,
        method: "GET".to_string(),
        path: "/api/v1/terminal/ws".to_string(),
        status,
        ok,
        duration_ms: 0,
        client: client.unwrap_or_else(|| "-".to_string()),
        user_agent,
        auth: auth_kind(state, status),
        detail,
    });
}
