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
//!
//! 子模块分工：[`parse`] 是两平台共用的行解析，[`windows`] 与 [`unix`] 各提供一份平台实现。

use std::collections::HashMap;

mod parse;

#[cfg(windows)]
#[path = "windows.rs"]
mod platform;
#[cfg(unix)]
#[path = "unix.rs"]
mod platform;

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
