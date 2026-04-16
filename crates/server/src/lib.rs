pub mod api;
pub mod state;
pub mod ws;

use axum::Router;
use gugu_core::manager::SharedManager;
use state::AppState;
use std::net::SocketAddr;
use tower_http::cors::CorsLayer;
use tower_http::services::ServeDir;

pub async fn run_server(
    addr: SocketAddr,
    manager: SharedManager,
    web_dir: Option<String>,
    mut shutdown_rx: tokio::sync::oneshot::Receiver<()>,
) -> anyhow::Result<()> {
    let state = AppState::new(manager);

    let mut app = Router::new()
        .merge(api::routes())
        .merge(ws::routes())
        .with_state(state)
        .layer(CorsLayer::permissive());

    if let Some(dir) = &web_dir {
        let path = std::path::PathBuf::from(dir);
        if path.exists() {
            app = app.fallback_service(ServeDir::new(dir));
        }
    }

    let listener = tokio::net::TcpListener::bind(addr).await?;
    tracing::info!("Web 服务已启动: http://{addr}");

    axum::serve(listener, app)
        .with_graceful_shutdown(async move {
            let _ = (&mut shutdown_rx).await;
        })
        .await?;

    Ok(())
}
