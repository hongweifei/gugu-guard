//! 多节点集成测试：节点表、HTTP 代理（含认证换发与请求体转发）与 WebSocket 中继。
//!
//! 从节点用一个最小的假服务模拟：它只认自己的 Key，并回固定版本与进程列表，
//! 这样中心节点是否把 Key 换成从节点的、是否带上了方法/路径/请求体都能直接观察到。

use std::collections::HashMap;
use std::net::SocketAddr;
use std::sync::Arc;

use axum::body::{to_bytes, Body};
use axum::extract::ws::{Message, WebSocketUpgrade};
use axum::extract::{Path, Query};
use axum::http::{HeaderMap, Method, Request, StatusCode};
use axum::middleware;
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post};
use axum::{Json, Router};
use futures::StreamExt;
use gugu_core::config::{AppConfig, NodeConfig};
use gugu_server::nodes::NodeRegistry;
use gugu_server::state::AppState;
use serde_json::{json, Value};
use tower::ServiceExt;

const NODE_KEY: &str = "edge-secret";
const HUB_KEY: &str = "hub-secret";

fn node_url(addr: SocketAddr) -> String {
    format!("http://{addr}")
}

// ── 假从节点 ──────────────────────────────────────────────

fn upstream_ok(headers: &HeaderMap) -> bool {
    headers
        .get("authorization")
        .and_then(|value| value.to_str().ok())
        == Some("Bearer edge-secret")
}

async fn upstream_info(headers: HeaderMap) -> Response {
    if !upstream_ok(&headers) {
        return StatusCode::UNAUTHORIZED.into_response();
    }
    Json(json!({
        "name": "edge",
        "hostname": "EDGE-PC",
        "platform": "linux",
        "version": "9.9.9",
        "uptime_secs": 42,
        "processes": 3,
        "running": 2,
        "terminal": false,
        "nodes": 0
    }))
    .into_response()
}

async fn upstream_processes(headers: HeaderMap) -> Response {
    if !upstream_ok(&headers) {
        return StatusCode::UNAUTHORIZED.into_response();
    }
    Json(json!([{ "name": "remote-app", "status": "running" }])).into_response()
}

/// 300 KB 的响应体：代理必须边收边转，不能截断也不能要求整段读进内存。
async fn upstream_large(headers: HeaderMap) -> Response {
    if !upstream_ok(&headers) {
        return StatusCode::UNAUTHORIZED.into_response();
    }
    "x".repeat(300 * 1024).into_response()
}

async fn upstream_start(headers: HeaderMap, Path(name): Path<String>, body: String) -> Response {
    if !upstream_ok(&headers) {
        return StatusCode::UNAUTHORIZED.into_response();
    }
    Json(json!({ "message": format!("started {name}"), "echo": body })).into_response()
}

async fn upstream_ws(
    Query(params): Query<HashMap<String, String>>,
    ws: WebSocketUpgrade,
) -> Response {
    if params.get("token").map(String::as_str) != Some(NODE_KEY) {
        return StatusCode::UNAUTHORIZED.into_response();
    }
    ws.on_upgrade(|mut socket| async move {
        let hello = json!({ "type": "status", "processes": [], "node": "edge" }).to_string();
        let _ = socket.send(Message::Text(hello.into())).await;
        while let Some(Ok(_)) = socket.recv().await {}
    })
}

async fn spawn_upstream() -> SocketAddr {
    let app = Router::new()
        .route("/api/v1/node", get(upstream_info))
        .route("/api/v1/processes", get(upstream_processes))
        .route("/api/v1/large", get(upstream_large))
        .route("/api/v1/processes/{name}/start", post(upstream_start))
        .route("/api/v1/ws", get(upstream_ws));
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
        .await
        .expect("绑定假从节点端口失败");
    let addr = listener.local_addr().expect("读取假从节点地址失败");
    tokio::spawn(async move {
        let _ = axum::serve(listener, app).await;
    });
    addr
}

// ── 中心节点 ──────────────────────────────────────────────

fn build_hub(node_addr: SocketAddr) -> (Router, Arc<NodeRegistry>) {
    let manager = gugu_core::manager::start(&AppConfig::default(), None);
    let registry = Arc::new(NodeRegistry::new(
        &[NodeConfig {
            name: "edge".into(),
            label: Some("边缘机".into()),
            url: node_url(node_addr),
            api_key: Some(NODE_KEY.into()),
        }],
        Some("中心机".into()),
    ));
    let state =
        AppState::new(manager, Some(HUB_KEY.into()), Vec::new()).with_nodes(Arc::clone(&registry));
    let app = gugu_server::nodes::routes()
        .merge(gugu_server::ws::routes())
        .layer(middleware::from_fn_with_state(
            state.clone(),
            gugu_server::api::auth_middleware,
        ))
        .with_state(state);
    (app, registry)
}

async fn call(app: Router, method: Method, uri: &str, body: Option<Value>) -> (StatusCode, Value) {
    let mut builder = Request::builder()
        .method(method)
        .uri(uri)
        .header("Authorization", format!("Bearer {HUB_KEY}"));
    let request = match body {
        Some(value) => {
            builder = builder.header("Content-Type", "application/json");
            builder
                .body(Body::from(value.to_string()))
                .expect("构造请求失败")
        }
        None => builder.body(Body::empty()).expect("构造请求失败"),
    };
    let response = app.oneshot(request).await.expect("请求处理失败");
    let status = response.status();
    let bytes = to_bytes(response.into_body(), 4 * 1024 * 1024)
        .await
        .expect("读取响应体失败");
    let json = serde_json::from_slice(&bytes).unwrap_or(Value::Null);
    (status, json)
}

