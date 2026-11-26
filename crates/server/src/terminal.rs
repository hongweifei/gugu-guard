//! WebUI 内置终端：把浏览器的 WebSocket 接到一个真实 PTY。
//!
//! 这里不做逐行命令执行，而是让 shell 自己跑在伪终端里，因此 `cd`、分页器、
//! 彩色输出、交互式程序都能像本地终端一样工作。每个连接一个会话，彼此独立。
//!
//! 安全边界：终端默认关闭；路由挂在 `protected` 下，自动继承 API Key 认证；
//! 监听非回环地址且未配置 API Key 时，CLI 会在启动阶段拒绝运行。

use std::net::IpAddr;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::{Duration, Instant};

use axum::extract::ws::{Message, WebSocket, WebSocketUpgrade};
use axum::extract::State;
use axum::http::{HeaderMap, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::routing::get;
use axum::{Json, Router};
use chrono::Utc;
use gugu_core::audit::{AuditEntry, AuthKind};
use portable_pty::{native_pty_system, Child, CommandBuilder, MasterPty, PtySize};
use serde_json::{json, Value};
use tokio::sync::mpsc;

use crate::state::AppState;

/// 同时打开的终端会话上限。
pub const MAX_SESSIONS: usize = 4;
/// 无输入也无输出多久后自动结束会话。
pub const IDLE_TIMEOUT: Duration = Duration::from_secs(30 * 60);
/// 读取 PTY 的缓冲区大小。
const READ_BUF: usize = 8192;
/// WebSocket 会话与 PTY 读取线程之间的积压上限。
const OUTPUT_QUEUE: usize = 64;

static ACTIVE_SESSIONS: AtomicUsize = AtomicUsize::new(0);

pub fn routes() -> Router<AppState> {
    Router::new()
        .route("/api/v1/terminal", get(info_handler))
        .route("/api/v1/terminal/ws", get(ws_handler))
}

/// 终端是否可用；关闭时返回 `enabled:false`，前端据此提示而不是静默隐藏。
async fn info_handler(State(state): State<AppState>) -> Json<Value> {
    if !state.terminal_enabled {
        return Json(json!({ "enabled": false }));
    }
    Json(json!({
        "enabled": true,
        "shell": shell_program(),
        "cwd": state.terminal_cwd.as_deref().unwrap_or_else(|| Path::new(".")).display().to_string(),
        "max_sessions": MAX_SESSIONS,
        "idle_timeout_secs": IDLE_TIMEOUT.as_secs(),
    }))
}

/// 未启用时在升级前直接 403，避免浏览器先建立连接再收到一屏错误。
async fn ws_handler(
    ws: WebSocketUpgrade,
    State(state): State<AppState>,
    headers: HeaderMap,
) -> Response {
    if !state.terminal_enabled {
        return (
            StatusCode::FORBIDDEN,
            Json(json!({ "error": "终端未启用" })),
        )
            .into_response();
    }
    // WebSocket 不受浏览器同源策略约束：没有这道检查，任何网页都能连本机终端。
    let has_key = state.api_key.as_deref().is_some_and(|key| !key.is_empty());
    if !origin_allowed(&headers, has_key) {
        tracing::warn!("拒绝跨站终端 WebSocket 请求");
        return (
            StatusCode::FORBIDDEN,
            Json(json!({ "error": "终端只接受同源页面连接" })),
        )
            .into_response();
    }
    let Some(guard) = SessionGuard::acquire() else {
        return (
            StatusCode::TOO_MANY_REQUESTS,
            Json(json!({ "error": format!("终端会话已达上限（{MAX_SESSIONS} 个）") })),
        )
            .into_response();
    };
    ws.on_upgrade(move |socket| handle_socket(socket, state, headers, guard))
}

/// 会话名额，Drop 时归还，保证异常路径也不会泄漏名额。
struct SessionGuard;

impl SessionGuard {
    fn acquire() -> Option<Self> {
        let mut current = ACTIVE_SESSIONS.load(Ordering::SeqCst);
        loop {
            if current >= MAX_SESSIONS {
                return None;
            }
            match ACTIVE_SESSIONS.compare_exchange(
                current,
                current + 1,
                Ordering::SeqCst,
                Ordering::SeqCst,
            ) {
                Ok(_) => return Some(Self),
                Err(actual) => current = actual,
            }
        }
    }
}

impl Drop for SessionGuard {
    fn drop(&mut self) {
        ACTIVE_SESSIONS.fetch_sub(1, Ordering::SeqCst);
    }
}

struct Session {
    socket: WebSocket,
    pty: Pty,
    /// 会话起点，用于关闭审计里记时长。
    started: Instant,
}

/// 已启动的 PTY 及其读写端。
struct Pty {
    master: Box<dyn MasterPty + Send>,
    writer: Box<dyn std::io::Write + Send>,
    child: Box<dyn Child + Send + Sync>,
    output_rx: mpsc::Receiver<Vec<u8>>,
    shell: String,
}

impl Drop for Pty {
    fn drop(&mut self) {
        // 无论正常结束、客户端断开还是任务被取消，都不留下孤立的 shell。
        let _ = self.child.kill();
    }
}

async fn handle_socket(
    mut socket: WebSocket,
    state: AppState,
    headers: HeaderMap,
    guard: SessionGuard,
) {
    let cwd = state
        .terminal_cwd
        .clone()
        .unwrap_or_else(|| PathBuf::from("."));
    let (shell, args) = shell_command();
    let shell_name = Path::new(&shell)
        .file_name()
        .map_or_else(|| shell.clone(), |name| name.to_string_lossy().to_string());

    let spawned = spawn_pty(&shell, &args, &cwd);
    let client = client_of(&headers);
    let user_agent = header_value(&headers, "user-agent");

    let mut session = match spawned {
        Ok(pty) => {
            record_audit(
                &state,
                "terminal.open",
                "打开终端",
                StatusCode::SWITCHING_PROTOCOLS.as_u16(),
                true,
                client.clone(),
                user_agent.clone(),
                Some(format!("{shell_name} @ {}", cwd.display())),
            );
            Session {
                socket,
                pty,
                started: Instant::now(),
            }
        }
        Err(error) => {
            tracing::warn!("创建终端会话失败: {error:#}");
            record_audit(
                &state,
                "terminal.open",
                "打开终端",
                StatusCode::INTERNAL_SERVER_ERROR.as_u16(),
                false,
                client,
                user_agent,
                Some(format!("{shell_name} 启动失败: {error}")),
            );
            let _ = send_json(
                &mut socket,
                json!({ "type": "error", "message": format!("无法启动 shell: {error}") }),
            )
            .await;
            let _ = socket.send(Message::Close(None)).await;
            drop(guard);
            return;
        }
    };

    let _ = send_json(
        &mut session.socket,
        json!({
            "type": "ready",
            "shell": session.pty.shell,
            "cwd": cwd.display().to_string(),
            "max_sessions": MAX_SESSIONS,
            "idle_timeout_secs": IDLE_TIMEOUT.as_secs(),
        }),
    )
    .await;

    let outcome = run_session(&mut session, &state).await;
    let elapsed = session.started.elapsed();

    record_audit(
        &state,
        "terminal.close",
        "关闭终端",
        StatusCode::OK.as_u16(),
        true,
        client_of(&headers),
        header_value(&headers, "user-agent"),
        Some(format!(
            "{}，时长 {} 秒，shell {}",
            outcome,
            elapsed.as_secs(),
            session.pty.shell
        )),
    );

    let _ = session.socket.send(Message::Close(None)).await;
    drop(session);
    drop(guard);
}

/// 会话主循环：浏览器输入 → PTY，PTY 输出 → 浏览器，直到任一侧结束或空闲超时。
async fn run_session(session: &mut Session, state: &AppState) -> &'static str {
    let idle = tokio::time::sleep(IDLE_TIMEOUT);
    tokio::pin!(idle);
    let mut heartbeat = tokio::time::interval(Duration::from_secs(30));
    // PTY 按字节流返回，UTF-8 多字节字符可能被切到两个块里；保留未完成的后缀。
    let mut pending_output = Vec::new();

    loop {
        tokio::select! {
            message = session.socket.recv() => {
                match message {
                    Some(Ok(Message::Text(text))) => {
                        idle.as_mut().reset(tokio::time::Instant::now() + IDLE_TIMEOUT);
                        if handle_client_message(session, text.as_str()).await.is_err() {
                            return "客户端断开";
                        }
                    }
                    Some(Ok(Message::Binary(bytes))) => {
                        idle.as_mut().reset(tokio::time::Instant::now() + IDLE_TIMEOUT);
                        if let Err(error) = session.pty.writer.write_all(&bytes) {
                            tracing::debug!("终端写入失败: {error}");
                            return "PTY 写入失败";
                        }
                    }
                    Some(Ok(Message::Close(_)) | Err(_)) | None => return "客户端断开",
                    Some(Ok(_)) => {}
                }
            }
            chunk = session.pty.output_rx.recv() => {
                let Some(chunk) = chunk else {
                    // 收尾时把不完整的尾巴按有损方式送出，至少不吞掉最后的提示符。
                    let rest = String::from_utf8_lossy(&pending_output).to_string();
                    if !rest.is_empty() {
                        let _ = send_json(
                            &mut session.socket,
                            json!({ "type": "data", "data": rest }),
                        ).await;
                    }
                    let code = session
                        .pty
                        .child
                        .try_wait()
                        .ok()
                        .flatten()
                        .map(|status| status.exit_code());
                    let _ = send_json(
                        &mut session.socket,
                        json!({ "type": "exit", "code": code, "reason": "shell 已退出" }),
                    ).await;
                    return "shell 已退出";
                };
                idle.as_mut().reset(tokio::time::Instant::now() + IDLE_TIMEOUT);
                pending_output.extend_from_slice(&chunk);
                let text = take_utf8_prefix(&mut pending_output);
                if text.is_empty() {
                    continue;
                }
                if send_json(&mut session.socket, json!({ "type": "data", "data": text }))
                    .await
                    .is_err()
                {
                    return "客户端断开";
                }
            }
            () = &mut idle => {
                let _ = send_json(
                    &mut session.socket,
                    json!({ "type": "exit", "code": null, "reason": "空闲超时" }),
                ).await;
                tracing::info!("终端会话空闲超时，已结束（{}）", state.terminal_cwd.as_deref().unwrap_or_else(|| Path::new(".")).display());
                return "空闲超时";
            }
            _ = heartbeat.tick() => {
                if session.socket.send(Message::Ping(Vec::new().into())).await.is_err() {
                    return "客户端断开";
                }
            }
        }
    }
}

