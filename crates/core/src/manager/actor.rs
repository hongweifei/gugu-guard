//! Actor 主循环与命令处理：每个写操作如何落到 `State` 上。

use crate::config::{AppConfig, ProcessConfig};
use crate::error::{GuguError, Result};
use crate::process::{LogEntry, ManagedProcess};
use tokio::sync::{broadcast, mpsc, watch};

use super::monitor::run_monitor_cycle;
use super::state::State;
use super::{Command, ProcessSnapshot};

pub(super) async fn actor_loop(
    mut state: State,
    mut cmd_rx: mpsc::Receiver<Command>,
    snap_tx: watch::Sender<ProcessSnapshot>,
) {
    let mut monitor_interval = tokio::time::interval(std::time::Duration::from_secs(2));

    loop {
        tokio::select! {
            biased;

            cmd = cmd_rx.recv() => {
                let Some(cmd) = cmd else { break };
                match cmd {
                    Command::Shutdown => {
                        state.stop_all().await;
                        break;
                    }
                    c => handle(&mut state, &snap_tx, c).await,
                }
            }

            _ = monitor_interval.tick() => {
                run_monitor_cycle(&mut state).await;
                broadcast(&state, &snap_tx);
            }
        }
    }

    tracing::info!("进程管理器 actor 已停止");
}

fn broadcast(state: &State, snap_tx: &watch::Sender<ProcessSnapshot>) {
    let _ = snap_tx.send(state.snapshot());
}

pub(super) async fn handle(
    state: &mut State,
    snap_tx: &watch::Sender<ProcessSnapshot>,
    cmd: Command,
) {
    match cmd {
        Command::StartProcess { name, reply } => {
            let _ = reply.send(do_start(state, &name).await);
            broadcast(state, snap_tx);
        }
        Command::StopProcess { name, reply } => {
            let _ = reply.send(do_stop(state, &name).await);
            broadcast(state, snap_tx);
        }
        Command::RestartProcess { name, reply } => {
            let _ = reply.send(do_restart(state, &name).await);
            broadcast(state, snap_tx);
        }
        Command::CheckHealth { name, reply } => {
            let _ = reply.send(do_check_health(state, &name).await);
            broadcast(state, snap_tx);
        }
        Command::AddProcess {
            name,
            config,
            start_now,
            reply,
        } => {
            let _ = reply.send(do_add(state, name, config, start_now).await);
            broadcast(state, snap_tx);
        }
        Command::UpdateProcess {
            name,
            config,
            new_name,
            force_restart,
            reply,
        } => {
            let _ = reply.send(do_update(state, &name, config, new_name, force_restart).await);
            broadcast(state, snap_tx);
        }
        Command::RemoveProcess { name, reply } => {
            let _ = reply.send(do_remove(state, &name).await);
            broadcast(state, snap_tx);
        }
        Command::ClearLogs { name, reply } => {
            let _ = reply.send(do_clear_logs(state, &name).await);
        }
        Command::ReloadConfig { config, reply } => {
            let _ = reply.send(do_reload(state, &config).await);
            broadcast(state, snap_tx);
        }
        Command::ReloadFromFile { reply } => {
            let _ = reply.send(do_reload_from_file(state).await);
            broadcast(state, snap_tx);
        }
        Command::GetProcessConfig { name, reply } => {
            // 返回配置声明式视图 (相对路径)，与 gugu.toml 磁盘内容一致：
            // WebUI 编辑对话框据此回填，保存时原样回传，不再泄漏解析后的绝对路径。
            let cfg = state
                .processes
                .get(&name)
                .map(|p| state.declared_paths(p.config()));
            let _ = reply.send(Ok(cfg));
        }
        Command::GetProcessLogs { name, lines, reply } => {
            let _ = reply.send(do_get_logs(state, &name, lines).await);
        }
        Command::SubscribeLogs { name, reply } => {
            let _ = reply.send(do_subscribe(state, &name));
        }
        Command::Shutdown => unreachable!(),
    }
}

// ── 具体操作 ─────────────────────────────────────────────────

fn get_proc<'a>(state: &'a mut State, name: &str) -> Result<&'a mut ManagedProcess> {
    state
        .processes
        .get_mut(name)
        .ok_or_else(|| GuguError::ProcessNotFound(name.to_string()))
}

async fn do_start(state: &mut State, name: &str) -> Result<()> {
    let proc = get_proc(state, name)?;
    proc.reset_crash_restart_count();
    proc.start().await
}

async fn do_stop(state: &mut State, name: &str) -> Result<()> {
    get_proc(state, name)?.stop().await
}

async fn do_restart(state: &mut State, name: &str) -> Result<()> {
    get_proc(state, name)?.restart().await
}

async fn do_check_health(state: &mut State, name: &str) -> Result<bool> {
    let hc = {
        let proc = get_proc(state, name)?;
        proc.config()
            .health_check
            .clone()
            .ok_or_else(|| GuguError::ConfigError("该进程未配置健康检查".into()))?
    };
    let healthy = crate::health::check_health(&hc).await;
    if let Some(proc) = state.processes.get_mut(name) {
        proc.set_healthy(Some(healthy));
    }
    Ok(healthy)
}

