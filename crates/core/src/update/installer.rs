//! 换入新二进制：先校验，再交给替身进程在旧进程退出后完成替换与拉起。
//!
//! 运行中的可执行文件在 Windows 上无法覆盖、在 Unix 上无法写入（ETXTBSY），
//! 所以替换动作不能由守护进程自己做。这里的做法是从暂存目录启动同一份新二进制的
//! 隐藏子命令，由它等待旧进程退出、改名换入、再按配置拉起守护进程。

use std::fs::OpenOptions;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

use super::version::Version;

/// 暂存目录名：藏在可执行文件旁边，每次更新按版本分目录。
pub const STAGING_DIR: &str = ".gugu-update";

#[cfg(windows)]
const STAGED_NAME: &str = "gugu.exe";
#[cfg(unix)]
const STAGED_NAME: &str = "gugu";

/// 一次替换的四个路径。
#[derive(Debug, Clone)]
pub struct InstallPlan {
    /// 正在运行的可执行文件，替换目标。
    pub target: PathBuf,
    /// 已校验的新二进制。
    pub staged: PathBuf,
    /// 旧文件的备份路径（同目录 `gugu.exe.bak`）。
    pub backup: PathBuf,
    /// 是否保留备份。
    pub keep_backup: bool,
}

impl InstallPlan {
    #[must_use]
    pub fn new(exe: &Path, version: &Version, keep_backup: bool) -> Self {
        let target = exe.to_path_buf();
        let staged = staging_dir(exe, version).join(STAGED_NAME);
        let name = exe
            .file_name()
            .map(|name| name.to_string_lossy().to_string())
            .unwrap_or_else(|| STAGED_NAME.to_string());
        let backup = exe.with_file_name(format!("{name}.bak"));
        Self {
            target,
            staged,
            backup,
            keep_backup,
        }
    }

    /// 新二进制的暂存目录。
    #[must_use]
    pub fn staging_dir(&self) -> PathBuf {
        self.staged
            .parent()
            .map(Path::to_path_buf)
            .unwrap_or_else(|| PathBuf::from("."))
    }
}

/// 某个版本的暂存目录：`<exe 所在目录>/.gugu-update/<版本>`。
#[must_use]
pub fn staging_dir(exe: &Path, version: &Version) -> PathBuf {
    exe.parent()
        .unwrap_or_else(|| Path::new("."))
        .join(STAGING_DIR)
        .join(version.to_string())
}

/// 运行 `--version` 核对暂存文件确实是目标版本的 gugu。
///
/// # Errors
/// 文件无法执行、退出码非零或输出里没有目标版本时返回原因。
pub fn verify_binary(bin: &Path, expect: &Version) -> Result<(), String> {
    let output = Command::new(bin)
        .arg("--version")
        .output()
        .map_err(|e| format!("无法运行 {} --version: {e}", bin.display()))?;
    if !output.status.success() {
        return Err(format!(
            "{} --version 退出码为 {:?}",
            bin.display(),
            output.status.code()
        ));
    }
    let mut text = String::from_utf8_lossy(&output.stdout).to_string();
    text.push_str(&String::from_utf8_lossy(&output.stderr));
    let found = text
        .split_whitespace()
        .filter_map(Version::parse)
        .find(|version| version == expect);
    match found {
        Some(_) => Ok(()),
        None => Err(format!(
            "{} 报出的版本与目标 {} 不一致（输出：{}）",
            bin.display(),
            expect,
            text.trim()
        )),
    }
}

