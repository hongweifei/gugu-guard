use anyhow::{Context, Result};
use clap::{Parser, Subcommand};
use comfy_table::{modifiers::UTF8_ROUND_CORNERS, presets::UTF8_FULL, Table, Cell, Color, Attribute};
use gugu_core::config::AppConfig;
use std::collections::HashMap;
use std::path::{Path, PathBuf};

const SERVICE_NAME: &str = "GuguGuard";

#[derive(Parser)]
#[command(name = "gugu", about = "咕咕鸽进程守护 - 跨平台进程管理工具", version)]
struct Cli {
    #[command(subcommand)]
    command: Commands,

    #[arg(short, long, default_value = "gugu.toml", global = true)]
    config: PathBuf,

    #[arg(long, global = true)]
    server: Option<String>,
}

#[derive(Subcommand)]
enum Commands {
    #[command(about = "启动守护进程")]
    Run,

    #[command(about = "注册为系统服务 (开机自启)")]
    Install,

    #[command(about = "卸载系统服务")]
    Uninstall,

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
}

#[tokio::main]
async fn main() -> Result<()> {
    let cli = Cli::parse();

    match &cli.command {
        Commands::Run => run_daemon(&cli.config).await,
        Commands::Install => install_service(&cli.config),
        Commands::Uninstall => uninstall_service(),
        _ => {
            let server_url = get_server_url(&cli)?;
            run_client(&cli.command, &server_url).await
        }
    }
}

fn get_server_url(cli: &Cli) -> Result<String> {
    if let Some(ref url) = cli.server {
        return Ok(url.clone());
    }
    let config = AppConfig::load(&cli.config).context("加载配置文件失败")?;
    let addr = config.daemon.web.addr.as_deref().unwrap_or("127.0.0.1");
    let port = config.daemon.web.port.unwrap_or(9090);
    let connect_addr = if addr == "0.0.0.0" { "127.0.0.1" } else { addr };
    Ok(format!("http://{connect_addr}:{port}"))
}

fn pid_path(config_path: &Path) -> PathBuf {
    config_path.parent().unwrap_or(Path::new(".")).join("gugu.pid")
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
                s.contains(&pid.to_string())
            }
            Err(_) => false,
        }
    }
    #[cfg(unix)]
    {
        std::path::Path::new(&format!("/proc/{pid}")).exists()
    }
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
    if path.exists() {
        let _ = std::fs::remove_file(path);
    }
}

async fn wait_for_shutdown_signal() {
    #[cfg(unix)]
    {
        let mut sigterm = tokio::signal::unix::signal(
            tokio::signal::unix::SignalKind::terminate()
        ).expect("无法注册 SIGTERM 处理器");
        let mut sighup = tokio::signal::unix::signal(
            tokio::signal::unix::SignalKind::hangup()
        ).expect("无法注册 SIGHUP 处理器");

        tokio::select! {
            _ = tokio::signal::ctrl_c() => tracing::info!("收到 SIGINT (Ctrl+C)"),
            _ = sigterm.recv() => tracing::info!("收到 SIGTERM"),
            _ = sighup.recv() => tracing::info!("收到 SIGHUP"),
        }
    }
    #[cfg(not(unix))]
    {
        tokio::signal::ctrl_c().await.expect("无法注册 Ctrl+C 处理器");
        tracing::info!("收到 Ctrl+C 信号");
    }
}

async fn run_daemon(config_path: &PathBuf) -> Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| "gugu_core=info,gugu_server=info,gugu=info".into()),
        )
        .init();

    let pid = pid_path(config_path);
    write_pid_file(&pid)?;

    let config = AppConfig::load(config_path).context("加载配置文件失败")?;

    tracing::info!("咕咕鸽进程守护 v{} 启动中...", env!("CARGO_PKG_VERSION"));
    tracing::info!("配置文件: {}", config_path.display());
    tracing::info!("进程 PID: {}", current_pid());

    let manager = gugu_core::ProcessManager::new(&config, Some(config_path.clone()));
    let shared = manager.shared();

    {
        let mut mgr = shared.write().await;
        mgr.start_all().await;
    }

    let monitor_manager = shared.clone();
    tokio::spawn(async move {
        gugu_core::manager::start_monitor(monitor_manager).await;
    });

    let addr_str = config.server_addr();
    let addr: std::net::SocketAddr = addr_str.parse()
        .context(format!("解析地址失败: {addr_str}"))?;
    let web_dir = find_web_dir();

    let (shutdown_tx, shutdown_rx) = tokio::sync::oneshot::channel::<()>();

    let server_shared = shared.clone();
    let server_handle = tokio::spawn(async move {
        if let Err(e) = gugu_server::run_server(addr, server_shared, web_dir, shutdown_rx).await {
            tracing::error!("Web 服务错误: {}", e);
        }
    });

    wait_for_shutdown_signal().await;

    tracing::info!("正在优雅停止所有进程...");
    let _ = shutdown_tx.send(());

    {
        let mut mgr = shared.write().await;
        mgr.stop_all().await;
    }

    let _ = tokio::time::timeout(
        std::time::Duration::from_secs(10),
        server_handle,
    ).await;

    remove_pid_file(&pid);
    tracing::info!("咕咕鸽进程守护已安全停止");
    Ok(())
}

