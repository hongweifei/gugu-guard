pub mod api;
pub mod assets;
pub mod metrics;
pub mod state;
pub mod ws;

use axum::body::Body;
use axum::extract::Request;
use axum::http::{header, HeaderValue, StatusCode};
use axum::middleware;
use axum::response::{Html, IntoResponse, Response};
use axum::Router;
use gugu_core::manager::SharedManager;
use state::AppState;
use std::net::SocketAddr;
use tower_http::cors::CorsLayer;

fn embedded_static_handler(req: Request) -> Response {
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

/// 启动 HTTP/WebSocket 服务器。
///
/// # Errors
/// 端口绑定失败或服务器运行异常时返回错误。
pub async fn run_server(
    addr: SocketAddr,
    manager: SharedManager,
    api_key: Option<String>,
    cors_origins: Vec<String>,
    mut shutdown_rx: tokio::sync::oneshot::Receiver<()>,
) -> anyhow::Result<()> {
    let state = AppState::new(manager, api_key, cors_origins);

    let cors_layer = if state.cors_origins.is_empty() {
        CorsLayer::permissive()
    } else {
        let origins: Vec<HeaderValue> = state
            .cors_origins
            .iter()
            .filter_map(|o: &String| o.parse().ok())
            .collect();
        CorsLayer::new()
            .allow_origin(origins)
            .allow_methods(tower_http::cors::Any)
            .allow_headers(tower_http::cors::Any)
    };

    let protected = api::routes()
        .merge(ws::routes())
        .layer(middleware::from_fn_with_state(
            state.clone(),
            api::auth_middleware,
        ));

    let app = Router::new()
        .merge(metrics::routes())
        .merge(protected)
        .layer(cors_layer)
        .fallback(|req| std::future::ready(embedded_static_handler(req)))
        .with_state(state);

    let listener = tokio::net::TcpListener::bind(addr).await?;
    tracing::info!("Web 服务已启动: http://{addr}");

    axum::serve(listener, app)
        .with_graceful_shutdown(async move {
            let _ = (&mut shutdown_rx).await;
        })
        .await?;

    Ok(())
}
