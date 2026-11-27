//! 审计中间件：只拦「会改变状态」的请求（POST / PUT / DELETE），
//! 把动作、对象、结果、来源与耗时记进 [`gugu_core::audit::AuditStore`]。
//!
//! 记在中间件而不是每个 handler 里：新增端点不会漏记，认证失败也照样留下痕迹。

use std::net::SocketAddr;
use std::time::Instant;

use axum::body::{to_bytes, Body};
use axum::extract::{ConnectInfo, Request, State};
use axum::http::{Method, StatusCode};
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

/// 一个请求对应的审计动作。
struct Action {
    code: &'static str,
    label: &'static str,
    target: Option<String>,
}

pub async fn audit_middleware(State(state): State<AppState>, req: Request, next: Next) -> Response {
    let Some(action) = classify(req.method(), req.uri().path()) else {
        return next.run(req).await;
    };

    let method = req.method().as_str().to_string();
    let path = req.uri().path().to_string();
    let client = client_of(&req);
    let user_agent = header_of(&req, "user-agent").map(|ua| truncate(&ua, MAX_UA_CHARS));
    let started = Instant::now();

    let response = next.run(req).await;
    let status = response.status();
    let duration_ms = started.elapsed().as_millis() as u64;
    let auth = auth_kind(&state, status);

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
fn auth_kind(state: &AppState, status: StatusCode) -> AuthKind {
    let protected = state.api_key.as_deref().is_some_and(|key| !key.is_empty());
    match (protected, status) {
        (_, StatusCode::UNAUTHORIZED) => AuthKind::Denied,
        (true, _) => AuthKind::Key,
        (false, _) => AuthKind::None,
    }
}

fn header_of(req: &Request, name: &str) -> Option<String> {
    req.headers()
        .get(name)
        .and_then(|value| value.to_str().ok())
        .map(str::to_string)
}

/// 来源地址：优先代理头，其次连接的对端地址（未开启 ConnectInfo 时记 "-"）。
fn client_of(req: &Request) -> String {
    if let Some(forwarded) = header_of(req, "x-forwarded-for") {
        if let Some(first) = forwarded.split(',').next() {
            let trimmed = first.trim();
            if !trimmed.is_empty() {
                return trimmed.to_string();
            }
        }
    }
    if let Some(real) = header_of(req, "x-real-ip") {
        let trimmed = real.trim();
        if !trimmed.is_empty() {
            return trimmed.to_string();
        }
    }
    req.extensions()
        .get::<ConnectInfo<SocketAddr>>()
        .map_or_else(|| "-".to_string(), |info| info.0.to_string())
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
fn classify(method: &Method, path: &str) -> Option<Action> {
    if !matches!(*method, Method::POST | Method::PUT | Method::DELETE) {
        return None;
    }
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

    classify_segments(method, &segments)
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
            let action = classify(&method, path).expect("写操作应被审计");
            assert_eq!(action.code, code, "{method} {path}");
            assert_eq!(action.target.as_deref(), target, "{method} {path}");
        }
    }

    #[test]
    fn reads_are_not_audited() {
        assert!(classify(&Method::GET, "/api/v1/processes").is_none());
        assert!(classify(&Method::OPTIONS, "/api/v1/processes/web").is_none());
        assert!(classify(&Method::HEAD, "/api/v1/processes").is_none());
    }

    #[test]
    fn node_prefixed_writes_carry_the_node_in_target() {
        let action = classify(&Method::POST, "/api/v1/nodes/web1/processes/api/start")
            .expect("代理写操作应被审计");
        assert_eq!(action.code, "process.start");
        assert_eq!(action.target.as_deref(), Some("web1/api"));

        let reload =
            classify(&Method::POST, "/api/v1/nodes/web1/reload").expect("代理重载应被审计");
        assert_eq!(reload.code, "config.reload");
        assert_eq!(reload.target.as_deref(), Some("web1"));

        assert!(classify(&Method::GET, "/api/v1/nodes/web1/processes").is_none());
        assert!(classify(&Method::GET, "/api/v1/nodes").is_none());
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
