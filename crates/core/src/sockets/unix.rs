//! Unix：只有 socket inode，没有 pid。连接数统计不需要 pid，所以常态零成本；
//! 只有在端口冲突、需要告诉用户「谁占着」时才按 inode 反查 `/proc/<pid>/fd`。

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
    for pid in read_dir_pids() {
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

/// `/proc` 下的数字目录（即所有 pid）；仅反查归属时使用。
fn read_dir_pids() -> Vec<u32> {
    let Ok(dir) = std::fs::read_dir("/proc") else {
        return Vec::new();
    };
    dir.flatten()
        .filter_map(|e| e.file_name().to_str()?.parse::<u32>().ok())
        .collect()
}
