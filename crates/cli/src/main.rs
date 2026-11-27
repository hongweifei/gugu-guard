//! gugu CLI 入口：命令定义与分发。
//! 守护进程核心在 [`daemon`]，客户端子命令在 [`client`]，
//! 开机自启在 [`install`]，服务标识在 [`service`]。

use anyhow::Result;
use clap::{Parser, Subcommand};
use std::path::{Path, PathBuf};

mod check;
mod client;
mod daemon;
mod install;
mod lifecycle;
mod self_update;
mod service;
#[cfg(windows)]
mod windows_service;

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

    /// 操作哪台节点：`gugu nodes` 里的标识，缺省为本机 local
    #[arg(long, global = true)]
    node: Option<String>,

    #[arg(long, global = true, hide = true)]
    mode: Option<String>,

    /// 分离到后台运行（供开机自启项调用，不占用控制台）
    #[arg(long, global = true, hide = true)]
    detach: bool,
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

    #[command(about = "注册开机自启 (task: 系统级 | service: Windows 服务 | user: 当前用户)")]
    Install {
        #[arg(short, long, default_value = "task")]
        mode: String,
        /// 只打印将要写入/执行的内容，不改动系统
        #[arg(long)]
        dry_run: bool,
    },

    #[command(about = "卸载开机自启")]
    Uninstall {
        #[arg(short, long, default_value = "task")]
        mode: String,
        /// 只打印将要执行的内容，不改动系统
        #[arg(long)]
        dry_run: bool,
    },

    #[command(about = "校验配置文件 (只读，不启动进程)")]
    Check,

    #[command(about = "守护进程自身生命周期 (后台启动 / 优雅停止)")]
    Daemon {
        #[command(subcommand)]
        action: DaemonAction,
    },

    #[command(about = "显示所有进程状态")]
    Status {
        /// 以 JSON 输出，便于脚本消费
        #[arg(long)]
        json: bool,
    },

    #[command(about = "列出已配置的进程")]
    List {
        /// 以 JSON 输出，便于脚本消费
        #[arg(long)]
        json: bool,
    },

    #[command(about = "列出中心节点纳管的节点与在线状态")]
    Nodes {
        /// 以 JSON 输出，便于脚本消费
        #[arg(long)]
        json: bool,
    },

    #[command(about = "启动指定进程")]
    Start { name: String },

    #[command(about = "停止指定进程")]
    Stop { name: String },

    #[command(about = "重启指定进程")]
    Restart { name: String },

    #[command(about = "添加一个新进程")]
    Add {
        name: String,
        #[command(flatten)]
        opts: Box<AddOptions>,
    },

    #[command(about = "移除一个进程")]
    Remove { name: String },

    #[command(about = "查看进程日志（可按级别/关键字/输出流过滤）")]
    Logs {
        name: String,
        /// 实时读取的条数上限
        #[arg(short, long, default_value = "50")]
        lines: usize,
        /// 最低级别：trace / debug / info / warn / error / fatal
        #[arg(long)]
        level: Option<String>,
        /// 关键字（空白分隔，全部命中才算匹配）
        #[arg(short = 'q', long)]
        query: Option<String>,
        /// 只查看某条输出流：stdout / stderr
        #[arg(long)]
        stream: Option<String>,
        /// 改为检索已落盘的历史日志文件
        #[arg(long)]
        file: bool,
        /// 历史检索时最多倒读的 MB 总量（当前文件与 .1–.6 备份共享）
        #[arg(long, default_value_t = 4)]
        budget_mb: u64,
        /// 历史检索的命中条数上限
        #[arg(short = 'n', long, default_value_t = 200)]
        limit: usize,
    },

    #[command(about = "查看进程指标时序 (CPU / 内存 / 连接数，默认最近 60 分钟)")]
    History {
        name: String,
        #[arg(short, long, default_value_t = 60)]
        minutes: u64,
        /// 以 JSON 输出，便于脚本消费
        #[arg(long)]
        json: bool,
    },

    #[command(about = "查看进程退出历史与可用率 (落盘记录，跨守护重启保留)")]
    Exits {
        name: String,
        #[arg(short, long, default_value_t = 60)]
        minutes: u64,
        /// 最多列出的事件条数
        #[arg(short, long, default_value_t = 20)]
        limit: usize,
        /// 以 JSON 输出，便于脚本消费
        #[arg(long)]
        json: bool,
    },

    #[command(about = "查看整机带宽时序 (所有网卡合计，非单进程口径)")]
    Net {
        #[arg(short, long, default_value_t = 60)]
        minutes: u64,
        /// 以 JSON 输出，便于脚本消费
        #[arg(long)]
        json: bool,
    },

    #[command(about = "查看操作审计（谁在何时对哪个进程做了什么，落盘记录）")]
    Audit {
        /// 最多返回的条数
        #[arg(short, long, default_value_t = 100)]
        limit: usize,
        /// 只看某个动作码：process.create / process.start / process.stop / group.restart ...
        #[arg(long)]
        action: Option<String>,
        /// 只看失败的操作
        #[arg(long)]
        failed: bool,
        /// 只看最近 N 分钟（0 表示不限）
        #[arg(long, default_value_t = 0)]
        since: u64,
        /// 以 JSON 输出，便于脚本消费
        #[arg(long)]
        json: bool,
    },

    #[command(about = "重新加载配置文件")]
    Reload,

    #[command(about = "检查并安装新版本（更新正在运行的守护进程）")]
    Update {
        /// 只检查，不安装
        #[arg(long)]
        check: bool,
        /// 指定要安装的版本，缺省用最新正式版
        #[arg(long)]
        version: Option<String>,
        /// 安装后不等待新版本上线
        #[arg(long)]
        no_wait: bool,
    },

    #[command(about = "文件管理：浏览、预览、下载、上传与增删改")]
    Fs {
        #[command(subcommand)]
        action: FsAction,
    },

    /// 内部：等旧进程退出后换入新二进制并重启（由更新流程拉起）
    #[command(name = "__apply-update", hide = true)]
    ApplyUpdate {
        #[arg(long)]
        target: PathBuf,
        #[arg(long)]
        pid: u32,
        #[arg(long)]
        cwd: Option<PathBuf>,
        #[arg(long)]
        no_restart: bool,
        #[arg(long)]
        no_backup: bool,
    },
}

