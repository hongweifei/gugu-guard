use anyhow::{Context, Result};
use gugu_core::config::AppConfig;
use std::path::{Path, PathBuf};

pub struct CoreHandles {
    pub shared: gugu_core::manager::SharedManager,
    pub shutdown_tx: tokio::sync::oneshot::Sender<()>,
    pub server_handle: tokio::task::JoinHandle<()>,
    pub pid_path: PathBuf,
}

/// 内置配置模板：首次运行且找不到配置时写入，让配置永远出现在程序旁边。
pub const DEFAULT_CONFIG_TEMPLATE: &str = "\
# 咕咕鸽进程守护 配置文件
# 其中的相对路径 (日志、PID、工作目录) 都相对于本文件所在目录 (即程序目录)，
# 把整个文件夹搬到任何机器、任何位置都不需要改配置。

[daemon]
pid_file = \"gugu.pid\"
log_dir = \"logs\"
# api_key = \"your-secret-key\"

[daemon.web]
addr = \"0.0.0.0\"
port = 9090
# cors_origins = [\"http://localhost:3000\"]

# 进程可以通过 WebUI 或 gugu add 添加 (会自动写回本文件)，也可以手动在此声明。
# 修改后执行 gugu reload 或重启守护进程生效。示例：

# [processes.my-app]
# command = \"node\"
# args = [\"app.js\"]
# auto_start = true
# auto_restart = true
# max_restarts = 3
# restart_delay_secs = 5
# max_log_size_mb = 10
# stdout_log = \"logs/my-app-stdout.log\"
# stderr_log = \"logs/my-app-stderr.log\"

# [processes.my-app.health_check]
# type = \"tcp\"
# port = 8080
";

/// 配置文件不存在时，在目标位置生成内置模板。返回是否新建。
///
/// # Errors
/// 创建目录或写入文件失败时返回底层 IO 错误。
pub fn ensure_config_template(path: &Path) -> Result<bool> {
    if path.exists() {
        return Ok(false);
    }
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)
            .with_context(|| format!("创建配置目录失败: {}", parent.display()))?;
    }
    std::fs::write(path, DEFAULT_CONFIG_TEMPLATE)
        .with_context(|| format!("写入配置模板失败: {}", path.display()))?;
    Ok(true)
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
    match ensure_config_template(config_path) {
        Ok(true) => tracing::info!(
            "未找到配置文件，已自动生成默认模板: {}，编辑后运行 gugu reload 生效",
            config_path.display()
        ),
        Ok(false) => {}
        Err(e) => tracing::warn!("{e:#} (将按默认空配置启动)"),
    }
    let config = AppConfig::load(config_path).context("加载配置文件失败")?;

    let pid = pid_path(&config, config_path);
    write_pid_file(&pid)?;

    let api_key = cli_api_key
        .or(config.daemon.api_key.clone())
        .or_else(|| std::env::var("GUGU_API_KEY").ok());

    tracing::info!("咕咕鸽进程守护 v{} 启动中...", env!("CARGO_PKG_VERSION"));
    tracing::info!("配置文件: {}", gugu_core::config::canonicalize_clean(config_path).display());
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
