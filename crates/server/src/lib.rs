pub mod api;
pub mod assets;
pub mod audit;
pub mod metrics;
pub mod nodes;
mod peer;
pub mod state;
pub mod terminal;
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
use gugu_core::config::NodeConfig;
use gugu_core::manager::SharedManager;
use state::AppState;
use tower_http::cors::CorsLayer;

/// Web 服务的可选配置：认证、跨域、审计落盘位置、内置终端与从节点表。
#[derive(Debug, Clone, Default)]
pub struct ServerOptions {
    pub api_key: Option<String>,
    pub cors_origins: Vec<String>,
    /// 审计文件目录；None 时审计只留内存。
    pub audit_dir: Option<PathBuf>,
    /// 是否开放 WebUI 内置终端；默认关闭。
    pub terminal: bool,
    /// 终端会话的工作目录；None 时用当前目录。
    pub terminal_cwd: Option<PathBuf>,
    /// 由本节点纳管的从节点；空表示只有本机。
    pub nodes: Vec<NodeConfig>,
    /// 本机在节点表里的显示名；None 时用主机名。
    pub node_name: Option<String>,
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
    // 库调用方也不能绕过这条门槛：非回环监听 + 无 Key + 终端开启时不启动。
    if options.terminal {
        terminal::check_exposure(addr.ip(), options.api_key.as_deref())
            .map_err(anyhow::Error::msg)?;
    }
    // 节点表配错（重名、地址不合法、对外地址没 Key）同样直接拒绝启动。
    let node_problems = gugu_core::config::validate_nodes(&options.nodes);
    if !node_problems.is_empty() {
        return Err(anyhow::anyhow!(
            "节点配置有误: {}",
            node_problems.join("；")
        ));
    }

    let audit = options
        .audit_dir
        .map_or_else(AuditStore::disabled, AuditStore::new);
    let nodes = Arc::new(nodes::NodeRegistry::new(
        &options.nodes,
        options.node_name.clone(),
    ));
    let poller_target = Arc::clone(&nodes);
    let state = AppState::new(manager, options.api_key, options.cors_origins)
        .with_audit(Arc::new(audit))
        .with_nodes(nodes)
        .with_shutdown(api_shutdown_tx)
        .with_terminal(options.terminal, options.terminal_cwd);

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
        .merge(terminal::routes())
        .merge(nodes::routes())
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

    // 探测任务在端口绑定成功之后才起：绑定失败会直接返回，不留一个游离任务。
    // 没有从节点时任务立即结束。
    let poller = poller_target.spawn_poller();

    let served = axum::serve(
        listener,
        app.into_make_service_with_connect_info::<SocketAddr>(),
    )
    .with_graceful_shutdown(async move {
        let _ = (&mut shutdown_rx).await;
    })
    .await;
    poller.abort();
    served?;

    Ok(())
}
