use std::ffi::OsString;
use std::sync::mpsc;
use std::time::Duration;

use anyhow::Context;
use windows_service::service::{
    ServiceControl, ServiceControlAccept, ServiceExitCode, ServiceState, ServiceStatus,
    ServiceType,
};
use windows_service::service_control_handler::{self, ServiceControlHandlerResult};
use windows_service::{define_windows_service, service_dispatcher};

use gugu_core::config::AppConfig;

const SERVICE_NAME: &str = "GuguGuard";
const SERVICE_TYPE: ServiceType = ServiceType::OWN_PROCESS;

define_windows_service!(ffi_service_main, my_service_main);

fn my_service_main(arguments: Vec<OsString>) {
    let config_path = arguments
        .iter()
        .position(|a| a == "-c")
        .and_then(|i| arguments.get(i + 1))
        .map(std::path::PathBuf::from)
        .unwrap_or_else(|| std::path::PathBuf::from("gugu.toml"));

    if let Err(e) = run_service_inner(&config_path) {
        tracing::error!("Service 运行失败: {}", e);
    }
}

pub fn start() -> anyhow::Result<()> {
    service_dispatcher::start(SERVICE_NAME, ffi_service_main)
        .map_err(|e| anyhow::anyhow!("启动 Service Dispatcher 失败: {}", e))
}

fn run_service_inner(config_path: &std::path::Path) -> windows_service::Result<()> {
    let (shutdown_tx, shutdown_rx) = mpsc::channel::<()>();

    let event_handler = move |control_event| -> ServiceControlHandlerResult {
        match control_event {
            ServiceControl::Stop => {
                let _ = shutdown_tx.send(());
                ServiceControlHandlerResult::NoError
            }
            ServiceControl::Interrogate => ServiceControlHandlerResult::NoError,
            _ => ServiceControlHandlerResult::NotImplemented,
        }
    };

    let status_handle = service_control_handler::register(SERVICE_NAME, event_handler)?;

    status_handle.set_service_status(ServiceStatus {
        service_type: SERVICE_TYPE,
        current_state: ServiceState::StartPending,
        controls_accepted: ServiceControlAccept::empty(),
        exit_code: ServiceExitCode::Win32(0),
        checkpoint: 0,
        wait_hint: Duration::from_secs(5),
        process_id: None,
    })?;

    let rt = tokio::runtime::Runtime::new().expect("无法创建 tokio runtime");
    rt.block_on(async {
        tracing_subscriber::fmt()
            .with_env_filter(
                tracing_subscriber::EnvFilter::try_from_default_env()
                    .unwrap_or_else(|_| "gugu_core=info,gugu_server=info,gugu=info".into()),
            )
            .init();

        let config = match AppConfig::load(config_path) {
            Ok(c) => c,
            Err(e) => {
                tracing::error!("加载配置文件失败: {}", e);
                let _ = status_handle.set_service_status(ServiceStatus {
                    service_type: SERVICE_TYPE,
                    current_state: ServiceState::Stopped,
                    controls_accepted: ServiceControlAccept::empty(),
                    exit_code: ServiceExitCode::Win32(1),
                    checkpoint: 0,
                    wait_hint: Duration::default(),
                    process_id: None,
                });
                return;
            }
        };

        let effective_api_key = config.daemon.api_key.clone().or_else(|| std::env::var("GUGU_API_KEY").ok());

        tracing::info!("咕咕鸽进程守护 v{} (Service 模式) 启动中...", env!("CARGO_PKG_VERSION"));
        tracing::info!("配置文件: {}", config_path.display());

        let manager = gugu_core::ProcessManager::new(&config, Some(config_path.to_path_buf()));
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
        let addr: std::net::SocketAddr = match addr_str.parse() {
            Ok(a) => a,
            Err(e) => {
                tracing::error!("解析地址失败 {}: {}", addr_str, e);
                let _ = status_handle.set_service_status(ServiceStatus {
                    service_type: SERVICE_TYPE,
                    current_state: ServiceState::Stopped,
                    controls_accepted: ServiceControlAccept::empty(),
                    exit_code: ServiceExitCode::Win32(1),
                    checkpoint: 0,
                    wait_hint: Duration::default(),
                    process_id: None,
                });
                return;
            }
        };

        let (web_shutdown_tx, web_shutdown_rx) = tokio::sync::oneshot::channel::<()>();

        let server_shared = shared.clone();
        let server_api_key = effective_api_key;
        let server_cors_origins = config.daemon.web.cors_origins.clone();
        let server_handle = tokio::spawn(async move {
            if let Err(e) = gugu_server::run_server(
                addr,
                server_shared,
                server_api_key,
                server_cors_origins,
                web_shutdown_rx,
            ).await {
                tracing::error!("Web 服务错误: {}", e);
            }
        });

        let _ = status_handle.set_service_status(ServiceStatus {
            service_type: SERVICE_TYPE,
            current_state: ServiceState::Running,
            controls_accepted: ServiceControlAccept::STOP,
            exit_code: ServiceExitCode::Win32(0),
            checkpoint: 0,
            wait_hint: Duration::default(),
            process_id: None,
        });

        loop {
            match shutdown_rx.recv_timeout(Duration::from_secs(1)) {
                Ok(()) | Err(mpsc::RecvTimeoutError::Disconnected) => break,
                Err(mpsc::RecvTimeoutError::Timeout) => continue,
            }
        }

        tracing::info!("正在优雅停止所有进程...");
        let _ = status_handle.set_service_status(ServiceStatus {
            service_type: SERVICE_TYPE,
            current_state: ServiceState::StopPending,
            controls_accepted: ServiceControlAccept::empty(),
            exit_code: ServiceExitCode::Win32(0),
            checkpoint: 0,
            wait_hint: Duration::from_secs(10),
            process_id: None,
        });

        let _ = web_shutdown_tx.send(());
        {
            let mut mgr = shared.write().await;
            mgr.stop_all().await;
        }

        match tokio::time::timeout(Duration::from_secs(5), server_handle).await {
            Ok(_) => {}
            Err(_) => {
                tracing::warn!("Web 服务关闭超时，强制终止");
            }
        }

        let _ = status_handle.set_service_status(ServiceStatus {
            service_type: SERVICE_TYPE,
            current_state: ServiceState::Stopped,
            controls_accepted: ServiceControlAccept::empty(),
            exit_code: ServiceExitCode::Win32(0),
            checkpoint: 0,
            wait_hint: Duration::default(),
            process_id: None,
        });

        tracing::info!("咕咕鸽进程守护已安全停止");
    });

    Ok(())
}