/// 处理客户端 JSON 消息；返回 Err 表示应结束会话。
async fn handle_client_message(session: &mut Session, text: &str) -> Result<(), ()> {
    let Ok(message) = serde_json::from_str::<Value>(text) else {
        return Ok(());
    };
    match message.get("type").and_then(Value::as_str) {
        Some("input") => {
            let data = message
                .get("data")
                .and_then(Value::as_str)
                .unwrap_or_default();
            if data.is_empty() {
                return Ok(());
            }
            session
                .pty
                .writer
                .write_all(data.as_bytes())
                .map_err(|error| {
                    tracing::debug!("终端写入失败: {error}");
                })
        }
        Some("resize") => {
            let cols = message
                .get("cols")
                .and_then(Value::as_u64)
                .unwrap_or(80)
                .clamp(1, 1000) as u16;
            let rows = message
                .get("rows")
                .and_then(Value::as_u64)
                .unwrap_or(24)
                .clamp(1, 1000) as u16;
            // 尺寸只影响排版，失败不必断开会话。
            if let Err(error) = session.pty.master.resize(PtySize {
                rows,
                cols,
                pixel_width: 0,
                pixel_height: 0,
            }) {
                tracing::debug!("调整终端尺寸失败: {error}");
            }
            Ok(())
        }
        _ => Ok(()),
    }
}