/// `gugu add` 的进程参数：字段多，单独成组，`Commands` 保持轻量。
#[derive(clap::Args)]
struct AddOptions {
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
    /// 日志格式：auto / json / logfmt / text（默认 auto 逐档尝试）
    #[arg(long)]
    log_format: Option<String>,
    /// 日志文本编码：auto / utf-8 / gbk（默认 auto：UTF-8 优先、GBK 兜底）
    #[arg(long)]
    encoding: Option<String>,
    /// 资源硬限：CPU，如 50% / 0.5（核数）/ "50000 100000"；Linux 走 cgroup 配额，Windows 按整机占比节流
    #[arg(long)]
    cpu_max: Option<String>,
    /// 资源硬限：内存，如 512M / 2G；Linux 越界整组 OOM，Windows 限制 Job 提交内存
    #[arg(long)]
    memory_max: Option<String>,
    /// 资源硬限：进程/任务数，防 fork 炸弹；Windows 只统计进程
    #[arg(long)]
    pids_max: Option<String>,
    /// 进程监听的端口（逗号分隔）：用于启动前端口预检、连接数统计与就绪判断
    #[arg(short = 'p', long, value_delimiter = ',')]
    port: Vec<u16>,
    /// 关闭启动前的端口占用预检
    #[arg(long)]
    no_port_check: bool,
    /// 依赖就绪的最长等待秒数
    #[arg(long, default_value_t = 60)]
    ready_timeout: u64,
    #[arg(short, long)]
    start: bool,
}

/// `gugu fs` 的子命令：路径一律按守护进程所在机器解释。
#[derive(Subcommand)]
enum FsAction {
    #[command(about = "列出目录内容（缺省为白名单第一个根或当前目录）")]
    Ls {
        path: Option<String>,
        /// 以 JSON 输出，便于脚本消费
        #[arg(long)]
        json: bool,
    },

    #[command(about = "查看文本文件内容")]
    Cat {
        path: String,
        /// 最多读取的字节数（0 表示用服务端默认上限）
        #[arg(long, default_value_t = 0)]
        limit: u64,
    },

    #[command(about = "下载文件到本地")]
    Get {
        path: String,
        /// 本地保存路径，缺省用远端文件名
        #[arg(short, long)]
        output: Option<PathBuf>,
    },

    #[command(about = "上传本地文件")]
    Put {
        /// 本地文件路径
        local: PathBuf,
        /// 远端目标路径
        remote: String,
        /// 覆盖同名文件
        #[arg(long)]
        overwrite: bool,
    },

    #[command(about = "新建目录")]
    Mkdir {
        path: String,
        /// 逐级创建，已存在不算错
        #[arg(short, long)]
        parents: bool,
    },

    #[command(about = "新建空文件")]
    Touch { path: String },

    #[command(about = "移动或重命名")]
    Mv {
        from: String,
        to: String,
        #[arg(long)]
        overwrite: bool,
    },

    #[command(about = "复制文件或目录（目录递归）")]
    Cp {
        from: String,
        to: String,
        #[arg(long)]
        overwrite: bool,
    },

