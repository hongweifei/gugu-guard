//! Windows：`GetExtendedTcpTable` 直接带 `dwOwningPid`，归属与计数都是现成的。

use std::ffi::c_void;

use windows_sys::Win32::Foundation::{ERROR_INSUFFICIENT_BUFFER, NO_ERROR};
use windows_sys::Win32::NetworkManagement::IpHelper::{
    GetExtendedTcpTable, MIB_TCP6ROW_OWNER_PID, MIB_TCPROW_OWNER_PID, TCP_TABLE_OWNER_PID_ALL,
};
use windows_sys::Win32::Networking::WinSock::{AF_INET, AF_INET6};

use super::{Listener, Socket, TcpState};

/// `dwLocalPort` / `dwRemotePort` 以网络字节序存在低 16 位里，读出来要还原。
fn port_of(raw: u32) -> u16 {
    let low = (raw & 0xff) as u8;
    let high = ((raw >> 8) & 0xff) as u8;
    u16::from_be_bytes([low, high])
}

fn state_of(raw: u32) -> TcpState {
    match raw {
        2 => TcpState::Listen,
        5 => TcpState::Established,
        _ => TcpState::Other,
    }
}

/// 拉一张 `TCP_TABLE_OWNER_PID_ALL` 表到自管缓冲区。
///
/// 第一次调用只为拿长度（返回 `ERROR_INSUFFICIENT_BUFFER` 是预期结果），
/// 第二次把表直接写进我们的缓冲区——返回的指针不再是系统内存，无需释放。
fn fetch_table(family: u32) -> Option<Vec<u8>> {
    let mut size: u32 = 0;
    let ret = unsafe {
        GetExtendedTcpTable(
            std::ptr::null_mut(),
            &mut size,
            0,
            family,
            TCP_TABLE_OWNER_PID_ALL,
            0,
        )
    };
    if ret != ERROR_INSUFFICIENT_BUFFER || size == 0 {
        return None;
    }
    let mut buffer = vec![0u8; size as usize];
    let ret = unsafe {
        GetExtendedTcpTable(
            buffer.as_mut_ptr().cast::<c_void>(),
            &mut size,
            0,
            family,
            TCP_TABLE_OWNER_PID_ALL,
            0,
        )
    };
    (ret == NO_ERROR).then_some(buffer)
}

/// 表头里的条目数；缓冲区来自 `Vec<u8>`，因此按未对齐读取。
fn entry_count(buffer: &[u8]) -> usize {
    if buffer.len() < std::mem::size_of::<u32>() {
        return 0;
    }
    unsafe { std::ptr::read_unaligned(buffer.as_ptr().cast::<u32>()) as usize }
}

fn collect_ipv4(out: &mut Vec<Socket>) {
    let Some(buffer) = fetch_table(u32::from(AF_INET)) else {
        return;
    };
    let count = entry_count(&buffer);
    let rows = buffer
        .as_ptr()
        .wrapping_add(std::mem::size_of::<u32>())
        .cast::<MIB_TCPROW_OWNER_PID>();
    for i in 0..count {
        let row = unsafe { std::ptr::read_unaligned(rows.add(i)) };
        out.push(Socket {
            local_port: port_of(row.dwLocalPort),
            state: state_of(row.dwState),
            pid: Some(row.dwOwningPid),
        });
    }
}

fn collect_ipv6(out: &mut Vec<Socket>) {
    let Some(buffer) = fetch_table(u32::from(AF_INET6)) else {
        return;
    };
    let count = entry_count(&buffer);
    let rows = buffer
        .as_ptr()
        .wrapping_add(std::mem::size_of::<u32>())
        .cast::<MIB_TCP6ROW_OWNER_PID>();
    for i in 0..count {
        let row = unsafe { std::ptr::read_unaligned(rows.add(i)) };
        out.push(Socket {
            local_port: port_of(row.dwLocalPort),
            state: state_of(row.dwState),
            pid: Some(row.dwOwningPid),
        });
    }
}

pub(super) fn snapshot() -> Vec<Socket> {
    let mut out = Vec::new();
    collect_ipv4(&mut out);
    collect_ipv6(&mut out);
    out
}

pub(super) fn listener(port: u16) -> Option<Listener> {
    let socket = snapshot()
        .into_iter()
        .find(|s| s.state == TcpState::Listen && s.local_port == port)?;
    Some(Listener {
        port,
        pid: socket.pid,
        process: socket.pid.and_then(super::process_name),
    })
}

pub(super) fn is_listening(port: u16) -> bool {
    snapshot()
        .iter()
        .any(|s| s.state == TcpState::Listen && s.local_port == port)
}

pub(super) fn process_name(pid: u32) -> Option<String> {
    use windows_sys::Win32::Foundation::{CloseHandle, INVALID_HANDLE_VALUE};
    use windows_sys::Win32::System::Diagnostics::ToolHelp::{
        CreateToolhelp32Snapshot, Process32FirstW, Process32NextW, PROCESSENTRY32W,
        TH32CS_SNAPPROCESS,
    };

    unsafe {
        let snapshot = CreateToolhelp32Snapshot(TH32CS_SNAPPROCESS, 0);
        if snapshot == INVALID_HANDLE_VALUE {
            return None;
        }
        let mut entry: PROCESSENTRY32W = std::mem::zeroed();
        entry.dwSize = std::mem::size_of::<PROCESSENTRY32W>() as u32;
        let mut name = None;
        if Process32FirstW(snapshot, &mut entry) != 0 {
            loop {
                if entry.th32ProcessID == pid {
                    let wide = &entry.szExeFile;
                    let len = wide.iter().position(|&c| c == 0).unwrap_or(wide.len());
                    name = Some(String::from_utf16_lossy(&wide[..len]));
                    break;
                }
                if Process32NextW(snapshot, &mut entry) == 0 {
                    break;
                }
            }
        }
        CloseHandle(snapshot);
        name
    }
}