fn find_web_dir() -> Option<String> {
    let candidates = ["web", "../web", "../../web"];
    for dir in &candidates {
        let path = std::path::Path::new(dir);
        if path.exists() && path.join("index.html").exists() {
            return Some(dir.to_string());
        }
    }
    None
}

fn install_service(config_path: &Path) -> Result<()> {
    let exe = std::env::current_exe()
        .context("无法获取当前可执行文件路径")?;
    let config_abs = std::fs::canonicalize(config_path)
        .unwrap_or_else(|_| config_path.to_path_buf());

    #[cfg(windows)]
    {
        let tr = format!("\"{}\" run -c \"{}\"", exe.display(), config_abs.display());

        let status = std::process::Command::new("schtasks")
            .args([
                "/create",
                "/tn", SERVICE_NAME,
                "/tr", &tr,
                "/sc", "onstart",
                "/ru", "SYSTEM",
                "/rl", "HIGHEST",
                "/f",
            ])
            .status()
            .context("无法执行 schtasks，请以管理员身份运行")?;

        if status.success() {
            println!("已安装为开机自启任务 (任务计划程序: {SERVICE_NAME})");
            println!("  程序: {}", exe.display());
            println!("  配置: {}", config_abs.display());
        } else {
            anyhow::bail!("注册任务失败，请确认以管理员身份运行");
        }
    }

    #[cfg(unix)]
    {
        let unit = format!(
            "[Unit]\n\
             Description=咕咕鸽进程守护 (GuguGuard)\n\
             After=network.target\n\n\
             [Service]\n\
             Type=simple\n\
             ExecStart={exe} run -c {config}\n\
             Restart=on-failure\n\
             RestartSec=5\n\
             KillSignal=SIGTERM\n\
             TimeoutStopSec=30\n\
             WorkingDirectory={workdir}\n\n\
             [Install]\n\
             WantedBy=multi-user.target\n",
            exe = exe.display(),
            config = config_abs.display(),
            workdir = config_abs.parent().unwrap_or(Path::new("/")).display(),
        );

        let unit_path = "/etc/systemd/system/guguguard.service";
        std::fs::write(unit_path, &unit)
            .context("写入 systemd unit 文件失败，请使用 sudo 运行")?;

        std::process::Command::new("systemctl")
            .args(["daemon-reload"])
            .status()?;
        std::process::Command::new("systemctl")
            .args(["enable", "guguguard"])
            .status()?;

        println!("已安装为 systemd 服务 (guguguard)");
        println!("  unit: {unit_path}");
        println!("  启动: systemctl start guguguard");
        println!("  状态: systemctl status guguguard");
    }

    Ok(())
}

fn uninstall_service() -> Result<()> {
    #[cfg(windows)]
    {
        let status = std::process::Command::new("schtasks")
            .args(["/delete", "/tn", SERVICE_NAME, "/f"])
            .status()
            .context("无法执行 schtasks，请以管理员身份运行")?;

        if status.success() {
            println!("已卸载开机自启任务 ({SERVICE_NAME})");
        } else {
            anyhow::bail!("卸载任务失败，任务可能不存在");
        }
    }

    #[cfg(unix)]
    {
        std::process::Command::new("systemctl")
            .args(["stop", "guguguard"])
            .status().ok();
        std::process::Command::new("systemctl")
            .args(["disable", "guguguard"])
            .status().ok();
        let _ = std::fs::remove_file("/etc/systemd/system/guguguard.service");
        std::process::Command::new("systemctl")
            .args(["daemon-reload"])
            .status()?;

        println!("已卸载 systemd 服务 (guguguard)");
    }

    Ok(())
}

