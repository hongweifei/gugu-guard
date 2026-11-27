//! Windows 硬限：Job Object。
//!
//! 三个字段的落点：
//! - `cpu_max` → CPU rate control（`HARD_CAP`），按本机逻辑核数换算成整机占比；
//! - `memory_max` → `JobMemoryLimit`，Job 内全部进程的提交内存合计上限；
//! - `pids_max` → `ActiveProcessLimit`，同时活动的进程数上限。
//!
//! 限值在 `CreateJobObject` 之后、`AssignProcessToJobObject` 之前写入；
//! 主进程进入 Job 后，它派生的子进程自动继承同一组限值。
//!
//! 与 cgroup 的语义差异：
//! - CPU 是节流：超过占比后线程被降速运行，进程不会被杀，计量口径是整机而非单核；
//! - 内存是提交量：越界时新的分配失败（进程通常自行报错或崩溃），不是整组 OOM kill；
//! - 进程数只统计进程，不统计线程，与 `pids.max` 统计任务数略有不同。

use std::num::NonZeroUsize;
use std::path::Path;

use super::limits::{CgroupSpec, CpuLimit};
use crate::process::job::{self, Job, JobLimits};

/// 一个已写入限值的 Job Object。没有配置硬限时同样存在，用来兜住进程树生命周期。
pub(crate) struct Limiter {
    job: Job,
}

impl Limiter {
    /// 创建 Job Object 并写入限值；`spec` 为空时只设 `KILL_ON_JOB_CLOSE`。
    ///
    /// `base` 是 cgroup 的挂载点，Windows 上没有对应概念，保留参数是为了与 Linux 侧签名一致。
    ///
    /// # Errors
    /// Job 创建失败、限值写不进去时返回可展示的中文原因。
    pub fn create(_base: &Path, name: &str, spec: &CgroupSpec) -> Result<Self, String> {
        let limits = translate(spec)?;
        let job = job::create_job(&limits).map_err(|e| format!("创建 Job Object 失败: {e}"))?;
        if !limits.is_empty() {
            let mut notes = vec![spec.describe()];
            if let Some(rate) = limits.cpu_rate {
                notes.push(format!("整机 CPU 占比 {:.1}%", f64::from(rate) / 100.0));
            }
            tracing::info!("[{name}] Job Object 硬限已就绪: {}", notes.join("，"));
        }
        Ok(Self { job })
    }

    /// 把主进程放进 Job。`spec` 为空时失败只影响生命周期保护，由调用方决定是否致命。
    ///
    /// # Errors
    /// 拿不到 PID、`AssignProcessToJobObject` 失败时返回原因。
    pub fn finish_spawn(&mut self, pid: Option<u32>) -> Result<(), String> {
        let Some(pid) = pid else {
            return Err("进程没有 PID，无法加入 Job Object".into());
        };
        self.job
            .assign_process(pid)
            .map_err(|e| format!("无法把进程 {pid} 加入 Job Object: {e}"))
    }

    /// 终止 Job 里的全部进程。停止路径先调用它，拿不到才回落到信号。
    pub fn terminate(&self) -> bool {
        self.job.terminate()
    }

    /// 终止整棵进程树并释放 Job 句柄。
    pub fn kill_and_remove(self) {
        self.job.terminate();
    }
}

/// `CgroupSpec` → `JobLimits`，只做口径换算，不碰系统调用。
fn translate(spec: &CgroupSpec) -> Result<JobLimits, String> {
    Ok(JobLimits {
        cpu_rate: spec.cpu.as_ref().map(cpu_rate),
        job_memory_bytes: spec.memory_bytes,
        processes: spec
            .pids
            .map(|pids| {
                u32::try_from(pids)
                    .map_err(|_| format!("pids_max {pids} 超出 Windows 可表示的范围"))
            })
            .transpose()?,
    })
}

/// 单核配额 → 整机占比（1..=10000，100 = 1%）。
///
/// cgroup 的 `50%` 指半个核，Job Object 的 `CpuRate` 指整机处理能力：
/// 8 核机器上的半个核是整机的 6.25%。
fn cpu_rate(limit: &CpuLimit) -> u32 {
    let cores = limit.quota as f64 / limit.period as f64;
    let total = std::thread::available_parallelism().map_or(1, NonZeroUsize::get) as f64;
    (cores / total * 10_000.0).round().clamp(1.0, 10_000.0) as u32
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cpu_rate_is_machine_share() {
        let total = std::thread::available_parallelism().map_or(1, NonZeroUsize::get) as f64;
        let half_core = CpuLimit {
            quota: 50_000,
            period: 100_000,
        };
        let expected = (0.5 / total * 10_000.0).round().clamp(1.0, 10_000.0) as u32;
        assert_eq!(cpu_rate(&half_core), expected);
    }

    #[test]
    fn translate_keeps_memory_and_rejects_huge_pids() {
        let spec = CgroupSpec {
            cpu: None,
            memory_bytes: Some(512 * 1024 * 1024),
            pids: Some(64),
        };
        let limits = translate(&spec).expect("可换算");
        assert_eq!(limits.job_memory_bytes, Some(512 * 1024 * 1024));
        assert_eq!(limits.processes, Some(64));
        assert!(!limits.is_empty());

        let too_many = CgroupSpec {
            cpu: None,
            memory_bytes: None,
            pids: Some(u64::from(u32::MAX) + 1),
        };
        assert!(translate(&too_many).is_err());
    }

    #[test]
    fn empty_spec_maps_to_empty_limits() {
        assert!(translate(&CgroupSpec::default())
            .expect("空规格可换算")
            .is_empty());
    }
}
