//! 审计中间件：只拦「会改变状态」的请求（POST / PUT / DELETE），
//! 把动作、对象、结果、来源与耗时记进 [`gugu_core::audit::AuditStore`]。
//!
//! 记在中间件而不是每个 handler 里：新增端点不会漏记，认证失败也照样留下痕迹。

use std::net::SocketAddr;
use std::time::Instant;

use axum::body::{to_bytes, Body};
use axum::extract::{ConnectInfo, Request, State};
use axum::http::{HeaderMap, Method, Uri};
use axum::middleware::Next;
use axum::response::Response;
use chrono::Utc;
use gugu_core::audit::{AuditEntry, AuthKind};

use crate::state::AppState;

/// 错误正文最多记这么多字符，避免把整段响应塞进审计。
const MAX_DETAIL_CHARS: usize = 300;
/// 读取响应体的上限；审计只覆盖写操作，响应都是小 JSON。
const MAX_BODY_BYTES: usize = 128 * 1024;
const MAX_UA_CHARS: usize = 200;
/// 来源地址不可能有这么长，超出的部分直接截掉。
const MAX_IP_CHARS: usize = 64;

/// 一个请求对应的审计动作。
struct Action {
    code: &'static str,
    label: &'static str,
    target: Option<String>,
}

pub async fn audit_middleware(State(state): State<AppState>, req: Request, next: Next) -> Response {
    let Some(action) = classify(req.method(), req.uri()) else {
        return next.run(req).await;
    };

    let method = req.method().as_str().to_string();
    let path = req.uri().path().to_string();
    let client = client_of(&req);
    let user_agent = header_value(req.headers(), "user-agent", MAX_UA_CHARS);
    let started = Instant::now();

    let response = next.run(req).await;
    let status = response.status();
    let duration_ms = started.elapsed().as_millis() as u64;
    let auth = auth_kind(&state, status.as_u16());

    // 成功响应不读正文，直接原样返回；失败时才取一句话作为原因。
    let detail;
    let response = if status.is_success() {
        detail = None;
        response
    } else {
        let (parts, body) = response.into_parts();
        let bytes = to_bytes(body, MAX_BODY_BYTES)
            .await
            .unwrap_or_default()
            .to_vec();
        detail = failure_detail(&bytes);
        Response::from_parts(parts, Body::from(bytes))
    };

    state.audit.record(AuditEntry {
        at: Utc::now(),
        action: action.code.to_string(),
        label: action.label.to_string(),
        target: action.target,
        method,
        path,
        status: status.as_u16(),
        ok: status.is_success(),
        duration_ms,
        client,
        user_agent,
        auth,
        detail,
    });

    response
}

/// 认证结果：配了 key 且没被 401 拒绝就是带 key 的请求。
pub(crate) fn auth_kind(state: &AppState, status: u16) -> AuthKind {
    let protected = state.api_key.as_deref().is_some_and(|key| !key.is_empty());
    match (protected, status) {
        (_, 401) => AuthKind::Denied,
        (true, _) => AuthKind::Key,
        (false, _) => AuthKind::None,
    }
}

/// 取一个请求头，去掉首尾空白并按字符截断；空值当作没带。
pub(crate) fn header_value(headers: &HeaderMap, name: &str, max_chars: usize) -> Option<String> {
    let value = headers.get(name)?.to_str().ok()?.trim();
    (!value.is_empty()).then(|| truncate(value, max_chars))
}

/// 来源地址：优先代理头 `x-forwarded-for`（取第一段）、其次 `x-real-ip`。
///
/// WebSocket handler 拿不到对端地址，与中间件共用这一套，两边记的来源口径一致。
pub(crate) fn client_of_headers(headers: &HeaderMap) -> Option<String> {
    if let Some(forwarded) = headers
        .get("x-forwarded-for")
        .and_then(|value| value.to_str().ok())
    {
        if let Some(first) = forwarded.split(',').next() {
            let first = first.trim();
            if !first.is_empty() {
                return Some(first.to_string());
            }
        }
    }
    header_value(headers, "x-real-ip", MAX_IP_CHARS)
}

fn client_of(req: &Request) -> String {
    client_of_headers(req.headers()).unwrap_or_else(|| {
        req.extensions()
            .get::<ConnectInfo<SocketAddr>>()
            .map_or_else(|| "-".to_string(), |info| info.0.to_string())
    })
}

