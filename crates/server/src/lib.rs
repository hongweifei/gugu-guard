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
use std::time::Duration;

use axum::body::Body;
use axum::extract::Request;
use axum::http::{header, HeaderValue, StatusCode};
use axum::middleware;
use axum::response::{Html, IntoResponse, Response};
use axum::Router;
use socket2::{Domain, Protocol, Socket, Type};
use state::AppState;
use tower_http::cors::CorsLayer;

/// 端口被占用时等待释放的总时长上限。
const BIND_WAIT: Duration = Duration::from_secs(30);
/// 每次重试之间的间隔。
const BIND_STEP: Duration = Duration::from_millis(500);

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
    state: AppState,
    mut shutdown_rx: tokio::sync::oneshot::Receiver<()>,
) -> anyhow::Result<()> {
    // 库调用方也不能绕过这条门槛：非回环监听 + 无 Key + 终端开启时不启动。
    if state.terminal_enabled {
        terminal::check_exposure(addr.ip(), state.api_key.as_deref())
            .map_err(anyhow::Error::msg)?;
    }

    let poller_target = std::sync::Arc::clone(&state.nodes);

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

    let listener = bind_with_retry(addr).await?;
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

/// 绑定监听端口。
///
/// 自更新替换完成后新进程紧接着启动：旧进程刚退出，Windows 的套接字表里仍挂着它的
/// `LISTEN` 条目与 `TIME_WAIT` 连接，此时 `bind` 返回 `WSAEADDRINUSE`，而端口已经无人接管。
/// 因此先判断占用者是否还活着——活着就如实报错（端口确实被别人占住），只剩残影或
/// `TIME_WAIT` 就改以可复用地址绑定。整段最多等 [`BIND_WAIT`]。
async fn bind_with_retry(addr: SocketAddr) -> std::io::Result<tokio::net::TcpListener> {
    let start = tokio::time::Instant::now();
    let mut warned = false;
    loop {
        match bind_socket(addr, false) {
            Ok(listener) => return tokio::net::TcpListener::from_std(listener),
            Err(e) if e.kind() == std::io::ErrorKind::AddrInUse => {
                if let Some(busy) = live_owner(addr.port()) {
                    return Err(busy);
                }
                if !warned {
                    tracing::warn!("端口 {addr} 上只剩上一代的残留连接，改以可复用地址绑定");
                    warned = true;
                }
                if let Ok(listener) = bind_socket(addr, true) {
                    return tokio::net::TcpListener::from_std(listener);
                }
                if start.elapsed() >= BIND_WAIT {
                    return Err(e);
                }
                tokio::time::sleep(BIND_STEP).await;
            }
            Err(e) => return Err(e),
        }
    }
}

/// 端口被另一个活着的进程监听时返回对应错误。
///
/// 套接字表里的占用者可能已经退出（进程号还在表里盘桓一秒），也可能就是本进程
/// 自己的上一轮，这两种情况都返回 `None`，交给调用方用可复用地址接管。
fn live_owner(port: u16) -> Option<std::io::Error> {
    let owner = gugu_core::sockets::listen_on(port)?;
    let pid = owner.pid?;
    let alive = pid != std::process::id() && gugu_core::sockets::process_name(pid).is_some();
    alive.then(|| std::io::Error::new(std::io::ErrorKind::AddrInUse, owner.describe()))
}

/// 建一个监听套接字；`reuse` 决定是否允许接管 `TIME_WAIT` 残留。
///
/// 建好后立刻标记为不可继承：守护进程会启动替身与子进程，套接字一旦被它们继承，
/// 旧进程退出后端口仍被占着，新一代就再也绑不上。
fn bind_socket(addr: SocketAddr, reuse: bool) -> std::io::Result<std::net::TcpListener> {
    let socket = Socket::new(Domain::for_address(addr), Type::STREAM, Some(Protocol::TCP))?;
    // Unix 上复用地址是重启的常态，与标准库默认行为一致；Windows 上只在接管残留时才开，
    // 免得把「端口真被别的进程占着」也一并放过。
    #[cfg(unix)]
    socket.set_reuse_address(true)?;
    #[cfg(windows)]
    if reuse {
        socket.set_reuse_address(true)?;
    }
    socket.set_nonblocking(true)?;
    socket.bind(&addr.into())?;
    socket.listen(1024)?;
    let listener: std::net::TcpListener = socket.into();
    hide_from_children(&listener)?;
    Ok(listener)
}

/// 让监听套接字不被后续子进程继承。
#[cfg(windows)]
fn hide_from_children(listener: &std::net::TcpListener) -> std::io::Result<()> {
    use std::os::windows::io::AsRawSocket;

    use windows_sys::Win32::Foundation::{SetHandleInformation, HANDLE, HANDLE_FLAG_INHERIT};

    let handle = listener.as_raw_socket() as HANDLE;
    if unsafe { SetHandleInformation(handle, HANDLE_FLAG_INHERIT, 0) } == 0 {
        return Err(std::io::Error::last_os_error());
    }
    Ok(())
}

/// Unix 的套接字默认带 `FD_CLOEXEC`，子进程不会继承。
#[cfg(unix)]
fn hide_from_children(_listener: &std::net::TcpListener) -> std::io::Result<()> {
    Ok(())
}
