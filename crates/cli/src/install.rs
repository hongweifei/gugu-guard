//! 开机自启的安装/卸载：Windows 走任务计划程序或 Windows Service，
//! Linux 生成 systemd unit。注册前确保配置文件真实存在（默认在程序目录）。

use anyhow::{Context, Result};
use std::path::Path;

use crate::daemon;

#[cfg(windows)]
use crate::service::SERVICE_NAME;
#[cfg(unix)]
use crate::service::{SERVICE_DISPLAY, SYSTEMD_UNIT, SYSTEMD_UNIT_PATH};

pub(crate) fn install_service(config_path: &Path, mode: &str) -> Result<()> {
    // 注册自启前确保配置文件真实存在（默认位于程序目录），
    // 避免开机后服务指向一个不存在的路径。
    match daemon::ensure_config_template(config_path) {
        Ok(true) => println!(
            "未找到配置文件，已生成默认模板: {}\n请先编辑其中的进程配置，再启动服务",
            config_path.display()
        ),
        Ok(false) => {}
        Err(e) => return Err(e),
    }
    match mode {
        "service" => {
            #[cfg(windows)]
            {
                crate::windows_service::install(config_path)
            }
            #[cfg(not(windows))]
            {
                anyhow::bail!("Windows Service 仅在 Windows 上可用");
            }
        }
        "task" => install_boot_task(config_path),
        _ => anyhow::bail!("不支持的安装模式: {mode} (可选: task, service)"),
    }
}

pub(crate) fn uninstall_service(mode: &str) -> Result<()> {
    match mode {
        "service" => {
            #[cfg(windows)]
            {
                crate::windows_service::uninstall()
            }
            #[cfg(not(windows))]
            {
                anyhow::bail!("Windows Service 仅在 Windows 上可用");
            }
        }
        "task" => uninstall_boot_task(),
        _ => anyhow::bail!("不支持的卸载模式: {mode} (可选: task, service)"),
    }
}

fn install_boot_task(config_path: &Path) -> Result<()> {
    let exe = std::env::current_exe().context("无法获取当前可执行文件路径")?;
    let config_abs = gugu_core::config::canonicalize_clean(config_path);

    #[cfg(windows)]
    {
        let tr = format!("\"{}\" run -c \"{}\"", exe.display(), config_abs.display());

        let status = std::process::Command::new("schtasks")
            .args([
                "/create",
                "/tn",
                SERVICE_NAME,
                "/tr",
                &tr,
                "/sc",
                "onstart",
                "/ru",
                "SYSTEM",
                "/rl",
                "HIGHEST",
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
             Description={display}\n\
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
            display = SERVICE_DISPLAY,
            exe = exe.display(),
            config = config_abs.display(),
            workdir = config_abs.parent().unwrap_or(Path::new("/")).display(),
        );

        std::fs::write(SYSTEMD_UNIT_PATH, &unit)
            .context("写入 systemd unit 文件失败，请使用 sudo 运行")?;

        systemctl(&["daemon-reload"], true)?;
        systemctl(&["enable", SYSTEMD_UNIT], true)?;

        println!("已安装为 systemd 服务 ({SYSTEMD_UNIT})");
        println!("  unit: {SYSTEMD_UNIT_PATH}");
        println!("  启动: systemctl start {SYSTEMD_UNIT}");
        println!("  状态: systemctl status {SYSTEMD_UNIT}");
    }

    Ok(())
}

fn uninstall_boot_task() -> Result<()> {
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
        // 卸载路径容忍未运行/未启用状态，仅 daemon-reload 失败才致命
        systemctl(&["stop", SYSTEMD_UNIT], false).ok();
        systemctl(&["disable", SYSTEMD_UNIT], false).ok();
        let _ = std::fs::remove_file(SYSTEMD_UNIT_PATH);
        systemctl(&["daemon-reload"], true)?;

        println!("已卸载 systemd 服务 ({SYSTEMD_UNIT})");
    }

    Ok(())
}

#[cfg(unix)]
fn systemctl(args: &[&str], fatal: bool) -> Result<()> {
    let status = std::process::Command::new("systemctl")
        .args(args)
        .status()
        .with_context(|| format!("无法执行 systemctl {}", args.join(" ")))?;
    if fatal && !status.success() {
        anyhow::bail!(
            "systemctl {} 失败 (退出码 {:?})",
            args.join(" "),
            status.code()
        );
    }
    Ok(())
}
