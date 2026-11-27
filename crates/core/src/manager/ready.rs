//! 启动时序里的就绪等待：依赖探通之后才拉起下游进程。
//!
//! 就绪判定复用已有的检查口径：
//! - 配了 `health_check` 就轮询健康检查（TCP/HTTP，语义最准）；
//! - 只配了端口就以端口进入 LISTEN 为准（`/proc` 表直读，比连上去更轻）；
//! - 两者都没有，视为启动即就绪（顺序由拓扑排序保证）。

use std::time::Duration;

use tokio::time::Instant;

use crate::process::ManagedProcess;

use super::state::State;

/// 就绪轮询间隔：比健康检查的 `interval_secs` 密，否则冷启动要等一整个检查间隔。
const POLL_INTERVAL: Duration = Duration::from_millis(500);

/// 等待 `dep` 就绪；超时、依赖退出或压根没在运行时返回 false。
pub(super) async fn wait_dependency(state: &mut State, dep: &str, timeout: Duration) -> bool {
    let Some(proc) = state.processes.get(dep) else {
        return false;
    };
    if !proc.is_running() {
        tracing::warn!("[{dep}] 未在运行（或已被停机），依赖它的进程不会启动");
        return false;
    }
    let (health, ports) = readiness_target(proc);
    if health.is_none() && ports.is_empty() {
        return true;
    }

    let deadline = Instant::now() + timeout;
    loop {
        let alive = match state.processes.get_mut(dep) {
            Some(proc) => proc.check_alive(),
            None => false,
        };
        if !alive {
            tracing::warn!("[{dep}] 在等待就绪期间退出");
            return false;
        }

        let ready = match &health {
            Some(config) => crate::health::check_health(config).await,
            None => ports.iter().all(|port| crate::sockets::is_listening(*port)),
        };
        if ready {
            if let Some(proc) = state.processes.get_mut(dep) {
                proc.set_healthy(Some(true));
            }
            tracing::info!("[{dep}] 已就绪");
            return true;
        }

        if Instant::now() >= deadline {
            tracing::warn!("[{dep}] 等待就绪超时 ({timeout:?})");
            return false;
        }
        tokio::time::sleep(POLL_INTERVAL).await;
    }
}

fn readiness_target(proc: &ManagedProcess) -> (Option<crate::config::HealthCheckConfig>, Vec<u16>) {
    (proc.config().health_check.clone(), proc.watched_ports())
}

#[cfg(test)]
mod tests {
    // 就绪等待的真实进程行为在 `tests/manager.rs` 的端到端用例里覆盖；
    // 这里只锁住「检查目标取自配置」的判定口径。
    use super::*;
    use crate::config::ProcessConfig;

    fn config() -> ProcessConfig {
        ProcessConfig {
            command: "true".into(),
            args: Vec::new(),
            working_dir: None,
            env: Default::default(),
            auto_start: true,
            auto_restart: true,
            max_restarts: 3,
            restart_delay_secs: 1,
            restart_reset_secs: 60,
            shell: false,
            stop_command: None,
            stop_timeout_secs: 1,
            health_check: None,
            unhealthy_restart: false,
            depends_on: Vec::new(),
            group: None,
            max_memory_mb: None,
            cpu_max: None,
            memory_max: None,
            pids_max: None,
            max_log_size_mb: None,
            stdout_log: None,
            stderr_log: None,
            log_format: Default::default(),
            encoding: Default::default(),
            listen_ports: Vec::new(),
            port_check: true,
            ready_timeout_secs: 60,
            log_alert: None,
        }
    }

    #[test]
    fn without_probe_target_readiness_is_immediate() {
        let proc = ManagedProcess::new("plain".into(), config());
        let (health, ports) = readiness_target(&proc);
        assert!(health.is_none());
        assert!(ports.is_empty());
    }

    #[test]
    fn declared_ports_become_the_readiness_target() {
        let mut cfg = config();
        cfg.listen_ports = vec![8080, 8080, 443];
        let proc = ManagedProcess::new("web".into(), cfg);
        let (health, ports) = readiness_target(&proc);
        assert!(health.is_none());
        assert_eq!(ports, vec![443, 8080], "端口应去重并排序");
    }
}
