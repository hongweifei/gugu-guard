//! 终端开关与信息接口的集成测试：关闭时明确返回 enabled=false，开启时给出 shell 与限制。

use std::path::PathBuf;
use std::sync::Arc;

use axum::body::{to_bytes, Body};
use axum::http::{Request, StatusCode};
use axum::middleware;
use axum::Router;
use gugu_core::audit::AuditStore;
use gugu_core::config::AppConfig;
use gugu_server::state::AppState;
use serde_json::Value;
use tower::ServiceExt;

fn build_app(terminal: bool, api_key: Option<String>) -> Router {
    let config = AppConfig::default();
    let shared = gugu_core::manager::start(&config, None);
    let state = AppState::new(shared)
        .with_api_key(api_key)
        .with_audit(Arc::new(AuditStore::disabled()))
        .with_terminal(terminal, Some(PathBuf::from(".")));
    let protected = gugu_server::terminal::routes().layer(middleware::from_fn_with_state(
        state.clone(),
        gugu_server::api::auth_middleware,
    ));
    Router::new().merge(protected).with_state(state)
}

async fn get(app: Router, uri: &str, token: Option<&str>) -> (StatusCode, Value) {
    let token = token.map_or_else(String::new, |token| format!("?token={token}"));
    let request = Request::builder()
        .uri(format!("{uri}{token}"))
        .body(Body::empty())
        .expect("构造请求失败");
    let response = app.oneshot(request).await.expect("请求处理失败");
    let status = response.status();
    let bytes = to_bytes(response.into_body(), 16 * 1024)
        .await
        .expect("读取响应体失败");
    let json = serde_json::from_slice(&bytes).unwrap_or(Value::Null);
    (status, json)
}

#[tokio::test]
async fn disabled_terminal_reports_false() {
    let (status, body) = get(build_app(false, None), "/api/v1/terminal", None).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["enabled"], false);
}

#[tokio::test]
async fn enabled_terminal_reports_shell_and_limits() {
    let (status, body) = get(build_app(true, None), "/api/v1/terminal", None).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["enabled"], true);
    assert!(body["shell"]
        .as_str()
        .is_some_and(|shell| !shell.is_empty()));
    assert_eq!(body["max_sessions"], gugu_server::terminal::MAX_SESSIONS);
    assert_eq!(
        body["idle_timeout_secs"],
        gugu_server::terminal::IDLE_TIMEOUT.as_secs()
    );
}

#[tokio::test]
async fn terminal_info_requires_api_key_when_configured() {
    let app = build_app(true, Some("secret".into()));
    let (status, _) = get(app.clone(), "/api/v1/terminal", None).await;
    assert_eq!(status, StatusCode::UNAUTHORIZED);

    let (status, body) = get(app, "/api/v1/terminal", Some("secret")).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["enabled"], true);
}
