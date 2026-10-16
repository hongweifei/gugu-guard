//! 守护进程自身的生命周期：`gugu daemon start`（后台分离启动）/ `daemon stop`（优雅停止）。
//!
//! stop 走 `POST /api/v1/shutdown`：服务端先回响应，再由守护进程的信号循环
//! 完成「停子进程 → 关服务器 → 清 pid 文件」；本模块只负责请求 + 等端口关闭。

use anyhow::{Context, Result};
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use crate::{client, daemon};

/// 后台启动守护进程，等待其可服务后返回。
pub(crate) async fn start(config_path: &Path) -> Result<()> {
    // 已在运行就别再起一个（pid 锁会拦，但这里给出更友好的提示）
    if let Ok(url) = client::get_server_url(config_path, None) {
        if probe(&url, Duration::from_millis(300)).await {
            println!("守护进程已在运行: {url}");
            return Ok(());
        }
    }

    let exe = std::env::current_exe().context("无法获取当前可执行文件路径")?;
    let config_abs = gugu_core::config::canonicalize_clean(config_path);
    // 首次启动先落地配置模板，避免子进程只在默认空配置上跑
    let _ = daemon::ensure_config_template(&config_abs);

    let mut cmd = std::process::Command::new(exe);
    cmd.arg("run").arg("-c").arg(&config_abs);
    cmd.stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null());

    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        // 无控制台窗口 + 脱离父进程的控制台
        const CREATE_NO_WINDOW: u32 = 0x0800_0000;
        const DETACHED_PROCESS: u32 = 0x0000_0008;
        cmd.creation_flags(CREATE_NO_WINDOW | DETACHED_PROCESS);
    }
    #[cfg(unix)]
    {
        use std::os::unix::process::CommandExt;
        // 独立进程组：父 shell 退出不会带走守护进程
        cmd.process_group(0);
    }

    let child = cmd.spawn().context("启动守护进程失败")?;
    let pid = child.id();

    let url = client::get_server_url(config_path, None)?;
    let deadline = Instant::now() + Duration::from_secs(10);
    while Instant::now() < deadline {
        if probe(&url, Duration::from_millis(500)).await {
            println!("守护进程已启动 (PID: {pid})");
            println!("  配置: {}", config_abs.display());
            println!("  面板: {url}");
            return Ok(());
        }
        tokio::time::sleep(Duration::from_millis(200)).await;
    }

    anyhow::bail!("守护进程未在 10 秒内就绪，请查看日志目录下的 gugu-daemon 日志")
}

/// 请求守护进程优雅停止，并等待其端口关闭。
pub(crate) async fn stop(
    config_path: &Path,
    server_override: Option<&str>,
    api_key_override: Option<&str>,
) -> Result<()> {
    let url = client::get_server_url(config_path, server_override)?;
    if !probe(&url, Duration::from_millis(300)).await {
        println!("守护进程未在运行（{url} 不可达）");
        report_pid_file(config_path);
        return Ok(());
    }

    let key = client::get_api_key(config_path, api_key_override)?;
    let http = reqwest::Client::new();
    let mut req = http.post(format!("{url}/api/v1/shutdown"));
    if let Some(key) = key.as_deref() {
        req = req.header("Authorization", format!("Bearer {key}"));
    }
    let resp = req.send().await.context("发送停止请求失败")?;
    let status = resp.status();
    let body = resp.text().await.unwrap_or_default();
    if !status.is_success() {
        anyhow::bail!("停止请求被拒绝 ({status}): {body}");
    }

    println!("已请求优雅停止，等待进程退出...");
    let deadline = Instant::now() + Duration::from_secs(15);
    while Instant::now() < deadline {
        if !probe(&url, Duration::from_millis(300)).await {
            println!("守护进程已停止");
            return Ok(());
        }
        tokio::time::sleep(Duration::from_millis(300)).await;
    }

    anyhow::bail!("等待 15 秒仍未停止，请查看日志或停止对应的服务/任务")
}

/// 探测守护进程是否在服务：HTTP 有响应即算在运行（401 也算）。
async fn probe(url: &str, timeout: Duration) -> bool {
    let Ok(client) = reqwest::Client::builder().timeout(timeout).build() else {
        return false;
    };
    client
        .get(format!("{url}/api/v1/processes"))
        .send()
        .await
        .is_ok()
}

/// 不可达时顺带报出 pid 文件内容，便于用户自行确认。
fn report_pid_file(config_path: &Path) {
    let config_abs = gugu_core::config::canonicalize_clean(config_path);
    let dir = config_abs
        .parent()
        .map(Path::to_path_buf)
        .unwrap_or_else(|| PathBuf::from("."));
    let pid_file = dir.join(gugu_core::config::PID_FILE_NAME);
    if let Ok(text) = std::fs::read_to_string(&pid_file) {
        println!("  pid 文件: {} ({})", pid_file.display(), text.trim());
    }
}
