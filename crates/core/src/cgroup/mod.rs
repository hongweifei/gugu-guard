//! 资源硬限：Linux 用 cgroup v2，Windows 用 Job Object，其余平台配置即失败。
//!
//! 与 `max_memory_mb` 的区别很重要：
//! - `max_memory_mb` 是守护进程自己的看护逻辑：超限后按自动重启路径重启，进程在被重启前可以继续吃内存；
//! - 这里的 `memory_max` 是操作系统硬限：Linux 上越过即触发该 cgroup 的内存 OOM，
//!   整组进程一起被杀；Windows 上是 Job 的提交内存上限，越界时分配失败、进程通常自行退出。
//!
//! 两种落地方式的执行时机不同：
//! - Linux：目录与限值先建好，子进程在 `fork` 之后、`exec` 之前把自己写入 `cgroup.procs`，
//!   进程树从第一条指令起就在限值内；
//! - Windows：Job 与限值先建好，`spawn` 之后把主进程分配给 Job，
//!   此后派生的子进程自动继承限值。
//!
//! 子模块分工：
//! - [`limits`]：字段解析、格式化、目录命名（与平台无关，可单测）
//! - [`linux`]：cgroup 文件系统的实际操作
//! - [`windows`]：Job Object 限值与进程树归属
//! - `unsupported`：既不是 Linux 也不是 Windows 的明确拒绝

mod limits;

#[cfg(target_os = "linux")]
pub(crate) mod linux;

#[cfg(windows)]
pub(crate) mod windows;

#[cfg(not(any(target_os = "linux", windows)))]
mod unsupported;

use std::path::Path;

pub use limits::{format_bytes, parse_memory, parse_pids, slug, CgroupSpec, CpuLimit};

/// 默认 cgroup 2 统一层次挂载点；可用 `daemon.cgroup_root` 覆盖。仅 Linux 使用。
pub const DEFAULT_BASE: &str = "/sys/fs/cgroup";

#[cfg(target_os = "linux")]
pub(crate) use linux::Cgroup as Limiter;

#[cfg(windows)]
pub(crate) use windows::Limiter;

#[cfg(not(any(target_os = "linux", windows)))]
pub(crate) use unsupported::Limiter;

/// 按平台准备硬限。
///
/// Linux 只在配置了限值时创建 cgroup；Windows 无论是否配置都创建 Job，
/// 未配置时它承担进程树生命周期保护（`KILL_ON_JOB_CLOSE`）。
///
/// # Errors
/// 限值在目标平台上无法落地时返回可展示的原因：Linux 是 cgroup 创建失败，
/// Windows 是 Job 限值写入失败，其余平台是「配置了硬限但不支持」。
#[cfg(target_os = "linux")]
pub(crate) fn prepare(
    base: &Path,
    name: &str,
    spec: &CgroupSpec,
) -> Result<Option<Limiter>, String> {
    if spec.is_active() {
        Limiter::create(base, name, spec).map(Some)
    } else {
        Ok(None)
    }
}

#[cfg(windows)]
pub(crate) fn prepare(
    base: &Path,
    name: &str,
    spec: &CgroupSpec,
) -> Result<Option<Limiter>, String> {
    match Limiter::create(base, name, spec) {
        Ok(limiter) => Ok(Some(limiter)),
        // 没配硬限时 Job 只负责生命周期，建不出来不阻止进程运行，与旧行为一致。
        Err(reason) if !spec.is_active() => {
            tracing::warn!("[{name}] 创建 Job Object 失败，进程树生命周期不受 Job 保护: {reason}");
            Ok(None)
        }
        Err(reason) => Err(reason),
    }
}

#[cfg(not(any(target_os = "linux", windows)))]
pub(crate) fn prepare(
    base: &Path,
    name: &str,
    spec: &CgroupSpec,
) -> Result<Option<Limiter>, String> {
    if spec.is_active() {
        Limiter::create(base, name, spec).map(Some)
    } else {
        Ok(None)
    }
}
