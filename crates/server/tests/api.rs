use axum::body::Body;
use axum::http::{Request, StatusCode};
use axum::middleware;
use gugu_core::config::AppConfig;
use gugu_core::manager::SharedManager;
use gugu_server::state::AppState;
use tower::ServiceExt;

// ── 测试辅助 ──────────────────────────────────────────────

fn make_app() -> axum::Router {
    make_app_with_key(None)
}

fn make_app_with_key(api_key: Option<String>) -> axum::Router {
    let config = AppConfig::default();
    let shared = gugu_core::manager::start(&config, None);
    build_router(shared, api_key)
}

fn make_app_with_dir(dir: &std::path::Path) -> axum::Router {
    make_app_with_dir_and_key(dir, None)
}

fn make_app_with_dir_and_key(dir: &std::path::Path, api_key: Option<String>) -> axum::Router {
    let config = AppConfig::default();
    let config_path = dir.join("gugu.toml");
    let shared = gugu_core::manager::start(&config, Some(config_path));
    build_router(shared, api_key)
}

fn spawn_app_with_shared(dir: &std::path::Path) -> (axum::Router, SharedManager) {
    let config = AppConfig::default();
    let config_path = dir.join("gugu.toml");
    let shared = gugu_core::manager::start(&config, Some(config_path));
    let app = build_router(shared.clone(), None);
    (app, shared)
}

fn build_router(shared: SharedManager, api_key: Option<String>) -> axum::Router {
    let state = AppState::new(shared, api_key, Vec::new());
    let cors_layer = tower_http::cors::CorsLayer::permissive();

    let protected = gugu_server::api::routes()
        .merge(gugu_server::ws::routes())
        .merge(gugu_server::metrics::routes())
        .layer(middleware::from_fn_with_state(
            state.clone(),
            gugu_server::api::auth_middleware,
        ));

    axum::Router::new()
        .merge(protected)
        .layer(cors_layer)
        .with_state(state)
}

fn create_process_body(command: &str, start_now: bool) -> serde_json::Value {
    create_process_body_with_group(command, start_now, None)
}

fn create_process_body_with_group(command: &str, start_now: bool, group: Option<&str>) -> serde_json::Value {
    let mut body = serde_json::json!({
        "command": command,
        "auto_start": false,
        "start_now": start_now,
    });
    if let Some(g) = group {
        body["group"] = serde_json::Value::String(g.to_string());
    }
    body
}

async fn send_get(app: axum::Router, uri: &str) -> axum::http::Response<Body> {
    app.oneshot(
        Request::builder()
            .uri(uri)
            .body(Body::empty())
            .unwrap(),
    )
    .await
    .unwrap()
}

async fn send_post(app: axum::Router, uri: &str) -> axum::http::Response<Body> {
    app.oneshot(
        Request::builder()
            .method("POST")
            .uri(uri)
            .body(Body::empty())
            .unwrap(),
    )
    .await
    .unwrap()
}

async fn send_delete(app: axum::Router, uri: &str) -> axum::http::Response<Body> {
    app.oneshot(
        Request::builder()
            .method("DELETE")
            .uri(uri)
            .body(Body::empty())
            .unwrap(),
    )
    .await
    .unwrap()
}

async fn create_process(app: axum::Router, name: &str, command: &str, start_now: bool) -> axum::http::Response<Body> {
    let body = create_process_body(command, start_now);
    app.oneshot(
        Request::builder()
            .method("POST")
            .uri(format!("/api/v1/processes/{name}"))
            .header("content-type", "application/json")
            .body(Body::from(serde_json::to_string(&body).unwrap()))
            .unwrap(),
    )
    .await
    .unwrap()
}

// ── GET /api/v1/processes ────────────────────────────────────

mod list_processes {
    use super::*;

    #[tokio::test]
    async fn returns_ok_with_empty_list() {
        let resp = send_get(make_app(), "/api/v1/processes").await;
        assert_eq!(resp.status(), StatusCode::OK, "空列表应返回 200");
    }
}

// ── GET /api/v1/processes/:name ──────────────────────────────

mod get_process {
    use super::*;

    #[tokio::test]
    async fn returns_not_found_for_unknown() {
        let resp = send_get(make_app(), "/api/v1/processes/nonexistent").await;
        assert_eq!(resp.status(), StatusCode::NOT_FOUND, "不存在的进程应返回 404");
    }
}

// ── POST /api/v1/processes/:name ─────────────────────────────

mod create_process {
    use super::*;

