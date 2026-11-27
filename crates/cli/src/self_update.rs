//! 替身进程：旧守护进程退出后换入新二进制，并按需重新拉起。
//!
//! 更新流程把新二进制从暂存目录启动成这个隐藏子命令，因此这里运行的是**新版本**，
//! 而被替换的目标文件此刻已经不再被旧进程占用。整个过程写进暂存目录的 `apply.log`：
//! 守护进程自身日志在替换后才会由新进程续写，替换期间发生的事只能靠这份日志追。

use std::path::{Path, PathBuf};
#[cfg(unix)]
use std::process::{Command, Stdio};
use std::time::Duration;

#[cfg(unix)]
use anyhow::Context;
use anyhow::{bail, Result};
use gugu_core::update::installer::{self, InstallPlan};
use gugu_core::update::version::Version;

/// 等旧进程自行退出的上限；超过说明优雅停止没有生效，直接放弃替换。
const EXIT_TIMEOUT: Duration = Duration::from_secs(60);

/// 执行一次替换。参数由更新流程在拉起替身时给出。
pub fn run(
    target: &Path,
    pid: u32,
    cwd: Option<&Path>,
    restart: bool,
    keep_backup: bool,
) -> Result<()> {
    let version = Version::current();
    log(&format!(
        "替身启动：目标 {}，旧进程 PID {pid}，版本 {version}",
        target.display()
    ));

    if !installer::wait_for_exit(pid, EXIT_TIMEOUT) {
        bail!("等待旧进程 {pid} 退出超时，未做任何替换");
    }
    log("旧进程已退出，开始替换");

    let plan = InstallPlan::new(target, &version, keep_backup);
    if !plan.staged.exists() {
        bail!("暂存文件 {} 不存在", plan.staged.display());
    }
    if let Err(error) = installer::install(&plan) {
        rollback(&plan);
        bail!("替换失败: {error}");
    }
    if let Err(error) = installer::verify_binary(target, &version) {
        bail!("替换后校验 {} 失败: {error}", target.display());
    }
    log("替换完成，已校验版本");

    let staging = plan.staging_dir();
    installer::cleanup_staging(target, Some(&staging));

    if restart {
        start_daemon(target, cwd)?;
    } else {
        log("按配置不自动重启，请手动启动守护进程");
    }
    Ok(())
}

/// 目标文件已改名成备份、新文件没能就位时，把备份放回去。
fn rollback(plan: &InstallPlan) {
    if plan.target.exists() || !plan.backup.exists() {
        return;
    }
    match std::fs::rename(&plan.backup, &plan.target) {
        Ok(()) => log("已回滚到旧版本"),
        Err(error) => log(&format!(
            "回滚失败（{}）：{error}；旧版本仍在 {}",
            plan.target.display(),
            plan.backup.display()
        )),
    }
}

fn start_daemon(exe: &Path, cwd: Option<&Path>) -> Result<()> {
    let dir = cwd
        .map(Path::to_path_buf)
        .or_else(|| exe.parent().map(Path::to_path_buf))
        .unwrap_or_else(|| PathBuf::from("."));
    let pid = spawn_daemon(exe, &dir)?;
    log(&format!(
        "已请求重新启动（工作目录 {}，PID {pid}）",
        dir.display()
    ));
    Ok(())
}

/// 拉起 `gugu daemon start`。
///
/// 替身是从旧守护进程里启动的，本身带着它继承来的监听套接字；再往下启动时
/// 若继续继承句柄，新守护进程就永远拿不到端口。因此这里显式禁止句柄继承。
#[cfg(windows)]
fn spawn_daemon(exe: &Path, cwd: &Path) -> Result<u32> {
    use std::os::windows::ffi::OsStrExt;
    use std::ptr;

    use windows_sys::Win32::Foundation::{CloseHandle, FALSE};
    use windows_sys::Win32::System::Threading::{
        CreateProcessW, CREATE_NEW_PROCESS_GROUP, CREATE_NO_WINDOW, DETACHED_PROCESS,
        PROCESS_INFORMATION, STARTUPINFOW,
    };

    fn quote(value: &std::ffi::OsStr) -> Vec<u16> {
        let mut wide: Vec<u16> = Vec::new();
        wide.push(u16::from(b'"'));
        for unit in value.encode_wide() {
            if unit == u16::from(b'"') {
                wide.push(u16::from(b'\\'));
            }
            wide.push(unit);
        }
        wide.push(u16::from(b'"'));
        wide
    }

    let mut command_line = quote(exe.as_os_str());
    for arg in ["daemon", "start"] {
        command_line.push(u16::from(b' '));
        command_line.extend(quote(std::ffi::OsStr::new(arg)));
    }
    let mut command_line: Vec<u16> = command_line.into_iter().chain([0]).collect();
    let cwd_wide: Vec<u16> = cwd.as_os_str().encode_wide().chain([0]).collect();

    let mut startup: STARTUPINFOW = unsafe { std::mem::zeroed() };
    startup.cb = std::mem::size_of::<STARTUPINFOW>() as u32;
    let mut process: PROCESS_INFORMATION = unsafe { std::mem::zeroed() };

    let created = unsafe {
        CreateProcessW(
            ptr::null(),
            command_line.as_mut_ptr(),
            ptr::null(),
            ptr::null(),
            FALSE,
            DETACHED_PROCESS | CREATE_NEW_PROCESS_GROUP | CREATE_NO_WINDOW,
            ptr::null(),
            cwd_wide.as_ptr(),
            &startup,
            &mut process,
        )
    };
    if created == 0 {
        let error = std::io::Error::last_os_error();
        bail!("重新启动 {} 失败: {error}", exe.display());
    }
    unsafe {
        CloseHandle(process.hThread);
        CloseHandle(process.hProcess);
    }
    Ok(process.dwProcessId)
}

#[cfg(unix)]
fn spawn_daemon(exe: &Path, cwd: &Path) -> Result<u32> {
    let mut command = Command::new(exe);
    command
        .args(["daemon", "start"])
        .current_dir(cwd)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null());
    // 独立进程组：替身退出不会带走新守护进程
    use std::os::unix::process::CommandExt;
    command.process_group(0);
    let child = command
        .spawn()
        .with_context(|| format!("重新启动 {} 失败", exe.display()))?;
    Ok(child.id())
}

fn log(message: &str) {
    println!("[{}] {message}", chrono::Local::now().format("%H:%M:%S"));
}
