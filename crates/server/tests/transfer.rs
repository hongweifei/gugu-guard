//! 跨节点搬运集成测试：中心节点编排两台真实实例，源侧打包、目标侧解压、删源。
//!
//! 源节点是一个完整实例（白名单指向临时目录），中心节点按 `[nodes]` 配置指向它，
//! 因此打包、上传、解压、删除四步都走真实 HTTP，能观察到实际落盘结果与残留。

use std::net::SocketAddr;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use axum::body::{to_bytes, Body};
use axum::http::{Request, StatusCode};
use axum::middleware;
use axum::Router;
use gugu_core::config::{AppConfig, NodeConfig};
use gugu_core::fs::TEMP_PREFIX;
use gugu_server::nodes::NodeRegistry;
use gugu_server::state::AppState;
use serde_json::{json, Value};
use tower::ServiceExt;

const HUB_KEY: &str = "hub-secret";
const NODE_KEY: &str = "edge-secret";
const NODE_NAME: &str = "edge";

struct TempDir {
    path: PathBuf,
}

impl TempDir {
    fn new() -> Self {
        use std::sync::atomic::{AtomicU64, Ordering};
        static SEQ: AtomicU64 = AtomicU64::new(0);

        let nanos = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_nanos())
            .unwrap_or(0);
        let seq = SEQ.fetch_add(1, Ordering::Relaxed);
        let path = std::env::temp_dir().join(format!(
            "gugu-transfer-test-{}-{nanos}-{seq}",
            std::process::id()
        ));
        std::fs::create_dir_all(&path).expect("创建测试临时目录失败");
        Self { path }
    }

    fn path(&self) -> &Path {
        &self.path
    }
}

impl Drop for TempDir {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.path);
    }
}

/// 白名单路径在请求里一律用正斜杠。
fn fs_path(path: &Path) -> String {
    path.display().to_string().replace('\\', "/")
}

/// 百分号编码：中文文件名与盘符冒号都能安全进查询串。
fn encode(path: &str) -> String {
    let mut out = String::new();
    for byte in path.as_bytes() {
        let ch = char::from(*byte);
        if ch.is_ascii_alphanumeric() || matches!(ch, '-' | '_' | '.' | '~' | '/') {
            out.push(ch);
        } else {
            out.push_str(&format!("%{byte:02X}"));
        }
    }
    out
}

// ── 两台实例 ──────────────────────────────────────────────

/// 一台普通节点：白名单一个目录，写权限按搬入还是搬出分别设置。
async fn spawn_node(root: &Path, write: bool) -> SocketAddr {
    let manager = gugu_core::manager::start(&AppConfig::default(), None);
    let state = AppState::new(manager)
        .with_api_key(Some(NODE_KEY.into()))
        .with_fs(vec![root.to_path_buf()], write);
    let app = Router::new()
        .merge(
            gugu_server::api::routes().layer(middleware::from_fn_with_state(
                state.clone(),
                gugu_server::api::auth_middleware,
            )),
        )
        .with_state(state);

    let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
        .await
        .expect("绑定节点端口失败");
    let addr = listener.local_addr().expect("读取节点地址失败");
    tokio::spawn(async move {
        let _ = axum::serve(listener, app).await;
    });
    addr
}

/// 中心节点：本机白名单指向目标目录，节点表登记上面那台实例。
fn hub_app(root: &Path, peer: SocketAddr, write: bool) -> Router {
    let manager = gugu_core::manager::start(&AppConfig::default(), None);
    let registry = Arc::new(NodeRegistry::new(
        &[NodeConfig {
            name: NODE_NAME.into(),
            label: None,
            url: format!("http://{peer}"),
            api_key: Some(NODE_KEY.into()),
        }],
        Some("中心机".into()),
    ));
    let state = AppState::new(manager)
        .with_api_key(Some(HUB_KEY.into()))
        .with_fs(vec![root.to_path_buf()], write)
        .with_registry(registry);
    Router::new()
        .merge(
            gugu_server::api::routes().layer(middleware::from_fn_with_state(
                state.clone(),
                gugu_server::api::auth_middleware,
            )),
        )
        .with_state(state)
}

