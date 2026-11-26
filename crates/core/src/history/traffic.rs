//! 机器级带宽采样：整机网卡的收发速率。
//!
//! 这是**整机**口径，不是单进程口径——内核不按进程记网卡字节，按进程的精确流量需要
//! nftables cgroup 计数或 eBPF（都要 root）。整机口径无需任何特权，能回答
//! 「这台机器带宽打满了没有」。

use std::time::Instant;

/// 一段采样区间内的平均速率。
#[derive(Debug, Clone, Copy, Default, serde::Serialize, serde::Deserialize)]
pub struct NetRate {
    pub rx_bytes_per_sec: f64,
    pub tx_bytes_per_sec: f64,
}

/// 用前一次计数器求速率，因此首次调用只建基准、返回 None。
#[derive(Debug, Default)]
pub struct TrafficSampler {
    prev: Option<(u64, u64, Instant)>,
}

impl TrafficSampler {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn sample(&mut self) -> Option<NetRate> {
        let (rx, tx) = platform::counters()?;
        let now = Instant::now();
        let rate = self.prev.and_then(|(prev_rx, prev_tx, prev_at)| {
            let secs = now.duration_since(prev_at).as_secs_f64();
            if secs <= 0.0 {
                return None;
            }
            Some(NetRate {
                rx_bytes_per_sec: rx.saturating_sub(prev_rx) as f64 / secs,
                tx_bytes_per_sec: tx.saturating_sub(prev_tx) as f64 / secs,
            })
        });
        self.prev = Some((rx, tx, now));
        rate
    }
}

/// `/proc/net/dev` 解析：跳过回环，累加所有网卡的收发字节。
///
/// 行格式：`iface: rx_bytes rx_packets ... tx_bytes ...`（冒号后 16 个字段）。
#[must_use]
pub fn parse_net_dev(text: &str) -> (u64, u64) {
    let mut rx_total = 0u64;
    let mut tx_total = 0u64;
    for line in text.lines() {
        let Some((name, rest)) = line.split_once(':') else {
            continue;
        };
        if name.trim() == "lo" {
            continue;
        }
        let fields: Vec<&str> = rest.split_whitespace().collect();
        // 第 1 个字段是 rx_bytes，第 9 个是 tx_bytes
        let rx = fields.first().and_then(|v| v.parse::<u64>().ok());
        let tx = fields.get(8).and_then(|v| v.parse::<u64>().ok());
        if let (Some(rx), Some(tx)) = (rx, tx) {
            rx_total = rx_total.saturating_add(rx);
            tx_total = tx_total.saturating_add(tx);
        }
    }
    (rx_total, tx_total)
}

#[cfg(unix)]
mod platform {
    pub(super) fn counters() -> Option<(u64, u64)> {
        let text = std::fs::read_to_string("/proc/net/dev").ok()?;
        Some(super::parse_net_dev(&text))
    }
}

#[cfg(windows)]
mod platform {
    use windows_sys::Win32::NetworkManagement::IpHelper::{
        FreeMibTable, GetIfTable2, MIB_IF_TABLE2,
    };
    use windows_sys::Win32::NetworkManagement::Ndis::IfOperStatusUp;

    /// 回环接口不作为带宽口径。
    const IF_TYPE_SOFTWARE_LOOPBACK: u32 = 24;

    pub(super) fn counters() -> Option<(u64, u64)> {
        let mut table: *mut MIB_IF_TABLE2 = std::ptr::null_mut();
        let ret = unsafe { GetIfTable2(&mut table) };
        if ret != 0 || table.is_null() {
            return None;
        }
        let mut rx = 0u64;
        let mut tx = 0u64;
        unsafe {
            let header = &*table;
            let count = header.NumEntries as usize;
            let rows = header.Table.as_ptr();
            for i in 0..count {
                let row = &*rows.add(i);
                // 只排除回环与断开状态的网卡，隧道/虚拟网卡同样算整机流量
                if row.Type == IF_TYPE_SOFTWARE_LOOPBACK || row.OperStatus != IfOperStatusUp {
                    continue;
                }
                rx = rx.saturating_add(row.InOctets);
                tx = tx.saturating_add(row.OutOctets);
            }
            FreeMibTable(table as *const core::ffi::c_void);
        }
        Some((rx, tx))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_net_dev_skipping_loopback_and_header() {
        let text = "Inter-|   Receive                                                |  Transmit\n\
 face |bytes    packets errs drop fifo frame compressed multicast|bytes    packets errs drop fifo colls carrier compressed\n\
    lo: 1000000   1000    0    0    0     0          0         0  1000000   1000    0    0    0     0       0          0\n\
  eth0: 5000      50      0    0    0     0          0         0   300      3     0    0    0     0      0          0\n\
  eth1: 700       7       0    0    0     0          0         0   200      2     0    0    0     0      0          0\n";
        let (rx, tx) = parse_net_dev(text);
        assert_eq!(rx, 5700, "只累加非回环网卡");
        assert_eq!(tx, 500);
    }

    #[test]
    fn skips_malformed_lines() {
        let text = "bad line without colon\neth0: not_a_number 1 2\nwg0: 1 2 3 4 5 6 7 8 9\n";
        let (rx, tx) = parse_net_dev(text);
        assert_eq!(rx, 1);
        assert_eq!(tx, 9);
    }

    #[test]
    fn sampler_first_call_has_no_rate() {
        let mut sampler = TrafficSampler::new();
        assert!(sampler.sample().is_none(), "首次采样只建基准");
        assert!(sampler.sample().is_some(), "第二次应能算出速率");
    }

    #[test]
    fn empty_input_yields_zero() {
        assert_eq!(parse_net_dev(""), (0, 0));
    }
}