    #[command(about = "删除文件或目录")]
    Rm {
        path: String,
        /// 递归删除非空目录
        #[arg(short, long)]
        recursive: bool,
    },
}

#[derive(Subcommand)]
enum DaemonAction {
    #[command(about = "后台启动守护进程（分离运行，日志进文件）")]
    Start,

    #[command(about = "优雅停止守护进程（POST /api/v1/shutdown）")]
    Stop,
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
            if cli.detach {
                // 开机自启项（Run 键）调用：由自己分离出一个无窗口的后台进程
                return lifecycle::start(&config_path).await;
            }
            run_daemon(&config_path, cli.api_key.clone()).await
        }
        Commands::Install { mode, dry_run } => {
            install::install_service(&config_path, mode, *dry_run)
        }
        Commands::Uninstall { mode, dry_run } => install::uninstall_service(mode, *dry_run),
        Commands::Check => check::run(&config_path),
        Commands::ApplyUpdate {
            target,
            pid,
            cwd,
            no_restart,
            no_backup,
        } => self_update::run(target, *pid, cwd.as_deref(), !*no_restart, !*no_backup),
        Commands::Daemon { action } => match action {
            DaemonAction::Start => lifecycle::start(&config_path).await,
            DaemonAction::Stop => {
                lifecycle::stop(&config_path, cli.server.as_deref(), cli.api_key.as_deref()).await
            }
        },
        // 客户端命令：逐个列出，新增命令时这里和 client::run_client 都会报编译错误，
        // 不会因为一条 `_` 兜底而把本地命令误送进客户端分发。
        command @ (Commands::Nodes { .. }
        | Commands::Status { .. }
        | Commands::List { .. }
        | Commands::Start { .. }
        | Commands::Stop { .. }
        | Commands::Restart { .. }
        | Commands::Add { .. }
        | Commands::Remove { .. }
        | Commands::Logs { .. }
        | Commands::History { .. }
        | Commands::Exits { .. }
        | Commands::Net { .. }
        | Commands::Audit { .. }
        | Commands::Update { .. }
        | Commands::Fs { .. }
        | Commands::Reload) => {
            let server_url = client::get_server_url(&config_path, cli.server.as_deref())?;
            let api_key = client::get_api_key(&config_path, cli.api_key.as_deref())?;
            client::run_client(
                command,
                &server_url,
                api_key.as_deref(),
                cli.node.as_deref(),
            )
            .await
        }
    }
}

async fn run_daemon(config_path: &Path, cli_api_key: Option<String>) -> Result<()> {
    // 日志初始化 (控制台 + 文件) 统一在 daemon::run_core 内完成
    let mut handles = daemon::run_core(config_path, cli_api_key).await?;
    let mut api_shutdown_rx = handles.api_shutdown_rx.take();
    #[cfg(unix)]
    let shared = handles.shared.clone();
    #[cfg(unix)]
    let config_path_owned = config_path.to_path_buf();

    #[cfg(unix)]
    {
        let mut sigterm = tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate())
            .expect("无法注册 SIGTERM 处理器");
        let mut sighup = tokio::signal::unix::signal(tokio::signal::unix::SignalKind::hangup())
            .expect("无法注册 SIGHUP 处理器");

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
                requested = wait_api_shutdown(&mut api_shutdown_rx) => {
                    if requested {
                        tracing::info!("收到 API 停止请求 (POST /api/v1/shutdown)");
                    } else {
                        tracing::error!("Web 服务已退出，停止通道随之关闭");
                    }
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
                    reload_config.warn_unknown_keys();
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
        tokio::select! {
            r = tokio::signal::ctrl_c() => {
                r.expect("无法注册 Ctrl+C 处理器");
                tracing::info!("收到 Ctrl+C 信号");
            }
            requested = wait_api_shutdown(&mut api_shutdown_rx) => {
                if requested {
                    tracing::info!("收到 API 停止请求 (POST /api/v1/shutdown)");
                } else {
                    tracing::error!("Web 服务已退出，停止通道随之关闭");
                }
            }
        }
    }

    daemon::graceful_shutdown(handles).await;
    Ok(())
}

/// 等待 `POST /api/v1/shutdown` 的停止信号；未接线时永不返回。
///
/// 返回 `true` 表示确实收到了停止请求；`false` 表示发送端已经全部释放，
/// 也就是 Web 服务没能继续运行。两种情况都要退出，但原因不同，日志必须分开。
async fn wait_api_shutdown(rx: &mut Option<tokio::sync::mpsc::Receiver<()>>) -> bool {
    match rx {
        Some(rx) => rx.recv().await.is_some(),
        None => std::future::pending().await,
    }
}