fn spawn_pty(shell: &str, args: &[&str], cwd: &Path) -> anyhow::Result<Pty> {
    let pair = native_pty_system().openpty(PtySize {
        rows: 24,
        cols: 80,
        pixel_width: 0,
        pixel_height: 0,
    })?;

    let mut command = CommandBuilder::new(shell);
    for arg in args {
        command.arg(arg);
    }
    command.cwd(cwd);
    command.env("TERM", "xterm-256color");
    command.env("COLORTERM", "truecolor");

    let mut child = pair.slave.spawn_command(command)?;
    // slave 端交给子进程后即可关闭，否则子进程退出后 master 可能读不到 EOF。
    drop(pair.slave);

    let reader = pair.master.try_clone_reader()?;
    let writer = pair.master.take_writer()?;

    let (output_tx, output_rx) = mpsc::channel::<Vec<u8>>(OUTPUT_QUEUE);
    std::thread::spawn(move || {
        let mut reader = reader;
        let mut buf = [0u8; READ_BUF];
        loop {
            match std::io::Read::read(&mut reader, &mut buf) {
                Ok(0) => break,
                Ok(n) => {
                    if output_tx.blocking_send(buf[..n].to_vec()).is_err() {
                        break;
                    }
                }
                Err(error) => {
                    tracing::debug!("读取终端输出结束: {error}");
                    break;
                }
            }
        }
        // 关闭发送端，让会话循环收到 None 并收尾。
        drop(output_tx);
    });

    // 进程退出码延迟到关闭审计时才用，先探测一次避免留下僵尸。
    let _ = child.try_wait();

    let shell_name = Path::new(shell).file_name().map_or_else(
        || shell.to_string(),
        |name| name.to_string_lossy().to_string(),
    );

    Ok(Pty {
        master: pair.master,
        writer,
        child,
        output_rx,
        shell: shell_name,
    })
}