    #[tokio::test]
    async fn returns_created_for_valid_input() {
        let dir = tempfile::tempdir().unwrap();
        let app = make_app_with_dir(dir.path());

        let resp = create_process(app, "test-svc", "echo hello", false).await;
        assert_eq!(resp.status(), StatusCode::CREATED, "有效输入应返回 201");
    }

    #[tokio::test]
    async fn returns_bad_request_for_empty_command() {
        let dir = tempfile::tempdir().unwrap();
        let app = make_app_with_dir(dir.path());

        let resp = create_process(app, "bad", "   ", false).await;
        assert_eq!(resp.status(), StatusCode::BAD_REQUEST, "空命令应返回 400");
    }

    #[tokio::test]
    async fn returns_bad_request_for_duplicate() {
        let dir = tempfile::tempdir().unwrap();
        let app = make_app_with_dir(dir.path());

        let resp = create_process(app.clone(), "dup", "echo hello", false).await;
        assert_eq!(resp.status(), StatusCode::CREATED);

        let resp = create_process(app, "dup", "echo hello", false).await;
        assert_eq!(resp.status(), StatusCode::BAD_REQUEST, "重复名称应返回 400");
    }

    #[tokio::test]
    async fn process_appears_in_snapshot_after_creation() {
        let dir = tempfile::tempdir().unwrap();
        let (app, shared) = spawn_app_with_shared(dir.path());

        let resp = create_process(app, "my-svc", "echo hello", false).await;
        assert_eq!(resp.status(), StatusCode::CREATED);

        let info = shared.get_process_info("my-svc");
        assert!(info.is_some(), "创建后快照中应有该进程");
        assert_eq!(info.unwrap().name, "my-svc", "进程名应一致");
    }
}

// ── POST /api/v1/processes/:name/start ───────────────────────

mod start_process {
    use super::*;

    #[tokio::test]
    async fn returns_bad_request_for_unknown() {
        let resp = send_post(make_app(), "/api/v1/processes/nonexistent/start").await;
        assert_eq!(resp.status(), StatusCode::BAD_REQUEST, "不存在的进程应返回 400");
    }
}

// ── POST /api/v1/processes/:name/stop ────────────────────────

mod stop_process {
    use super::*;

    #[tokio::test]
    async fn returns_bad_request_for_unknown() {
        let resp = send_post(make_app(), "/api/v1/processes/nonexistent/stop").await;
        assert_eq!(resp.status(), StatusCode::BAD_REQUEST, "不存在的进程应返回 400");
    }
}

// ── POST /api/v1/processes/:name/restart ─────────────────────

mod restart_process {
    use super::*;

    #[tokio::test]
    async fn returns_bad_request_for_unknown() {
        let resp = send_post(make_app(), "/api/v1/processes/nonexistent/restart").await;
        assert_eq!(resp.status(), StatusCode::BAD_REQUEST, "不存在的进程应返回 400");
    }
}

// ── DELETE /api/v1/processes/:name ───────────────────────────

mod delete_process {
    use super::*;

    #[tokio::test]
    async fn returns_not_found_for_unknown() {
        let resp = send_delete(make_app(), "/api/v1/processes/nonexistent").await;
        assert_eq!(resp.status(), StatusCode::NOT_FOUND, "不存在的进程应返回 404");
    }
}

// ── GET /api/v1/processes/:name/config ───────────────────────

mod get_process_config {
    use super::*;

    #[tokio::test]
    async fn returns_not_found_for_unknown() {
        let resp = send_get(make_app(), "/api/v1/processes/nonexistent/config").await;
        assert_eq!(resp.status(), StatusCode::NOT_FOUND, "不存在的进程应返回 404");
    }
}

// ── GET /api/v1/processes/:name/logs ─────────────────────────

mod get_logs {
    use super::*;

    #[tokio::test]
    async fn returns_not_found_for_unknown() {
        let resp = send_get(make_app(), "/api/v1/processes/nonexistent/logs?lines=50").await;
        assert_eq!(resp.status(), StatusCode::NOT_FOUND, "不存在的进程应返回 404");
    }
}

// ── DELETE /api/v1/processes/:name/logs ──────────────────────

mod clear_logs {
    use super::*;

    #[tokio::test]
    async fn returns_not_found_for_unknown() {
        let resp = send_delete(make_app(), "/api/v1/processes/nonexistent/logs").await;
        assert_eq!(resp.status(), StatusCode::NOT_FOUND, "不存在的进程应返回 404");
    }
}

