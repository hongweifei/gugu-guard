//! 非 Linux 平台：cgroup v2 不存在，配置了硬限就在启动时明确报错。

use std::path::Path;

use super::CgroupSpec;

/// 占位类型，保持 `ManagedProcess` 在各平台结构一致。
pub struct Cgroup;

impl Cgroup {
    /// # Errors
    /// 总是返回「仅支持 Linux」的说明。
    pub fn create(_base: &Path, _name: &str, _spec: &CgroupSpec) -> Result<Self, String> {
        Err(format!(
            "cgroup v2 硬限仅支持 Linux（当前平台：{}）；请去掉 cpu_max / memory_max / pids_max，或改用 max_memory_mb 这类守护进程自带的看护",
            std::env::consts::OS
        ))
    }

    /// 占位：`create` 永远不会成功，因此不会走到这里。
    pub fn finish_spawn(&mut self) {}

    /// 占位：`create` 永远不会成功，因此不会走到这里。
    pub fn kill_and_remove(self) {}
}
