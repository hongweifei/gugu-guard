use anyhow::{Context, Result};
use gugu_core::config::AppConfig;
use std::path::{Path, PathBuf};

pub struct CoreHandles {
    pub shared: gugu_core::manager::SharedManager,
    pub shutdown_tx: tokio::sync::oneshot::Sender<()>,
    pub server_handle: tokio::task::JoinHandle<()>,
    pub pid_path: PathBuf,
}

fn pid_path(config: &AppConfig, config_path: &Path) -> PathBuf {
    let config_dir = config_path.parent().unwrap_or(Path::new("."));
    match config.daemon.pid_file {
        Some(ref pid_file) => gugu_core::config::resolve_relative_path(pid_file, config_dir),
        None => config_dir.join("gugu.pid"),
    }
}

fn current_pid() -> u32 {
    std::process::id()
}

fn is_pid_running(pid: u32) -> bool {
    #[cfg(windows)]
    {
        let output = std::process::Command::new("tasklist")
            .args(["/FI", &format!("PID eq {pid}"), "/NH", "/FO", "CSV"])
            .output();
        match output {
            Ok(o) => {
                let s = String::from_utf8_lossy(&o.stdout);
                s.contains(&format!("\"{pid}\""))
            }
            Err(_) => false,
        }
    }
    #[cfg(unix)]
    { std::path::Path::new(&format!("/proc/{pid}")).exists() }
}

fn write_pid_file(path: &Path) -> Result<()> {
    if path.exists() {
        let content = std::fs::read_to_string(path).unwrap_or_default();
        if let Ok(old_pid) = content.trim().parse::<u32>() {
            if old_pid != current_pid() && is_pid_running(old_pid) {
                anyhow::bail!(
                    "另一个守护进程正在运行 (PID: {old_pid})，请先停止它或删除 {}",
                    path.display()
                );
            }
        }
    }
    std::fs::write(path, current_pid().to_string())?;
    tracing::debug!("PID 文件已写入: {} (PID: {})", path.display(), current_pid());
    Ok(())
}

fn remove_pid_file(path: &Path) {
    if path.exists() { let _ = std::fs::remove_file(path); }
}

pub async fn run_core(config_path: &Path, cli_api_key: Option<String>) -> Result<CoreHandles> {
    let config = AppConfig::load(config_path).context("加载配置文件失败")?;

    let pid = pid_path(&config, config_path);
    write_pid_file(&pid)?;

    let api_key = cli_api_key
        .or(config.daemon.api_key.clone())
        .or_else(|| std::env::var("GUGU_API_KEY").ok());

    tracing::info!("咕咕鸽进程守护 v{} 启动中...", env!("CARGO_PKG_VERSION"));
    tracing::info!("配置文件: {}", config_path.display());
    tracing::info!("进程 PID: {}", current_pid());

    if api_key.is_some() {
        tracing::info!("API Key 认证已启用");
    }

    // 启动 manager actor（内部会自动启动所有 auto_start 进程和监控循环）
    let shared = gugu_core::manager::start(&config, Some(config_path.to_path_buf()));

    let addr_str = config.server_addr();
    let addr: std::net::SocketAddr = addr_str
        .parse()
        .context(format!("解析地址失败: {addr_str}"))?;

    let (shutdown_tx, shutdown_rx) = tokio::sync::oneshot::channel::<()>();

    let server_shared = shared.clone();
    let server_api_key = api_key;
    let server_cors_origins = config.daemon.web.cors_origins;
    let server_handle = tokio::spawn(async move {
        if let Err(e) = gugu_server::run_server(
            addr,
            server_shared,
            server_api_key,
            server_cors_origins,
            shutdown_rx,
        ).await {
            tracing::error!("Web 服务错误: {e}");
        }
    });

    Ok(CoreHandles {
        shared,
        shutdown_tx,
        server_handle,
        pid_path: pid,
    })
}

pub async fn graceful_shutdown(handles: CoreHandles) {
    tracing::info!("正在优雅停止所有进程...");
    let _ = handles.shutdown_tx.send(());

    // 请求 actor 关闭（会 stop_all）
    handles.shared.shutdown();

    match tokio::time::timeout(std::time::Duration::from_secs(5), handles.server_handle).await {
        Ok(_) => {}
        Err(_) => {
            tracing::warn!("Web 服务关闭超时，强制终止");
        }
    }

    remove_pid_file(&handles.pid_path);
    tracing::info!("咕咕鸽进程守护已安全停止");
}
