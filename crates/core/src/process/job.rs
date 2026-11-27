//! Windows Job Object 封装，用于可靠管理进程树生命周期，并承担 CPU、内存、进程数硬限。
//!
//! 核心机制：
//! - `JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE`: 当 Job 句柄关闭（守护进程退出/崩溃）时
//!   自动终止所有关联进程，防止孤儿进程
//! - `TerminateJobObject`: 主动终止整棵进程树，比 `taskkill /T` 更可靠
//!   因为 Job Object 跟踪所有后代进程，包括 cmd /C 产生的脱离进程树的子进程
//! - 硬限：`JobObjectCpuRateControlInformation` 限制 CPU 占比，
//!   `JobMemoryLimit` 限制整组提交内存，`ActiveProcessLimit` 限制活动进程数

use std::io;

use windows_sys::Win32::Foundation::CloseHandle;
use windows_sys::Win32::System::JobObjects::{
    AssignProcessToJobObject, JobObjectCpuRateControlInformation,
    JobObjectExtendedLimitInformation, SetInformationJobObject, TerminateJobObject,
    JOBOBJECT_BASIC_LIMIT_INFORMATION, JOBOBJECT_CPU_RATE_CONTROL_INFORMATION,
    JOBOBJECT_CPU_RATE_CONTROL_INFORMATION_0, JOBOBJECT_EXTENDED_LIMIT_INFORMATION,
    JOB_OBJECT_CPU_RATE_CONTROL_ENABLE, JOB_OBJECT_CPU_RATE_CONTROL_HARD_CAP,
    JOB_OBJECT_LIMIT_ACTIVE_PROCESS, JOB_OBJECT_LIMIT_JOB_MEMORY,
    JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE,
};
use windows_sys::Win32::System::Threading::{OpenProcess, PROCESS_SET_QUOTA, PROCESS_TERMINATE};

/// 以 usize 存储 HANDLE 原始值，使 Job 满足 Send（*mut c_void 不满足 Send）。
/// SAFE: HANDLE 本质上是内核对象的整数标识符，可在线程间安全传递。
pub(crate) struct Job {
    handle: usize,
}

// SAFETY: Windows HANDLE 是内核对象标识符，可安全跨线程使用。
unsafe impl Send for Job {}

/// Job Object 的硬限；`None` 表示该项不设上限。
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub(crate) struct JobLimits {
    /// 整机 CPU 占比，取值 1..=10000（100 表示 1%），超过即被节流。
    pub cpu_rate: Option<u32>,
    /// Job 内全部进程的提交内存合计上限（字节）。
    pub job_memory_bytes: Option<u64>,
    /// 同时活动的进程数上限。
    pub processes: Option<u32>,
}

impl JobLimits {
    #[must_use]
    pub(crate) fn is_empty(&self) -> bool {
        self.cpu_rate.is_none() && self.job_memory_bytes.is_none() && self.processes.is_none()
    }
}

extern "system" {
    fn CreateJobObjectW(
        lpjobattributes: *const std::ffi::c_void,
        lpname: *const u16,
    ) -> *mut std::ffi::c_void;
}

impl Job {
    /// 将进程分配到此 Job Object。分配之后派生的子进程自动落在同一个 Job 里。
    pub(crate) fn assign_process(&self, pid: u32) -> io::Result<()> {
        let proc_handle = unsafe { OpenProcess(PROCESS_SET_QUOTA | PROCESS_TERMINATE, 0, pid) };
        if proc_handle.is_null() {
            return Err(io::Error::last_os_error());
        }
        let result = unsafe { AssignProcessToJobObject(self.as_handle(), proc_handle) };
        unsafe { CloseHandle(proc_handle) };
        if result == 0 {
            Err(io::Error::last_os_error())
        } else {
            Ok(())
        }
    }

    /// 终止 Job 中所有进程。成功返回 true。
    pub(crate) fn terminate(&self) -> bool {
        unsafe { TerminateJobObject(self.as_handle(), 1) != 0 }
    }

    fn as_handle(&self) -> windows_sys::Win32::Foundation::HANDLE {
        self.handle as *mut _
    }
}

impl Drop for Job {
    fn drop(&mut self) {
        if self.handle != 0 {
            unsafe { CloseHandle(self.as_handle()) };
        }
    }
}

/// 创建匿名 Job Object：始终设置 KILL_ON_JOB_CLOSE，再按需写入硬限。
pub(crate) fn create_job(limits: &JobLimits) -> io::Result<Job> {
    let handle = unsafe { CreateJobObjectW(std::ptr::null(), std::ptr::null()) };
    if handle.is_null() {
        return Err(io::Error::last_os_error());
    }
    let h: windows_sys::Win32::Foundation::HANDLE = handle;

    let mut info = JOBOBJECT_EXTENDED_LIMIT_INFORMATION {
        BasicLimitInformation: JOBOBJECT_BASIC_LIMIT_INFORMATION {
            LimitFlags: JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE,
            ..unsafe { std::mem::zeroed() }
        },
        ..unsafe { std::mem::zeroed() }
    };
    if let Some(bytes) = limits.job_memory_bytes {
        info.JobMemoryLimit = usize::try_from(bytes).unwrap_or(usize::MAX);
        info.BasicLimitInformation.LimitFlags |= JOB_OBJECT_LIMIT_JOB_MEMORY;
    }
    if let Some(processes) = limits.processes {
        info.BasicLimitInformation.ActiveProcessLimit = processes;
        info.BasicLimitInformation.LimitFlags |= JOB_OBJECT_LIMIT_ACTIVE_PROCESS;
    }

    let result = unsafe {
        SetInformationJobObject(
            h,
            JobObjectExtendedLimitInformation,
            &info as *const _ as *const _,
            std::mem::size_of::<JOBOBJECT_EXTENDED_LIMIT_INFORMATION>() as u32,
        )
    };
    if result == 0 {
        unsafe { CloseHandle(h) };
        return Err(io::Error::last_os_error());
    }

    if let Some(rate) = limits.cpu_rate {
        let cpu = JOBOBJECT_CPU_RATE_CONTROL_INFORMATION {
            ControlFlags: JOB_OBJECT_CPU_RATE_CONTROL_ENABLE | JOB_OBJECT_CPU_RATE_CONTROL_HARD_CAP,
            Anonymous: JOBOBJECT_CPU_RATE_CONTROL_INFORMATION_0 { CpuRate: rate },
        };
        let result = unsafe {
            SetInformationJobObject(
                h,
                JobObjectCpuRateControlInformation,
                &cpu as *const _ as *const _,
                std::mem::size_of::<JOBOBJECT_CPU_RATE_CONTROL_INFORMATION>() as u32,
            )
        };
        if result == 0 {
            unsafe { CloseHandle(h) };
            return Err(io::Error::last_os_error());
        }
    }

    Ok(Job {
        handle: handle as usize,
    })
}
