//! cgroup v2 硬限的 Linux 实现。
//!
//! 目录布局：`<base>/gugu/<slug>-<hash>/`。`gugu` 这一层只用来集中管理，
//! 从不往里放进程（cgroup v2 不允许「内部有进程的 cgroup 再启用控制器」）。

use std::fs;
use std::io;
use std::os::fd::{AsRawFd, RawFd};
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use super::limits::slug;
use super::CgroupSpec;

/// 清理残留任务时等待退出的上限。
const KILL_WAIT: Duration = Duration::from_secs(1);
/// 轮询 `cgroup.procs` 的间隔。
const POLL_INTERVAL: Duration = Duration::from_millis(20);

/// 一个已准备好的进程 cgroup。创建时就把限值写好，attach 后子进程才落进去。
pub struct Cgroup {
    path: PathBuf,
    /// `cgroup.procs` 的父进程侧句柄；子进程在 pre_exec 里用这个 fd 写入自己的 pid。
    procs: Option<fs::File>,
}

impl Cgroup {
    /// 创建目录、启用控制器、写入限值；任何一步失败都返回可展示的中文原因。
    ///
    /// # Errors
    /// 不是 cgroup v2 挂载点、控制器缺失、没有写权限、残留任务清不掉时返回错误。
    pub fn create(base: &Path, name: &str, spec: &CgroupSpec) -> Result<Self, String> {
        if !base.join("cgroup.controllers").is_file() {
            return Err(format!(
                "{} 不是 cgroup v2 统一层次挂载点（缺少 cgroup.controllers）；cgroup v1 或未启用 cgroup 的系统无法施加硬限",
                base.display()
            ));
        }
        let group_root = base.join("gugu");
        fs::create_dir_all(&group_root).map_err(|e| {
            format!(
                "创建 {} 失败: {e}（硬限需要 root 权限，或一个已委派写权限的 cgroup 目录作为 daemon.cgroup_root）",
                group_root.display()
            )
        })?;
        enable_controllers(base, &group_root, spec)?;

        let path = group_root.join(slug(name));
        clear_stale(&path)?;
        fs::create_dir(&path).map_err(|e| format!("创建 cgroup {} 失败: {e}", path.display()))?;

        let prepared = write_limits(&path, spec).and_then(|()| open_procs(&path));
        match prepared {
            Ok(procs) => Ok(Self {
                path,
                procs: Some(procs),
            }),
            Err(reason) => {
                let _ = fs::write(path.join("cgroup.kill"), "1");
                let _ = fs::remove_dir(&path);
                Err(reason)
            }
        }
    }

    /// 在 `fork` 之后、`exec` 之前把子进程自己写进 `cgroup.procs`。
    ///
    /// 用预先打开的 fd 而不是在 hook 里拼路径：`fork` 后的子进程只调 `getpid` 与 `write`，
    /// 两者都是 async-signal-safe，不会在多线程运行时里死锁。
    ///
    /// # Errors
    /// 已经 attach 过、或 `cgroup.procs` 句柄不可用时返回错误。
    pub fn attach(&self, cmd: &mut tokio::process::Command) -> Result<(), String> {
        let fd = self
            .procs
            .as_ref()
            .map(AsRawFd::as_raw_fd)
            .ok_or_else(|| "cgroup.procs 句柄已关闭，无法把进程放进 cgroup".to_string())?;
        // SAFETY: pre_exec 在 fork 后、exec 前的子进程里运行。
        // 闭包只捕获 RawFd 并调用 libc::getpid / libc::write，不分配内存、不取锁。
        unsafe {
            use std::os::unix::process::CommandExt;
            cmd.as_std_mut().pre_exec(move || write_own_pid(fd));
        }
        Ok(())
    }

    /// spawn 结束后关掉父进程这一侧的 `cgroup.procs` 句柄（子进程的 fd 随 exec 关闭）。
    /// 子进程已经在 `exec` 之前写进 cgroup，这里没有额外操作，也没有失败路径。
    ///
    /// # Errors
    /// 不会返回错误。
    pub fn finish_spawn(&mut self, _pid: Option<u32>) -> Result<(), String> {
        self.procs = None;
        Ok(())
    }

