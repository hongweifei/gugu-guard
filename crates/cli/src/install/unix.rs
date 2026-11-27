//! systemd 落地方式：系统单元与当前用户单元。

use anyhow::{Context, Result};
use std::path::{Path, PathBuf};

use crate::service::{SYSTEMD_UNIT, SYSTEMD_UNIT_PATH, SYSTEMD_USER_UNIT_REL};

use super::units::build_systemd_unit;
use super::{args_of, canonical, capture, current_exe, ensure_admin, run};

pub(super) fn install_system_level(config_path: &Path, dry_run: bool) -> Result<()> {
    ensure_admin(dry_run)?;
    let exe = current_exe()?;
    let config_abs = canonical(config_path);
    let workdir = config_abs.parent().unwrap_or(Path::new("/"));
    let unit = build_systemd_unit(
        &exe,
        &config_abs,
        workdir,
        crate::service::SERVICE_DISPLAY,
        false,
    );

    if dry_run {
        println!("[dry-run] 将写入 {SYSTEMD_UNIT_PATH}:");
        print!("{unit}");
        println!("[dry-run] systemctl daemon-reload");
        println!("[dry-run] systemctl enable {SYSTEMD_UNIT}");
        return Ok(());
    }

    std::fs::write(SYSTEMD_UNIT_PATH, &unit).context("写入 systemd unit 文件失败，请使用 sudo")?;
    systemctl(&["daemon-reload"], true)?;
    systemctl(&["enable", SYSTEMD_UNIT], true)?;

    println!("已安装为 systemd 服务 ({SYSTEMD_UNIT})");
    println!("  unit: {SYSTEMD_UNIT_PATH}");
    match capture("systemctl", &args_of(&["is-enabled", SYSTEMD_UNIT])) {
        Ok(text) => println!("  回读: is-enabled = {text}"),
        Err(e) => println!("  ⚠ 回读失败: {e}"),
    }
    println!("  启动: systemctl start {SYSTEMD_UNIT}");
    println!("  状态: systemctl status {SYSTEMD_UNIT}");
    Ok(())
}

pub(super) fn uninstall_system_level(dry_run: bool) -> Result<()> {
    ensure_admin(dry_run)?;
    if dry_run {
        println!("[dry-run] systemctl stop {SYSTEMD_UNIT}");
        println!("[dry-run] systemctl disable {SYSTEMD_UNIT}");
        println!("[dry-run] 删除 {SYSTEMD_UNIT_PATH}");
        println!("[dry-run] systemctl daemon-reload");
        return Ok(());
    }
    // 容忍未运行/未启用：仅 daemon-reload 失败才致命
    systemctl(&["stop", SYSTEMD_UNIT], false).ok();
    systemctl(&["disable", SYSTEMD_UNIT], false).ok();
    if !Path::new(SYSTEMD_UNIT_PATH).exists() {
        println!("未找到 systemd 单元 ({SYSTEMD_UNIT_PATH})，无需卸载");
        return Ok(());
    }
    let _ = std::fs::remove_file(SYSTEMD_UNIT_PATH);
    systemctl(&["daemon-reload"], true)?;
    println!("已卸载 systemd 服务 ({SYSTEMD_UNIT})");
    Ok(())
}

