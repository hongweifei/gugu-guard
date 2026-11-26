pub mod api;
pub mod assets;
pub mod audit;
pub mod metrics;
pub mod state;
pub mod ws;

use std::net::SocketAddr;
use std::path::PathBuf;
use std::sync::Arc;

use axum::body::Body;
use axum::extract::Request;
use axum::http::{header, HeaderValue, StatusCode};
use axum::middleware;
use axum::response::{Html, IntoResponse, Response};
use axum::Router;
use gugu_core::audit::AuditStore;
use gugu_core::manager::SharedManager;
use state::AppState;
use tower_http::cors::CorsLayer;

/// Web 服务的可选配置：认证、跨域与审计落盘位置。
#[derive(Debug, Clone, Default)]
pub struct ServerOptions {
    pub api_key: Option<String>,
    pub cors_origins: Vec<String>,
    /// 审计文件目录；None 时审计只留内存。
    pub audit_dir: Option<PathBuf>,
}

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
    options: ServerOptions,
    mut shutdown_rx: tokio::sync::oneshot::Receiver<()>,
    api_shutdown_tx: Option<tokio::sync::mpsc::Sender<()>>,
) -> anyhow::Result<()> {
    let audit = options
        .audit_dir
        .map_or_else(AuditStore::disabled, AuditStore::new);
    let state = AppState::new(manager, options.api_key, options.cors_origins)
        .with_audit(Arc::new(audit))
        .with_shutdown(api_shutdown_tx);

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

    // 审计层在最外层：认证失败的请求也会被记下来。
    let protected = api::routes()
        .merge(ws::routes())
        .merge(metrics::routes())
        .layer(middleware::from_fn_with_state(
            state.clone(),
            api::auth_middleware,
        ))
        .layer(middleware::from_fn_with_state(
            state.clone(),
            audit::audit_middleware,
        ));

    let app = Router::new()
        .merge(protected)
        .layer(cors_layer)
        .fallback(|req| std::future::ready(embedded_static_handler(req)))
        .with_state(state);

    let listener = tokio::net::TcpListener::bind(addr).await?;
    tracing::info!("Web 服务已启动: http://{addr}");

    axum::serve(
        listener,
        app.into_make_service_with_connect_info::<SocketAddr>(),
    )
    .with_graceful_shutdown(async move {
        let _ = (&mut shutdown_rx).await;
    })
    .await?;

    Ok(())
}