#[tokio::test]
async fn lists_local_and_remote_nodes_with_health() {
    let addr = spawn_upstream().await;
    let (app, registry) = build_hub(addr);

    // 首次探测前：从节点存在但状态未知。
    let (status, body) = call(app.clone(), Method::GET, "/api/v1/nodes", None).await;
    assert_eq!(status, StatusCode::OK);
    let nodes = body["nodes"].as_array().expect("应有节点表");
    assert_eq!(nodes.len(), 2);
    assert_eq!(nodes[0]["id"], "local");
    assert_eq!(nodes[0]["name"], "中心机");
    assert_eq!(nodes[0]["local"], true);
    assert_eq!(nodes[1]["id"], "edge");
    assert_eq!(nodes[1]["name"], "边缘机");
    assert_eq!(nodes[1]["online"], false);

    registry.refresh().await;

    let (_, body) = call(app, Method::GET, "/api/v1/nodes", None).await;
    let edge = &body["nodes"][1];
    assert_eq!(edge["online"], true);
    assert_eq!(edge["version"], "9.9.9");
    assert_eq!(edge["hostname"], "EDGE-PC");
    assert_eq!(edge["processes"], 3);
    assert_eq!(edge["running"], 2);
    assert!(edge["latency_ms"].is_u64());
}

#[tokio::test]
async fn proxies_reads_and_writes_to_the_node() {
    let addr = spawn_upstream().await;
    let (app, _) = build_hub(addr);

    let (status, body) = call(
        app.clone(),
        Method::GET,
        "/api/v1/nodes/edge/processes",
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body[0]["name"], "remote-app");

    let (status, body) = call(
        app,
        Method::POST,
        "/api/v1/nodes/edge/processes/remote-app/start",
        Some(json!({ "reason": "test" })),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["message"], "started remote-app");
    assert_eq!(body["echo"], r#"{"reason":"test"}"#, "请求体应原样转发");
}

#[tokio::test]
async fn proxies_large_responses_without_truncation() {
    let addr = spawn_upstream().await;
    let (app, _) = build_hub(addr);

    let request = Request::builder()
        .uri("/api/v1/nodes/edge/large")
        .header("Authorization", format!("Bearer {HUB_KEY}"))
        .body(Body::empty())
        .expect("构造请求失败");
    let response = app.oneshot(request).await.expect("请求处理失败");
    assert_eq!(response.status(), StatusCode::OK);
    let bytes = to_bytes(response.into_body(), 4 * 1024 * 1024)
        .await
        .expect("读取代理响应失败");
    assert_eq!(bytes.len(), 300 * 1024, "代理应完整转交大响应体");
}

#[tokio::test]
async fn rejects_unknown_and_local_proxy_targets() {
    let addr = spawn_upstream().await;
    let (app, _) = build_hub(addr);

    let (status, body) = call(
        app.clone(),
        Method::GET,
        "/api/v1/nodes/nope/processes",
        None,
    )
    .await;
    assert_eq!(status, StatusCode::NOT_FOUND);
    assert!(body["error"].as_str().is_some_and(|e| e.contains("nope")));

    let (status, _) = call(
        app.clone(),
        Method::GET,
        "/api/v1/nodes/local/processes",
        None,
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);

    // 百分号编码的 `..` 不会被 URL 归一化，服务端按未知节点拒绝，请求留在代理路径上。
    let (status, body) = call(
        app.clone(),
        Method::GET,
        "/api/v1/nodes/%2E%2E/processes",
        None,
    )
    .await;
    assert_eq!(status, StatusCode::NOT_FOUND);
    assert_eq!(body["error"], "未知节点: ..");

    // 没带中心节点 Key：代理入口本身也受认证保护。
    let request = Request::builder()
        .uri("/api/v1/nodes/edge/processes")
        .body(Body::empty())
        .expect("构造请求失败");
    let response = app.oneshot(request).await.expect("请求处理失败");
    assert_eq!(response.status(), StatusCode::UNAUTHORIZED);
}

#[tokio::test]
async fn relays_websocket_from_the_selected_node() {
    let upstream = spawn_upstream().await;
    let (app, _) = build_hub(upstream);
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
        .await
        .expect("绑定中心节点端口失败");
    let hub_addr = listener.local_addr().expect("读取中心节点地址失败");
    tokio::spawn(async move {
        let _ = axum::serve(listener, app).await;
    });

    let url = format!("ws://{hub_addr}/api/v1/ws?token={HUB_KEY}&node=edge");
    let (mut socket, _) = tokio_tungstenite::connect_async(url)
        .await
        .expect("连接中心节点 WebSocket 失败");
    let message = tokio::time::timeout(std::time::Duration::from_secs(5), socket.next())
        .await
        .expect("等待中继消息超时")
        .expect("中继连接提前结束")
        .expect("读取中继消息失败");
    let text = message.into_text().expect("应为文本消息");
    let value: Value = serde_json::from_str(&text).expect("中继消息应为 JSON");
    assert_eq!(value["type"], "status");
    assert_eq!(value["node"], "edge");
}