async fn do_add(
    state: &mut State,
    name: String,
    config: ProcessConfig,
    start_now: bool,
) -> Result<()> {
    if state.processes.contains_key(&name) {
        return Err(GuguError::ConfigError(format!("进程 '{name}' 已存在")));
    }
    config.validate()?;
    let mut proc = ManagedProcess::new(name.clone(), config);
    if start_now {
        proc.start().await?;
    }
    state.processes.insert(name.clone(), proc);
    tracing::info!("[{}] 进程配置已添加", name);
    state.save_config()
}

async fn do_update(
    state: &mut State,
    name: &str,
    config: ProcessConfig,
    new_name: Option<String>,
    force_restart: bool,
) -> Result<()> {
    if !state.processes.contains_key(name) {
        return Err(GuguError::ProcessNotFound(name.to_string()));
    }
    config.validate()?;

    let was_running = state.processes[name].is_running();
    let needs_restart = force_restart || !state.processes[name].config().runtime_fields_eq(&config);

    if needs_restart && was_running {
        // SAFETY: 上方已验证 name 存在于 processes
        state
            .processes
            .get_mut(name)
            .expect("已验证存在")
            .stop()
            .await?;
    }
    // SAFETY: 同上
    *state
        .processes
        .get_mut(name)
        .expect("已验证存在")
        .config_mut() = config;

    if let Some(ref new) = new_name {
        if new != name {
            if state.processes.contains_key(new) {
                return Err(GuguError::ConfigError(format!("进程 '{new}' 已存在")));
            }
            // SAFETY: name 已验证存在，且 new 不存在
            let mut proc = state.processes.remove(name).expect("已验证存在");
            proc.rename(new.clone());
            state.processes.insert(new.clone(), proc);
            tracing::info!("[{}] 进程已改名为 [{}]", name, new);
        }
    }

    let target = new_name.as_deref().unwrap_or(name);
    if needs_restart && was_running {
        if let Some(proc) = state.processes.get_mut(target) {
            let _ = proc.start().await;
        }
    }
    tracing::info!("[{}] 进程配置已更新", target);
    state.save_config()
}

async fn do_remove(state: &mut State, name: &str) -> Result<()> {
    let Some(proc) = state.processes.get_mut(name) else {
        return Err(GuguError::ProcessNotFound(name.to_string()));
    };
    if proc.is_running() {
        proc.stop().await?;
    }
    state.processes.remove(name);
    tracing::info!("[{}] 进程已移除", name);
    state.save_config()
}

async fn do_clear_logs(state: &mut State, name: &str) -> Result<()> {
    get_proc(state, name)?.clear_logs().await;
    Ok(())
}

async fn do_get_logs(state: &mut State, name: &str, lines: usize) -> Result<Vec<LogEntry>> {
    let proc = state
        .processes
        .get(name)
        .ok_or_else(|| GuguError::ProcessNotFound(name.to_string()))?;
    Ok(proc.logs(lines).await)
}

fn do_subscribe(state: &State, name: &str) -> Result<broadcast::Receiver<LogEntry>> {
    let proc = state
        .processes
        .get(name)
        .ok_or_else(|| GuguError::ProcessNotFound(name.to_string()))?;
    Ok(proc.subscribe_logs())
}

async fn do_reload(state: &mut State, new_config: &AppConfig) -> Result<()> {
    state.daemon_config = new_config.daemon.clone();

    let (config_dir, log_base) = State::bases(new_config, state.config_path.as_deref());
    state.config_dir = config_dir.clone();
    state.log_base = log_base.clone();

    let to_remove: Vec<String> = state
        .processes
        .keys()
        .filter(|k| !new_config.processes.contains_key(*k))
        .cloned()
        .collect();
    for name in &to_remove {
        if let Some(proc) = state.processes.get_mut(name) {
            if proc.is_running() {
                let _ = proc.stop().await;
            }
        }
        state.processes.remove(name);
        tracing::info!("[{}] 进程已移除 (配置重载)", name);
    }

    for (name, config) in &new_config.processes {
        if let Err(e) = config.validate() {
            tracing::warn!("[{}] 配置验证失败，跳过: {}", name, e);
            continue;
        }
        let resolved = State::resolve_paths(config, &config_dir, &log_base);
        if let Some(proc) = state.processes.get_mut(name) {
            let was_running = proc.is_running();
            let needs_restart = was_running && !proc.config().runtime_fields_eq(&resolved);
            *proc.config_mut() = resolved;
            if needs_restart {
                let _ = proc.stop().await;
                let _ = proc.start().await;
                tracing::info!("[{}] 配置已更新并重启 (配置重载)", name);
            } else {
                tracing::info!("[{}] 配置已更新 (配置重载)", name);
            }
        } else {
            let mut proc = ManagedProcess::new(name.clone(), resolved);
            if config.auto_start {
                let _ = proc.start().await;
            }
            state.processes.insert(name.clone(), proc);
            tracing::info!("[{}] 新进程已添加 (配置重载)", name);
        }
    }
    Ok(())
}

async fn do_reload_from_file(state: &mut State) -> Result<()> {
    let path = state
        .config_path
        .clone()
        .ok_or_else(|| GuguError::ConfigError("未配置配置文件路径".into()))?;
    let config = AppConfig::load(&path)?;
    do_reload(state, &config).await
}
