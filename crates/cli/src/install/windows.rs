//! Windows 落地方式：任务计划、当前用户 Run 键与 Windows 服务。

use anyhow::Result;
use std::path::Path;

use crate::service::{RUN_KEY, SERVICE_NAME};

use super::units::{run_command, run_command_detached};
use super::{args_of, canonical, capture, current_exe, ensure_admin, run};

pub(super) fn install_system_level(config_path: &Path, dry_run: bool) -> Result<()> {
    ensure_admin(dry_run)?;
    let exe = current_exe()?;
    let config_abs = canonical(config_path);

    let tr = run_command(&exe, &config_abs);
    let args = args_of(&[
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
    ]);
    if !run("schtasks", &args, dry_run)? {
        anyhow::bail!("注册开机任务失败，请确认以管理员身份运行");
    }
    if dry_run {
        return Ok(());
    }

    println!("已安装为开机自启任务 (任务计划程序: {SERVICE_NAME})");
    println!("  程序: {}", exe.display());
    println!("  配置: {}", config_abs.display());
    match capture("schtasks", &args_of(&["/query", "/tn", SERVICE_NAME])) {
        Ok(text) => println!(
            "  回读: {}",
            text.lines().next().unwrap_or("(无输出)").trim()
        ),
        Err(e) => println!("  ⚠ 回读失败: {e}"),
    }
    println!("  注意: 以 SYSTEM 身份在开机时启动；卸载用 `gugu uninstall`");
    Ok(())
}

pub(super) fn uninstall_system_level(dry_run: bool) -> Result<()> {
    ensure_admin(dry_run)?;
    if dry_run {
        println!("[dry-run] schtasks /end /tn {SERVICE_NAME}");
        println!("[dry-run] schtasks /delete /tn {SERVICE_NAME} /f");
        return Ok(());
    }
    // 先结束正在运行的实例，再删除任务（否则删完任务进程仍在跑）
    let _ = run("schtasks", &args_of(&["/end", "/tn", SERVICE_NAME]), false)?;
    if !run(
        "schtasks",
        &args_of(&["/delete", "/tn", SERVICE_NAME, "/f"]),
        false,
    )? {
        println!("未找到开机自启任务 ({SERVICE_NAME})，无需卸载");
        return Ok(());
    }
    println!("已卸载开机自启任务 ({SERVICE_NAME})");
    Ok(())
}

pub(super) fn install_user_level(config_path: &Path, dry_run: bool) -> Result<()> {
    let exe = current_exe()?;
    let config_abs = canonical(config_path);
    let args = args_of(&[
        "add",
        RUN_KEY,
        "/v",
        SERVICE_NAME,
        "/t",
        "REG_SZ",
        "/d",
        &run_command_detached(&exe, &config_abs),
        "/f",
    ]);
    if !run("reg", &args, dry_run)? {
        anyhow::bail!("写入当前用户自启项失败 ({RUN_KEY})");
    }
    if dry_run {
        return Ok(());
    }

    println!("已安装为当前用户登录自启 (注册表 {RUN_KEY} → {SERVICE_NAME})");
    println!("  程序: {}", exe.display());
    println!("  配置: {}", config_abs.display());
    match capture("reg", &args_of(&["query", RUN_KEY, "/v", SERVICE_NAME])) {
        Ok(text) => {
            let value = text
                .lines()
                .find(|l| l.contains(SERVICE_NAME))
                .unwrap_or("(无输出)")
                .trim();
            println!("  回读: {value}");
        }
        Err(e) => println!("  ⚠ 回读失败: {e}"),
    }
    println!("  说明: 登录后自动启动（无需管理员）；若要不登录也跑，请用 --mode task（需管理员）");
    Ok(())
}

pub(super) fn uninstall_user_level(dry_run: bool) -> Result<()> {
    if dry_run {
        println!("[dry-run] reg delete {RUN_KEY} /v {SERVICE_NAME} /f");
        return Ok(());
    }
    if !run(
        "reg",
        &args_of(&["delete", RUN_KEY, "/v", SERVICE_NAME, "/f"]),
        false,
    )? {
        println!("未找到当前用户自启项 ({SERVICE_NAME})，无需卸载");
        return Ok(());
    }
    println!("已卸载当前用户自启项 ({SERVICE_NAME})");
    Ok(())
}

pub(super) fn install_windows_service(config_path: &Path, dry_run: bool) -> Result<()> {
    ensure_admin(dry_run)?;
    crate::windows_service::install(config_path, dry_run)
}

pub(super) fn uninstall_windows_service(dry_run: bool) -> Result<()> {
    ensure_admin(dry_run)?;
    crate::windows_service::uninstall(dry_run)
}
