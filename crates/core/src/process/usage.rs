//! 进程树资源占用采样：CPU 占单核百分比 + 内存（RSS 合计）。
//!
//! 自己实现而不引 `sysinfo`：后者会带回 `windows` crate、`tempfile`（getrandom）、
//! rayon/itertools/ntapi 等一整串依赖，抵消掉依赖去重（提交 3f1f491）的成果。
//! 平台实现：Windows 用 ToolHelp 快照 + `GetProcessTimes`/`GetProcessMemoryInfo`；
//! Unix 读 `/proc/<pid>/stat`。

use std::time::Instant;

/// 一次进程树采样结果。`cpu_percent` 是**相对单核**的百分比（多线程可 >100）。
#[derive(Debug, Clone, Copy, Default, serde::Serialize, serde::Deserialize)]
pub struct UsageSample {
    pub cpu_percent: f32,
    pub memory_bytes: u64,
    pub process_count: usize,
}

/// 采样器：保存上一次的 CPU 时间，才能算出区间占用率。
#[derive(Default)]
pub struct UsageSampler {
    prev: Option<(u64, Instant)>,
}

impl UsageSampler {
    #[must_use]
    pub fn new() -> Self {
        Self { prev: None }
    }

    /// 采样 `pid` 所在进程树；进程不存在或权限不足时返回 None。
    pub fn sample(&mut self, pid: u32) -> Option<UsageSample> {
        let (cpu, memory_bytes, process_count) = platform::sample_tree(pid)?;
        let now = Instant::now();
        let cpu_percent = match self.prev {
            Some((prev_cpu, prev_at)) => percent_from_delta(
                prev_cpu,
                cpu,
                now.duration_since(prev_at).as_secs_f64(),
                platform::cpu_ticks_per_sec(),
            ),
            None => 0.0,
        };
        self.prev = Some((cpu, now));
        Some(UsageSample { cpu_percent, memory_bytes, process_count })
    }
}

/// 由两次采样的 CPU 时间差计算占单核百分比。
fn percent_from_delta(prev_cpu: u64, cpu: u64, elapsed_secs: f64, ticks_per_sec: u64) -> f32 {
    if elapsed_secs <= 0.0 || ticks_per_sec == 0 {
        return 0.0;
    }
    let delta = cpu.saturating_sub(prev_cpu) as f64;
    (delta / ticks_per_sec as f64 / elapsed_secs * 100.0) as f32
}

/// 人类可读的字节数（日志与 UI 提示用）。
#[must_use]
pub fn format_bytes(bytes: u64) -> String {
    const KB: f64 = 1024.0;
    const MB: f64 = KB * 1024.0;
    const GB: f64 = MB * 1024.0;
    let b = bytes as f64;
    if b >= GB {
        format!("{:.2} GB", b / GB)
    } else if b >= MB {
        format!("{:.1} MB", b / MB)
    } else if b >= KB {
        format!("{:.1} KB", b / KB)
    } else {
        format!("{bytes} B")
    }
}

#[cfg(windows)]
mod platform {
    use std::collections::HashMap;

    use windows_sys::Win32::Foundation::{CloseHandle, FILETIME, INVALID_HANDLE_VALUE};
    use windows_sys::Win32::System::Diagnostics::ToolHelp::{
        CreateToolhelp32Snapshot, Process32FirstW, Process32NextW, PROCESSENTRY32W,
        TH32CS_SNAPPROCESS,
    };
    use windows_sys::Win32::System::ProcessStatus::{GetProcessMemoryInfo, PROCESS_MEMORY_COUNTERS};
    use windows_sys::Win32::System::Threading::{
        GetProcessTimes, OpenProcess, PROCESS_QUERY_INFORMATION, PROCESS_QUERY_LIMITED_INFORMATION,
        PROCESS_VM_READ,
    };

    /// Windows 的进程时间单位是 100ns。
    pub(super) fn cpu_ticks_per_sec() -> u64 {
        10_000_000
    }

    /// 全系统 pid → 父 pid 映射。
    fn parent_map() -> Option<HashMap<u32, u32>> {
        unsafe {
            let snapshot = CreateToolhelp32Snapshot(TH32CS_SNAPPROCESS, 0);
            if snapshot == INVALID_HANDLE_VALUE {
                return None;
            }
            let mut entry: PROCESSENTRY32W = std::mem::zeroed();
            entry.dwSize = std::mem::size_of::<PROCESSENTRY32W>() as u32;
            let mut map = HashMap::new();
            if Process32FirstW(snapshot, &mut entry) != 0 {
                loop {
                    map.insert(entry.th32ProcessID, entry.th32ParentProcessID);
                    if Process32NextW(snapshot, &mut entry) == 0 {
                        break;
                    }
                }
            }
            CloseHandle(snapshot);
            Some(map)
        }
    }

