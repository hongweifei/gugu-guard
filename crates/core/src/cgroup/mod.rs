//! cgroup v2 硬限：把被守护进程放进独立 cgroup，由内核保证 CPU、内存、任务数上限。
//!
//! 与 `max_memory_mb` 的区别很重要：
//! - `max_memory_mb` 是守护进程自己的看护逻辑：超限后按自动重启路径重启，进程在被重启前可以继续吃内存；
//! - 这里的 `memory_max` 是内核硬限：越过即触发该 cgroup 的内存 OOM，整组进程一起被杀，守护进程只能观测到退出。
//!
//! 只有 Linux 有 cgroup v2。其他平台配置了限值会在启动时明确失败，不会静默忽略。
//!
//! Linux 下每个进程拿到 `<cgroup_root>/gugu/<可读名>-<哈希>` 的目录，
//! 限值在 `exec` 之前写入，子进程在 `fork` 之后、`exec` 之前把自己写入 `cgroup.procs`：
//! 这样进程树从第一条指令起就在限值内，不存在「先跑起来再搬进去」的空窗。
//!
//! 子模块分工：
//! - [`limits`]：字段解析、格式化、目录命名（与平台无关，可单测）
//! - [`linux`]：cgroup 文件系统的实际操作
//! - `unsupported`：非 Linux 的明确拒绝

mod limits;

#[cfg(target_os = "linux")]
pub(crate) mod linux;

#[cfg(not(target_os = "linux"))]
mod unsupported;

pub use limits::{format_bytes, parse_memory, parse_pids, slug, CgroupSpec, CpuLimit};

/// 默认 cgroup 2 统一层次挂载点；可用 `daemon.cgroup_root` 覆盖。
pub const DEFAULT_BASE: &str = "/sys/fs/cgroup";

#[cfg(target_os = "linux")]
pub use linux::Cgroup;

#[cfg(not(target_os = "linux"))]
pub use unsupported::Cgroup;
