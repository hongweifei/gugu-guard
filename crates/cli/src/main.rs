//! gugu CLI 入口：命令定义与分发。
//! 守护进程核心在 [`daemon`]，客户端子命令在 [`client`]，
//! 开机自启在 [`install`]，服务标识在 [`service`]。

use anyhow::Result;
use clap::{Parser, Subcommand};
use std::path::{Path, PathBuf};

#[cfg(windows)]
mod windows_service;
mod client;
mod daemon;
mod install;
mod service;

#[derive(Parser)]
#[command(name = "gugu", about = "咕咕鸽进程守护 - 跨平台进程管理工具", version)]
struct Cli {
    #[command(subcommand)]
    command: Commands,

    #[arg(short, long, global = true)]
    config: Option<PathBuf>,

    #[arg(long, global = true)]
    server: Option<String>,

    #[arg(long, global = true)]
    api_key: Option<String>,

    #[arg(long, global = true, hide = true)]
    mode: Option<String>,
}

/// 解析配置文件路径：未显式指定时，优先使用程序同目录下的 gugu.toml，
/// 其次是当前目录下的 gugu.toml，两者都不存在时返回程序同目录路径
/// (运行时会在此自动生成)。显式传入的路径按原样使用。
fn resolve_config_path(explicit: Option<PathBuf>) -> PathBuf {
    explicit.unwrap_or_else(gugu_core::config::default_config_path)
}

#[derive(Subcommand)]
enum Commands {
    #[command(about = "启动守护进程")]
    Run,

    #[command(about = "注册为系统服务 (开机自启)")]
    Install {
        #[arg(short, long, default_value = "task")]
        mode: String,
    },

    #[command(about = "卸载系统服务")]
    Uninstall {
        #[arg(short, long, default_value = "task")]
        mode: String,
    },

    #[command(about = "显示所有进程状态")]
    Status,

    #[command(about = "列出已配置的进程")]
    List,

    #[command(about = "启动指定进程")]
    Start { name: String },

    #[command(about = "停止指定进程")]
    Stop { name: String },

    #[command(about = "重启指定进程")]
    Restart { name: String },

    #[command(about = "添加一个新进程")]
    Add {
        name: String,
        #[arg(short = 'x', long)]
        command: String,
        #[arg(short, long, value_delimiter = ',')]
        args: Vec<String>,
        #[arg(short, long)]
        dir: Option<String>,
        #[arg(long, value_delimiter = ',')]
        env: Vec<String>,
        #[arg(long)]
        no_auto_start: bool,
        #[arg(long)]
        no_auto_restart: bool,
        #[arg(long, default_value_t = 3)]
        max_restarts: u32,
        #[arg(long, default_value_t = 5)]
        restart_delay: u64,
        #[arg(long, default_value_t = 10)]
        stop_timeout: u64,
        #[arg(long)]
        stdout_log: Option<String>,
        #[arg(long)]
        stderr_log: Option<String>,
        #[arg(short, long)]
        start: bool,
    },

    #[command(about = "移除一个进程")]
    Remove { name: String },

    #[command(about = "查看进程日志")]
    Logs {
        name: String,
        #[arg(short, long, default_value = "50")]
        lines: usize,
    },

    #[command(about = "重新加载配置文件")]
    Reload,
}

#[tokio::main]
async fn main() -> Result<()> {
    let cli = Cli::parse();
    let config_path = resolve_config_path(cli.config.clone());

    match &cli.command {
        Commands::Run => {
            if cli.mode.as_deref() == Some("service") {
                #[cfg(windows)]
                {
                    return windows_service::start();
                }
                #[cfg(not(windows))]
                {
                    anyhow::bail!("Service 模式仅在 Windows 上可用");
                }
            }
            run_daemon(&config_path, cli.api_key.clone()).await
        }
        Commands::Install { mode } => install::install_service(&config_path, mode),
        Commands::Uninstall { mode } => install::uninstall_service(mode),
        _ => {
            let server_url = client::get_server_url(&config_path, cli.server.as_deref())?;
            let api_key = client::get_api_key(&config_path, cli.api_key.as_deref())?;
            client::run_client(&cli.command, &server_url, api_key.as_deref()).await
        }
    }
}

async fn run_daemon(config_path: &Path, cli_api_key: Option<String>) -> Result<()> {
    // 日志初始化 (控制台 + 文件) 统一在 daemon::run_core 内完成
    let handles = daemon::run_core(config_path, cli_api_key).await?;
    #[cfg(unix)]
    let shared = handles.shared.clone();
    #[cfg(unix)]
    let config_path_owned = config_path.to_path_buf();

    #[cfg(unix)]
    {
        let mut sigterm = tokio::signal::unix::signal(
            tokio::signal::unix::SignalKind::terminate()
        ).expect("无法注册 SIGTERM 处理器");
        let mut sighup = tokio::signal::unix::signal(
            tokio::signal::unix::SignalKind::hangup()
        ).expect("无法注册 SIGHUP 处理器");

        loop {
            tokio::select! {
                _ = tokio::signal::ctrl_c() => {
                    tracing::info!("收到 SIGINT (Ctrl+C)");
                    break;
                }
                _ = sigterm.recv() => {
                    tracing::info!("收到 SIGTERM");
                    break;
                }
                _ = sighup.recv() => {
                    tracing::info!("收到 SIGHUP，重新加载配置...");
                    let reload_config = match gugu_core::config::AppConfig::load(&config_path_owned) {
                        Ok(c) => c,
                        Err(e) => {
                            tracing::error!("重载配置文件失败: {e}");
                            continue;
                        }
                    };
                    match shared.reload_config(&reload_config).await {
                        Ok(()) => tracing::info!("配置已重新加载"),
                        Err(e) => tracing::error!("重载配置失败: {e}"),
                    }
                }
            }
        }
    }

    #[cfg(not(unix))]
    {
        tokio::signal::ctrl_c().await.expect("无法注册 Ctrl+C 处理器");
        tracing::info!("收到 Ctrl+C 信号");
    }

    daemon::graceful_shutdown(handles).await;
    Ok(())
}