    fn tree_pids(root: u32, parents: &HashMap<u32, u32>) -> Vec<u32> {
        let mut tree = vec![root];
        let mut i = 0;
        while i < tree.len() {
            let parent = tree[i];
            for (&pid, &ppid) in parents {
                if ppid == parent && pid != parent && !tree.contains(&pid) {
                    tree.push(pid);
                }
            }
            i += 1;
        }
        tree
    }

    fn filetime_to_u64(ft: FILETIME) -> u64 {
        ((ft.dwHighDateTime as u64) << 32) | ft.dwLowDateTime as u64
    }

    /// 内核 + 用户态 CPU 时间 (100ns)。
    fn process_cpu_time(pid: u32) -> Option<u64> {
        unsafe {
            let handle = OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, 0, pid);
            if handle.is_null() {
                return None;
            }
            let mut creation: FILETIME = std::mem::zeroed();
            let mut exit: FILETIME = std::mem::zeroed();
            let mut kernel: FILETIME = std::mem::zeroed();
            let mut user: FILETIME = std::mem::zeroed();
            let ok = GetProcessTimes(handle, &mut creation, &mut exit, &mut kernel, &mut user);
            CloseHandle(handle);
            if ok == 0 {
                return None;
            }
            Some(filetime_to_u64(kernel) + filetime_to_u64(user))
        }
    }

    /// 工作集 (常驻内存)。
    fn process_memory(pid: u32) -> Option<u64> {
        unsafe {
            let handle = OpenProcess(PROCESS_QUERY_INFORMATION | PROCESS_VM_READ, 0, pid);
            if handle.is_null() {
                return None;
            }
            let mut counters: PROCESS_MEMORY_COUNTERS = std::mem::zeroed();
            let ok = GetProcessMemoryInfo(
                handle,
                &mut counters,
                std::mem::size_of::<PROCESS_MEMORY_COUNTERS>() as u32,
            );
            CloseHandle(handle);
            if ok == 0 {
                return None;
            }
            Some(counters.WorkingSetSize as u64)
        }
    }

    pub(super) fn sample_tree(root: u32) -> Option<(u64, u64, usize)> {
        let parents = parent_map()?;
        if !parents.contains_key(&root) {
            return None;
        }
        let mut cpu = 0u64;
        let mut memory = 0u64;
        let mut count = 0usize;
        for pid in tree_pids(root, &parents) {
            if let Some(c) = process_cpu_time(pid) {
                cpu += c;
                count += 1;
            }
            memory += process_memory(pid).unwrap_or(0);
        }
        Some((cpu, memory, count))
    }
}

#[cfg(any(unix, test))]
mod proc_stat {
    /// `/proc/<pid>/stat` 里我们关心的字段。
    pub(super) struct ParsedStat {
        pub(super) ppid: u32,
        pub(super) cpu_ticks: u64,
        pub(super) rss_pages: u64,
    }

    /// 解析 `/proc/<pid>/stat` 的一行。
    ///
    /// 关键点：`comm` 字段可能含空格与括号（如 `(my prog (v2))`），
    /// 所以必须从**最后一个** `')'` 之后开始按空白切分；
    /// 之后第 i 个 token 对应原字段号 `i + 3`。
    pub(super) fn parse(text: &str) -> Option<ParsedStat> {
        let close = text.rfind(')')?;
        let rest: Vec<&str> = text[close + 1..].split_whitespace().collect();
        let ppid = rest.get(1)?.parse().ok()?;
        let utime: u64 = rest.get(11)?.parse().ok()?;
        let stime: u64 = rest.get(12)?.parse().ok()?;
        let rss_pages: u64 = rest.get(21)?.parse().ok()?;
        Some(ParsedStat {
            ppid,
            cpu_ticks: utime + stime,
            rss_pages,
        })
    }
}

#[cfg(unix)]
mod platform {
    use super::proc_stat;

    fn clock_ticks_per_sec() -> u64 {
        let v = unsafe { libc::sysconf(libc::_SC_CLK_TCK) };
        if v > 0 { v as u64 } else { 100 }
    }

    fn page_size() -> u64 {
        let v = unsafe { libc::sysconf(libc::_SC_PAGESIZE) };
        if v > 0 { v as u64 } else { 4096 }
    }

    pub(super) fn cpu_ticks_per_sec() -> u64 {
        clock_ticks_per_sec()
    }

    fn read_stat(pid: u32) -> Option<(u32, u64, u64)> {
        let text = std::fs::read_to_string(format!("/proc/{pid}/stat")).ok()?;
        let parsed = proc_stat::parse(&text)?;
        Some((
            parsed.ppid,
            parsed.cpu_ticks,
            parsed.rss_pages * page_size(),
        ))
    }