pub(super) fn install_user_level(config_path: &Path, dry_run: bool) -> Result<()> {
    let exe = current_exe()?;
    let config_abs = canonical(config_path);
    let workdir = config_abs.parent().unwrap_or(Path::new("/"));
    let unit_path = user_unit_path()?;
    let unit = build_systemd_unit(
        &exe,
        &config_abs,
        workdir,
        crate::service::SERVICE_DISPLAY,
        true,
    );

    if dry_run {
        println!("[dry-run] 将写入 {}:", unit_path.display());
        print!("{unit}");
        println!("[dry-run] systemctl --user daemon-reload");
        println!("[dry-run] systemctl --user enable {SYSTEMD_UNIT}");
        println!("[dry-run] loginctl enable-linger $USER（未登录也运行）");
        return Ok(());
    }

    if let Some(parent) = unit_path.parent() {
        std::fs::create_dir_all(parent)
            .with_context(|| format!("创建目录失败: {}", parent.display()))?;
    }
    std::fs::write(&unit_path, &unit)
        .with_context(|| format!("写入用户单元失败: {}", unit_path.display()))?;

    let reloaded = systemctl_user(&["daemon-reload"], true)?;
    if !reloaded {
        println!(
            "⚠ 无法连接用户级 systemd 会话，单元已写入 {}\n  请在你的桌面/登录会话里执行: systemctl --user daemon-reload && systemctl --user enable {SYSTEMD_UNIT}",
            unit_path.display()
        );
        return Ok(());
    }
    systemctl_user(&["enable", SYSTEMD_UNIT], true)?;

    println!("已安装为当前用户自启 (systemd --user: {SYSTEMD_UNIT})");
    println!("  unit: {}", unit_path.display());
    match capture(
        "systemctl",
        &args_of(&["--user", "is-enabled", SYSTEMD_UNIT]),
    ) {
        Ok(text) => println!("  回读: is-enabled = {text}"),
        Err(e) => println!("  ⚠ 回读失败: {e}"),
    }
    // 未登录也运行：需要 linger，可能触发 polkit 授权，失败不致命
    let user = std::env::var("USER").unwrap_or_else(|_| "$USER".to_string());
    match run("loginctl", &args_of(&["enable-linger", &user]), false) {
        Ok(true) => println!("  已开启 linger（未登录也会在开机时启动）"),
        Ok(false) => {
            println!("  ⚠ linger 未开启：仅在登录后启动，可手动执行 loginctl enable-linger {user}")
        }
        Err(e) => println!("  ⚠ 无法执行 loginctl: {e}"),
    }
    println!("  启动: systemctl --user start {SYSTEMD_UNIT}");
    Ok(())
}

pub(super) fn uninstall_user_level(dry_run: bool) -> Result<()> {
    let unit_path = user_unit_path()?;
    if dry_run {
        println!("[dry-run] systemctl --user stop {SYSTEMD_UNIT}");
        println!("[dry-run] systemctl --user disable {SYSTEMD_UNIT}");
        println!("[dry-run] 删除 {}", unit_path.display());
        println!("[dry-run] systemctl --user daemon-reload");
        return Ok(());
    }
    let _ = systemctl_user(&["stop", SYSTEMD_UNIT], false);
    let _ = systemctl_user(&["disable", SYSTEMD_UNIT], false);
    if !unit_path.exists() {
        println!("未找到用户级单元 ({}), 无需卸载", unit_path.display());
        return Ok(());
    }
    let _ = std::fs::remove_file(&unit_path);
    let _ = systemctl_user(&["daemon-reload"], false);
    println!("已卸载当前用户自启 (systemd --user: {SYSTEMD_UNIT})");
    Ok(())
}

fn user_unit_path() -> Result<PathBuf> {
    let home = std::env::var("HOME").context("未设置 HOME，无法定位用户级 systemd 目录")?;
    Ok(PathBuf::from(home).join(SYSTEMD_USER_UNIT_REL))
}

fn systemctl(args: &[&str], fatal: bool) -> Result<()> {
    let status = std::process::Command::new("systemctl")
        .args(args)
        .status()
        .with_context(|| format!("无法执行 systemctl {}", args.join(" ")))?;
    if fatal && !status.success() {
        anyhow::bail!(
            "systemctl {} 失败 (退出码 {:?})，请确认使用 sudo 且系统使用 systemd",
            args.join(" "),
            status.code()
        );
    }
    Ok(())
}

/// `systemctl --user <args>`；返回是否执行成功（用户会话不可用时返回 false）。
fn systemctl_user(args: &[&str], fatal: bool) -> Result<bool> {
    let mut full = vec!["--user"];
    full.extend_from_slice(args);
    let status = std::process::Command::new("systemctl")
        .args(&full)
        .status()
        .with_context(|| format!("无法执行 systemctl {}", full.join(" ")))?;
    if status.success() {
        return Ok(true);
    }
    if fatal {
        anyhow::bail!(
            "systemctl --user {} 失败 (退出码 {:?})，请在登录会话中执行，或改用 --mode task (需 sudo)",
            args.join(" "),
            status.code()
        );
    }
    Ok(false)
}
