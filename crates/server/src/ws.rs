use axum::{
    extract::State,
    response::IntoResponse,
    routing::get,
    Router,
};
use gugu_core::process::{LogEntry, ProcessInfo};
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

    let mut log_rx = {
        let mgr = state.manager.read().await;
        let names = mgr.all_process_names();
        let mut rx_map: Vec<(String, broadcast::Receiver<LogEntry>)> = Vec::new();
        for name in &names {
            if let Ok(rx) = mgr.subscribe_process_logs(name) {
                rx_map.push((name.clone(), rx));
            }
        }
        rx_map
    };

    let mut log_buf = Vec::new();

    loop {
        tokio::select! {
            _ = status_interval.tick() => {
                let mgr = state.manager.read().await;
                let processes: Vec<ProcessInfo> = mgr.list_processes();
                let current_names: Vec<String> = processes.iter().map(|p| p.name.clone()).collect();

                let msg = json!({
                    "type": "status",
                    "processes": processes,
                });

                match serde_json::to_string(&msg) {
                    Ok(text) => {
                        if socket.send(axum::extract::ws::Message::Text(text)).await.is_err() {
                            break;
                        }
                    }
                    Err(_) => continue,
                }

                let mut new_rx = Vec::new();
                for (name, rx) in log_rx.drain(..) {
                    if current_names.contains(&name) {
                        new_rx.push((name, rx));
                    }
                }
                for name in &current_names {
                    if !new_rx.iter().any(|(n, _)| n == name) {
                        if let Ok(rx) = mgr.subscribe_process_logs(name) {
                            new_rx.push((name.clone(), rx));
                        }
                    }
                }
                log_rx = new_rx;
                drop(mgr);
            }
            _ = log_interval.tick() => {
                for (name, rx) in &mut log_rx {
                    loop {
                        match rx.try_recv() {
                            Ok(entry) => {
                                let mut e = entry.clone();
                                e.process_name = Some(name.clone());
                                log_buf.push(e);
                            }
                            Err(broadcast::error::TryRecvError::Empty) => break,
                            Err(broadcast::error::TryRecvError::Lagged(n)) => {
                                tracing::debug!("[{}] 日志广播落后 {} 条，已跳过", name, n);
                                break;
                            }
                            Err(broadcast::error::TryRecvError::Closed) => break,
                        }
                    }
                }
                if !log_buf.is_empty() {
                    let entries: Vec<_> = std::mem::take(&mut log_buf);
                    let msg = json!({
                        "type": "logs",
                        "entries": entries,
                    });
                    match serde_json::to_string(&msg) {
                        Ok(text) => {
                            if socket.send(axum::extract::ws::Message::Text(text)).await.is_err() {
                                break;
                            }
                        }
                        Err(_) => continue,
                    }
                }
            }
            result = socket.recv() => {
                match result {
                    Some(Ok(axum::extract::ws::Message::Close(_))) => break,
                    Some(Ok(axum::extract::ws::Message::Text(text))) => {
                        if let Ok(cmd) = serde_json::from_str::<serde_json::Value>(&text) {
                            if cmd.get("type").and_then(|v| v.as_str()) == Some("subscribe") {
                                if let Some(process_name) = cmd.get("process").and_then(|v| v.as_str()) {
                                    let mgr = state.manager.read().await;
                                    if let Ok(rx) = mgr.subscribe_process_logs(process_name) {
                                        log_rx.retain(|(n, _)| n != process_name);
                                        log_rx.push((process_name.to_string(), rx));
                                    }
                                }
                            }
                        }
                    }
                    Some(Ok(_)) => {}
                    Some(Err(_)) => break,
                    None => break,
                }
            }
        }
    }
}
