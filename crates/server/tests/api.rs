use axum::body::Body;
use axum::http::{Request, StatusCode};
use axum::middleware;
use gugu_core::config::AppConfig;
use gugu_core::ProcessManager;
use gugu_server::state::AppState;
use tower::ServiceExt;

fn make_app() -> axum::Router {
    make_app_with_key(None)
}

fn make_app_with_key(api_key: Option<String>) -> axum::Router {
    let config = AppConfig::default();
    let manager = ProcessManager::new(&config, None);
    let shared = manager.shared();
    let state = AppState::new(shared, api_key, Vec::new());

    let cors_layer = tower_http::cors::CorsLayer::permissive();

    axum::Router::new()
        .merge(gugu_server::api::routes())
        .layer(middleware::from_fn_with_state(
            state.clone(),
            gugu_server::api::auth_middleware,
        ))
        .layer(cors_layer)
        .with_state(state)
}

#[tokio::test]
async fn list_processes_empty() {
    let app = make_app();

    let resp = app
        .oneshot(
            Request::builder()
                .uri("/api/v1/processes")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(resp.status(), StatusCode::OK);
}

#[tokio::test]
async fn get_process_not_found() {
    let app = make_app();

    let resp = app
        .oneshot(
            Request::builder()
                .uri("/api/v1/processes/nonexistent")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(resp.status(), StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn get_stats_empty() {
    let app = make_app();

    let resp = app
        .oneshot(
            Request::builder()
                .uri("/api/v1/stats")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(resp.status(), StatusCode::OK);
}

#[tokio::test]
async fn create_process() {
    let dir = tempfile::tempdir().unwrap();
    let config_path = dir.path().join("gugu.toml");
    let config = AppConfig::default();
    let manager = ProcessManager::new(&config, Some(config_path));
    let shared = manager.shared();
    let state = AppState::new(shared, None, Vec::new());

    let cors_layer = tower_http::cors::CorsLayer::permissive();
    let app = axum::Router::new()
        .merge(gugu_server::api::routes())
        .layer(middleware::from_fn_with_state(
            state.clone(),
            gugu_server::api::auth_middleware,
        ))
        .layer(cors_layer)
        .with_state(state);

    let body = serde_json::json!({
        "command": "echo hello",
        "auto_start": false,
        "start_now": false,
    });

    let resp = app
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/api/v1/processes/test-svc")
                .header("content-type", "application/json")
                .body(Body::from(serde_json::to_string(&body).unwrap()))
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(resp.status(), StatusCode::CREATED);
}

#[tokio::test]
async fn auth_rejects_without_key() {
    let app = make_app_with_key(Some("secret-key".to_string()));

    let resp = app
        .oneshot(
            Request::builder()
                .uri("/api/v1/processes")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(resp.status(), StatusCode::UNAUTHORIZED);
}

#[tokio::test]
async fn auth_accepts_valid_bearer() {
    let app = make_app_with_key(Some("secret-key".to_string()));

    let resp = app
        .oneshot(
            Request::builder()
                .uri("/api/v1/processes")
                .header("Authorization", "Bearer secret-key")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(resp.status(), StatusCode::OK);
}

#[tokio::test]
async fn auth_rejects_invalid_bearer() {
    let app = make_app_with_key(Some("secret-key".to_string()));

    let resp = app
        .oneshot(
            Request::builder()
                .uri("/api/v1/processes")
                .header("Authorization", "Bearer wrong-key")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(resp.status(), StatusCode::UNAUTHORIZED);
}

#[tokio::test]
async fn auth_accepts_valid_token_query() {
    let app = make_app_with_key(Some("secret-key".to_string()));

    let resp = app
        .oneshot(
            Request::builder()
                .uri("/api/v1/processes?token=secret-key")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(resp.status(), StatusCode::OK);
}

#[tokio::test]
async fn start_process_not_found() {
    let app = make_app();

    let resp = app
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/api/v1/processes/nonexistent/start")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(resp.status(), StatusCode::BAD_REQUEST);
}

#[tokio::test]
async fn stop_process_not_found() {
    let app = make_app();

    let resp = app
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/api/v1/processes/nonexistent/stop")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(resp.status(), StatusCode::BAD_REQUEST);
}

#[tokio::test]
async fn delete_process_not_found() {
    let app = make_app();

    let resp = app
        .oneshot(
            Request::builder()
                .method("DELETE")
                .uri("/api/v1/processes/nonexistent")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(resp.status(), StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn get_logs_not_found() {
    let app = make_app();

    let resp = app
        .oneshot(
            Request::builder()
                .uri("/api/v1/processes/nonexistent/logs?lines=50")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(resp.status(), StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn health_check_not_configured() {
    let dir = tempfile::tempdir().unwrap();
    let config_path = dir.path().join("gugu.toml");
    let config = AppConfig::default();
    let manager = ProcessManager::new(&config, Some(config_path));
    let shared = manager.shared();
    let state = AppState::new(shared, None, Vec::new());

    let cors_layer = tower_http::cors::CorsLayer::permissive();
    let app = axum::Router::new()
        .merge(gugu_server::api::routes())
        .layer(middleware::from_fn_with_state(
            state.clone(),
            gugu_server::api::auth_middleware,
        ))
        .layer(cors_layer)
        .with_state(state);

    // 先创建进程
    let body = serde_json::json!({
        "command": "echo hello",
        "auto_start": false,
        "start_now": false,
    });
    app.clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/api/v1/processes/test-svc")
                .header("content-type", "application/json")
                .body(Body::from(serde_json::to_string(&body).unwrap()))
                .unwrap(),
        )
        .await
        .unwrap();

    let resp = app
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/api/v1/processes/test-svc/health")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(resp.status(), StatusCode::BAD_REQUEST);
}
