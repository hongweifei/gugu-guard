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

use anyhow::{Context, Result};
use std::path::{Path, PathBuf};

use crate::daemon;

#[cfg(windows)]
use crate::service::{RUN_KEY, SERVICE_NAME};
#[cfg(unix)]
use crate::service::{SYSTEMD_UNIT, SYSTEMD_UNIT_PATH, SYSTEMD_USER_UNIT_REL};

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

// ── 纯构造函数（可在任意平台单测）─────────────────────────────

/// Windows：任务计划 / Run 项 / 服务共用的启动命令。路径一律加引号，含空格也安全。
/// （unix 下用不到，故仅在 Windows 与测试构建里编译，避免 dead_code 在 CI 上变成错误。）
#[cfg(any(windows, test))]
fn run_command(exe: &Path, config: &Path) -> String {
    format!("\"{}\" run -c \"{}\"", exe.display(), config.display())
}

/// Windows：用户级自启命令 —— 追加 `--detach`，登录时由自己转入后台而非弹控制台。
#[cfg(any(windows, test))]
fn run_command_detached(exe: &Path, config: &Path) -> String {
    format!("{} --detach", run_command(exe, config))
}

/// systemd unit 文本。`ExecStart`/`WorkingDirectory` 一律加引号，含空格的安装路径可用。
#[cfg(any(unix, test))]
fn build_systemd_unit(
    exe: &Path,
    config: &Path,
    workdir: &Path,
    display: &str,
    user_level: bool,
) -> String {
    let (after, wanted) = if user_level {
        ("default.target", "default.target")
    } else {
        ("network.target", "multi-user.target")
    };
    format!(
        "[Unit]\n\
         Description={display}\n\
         After={after}\n\n\
         [Service]\n\
         Type=simple\n\
         ExecStart=\"{exe}\" run -c \"{config}\"\n\
         Restart=on-failure\n\
         RestartSec=5\n\
         KillSignal=SIGTERM\n\
         TimeoutStopSec=30\n\
         WorkingDirectory=\"{workdir}\"\n\n\
         [Install]\n\
         WantedBy={wanted}\n",
        display = display,
        after = after,
        exe = exe.display(),
        config = config.display(),
        workdir = workdir.display(),
        wanted = wanted,
    )
}

// ── 入口 ──────────────────────────────────────────────────────

pub(crate) fn install_service(config_path: &Path, mode: &str, dry_run: bool) -> Result<()> {
    let mode = Mode::parse(mode)?;

    // 注册自启前确保配置文件真实存在（默认位于程序目录），
    // 避免开机后服务指向一个不存在的路径。预览模式不落盘。
    if dry_run {
        if !config_path.exists() {
            println!(
                "[dry-run] 配置文件不存在，实际安装时会先生成模板: {}",
                config_path.display()
            );
        }
    } else {
        match daemon::ensure_config_template(config_path) {
            Ok(true) => println!(
                "未找到配置文件，已生成默认模板: {}\n请先编辑其中的进程配置，再启动服务",
                config_path.display()
            ),
            Ok(false) => {}
            Err(e) => return Err(e),
        }
    }

    match mode {
        Mode::Task => install_system_level(config_path, dry_run),
        Mode::Service => install_windows_service(config_path, dry_run),
        Mode::User => install_user_level(config_path, dry_run),
    }
}

pub(crate) fn uninstall_service(mode: &str, dry_run: bool) -> Result<()> {
    match Mode::parse(mode)? {
        Mode::Task => uninstall_system_level(dry_run),
        Mode::Service => uninstall_windows_service(dry_run),
        Mode::User => uninstall_user_level(dry_run),
    }
}

// ── 通用执行辅助 ──────────────────────────────────────────────

fn current_exe() -> Result<PathBuf> {
    std::env::current_exe().context("无法获取当前可执行文件路径")
}

fn canonical(path: &Path) -> PathBuf {
    gugu_core::config::canonicalize_clean(path)
}

