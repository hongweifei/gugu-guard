//! 2 秒监控循环：崩溃计数衰减、探活自动重启、健康检查连续失败判定。

use crate::config::HealthCheckConfig;
use crate::process::ProcessStatus;

use super::state::State;

pub(super) async fn run_monitor_cycle(state: &mut State) {
    // 存活达到 restart_reset_secs 的进程把崩溃重启计数归零，守护长期有效
    for (name, proc) in &mut state.processes {
        let uptime = proc.info().uptime_secs.unwrap_or(0);
        if proc.maybe_decay_crash_count(uptime) {
            tracing::info!("[{name}] 已稳定运行 {uptime}s，崩溃重启计数已归零");
        }
    }

    let dead = find_dead(state);
    if !dead.is_empty() {
        // 并行等待各进程的重启延迟，然后依次启动
        let delays: Vec<(String, tokio::time::Sleep)> = dead.into_iter()
            .map(|(name, delay)| {
                tracing::info!("[{}] 准备自动重启，等待 {:?}", name, delay);
                (name, tokio::time::sleep(delay))
            })
            .collect();
        // 使用 join_all 并行等待所有延迟
        let names: Vec<String> = delays.iter().map(|(n, _)| n.clone()).collect();
        futures::future::join_all(delays.into_iter().map(|(_, sl)| sl)).await;
        // 延迟结束后依次启动（需要 &mut state，无法并行）
        for name in names {
            if let Some(proc) = state.processes.get_mut(&name) {
                if !proc.is_running() {
                    if let Err(e) = proc.start().await {
                        tracing::error!("[{}] 自动重启失败: {}", name, e);
                    }
                }
            }
        }
    }

    run_health_checks(state).await;
}

fn find_dead(state: &mut State) -> Vec<(String, std::time::Duration)> {
    let mut dead = Vec::new();
    for (name, proc) in &mut state.processes {
        if proc.is_running() && !proc.check_alive() && proc.should_auto_restart() {
            proc.set_status(ProcessStatus::Restarting);
            proc.mark_crash_restart();
            dead.push((name.clone(), proc.restart_delay()));
        }
    }
    dead
}

async fn run_health_checks(state: &mut State) {
    let now = std::time::Instant::now;

    let to_check: Vec<(String, HealthCheckConfig, bool)> = {
        let mut checks = Vec::new();
        for (name, proc) in &mut state.processes {
            if !proc.is_running() { continue; }
            let (hc, should_check, ur) = {
                let cfg = proc.config();
                match &cfg.health_check {
                    Some(hc) => {
                        let interval = std::time::Duration::from_secs(hc.interval_secs);
                        let should = proc.last_health_check()
                            .is_none_or(|last| now().duration_since(last) >= interval);
                        (hc.clone(), should, cfg.unhealthy_restart)
                    }
                    None => continue,
                }
            };
            if should_check {
                proc.set_last_health_check(Some(now()));
                checks.push((name.clone(), hc, ur));
            }
        }
        checks
    };

    let futures: Vec<_> = to_check.iter().map(|(_, hc, _)| crate::health::check_health(hc)).collect();
    let results = futures::future::join_all(futures).await;

    let mut to_restart = Vec::new();
    for ((name, hc, unhealthy_restart), healthy) in to_check.iter().zip(results) {
        if let Some(proc) = state.processes.get_mut(name) {
            proc.set_healthy(Some(healthy));
            let (fails, tripped) = proc.record_health(healthy);
            if !healthy {
                if tripped {
                    tracing::warn!("[{name}] 健康检查连续失败 {fails} 次 (阈值 {})，判定不健康", hc.fail_threshold);
                    if *unhealthy_restart {
                        to_restart.push(name.clone());
                    }
                } else {
                    tracing::warn!("[{name}] 健康检查失败 ({fails}/{})，等待连续失败确认", hc.fail_threshold);
                }
            }
        }
    }

    for name in to_restart {
        tracing::warn!("[{}] 健康检查失败，准备重启", name);
        if let Some(proc) = state.processes.get_mut(&name) {
            if proc.is_running() {
                let _ = proc.stop().await;
            }
            if proc.should_auto_restart() {
                proc.set_status(ProcessStatus::Restarting);
                proc.mark_crash_restart();
                if let Err(e) = proc.start().await {
                    tracing::error!("[{}] 健康检查重启失败: {}", name, e);
                }
            }
        }
    }
}
