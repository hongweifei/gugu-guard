//! 2 秒监控循环：崩溃计数衰减、探活自动重启、健康检查连续失败判定。

use crate::config::HealthCheckConfig;
use crate::notify;
use crate::process::{format_bytes, ProcessStatus};

use super::state::State;

/// 待投递的通知：`(事件类型, 进程名, 文案)`。
type Notice = (String, String, String);

/// 待自动重启的进程：`(名称, 重启延迟)`。
type PendingRestart = (String, std::time::Duration);

/// 已退出且不会再被拉起的进程：`(名称, 退出码, 已重启次数)`。
type DownExit = (String, Option<i32>, u32);

/// 把收集到的事件投给 webhook（未配置则什么都不做）。
fn fire(state: &State, events: Vec<Notice>) {
    let Some((url, format)) = state.daemon_config.notify_target() else {
        return;
    };
    for (event_type, name, message) in events {
        let payload = notify::event(&event_type, &name, &message);
        notify::spawn(url.to_string(), format, message, payload);
    }
}

pub(super) async fn run_monitor_cycle(state: &mut State) {
    let mut events: Vec<Notice> = Vec::new();

    // 存活达到 restart_reset_secs 的进程把崩溃重启计数归零，守护长期有效
    for (name, proc) in &mut state.processes {
        let uptime = proc.info().uptime_secs.unwrap_or(0);
        if proc.maybe_decay_crash_count(uptime) {
            tracing::info!("[{name}] 已稳定运行 {uptime}s，崩溃重启计数已归零");
        }
    }

    // 资源采样：运行中的进程刷新进程树 CPU/内存（非运行态会清空）
    for proc in state.processes.values_mut() {
        proc.sample_usage();
    }

    // 内存超限：按自动重启路径处理（计入崩溃重启计数，避免重启风暴）
    let mut over_limit: Vec<(String, u64, u64)> = Vec::new();
    for (name, proc) in &state.processes {
        let (Some(limit_mb), Some(usage)) = (proc.config().max_memory_mb, proc.usage()) else {
            continue;
        };
        if limit_mb == 0 {
            continue;
        }
        let limit_bytes = limit_mb.saturating_mul(1024 * 1024);
        if usage.memory_bytes > limit_bytes {
            over_limit.push((name.clone(), usage.memory_bytes, limit_bytes));
        }
    }
    for (name, used, limit) in over_limit {
        tracing::warn!(
            "[{name}] 进程树内存 {} 超过上限 {}，准备重启",
            format_bytes(used),
            format_bytes(limit)
        );
        events.push((
            notify::EVENT_MEMORY_LIMIT.to_string(),
            name.clone(),
            format!(
                "进程 {name} 进程树内存 {} 超过上限 {}，已按自动重启处理",
                format_bytes(used),
                format_bytes(limit)
            ),
        ));
        if let Some(proc) = state.processes.get_mut(&name) {
            if !proc.should_auto_restart() {
                tracing::warn!("[{name}] 已达崩溃重启上限，不再自动重启（内存仍超限）");
                continue;
            }
            proc.mark_crash_restart();
            // 先 stop() 再改状态：stop() 的前置条件是 is_running()，
            // 提前置 Restarting 会让它直接返回 NotRunning ⇒ 旧进程变孤儿。
            let _ = proc.stop().await;
            proc.set_status(ProcessStatus::Restarting);
            match proc.start().await {
                Ok(()) => proc.mark_last_exit_auto_restarted(),
                Err(e) => tracing::error!("[{name}] 内存超限重启失败: {e}"),
            }
        }
    }

    let (dead, down) = find_dead(state);
    for (name, code, restarts) in down {
        let code_txt = code.map_or_else(|| "未知".to_string(), |c| c.to_string());
        events.push((
            notify::EVENT_DOWN.to_string(),
            name.clone(),
            format!("进程 {name} 已退出（退出码 {code_txt}），累计重启 {restarts} 次后不再自动拉起"),
        ));
    }
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
                    match proc.start().await {
                        Ok(()) => {
                            proc.mark_last_exit_auto_restarted();
                            let restarts = proc.info().restart_count;
                            events.push((
                                notify::EVENT_RECOVERED.to_string(),
                                name.clone(),
                                format!("进程 {name} 已被守护进程重新拉起（累计重启 {restarts} 次）"),
                            ));
                        }
                        Err(e) => tracing::error!("[{}] 自动重启失败: {}", name, e),
                    }
                }
            }
        }
    }

    events.extend(run_health_checks(state).await);

    fire(state, events);
}

/// 返回需要通知的事件（退出后不再拉起 / 健康判定不健康 / 重启成功）。
fn find_dead(state: &mut State) -> (Vec<PendingRestart>, Vec<DownExit>) {
    let mut dead = Vec::new();
    let mut down = Vec::new();
    for (name, proc) in &mut state.processes {
        if proc.is_running() && !proc.check_alive() {
            let last_code = proc.recent_exits().last().and_then(|e| e.code);
            if proc.should_auto_restart() {
                proc.set_status(ProcessStatus::Restarting);
                proc.mark_crash_restart();
                dead.push((name.clone(), proc.restart_delay()));
            } else {
                // 不会再被拉起：这一次退出必须通知到人
                down.push((name.clone(), last_code, proc.info().restart_count));
            }
        }
    }
    (dead, down)
}

async fn run_health_checks(state: &mut State) -> Vec<Notice> {
    let mut events: Vec<Notice> = Vec::new();
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
                // 通知只在「刚跨过阈值」那一次发出：持续不健康不会每轮刷屏；
                // 恢复后再次失败会重新跨过阈值，届时会再通知。
                if fails == hc.fail_threshold {
                    events.push((
                        notify::EVENT_UNHEALTHY.to_string(),
                        name.clone(),
                        format!(
                            "进程 {name} 健康检查连续失败 {fails} 次（阈值 {}），判定不健康",
                            hc.fail_threshold
                        ),
                    ));
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
                match proc.start().await {
                    Ok(()) => {
                        proc.mark_last_exit_auto_restarted();
                        events.push((
                            notify::EVENT_RECOVERED.to_string(),
                            name.clone(),
                            format!("进程 {name} 健康检查失败后已被重新拉起"),
                        ));
                    }
                    Err(e) => tracing::error!("[{}] 健康检查重启失败: {}", name, e),
                }
            }
        }
    }

    events
}
