use axum::body::Body;
use axum::http::{Request, StatusCode};
use axum::middleware;
use gugu_core::config::AppConfig;
use gugu_core::manager::SharedManager;
use gugu_server::state::AppState;
use tower::ServiceExt;

// ── 测试辅助 ──────────────────────────────────────────────

/// 测试用临时目录：Drop 时递归删除。不用 tempfile 依赖
/// （它会把 getrandom 等一串传递依赖带进依赖树）。
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
        "gugu-api-test-{}-{nanos}-{seq}",
        std::process::id()
    ));
    std::fs::create_dir_all(&path).expect("创建测试临时目录失败");
    TempDir { path }
}

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
    build_router_with_shutdown(shared, api_key, None)
}

fn build_router_with_shutdown(
    shared: SharedManager,
    api_key: Option<String>,
    shutdown_tx: Option<tokio::sync::mpsc::Sender<()>>,
) -> axum::Router {
    build_router_with_roots(shared, api_key, shutdown_tx, Vec::new())
}

fn build_router_with_roots(
    shared: SharedManager,
    api_key: Option<String>,
    shutdown_tx: Option<tokio::sync::mpsc::Sender<()>>,
    fs_roots: Vec<std::path::PathBuf>,
) -> axum::Router {
    build_router_with_fs(shared, api_key, shutdown_tx, fs_roots, false)
}