/// 从错误响应里取一句话：优先 JSON 的 error / message，其次整段正文。
fn failure_detail(bytes: &[u8]) -> Option<String> {
    let text = String::from_utf8_lossy(bytes);
    let detail = serde_json::from_str::<serde_json::Value>(&text)
        .ok()
        .and_then(|value| {
            value
                .get("error")
                .or_else(|| value.get("message"))
                .and_then(|field| field.as_str().map(str::to_string))
        })
        .unwrap_or_else(|| text.trim().to_string());
    (!detail.is_empty()).then(|| truncate(&detail, MAX_DETAIL_CHARS))
}

fn truncate(text: &str, max_chars: usize) -> String {
    if text.chars().count() <= max_chars {
        return text.to_string();
    }
    text.chars().take(max_chars).collect()
}

/// 把方法 + 路径映射成审计动作。GET / HEAD / OPTIONS 返回 None（不审计）。
///
/// 代理路径 `/api/v1/nodes/{node}/...` 剥掉节点前缀后按同一张表分类，
/// 对象前面带上节点名（`web1/api`），从节点的写操作在审计里不会和本机混在一起。
fn classify(method: &Method, uri: &Uri) -> Option<Action> {
    if !matches!(*method, Method::POST | Method::PUT | Method::DELETE) {
        return None;
    }
    let path = uri.path();
    let segments: Vec<&str> = path.trim_matches('/').split('/').collect();

    if let Some(rest) = segments.strip_prefix(&["api", "v1", "nodes"][..]) {
        if let Some((node, inner)) = rest.split_first() {
            if inner.is_empty() {
                return Some(Action {
                    code: "http.write",
                    label: "写操作",
                    target: Some((*node).to_string()),
                });
            }
            let action = classify_segments(method, inner)?;
            let target = Some(match action.target {
                Some(target) => format!("{node}/{target}"),
                None => (*node).to_string(),
            });
            return Some(Action {
                code: action.code,
                label: action.label,
                target,
            });
        }
    }

    let mut action = classify_segments(method, &segments)?;
    // 文件操作的地址在查询串里，把它带进来，审计才能看出动了哪个路径。
    if action.target.is_none() {
        action.target = query_path(uri);
    }
    Some(action)
}

/// 按资源段分类；`segments` 可以带也可以不带 `/api/v1` 前缀。
fn classify_segments(method: &Method, segments: &[&str]) -> Option<Action> {
    let rest = segments
        .strip_prefix(&["api", "v1"][..])
        .unwrap_or(segments);

    let action = match (method.as_str(), rest) {
        ("POST", ["processes", _]) => Some(("process.create", "添加进程")),
        ("PUT", ["processes", _]) => Some(("process.update", "更新进程")),
        ("DELETE", ["processes", _]) => Some(("process.delete", "删除进程")),
        ("POST", ["processes", _, "start"]) => Some(("process.start", "启动进程")),
        ("POST", ["processes", _, "stop"]) => Some(("process.stop", "停止进程")),
        ("POST", ["processes", _, "restart"]) => Some(("process.restart", "重启进程")),
        ("POST", ["processes", _, "health"]) => Some(("process.health", "健康检查")),
        ("DELETE", ["processes", _, "logs"]) => Some(("logs.clear", "清空日志")),
        ("POST", ["groups", _, "start"]) => Some(("group.start", "启动分组")),
        ("POST", ["groups", _, "stop"]) => Some(("group.stop", "停止分组")),
        ("POST", ["groups", _, "restart"]) => Some(("group.restart", "重启分组")),
        ("POST", ["reload"]) => Some(("config.reload", "重新加载配置")),
        ("POST", ["shutdown"]) => Some(("daemon.shutdown", "停止守护进程")),
        ("POST", ["fs", "upload"]) => Some(("fs.upload", "上传文件")),
        ("POST", ["fs", "upload", "session"]) => Some(("fs.upload.begin", "开始分片上传")),
        ("PUT", ["fs", "upload", "session", _]) => Some(("fs.upload.chunk", "上传分片")),
        ("POST", ["fs", "upload", "session", _, "finish"]) => Some(("fs.upload", "完成分片上传")),
        ("DELETE", ["fs", "upload", "session", _]) => Some(("fs.upload.abort", "放弃上传")),
        ("PUT", ["fs", "write"]) => Some(("fs.write", "保存文件")),
        ("POST", ["fs", "extract"]) => Some(("fs.extract", "解压归档")),
        ("POST", ["fs", "mkdir"]) => Some(("fs.mkdir", "新建目录")),
        ("POST", ["fs", "touch"]) => Some(("fs.create", "新建文件")),
        ("POST", ["fs", "rename"]) => Some(("fs.rename", "移动或重命名")),
        ("POST", ["fs", "transfer"]) => Some(("fs.transfer", "跨节点搬运")),
        ("POST", ["fs", "copy"]) => Some(("fs.copy", "复制文件")),
        ("DELETE", ["fs"]) => Some(("fs.delete", "删除文件")),
        // 以后新增的写接口至少会留下痕迹，不会静默漏记。
        _ => Some(("http.write", "写操作")),
    }?;

    let target = match rest {
        ["processes" | "groups", _, ..] => rest.get(1).map(ToString::to_string),
        _ => None,
    };
    Some(Action {
        code: action.0,
        label: action.1,
        target,
    })
}