async fn run_client(command: &Commands, server_url: &str) -> Result<()> {
    let client = reqwest::Client::new();

    match command {
        Commands::Status => {
            let resp = client
                .get(format!("{server_url}/api/v1/processes"))
                .send()
                .await
                .context("无法连接到守护进程，请确认是否已启动")?;
            let processes: Vec<gugu_core::process::ProcessInfo> = resp.json().await?;
            print_status_table(&processes);
        }

        Commands::List => {
            let resp = client
                .get(format!("{server_url}/api/v1/processes"))
                .send()
                .await
                .context("无法连接到守护进程")?;
            let processes: Vec<gugu_core::process::ProcessInfo> = resp.json().await?;
            print_list_table(&processes);
        }

        Commands::Start { name } => {
            let resp = client
                .post(format!("{server_url}/api/v1/processes/{name}/start"))
                .send()
                .await?;
            print_api_response(resp).await?;
        }

        Commands::Stop { name } => {
            let resp = client
                .post(format!("{server_url}/api/v1/processes/{name}/stop"))
                .send()
                .await?;
            print_api_response(resp).await?;
        }

        Commands::Restart { name } => {
            let resp = client
                .post(format!("{server_url}/api/v1/processes/{name}/restart"))
                .send()
                .await?;
            print_api_response(resp).await?;
        }

        Commands::Add {
            name, command, args, dir, env,
            no_auto_start, no_auto_restart, max_restarts,
            restart_delay, stdout_log, stderr_log, start,
        } => {
            let mut env_map = HashMap::new();
            for e in env {
                if let Some((k, v)) = e.split_once('=') {
                    env_map.insert(k.to_string(), v.to_string());
                }
            }
            let body = serde_json::json!({
                "command": command,
                "args": args,
                "working_dir": dir,
                "env": env_map,
                "auto_start": !no_auto_start,
                "auto_restart": !no_auto_restart,
                "max_restarts": max_restarts,
                "restart_delay_secs": restart_delay,
                "stdout_log": stdout_log,
                "stderr_log": stderr_log,
                "start_now": start,
            });
            let resp = client
                .post(format!("{server_url}/api/v1/processes/{name}"))
                .json(&body)
                .send()
                .await?;
            print_api_response(resp).await?;
        }

        Commands::Remove { name } => {
            let resp = client
                .delete(format!("{server_url}/api/v1/processes/{name}"))
                .send()
                .await?;
            print_api_response(resp).await?;
        }

        Commands::Logs { name, lines } => {
            let resp = client
                .get(format!("{server_url}/api/v1/processes/{name}/logs?lines={lines}"))
                .send()
                .await?;
            let logs: Vec<gugu_core::process::LogEntry> = resp.json().await?;
            for entry in &logs {
                let time = entry.timestamp.format("%H:%M:%S");
                let prefix = match entry.stream {
                    gugu_core::process::LogStream::Stdout => "OUT",
                    gugu_core::process::LogStream::Stderr => "ERR",
                };
                println!("[{time}] [{prefix}] {}", entry.line);
            }
            if logs.is_empty() {
                println!("暂无日志");
            }
        }

        Commands::Run | Commands::Install | Commands::Uninstall => unreachable!(),
    }

    Ok(())
}

fn print_status_table(processes: &[gugu_core::process::ProcessInfo]) {
    if processes.is_empty() {
        println!("暂无进程，使用 gugu add 添加进程");
        return;
    }

    let mut table = Table::new();
    table.load_preset(UTF8_FULL).apply_modifier(UTF8_ROUND_CORNERS);
    table.set_header(vec![
        Cell::new("名称").add_attribute(Attribute::Bold),
        Cell::new("状态").add_attribute(Attribute::Bold),
        Cell::new("PID").add_attribute(Attribute::Bold),
        Cell::new("运行时间").add_attribute(Attribute::Bold),
        Cell::new("重启次数").add_attribute(Attribute::Bold),
    ]);

    for p in processes {
        let status_cell = match &p.status {
            gugu_core::process::ProcessStatus::Running => Cell::new("运行中").fg(Color::Green),
            gugu_core::process::ProcessStatus::Stopped => Cell::new("已停止").fg(Color::Yellow),
            gugu_core::process::ProcessStatus::Starting => Cell::new("启动中").fg(Color::Cyan),
            gugu_core::process::ProcessStatus::Failed(e) => Cell::new(format!("失败: {e}")).fg(Color::Red),
            gugu_core::process::ProcessStatus::Restarting => Cell::new("重启中").fg(Color::Magenta),
        };

        let uptime = p.uptime_secs.map(|s| {
            format!("{:02}:{:02}:{:02}", s / 3600, (s % 3600) / 60, s % 60)
        }).unwrap_or_default();

        table.add_row(vec![
            Cell::new(&p.name),
            status_cell,
            Cell::new(p.pid.map(|id| id.to_string()).unwrap_or_default()),
            Cell::new(uptime),
            Cell::new(p.restart_count),
        ]);
    }

    println!("{table}");
}

fn print_list_table(processes: &[gugu_core::process::ProcessInfo]) {
    if processes.is_empty() {
        println!("暂无进程，使用 gugu add 添加进程");
        return;
    }

    let mut table = Table::new();
    table.load_preset(UTF8_FULL).apply_modifier(UTF8_ROUND_CORNERS);
    table.set_header(vec![
        Cell::new("名称").add_attribute(Attribute::Bold),
        Cell::new("命令").add_attribute(Attribute::Bold),
        Cell::new("自动启动").add_attribute(Attribute::Bold),
        Cell::new("自动重启").add_attribute(Attribute::Bold),
    ]);

    for p in processes {
        let cmd = if p.args.is_empty() {
            p.command.clone()
        } else {
            format!("{} {}", p.command, p.args.join(" "))
        };
        table.add_row(vec![
            Cell::new(&p.name),
            Cell::new(cmd),
            Cell::new(if p.auto_start { "是" } else { "否" }),
            Cell::new(if p.auto_restart { "是" } else { "否" }),
        ]);
    }

    println!("{table}");
}

async fn print_api_response(resp: reqwest::Response) -> Result<()> {
    let body: serde_json::Value = resp.json().await?;
    if let Some(msg) = body.get("message") {
        println!("{}", msg);
    } else if let Some(err) = body.get("error") {
        eprintln!("错误: {}", err);
    }
    Ok(())
}