fn build_router_with_fs(
    shared: SharedManager,
    api_key: Option<String>,
    shutdown_tx: Option<tokio::sync::mpsc::Sender<()>>,
    fs_roots: Vec<std::path::PathBuf>,
    fs_write: bool,
) -> axum::Router {
    let state = AppState::new(shared)
        .with_api_key(api_key)
        .with_shutdown(shutdown_tx)
        .with_fs(fs_roots, fs_write);
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

fn create_process_body_with_group(
    command: &str,
    start_now: bool,
    group: Option<&str>,
) -> serde_json::Value {
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
    app.oneshot(Request::builder().uri(uri).body(Body::empty()).unwrap())
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

async fn create_process(
    app: axum::Router,
    name: &str,
    command: &str,
    start_now: bool,
) -> axum::http::Response<Body> {
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
        assert_eq!(
            resp.status(),
            StatusCode::NOT_FOUND,
            "不存在的进程应返回 404"
        );
    }
}

// ── POST /api/v1/processes/:name ─────────────────────────────

mod create_process {
    use super::*;

    #[tokio::test]
    async fn returns_created_for_valid_input() {
        let dir = tempdir();
        let app = make_app_with_dir(dir.path());

        let resp = create_process(app, "test-svc", "echo hello", false).await;
        assert_eq!(resp.status(), StatusCode::CREATED, "有效输入应返回 201");
    }

    #[tokio::test]
    async fn returns_bad_request_for_empty_command() {
        let dir = tempdir();
        let app = make_app_with_dir(dir.path());

        let resp = create_process(app, "bad", "   ", false).await;
        assert_eq!(resp.status(), StatusCode::BAD_REQUEST, "空命令应返回 400");
    }

    #[tokio::test]
    async fn returns_bad_request_for_duplicate() {
        let dir = tempdir();
        let app = make_app_with_dir(dir.path());

        let resp = create_process(app.clone(), "dup", "echo hello", false).await;
        assert_eq!(resp.status(), StatusCode::CREATED);

        let resp = create_process(app, "dup", "echo hello", false).await;
        assert_eq!(resp.status(), StatusCode::BAD_REQUEST, "重复名称应返回 400");
    }

    #[tokio::test]
    async fn process_appears_in_snapshot_after_creation() {
        let dir = tempdir();
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
        assert_eq!(
            resp.status(),
            StatusCode::BAD_REQUEST,
            "不存在的进程应返回 400"
        );
    }
}

// ── POST /api/v1/processes/:name/stop ────────────────────────

mod stop_process {
    use super::*;

    #[tokio::test]
    async fn returns_bad_request_for_unknown() {
        let resp = send_post(make_app(), "/api/v1/processes/nonexistent/stop").await;
        assert_eq!(
            resp.status(),
            StatusCode::BAD_REQUEST,
            "不存在的进程应返回 400"
        );
    }
}

// ── POST /api/v1/processes/:name/restart ─────────────────────

mod restart_process {
    use super::*;

    #[tokio::test]
    async fn returns_bad_request_for_unknown() {
        let resp = send_post(make_app(), "/api/v1/processes/nonexistent/restart").await;
        assert_eq!(
            resp.status(),
            StatusCode::BAD_REQUEST,
            "不存在的进程应返回 400"
        );
    }
}

// ── DELETE /api/v1/processes/:name ───────────────────────────

mod delete_process {
    use super::*;

    #[tokio::test]
    async fn returns_not_found_for_unknown() {
        let resp = send_delete(make_app(), "/api/v1/processes/nonexistent").await;
        assert_eq!(
            resp.status(),
            StatusCode::NOT_FOUND,
            "不存在的进程应返回 404"
        );
    }
}

// ── GET /api/v1/processes/:name/config ───────────────────────

mod get_process_config {
    use super::*;

    #[tokio::test]
    async fn returns_not_found_for_unknown() {
        let resp = send_get(make_app(), "/api/v1/processes/nonexistent/config").await;
        assert_eq!(
            resp.status(),
            StatusCode::NOT_FOUND,
            "不存在的进程应返回 404"
        );
    }
}

// ── GET /api/v1/processes/:name/logs ─────────────────────────

mod get_logs {
    use super::*;

    #[tokio::test]
    async fn returns_not_found_for_unknown() {
        let resp = send_get(make_app(), "/api/v1/processes/nonexistent/logs?lines=50").await;
        assert_eq!(
            resp.status(),
            StatusCode::NOT_FOUND,
            "不存在的进程应返回 404"
        );
    }
}

// ── DELETE /api/v1/processes/:name/logs ──────────────────────

mod clear_logs {
    use super::*;

    #[tokio::test]
    async fn returns_not_found_for_unknown() {
        let resp = send_delete(make_app(), "/api/v1/processes/nonexistent/logs").await;
        assert_eq!(
            resp.status(),
            StatusCode::NOT_FOUND,
            "不存在的进程应返回 404"
        );
    }
}

// ── GET /api/v1/processes/:name/logs/download ────────────────

mod download_logs {
    use super::*;

    #[tokio::test]
    async fn returns_not_found_for_unknown() {
        let resp = send_get(make_app(), "/api/v1/processes/nonexistent/logs/download").await;
        assert_eq!(
            resp.status(),
            StatusCode::NOT_FOUND,
            "不存在的进程应返回 404"
        );
    }
}

// ── POST /api/v1/processes/:name/health ──────────────────────

mod health_check {
    use super::*;

    #[tokio::test]
    async fn returns_bad_request_when_not_configured() {
        let dir = tempdir();
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

mod shutdown_endpoint {
    use super::*;

    #[tokio::test]
    async fn signals_daemon_shutdown_when_wired() {
        let (tx, mut rx) = tokio::sync::mpsc::channel(1);
        let shared = gugu_core::manager::start(&AppConfig::default(), None);
        let app = build_router_with_shutdown(shared, None, Some(tx));

        let resp = send_post(app, "/api/v1/shutdown").await;
        assert_eq!(resp.status(), StatusCode::OK, "接线时应返回 200");
        assert!(rx.try_recv().is_ok(), "应真的发出停止信号");
    }

    #[tokio::test]
    async fn reports_not_implemented_without_wiring() {
        // 未注入信号通道时不许假装成功（否则调用方会一直等进程退出）
        let resp = send_post(make_app(), "/api/v1/shutdown").await;
        assert_eq!(resp.status(), StatusCode::NOT_IMPLEMENTED);
    }
}

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

        assert_eq!(
            resp.status(),
            StatusCode::UNAUTHORIZED,
            "错误 Bearer 应返回 401"
        );
    }

    #[tokio::test]
    async fn accepts_valid_token_in_query() {
        let app = make_app_with_key(Some("secret-key".to_string()));
        let resp = send_get(app, "/api/v1/processes?token=secret-key").await;

        assert_eq!(resp.status(), StatusCode::OK, "有效 query token 应返回 200");
    }
}

// ── /api/v1/fs ────────────────────────────────────────��─────

mod fs_management {
    use super::*;

    fn app_with_root(root: &std::path::Path) -> axum::Router {
        app_with(root, false)
    }

    fn app_with(root: &std::path::Path, writable: bool) -> axum::Router {
        let config = AppConfig::default();
        let shared = gugu_core::manager::start(&config, None);
        build_router_with_fs(shared, None, None, vec![root.to_path_buf()], writable)
    }

    /// URI 里只放正斜杠：Windows 的反斜杠不是合法 URI 字符。
    fn uri_path(path: &std::path::Path) -> String {
        path.display().to_string().replace('\\', "/")
    }

    fn encode(path: &std::path::Path) -> String {
        uri_path(path)
            .replace('%', "%25")
            .replace(' ', "%20")
            .replace('#', "%23")
            .replace('?', "%3F")
            .replace('+', "%2B")
    }

    async fn send_json(
        app: axum::Router,
        method: &str,
        uri: &str,
        body: serde_json::Value,
    ) -> axum::http::Response<Body> {
        app.oneshot(
            Request::builder()
                .method(method)
                .uri(uri)
                .header("content-type", "application/json")
                .body(Body::from(serde_json::to_vec(&body).expect("序列化请求体")))
                .unwrap(),
        )
        .await
        .unwrap()
    }

    async fn text_of(resp: axum::http::Response<Body>) -> String {
        let bytes = axum::body::to_bytes(resp.into_body(), 1 << 20)
            .await
            .expect("读取响应体");
        String::from_utf8_lossy(&bytes).into_owned()
    }

    #[tokio::test]
    async fn lists_the_current_directory() {
        let resp = send_get(make_app(), "/api/v1/fs/list").await;
        assert_eq!(resp.status(), StatusCode::OK, "列出当前目录应返回 200");
        let body = text_of(resp).await;
        assert!(body.contains("\"entries\""), "{body}");
        assert!(body.contains("\"writable\":false"), "缺省为只读: {body}");
    }

    #[tokio::test]
    async fn missing_path_is_not_found() {
        let resp = send_get(
            make_app(),
            "/api/v1/fs/list?path=/nonexistent/path/that/does/not/exist",
        )
        .await;
        assert_eq!(resp.status(), StatusCode::NOT_FOUND);
    }

    #[tokio::test]
    async fn allows_a_directory_inside_the_root() {
        let root = tempdir();
        let inside = root.path().join("inside");
        std::fs::create_dir_all(&inside).expect("创建子目录失败");

        let resp = send_get(
            app_with_root(root.path()),
            &format!("/api/v1/fs/list?path={}", encode(&inside)),
        )
        .await;

        assert_eq!(resp.status(), StatusCode::OK, "白名单内的目录应放行");
    }

    #[tokio::test]
    async fn rejects_a_directory_outside_the_root() {
        let root = tempdir();
        let outside = tempdir();

        let resp = send_get(
            app_with_root(root.path()),
            &format!("/api/v1/fs/list?path={}", encode(outside.path())),
        )
        .await;

        assert_eq!(resp.status(), StatusCode::FORBIDDEN, "越界目录应返回 403");
    }

    #[tokio::test]
    async fn defaults_to_the_first_root() {
        let root = tempdir();
        let resp = send_get(app_with_root(root.path()), "/api/v1/fs/list?path=").await;
        assert_eq!(resp.status(), StatusCode::OK, "缺省路径应落在第一个根目录");
    }

    #[tokio::test]
    async fn sibling_with_a_shared_prefix_is_not_inside() {
        let parent = tempdir();
        let root = parent.path().join("data");
        let sibling = parent.path().join("data-x");
        std::fs::create_dir_all(&root).expect("创建根目录失败");
        std::fs::create_dir_all(&sibling).expect("创建兄弟目录失败");

        let resp = send_get(
            app_with_root(&root),
            &format!("/api/v1/fs/list?path={}", encode(&sibling)),
        )
        .await;

        assert_eq!(
            resp.status(),
            StatusCode::FORBIDDEN,
            "按路径分量比较，data-x 不在 data 之下"
        );
    }

    #[tokio::test]
    async fn roots_reports_the_whitelist_and_write_mode() {
        let root = tempdir();
        let resp = send_get(app_with(root.path(), true), "/api/v1/fs/roots").await;
        assert_eq!(resp.status(), StatusCode::OK);
        let text = text_of(resp).await;
        assert!(text.contains("\"writable\":true"), "{text}");
        assert!(
            text.to_lowercase()
                .contains(&uri_path(root.path()).to_lowercase()),
            "根目录清单应包含白名单路径: {text}"
        );
    }

    #[tokio::test]
    async fn stat_and_touch_handle_a_single_file() {
        let root = tempdir();
        let file = root.path().join("empty.log");

        let touched = send_json(
            app_with(root.path(), true),
            "POST",
            "/api/v1/fs/touch",
            serde_json::json!({ "path": uri_path(&file) }),
        )
        .await;
        assert_eq!(
            touched.status(),
            StatusCode::OK,
            "{}",
            text_of(touched).await
        );
        assert_eq!(std::fs::metadata(&file).expect("文件应存在").len(), 0);

        let stat = send_get(
            app_with(root.path(), true),
            &format!("/api/v1/fs/stat?path={}", encode(&file)),
        )
        .await;
        assert_eq!(stat.status(), StatusCode::OK);
        let text = text_of(stat).await;
        assert!(text.contains("\"kind\":\"file\""), "{text}");
    }

    #[tokio::test]
    async fn stat_without_a_path_is_a_bad_request() {
        let root = tempdir();
        let resp = send_get(app_with_root(root.path()), "/api/v1/fs/stat").await;
        assert_eq!(resp.status(), StatusCode::BAD_REQUEST);
    }

    #[tokio::test]
    async fn read_only_mode_refuses_every_mutation() {
        let root = tempdir();
        let target = root.path().join("new-dir");
        let body = serde_json::json!({ "path": uri_path(&target) });

        let resp = send_json(app_with_root(root.path()), "POST", "/api/v1/fs/mkdir", body).await;
        assert_eq!(resp.status(), StatusCode::FORBIDDEN, "只读模式返回 403");
        assert!(!target.exists());
        let text = text_of(resp).await;
        assert!(text.contains("fs_write"), "{text}");
    }

    #[tokio::test]
    async fn creates_writes_copies_moves_and_deletes() {
        let root = tempdir();
        let folder = root.path().join("docs");
        let file = folder.join("a.txt");
        let copy = folder.join("b.txt");

        let created = send_json(
            app_with(root.path(), true),
            "POST",
            "/api/v1/fs/mkdir",
            serde_json::json!({ "path": uri_path(&folder) }),
        )
        .await;
        assert_eq!(
            created.status(),
            StatusCode::OK,
            "{}",
            text_of(created).await
        );

        let saved = send_json(
            app_with(root.path(), true),
            "PUT",
            "/api/v1/fs/write",
            serde_json::json!({ "path": uri_path(&file), "content": "你好", "create": true }),
        )
        .await;
        assert_eq!(saved.status(), StatusCode::OK, "{}", text_of(saved).await);

        let read = send_get(
            app_with(root.path(), true),
            &format!("/api/v1/fs/read?path={}", encode(&file)),
        )
        .await;
        assert_eq!(read.status(), StatusCode::OK);
        let text = text_of(read).await;
        assert!(text.contains("你好"), "{text}");
        assert!(text.contains("\"encoding\":\"utf-8\""), "{text}");

        let copied = send_json(
            app_with(root.path(), true),
            "POST",
            "/api/v1/fs/copy",
            serde_json::json!({ "from": uri_path(&file), "to": uri_path(&copy) }),
        )
        .await;
        assert_eq!(copied.status(), StatusCode::OK, "{}", text_of(copied).await);

        let renamed = file.with_file_name("c.txt");
        let moved = send_json(
            app_with(root.path(), true),
            "POST",
            "/api/v1/fs/rename",
            serde_json::json!({ "from": uri_path(&file), "to": uri_path(&renamed) }),
        )
        .await;
        assert_eq!(moved.status(), StatusCode::OK, "{}", text_of(moved).await);

        let removed = send_delete(
            app_with(root.path(), true),
            &format!("/api/v1/fs?path={}&recursive=true", encode(&folder)),
        )
        .await;
        assert_eq!(
            removed.status(),
            StatusCode::OK,
            "{}",
            text_of(removed).await
        );
        assert!(!folder.exists());
    }

    #[tokio::test]
    async fn uploads_the_request_body_to_the_target_path() {
        let root = tempdir();
        let target = root.path().join("uploaded.bin");
        let payload: Vec<u8> = (0..4096u32).map(|value| (value % 251) as u8).collect();

        let resp = app_with(root.path(), true)
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri(format!("/api/v1/fs/upload?path={}", encode(&target)))
                    .body(Body::from(payload.clone()))
                    .unwrap(),
            )
            .await
            .unwrap();

        assert_eq!(resp.status(), StatusCode::OK, "{}", text_of(resp).await);
        assert_eq!(std::fs::read(&target).expect("上传后的文件"), payload);
    }

    #[tokio::test]
    async fn upload_refuses_to_overwrite_without_the_flag() {
        let root = tempdir();
        let target = root.path().join("exists.txt");
        std::fs::write(&target, b"old").expect("写文件");

        let resp = app_with(root.path(), true)
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri(format!("/api/v1/fs/upload?path={}", encode(&target)))
                    .body(Body::from("new"))
                    .unwrap(),
            )
            .await
            .unwrap();

        assert_eq!(resp.status(), StatusCode::BAD_REQUEST);
        assert_eq!(std::fs::read(&target).expect("原文件仍在"), b"old");
    }

    #[tokio::test]
    async fn mutations_outside_the_root_are_forbidden() {
        let root = tempdir();
        let outside = tempdir();
        let resp = send_json(
            app_with(root.path(), true),
            "POST",
            "/api/v1/fs/mkdir",
            serde_json::json!({ "path": uri_path(&outside.path().join("x")) }),
        )
        .await;
        assert_eq!(resp.status(), StatusCode::FORBIDDEN);
        assert!(!outside.path().join("x").exists());
    }

    /// 每次请求都消费一个 Router，这里统一克隆。
    async fn put_chunk(
        app: axum::Router,
        id: &str,
        offset: u64,
        path: &std::path::Path,
        body: &str,
    ) -> (StatusCode, String) {
        let resp = app
            .oneshot(
                Request::builder()
                    .method("PUT")
                    .uri(format!(
                        "/api/v1/fs/upload/session/{id}?offset={offset}&path={}",
                        encode(path)
                    ))
                    .body(Body::from(body.to_string()))
                    .unwrap(),
            )
            .await
            .unwrap();
        let status = resp.status();
        (status, text_of(resp).await)
    }

    async fn begin_session(
        app: &axum::Router,
        target: &std::path::Path,
        size: u64,
    ) -> serde_json::Value {
        let resp = send_json(
            app.clone(),
            "POST",
            &format!("/api/v1/fs/upload/session?path={}", encode(target)),
            serde_json::json!({ "path": uri_path(target), "size": size }),
        )
        .await;
        let status = resp.status();
        let text = text_of(resp).await;
        assert_eq!(status, StatusCode::OK, "{text}");
        serde_json::from_str(&text).expect("会话 JSON")
    }
    #[tokio::test]
    async fn archive_packs_a_directory_under_its_own_name() {
        let root = tempdir();
        let dir = root.path().join("demo");
        std::fs::create_dir_all(dir.join("sub")).expect("建目录");
        std::fs::write(dir.join("a.txt"), b"alpha").expect("写文件");
        std::fs::write(dir.join("sub/b.txt"), b"beta").expect("写文件");

        let resp = send_get(
            app_with_root(root.path()),
            &format!("/api/v1/fs/archive?path={}", encode(&dir)),
        )
        .await;
        let status = resp.status();
        if status != StatusCode::OK {
            panic!("{}", text_of(resp).await);
        }
        let disposition = resp
            .headers()
            .get("content-disposition")
            .and_then(|value| value.to_str().ok())
            .expect("应带归档文件名");
        assert!(disposition.contains("demo.zip"), "{disposition}");
        assert_eq!(
            resp.headers()
                .get("content-type")
                .and_then(|v| v.to_str().ok()),
            Some("application/zip")
        );

        let bytes = axum::body::to_bytes(resp.into_body(), 1 << 20)
            .await
            .expect("读取归档");
        let mut archive =
            zip::ZipArchive::new(std::io::Cursor::new(bytes.to_vec())).expect("读取 zip");
        let mut names = Vec::new();
        for index in 0..archive.len() {
            names.push(
                archive
                    .by_index(index)
                    .expect("条目可读")
                    .name()
                    .to_string(),
            );
        }
        assert!(names.contains(&"demo/a.txt".to_string()), "{names:?}");
        assert!(names.contains(&"demo/sub/b.txt".to_string()), "{names:?}");
    }

    #[tokio::test]
    async fn archive_refuses_single_files_and_outside_paths() {
        let root = tempdir();
        let file = root.path().join("a.txt");
        std::fs::write(&file, b"a").expect("写文件");
        let outside = tempdir();

        let file_resp = send_get(
            app_with_root(root.path()),
            &format!("/api/v1/fs/archive?path={}", encode(&file)),
        )
        .await;
        assert_eq!(
            file_resp.status(),
            StatusCode::BAD_REQUEST,
            "单个文件走下载"
        );

        let outside_resp = send_get(
            app_with_root(root.path()),
            &format!("/api/v1/fs/archive?path={}", encode(outside.path())),
        )
        .await;
        assert_eq!(
            outside_resp.status(),
            StatusCode::FORBIDDEN,
            "越界目录应拒绝"
        );
    }

    fn write_zip(path: &std::path::Path, entries: &[(&str, &[u8])]) {
        use std::io::Write;
        let file = std::fs::File::create(path).expect("建归档");
        let mut zip = zip::ZipWriter::new(file);
        let options = zip::write::SimpleFileOptions::default();
        for (name, body) in entries {
            zip.start_file(*name, options).expect("写条目");
            zip.write_all(body).expect("写内容");
        }
        zip.finish().expect("收尾");
    }

    fn extract_body(path: &std::path::Path) -> (String, serde_json::Value) {
        (
            format!("/api/v1/fs/extract?path={}", encode(path)),
            serde_json::json!({ "path": uri_path(path) }),
        )
    }

    #[tokio::test]
    async fn extract_unpacks_the_archive_beside_it() {
        let root = tempdir();
        let archive = root.path().join("资料.zip");
        write_zip(&archive, &[("a.txt", b"alpha"), ("sub/中文.txt", b"beta")]);
        let (uri, body) = extract_body(&archive);

        let resp = send_json(app_with(root.path(), true), "POST", &uri, body).await;
        let status = resp.status();
        let text = text_of(resp).await;
        assert_eq!(status, StatusCode::OK, "{text}");
        assert!(text.contains("已解压"), "{text}");

        let target = root.path().join("资料");
        assert_eq!(std::fs::read(target.join("a.txt")).expect("读回"), b"alpha");
        assert_eq!(
            std::fs::read(target.join("sub/中文.txt")).expect("读回"),
            b"beta"
        );
        assert!(!has_leftovers(root.path()), "解压不该留下中转目录");
    }

    #[tokio::test]
    async fn extract_needs_overwrite_and_respects_read_only() {
        let root = tempdir();
        let archive = root.path().join("data.zip");
        write_zip(&archive, &[("fresh.txt", b"new")]);
        std::fs::create_dir_all(root.path().join("data")).expect("建旧目录");
        std::fs::write(root.path().join("data/stale.txt"), b"old").expect("写旧文件");
        let (uri, body) = extract_body(&archive);

        let resp = send_json(app_with(root.path(), true), "POST", &uri, body.clone()).await;
        let status = resp.status();
        let text = text_of(resp).await;
        assert_eq!(status, StatusCode::BAD_REQUEST, "{text}");
        assert!(text.contains("目标已存在"), "{text}");

        let readonly = send_json(app_with(root.path(), false), "POST", &uri, body.clone()).await;
        assert_eq!(readonly.status(), StatusCode::FORBIDDEN, "只读模式应拒绝");

        let mut overwrite = body;
        overwrite["overwrite"] = serde_json::Value::Bool(true);
        let resp = send_json(app_with(root.path(), true), "POST", &uri, overwrite).await;
        let status = resp.status();
        let text = text_of(resp).await;
        assert_eq!(status, StatusCode::OK, "{text}");
        assert!(root.path().join("data/fresh.txt").is_file());
        assert!(
            !root.path().join("data/stale.txt").exists(),
            "覆盖是整目录替换"
        );
    }

    #[tokio::test]
    async fn extract_refuses_unknown_formats_and_outside_paths() {
        let root = tempdir();
        let unsupported = root.path().join("a.7z");
        std::fs::write(&unsupported, b"7z").expect("写文件");
        let (uri, body) = extract_body(&unsupported);
        let resp = send_json(app_with(root.path(), true), "POST", &uri, body).await;
        let status = resp.status();
        let text = text_of(resp).await;
        assert_eq!(status, StatusCode::BAD_REQUEST, "{text}");
        assert!(text.contains("不支持"), "{text}");

        let outside = tempdir();
        let escape = outside.path().join("x.zip");
        std::fs::write(&escape, b"PK").expect("写越界文件");
        let (uri, body) = extract_body(&escape);
        let resp = send_json(app_with(root.path(), true), "POST", &uri, body).await;
        assert_eq!(resp.status(), StatusCode::FORBIDDEN, "越界路径应拒绝");
    }

    #[tokio::test]
    async fn chunked_upload_assembles_the_file_and_cleans_up() {
        let root = tempdir();
        let target = root.path().join("big.bin");
        let app = app_with(root.path(), true);
        let session = begin_session(&app, &target, 11).await;
        let id = session["id"].as_str().expect("会话编号").to_string();
        assert_eq!(session["offset"], 0);

        let (status, body) = put_chunk(app.clone(), &id, 0, &target, "hello ").await;
        assert_eq!(status, StatusCode::OK, "{body}");
        let (status, body) = put_chunk(app.clone(), &id, 6, &target, "world").await;
        assert_eq!(status, StatusCode::OK, "{body}");
        assert!(body.contains("\"offset\":11"), "{body}");

        let finish = send_json(
            app.clone(),
            "POST",
            &format!("/api/v1/fs/upload/session/{id}/finish"),
            serde_json::json!({}),
        )
        .await;
        let status = finish.status();
        let text = text_of(finish).await;
        assert_eq!(status, StatusCode::OK, "{text}");
        assert_eq!(
            std::fs::read(&target).expect("完成后的文件"),
            b"hello world"
        );
        assert!(!has_leftovers(root.path()), "临时文件应随完成一起消失");
    }

    #[tokio::test]
    async fn chunk_with_a_wrong_offset_is_rejected_without_writing() {
        let root = tempdir();
        let target = root.path().join("big.bin");
        let app = app_with(root.path(), true);
        let session = begin_session(&app, &target, 11).await;
        let id = session["id"].as_str().expect("会话编号").to_string();

        let (status, _) = put_chunk(app.clone(), &id, 0, &target, "hello ").await;
        assert_eq!(status, StatusCode::OK);

        let (status, body) = put_chunk(app.clone(), &id, 3, &target, "xxx").await;
        assert_eq!(status, StatusCode::BAD_REQUEST, "{body}");
        assert!(body.contains("偏移不匹配"), "{body}");

        // 偏移没被推进，后面的分片照旧接在 6 字节之后。
        let (status, _) = put_chunk(app.clone(), &id, 6, &target, "world").await;
        assert_eq!(status, StatusCode::OK);
        let finish = send_json(
            app.clone(),
            "POST",
            &format!("/api/v1/fs/upload/session/{id}/finish"),
            serde_json::json!({}),
        )
        .await;
        assert_eq!(finish.status(), StatusCode::OK);
        assert_eq!(
            std::fs::read(&target).expect("完成后的文件"),
            b"hello world"
        );
    }

    #[tokio::test]
    async fn a_chunk_session_reports_its_offset_for_resuming() {
        let root = tempdir();
        let target = root.path().join("big.bin");
        let app = app_with(root.path(), true);
        let session = begin_session(&app, &target, 0).await;
        let id = session["id"].as_str().expect("会话编号").to_string();
        let (status, _) = put_chunk(app.clone(), &id, 0, &target, "abc").await;
        assert_eq!(status, StatusCode::OK);

        let resp = send_get(app.clone(), &format!("/api/v1/fs/upload/session/{id}")).await;
        assert_eq!(resp.status(), StatusCode::OK);
        let view: serde_json::Value =
            serde_json::from_str(&text_of(resp).await).expect("会话 JSON");
        assert_eq!(view["offset"], 3);
        assert_eq!(view["path"].as_str(), Some(uri_path(&target).as_str()));
    }

    #[tokio::test]
    async fn aborting_a_session_removes_the_partial_file() {
        let root = tempdir();
        let target = root.path().join("big.bin");
        let app = app_with(root.path(), true);
        let session = begin_session(&app, &target, 11).await;
        let id = session["id"].as_str().expect("会话编号").to_string();
        put_chunk(app.clone(), &id, 0, &target, "hello").await;

        let resp = send_delete(app.clone(), &format!("/api/v1/fs/upload/session/{id}")).await;
        assert_eq!(resp.status(), StatusCode::OK);
        assert!(!target.exists(), "放弃后目标不该出现");
        assert!(!has_leftovers(root.path()), "临时文件应被删掉");
    }

    #[tokio::test]
    async fn chunk_sessions_respect_read_only_and_the_whitelist() {
        let root = tempdir();
        let outside = tempdir();
        let target = root.path().join("big.bin");

        let read_only = send_json(
            app_with(root.path(), false),
            "POST",
            &format!("/api/v1/fs/upload/session?path={}", encode(&target)),
            serde_json::json!({ "path": uri_path(&target), "size": 1 }),
        )
        .await;
        assert_eq!(read_only.status(), StatusCode::FORBIDDEN);

        let escaped = send_json(
            app_with(root.path(), true),
            "POST",
            &format!(
                "/api/v1/fs/upload/session?path={}",
                encode(&outside.path().join("x.bin"))
            ),
            serde_json::json!({ "path": uri_path(&outside.path().join("x.bin")), "size": 1 }),
        )
        .await;
        assert_eq!(escaped.status(), StatusCode::FORBIDDEN);
    }

    /// 目录里是否还留着传输中转文件。
    fn has_leftovers(dir: &std::path::Path) -> bool {
        std::fs::read_dir(dir)
            .expect("读目录")
            .flatten()
            .any(|item| {
                item.file_name()
                    .to_string_lossy()
                    .starts_with(gugu_core::fs::TEMP_PREFIX)
            })
    }

    #[tokio::test]
    async fn download_streams_the_file_with_its_name() {
        let root = tempdir();
        let target = root.path().join("报表.csv");
        std::fs::write(&target, "a,b\n1,2\n").expect("写文件");

        let resp = send_get(
            app_with_root(root.path()),
            &format!("/api/v1/fs/download?path={}", encode(&target)),
        )
        .await;

        assert_eq!(resp.status(), StatusCode::OK);
        let disposition = resp
            .headers()
            .get("content-disposition")
            .and_then(|value| value.to_str().ok())
            .expect("应带下载文件名");
        assert!(disposition.contains("filename*=UTF-8''"), "{disposition}");
        let bytes = axum::body::to_bytes(resp.into_body(), 1 << 20)
            .await
            .expect("读取下载内容");
        assert_eq!(&bytes[..], "a,b\n1,2\n".as_bytes());
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
        let body = axum::body::to_bytes(resp.into_body(), 4096).await.unwrap();
        let text = String::from_utf8(body.to_vec()).unwrap();
        assert!(
            text.contains("gugu_processes "),
            "/metrics 应包含 gugu_processes 聚合指标"
        );
        assert!(
            text.contains("gugu_processes_running"),
            "/metrics 应包含 gugu_processes_running"
        );
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
        let dir = tempdir();
        let app = make_app_with_dir(dir.path());
        let body = create_process_body_with_group("echo a", false, Some("web"));
        app.clone()
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/api/v1/processes/svc-a")
                    .header("content-type", "application/json")
                    .body(Body::from(serde_json::to_string(&body).unwrap()))
                    .unwrap(),
            )
            .await
            .unwrap();
        let body = create_process_body_with_group("echo b", false, Some("web"));
        app.clone()
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/api/v1/processes/svc-b")
                    .header("content-type", "application/json")
                    .body(Body::from(serde_json::to_string(&body).unwrap()))
                    .unwrap(),
            )
            .await
            .unwrap();

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
        let dir = tempdir();
        let app = make_app_with_dir(dir.path());
        let body = create_process_body_with_group("echo x", false, Some("api"));
        app.clone()
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/api/v1/processes/my-api")
                    .header("content-type", "application/json")
                    .body(Body::from(serde_json::to_string(&body).unwrap()))
                    .unwrap(),
            )
            .await
            .unwrap();

        let resp = send_get(app, "/api/v1/groups/api").await;
        assert_eq!(resp.status(), StatusCode::OK);
        let body = axum::body::to_bytes(resp.into_body(), 4096).await.unwrap();
        let group: serde_json::Value = serde_json::from_slice(&body).unwrap();
        assert_eq!(group["name"], "api");
        assert!(group["processes"]
            .as_array()
            .unwrap()
            .iter()
            .any(|p| p == "my-api"));
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

// ── POST /api/v1/processes/:name：可选字段显式传 null ─────────

mod create_process_nullable_fields {
    use super::*;

    #[tokio::test]
    async fn accepts_explicit_nulls_from_cli_clients() {
        let dir = tempdir();
        let app = make_app_with_dir(dir.path());
        let body = serde_json::json!({
            "command": "ping",
            "args": [],
            "auto_start": false,
            "start_now": false,
            "log_format": null,
            "encoding": null,
            "stdout_log": null,
            "stderr_log": null,
            "cpu_max": null,
        });
        let resp = app
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/api/v1/processes/nullable")
                    .header("content-type", "application/json")
                    .body(Body::from(body.to_string()))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(
            resp.status(),
            StatusCode::CREATED,
            "log_format: null 应按默认值处理，CLI 未传格式时就是这种请求"
        );
        let saved = std::fs::read_to_string(dir.path().join("gugu.toml")).unwrap();
        assert!(
            saved.contains("log_format = \"auto\""),
            "落盘配置应写入默认的 auto 格式，实际:\n{saved}"
        );
        assert!(
            saved.contains("encoding = \"auto\""),
            "落盘配置应写入默认的 auto 编码，实际:\n{saved}"
        );
    }
}