/// 换入：旧文件改名成备份，新文件就位。
///
/// 只在旧进程已经退出之后调用；文件被占用时直接报错，不留下半替换状态。
///
/// # Errors
/// 备份改名失败、复制失败或权限设置失败时返回原因。
pub fn install(plan: &InstallPlan) -> Result<(), String> {
    if plan.target.exists() {
        if plan.backup.exists() {
            std::fs::remove_file(&plan.backup)
                .map_err(|e| format!("清理旧备份 {} 失败: {e}", plan.backup.display()))?;
        }
        std::fs::rename(&plan.target, &plan.backup).map_err(|e| {
            format!(
                "备份 {} 失败（文件可能仍被占用）: {e}",
                plan.target.display()
            )
        })?;
    }

    #[cfg(windows)]
    std::fs::copy(&plan.staged, &plan.target)
        .map_err(|e| format!("写入 {} 失败: {e}", plan.target.display()))?;

    #[cfg(unix)]
    {
        std::fs::rename(&plan.staged, &plan.target)
            .map_err(|e| format!("写入 {} 失败: {e}", plan.target.display()))?;
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&plan.target, std::fs::Permissions::from_mode(0o755))
            .map_err(|e| format!("设置 {} 的可执行权限失败: {e}", plan.target.display()))?;
    }

    if !plan.keep_backup {
        let _ = std::fs::remove_file(&plan.backup);
    }
    Ok(())
}

/// 启动替身进程执行替换；返回它的进程号。
///
/// 替身从暂存目录运行，因此替换目标文件不会影响它自己。
///
/// # Errors
/// 暂存文件无法启动或日志文件打不开时返回原因。
pub fn spawn_shim(plan: &InstallPlan, pid: u32, restart: bool, cwd: &Path) -> Result<u32, String> {
    let log_path = plan.staging_dir().join("apply.log");
    let mut log = OpenOptions::new()
        .create(true)
        .append(true)
        .open(&log_path)
        .map_err(|e| format!("打开 {} 失败: {e}", log_path.display()))?;
    let _ = writeln!(log, "\n=== 等待 PID {pid} 退出后替换 ===");

    let mut command = Command::new(&plan.staged);
    command
        .arg("__apply-update")
        .arg("--target")
        .arg(&plan.target)
        .arg("--pid")
        .arg(pid.to_string())
        .arg("--cwd")
        .arg(cwd)
        .stdin(Stdio::null())
        .stdout(Stdio::from(
            log.try_clone()
                .map_err(|e| format!("复制日志句柄失败: {e}"))?,
        ))
        .stderr(Stdio::from(log));
    if !restart {
        command.arg("--no-restart");
    }
    if !plan.keep_backup {
        command.arg("--no-backup");
    }
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        const DETACHED_PROCESS: u32 = 0x0000_0008;
        const CREATE_NEW_PROCESS_GROUP: u32 = 0x0000_0200;
        command.creation_flags(DETACHED_PROCESS | CREATE_NEW_PROCESS_GROUP);
    }

    let child = command
        .spawn()
        .map_err(|e| format!("启动替身进程 {} 失败: {e}", plan.staged.display()))?;
    Ok(child.id())
}

/// 进程是否仍在运行。
#[must_use]
pub fn pid_alive(pid: u32) -> bool {
    if pid == 0 {
        return false;
    }
    #[cfg(windows)]
    {
        use windows_sys::Win32::Foundation::CloseHandle;
        use windows_sys::Win32::System::Threading::{
            GetExitCodeProcess, OpenProcess, PROCESS_QUERY_LIMITED_INFORMATION,
        };
        const STILL_ACTIVE: u32 = 259;
        // SAFETY: 只查询退出码，取得句柄后立即关闭，不触碰目标进程内存。
        unsafe {
            let handle = OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, 0, pid);
            if handle.is_null() {
                return false;
            }
            let mut code: u32 = 0;
            let ok = GetExitCodeProcess(handle, &mut code);
            CloseHandle(handle);
            ok != 0 && code == STILL_ACTIVE
        }
    }
    #[cfg(unix)]
    {
        // SAFETY: 信号 0 只做存在性与权限检查，不投递任何信号。
        unsafe { libc::kill(pid as libc::pid_t, 0) == 0 }
    }
}

