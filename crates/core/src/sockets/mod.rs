//! 本机 TCP 套接字表：监听端口归属与连接数。
//!
//! 端口预检（启动前这个端口是不是被别的进程占着）和连接数统计（这个服务挂了几条连接）
//! 读的是同一张表，所以放在一起，只为 `/proc/net/tcp`（Linux）与 `GetExtendedTcpTable`（Windows）
//! 各写一次。
//!
//! 平台差异：
//! - Windows 的 TCP 表直接带 `dwOwningPid`，归属与计数都是现成的；
//! - Linux 只有 socket inode，没有 pid。连接数统计不需要 pid，所以常态零成本；
//!   只有在端口冲突、需要告诉用户"谁占着"时才按 inode 反查 `/proc/<pid>/fd`。

use std::collections::HashMap;

pub use parse::{port_from_line, TcpState};

/// 一条 TCP 套接字的观测结果。
#[derive(Debug, Clone, Copy)]
pub struct Socket {
    pub local_port: u16,
    pub state: TcpState,
    /// 归属进程；Linux 只在反查时才填，Windows 始终可填。
    pub pid: Option<u32>,
}

/// 端口占用者：端口号 + PID + 进程名（拿得到名字时才填）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Listener {
    pub port: u16,
    pub pid: Option<u32>,
    pub process: Option<String>,
}

impl Listener {
    /// 面向用户的一句话描述，例如 `8080 已被 nginx (PID 3312) 占用`。
    #[must_use]
    pub fn describe(&self) -> String {
        match (&self.process, self.pid) {
            (Some(name), Some(pid)) => format!("端口 {} 已被 {name} (PID {pid}) 占用", self.port),
            (None, Some(pid)) => format!("端口 {} 已被 PID {pid} 占用", self.port),
            (Some(name), None) => format!("端口 {} 已被 {name} 占用", self.port),
            (None, None) => format!("端口 {} 已被占用", self.port),
        }
    }
}

/// 当前所有 TCP 套接字（含 LISTEN 与已建立连接）。
#[must_use]
pub fn snapshot() -> Vec<Socket> {
    platform::snapshot()
}

/// 每个本地端口的已建立连接数（状态 ESTABLISHED，含从该端口发起的出站连接）。
#[must_use]
pub fn established_counts() -> HashMap<u16, u32> {
    let mut counts: HashMap<u16, u32> = HashMap::new();
    for socket in platform::snapshot() {
        if socket.state == TcpState::Established {
            *counts.entry(socket.local_port).or_default() += 1;
        }
    }
    counts
}

/// 端口是否已有进程在监听；有则返回占用者（带进程名）。
#[must_use]
pub fn listen_on(port: u16) -> Option<Listener> {
    platform::listener(port)
}

/// 端口是否处于 LISTEN（就绪探测的高频路径，不做归属反查）。
#[must_use]
pub fn is_listening(port: u16) -> bool {
    platform::is_listening(port)
}

/// PID 对应的进程名（拿不到时返回 None）。
#[must_use]
pub fn process_name(pid: u32) -> Option<String> {
    platform::process_name(pid)
}

/// `/proc/net/tcp{,6}` 行解析：纯文本逻辑，与平台无关，因此 Windows 上也能跑单测。
mod parse {
    /// `/proc/net/tcp` 里的连接状态码（字段 `st`）。
    #[derive(Debug, Clone, Copy, PartialEq, Eq)]
    pub enum TcpState {
        Listen,
        Established,
        Other,
    }

    impl TcpState {
        #[must_use]
        pub fn from_hex(value: &str) -> Self {
            match value {
                "0A" | "0a" => Self::Listen,
                "01" => Self::Established,
                _ => Self::Other,
            }
        }
    }

    /// 解析 `/proc/net/tcp{,6}` 的一行，返回 (本地端口, 状态, inode)。
    ///
    /// 行格式：`sl local_address rem_address st tx_queue:rx_queue tr:tm->when retrnsmt uid timeout inode`，
    /// 其中 `local_address` 是 `HEXADDR:HEXPORT`（IPv6 是 32 位十六进制地址）。
    /// 表头行与格式不符的行返回 None。
    #[must_use]
    pub fn port_from_line(line: &str) -> Option<(u16, TcpState, Option<u64>)> {
        let fields: Vec<&str> = line.split_whitespace().collect();
        let local = *fields.get(1)?;
        let state = TcpState::from_hex(fields.get(3)?);
        let (_, port) = local.rsplit_once(':')?;
        let port = u16::from_str_radix(port, 16).ok()?;
        // 第 10 列是 inode：拿不到就退化为 None，供上层跳过归属反查。
        let inode = fields.get(9).and_then(|v| v.parse::<u64>().ok());
        Some((port, state, inode))
    }
}

#[cfg(windows)]
mod platform {
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
}

#[cfg(unix)]
mod platform {
    use super::{Listener, Socket, TcpState};

    fn read_table(path: &str, out: &mut Vec<Socket>) {
        let Ok(text) = std::fs::read_to_string(path) else {
            return;
        };
        for line in text.lines().skip(1) {
            let Some((port, state, _inode)) = super::port_from_line(line) else {
                continue;
            };
            out.push(Socket {
                local_port: port,
                state,
                pid: None,
            });
        }
    }

    /// 读 tcp/tcp6 两张表；inode 需要留着查归属，所以单独再读一遍监听项。
    pub(super) fn snapshot() -> Vec<Socket> {
        let mut out = Vec::new();
        read_table("/proc/net/tcp", &mut out);
        read_table("/proc/net/tcp6", &mut out);
        out
    }

