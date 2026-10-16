use std::ffi::OsString;
use std::sync::mpsc;
use std::time::Duration;

use anyhow::Context;
use windows_service::service::{
    ServiceControl, ServiceControlAccept, ServiceExitCode, ServiceState, ServiceStatus, ServiceType,
};
use windows_service::service_control_handler::{self, ServiceControlHandlerResult};
use windows_service::{define_windows_service, service_dispatcher};

use crate::service::{SERVICE_DISPLAY, SERVICE_NAME};

const SERVICE_TYPE: ServiceType = ServiceType::OWN_PROCESS;

define_windows_service!(ffi_service_main, my_service_main);

fn my_service_main(arguments: Vec<OsString>) {
    let config_path = arguments
        .iter()
        .position(|a| a == "-c")
        .and_then(|i| arguments.get(i + 1))
        .map(std::path::PathBuf::from)
        .unwrap_or_else(gugu_core::config::default_config_path);

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

    set_status(
        status_handle,
        ServiceState::StartPending,
        ServiceControlAccept::empty(),
        0,
        Duration::from_secs(5),
    );

    let rt = tokio::runtime::Runtime::new().expect("无法创建 tokio runtime");
    rt.block_on(async {
        // 日志初始化 (文件层为主，服务无控制台) 统一在 daemon::run_core 内完成
        let handles = match crate::daemon::run_core(config_path, None).await {
            Ok(h) => h,
            Err(e) => {
                tracing::error!("启动核心失败: {}", e);
                set_status(
                    status_handle,
                    ServiceState::Stopped,
                    ServiceControlAccept::empty(),
                    1,
                    Duration::default(),
                );
                return;
            }
        };

        set_status(
            status_handle,
            ServiceState::Running,
            ServiceControlAccept::STOP,
            0,
            Duration::default(),
        );

        loop {
            match shutdown_rx.recv_timeout(Duration::from_secs(1)) {
                Ok(()) | Err(mpsc::RecvTimeoutError::Disconnected) => break,
                Err(mpsc::RecvTimeoutError::Timeout) => {}
            }
        }

        set_status(
            status_handle,
            ServiceState::StopPending,
            ServiceControlAccept::empty(),
            0,
            Duration::from_secs(10),
        );

        crate::daemon::graceful_shutdown(handles).await;

        set_status(
            status_handle,
            ServiceState::Stopped,
            ServiceControlAccept::empty(),
            0,
            Duration::default(),
        );
    });

    Ok(())
}