pub fn install(config_path: &std::path::Path) -> anyhow::Result<()> {
    use windows_service::service::{ServiceAccess, ServiceErrorControl, ServiceInfo, ServiceStartType};
    use windows_service::service_manager::{ServiceManager, ServiceManagerAccess};

    let exe = std::env::current_exe()
        .context("无法获取当前可执行文件路径")?;
    let config_abs = std::fs::canonicalize(config_path)
        .unwrap_or_else(|_| config_path.to_path_buf());

    let manager_access = ServiceManagerAccess::CONNECT | ServiceManagerAccess::CREATE_SERVICE;
    let service_manager = ServiceManager::local_computer(None::<&str>, manager_access)
        .map_err(|e| anyhow::anyhow!("无法连接服务管理器: {}", e))?;

    let launch_args = vec![
        OsString::from("run"),
        OsString::from("-c"),
        OsString::from(config_abs.to_string_lossy().to_string()),
        OsString::from("--mode"),
        OsString::from("service"),
    ];

    let service_info = ServiceInfo {
        name: OsString::from(SERVICE_NAME),
        display_name: OsString::from("咕咕鸽进程守护 (GuguGuard)"),
        service_type: SERVICE_TYPE,
        start_type: ServiceStartType::AutoStart,
        error_control: ServiceErrorControl::Normal,
        executable_path: exe.clone(),
        launch_arguments: launch_args,
        dependencies: vec![],
        account_name: None,
        account_password: None,
    };

    let service = service_manager
        .create_service(&service_info, ServiceAccess::CHANGE_CONFIG)
        .map_err(|e| anyhow::anyhow!("创建服务失败 (可能已存在，请先卸载): {}", e))?;

    service
        .set_description("咕咕鸽进程守护 - 跨平台进程管理工具，自动监控和重启子进程")
        .map_err(|e| anyhow::anyhow!("设置服务描述失败: {}", e))?;

    println!("成功: 已安装为 Windows 服务 ({SERVICE_NAME})");
    println!("  程序: {}", exe.display());
    println!("  配置: {}", config_abs.display());
    println!("  启动类型: 自动");
    println!("  管理: sc start {SERVICE_NAME} | sc stop {SERVICE_NAME} | sc query {SERVICE_NAME}");

    Ok(())
}

pub fn uninstall() -> anyhow::Result<()> {
    use windows_service::service::ServiceAccess;
    use windows_service::service_manager::{ServiceManager, ServiceManagerAccess};

    let manager_access = ServiceManagerAccess::CONNECT;
    let service_manager = ServiceManager::local_computer(None::<&str>, manager_access)
        .map_err(|e| anyhow::anyhow!("无法连接服务管理器: {}", e))?;

    let service_access = ServiceAccess::QUERY_STATUS | ServiceAccess::STOP | ServiceAccess::DELETE;
    let service = service_manager
        .open_service(SERVICE_NAME, service_access)
        .map_err(|e| anyhow::anyhow!("打开服务失败 (可能未安装): {}", e))?;

    service
        .delete()
        .map_err(|e| anyhow::anyhow!("删除服务失败: {}", e))?;

    let status = service.query_status()
        .map_err(|e| anyhow::anyhow!("查询服务状态失败: {}", e))?;

    if status.current_state != ServiceState::Stopped {
        match service.stop() {
            Ok(_) => println!("已发送停止请求..."),
            Err(_) => println!("服务将在下次重启时被移除"),
        }
    }

    println!("成功: 已卸载 Windows 服务 ({SERVICE_NAME})");

    Ok(())
}