/// 执行外部命令；`dry_run` 时只打印将要执行的命令。返回是否成功。
fn run(program: &str, args: &[String], dry_run: bool) -> Result<bool> {
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
fn capture(program: &str, args: &[String]) -> Result<String> {
    let out = std::process::Command::new(program)
        .args(args)
        .output()
        .with_context(|| format!("无法执行 {program}"))?;
    Ok(String::from_utf8_lossy(&out.stdout).trim().to_string())
}

fn args_of(items: &[&str]) -> Vec<String> {
    items.iter().map(|s| (*s).to_string()).collect()
}

/// 权限预检：给出「怎么带权限重跑」或「改用无需管理员的 user 模式」两条明确出路。
/// `dry_run` 只做预览、不改系统，因此不拦（否则任何人都无法预览系统级安装）。
fn ensure_admin(dry_run: bool) -> Result<()> {
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

// ── Windows ───────────────────────────────────────────────────

#[cfg(windows)]
fn install_system_level(config_path: &Path, dry_run: bool) -> Result<()> {
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

#[cfg(windows)]
fn uninstall_system_level(dry_run: bool) -> Result<()> {
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

#[cfg(windows)]
fn install_user_level(config_path: &Path, dry_run: bool) -> Result<()> {
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

#[cfg(windows)]
fn uninstall_user_level(dry_run: bool) -> Result<()> {
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

#[cfg(windows)]
fn install_windows_service(config_path: &Path, dry_run: bool) -> Result<()> {
    ensure_admin(dry_run)?;
    crate::windows_service::install(config_path, dry_run)
}

#[cfg(windows)]
fn uninstall_windows_service(dry_run: bool) -> Result<()> {
    ensure_admin(dry_run)?;
    crate::windows_service::uninstall(dry_run)
}

#[cfg(not(windows))]
fn install_windows_service(_config_path: &Path, _dry_run: bool) -> Result<()> {
    anyhow::bail!("Windows Service 仅在 Windows 上可用（Linux 请用 --mode task 或 --mode user）")
}

#[cfg(not(windows))]
fn uninstall_windows_service(_dry_run: bool) -> Result<()> {
    anyhow::bail!("Windows Service 仅在 Windows 上可用")
}

// ── Linux / Unix ──────────────────────────────────────────────

#[cfg(unix)]
fn install_system_level(config_path: &Path, dry_run: bool) -> Result<()> {
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

#[cfg(unix)]
fn uninstall_system_level(dry_run: bool) -> Result<()> {
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

#[cfg(unix)]
fn install_user_level(config_path: &Path, dry_run: bool) -> Result<()> {
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

#[cfg(unix)]
fn uninstall_user_level(dry_run: bool) -> Result<()> {
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

#[cfg(unix)]
fn user_unit_path() -> Result<PathBuf> {
    let home = std::env::var("HOME").context("未设置 HOME，无法定位用户级 systemd 目录")?;
    Ok(PathBuf::from(home).join(SYSTEMD_USER_UNIT_REL))
}

#[cfg(unix)]
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
#[cfg(unix)]
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

    #[test]
    fn run_command_quotes_paths_with_spaces() {
        let cmd = run_command(
            Path::new(r"C:\Program Files\gugu guard\gugu.exe"),
            Path::new(r"C:\my cfg\gugu.toml"),
        );
        assert_eq!(
            cmd,
            "\"C:\\Program Files\\gugu guard\\gugu.exe\" run -c \"C:\\my cfg\\gugu.toml\""
        );
        // 两侧都有引号：路径含空格时 schtasks /reg 才不会把参数拆开
        assert_eq!(cmd.matches('"').count(), 4);
    }

    #[test]
    fn detached_command_appends_flag_after_args() {
        let cmd = run_command_detached(Path::new("/opt/gugu"), Path::new("/opt/gugu.toml"));
        assert_eq!(cmd, "\"/opt/gugu\" run -c \"/opt/gugu.toml\" --detach");
    }

    #[test]
    fn systemd_unit_quotes_paths_and_targets_multi_user() {
        let unit = build_systemd_unit(
            Path::new("/opt/my gugu/gugu"),
            Path::new("/opt/my gugu/gugu.toml"),
            Path::new("/opt/my gugu"),
            "GuguGuard Test",
            false,
        );
        assert!(
            unit.contains("ExecStart=\"/opt/my gugu/gugu\" run -c \"/opt/my gugu/gugu.toml\""),
            "含空格路径必须被引号包住: {unit}"
        );
        assert!(unit.contains("WorkingDirectory=\"/opt/my gugu\""), "{unit}");
        assert!(unit.contains("WantedBy=multi-user.target"), "{unit}");
        assert!(unit.contains("After=network.target"), "{unit}");
        assert!(unit.contains("Restart=on-failure"), "{unit}");
    }

    #[test]
    fn systemd_user_unit_targets_default_target() {
        let unit = build_systemd_unit(
            Path::new("/home/u/gugu"),
            Path::new("/home/u/gugu.toml"),
            Path::new("/home/u"),
            "GuguGuard Test",
            true,
        );
        assert!(unit.contains("WantedBy=default.target"), "{unit}");
        assert!(unit.contains("After=default.target"), "{unit}");
        assert!(
            !unit.contains("multi-user.target"),
            "用户级不应挂系统 target: {unit}"
        );
    }

    #[test]
    fn systemd_unit_has_required_sections() {
        let unit = build_systemd_unit(
            Path::new("/x/gugu"),
            Path::new("/x/gugu.toml"),
            Path::new("/x"),
            "GuguGuard Test",
            false,
        );
        for section in ["[Unit]", "[Service]", "[Install]"] {
            assert!(unit.contains(section), "缺少 {section}: {unit}");
        }
        assert!(unit.ends_with('\n'), "单元文件应以换行结尾");
    }
}
