//! 限值的解析、格式化与 cgroup 目录命名：不触碰文件系统，任何平台都能单测。

use std::fmt;

/// 字节数的短格式（1024 进制，保留一位小数）。
pub use crate::units::format_bytes;

/// 单核的默认调度周期（微秒），与内核默认值一致。
const CPU_PERIOD_US: u64 = 100_000;
/// 内核要求的最小配额（微秒）。
const CPU_MIN_QUOTA_US: u64 = 1_000;
/// 内存上限的下限：小于一页没有意义。
const MIN_MEMORY_BYTES: u64 = 4 * 1024;
/// 内存上限的上限，避免明显笔误（如把字节当成 MB 多写几个零）。
const MAX_MEMORY_BYTES: u64 = 1 << 50;

/// CPU 带宽上限，对应 `cpu.max` 的 `quota period` 两个值（微秒）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CpuLimit {
    pub quota: u64,
    pub period: u64,
}

impl CpuLimit {
    /// 支持三种写法：`50%`（占单核百分比）、`0.5`（核数）、`50000 100000`（原始 quota/period）。
    ///
    /// # Errors
    /// 数字无法解析、小于内核允许的最小配额、周期越界时返回可直接展示的说明。
    pub fn parse(raw: &str) -> Result<Option<Self>, String> {
        let raw = raw.trim();
        if raw.is_empty() || is_unlimited(raw) {
            return Ok(None);
        }
        if let Some(percent) = raw.strip_suffix('%') {
            let percent = parse_positive(percent, "cpu_max 百分比")?;
            let quota = (percent * CPU_PERIOD_US as f64 / 100.0).round() as u64;
            return Self::from_quota(quota, CPU_PERIOD_US);
        }
        let parts: Vec<&str> = raw.split_whitespace().collect();
        if parts.len() == 2 {
            let quota = parse_u64(parts[0], "cpu_max 配额")?;
            let period = parse_u64(parts[1], "cpu_max 周期")?;
            if !(1_000..=1_000_000).contains(&period) {
                return Err(format!(
                    "cpu_max 的周期 {period} 越界：内核只接受 1000..=1000000 微秒"
                ));
            }
            return Self::from_quota(quota, period);
        }
        // 单个数字按「核数」理解：1 = 一个核，0.5 = 半个核。
        let cores = parse_positive(raw, "cpu_max 核数")?;
        let quota = (cores * CPU_PERIOD_US as f64).round() as u64;
        Self::from_quota(quota, CPU_PERIOD_US)
    }

    fn from_quota(quota: u64, period: u64) -> Result<Option<Self>, String> {
        if quota < CPU_MIN_QUOTA_US {
            return Err(format!(
                "cpu_max 太小（{quota}µs / {period}µs）：内核要求配额至少 {CPU_MIN_QUOTA_US}µs，即 1% 单核"
            ));
        }
        Ok(Some(Self { quota, period }))
    }

    /// 人能读的百分比（如 `50%`、`150%`）。
    #[must_use]
    pub fn percent(&self) -> f64 {
        self.quota as f64 * 100.0 / self.period as f64
    }
}

impl fmt::Display for CpuLimit {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{} {}", self.quota, self.period)
    }
}

/// 解析内存上限：字节数，或带 B / K / M / G / T 后缀（按 1024 进制）。
///
/// # Errors
/// 单位无法识别、数值非正、低于一页或超过 1 PiB 时返回说明。
pub fn parse_memory(raw: &str) -> Result<Option<u64>, String> {
    let raw = raw.trim();
    if raw.is_empty() || is_unlimited(raw) {
        return Ok(None);
    }
    let (number, suffix) = split_number_suffix(raw);
    let value = parse_positive(number, "memory_max")?;
    let multiplier = memory_multiplier(suffix)?;
    let bytes = (value * multiplier as f64).round() as u64;
    if bytes < MIN_MEMORY_BYTES {
        return Err(format!(
            "memory_max 太小：至少 {MIN_MEMORY_BYTES} 字节（一页）"
        ));
    }
    if bytes > MAX_MEMORY_BYTES {
        return Err("memory_max 超过 1 PiB，请检查单位".into());
    }
    Ok(Some(bytes))
}

/// 解析任务数上限（`pids.max`）：正整数或 `max`。
///
/// # Errors
/// 数值为 0 或不是整数时返回说明。
pub fn parse_pids(raw: &str) -> Result<Option<u64>, String> {
    let raw = raw.trim();
    if raw.is_empty() || is_unlimited(raw) {
        return Ok(None);
    }
    let value = parse_u64(raw, "pids_max")?;
    if value == 0 {
        return Err("pids_max 必须大于 0（0 会让进程创建不了任何任务）".into());
    }
    Ok(Some(value))
}