fn shell_command() -> (String, Vec<&'static str>) {
    #[cfg(windows)]
    {
        let pwsh = find_in_path("pwsh.exe");
        let shell = pwsh.unwrap_or_else(|| "powershell.exe".to_string());
        (shell, vec!["-NoLogo"])
    }
    #[cfg(not(windows))]
    {
        let shell = std::env::var("SHELL")
            .ok()
            .filter(|shell| !shell.trim().is_empty())
            .unwrap_or_else(|| "/bin/bash".to_string());
        (shell, Vec::new())
    }
}

fn shell_program() -> String {
    let (shell, _) = shell_command();
    Path::new(&shell)
        .file_name()
        .map_or(shell.clone(), |name| name.to_string_lossy().to_string())
}

#[cfg(windows)]
fn find_in_path(name: &str) -> Option<String> {
    let path = std::env::var_os("PATH")?;
    std::env::split_paths(&path)
        .map(|dir| dir.join(name))
        .find(|candidate| candidate.is_file())
        .map(|candidate| candidate.to_string_lossy().to_string())
}

/// 绑定地址是否只对本机开放。`0.0.0.0`、局域网地址与具体公网地址都算对外。
#[must_use]
pub fn is_loopback(addr: IpAddr) -> bool {
    addr.is_loopback()
}

/// CLI 启动前的安全校验：开启终端、监听非回环、又没有 API Key 时拒绝启动。
///
/// # Errors
/// 返回人类可读的拒绝原因，调用方直接把它作为启动失败信息。
pub fn check_exposure(addr: IpAddr, api_key: Option<&str>) -> Result<(), String> {
    let has_key = api_key.is_some_and(|key| !key.trim().is_empty());
    if is_loopback(addr) || has_key {
        return Ok(());
    }
    Err(format!(
        "已启用 WebUI 终端（daemon.web.terminal = true），但监听地址 {addr} 不是回环地址且未配置 API Key。\
         终端等于把 shell 交给任何能访问面板的人：请配置 daemon.api_key（或 GUGU_API_KEY），或只监听 127.0.0.1。"
    ))
}

async fn send_json(socket: &mut WebSocket, value: Value) -> Result<(), ()> {
    let text = serde_json::to_string(&value).map_err(|_| ())?;
    socket
        .send(Message::Text(text.into()))
        .await
        .map_err(|_| ())
}

fn header_value(headers: &HeaderMap, name: &str) -> Option<String> {
    headers
        .get(name)
        .and_then(|value| value.to_str().ok())
        .map(|value| value.chars().take(200).collect())
}

/// 浏览器 WebSocket 握手会带 `Origin`。只接受与 `Host` 完全一致的来源；
/// 没有 API Key 时还要求 Host 本身是回环，挡住 DNS rebinding 把恶意域名指向 127.0.0.1。
/// 没有 `Origin` 的客户端（CLI、脚本）不受同源限制，但仍受 API Key 认证约束。
fn origin_allowed(headers: &HeaderMap, has_key: bool) -> bool {
    let Some(origin) = headers.get("origin").and_then(|value| value.to_str().ok()) else {
        return true;
    };
    let Some(host) = headers.get("host").and_then(|value| value.to_str().ok()) else {
        return false;
    };
    let Some(origin_host) = origin_authority(origin) else {
        return false;
    };
    let host = host.to_ascii_lowercase();
    if origin_host != host {
        return false;
    }
    has_key || is_loopback_host(&host)
}

