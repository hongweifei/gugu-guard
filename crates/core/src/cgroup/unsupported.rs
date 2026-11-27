//! 既不是 Linux 也不是 Windows：没有可用的硬限机制，配置了限值就在启动时明确报错。

use std::path::Path;

use super::CgroupSpec;

/// 占位类型，保持 `ManagedProcess` 在各平台结构一致。
pub struct Limiter;

impl Limiter {
    /// # Errors
    /// 总是返回「仅支持 Linux 与 Windows」的说明。
    pub fn create(_base: &Path, _name: &str, _spec: &CgroupSpec) -> Result<Self, String> {
        Err(format!(
            "资源硬限仅支持 Linux（cgroup v2）与 Windows（Job Object），当前平台是 {}；请去掉 cpu_max / memory_max / pids_max，或改用 max_memory_mb 这类守护进程自带的看护",
            std::env::consts::OS
        ))
    }

    /// 占位：`create` 永远不会成功，因此不会走到这里。
    ///
    /// # Errors
    /// 不会返回错误。
    pub fn finish_spawn(&mut self, _pid: Option<u32>) -> Result<(), String> {
        Ok(())
    }

    /// 占位：`create` 永远不会成功，因此不会走到这里。
    pub fn kill_and_remove(self) {}
}