    /// 杀掉 cgroup 里所有进程并删除目录。用于进程退出、停止与启动失败。
    pub fn kill_and_remove(mut self) {
        self.procs = None;
        kill_tasks(&self.path);
        if !wait_empty(&self.path, KILL_WAIT) {
            tracing::warn!(
                "cgroup {} 里的进程在 {:?} 内没有全部退出，目录保留给下次启动清理",
                self.path.display(),
                KILL_WAIT
            );
            return;
        }
        if let Err(e) = fs::remove_dir(&self.path) {
            if e.kind() != io::ErrorKind::NotFound {
                tracing::debug!("删除 cgroup {} 失败: {}", self.path.display(), e);
            }
        }
    }
}

impl Drop for Cgroup {
    fn drop(&mut self) {
        // 只删空目录：还有进程在里面时留给下次启动的 clear_stale 处理，
        // 不在这里悄悄杀别人的进程。
        let _ = fs::remove_dir(&self.path);
    }
}

pub(crate) fn enable_controllers(
    base: &Path,
    group_root: &Path,
    spec: &CgroupSpec,
) -> Result<(), String> {
    let needed = spec.controllers();
    if needed.is_empty() {
        return Ok(());
    }
    let available = read_words(&base.join("cgroup.controllers"));
    for name in needed.iter().copied() {
        if !available.iter().any(|c| c.as_str() == name) {
            return Err(format!(
                "cgroup v2 没有可用的 {name} 控制器（{} 的 cgroup.controllers 里没有），无法施加该硬限",
                base.display()
            ));
        }
    }
    // 控制器要逐层下放：先在挂载点启用，`gugu` 才有这些控制器；`gugu` 自己没有进程，
    // 再在它上面启用一次，每个进程目录才会出现 `cpu.max` / `memory.max` / `pids.max`。
    enable_subtree(base, &needed)?;
    enable_subtree(group_root, &needed)?;
    // 父层启用后，子目录的 cgroup.controllers 才会出现这些控制器；
    // 真实内核一定会提供这个文件，读不到时（测试用的假目录）就跳过这步核对。
    if let Ok(child) = fs::read_to_string(group_root.join("cgroup.controllers")) {
        for name in needed.iter().copied() {
            if !child.split_whitespace().any(|c| c == name) {
                return Err(format!(
                    "控制器 {name} 没有出现在 {} 的子树里，硬限未生效",
                    group_root.display()
                ));
            }
        }
    }
    Ok(())
}

/// 在 `dir/cgroup.subtree_control` 里补上缺失的控制器。
///
/// 内核只接受带符号的增量写法（`+cpu +memory`），且下发空指令会被拒绝，
/// 所以只写缺项、一个不缺时什么都不写。
fn enable_subtree(dir: &Path, needed: &[&str]) -> Result<(), String> {
    let enabled: Vec<String> = read_words(&dir.join("cgroup.subtree_control"))
        .into_iter()
        .filter_map(|word| word.strip_prefix('+').map(str::to_string))
        .collect();
    let missing: Vec<&str> = needed
        .iter()
        .copied()
        .filter(|name| !enabled.iter().any(|c| c == name))
        .collect();
    if missing.is_empty() {
        return Ok(());
    }
    let request: String = missing
        .iter()
        .map(|n| format!("+{n}"))
        .collect::<Vec<_>>()
        .join(" ");
    fs::write(dir.join("cgroup.subtree_control"), &request).map_err(|e| {
        format!(
            "在 {} 上启用控制器失败: {e}；该目录里已有进程时无法启用控制器，请换一个空的 cgroup 子树作为 daemon.cgroup_root",
            dir.display()
        )
    })
}