    fn all_stats() -> Vec<(u32, u32, u64, u64)> {
        let mut out = Vec::new();
        let Ok(dir) = std::fs::read_dir("/proc") else {
            return out;
        };
        for entry in dir.flatten() {
            let name = entry.file_name();
            let Some(name) = name.to_str() else { continue };
            if !name.bytes().all(|b| b.is_ascii_digit()) {
                continue;
            }
            let Ok(pid) = name.parse::<u32>() else { continue };
            if let Some((ppid, cpu_ticks, rss_bytes)) = read_stat(pid) {
                out.push((pid, ppid, cpu_ticks, rss_bytes));
            }
        }
        out
    }

    pub(super) fn sample_tree(root: u32) -> Option<(u64, u64, usize)> {
        let stats = all_stats();
        if !stats.iter().any(|(pid, ..)| *pid == root) {
            return None;
        }
        let mut tree = vec![root];
        let mut i = 0;
        while i < tree.len() {
            let parent = tree[i];
            for (pid, ppid, ..) in &stats {
                if *ppid == parent && *pid != parent && !tree.contains(pid) {
                    tree.push(*pid);
                }
            }
            i += 1;
        }
        let mut cpu = 0u64;
        let mut memory = 0u64;
        let mut count = 0usize;
        for pid in &tree {
            if let Some((_, _, cpu_ticks, rss_bytes)) = stats.iter().find(|(p, ..)| p == pid) {
                cpu += cpu_ticks;
                memory += rss_bytes;
                count += 1;
            }
        }
        Some((cpu, memory, count))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn percent_from_delta_scales_with_elapsed_time() {
        // 1 秒内用掉 0.5 秒 CPU ⇒ 50%
        assert!((percent_from_delta(0, 5_000_000, 1.0, 10_000_000) - 50.0).abs() < 0.01);
        // 2 秒内用掉 1 秒 ⇒ 50%
        assert!((percent_from_delta(0, 10_000_000, 2.0, 10_000_000) - 50.0).abs() < 0.01);
        // 时间倒流或零间隔不 panic、不产生 NaN
        assert_eq!(percent_from_delta(10, 20, 0.0, 10_000_000), 0.0);
        // 计数回绕（进程重启）按 0 处理
        assert_eq!(percent_from_delta(100, 10, 1.0, 10_000_000), 0.0);
    }

    #[test]
    fn sampler_returns_none_for_missing_process() {
        let mut sampler = UsageSampler::new();
        // 取一个几乎不可能存在的 pid
        assert!(sampler.sample(u32::MAX - 1).is_none());
    }

    /// `/proc` 解析逻辑与平台解耦，因此本机（Windows）也能真跑它的单测。
    #[test]
    fn parses_proc_stat_with_tricky_comm() {
        // comm 里故意放空格与括号：必须从最后一个 ')' 之后切分
        let line = "1234 (my prog (v2)) S 42 1234 1234 0 -1 4194560 100 0 0 0 11 22 0 0 20 0 3 0 987654 4096 512 0 0 0 0 0 0";
        let parsed = proc_stat::parse(line).expect("应能解析合法行");
        assert_eq!(parsed.ppid, 42, "ppid 取字段 4");
        assert_eq!(parsed.cpu_ticks, 33, "utime(11) + stime(22)");
        assert_eq!(parsed.rss_pages, 512, "rss 取字段 24");

        assert!(proc_stat::parse("garbage").is_none(), "无右括号应返回 None");
        assert!(proc_stat::parse("1 (x) S").is_none(), "字段不足应返回 None");
        assert!(proc_stat::parse("1 (x) S abc 1").is_none(), "字段非数字应返回 None");
    }

    #[test]
    fn format_bytes_uses_human_units() {
        assert_eq!(format_bytes(512), "512 B");
        assert_eq!(format_bytes(2048), "2.0 KB");
        assert_eq!(format_bytes(5 * 1024 * 1024), "5.0 MB");
        assert_eq!(format_bytes(3 * 1024 * 1024 * 1024), "3.00 GB");
    }

    #[cfg(windows)]
    #[test]
    fn samples_own_tree() {
        let mut sampler = UsageSampler::new();
        let sample = sampler.sample(std::process::id()).expect("应能采样自身进程");
        assert!(sample.process_count >= 1, "至少包含自身: {sample:?}");
        assert!(sample.memory_bytes > 0, "自身内存应大于 0: {sample:?}");
        // 第二次采样才有 CPU 百分比（首次无基准）
        let second = sampler.sample(std::process::id()).expect("第二次采样");
        assert!(second.cpu_percent >= 0.0);
    }
}
