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
        .map_err(|e| anyhow::anyhow!("启动 Service Dispatcher 失败: {e}"))
}

fn set_status(
    status_handle: windows_service::service_control_handler::ServiceStatusHandle,
    state: ServiceState,
    controls_accepted: ServiceControlAccept,
    exit_code: u32,
    wait_hint: Duration,
) {
    let _ = status_handle.set_service_status(ServiceStatus {
        service_type: SERVICE_TYPE,
        current_state: state,
        controls_accepted,
        exit_code: ServiceExitCode::Win32(exit_code),
        checkpoint: 0,
        wait_hint,
        process_id: None,
    });
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

    set_status(status_handle, ServiceState::StartPending, ServiceControlAccept::empty(), 0, Duration::from_secs(5));

    let rt = tokio::runtime::Runtime::new().expect("无法创建 tokio runtime");
    rt.block_on(async {
        tracing_subscriber::fmt()
            .with_env_filter(
                tracing_subscriber::EnvFilter::try_from_default_env()
                    .unwrap_or_else(|_| "gugu_core=info,gugu_server=info,gugu=info".into()),
            )
            .with_ansi(false) // Windows 服务无终端，禁用 ANSI 转义
            .init();

        let handles = match crate::daemon::run_core(config_path, None).await {
            Ok(h) => h,
            Err(e) => {
                tracing::error!("启动核心失败: {}", e);
                set_status(status_handle, ServiceState::Stopped, ServiceControlAccept::empty(), 1, Duration::default());
                return;
            }
        };

        set_status(status_handle, ServiceState::Running, ServiceControlAccept::STOP, 0, Duration::default());

        loop {
            match shutdown_rx.recv_timeout(Duration::from_secs(1)) {
                Ok(()) | Err(mpsc::RecvTimeoutError::Disconnected) => break,
                Err(mpsc::RecvTimeoutError::Timeout) => {}
            }
        }

        set_status(status_handle, ServiceState::StopPending, ServiceControlAccept::empty(), 0, Duration::from_secs(10));

        crate::daemon::graceful_shutdown(handles).await;

        set_status(status_handle, ServiceState::Stopped, ServiceControlAccept::empty(), 0, Duration::default());
    });

    Ok(())
}

pub fn install(config_path: &std::path::Path) -> anyhow::Result<()> {
    use windows_service::service::{ServiceAccess, ServiceErrorControl, ServiceInfo, ServiceStartType};
    use windows_service::service_manager::{ServiceManager, ServiceManagerAccess};

    let exe = std::env::current_exe()
        .context("无法获取当前可执行文件路径")?;
    let config_abs = gugu_core::config::canonicalize_clean(config_path);

    let manager_access = ServiceManagerAccess::CONNECT | ServiceManagerAccess::CREATE_SERVICE;
    let service_manager = ServiceManager::local_computer(None::<&str>, manager_access)
        .map_err(|e| anyhow::anyhow!("无法连接服务管理器: {e}"))?;

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
        .map_err(|e| anyhow::anyhow!("创建服务失败 (可能已存在，请先卸载): {e}"))?;

    service
        .set_description("咕咕鸽进程守护 - 跨平台进程管理工具，自动监控和重启子进程")
        .map_err(|e| anyhow::anyhow!("设置服务描述失败: {e}"))?;

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
        .map_err(|e| anyhow::anyhow!("无法连接服务管理器: {e}"))?;

    let service_access = ServiceAccess::QUERY_STATUS | ServiceAccess::STOP | ServiceAccess::DELETE;
    let service = service_manager
        .open_service(SERVICE_NAME, service_access)
        .map_err(|e| anyhow::anyhow!("打开服务失败 (可能未安装): {e}"))?;

    service
        .delete()
        .map_err(|e| anyhow::anyhow!("删除服务失败: {e}"))?;

    let status = service.query_status()
        .map_err(|e| anyhow::anyhow!("查询服务状态失败: {e}"))?;

    if status.current_state != ServiceState::Stopped {
        match service.stop() {
            Ok(_) => println!("已发送停止请求..."),
            Err(_) => println!("服务将在下次重启时被移除"),
        }
    }

    println!("成功: 已卸载 Windows 服务 ({SERVICE_NAME})");

    Ok(())
}