async fn transfer(
    app: Router,
    from: &str,
    to: &str,
    from_node: &str,
    to_node: &str,
    overwrite: bool,
) -> (StatusCode, Value) {
    let body = json!({
        "from": from,
        "to": to,
        "from_node": from_node,
        "to_node": to_node,
        "overwrite": overwrite,
    });
    let request = Request::builder()
        .method("POST")
        .uri(format!("/api/v1/fs/transfer?from={}", encode(from)))
        .header("Authorization", format!("Bearer {HUB_KEY}"))
        .header("Content-Type", "application/json")
        .body(Body::from(body.to_string()))
        .expect("构造请求失败");
    let response = app.oneshot(request).await.expect("请求处理失败");
    let status = response.status();
    let bytes = to_bytes(response.into_body(), 1 << 20)
        .await
        .expect("读取响应体失败");
    (
        status,
        serde_json::from_slice(&bytes).unwrap_or(Value::Null),
    )
}

/// 目录里不该留下搬运用的中转归档。
fn leftovers(dir: &Path) -> Vec<String> {
    let mut names: Vec<String> = std::fs::read_dir(dir)
        .map(|items| {
            items
                .filter_map(std::result::Result::ok)
                .map(|item| item.file_name().to_string_lossy().to_string())
                .filter(|name| name.starts_with(TEMP_PREFIX))
                .collect::<Vec<String>>()
        })
        .unwrap_or_default();
    names.sort();
    names
}

// ── 用例 ──────────────────────────────────────────────────

