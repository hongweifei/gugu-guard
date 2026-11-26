//! 指标时序：把 2 秒一次的瞬时采样压成 1 分钟一桶，保留 24 小时。
//!
//! 内存环形缓冲，进程/机器两条序列共用同一套桶结构。守护重启后历史清空——
//! 需要跨重启的长期数据看 `history::exits` 的退出日志。

use std::collections::{HashMap, VecDeque};

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

use crate::process::UsageSample;

use super::traffic::NetRate;

/// 一个桶覆盖的秒数。
pub const BUCKET_SECS: i64 = 60;
/// 保留的桶数量（24 小时）。
pub const WINDOW_BUCKETS: usize = 24 * 60;

/// 一个进程在 1 分钟内的资源与连接汇总。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MetricPoint {
    pub at: DateTime<Utc>,
    pub cpu_avg: f32,
    pub cpu_peak: f32,
    pub mem_avg: u64,
    pub mem_peak: u64,
    /// 窗口内连接数峰值。
    pub connections: u32,
}

/// 一个机器级带宽桶。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TrafficPoint {
    pub at: DateTime<Utc>,
    pub rx_bytes_per_sec: f64,
    pub tx_bytes_per_sec: f64,
}

#[derive(Debug, Clone)]
struct Bucket {
    at: i64,
    cpu_sum: f64,
    cpu_peak: f32,
    mem_sum: u64,
    mem_peak: u64,
    conn_peak: u32,
    samples: u32,
}

impl Bucket {
    fn new(at: i64) -> Self {
        Self {
            at,
            cpu_sum: 0.0,
            cpu_peak: 0.0,
            mem_sum: 0,
            mem_peak: 0,
            conn_peak: 0,
            samples: 0,
        }
    }

    fn add(&mut self, usage: &UsageSample, connections: u32) {
        self.cpu_sum += f64::from(usage.cpu_percent);
        self.cpu_peak = self.cpu_peak.max(usage.cpu_percent);
        self.mem_sum = self.mem_sum.saturating_add(usage.memory_bytes);
        self.mem_peak = self.mem_peak.max(usage.memory_bytes);
        self.conn_peak = self.conn_peak.max(connections);
        self.samples += 1;
    }

    fn point(&self) -> MetricPoint {
        let samples = self.samples.max(1);
        MetricPoint {
            at: timestamp(self.at),
            cpu_avg: (self.cpu_sum / f64::from(samples)) as f32,
            cpu_peak: self.cpu_peak,
            mem_avg: self.mem_sum / u64::from(samples),
            mem_peak: self.mem_peak,
            connections: self.conn_peak,
        }
    }
}

#[derive(Debug, Clone)]
struct TrafficBucket {
    at: i64,
    rx_sum: f64,
    tx_sum: f64,
    samples: u32,
}

impl TrafficBucket {
    fn new(at: i64) -> Self {
        Self {
            at,
            rx_sum: 0.0,
            tx_sum: 0.0,
            samples: 0,
        }
    }

    fn add(&mut self, rate: &NetRate) {
        self.rx_sum += rate.rx_bytes_per_sec;
        self.tx_sum += rate.tx_bytes_per_sec;
        self.samples += 1;
    }

    fn point(&self) -> TrafficPoint {
        let samples = f64::from(self.samples.max(1));
        TrafficPoint {
            at: timestamp(self.at),
            rx_bytes_per_sec: self.rx_sum / samples,
            tx_bytes_per_sec: self.tx_sum / samples,
        }
    }
}

fn timestamp(epoch_secs: i64) -> DateTime<Utc> {
    DateTime::from_timestamp(epoch_secs, 0).unwrap_or_else(Utc::now)
}

/// 桶起点：按分钟对齐，多台/多进程同一时刻落在同一桶。
fn bucket_start(now: DateTime<Utc>) -> i64 {
    let secs = now.timestamp();
    secs - secs.rem_euclid(BUCKET_SECS)
}

/// 进程级与机器级的时序仓库。
#[derive(Debug, Default)]
pub struct MetricsStore {
    processes: HashMap<String, VecDeque<Bucket>>,
    traffic: VecDeque<TrafficBucket>,
}

impl MetricsStore {
    pub fn record_process(
        &mut self,
        name: &str,
        usage: &UsageSample,
        connections: u32,
        now: DateTime<Utc>,
    ) {
        let at = bucket_start(now);
        let series = self.processes.entry(name.to_string()).or_default();
        match series.back_mut() {
            Some(bucket) if bucket.at == at => bucket.add(usage, connections),
            _ => {
                let mut bucket = Bucket::new(at);
                bucket.add(usage, connections);
                series.push_back(bucket);
            }
        }
        prune(series, at);
    }

    pub fn record_traffic(&mut self, rate: &NetRate, now: DateTime<Utc>) {
        let at = bucket_start(now);
        match self.traffic.back_mut() {
            Some(bucket) if bucket.at == at => bucket.add(rate),
            _ => {
                let mut bucket = TrafficBucket::new(at);
                bucket.add(rate);
                self.traffic.push_back(bucket);
            }
        }
        while self
            .traffic
            .front()
            .is_some_and(|b| b.at < at - BUCKET_SECS * (WINDOW_BUCKETS as i64))
        {
            self.traffic.pop_front();
        }
    }