pub(crate) fn write_limits(path: &Path, spec: &CgroupSpec) -> Result<(), String> {
    if let Some(cpu) = &spec.cpu {
        write_value(&path.join("cpu.max"), &cpu.to_string())?;
    }
    if let Some(bytes) = spec.memory_bytes {
        // 先关 swap：允许换页就等于允许实际占用超过上限，与「硬限」的直觉相反。
        // 没有 swap 控制器的环境（如部分容器）跳过这一项。
        let swap = path.join("memory.swap.max");
        if swap.exists() {
            write_value(&swap, "0")?;
        }
        write_value(&path.join("memory.max"), &bytes.to_string())?;
        // 整组一起 OOM：不留下一批半死不活的子进程。
        let oom_group = path.join("memory.oom.group");
        if oom_group.exists() {
            write_value(&oom_group, "1")?;
        }
    }
    if let Some(pids) = spec.pids {
        write_value(&path.join("pids.max"), &pids.to_string())?;
    }
    Ok(())
}

fn write_value(path: &Path, value: &str) -> Result<(), String> {
    fs::write(path, value).map_err(|e| format!("写入 {} = {value} 失败: {e}", path.display()))
}

fn open_procs(path: &Path) -> Result<fs::File, String> {
    fs::OpenOptions::new()
        .write(true)
        .open(path.join("cgroup.procs"))
        .map_err(|e| format!("打开 {} 失败: {e}", path.join("cgroup.procs").display()))
}

fn read_words(path: &Path) -> Vec<String> {
    fs::read_to_string(path)
        .map(|content| {
            content
                .split_whitespace()
                .map(str::to_string)
                .collect::<Vec<_>>()
        })
        .unwrap_or_default()
}

/// 上次运行留下的同名目录：先杀干净再删，删不掉就报错（启动一个受限进程比留下不受控的旧进程好）。
fn clear_stale(path: &Path) -> Result<(), String> {
    if !path.exists() {
        return Ok(());
    }
    kill_tasks(path);
    if wait_empty(path, KILL_WAIT) && fs::remove_dir(path).is_ok() {
        return Ok(());
    }
    Err(format!(
        "cgroup 目录 {} 已存在且清不掉（里面的进程可能卡在 D 状态），请手工处理后重试",
        path.display()
    ))
}

fn kill_tasks(path: &Path) {
    // cgroup.kill 是内核 5.14+ 的整组原子杀；老内核退回逐个 SIGKILL。
    if fs::write(path.join("cgroup.kill"), "1").is_ok() {
        return;
    }
    if let Ok(content) = fs::read_to_string(path.join("cgroup.procs")) {
        for line in content.lines() {
            if let Ok(pid) = line.trim().parse::<i32>() {
                // SAFETY: kill 只是系统调用，pid 来自内核返回的 cgroup.procs。
                unsafe {
                    libc::kill(pid, libc::SIGKILL);
                }
            }
        }
    }
}

fn wait_empty(path: &Path, timeout: Duration) -> bool {
    let deadline = Instant::now() + timeout;
    loop {
        let empty = fs::read_to_string(path.join("cgroup.procs"))
            .map(|content| content.trim().is_empty())
            .unwrap_or(true);
        if empty {
            return true;
        }
        if Instant::now() >= deadline {
            return false;
        }
        std::thread::sleep(POLL_INTERVAL);
    }
}

/// 只能调用 async-signal-safe 的函数：fork 之后的多线程进程里不能分配内存或取锁。
fn write_own_pid(fd: RawFd) -> io::Result<()> {
    // SAFETY: getpid 无参数、无副作用。
    let pid = unsafe { libc::getpid() };
    let mut buffer = [0u8; 16];
    let len = encode_pid(pid, &mut buffer);
    // SAFETY: fd 是父进程预先打开的 cgroup.procs，buffer 在栈上且长度合法。
    let written = unsafe { libc::write(fd, buffer.as_ptr().cast(), len) };
    if written < 0 {
        return Err(io::Error::last_os_error());
    }
    Ok(())
}

fn encode_pid(pid: i32, buffer: &mut [u8; 16]) -> usize {
    let mut digits = [0u8; 10];
    let mut value = pid as u32;
    let mut count = 0;
    loop {
        digits[count] = b'0' + (value % 10) as u8;
        count += 1;
        value /= 10;
        if value == 0 {
            break;
        }
    }
    let mut len = 0;
    while count > 0 {
        count -= 1;
        buffer[len] = digits[count];
        len += 1;
    }
    buffer[len] = b'\n';
    len + 1
}