#[tokio::test]
async fn moves_a_directory_from_a_node_to_the_center() {
    let source = TempDir::new();
    let target = TempDir::new();
    let dir = source.path().join("demo");
    std::fs::create_dir_all(dir.join("sub")).expect("建目录");
    std::fs::write(dir.join("a.txt"), b"alpha").expect("写文件");
    std::fs::write(dir.join("sub/中文.txt"), b"beta").expect("写文件");

    let addr = spawn_node(source.path(), true).await;
    let app = hub_app(target.path(), addr, true);
    let landed = target.path().join("demo");

    let (status, body) = transfer(
        app,
        &fs_path(&dir),
        &fs_path(&landed),
        NODE_NAME,
        "local",
        false,
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    let message = body["message"].as_str().unwrap_or_default();
    assert!(message.contains("已搬运"), "{message}");
    assert!(message.contains("demo"), "{message}");

    // 目录内容原样落地：不套顶层目录，层级与文件名都保持归档里的样子。
    assert_eq!(std::fs::read(landed.join("a.txt")).expect("读回"), b"alpha");
    assert_eq!(
        std::fs::read(landed.join("sub/中文.txt")).expect("读回"),
        b"beta"
    );
    assert!(!dir.exists(), "删源后源目录不该还在");
    assert_eq!(
        leftovers(&landed),
        Vec::<String>::new(),
        "目标不该留下中转归档"
    );
    assert_eq!(
        leftovers(source.path()),
        Vec::<String>::new(),
        "源目录不该留下中转归档"
    );
}

#[tokio::test]
async fn moves_a_single_file_to_a_node() {
    let source = TempDir::new();
    let target = TempDir::new();
    let file = source.path().join("清单.txt");
    std::fs::write(&file, b"rows").expect("写文件");

    let addr = spawn_node(target.path(), true).await;
    let app = hub_app(source.path(), addr, true);
    let landed = target.path().join("清单.txt");

    let (status, body) = transfer(
        app,
        &fs_path(&file),
        &fs_path(&landed),
        "local",
        NODE_NAME,
        false,
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");

    assert_eq!(std::fs::read(&landed).expect("读回"), b"rows");
    assert!(!file.exists(), "源文件应被删除");
    assert_eq!(
        leftovers(target.path()),
        Vec::<String>::new(),
        "节点目录不该留下中转归档"
    );
}

#[tokio::test]
async fn refuses_same_node_and_unknown_nodes() {
    let root = TempDir::new();
    let addr = spawn_node(root.path(), true).await;
    let app = hub_app(root.path(), addr, true);

    let (status, body) = transfer(
        app.clone(),
        &fs_path(&root.path().join("a.txt")),
        &fs_path(&root.path().join("b.txt")),
        "local",
        "local",
        false,
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "{body}");
    assert!(
        body["error"]
            .as_str()
            .is_some_and(|text| text.contains("rename")),
        "同节点应指向改名端点: {body}"
    );

    let (status, body) = transfer(
        app,
        &fs_path(&root.path().join("a.txt")),
        &fs_path(&root.path().join("b.txt")),
        "web9",
        "local",
        false,
    )
    .await;
    assert_eq!(status, StatusCode::NOT_FOUND, "{body}");
    assert!(
        body["error"]
            .as_str()
            .is_some_and(|text| text.contains("web9")),
        "{body}"
    );
}

#[tokio::test]
async fn refuses_existing_target_and_missing_parent() {
    let source = TempDir::new();
    let target = TempDir::new();
    let file = source.path().join("note.txt");
    std::fs::write(&file, b"new").expect("写文件");
    let landed = target.path().join("note.txt");
    std::fs::write(&landed, b"old").expect("放一个占位文件");

    let addr = spawn_node(target.path(), true).await;
    let app = hub_app(source.path(), addr, true);

    let (status, body) = transfer(
        app.clone(),
        &fs_path(&file),
        &fs_path(&landed),
        "local",
        NODE_NAME,
        false,
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "{body}");
    assert!(
        body["error"]
            .as_str()
            .is_some_and(|text| text.contains("目标已存在")),
        "{body}"
    );
    assert_eq!(
        std::fs::read(&landed).expect("读回"),
        b"old",
        "拒绝时不该动目标"
    );
    assert!(file.is_file(), "拒绝时不该删源");

    let missing = target.path().join("没有这个目录/note.txt");
    let (status, body) = transfer(
        app.clone(),
        &fs_path(&file),
        &fs_path(&missing),
        "local",
        NODE_NAME,
        false,
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "{body}");
    assert!(
        body["error"]
            .as_str()
            .is_some_and(|text| text.contains("父目录")),
        "{body}"
    );

    let (status, body) = transfer(
        app,
        &fs_path(&file),
        &fs_path(&landed),
        "local",
        NODE_NAME,
        true,
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(
        std::fs::read(&landed).expect("读回"),
        b"new",
        "覆盖后应是新内容"
    );
    assert!(!file.exists(), "成功后删除源");
}

#[tokio::test]
async fn failing_source_removal_reports_where_the_content_landed() {
    let source = TempDir::new();
    let target = TempDir::new();
    let file = source.path().join("note.txt");
    std::fs::write(&file, b"hi").expect("写文件");

    // 源节点只读：可以打包，但不能删源。
    let addr = spawn_node(source.path(), false).await;
    let app = hub_app(target.path(), addr, true);
    let landed = target.path().join("note.txt");

    let (status, body) = transfer(
        app,
        &fs_path(&file),
        &fs_path(&landed),
        NODE_NAME,
        "local",
        false,
    )
    .await;
    assert_eq!(status, StatusCode::INTERNAL_SERVER_ERROR, "{body}");
    let message = body["error"].as_str().unwrap_or_default();
    assert!(message.contains("删除源失败"), "{message}");
    assert!(message.contains("内容已到达"), "{message}");
    assert_eq!(
        std::fs::read(&landed).expect("内容应已到达目标"),
        b"hi",
        "删源失败不影响目标侧结果"
    );
}