// ── GET /api/v1/processes/:name/logs/download ────────────────

mod download_logs {
    use super::*;

    #[tokio::test]
    async fn returns_not_found_for_unknown() {
        let resp = send_get(make_app(), "/api/v1/processes/nonexistent/logs/download").await;
        assert_eq!(resp.status(), StatusCode::NOT_FOUND, "不存在的进程应返回 404");
    }
}

// ── POST /api/v1/processes/:name/health ──────────────────────

mod health_check {
    use super::*;

    #[tokio::test]
    async fn returns_bad_request_when_not_configured() {
        let dir = tempfile::tempdir().unwrap();
        let app = make_app_with_dir(dir.path());
        create_process(app.clone(), "test-svc", "echo hello", false).await;

        let resp = send_post(app, "/api/v1/processes/test-svc/health").await;
        assert_eq!(
            resp.status(),
            StatusCode::BAD_REQUEST,
            "未配置健康检查应返回 400"
        );
    }
}

// ── GET /api/v1/stats ────────────────────────────────────────

mod get_stats {
    use super::*;

    #[tokio::test]
    async fn returns_ok_with_zero_counts() {
        let resp = send_get(make_app(), "/api/v1/stats").await;
        assert_eq!(resp.status(), StatusCode::OK, "空状态应返回 200");
    }
}

// ── POST /api/v1/reload ──────────────────────────────────────

mod reload_config {
    use super::*;

    #[tokio::test]
    async fn returns_server_error_without_config_path() {
        let resp = send_post(make_app(), "/api/v1/reload").await;
        assert_eq!(
            resp.status(),
            StatusCode::INTERNAL_SERVER_ERROR,
            "无配置文件路径应返回 500"
        );
    }
}

// ── 认证中间件 ──────────────────────────────────────────────

mod auth_middleware {
    use super::*;

    #[tokio::test]
    async fn rejects_request_without_key() {
        let app = make_app_with_key(Some("secret-key".to_string()));
        let resp = send_get(app, "/api/v1/processes").await;

        assert_eq!(resp.status(), StatusCode::UNAUTHORIZED, "无 key 应返回 401");
    }

    #[tokio::test]
    async fn accepts_valid_bearer_token() {
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

        assert_eq!(resp.status(), StatusCode::OK, "有效 Bearer 应返回 200");
    }

    #[tokio::test]
    async fn rejects_invalid_bearer_token() {
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

        assert_eq!(resp.status(), StatusCode::UNAUTHORIZED, "错误 Bearer 应返回 401");
    }

    #[tokio::test]
    async fn accepts_valid_token_in_query() {
        let app = make_app_with_key(Some("secret-key".to_string()));
        let resp = send_get(app, "/api/v1/processes?token=secret-key").await;

        assert_eq!(resp.status(), StatusCode::OK, "有效 query token 应返回 200");
    }
}

// ── GET /api/v1/fs/browse ────────────────────────────────────

mod fs_browse {
    use super::*;

    #[tokio::test]
    async fn returns_ok_for_current_directory() {
        let resp = send_get(make_app(), "/api/v1/fs/browse").await;
        assert_eq!(resp.status(), StatusCode::OK, "浏览当前目录应返回 200");
    }

    #[tokio::test]
    async fn returns_bad_request_for_nonexistent_path() {
        let resp = send_get(make_app(), "/api/v1/fs/browse?path=/nonexistent/path/that/does/not/exist").await;
        assert_eq!(resp.status(), StatusCode::BAD_REQUEST, "不存在的路径应返回 400");
    }
}

// ── GET /metrics ────────────────────────────────────────────

mod metrics_endpoint {
    use super::*;

    #[tokio::test]
    async fn rejects_without_auth() {
        let app = make_app_with_key(Some("secret-key".to_string()));
        let resp = send_get(app, "/metrics").await;
        assert_eq!(resp.status(), StatusCode::UNAUTHORIZED, "/metrics 需要认证");
    }

    #[tokio::test]
    async fn contains_gugu_processes_metric() {
        let resp = send_get(make_app(), "/metrics").await;
        let body = axum::body::to_bytes(resp.into_body(), 4096)
            .await
            .unwrap();
        let text = String::from_utf8(body.to_vec()).unwrap();
        assert!(text.contains("gugu_processes "), "/metrics 应包含 gugu_processes 聚合指标");
        assert!(text.contains("gugu_processes_running"), "/metrics 应包含 gugu_processes_running");
        assert!(
            text.contains("gugu_process_status") || text.contains("gugu_processes 0"),
            "/metrics 应包含 gugu_process_status 或显示 0 个进程"
        );
    }
}

