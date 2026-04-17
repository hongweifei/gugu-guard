use axum::{
    extract::State,
    response::IntoResponse,
    routing::get,
    Router,
};
use gugu_core::process::ProcessInfo;
use serde_json::json;

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
    let mut interval = tokio::time::interval(std::time::Duration::from_secs(2));

    loop {
        tokio::select! {
            _ = interval.tick() => {
                let mgr = state.manager.read().await;
                let processes: Vec<ProcessInfo> = mgr.list_processes();
                drop(mgr);

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
            }
            result = socket.recv() => {
                match result {
                    Some(Ok(axum::extract::ws::Message::Close(_))) => break,
                    Some(Ok(_)) => {}
                    Some(Err(_)) => break,
                    None => break,
                }
            }
        }
    }
}