pub fn install(config_path: &std::path::Path, dry_run: bool) -> anyhow::Result<()> {
    use windows_service::service::{
        ServiceAccess, ServiceErrorControl, ServiceInfo, ServiceStartType,
    };
    use windows_service::service_manager::{ServiceManager, ServiceManagerAccess};

    let exe = std::env::current_exe().context("无法获取当前可执行文件路径")?;
    let config_abs = gugu_core::config::canonicalize_clean(config_path);

    let launch_args = vec![
        OsString::from("run"),
        OsString::from("-c"),
        OsString::from(config_abs.to_string_lossy().to_string()),
        OsString::from("--mode"),
        OsString::from("service"),
    ];

    if dry_run {
        println!("[dry-run] 将注册 Windows 服务:");
        println!("  名称: {SERVICE_NAME}  显示名: {SERVICE_DISPLAY}");
        println!("  启动类型: AutoStart (开机自启)");
        println!("  可执行: {}", exe.display());
        println!(
            "  参数: {}",
            launch_args
                .iter()
                .map(|a| a.to_string_lossy().to_string())
                .collect::<Vec<_>>()
                .join(" ")
        );
        println!(
            "[dry-run] sc failure {SERVICE_NAME} reset= 86400 actions= restart/5000/restart/10000/restart/30000"
        );
        return Ok(());
    }

    let manager_access = ServiceManagerAccess::CONNECT | ServiceManagerAccess::CREATE_SERVICE;
    let service_manager = ServiceManager::local_computer(None::<&str>, manager_access)
        .map_err(|e| anyhow::anyhow!("无法连接服务管理器: {e}"))?;

    let service_info = ServiceInfo {
        name: OsString::from(SERVICE_NAME),
        display_name: OsString::from(SERVICE_DISPLAY),
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

    // 回读启动类型：不假设 create_service 一定按预期落盘
    match service.query_config() {
        Ok(cfg) => println!("  回读: 启动类型 = {:?}", cfg.start_type),
        Err(e) => println!("  ⚠ 回读启动类型失败: {e}"),
    }

    // 失败自动恢复：服务崩溃后由服务管理器按阶梯间隔拉起（不影响开机自启，失败只提示）
    let failure_args = [
        "failure",
        SERVICE_NAME,
        "reset=",
        "86400",
        "actions=",
        "restart/5000/restart/10000/restart/30000",
    ];
    match std::process::Command::new("sc").args(failure_args).status() {
        Ok(s) if s.success() => {
            let back = std::process::Command::new("sc")
                .args(["qfailure", SERVICE_NAME])
                .output()
                .map(|o| String::from_utf8_lossy(&o.stdout).trim().to_string())
                .unwrap_or_default();
            let first = back.lines().next().unwrap_or("").trim().to_string();
            println!("  已设置崩溃自动重启 (阶梯 5s/10s/30s): {first}");
        }
        Ok(s) => println!(
            "  ⚠ 设置崩溃自动重启失败 (退出码 {:?})，不影响开机自启",
            s.code()
        ),
        Err(e) => println!("  ⚠ 无法执行 sc 设置崩溃自动重启: {e}"),
    }

    println!("  管理: sc start {SERVICE_NAME} | sc stop {SERVICE_NAME} | sc query {SERVICE_NAME}");

    Ok(())
}

pub fn uninstall(dry_run: bool) -> anyhow::Result<()> {
    use windows_service::service::ServiceAccess;
    use windows_service::service_manager::{ServiceManager, ServiceManagerAccess};

    if dry_run {
        println!("[dry-run] sc stop {SERVICE_NAME}");
        println!("[dry-run] sc delete {SERVICE_NAME}");
        return Ok(());
    }

    let manager_access = ServiceManagerAccess::CONNECT;
    let service_manager = ServiceManager::local_computer(None::<&str>, manager_access)
        .map_err(|e| anyhow::anyhow!("无法连接服务管理器: {e}"))?;

    let service_access = ServiceAccess::QUERY_STATUS | ServiceAccess::STOP | ServiceAccess::DELETE;
    let service = match service_manager.open_service(SERVICE_NAME, service_access) {
        Ok(s) => s,
        Err(e) => {
            // 与 systemd 侧口径一致：本来就没装不算错误
            println!("未找到 Windows 服务 ({SERVICE_NAME})，无需卸载 ({e})");
            return Ok(());
        }
    };

    // 顺序修正：先停止（并等它真的停下）再删除；反序可能留下仍在运行的进程
    let status = service
        .query_status()
        .map_err(|e| anyhow::anyhow!("查询服务状态失败: {e}"))?;
    if status.current_state != ServiceState::Stopped {
        println!("正在停止服务...");
        if let Err(e) = service.stop() {
            println!("⚠ 发送停止请求失败: {e}（继续尝试删除）");
        }
        for _ in 0..20 {
            std::thread::sleep(Duration::from_millis(500));
            match service.query_status() {
                Ok(s) if s.current_state == ServiceState::Stopped => break,
                Ok(_) => {}
                Err(_) => break,
            }
        }
    }

    service
        .delete()
        .map_err(|e| anyhow::anyhow!("删除服务失败: {e}"))?;

    // 回读：确认已不可打开（服务管理器常把它标记为待删除）
    match service_manager.open_service(SERVICE_NAME, ServiceAccess::QUERY_STATUS) {
        Ok(_) => println!("已卸载 Windows 服务 ({SERVICE_NAME})（仍标记为待删除，重启后彻底移除）"),
        Err(_) => println!("成功: 已卸载 Windows 服务 ({SERVICE_NAME})"),
    }

    Ok(())
}
