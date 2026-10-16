//! Windows Job Object 封装，用于可靠管理进程树生命周期。
//!
//! 核心机制：
//! - `JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE`: 当 Job 句柄关闭（守护进程退出/崩溃）时
//!   自动终止所有关联进程，防止孤儿进程
//! - `TerminateJobObject`: 主动终止整棵进程树，比 `taskkill /T` 更可靠
//!   因为 Job Object 跟踪所有后代进程，包括 cmd /C 产生的脱离进程树的子进程

use std::io;

use windows_sys::Win32::Foundation::CloseHandle;
use windows_sys::Win32::System::JobObjects::{
    AssignProcessToJobObject, JobObjectExtendedLimitInformation, SetInformationJobObject,
    TerminateJobObject, JOBOBJECT_BASIC_LIMIT_INFORMATION, JOBOBJECT_EXTENDED_LIMIT_INFORMATION,
    JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE,
};
use windows_sys::Win32::System::Threading::{OpenProcess, PROCESS_SET_QUOTA, PROCESS_TERMINATE};

/// 以 usize 存储 HANDLE 原始值，使 Job 满足 Send（*mut c_void 不满足 Send）。
/// SAFE: HANDLE 本质上是内核对象的整数标识符，可在线程间安全传递。
pub(super) struct Job {
    handle: usize,
}

// SAFETY: Windows HANDLE 是内核对象标识符，可安全跨线程使用。
unsafe impl Send for Job {}

extern "system" {
    fn CreateJobObjectW(
        lpjobattributes: *const std::ffi::c_void,
        lpname: *const u16,
    ) -> *mut std::ffi::c_void;
}

impl Job {
    /// 将进程分配到此 Job Object。
    pub(super) fn assign_process(&self, pid: u32) -> io::Result<()> {
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
    pub(super) fn terminate(&self) -> bool {
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

/// 创建匿名 Job Object，设置 KILL_ON_JOB_CLOSE 以确保进程树在句柄关闭时被清理。
pub(super) fn create_kill_on_close_job() -> io::Result<Job> {
    let handle = unsafe { CreateJobObjectW(std::ptr::null(), std::ptr::null()) };
    if handle.is_null() {
        return Err(io::Error::last_os_error());
    }

    let info = JOBOBJECT_EXTENDED_LIMIT_INFORMATION {
        BasicLimitInformation: JOBOBJECT_BASIC_LIMIT_INFORMATION {
            LimitFlags: JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE,
            ..unsafe { std::mem::zeroed() }
        },
        ..unsafe { std::mem::zeroed() }
    };

    let h: windows_sys::Win32::Foundation::HANDLE = handle;
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

    Ok(Job {
        handle: handle as usize,
    })
}