/// 从 `http://host:port` 取出小写 `host:port`；带路径或 `null` 的来源一律拒绝。
fn origin_authority(origin: &str) -> Option<String> {
    let rest = origin
        .strip_prefix("http://")
        .or_else(|| origin.strip_prefix("https://"))?;
    if rest.contains('/') || rest.is_empty() {
        return None;
    }
    Some(rest.to_ascii_lowercase())
}

/// Host 是否只指向本机（含端口）；IPv6 形如 `[::1]:9090`。
fn is_loopback_host(host: &str) -> bool {
    let without_port = if let Some(rest) = host.strip_prefix('[') {
        rest.split(']').next().map(|ip| format!("[{ip}]"))
    } else {
        host.rsplit_once(':').map_or_else(
            || Some(host.to_string()),
            |(name, port)| port.parse::<u16>().ok().map(|_| name.to_string()),
        )
    };
    let Some(name) = without_port else {
        return false;
    };
    let name = name.trim_matches(['[', ']']);
    name == "localhost" || name.parse::<IpAddr>().is_ok_and(|ip| ip.is_loopback())
}

/// 与审计中间件同口径：优先代理头，其次对端地址（WS handler 没有 ConnectInfo，记 "-"）。
fn client_of(headers: &HeaderMap) -> String {
    if let Some(forwarded) = headers
        .get("x-forwarded-for")
        .and_then(|value| value.to_str().ok())
    {
        if let Some(first) = forwarded.split(',').next() {
            let trimmed = first.trim();
            if !trimmed.is_empty() {
                return trimmed.to_string();
            }
        }
    }
    if let Some(real) = headers
        .get("x-real-ip")
        .and_then(|value| value.to_str().ok())
    {
        let trimmed = real.trim();
        if !trimmed.is_empty() {
            return trimmed.to_string();
        }
    }
    "-".to_string()
}

#[allow(clippy::too_many_arguments)]
fn record_audit(
    state: &AppState,
    action: &str,
    label: &str,
    status: u16,
    ok: bool,
    client: String,
    user_agent: Option<String>,
    detail: Option<String>,
) {
    let auth = if state.api_key.as_deref().is_some_and(|key| !key.is_empty()) {
        AuthKind::Key
    } else {
        AuthKind::None
    };
    state.audit.record(AuditEntry {
        at: Utc::now(),
        action: action.to_string(),
        label: label.to_string(),
        target: None,
        method: "GET".to_string(),
        path: "/api/v1/terminal/ws".to_string(),
        status,
        ok,
        duration_ms: 0,
        client,
        user_agent,
        auth,
        detail,
    });
}