/// 从查询串里取文件路径：优先 `path`，其次改名/复制用的 `from`。
fn query_path(uri: &Uri) -> Option<String> {
    let query = uri.query()?;
    let mut found = None;
    for pair in query.split('&') {
        let Some((name, value)) = pair.split_once('=') else {
            continue;
        };
        if matches!(name, "path" | "from") && !value.is_empty() {
            found = Some(percent_decode(value));
            if name == "path" {
                break;
            }
        }
    }
    found
}

fn percent_decode(value: &str) -> String {
    let bytes = value.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut index = 0;
    while index < bytes.len() {
        match bytes[index] {
            b'%' if index + 2 < bytes.len() => {
                let hex = &value[index + 1..index + 3];
                match u8::from_str_radix(hex, 16) {
                    Ok(byte) => {
                        out.push(byte);
                        index += 3;
                    }
                    Err(_) => {
                        out.push(bytes[index]);
                        index += 1;
                    }
                }
            }
            b'+' => {
                out.push(b' ');
                index += 1;
            }
            byte => {
                out.push(byte);
                index += 1;
            }
        }
    }
    String::from_utf8_lossy(&out).into_owned()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn maps_process_operations_to_actions() {
        let cases = [
            (
                Method::POST,
                "/api/v1/processes/web",
                "process.create",
                Some("web"),
            ),
            (
                Method::PUT,
                "/api/v1/processes/web",
                "process.update",
                Some("web"),
            ),
            (
                Method::DELETE,
                "/api/v1/processes/web",
                "process.delete",
                Some("web"),
            ),
            (
                Method::POST,
                "/api/v1/processes/web/start",
                "process.start",
                Some("web"),
            ),
            (
                Method::POST,
                "/api/v1/processes/web/stop",
                "process.stop",
                Some("web"),
            ),
            (
                Method::POST,
                "/api/v1/processes/web/restart",
                "process.restart",
                Some("web"),
            ),
            (
                Method::POST,
                "/api/v1/processes/web/health",
                "process.health",
                Some("web"),
            ),
            (
                Method::DELETE,
                "/api/v1/processes/web/logs",
                "logs.clear",
                Some("web"),
            ),
            (
                Method::POST,
                "/api/v1/groups/api/restart",
                "group.restart",
                Some("api"),
            ),
            (Method::POST, "/api/v1/reload", "config.reload", None),
            (Method::POST, "/api/v1/shutdown", "daemon.shutdown", None),
            (Method::POST, "/api/v1/unknown", "http.write", None),
        ];
        for (method, path, code, target) in cases {
            let action = classify(&method, &uri(path)).expect("写操作应被审计");
            assert_eq!(action.code, code, "{method} {path}");
            assert_eq!(action.target.as_deref(), target, "{method} {path}");
        }
    }

    #[test]
    fn reads_are_not_audited() {
        assert!(classify(&Method::GET, &uri("/api/v1/processes")).is_none());
        assert!(classify(&Method::OPTIONS, &uri("/api/v1/processes/web")).is_none());
        assert!(classify(&Method::HEAD, &uri("/api/v1/processes")).is_none());
    }

    #[test]
    fn node_prefixed_writes_carry_the_node_in_target() {
        let action = classify(
            &Method::POST,
            &uri("/api/v1/nodes/web1/processes/api/start"),
        )
        .expect("代理写操作应被审计");
        assert_eq!(action.code, "process.start");
        assert_eq!(action.target.as_deref(), Some("web1/api"));

        let reload =
            classify(&Method::POST, &uri("/api/v1/nodes/web1/reload")).expect("代理重载应被审计");
        assert_eq!(reload.code, "config.reload");
        assert_eq!(reload.target.as_deref(), Some("web1"));

        assert!(classify(&Method::GET, &uri("/api/v1/nodes/web1/processes")).is_none());
        assert!(classify(&Method::GET, &uri("/api/v1/nodes")).is_none());
    }

    #[test]
    fn file_operations_carry_the_decoded_path() {
        let cases: [(Method, &str, &str, Option<&str>); 5] = [
            (
                Method::POST,
                "/api/v1/fs/upload?path=%2Fsrv%2Fa%20b.txt&overwrite=true",
                "fs.upload",
                Some("/srv/a b.txt"),
            ),
            (Method::PUT, "/api/v1/fs/write", "fs.write", None),
            (
                Method::POST,
                "/api/v1/fs/mkdir?path=/srv/logs",
                "fs.mkdir",
                Some("/srv/logs"),
            ),
            (
                Method::POST,
                "/api/v1/fs/rename?from=/srv/a.txt&to=/srv/b.txt",
                "fs.rename",
                Some("/srv/a.txt"),
            ),
            (
                Method::DELETE,
                "/api/v1/fs?path=/srv/old&recursive=true",
                "fs.delete",
                Some("/srv/old"),
            ),
        ];
        for (method, target_uri, code, target) in cases {
            let action = classify(&method, &uri(target_uri)).expect("文件写操作应被审计");
            assert_eq!(action.code, code, "{target_uri}");
            assert_eq!(action.target.as_deref(), target, "{target_uri}");
        }
    }

    #[test]
    fn chunk_uploads_are_audited_by_stage() {
        let cases: [(Method, &str, &str, Option<&str>); 4] = [
            (
                Method::POST,
                "/api/v1/fs/upload/session?path=/srv/big.bin",
                "fs.upload.begin",
                Some("/srv/big.bin"),
            ),
            (
                Method::PUT,
                "/api/v1/fs/upload/session/abc?offset=0&path=/srv/big.bin",
                "fs.upload.chunk",
                Some("/srv/big.bin"),
            ),
            (
                Method::POST,
                "/api/v1/fs/upload/session/abc/finish",
                "fs.upload",
                None,
            ),
            (
                Method::DELETE,
                "/api/v1/fs/upload/session/abc",
                "fs.upload.abort",
                None,
            ),
        ];
        for (method, target_uri, code, target) in cases {
            let action = classify(&method, &uri(target_uri)).expect("分片上传应被审计");
            assert_eq!(action.code, code, "{target_uri}");
            assert_eq!(action.target.as_deref(), target, "{target_uri}");
        }
    }

    fn uri(value: &str) -> Uri {
        value.parse().expect("测试用 URI 应合法")
    }

    #[test]
    fn failure_detail_prefers_json_error_field() {
        let bytes = r#"{"error":"启动进程 'web' 失败: 端口 8080 被占用"}"#.as_bytes();
        assert_eq!(
            failure_detail(bytes).as_deref(),
            Some("启动进程 'web' 失败: 端口 8080 被占用")
        );
        assert_eq!(failure_detail(b"  "), None);
        let plain = b"plain failure";
        assert_eq!(failure_detail(plain).as_deref(), Some("plain failure"));
    }

    #[test]
    fn truncation_counts_chars_not_bytes() {
        let text = "咕".repeat(MAX_DETAIL_CHARS + 5);
        let cut = truncate(&text, MAX_DETAIL_CHARS);
        assert_eq!(cut.chars().count(), MAX_DETAIL_CHARS);
        assert_eq!(truncate("短", 10), "短");
    }
}
