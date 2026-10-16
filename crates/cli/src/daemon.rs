use anyhow::{Context, Result};
use gugu_core::config::AppConfig;
use std::io::IsTerminal;
use std::path::{Path, PathBuf};
use tracing_appender::non_blocking::WorkerGuard;
use tracing_subscriber::layer::SubscriberExt;

pub struct CoreHandles {
    pub shared: gugu_core::manager::SharedManager,
    pub shutdown_tx: tokio::sync::oneshot::Sender<()>,
    pub server_handle: tokio::task::JoinHandle<()>,
    pub pid_path: PathBuf,
    /// 文件日志的非阻塞写线程句柄，随 CoreHandles 存活。
    pub _log_guard: Option<WorkerGuard>,
    /// `POST /api/v1/shutdown` 的停止信号接收端（由 CLI 的信号循环消费）。
    pub api_shutdown_rx: Option<tokio::sync::mpsc::Receiver<()>>,
}

/// 初始化日志：控制台 (仅在终端) + 按天滚动的文件层。
/// 文件层保证服务/计划任务形态下守护进程自身日志有处可查。
fn init_logging(log_base: &Path) -> Option<WorkerGuard> {
    let filter = tracing_subscriber::EnvFilter::try_from_default_env()
        .unwrap_or_else(|_| "gugu_core=info,gugu_server=info,gugu=info".into());

    let built = tracing_appender::rolling::RollingFileAppender::builder()
        .rotation(tracing_appender::rolling::Rotation::DAILY)
        .filename_prefix("gugu-daemon")
        .filename_suffix("log")
        .max_log_files(30)
        .build(log_base);
    let mut worker_guard = None;
    let file_layer = match built {
        Ok(appender) => {
            let (nb, guard) = tracing_appender::non_blocking(appender);
            worker_guard = Some(guard);
            Some(tracing_subscriber::fmt::layer().with_writer(nb).with_ansi(false))
        }
        Err(e) => {
            eprintln!("初始化文件日志失败 ({}): {e}，仅按现有方式输出", log_base.display());
            None
        }
    };
    let console = std::io::stdout().is_terminal()
        .then(|| tracing_subscriber::fmt::layer().with_writer(std::io::stdout));

    let subscriber = tracing_subscriber::registry().with(filter).with(file_layer).with(console);
    if tracing::subscriber::set_global_default(subscriber).is_err() {
        return None;
    }
    worker_guard
}

/// 内置配置模板：首次运行且找不到配置时写入，让配置永远出现在程序旁边。
/// 端口/文件名等从 core 的默认常量拼装，保证「模板即默认值」单一来源。
pub fn default_config_template() -> String {
    format!("\
# 咕咕鸽进程守护 配置文件
# 其中的相对路径 (日志、PID、工作目录) 都相对于本文件所在目录 (即程序目录)，
# 把整个文件夹搬到任何机器、任何位置都不需要改配置。

[daemon]
pid_file = \"{pid}\"
log_dir = \"logs\"
# api_key = \"your-secret-key\"

[daemon.web]
addr = \"{addr}\"
port = {port}
# cors_origins = [\"http://localhost:3000\"]

# 守护进程自身的运行日志按天写入 log_dir 下的 gugu-daemon.<日期>.log (保留最近 30 份)。

# 进程可以通过 WebUI 或 gugu add 添加 (会自动写回本文件，注释与相对路径都会保留)，也可以手动在此声明。
# 修改后执行 gugu reload 或重启守护进程生效。示例：

# [processes.my-app]
# command = \"node\"
# args = [\"app.js\"]
# auto_start = true
# auto_restart = true
# max_restarts = 3
# restart_delay_secs = 5
# restart_reset_secs = 60
# shell = false
# max_memory_mb = 512
# max_log_size_mb = 10
# stdout_log = \"logs/my-app-stdout.log\"
# stderr_log = \"logs/my-app-stderr.log\"

# [processes.my-app.health_check]
# type = \"tcp\"
# port = 8080
# fail_threshold = 3
",
        pid = gugu_core::config::PID_FILE_NAME,
        addr = gugu_core::config::DEFAULT_BIND_ADDR,
        port = gugu_core::config::DEFAULT_PORT,
    )
}

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
    std::fs::write(path, default_config_template())
        .with_context(|| format!("写入配置模板失败: {}", path.display()))?;
    Ok(true)
}

fn pid_path(config: &AppConfig, config_path: &Path) -> PathBuf {
    let config_dir = config_path.parent().unwrap_or(Path::new("."));
    match config.daemon.pid_file {
        Some(ref pid_file) => gugu_core::config::resolve_relative_path(pid_file, config_dir),
        None => config_dir.join(gugu_core::config::PID_FILE_NAME),
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
    let template_created = match ensure_config_template(config_path) {
        Ok(created) => created,
        Err(e) => {
            eprintln!("{e:#}");
            false
        }
    };
    let config = AppConfig::load(config_path).context("加载配置文件失败")?;

    let config_dir = config_path.parent().unwrap_or(Path::new(".")).to_path_buf();
    let log_base = config.daemon.log_dir.as_ref()
        .map(|d| gugu_core::config::resolve_relative_path(d, &config_dir))
        .unwrap_or_else(|| config_dir.clone());
    let log_guard = init_logging(&log_base);
    if template_created {
        tracing::info!(
            "未找到配置文件，已自动生成默认模板: {}，编辑后运行 gugu reload 生效",
            config_path.display()
        );
    }

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
    let (api_shutdown_tx, api_shutdown_rx) = tokio::sync::mpsc::channel::<()>(1);

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
            Some(api_shutdown_tx),
        ).await {
            tracing::error!("Web 服务错误: {e}");
        }
    });

    Ok(CoreHandles {
        shared,
        shutdown_tx,
        server_handle,
        pid_path: pid,
        _log_guard: log_guard,
        api_shutdown_rx: Some(api_shutdown_rx),
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