// ── GET /api/v1/groups ──────────────────────────────────────

mod list_groups {
    use super::*;

    #[tokio::test]
    async fn returns_empty_when_no_groups() {
        let resp = send_get(make_app(), "/api/v1/groups").await;
        assert_eq!(resp.status(), StatusCode::OK);
        let body = axum::body::to_bytes(resp.into_body(), 4096).await.unwrap();
        let groups: Vec<serde_json::Value> = serde_json::from_slice(&body).unwrap();
        assert!(groups.is_empty(), "无进程时应返回空组列表");
    }

    #[tokio::test]
    async fn returns_groups_with_members() {
        let dir = tempfile::tempdir().unwrap();
        let app = make_app_with_dir(dir.path());
        let body = create_process_body_with_group("echo a", false, Some("web"));
        app.clone().oneshot(
            Request::builder().method("POST").uri("/api/v1/processes/svc-a")
                .header("content-type", "application/json")
                .body(Body::from(serde_json::to_string(&body).unwrap())).unwrap(),
        ).await.unwrap();
        let body = create_process_body_with_group("echo b", false, Some("web"));
        app.clone().oneshot(
            Request::builder().method("POST").uri("/api/v1/processes/svc-b")
                .header("content-type", "application/json")
                .body(Body::from(serde_json::to_string(&body).unwrap())).unwrap(),
        ).await.unwrap();

        let resp = send_get(app, "/api/v1/groups").await;
        assert_eq!(resp.status(), StatusCode::OK);
        let body = axum::body::to_bytes(resp.into_body(), 4096).await.unwrap();
        let groups: Vec<serde_json::Value> = serde_json::from_slice(&body).unwrap();
        assert_eq!(groups.len(), 1, "应有一个组");
        assert_eq!(groups[0]["name"], "web");
    }
}

// ── GET /api/v1/groups/:group ───────────────────────────────

mod get_group {
    use super::*;

    #[tokio::test]
    async fn returns_not_found_for_unknown() {
        let resp = send_get(make_app(), "/api/v1/groups/nonexistent").await;
        assert_eq!(resp.status(), StatusCode::NOT_FOUND, "不存在的组应返回 404");
    }

    #[tokio::test]
    async fn returns_group_with_member_names() {
        let dir = tempfile::tempdir().unwrap();
        let app = make_app_with_dir(dir.path());
        let body = create_process_body_with_group("echo x", false, Some("api"));
        app.clone().oneshot(
            Request::builder().method("POST").uri("/api/v1/processes/my-api")
                .header("content-type", "application/json")
                .body(Body::from(serde_json::to_string(&body).unwrap())).unwrap(),
        ).await.unwrap();

        let resp = send_get(app, "/api/v1/groups/api").await;
        assert_eq!(resp.status(), StatusCode::OK);
        let body = axum::body::to_bytes(resp.into_body(), 4096).await.unwrap();
        let group: serde_json::Value = serde_json::from_slice(&body).unwrap();
        assert_eq!(group["name"], "api");
        assert!(group["processes"].as_array().unwrap().iter().any(|p| p == "my-api"));
    }
}

// ── POST /api/v1/groups/:group/start ────────────────────────

mod start_group {
    use super::*;

    #[tokio::test]
    async fn returns_not_found_for_unknown() {
        let resp = send_post(make_app(), "/api/v1/groups/nonexistent/start").await;
        assert_eq!(resp.status(), StatusCode::NOT_FOUND, "不存在的组应返回 404");
    }
}

// ── POST /api/v1/groups/:group/stop ─────────────────────────

mod stop_group {
    use super::*;

    #[tokio::test]
    async fn returns_not_found_for_unknown() {
        let resp = send_post(make_app(), "/api/v1/groups/nonexistent/stop").await;
        assert_eq!(resp.status(), StatusCode::NOT_FOUND, "不存在的组应返回 404");
    }
}

// ── POST /api/v1/groups/:group/restart ──────────────────────

mod restart_group {
    use super::*;

    #[tokio::test]
    async fn returns_not_found_for_unknown() {
        let resp = send_post(make_app(), "/api/v1/groups/nonexistent/restart").await;
        assert_eq!(resp.status(), StatusCode::NOT_FOUND, "不存在的组应返回 404");
    }
}