/// 一组已经解析好的硬限；全部为 None 表示不启用 cgroup。
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct CgroupSpec {
    pub cpu: Option<CpuLimit>,
    pub memory_bytes: Option<u64>,
    pub pids: Option<u64>,
}

impl CgroupSpec {
    /// 从三个配置字段解析。空白视为未配置。
    ///
    /// # Errors
    /// 任一字段写错时返回可直接展示给用户的说明（带字段名与允许的写法）。
    pub fn parse(
        cpu_max: Option<&str>,
        memory_max: Option<&str>,
        pids_max: Option<&str>,
    ) -> Result<Self, String> {
        Ok(Self {
            cpu: cpu_max.map(CpuLimit::parse).transpose()?.flatten(),
            memory_bytes: memory_max.map(parse_memory).transpose()?.flatten(),
            pids: pids_max.map(parse_pids).transpose()?.flatten(),
        })
    }

    #[must_use]
    pub fn is_active(&self) -> bool {
        self.cpu.is_some() || self.memory_bytes.is_some() || self.pids.is_some()
    }

    /// 需要启用的控制器名（用于 `cgroup.subtree_control`）。
    #[must_use]
    pub fn controllers(&self) -> Vec<&'static str> {
        let mut names = Vec::new();
        if self.cpu.is_some() {
            names.push("cpu");
        }
        if self.memory_bytes.is_some() {
            names.push("memory");
        }
        if self.pids.is_some() {
            names.push("pids");
        }
        names
    }

    /// 一句话描述，用于启动日志：「cpu 50%、内存 512 MB、pids 100」。
    #[must_use]
    pub fn describe(&self) -> String {
        let mut parts = Vec::new();
        if let Some(cpu) = &self.cpu {
            parts.push(format!("cpu {:.0}%", cpu.percent()));
        }
        if let Some(bytes) = self.memory_bytes {
            parts.push(format!("内存 {}", format_bytes(bytes)));
        }
        if let Some(pids) = self.pids {
            parts.push(format!("pids {pids}"));
        }
        parts.join("、")
    }
}

/// cgroup 目录名：可读前缀 + 原名的 FNV-1a 哈希，避免不同进程名清洗后撞车。
#[must_use]
pub fn slug(name: &str) -> String {
    let mut body = String::new();
    for ch in name.chars() {
        if body.len() >= 40 {
            break;
        }
        if ch.is_ascii_alphanumeric() || matches!(ch, '-' | '_') {
            body.push(ch.to_ascii_lowercase());
        } else {
            body.push('-');
        }
    }
    let body = body.trim_matches('-');
    let body = if body.is_empty() { "proc" } else { body };
    format!("{body}-{:08x}", fnv1a(name))
}

fn fnv1a(name: &str) -> u32 {
    let mut hash: u32 = 0x811c_9dc5;
    for byte in name.as_bytes() {
        hash ^= u32::from(*byte);
        hash = hash.wrapping_mul(0x0100_0193);
    }
    hash
}

fn is_unlimited(raw: &str) -> bool {
    matches!(
        raw.to_ascii_lowercase().as_str(),
        "max" | "none" | "unlimited"
    )
}

fn parse_u64(raw: &str, what: &str) -> Result<u64, String> {
    raw.trim()
        .parse::<u64>()
        .map_err(|_| format!("{what} 不是非负整数: {raw}"))
}

fn parse_positive(raw: &str, what: &str) -> Result<f64, String> {
    let value: f64 = raw
        .trim()
        .parse()
        .map_err(|_| format!("{what} 不是数字: {raw}"))?;
    if !value.is_finite() || value <= 0.0 {
        return Err(format!("{what} 必须大于 0: {raw}"));
    }
    Ok(value)
}

/// 把 `"512M"` / `"1.5 GB"` 拆成数字与单位两段；没有单位时后半段为空。
fn split_number_suffix(raw: &str) -> (&str, &str) {
    let split = raw
        .char_indices()
        .find(|(_, ch)| !ch.is_ascii_digit() && *ch != '.')
        .map_or(raw.len(), |(index, _)| index);
    (&raw[..split], raw[split..].trim())
}

fn memory_multiplier(suffix: &str) -> Result<u64, String> {
    let raw = suffix.trim();
    let suffix = raw.to_ascii_lowercase();
    Ok(match suffix.as_str() {
        "" | "b" => 1,
        "k" | "kb" | "ki" | "kib" => 1 << 10,
        "m" | "mb" | "mi" | "mib" => 1 << 20,
        "g" | "gb" | "gi" | "gib" => 1 << 30,
        "t" | "tb" | "ti" | "tib" => 1 << 40,
        _ => {
            return Err(format!(
                "memory_max 的单位 {raw} 无法识别（可用 B / K / M / G / T，按 1024 进制）"
            ));
        }
    })
}