    /// 在 `/proc/<pid>/fd` 里找 `socket:[inode]` 软链，反查监听端口的归属进程。
    fn pid_by_inode(inode: u64) -> Option<u32> {
        let target = format!("socket:[{inode}]");
        for pid in super::read_dir_pids() {
            let fd_dir = format!("/proc/{pid}/fd");
            let Ok(entries) = std::fs::read_dir(&fd_dir) else {
                continue;
            };
            for entry in entries.flatten() {
                let Ok(link) = std::fs::read_link(entry.path()) else {
                    continue;
                };
                if link.to_string_lossy() == target {
                    return Some(pid);
                }
            }
        }
        None
    }

    pub(super) fn listener(port: u16) -> Option<Listener> {
        let mut inode = None;
        for path in ["/proc/net/tcp", "/proc/net/tcp6"] {
            let Ok(text) = std::fs::read_to_string(path) else {
                continue;
            };
            for line in text.lines().skip(1) {
                let Some((line_port, state, line_inode)) = super::port_from_line(line) else {
                    continue;
                };
                if line_port == port && state == TcpState::Listen {
                    inode = line_inode;
                    break;
                }
            }
            if inode.is_some() {
                break;
            }
        }
        let inode = inode?;
        let pid = pid_by_inode(inode);
        Some(Listener {
            port,
            pid,
            process: pid.and_then(super::process_name),
        })
    }

    /// 只判断有没有 LISTEN，不反查归属（就绪探测每 500ms 一次，走这条轻路径）。
    pub(super) fn is_listening(port: u16) -> bool {
        for path in ["/proc/net/tcp", "/proc/net/tcp6"] {
            let Ok(text) = std::fs::read_to_string(path) else {
                continue;
            };
            let found = text.lines().skip(1).any(|line| {
                super::port_from_line(line)
                    .is_some_and(|(p, state, _)| p == port && state == TcpState::Listen)
            });
            if found {
                return true;
            }
        }
        false
    }

    pub(super) fn process_name(pid: u32) -> Option<String> {
        // cmdline 给完整命令行；权限不足时退回 comm（内核截断的可执行名）。
        let from_cmdline = std::fs::read_to_string(format!("/proc/{pid}/cmdline"))
            .ok()
            .map(|c| c.replace('\0', " ").trim().to_string())
            .filter(|c| !c.is_empty());
        if let Some(cmd) = from_cmdline {
            return Some(cmd.chars().take(80).collect());
        }
        let comm = std::fs::read_to_string(format!("/proc/{pid}/comm")).ok()?;
        let name = comm.trim();
        if name.is_empty() {
            None
        } else {
            Some(name.to_string())
        }
    }
}

/// `/proc` 下的数字目录（即所有 pid）；仅 Linux 反查归属时使用。
#[cfg(unix)]
fn read_dir_pids() -> Vec<u32> {
    let Ok(dir) = std::fs::read_dir("/proc") else {
        return Vec::new();
    };
    dir.flatten()
        .filter_map(|e| e.file_name().to_str()?.parse::<u32>().ok())
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_proc_net_tcp_lines() {
        // 表头 / 乱行不应产生端口
        assert!(port_from_line("  sl  local_address rem_address   st").is_none());
        assert!(port_from_line("garbage").is_none());

        // LISTEN 0.0.0.0:8080 (1F90)，inode 12345
        let listen = "   0: 00000000:1F90 00000000:0000 0A 00000000:00000000 \
                      00:00000000 00000000     0        0 12345 1 0000000000000000 100 0 0 10 0";
        let (port, state, inode) = port_from_line(listen).expect("应能解析 LISTEN 行");
        assert_eq!(port, 8080);
        assert_eq!(state, TcpState::Listen);
        assert_eq!(inode, Some(12345));

        // ESTABLISHED 127.0.0.1:9090 (2382) ↔ 127.0.0.1:54321 (D431)
        let established = "   1: 0100007F:2382 0100007F:D431 01 00000000:00000000 \
                            00:00000000 00000000     0        0 99 1 0000000000000000 20 4 30 10 -1";
        let (port, state, _) = port_from_line(established).expect("应能解析 ESTABLISHED 行");
        assert_eq!(port, 9090);
        assert_eq!(state, TcpState::Established);

        // IPv6 地址同样按最后一个冒号切出端口
        let v6 = "   0: 00000000000000000000000000000000:0050 00000000000000000000000000000000:0000 \
                  0A 00000000:00000000 00:00000000 00000000     0        0 777 1 0000000000000000 100 0 0 10 0";
        assert_eq!(port_from_line(v6).expect("IPv6 行").0, 80);
    }

    #[test]
    fn listener_describes_owner_readably() {
        let listener = Listener {
            port: 8080,
            pid: Some(3312),
            process: Some("nginx".into()),
        };
        assert_eq!(listener.describe(), "端口 8080 已被 nginx (PID 3312) 占用");
        let anonymous = Listener {
            port: 9000,
            pid: None,
            process: None,
        };
        assert_eq!(anonymous.describe(), "端口 9000 已被占用");
    }

    #[test]
    fn snapshot_counts_established_by_port() {
        // 只验证真实环境下不会 panic，且计数自洽
        let counts = established_counts();
        for (port, count) in &counts {
            assert!(*port > 0, "端口号应为有效值");
            assert!(*count >= 1);
        }
    }
}
