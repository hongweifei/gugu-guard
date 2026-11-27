//! 开机自启的安装/卸载。
//!
//! 三种模式（`--mode`）：
//! - `task`（默认）：系统级自启 —— Windows 任务计划程序 / Linux systemd 系统单元，**需要管理员**；
//! - `service`：仅 Windows，注册为 Windows 服务（自动启动 + 失败自动恢复），**需要管理员**；
//! - `user`：**当前用户级自启，无需管理员** —— Windows 当前用户 Run 键（登录即启）/
//!   Linux systemd 用户单元（配合 `loginctl enable-linger` 可做到未登录也起）。
//!
//! 安装前做权限预检，安装后**回读真实状态**（systemctl is-enabled / schtasks /query /
//! reg query / sc query），卸载对「本来就没装」保持容错。`--dry-run` 只打印将要写入或
//! 执行的内容，不碰系统。
//!
//! 子模块分工：
//! - [`units`]：纯构造函数（任务命令、用户级命令、systemd unit 文本），任意平台可单测
//! - [`windows`]：任务计划 / Run 键 / Windows 服务三种落地方式
//! - [`unix`]：systemd 系统单元与用户单元

use anyhow::{Context, Result};
use std::path::{Path, PathBuf};

use crate::daemon;

mod units;

#[cfg(unix)]
mod unix;
#[cfg(windows)]
mod windows;

/// 自启模式。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Mode {
    /// 系统级：Windows 任务计划 / Linux systemd 系统单元。
    Task,
    /// 仅 Windows：Windows 服务。
    Service,
    /// 当前用户级：Windows Run 键 / Linux systemd 用户单元（无需管理员）。
    User,
}

impl Mode {
    fn parse(raw: &str) -> Result<Self> {
        match raw {
            "task" => Ok(Self::Task),
            "service" => Ok(Self::Service),
            "user" => Ok(Self::User),
            other => anyhow::bail!("不支持的安装模式: {other} (可选: task, service, user)"),
        }
    }
}

// ── 入口 ──────────────────────────────────────────────────────

pub(crate) fn install_service(config_path: &Path, mode: &str, dry_run: bool) -> Result<()> {
    let mode = Mode::parse(mode)?;
    prepare_config(config_path, dry_run)?;
    dispatch_install(mode, config_path, dry_run)
}

pub(crate) fn uninstall_service(mode: &str, dry_run: bool) -> Result<()> {
    dispatch_uninstall(Mode::parse(mode)?, dry_run)
}

/// 注册自启前确保配置文件真实存在（默认位于程序目录），
/// 避免开机后服务指向一个不存在的路径。预览模式不落盘。
fn prepare_config(config_path: &Path, dry_run: bool) -> Result<()> {
    if dry_run {
        if !config_path.exists() {
            println!(
                "[dry-run] 配置文件不存在，实际安装时会先生成模板: {}",
                config_path.display()
            );
        }
        return Ok(());
    }
    match daemon::ensure_config_template(config_path) {
        Ok(true) => println!(
            "未找到配置文件，已生成默认模板: {}\n请先编辑其中的进程配置，再启动服务",
            config_path.display()
        ),
        Ok(false) => {}
        Err(e) => return Err(e),
    }
    Ok(())
}

#[cfg(windows)]
fn dispatch_install(mode: Mode, config_path: &Path, dry_run: bool) -> Result<()> {
    match mode {
        Mode::Task => windows::install_system_level(config_path, dry_run),
        Mode::Service => windows::install_windows_service(config_path, dry_run),
        Mode::User => windows::install_user_level(config_path, dry_run),
    }
}

#[cfg(unix)]
fn dispatch_install(mode: Mode, config_path: &Path, dry_run: bool) -> Result<()> {
    match mode {
        Mode::Task => unix::install_system_level(config_path, dry_run),
        Mode::Service => anyhow::bail!(
            "Windows Service 仅在 Windows 上可用（Linux 请用 --mode task 或 --mode user）"
        ),
        Mode::User => unix::install_user_level(config_path, dry_run),
    }
}

#[cfg(windows)]
fn dispatch_uninstall(mode: Mode, dry_run: bool) -> Result<()> {
    match mode {
        Mode::Task => windows::uninstall_system_level(dry_run),
        Mode::Service => windows::uninstall_windows_service(dry_run),
        Mode::User => windows::uninstall_user_level(dry_run),
    }
}

#[cfg(unix)]
fn dispatch_uninstall(mode: Mode, dry_run: bool) -> Result<()> {
    match mode {
        Mode::Task => unix::uninstall_system_level(dry_run),
        Mode::Service => anyhow::bail!("Windows Service 仅在 Windows 上可用"),
        Mode::User => unix::uninstall_user_level(dry_run),
    }
}

// ── 通用执行辅助 ──────────────────────────────────────────────

pub(super) fn current_exe() -> Result<PathBuf> {
    std::env::current_exe().context("无法获取当前可执行文件路径")
}

pub(super) fn canonical(path: &Path) -> PathBuf {
    gugu_core::config::canonicalize_clean(path)
}

/// 执行外部命令；`dry_run` 时只打印将要执行的命令。返回是否成功。
pub(super) fn run(program: &str, args: &[String], dry_run: bool) -> Result<bool> {
    if dry_run {
        println!("[dry-run] {program} {}", args.join(" "));
        return Ok(true);
    }
    let status = std::process::Command::new(program)
        .args(args)
        .status()
        .with_context(|| format!("无法执行 {program}，请确认它在 PATH 中"))?;
    Ok(status.success())
}

/// 执行并取回 stdout（用于安装后的状态回读）。
pub(super) fn capture(program: &str, args: &[String]) -> Result<String> {
    let out = std::process::Command::new(program)
        .args(args)
        .output()
        .with_context(|| format!("无法执行 {program}"))?;
    Ok(String::from_utf8_lossy(&out.stdout).trim().to_string())
}

pub(super) fn args_of(items: &[&str]) -> Vec<String> {
    items.iter().map(|s| (*s).to_string()).collect()
}

/// 权限预检：给出「怎么带权限重跑」或「改用无需管理员的 user 模式」两条明确出路。
/// `dry_run` 只做预览、不改系统，因此不拦（否则任何人都无法预览系统级安装）。
pub(super) fn ensure_admin(dry_run: bool) -> Result<()> {
    if dry_run || is_elevated() {
        return Ok(());
    }
    anyhow::bail!(
        "该模式需要管理员权限。\n\
         · Windows：以管理员身份打开终端后重跑；或用 `gugu install --mode user`（无需管理员，登录即启）\n\
         · Linux：用 sudo 重跑；或用 `gugu install --mode user`（配合 loginctl enable-linger 可未登录也起）"
    )
}

#[cfg(windows)]
fn is_elevated() -> bool {
    // SAFETY: IsUserAnAdmin 无参数、无副作用，仅查询当前进程令牌。
    unsafe { windows_sys::Win32::UI::Shell::IsUserAnAdmin() != 0 }
}

#[cfg(unix)]
fn is_elevated() -> bool {
    // SAFETY: geteuid 无参数、无副作用。
    unsafe { libc::geteuid() == 0 }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn mode_parse_accepts_three_modes() {
        assert_eq!(Mode::parse("task").unwrap(), Mode::Task);
        assert_eq!(Mode::parse("service").unwrap(), Mode::Service);
        assert_eq!(Mode::parse("user").unwrap(), Mode::User);
        assert!(Mode::parse("systemd").is_err(), "未知模式必须报错");
    }
}