/// 等旧进程退出；超时返回 false。
#[must_use]
pub fn wait_for_exit(pid: u32, timeout: Duration) -> bool {
    let deadline = Instant::now() + timeout;
    while Instant::now() < deadline {
        if !pid_alive(pid) {
            return true;
        }
        std::thread::sleep(Duration::from_millis(100));
    }
    !pid_alive(pid)
}

/// 清掉除 `keep` 之外的暂存版本目录，避免每次升级都留一份归档与新二进制。
pub fn cleanup_staging(exe: &Path, keep: Option<&Path>) {
    let root = exe
        .parent()
        .unwrap_or_else(|| Path::new("."))
        .join(STAGING_DIR);
    let Ok(entries) = std::fs::read_dir(&root) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if keep.is_some_and(|keep| path == keep) {
            continue;
        }
        let _ = std::fs::remove_dir_all(&path);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn plan_paths_sit_next_to_the_executable() {
        let exe = Path::new("/opt/gugu/gugu");
        let version = Version::parse("0.2.0").expect("版本");
        let plan = InstallPlan::new(exe, &version, true);
        assert_eq!(plan.target, PathBuf::from("/opt/gugu/gugu"));
        assert_eq!(plan.backup, PathBuf::from("/opt/gugu/gugu.bak"));
        assert!(plan.staged.to_string_lossy().contains(".gugu-update"));
        assert!(
            plan.staged.to_string_lossy().contains("0.2.0"),
            "暂存目录按版本分开: {}",
            plan.staged.display()
        );
    }

    #[test]
    fn swaps_files_and_keeps_a_backup() {
        let dir = std::env::temp_dir().join(format!("gugu-install-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).expect("建目录");
        let target = dir.join("gugu-test");
        std::fs::write(&target, b"old").expect("写旧文件");
        let version = Version::parse("9.9.9").expect("版本");
        let plan = InstallPlan::new(&target, &version, true);
        std::fs::create_dir_all(plan.staging_dir()).expect("建暂存目录");
        std::fs::write(&plan.staged, b"new").expect("写新文件");

        install(&plan).expect("替换成功");
        assert_eq!(std::fs::read(&target).expect("读目标"), b"new");
        assert_eq!(std::fs::read(&plan.backup).expect("读备份"), b"old");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn install_without_backup_removes_the_old_file() {
        let dir = std::env::temp_dir().join(format!("gugu-install-nobak-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).expect("建目录");
        let target = dir.join("gugu-test");
        std::fs::write(&target, b"old").expect("写旧文件");
        let version = Version::parse("9.9.9").expect("版本");
        let plan = InstallPlan::new(&target, &version, false);
        std::fs::create_dir_all(plan.staging_dir()).expect("建暂存目录");
        std::fs::write(&plan.staged, b"new").expect("写新文件");

        install(&plan).expect("替换成功");
        assert_eq!(std::fs::read(&target).expect("读目标"), b"new");
        assert!(!plan.backup.exists(), "不保留备份");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn waits_for_a_finished_process() {
        // 当前进程一定活着，等它退出必然超时；PID 0 视为不存在，立即返回。
        assert!(pid_alive(std::process::id()));
        assert!(
            !wait_for_exit(std::process::id(), Duration::from_millis(150)),
            "自身不会在超时内退出"
        );
        assert!(!pid_alive(0));
        assert!(wait_for_exit(0, Duration::from_millis(20)));
    }

    #[test]
    fn verify_rejects_a_non_gugu_binary() {
        let dir = std::env::temp_dir().join(format!("gugu-verify-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).expect("建目录");
        let path = dir.join("not-gugu");
        std::fs::write(&path, b"not an executable").expect("写文件");
        let err = verify_binary(&path, &Version::parse("0.2.0").expect("版本"))
            .expect_err("非可执行文件应失败");
        assert!(!err.is_empty());
        let _ = std::fs::remove_dir_all(&dir);
    }
}
