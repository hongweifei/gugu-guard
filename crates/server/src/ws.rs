use axum::{
    extract::State,
    response::IntoResponse,
    routing::get,
    Router,
};
use serde_json::json;
use tokio::sync::broadcast;

use crate::state::AppState;

pub fn routes() -> Router<AppState> {
    Router::new().route("/api/v1/ws", get(ws_handler))
}

async fn ws_handler(
    ws: axum::extract::ws::WebSocketUpgrade,
    State(state): State<AppState>,
) -> impl IntoResponse {
    ws.on_upgrade(move |socket| handle_socket(socket, state))
}

async fn handle_socket(mut socket: axum::extract::ws::WebSocket, state: AppState) {
    let mut status_interval = tokio::time::interval(std::time::Duration::from_secs(2));
    let mut log_interval = tokio::time::interval(std::time::Duration::from_millis(200));

    let mut log_rx = subscribe_all(&state).await;

    let mut log_buf = Vec::new();

    loop {
        tokio::select! {
            _ = status_interval.tick() => {
                let processes = state.manager.list_processes();
                let current_names: Vec<String> = processes.iter().map(|p| p.name.clone()).collect();

                let msg = json!({
                    "type": "status",
                    "processes": processes,
                });

                if let Ok(text) = serde_json::to_string(&msg) {
                    if socket.send(axum::extract::ws::Message::Text(text)).await.is_err() {
                        break;
                    }
                }

                log_rx = update_subscriptions(&state, log_rx, &current_names).await;
            }
            _ = log_interval.tick() => {
                drain_log_entries(&mut log_rx, &mut log_buf);
                if !log_buf.is_empty() {
                    let entries: Vec<_> = std::mem::take(&mut log_buf);
                    let msg = json!({ "type": "logs", "entries": entries });
                    if let Ok(text) = serde_json::to_string(&msg) {
                        if socket.send(axum::extract::ws::Message::Text(text)).await.is_err() {
                            break;
                        }
                    }
                }
            }
            result = socket.recv() => {
                match result {
                    Some(Ok(axum::extract::ws::Message::Close(_))) | Some(Err(_)) | None => break,
                    Some(Ok(axum::extract::ws::Message::Text(text))) => {
                        if let Ok(cmd) = serde_json::from_str::<serde_json::Value>(&text) {
                            if cmd.get("type").and_then(|v| v.as_str()) == Some("subscribe") {
                                if let Some(process_name) = cmd.get("process").and_then(|v| v.as_str()) {
                                    if let Ok(rx) = state.manager.subscribe_process_logs(process_name).await {
                                        log_rx.retain(|(n, _)| n != process_name);
                                        log_rx.push((process_name.to_string(), rx));
                                    }
                                }
                            }
                        }
                    }
                    Some(Ok(_)) => {}
                }
            }
        }
    }
}

// ── 辅助函数 ──────────────────────────────────────────────

type LogRx = Vec<(String, broadcast::Receiver<gugu_core::process::LogEntry>)>;

/// 订阅当前所有进程的日志广播。
async fn subscribe_all(state: &AppState) -> LogRx {
    let names = state.manager.all_process_names();
    let mut rx_map = Vec::with_capacity(names.len());
    for name in &names {
        if let Ok(rx) = state.manager.subscribe_process_logs(name).await {
            rx_map.push((name.clone(), rx));
        }
    }
    rx_map
}

/// 根据当前进程名列表更新日志订阅。
async fn update_subscriptions(state: &AppState, current_rx: LogRx, current_names: &[String]) -> LogRx {
    let mut new_rx = Vec::with_capacity(current_names.len());
    for (name, rx) in current_rx.into_iter() {
        if current_names.contains(&name) {
            new_rx.push((name, rx));
        }
    }
    for name in current_names {
        if !new_rx.iter().any(|(n, _)| n == name) {
            if let Ok(rx) = state.manager.subscribe_process_logs(name).await {
                new_rx.push((name.clone(), rx));
            }
        }
    }
    new_rx
}

/// 非阻塞地排空所有日志广播接收器中的待处理消息。
fn drain_log_entries(log_rx: &mut LogRx, buf: &mut Vec<gugu_core::process::LogEntry>) {
    for (name, rx) in log_rx {
        loop {
            match rx.try_recv() {
                Ok(entry) => {
                    let mut e = entry.clone();
                    e.process_name = Some(name.clone());
                    buf.push(e);
                }
                Err(broadcast::error::TryRecvError::Lagged(n)) => {
                    tracing::debug!("[{}] 日志广播落后 {} 条，已跳过", name, n);
                }
                Err(broadcast::error::TryRecvError::Empty | broadcast::error::TryRecvError::Closed) => break,
            }
        }
    }
}
