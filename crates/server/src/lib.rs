pub mod api;
pub mod assets;
pub mod state;
pub mod ws;

use axum::body::Body;
use axum::extract::Request;
use axum::http::{header, StatusCode};
use axum::response::{Html, IntoResponse, Response};
use axum::Router;
use gugu_core::manager::SharedManager;
use state::AppState;
use std::net::SocketAddr;
use tower_http::cors::CorsLayer;

async fn embedded_static_handler(req: Request) -> Response {
    let path = req.uri().path().trim_start_matches('/');
    let path = if path.is_empty() { "index.html" } else { path };

    match assets::WebAssets::get(path) {
        Some(file) => {
            let mime = mime_guess::from_path(path).first_or_octet_stream();
            Response::builder()
                .header(header::CONTENT_TYPE, mime.as_ref())
                .body(Body::from(file.data.to_vec()))
                .unwrap()
        }
        None => match assets::WebAssets::get("index.html") {
            Some(f) => Html(String::from_utf8_lossy(&f.data).to_string()).into_response(),
            None => StatusCode::NOT_FOUND.into_response(),
        },
    }
}

pub async fn run_server(
    addr: SocketAddr,
    manager: SharedManager,
    _web_dir: Option<String>,
    mut shutdown_rx: tokio::sync::oneshot::Receiver<()>,
) -> anyhow::Result<()> {
    let state = AppState::new(manager);

    let app = Router::new()
        .merge(api::routes())
        .merge(ws::routes())
        .with_state(state)
        .layer(CorsLayer::permissive())
        .fallback(|req| async move { embedded_static_handler(req).await });

    let listener = tokio::net::TcpListener::bind(addr).await?;
    tracing::info!("Web 服务已启动: http://{addr}");

    axum::serve(listener, app)
        .with_graceful_shutdown(async move {
            let _ = (&mut shutdown_rx).await;
        })
        .await?;

    Ok(())
}
