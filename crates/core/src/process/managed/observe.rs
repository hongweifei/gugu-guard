//! 资源用量、连接数、健康检查与日志告警的节拍采样。

use crate::process::logwatch::LogAlert;
use crate::process::UsageSample;

use super::ManagedProcess;

impl ManagedProcess {
    /// 采样进程树资源占用（监控循环按节拍调用；非运行态清空）。
    pub fn sample_usage(&mut self) {
        let Some(pid) = self.child.as_ref().and_then(tokio::process::Child::id) else {
            self.usage = None;
            self.connections = 0;
            return;
        };
        if let Some(sample) = self.usage_sampler.sample(pid) {
            self.usage = Some(sample);
        }
    }

    /// 按本端口的已建立连接数刷新；`counts` 是本周期的全机端口计数表。
    pub fn sample_connections(&mut self, counts: &std::collections::HashMap<u16, u32>) {
        if !self.is_running() {
            self.connections = 0;
            return;
        }
        self.connections = self
            .watched_ports()
            .iter()
            .filter_map(|port| counts.get(port))
            .sum();
    }

    #[must_use]
    pub fn usage(&self) -> Option<UsageSample> {
        self.usage
    }

    #[must_use]
    pub fn connections(&self) -> u32 {
        self.connections
    }

    /// 取走日志分析器攒下的告警（监控循环每节拍调用一次）。
    pub fn take_log_alerts(&self) -> Vec<LogAlert> {
        match &self.log_watcher {
            Some(watcher) => watcher.take_alerts(),
            None => Vec::new(),
        }
    }

    /// 记录一次健康检查结果，返回 (当前连续失败次数, 是否达到判定阈值)。
    /// 达到阈值才允许触发 unhealthy_restart —— 单次抖动不再误伤进程。
    pub fn record_health(&mut self, ok: bool) -> (u32, bool) {
        if ok {
            self.health_failures = 0;
            return (0, false);
        }
        self.health_failures += 1;
        let threshold = self
            .config
            .health_check
            .as_ref()
            .map_or(1, |hc| hc.fail_threshold);
        (self.health_failures, self.health_failures >= threshold)
    }

    /// 进程已运行且存活时长达到 `restart_reset_secs` 时，崩溃重启计数归零，
    /// 让守护长期有效（慢速崩溃循环也能一直被拉起，而非一次事故后永久放弃）。
    #[must_use]
    pub fn maybe_decay_crash_count(&mut self, uptime_secs: i64) -> bool {
        if self.crash_restart_count > 0
            && self.is_running()
            && uptime_secs >= self.config.restart_reset_secs.max(1) as i64
        {
            self.crash_restart_count = 0;
            return true;
        }
        false
    }
}