    /// 某个进程最近 `minutes` 分钟的指标点（按时间升序）。
    pub fn process_points(&self, name: &str, minutes: u64) -> Vec<MetricPoint> {
        let Some(series) = self.processes.get(name) else {
            return Vec::new();
        };
        let cutoff = Utc::now().timestamp() - (minutes as i64) * 60;
        series
            .iter()
            .filter(|b| b.at >= cutoff)
            .map(Bucket::point)
            .collect()
    }

    /// 机器带宽最近 `minutes` 分钟的点。
    pub fn traffic_points(&self, minutes: u64) -> Vec<TrafficPoint> {
        let cutoff = Utc::now().timestamp() - (minutes as i64) * 60;
        self.traffic
            .iter()
            .filter(|b| b.at >= cutoff)
            .map(TrafficBucket::point)
            .collect()
    }

    /// 最近一分钟的机器带宽。
    pub fn latest_traffic(&self) -> Option<TrafficPoint> {
        self.traffic.back().map(TrafficBucket::point)
    }

    /// 进程被移除时丢掉它的序列，避免无限增长。
    pub fn forget(&mut self, name: &str) {
        self.processes.remove(name);
    }

    /// 进程改名时把已有序列挪到新名字下。
    pub fn rename(&mut self, old: &str, new: &str) {
        if let Some(series) = self.processes.remove(old) {
            self.processes.insert(new.to_string(), series);
        }
    }
}

/// 只保留最近窗口内的桶；序列被清空就整条移除。
fn prune(series: &mut VecDeque<Bucket>, latest: i64) {
    let cutoff = latest - BUCKET_SECS * (WINDOW_BUCKETS as i64);
    while series.front().is_some_and(|b| b.at < cutoff) {
        series.pop_front();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn usage(cpu: f32, mem: u64) -> UsageSample {
        UsageSample {
            cpu_percent: cpu,
            memory_bytes: mem,
            process_count: 1,
        }
    }

    #[test]
    fn aggregates_same_minute_into_one_bucket() {
        let mut store = MetricsStore::default();
        let now = Utc::now();
        store.record_process("web", &usage(10.0, 100), 2, now);
        store.record_process("web", &usage(30.0, 300), 5, now);
        store.record_process("web", &usage(20.0, 200), 1, now);

        let points = store.process_points("web", 60);
        assert_eq!(points.len(), 1, "同一分钟只应有一个桶");
        let p = &points[0];
        assert!((p.cpu_avg - 20.0).abs() < 0.01, "CPU 取均值: {p:?}");
        assert_eq!(p.cpu_peak, 30.0);
        assert_eq!(p.mem_avg, 200);
        assert_eq!(p.mem_peak, 300);
        assert_eq!(p.connections, 5, "连接数取峰值");
    }

    #[test]
    fn minute_boundary_opens_a_new_bucket() {
        let mut store = MetricsStore::default();
        let now = Utc::now();
        store.record_process("web", &usage(1.0, 10), 0, now);
        // 下一分钟的同一秒
        let later = now + chrono::Duration::seconds(BUCKET_SECS);
        store.record_process("web", &usage(2.0, 20), 0, later);
        assert_eq!(store.process_points("web", 60).len(), 2);
    }

    #[test]
    fn prune_keeps_window_bounded() {
        let mut store = MetricsStore::default();
        let now = Utc::now();
        // 造 2 天前的一个点，然后按最新时间记录，旧桶应被清掉
        let old = now - chrono::Duration::days(2);
        store.record_process("web", &usage(1.0, 10), 0, old);
        store.record_process("web", &usage(1.0, 10), 0, now);
        let points = store.process_points("web", WINDOW_BUCKETS as u64);
        assert_eq!(points.len(), 1, "超出 24 小时的桶应被丢弃");
    }

    #[test]
    fn forget_removes_series() {
        let mut store = MetricsStore::default();
        store.record_process("web", &usage(1.0, 10), 0, Utc::now());
        assert!(!store.process_points("web", 60).is_empty());
        store.forget("web");
        assert!(store.process_points("web", 60).is_empty());
    }

    #[test]
    fn traffic_series_holds_average_rate() {
        let mut store = MetricsStore::default();
        let now = Utc::now();
        store.record_traffic(
            &NetRate {
                rx_bytes_per_sec: 100.0,
                tx_bytes_per_sec: 50.0,
            },
            now,
        );
        store.record_traffic(
            &NetRate {
                rx_bytes_per_sec: 300.0,
                tx_bytes_per_sec: 150.0,
            },
            now,
        );
        let points = store.traffic_points(60);
        assert_eq!(points.len(), 1);
        assert!((points[0].rx_bytes_per_sec - 200.0).abs() < 0.01);
        assert!((points[0].tx_bytes_per_sec - 100.0).abs() < 0.01);
        let latest = store.latest_traffic().expect("应能取到最近一分钟");
        assert!((latest.rx_bytes_per_sec - 200.0).abs() < 0.01);
    }
}
