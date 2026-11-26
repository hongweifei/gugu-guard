//! 审计中间件与查询接口的集成测试：动作映射、认证失败留痕、过滤条件。

use std::net::SocketAddr;
use std::sync::Arc;

use axum::body::{to_bytes, Body};
use axum::extract::ConnectInfo;
use axum::http::{Request, StatusCode};
use axum::middleware;
use axum::Router;
use gugu_core::audit::{AuditEntry, AuditStore};
use gugu_core::config::AppConfig;
use gugu_server::state::AppState;
use serde::Deserialize;
use tower::ServiceExt;

struct TempDir {
    path: std::path::PathBuf,
}

impl TempDir {
    fn path(&self) -> &std::path::Path {
        &self.path
    }
}

impl Drop for TempDir {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.path);
    }
}

fn tempdir() -> TempDir {
    use std::sync::atomic::{AtomicU64, Ordering};
    static SEQ: AtomicU64 = AtomicU64::new(0);
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_nanos())
        .unwrap_or(0);
    let seq = SEQ.fetch_add(1, Ordering::Relaxed);
    let path = std::env::temp_dir().join(format!(
        "gugu-audit-test-{}-{nanos}-{seq}",
        std::process::id()
    ));
    std::fs::create_dir_all(&path).expect("创建测试临时目录失败");
    TempDir { path }
}

/// 与生产同构的路由：认证层在内，审计层在外。
fn build_app(dir: &std::path::Path, api_key: Option<String>) -> (Router, Arc<AuditStore>) {
    let config = AppConfig::default();
    let shared = gugu_core::manager::start(&config, None);
    let audit = Arc::new(AuditStore::new(dir.to_path_buf()));
    let state = AppState::new(shared, api_key, Vec::new()).with_audit(Arc::clone(&audit));

    let protected = gugu_server::api::routes()
        .merge(gugu_server::ws::routes())
        .merge(gugu_server::metrics::routes())
        .layer(middleware::from_fn_with_state(
            state.clone(),
            gugu_server::api::auth_middleware,
        ))
        .layer(middleware::from_fn_with_state(
            state.clone(),
            gugu_server::audit::audit_middleware,
        ));

    let app = Router::new().merge(protected).with_state(state);
    (app, audit)
}

fn write_request(method: &str, uri: &str) -> Request<Body> {
    Request::builder()
        .method(method)
        .uri(uri)
        .extension(ConnectInfo(SocketAddr::from(([10, 0, 0, 1], 5678))))
        .header("user-agent", "gugu-cli/0.1.0")
        .header("content-type", "application/json")
        .body(Body::from("{}"))
        .expect("构造请求失败")
}

async fn send(app: Router, req: Request<Body>) -> (StatusCode, axum::body::Bytes) {
    let response = app.oneshot(req).await.expect("请求处理失败");
    let status = response.status();
    let bytes = to_bytes(response.into_body(), 64 * 1024)
        .await
        .expect("读取响应体失败");
    (status, bytes)
}

#[derive(Deserialize)]
struct AuditResponse {
    entries: Vec<AuditEntry>,
    retention_days: i64,
    file: Option<String>,
}

#[tokio::test]
async fn records_a_failed_start_with_client_and_detail() {
    let dir = tempdir();
    let (app, _audit) = build_app(dir.path(), None);

    let (status, _) = send(app, write_request("POST", "/api/v1/processes/ghost/start")).await;
    assert_eq!(status, StatusCode::BAD_REQUEST);

    // 换一个新的 store 实例读同一份落盘文件，验证重启后仍可查询
    let (audit_app, _) = build_app(dir.path(), None);
    let (status, body) = send(
        audit_app,
        Request::builder()
            .uri("/api/v1/audit?limit=10")
            .body(Body::empty())
            .expect("构造查询请求失败"),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    let body: AuditResponse = serde_json::from_slice(&body).expect("解析审计响应失败");
    assert_eq!(body.retention_days, 90);
    assert!(body.file.is_some(), "落盘模式下应返回文件路径");
    assert_eq!(body.entries.len(), 1);
    let entry = &body.entries[0];
    assert_eq!(entry.action, "process.start");
    assert_eq!(entry.target.as_deref(), Some("ghost"));
    assert_eq!(entry.status, 400);
    assert!(!entry.ok);
    assert_eq!(entry.client, "10.0.0.1:5678");
    assert_eq!(entry.user_agent.as_deref(), Some("gugu-cli/0.1.0"));
    assert!(entry
        .detail
        .as_deref()
        .is_some_and(|detail| detail.contains("ghost")));
}

#[tokio::test]
async fn records_denied_auth_and_keyed_requests() {
    let dir = tempdir();
    let (app, audit) = build_app(dir.path(), Some("secret".to_string()));

    let (status, _) = send(app.clone(), write_request("POST", "/api/v1/unknown")).await;
    assert_eq!(status, StatusCode::UNAUTHORIZED);

    let mut authorized = write_request("POST", "/api/v1/unknown");
    authorized
        .headers_mut()
        .insert("authorization", "Bearer secret".parse().unwrap());
    let (status, _) = send(app, authorized).await;
    assert_eq!(status, StatusCode::NOT_FOUND, "未知写接口也应留痕");

    let entries = audit.recent(10);
    assert_eq!(entries.len(), 2);
    assert_eq!(entries[0].auth, gugu_core::audit::AuthKind::Key);
    assert_eq!(entries[0].action, "http.write");
    assert_eq!(entries[1].auth, gugu_core::audit::AuthKind::Denied);
    assert_eq!(entries[1].status, 401);
}

#[tokio::test]
async fn read_requests_are_not_audited() {
    let dir = tempdir();
    let (app, audit) = build_app(dir.path(), None);

    for uri in [
        "/api/v1/processes",
        "/api/v1/audit?limit=5",
        "/api/v1/stats",
    ] {
        let (status, _) = send(
            app.clone(),
            Request::builder()
                .uri(uri)
                .body(Body::empty())
                .expect("构造请求失败"),
        )
        .await;
        assert_eq!(status, StatusCode::OK, "{uri}");
    }
    assert!(audit.recent(10).is_empty(), "GET 不进入审计");
}

#[tokio::test]
async fn audit_query_filters_by_action_result_and_limit() {
    let dir = tempdir();
    let (app, audit) = build_app(dir.path(), None);

    send(
        app.clone(),
        write_request("POST", "/api/v1/processes/a/start"),
    )
    .await;
    send(
        app.clone(),
        write_request("POST", "/api/v1/processes/b/stop"),
    )
    .await;
    send(app.clone(), write_request("DELETE", "/api/v1/processes/c")).await;

    let total = audit.recent(10).len();
    assert_eq!(total, 3);

    let (status, body) = send(
        app,
        Request::builder()
            .uri("/api/v1/audit?action=process.stop&ok=false&limit=1")
            .body(Body::empty())
            .expect("构造查询请求失败"),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    let body: AuditResponse = serde_json::from_slice(&body).expect("解析审计响应失败");
    assert_eq!(body.entries.len(), 1);
    assert_eq!(body.entries[0].action, "process.stop");
    assert_eq!(body.entries[0].target.as_deref(), Some("b"));
    assert!(!body.entries[0].ok);
}