/// 取出字节流里已经完整的 UTF-8 前缀，未完成的多字节字符留在末尾等下一块。
/// 真遇到非法字节时用替换字符顶替，避免一个坏字节吞掉后面的输出。
fn take_utf8_prefix(bytes: &mut Vec<u8>) -> String {
    let mut text = String::new();
    loop {
        match std::str::from_utf8(bytes) {
            Ok(all) => {
                text.push_str(all);
                bytes.clear();
                break;
            }
            Err(error) => {
                let valid = error.valid_up_to();
                if valid > 0 {
                    if let Ok(prefix) = std::str::from_utf8(&bytes[..valid]) {
                        text.push_str(prefix);
                    }
                    bytes.drain(..valid);
                }
                match error.error_len() {
                    Some(len) => {
                        text.push('\u{FFFD}');
                        bytes.drain(..len);
                    }
                    None => break,
                }
            }
        }
    }
    text
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn loopback_only_addresses_are_trusted() {
        for addr in ["127.0.0.1", "127.0.0.2", "::1"] {
            let ip: IpAddr = addr.parse().unwrap();
            assert!(is_loopback(ip), "{addr} 应视为回环");
            assert!(check_exposure(ip, None).is_ok());
        }
    }

    #[test]
    fn websocket_origin_must_match_host() {
        let map = |origin: Option<&'static str>, host: &'static str| {
            let mut headers = HeaderMap::new();
            if let Some(origin) = origin {
                headers.insert("origin", origin.parse().unwrap());
            }
            headers.insert("host", host.parse().unwrap());
            headers
        };

        assert!(
            origin_allowed(&map(None, "127.0.0.1:9090"), false),
            "脚本客户端无 Origin"
        );
        assert!(origin_allowed(
            &map(Some("http://127.0.0.1:9090"), "127.0.0.1:9090"),
            false
        ));
        assert!(origin_allowed(
            &map(Some("http://localhost:9090"), "localhost:9090"),
            false
        ));
        assert!(origin_allowed(
            &map(Some("http://[::1]:9090"), "[::1]:9090"),
            false
        ));

        assert!(!origin_allowed(
            &map(Some("http://evil.example"), "127.0.0.1:9090"),
            false
        ));
        assert!(
            !origin_allowed(&map(Some("https://gugu.example"), "gugu.example"), false),
            "无 Key 时 DNS rebinding 域名必须被挡住"
        );
        assert!(
            origin_allowed(&map(Some("https://gugu.example"), "gugu.example"), true),
            "有 Key 时允许非回环域名同源访问"
        );
        assert!(!origin_allowed(&map(Some("null"), "127.0.0.1:9090"), false));
        assert!(!origin_allowed(
            &map(Some("http://127.0.0.1:9090/path"), "127.0.0.1:9090"),
            true
        ));
        assert!(!origin_allowed(
            &map(Some("http://192.168.1.5:9090"), "192.168.1.5:9090"),
            false
        ));
        assert!(origin_allowed(
            &map(Some("http://192.168.1.5:9090"), "192.168.1.5:9090"),
            true
        ));
    }

    #[test]
    fn public_bind_without_key_is_rejected() {
        for addr in ["0.0.0.0", "192.168.1.10", "::"] {
            let ip: IpAddr = addr.parse().unwrap();
            let error = check_exposure(ip, None).unwrap_err();
            assert!(error.contains("api_key"), "{error}");
            let error = check_exposure(ip, Some("   ")).unwrap_err();
            assert!(error.contains("api_key"), "{error}");
        }
    }

    #[test]
    fn public_bind_with_key_is_allowed() {
        let ip: IpAddr = "0.0.0.0".parse().unwrap();
        assert!(check_exposure(ip, Some("secret")).is_ok());
    }

    #[test]
    fn utf8_output_split_across_chunks_is_reassembled() {
        let full = "咕咕鸽终端：你好，世界".as_bytes();
        for split in 1..full.len() {
            let mut pending = Vec::new();
            let mut text = String::new();
            pending.extend_from_slice(&full[..split]);
            text.push_str(&take_utf8_prefix(&mut pending));
            pending.extend_from_slice(&full[split..]);
            text.push_str(&take_utf8_prefix(&mut pending));
            assert_eq!(text, "咕咕鸽终端：你好，世界", "在字节 {split} 处切块");
            assert!(pending.is_empty(), "完整字符不应滞留");
        }
    }

    #[test]
    fn utf8_invalid_byte_does_not_swallow_the_rest() {
        let mut pending = vec![b'a', 0xff, b'b'];
        assert_eq!(take_utf8_prefix(&mut pending), "a\u{FFFD}b");
        assert!(pending.is_empty());
    }

    #[test]
    fn interactive_pty_executes_commands() {
        let (shell, args) = shell_command();
        let cwd = std::env::current_dir().unwrap();
        let mut pty = match spawn_pty(&shell, &args, &cwd) {
            Ok(pty) => pty,
            // 受限沙箱（如桌面助手自己的运行时）禁止 conhost 启动，CreatePseudoConsole
            // 返回 ERROR_INVALID_HANDLE。普通用户会话不受影响，这里跳过而不是伪装通过。
            Err(error) if cfg!(windows) && error.to_string().contains("psuedo console") => {
                eprintln!("跳过 PTY 测试：当前进程无法创建 ConPTY（{error}）");
                return;
            }
            Err(error) => panic!("创建 PTY 失败: {error}"),
        };
        pty.writer.write_all(b"echo gugu-terminal-ok\r\n").unwrap();

        let deadline = Instant::now() + Duration::from_secs(15);
        let mut output = String::new();
        while Instant::now() < deadline && !output.contains("gugu-terminal-ok\r\n") {
            match pty.output_rx.try_recv() {
                Ok(chunk) => output.push_str(&String::from_utf8_lossy(&chunk)),
                Err(mpsc::error::TryRecvError::Empty) => {
                    std::thread::sleep(Duration::from_millis(50));
                }
                Err(mpsc::error::TryRecvError::Disconnected) => break,
            }
        }
        let _ = pty.child.kill();
        assert!(
            output.contains("gugu-terminal-ok"),
            "PTY 未回显命令输出，实际拿到: {output:?}"
        );
    }
}
