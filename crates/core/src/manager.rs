use crate::config::{AppConfig, DaemonConfig, HealthCheckConfig, ProcessConfig};
use crate::error::{GuguError, Result};
use crate::process::{LogEntry, ManagedProcess, ProcessInfo, ProcessStatus};
use std::collections::{HashMap, HashSet, VecDeque};
use std::path::PathBuf;
use tokio::sync::{broadcast, mpsc, watch};

// ── Actor 命令 ──────────────────────────────────────────────

enum Command {
    StartProcess { name: String, reply: ReplyTx<()> },
    StopProcess { name: String, reply: ReplyTx<()> },
    RestartProcess { name: String, reply: ReplyTx<()> },
    CheckHealth { name: String, reply: ReplyTx<bool> },
    AddProcess { name: String, config: ProcessConfig, start_now: bool, reply: ReplyTx<()> },
    UpdateProcess { name: String, config: ProcessConfig, new_name: Option<String>, force_restart: bool, reply: ReplyTx<()> },
    RemoveProcess { name: String, reply: ReplyTx<()> },
    ClearLogs { name: String, reply: ReplyTx<()> },
    ReloadConfig { config: AppConfig, reply: ReplyTx<()> },
    ReloadFromFile { reply: ReplyTx<()> },
    GetProcessConfig { name: String, reply: ReplyTx<Option<ProcessConfig>> },
    GetProcessLogs { name: String, lines: usize, reply: ReplyTx<Vec<LogEntry>> },
    SubscribeLogs { name: String, reply: ReplyTx<broadcast::Receiver<LogEntry>> },
    Shutdown,
}

type ReplyTx<T> = tokio::sync::oneshot::Sender<Result<T>>;

// ── 状态快照 ────────────────────────────────────────────────

/// 进程状态快照，通过 watch 通道无锁读取。
#[derive(Debug, Clone, Default)]
pub struct ProcessSnapshot {
    pub processes: Vec<ProcessInfo>,
}

impl ProcessSnapshot {
    pub fn find(&self, name: &str) -> Option<&ProcessInfo> {
        self.processes.iter().find(|p| p.name == name)
    }

    pub fn names(&self) -> Vec<String> {
        self.processes.iter().map(|p| p.name.clone()).collect()
    }
}

// ── SharedManager（公开句柄） ────────────────────────────────

/// 进程管理器的共享句柄。
///
/// 读操作通过 watch 通道无锁获取快照；
/// 写操作通过 mpsc 命令通道序列化到单一 actor 任务执行。
#[derive(Clone)]
pub struct SharedManager {
    cmd_tx: mpsc::Sender<Command>,
    snapshot: watch::Receiver<ProcessSnapshot>,
}

impl SharedManager {
    // ── 读操作（无锁） ──

    pub fn list_processes(&self) -> Vec<ProcessInfo> {
        self.snapshot.borrow().processes.clone()
    }

    pub fn get_process_info(&self, name: &str) -> Option<ProcessInfo> {
        self.snapshot.borrow().find(name).cloned()
    }

    pub fn all_process_names(&self) -> Vec<String> {
        self.snapshot.borrow().names()
    }

    // ── 写操作（命令通道） ──

    pub async fn start_process(&self, name: &str) -> Result<()> {
        self.call(|r| Command::StartProcess { name: name.to_string(), reply: r }).await
    }

    pub async fn stop_process(&self, name: &str) -> Result<()> {
        self.call(|r| Command::StopProcess { name: name.to_string(), reply: r }).await
    }

    pub async fn restart_process(&self, name: &str) -> Result<()> {
        self.call(|r| Command::RestartProcess { name: name.to_string(), reply: r }).await
    }

    pub async fn check_process_health(&self, name: &str) -> Result<bool> {
        self.call_val(|r| Command::CheckHealth { name: name.to_string(), reply: r }).await
    }

    pub async fn add_process(&self, name: String, config: ProcessConfig, start_now: bool) -> Result<()> {
        self.call(|r| Command::AddProcess { name, config, start_now, reply: r }).await
    }

    pub async fn update_process(&self, name: &str, config: ProcessConfig, new_name: Option<String>, force_restart: bool) -> Result<()> {
        self.call(|r| Command::UpdateProcess { name: name.to_string(), config, new_name, force_restart, reply: r }).await
    }

    pub async fn remove_process(&self, name: &str) -> Result<()> {
        self.call(|r| Command::RemoveProcess { name: name.to_string(), reply: r }).await
    }

    pub async fn get_process_config(&self, name: &str) -> Result<Option<ProcessConfig>> {
        self.call_val(|r| Command::GetProcessConfig { name: name.to_string(), reply: r }).await
    }

    pub async fn get_process_logs(&self, name: &str, lines: usize) -> Result<Vec<LogEntry>> {
        self.call_val(|r| Command::GetProcessLogs { name: name.to_string(), lines, reply: r }).await
    }

    pub async fn clear_process_logs(&self, name: &str) -> Result<()> {
        self.call(|r| Command::ClearLogs { name: name.to_string(), reply: r }).await
    }

    pub async fn subscribe_process_logs(&self, name: &str) -> Result<broadcast::Receiver<LogEntry>> {
        self.call_val(|r| Command::SubscribeLogs { name: name.to_string(), reply: r }).await
    }

    pub async fn reload_config(&self, new_config: &AppConfig) -> Result<()> {
        self.call(|r| Command::ReloadConfig { config: new_config.clone(), reply: r }).await
    }

    pub async fn reload_from_file(&self) -> Result<()> {
        self.call(|r| Command::ReloadFromFile { reply: r }).await
    }

    /// 请求 actor 关闭，停止所有进程后退出循环。
    pub fn shutdown(&self) {
        // Shutdown 不需要回复，fire-and-forget。
        // 用 try_send 避免在 channel 满时阻塞。
        let _ = self.cmd_tx.try_send(Command::Shutdown);
    }

    // ── 内部 ──

    async fn call<F>(&self, f: F) -> Result<()>
    where F: FnOnce(ReplyTx<()>) -> Command {
        let (tx, rx) = tokio::sync::oneshot::channel();
        self.cmd_tx.send(f(tx)).await
            .map_err(|_| GuguError::ConfigError("进程管理器已关闭".into()))?;
        rx.await
            .map_err(|_| GuguError::ConfigError("进程管理器未响应".into()))?
    }

    async fn call_val<T, F>(&self, f: F) -> Result<T>
    where F: FnOnce(ReplyTx<T>) -> Command {
        let (tx, rx) = tokio::sync::oneshot::channel();
        self.cmd_tx.send(f(tx)).await
            .map_err(|_| GuguError::ConfigError("进程管理器已关闭".into()))?;
        rx.await
            .map_err(|_| GuguError::ConfigError("进程管理器未响应".into()))?
    }
}

// ── 拓扑排序（公共工具） ─────────────────────────────────────

pub(crate) fn topological_sort(processes: &HashMap<String, ProcessConfig>) -> Result<Vec<String>> {
    let mut in_degree: HashMap<&str, u32> = HashMap::with_capacity(processes.len());
    let mut dependents: HashMap<&str, Vec<&str>> = HashMap::with_capacity(processes.len());

    for name in processes.keys() {
        in_degree.insert(name.as_str(), 0);
        dependents.entry(name.as_str()).or_default();
    }

    for (name, config) in processes {
        for dep in &config.depends_on {
            if !processes.contains_key(dep) {
                tracing::warn!("[{}] 依赖的 '{}' 不存在，忽略", name, dep);
                continue;
            }
            dependents.entry(dep.as_str()).or_default().push(name.as_str());
            // SAFETY: 所有 processes 的 key 已在上方初始化到 in_degree
            *in_degree.get_mut(name.as_str()).unwrap() += 1;
        }
    }

    let mut queue: VecDeque<&str> = in_degree
        .iter()
        .filter(|(_, &deg)| deg == 0)
        .map(|(&name, _)| name)
        .collect();

    let mut result = Vec::with_capacity(processes.len());
    while let Some(name) = queue.pop_front() {
        result.push(name.to_string());
        if let Some(deps) = dependents.get(name) {
            for &dep in deps {
                if let Some(degree) = in_degree.get_mut(dep) {
                    *degree -= 1;
                    if *degree == 0 {
                        queue.push_back(dep);
                    }
                }
            }
        }
    }

    if result.len() != processes.len() {
        let result_set: HashSet<&str> = result.iter().map(String::as_str).collect();
        let remaining: Vec<&str> = processes.keys()
            .filter(|k| !result_set.contains(k.as_str()))
            .map(String::as_str)
            .collect();
        return Err(GuguError::CyclicDependency(
            format!("检测到循环依赖: {}", remaining.join(", "))
        ));
    }

    Ok(result)
}

// ── Actor 内部状态 ──────────────────────────────────────────

struct State {
    processes: HashMap<String, ManagedProcess>,
    daemon_config: DaemonConfig,
    config_path: Option<PathBuf>,
}

impl State {
    fn new(config: &AppConfig, config_path: Option<PathBuf>) -> Self {
        let config_dir = config_path.as_deref()
            .and_then(|p| p.parent())
            .unwrap_or(std::path::Path::new("."));

        let log_base = config.daemon.log_dir.as_ref()
            .map(|d| crate::config::resolve_relative_path(d, config_dir))
            .unwrap_or_else(|| config_dir.to_path_buf());

        let processes = config.processes.iter()
            .map(|(name, pc)| {
                let resolved = Self::resolve_paths(pc, config_dir, &log_base);
                (name.clone(), ManagedProcess::new(name.clone(), resolved))
            })
            .collect();

        Self { processes, daemon_config: config.daemon.clone(), config_path }
    }

    fn resolve_paths(c: &ProcessConfig, config_dir: &std::path::Path, log_base: &std::path::Path) -> ProcessConfig {
        let mut r = c.clone();
        if let Some(ref d) = r.working_dir {
            r.working_dir = Some(crate::config::resolve_relative_path(d, config_dir));
        }
        if let Some(ref p) = r.stdout_log {
            r.stdout_log = Some(crate::config::resolve_relative_path(p, log_base));
        }
        if let Some(ref p) = r.stderr_log {
            r.stderr_log = Some(crate::config::resolve_relative_path(p, log_base));
        }
        r
    }

    fn snapshot(&self) -> ProcessSnapshot {
        ProcessSnapshot { processes: self.processes.values().map(ManagedProcess::info).collect() }
    }

    fn save_config(&self) -> Result<()> {
        let Some(ref path) = self.config_path else { return Ok(()) };
        let config = AppConfig {
            daemon: self.daemon_config.clone(),
            processes: self.processes.iter()
                .map(|(n, p)| (n.clone(), p.config().clone())).collect(),
        };
        config.save(path)?;
        tracing::debug!("配置已保存到 {}", path.display());
        Ok(())
    }

    async fn start_all(&mut self) {
        let configs: HashMap<String, ProcessConfig> = self.processes.iter()
            .map(|(n, p)| (n.clone(), p.config().clone())).collect();

        let order = match topological_sort(&configs) {
            Ok(o) => o,
            Err(e) => {
                tracing::error!("{}，按默认顺序启动", e);
                self.processes.keys().cloned().collect()
            }
        };

        for name in order {
            if let Some(proc) = self.processes.get_mut(&name) {
                if proc.config().auto_start {
                    if let Err(e) = proc.start().await {
                        tracing::error!("自动启动进程 '{}' 失败: {}", name, e);
                    }
                }
            }
        }
    }

    async fn stop_all(&mut self) {
        let configs: HashMap<String, ProcessConfig> = self.processes.iter()
            .map(|(n, p)| (n.clone(), p.config().clone())).collect();

        // 按启动拓扑排序的逆序停止，确保依赖方先于被依赖方停止
        let names: Vec<String> = match topological_sort(&configs) {
            Ok(order) => order.into_iter().rev().collect(),
            Err(_) => self.processes.keys().cloned().collect(),
        };

        let timeout = std::time::Duration::from_secs(30);
        for name in names {
            if let Some(proc) = self.processes.get_mut(&name) {
                if proc.is_running() {
                    match tokio::time::timeout(timeout, proc.stop()).await {
                        Ok(Ok(())) => {}
                        Ok(Err(e)) => tracing::error!("停止进程 '{}' 失败: {}", name, e),
                        Err(_) => tracing::error!("停止进程 '{}' 超时 ({:?})", name, timeout),
                    }
                }
            }
        }
    }
}

// ── 启动入口 ─────────────────────────────────────────────────

/// 创建并启动进程管理器 actor，返回共享句柄。
///
/// 内部会自动启动所有 `auto_start` 进程和监控循环。
pub fn start(config: &AppConfig, config_path: Option<PathBuf>) -> SharedManager {
    let (cmd_tx, cmd_rx) = mpsc::channel(256);
    let mut state = State::new(config, config_path);
    let snapshot = state.snapshot();
    let (snap_tx, snap_rx) = watch::channel(snapshot);

    tokio::spawn(async move {
        state.start_all().await;
        actor_loop(state, cmd_rx, snap_tx).await;
    });

    SharedManager { cmd_tx, snapshot: snap_rx }
}

async fn actor_loop(mut state: State, mut cmd_rx: mpsc::Receiver<Command>, snap_tx: watch::Sender<ProcessSnapshot>) {
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

// ── 命令分发 ─────────────────────────────────────────────────

async fn handle(state: &mut State, snap_tx: &watch::Sender<ProcessSnapshot>, cmd: Command) {
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
        Command::AddProcess { name, config, start_now, reply } => {
            let _ = reply.send(do_add(state, name, config, start_now).await);
            broadcast(state, snap_tx);
        }
        Command::UpdateProcess { name, config, new_name, force_restart, reply } => {
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
            let cfg = state.processes.get(&name).map(|p| p.config().clone());
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
    state.processes.get_mut(name).ok_or_else(|| GuguError::ProcessNotFound(name.to_string()))
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
        proc.config().health_check.clone()
            .ok_or_else(|| GuguError::ConfigError("该进程未配置健康检查".into()))?
    };
    let healthy = crate::health::check_health(&hc).await;
    if let Some(proc) = state.processes.get_mut(name) {
        proc.set_healthy(Some(healthy));
    }
    Ok(healthy)
}

async fn do_add(state: &mut State, name: String, config: ProcessConfig, start_now: bool) -> Result<()> {
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

async fn do_update(state: &mut State, name: &str, config: ProcessConfig, new_name: Option<String>, force_restart: bool) -> Result<()> {
    if !state.processes.contains_key(name) {
        return Err(GuguError::ProcessNotFound(name.to_string()));
    }
    config.validate()?;

    let was_running = state.processes[name].is_running();
    let needs_restart = force_restart || !state.processes[name].config().runtime_fields_eq(&config);

    if needs_restart && was_running {
        // SAFETY: 上方已验证 name 存在于 processes
        state.processes.get_mut(name).expect("已验证存在").stop().await?;
    }
    // SAFETY: 同上
    *state.processes.get_mut(name).expect("已验证存在").config_mut() = config;

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
    let proc = state.processes.get(name)
        .ok_or_else(|| GuguError::ProcessNotFound(name.to_string()))?;
    Ok(proc.logs(lines).await)
}

fn do_subscribe(state: &State, name: &str) -> Result<broadcast::Receiver<LogEntry>> {
    let proc = state.processes.get(name)
        .ok_or_else(|| GuguError::ProcessNotFound(name.to_string()))?;
    Ok(proc.subscribe_logs())
}

async fn do_reload(state: &mut State, new_config: &AppConfig) -> Result<()> {
    state.daemon_config = new_config.daemon.clone();

    let config_dir = state.config_path.as_deref()
        .and_then(|p| p.parent())
        .unwrap_or(std::path::Path::new("."));
    let log_base = state.daemon_config.log_dir.as_ref()
        .map(|d| crate::config::resolve_relative_path(d, config_dir))
        .unwrap_or_else(|| config_dir.to_path_buf());

    let to_remove: Vec<String> = state.processes.keys()
        .filter(|k| !new_config.processes.contains_key(*k)).cloned().collect();
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
        let resolved = State::resolve_paths(config, config_dir, &log_base);
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
    let path = state.config_path.clone()
        .ok_or_else(|| GuguError::ConfigError("未配置配置文件路径".into()))?;
    let config = AppConfig::load(&path)?;
    do_reload(state, &config).await
}

// ── 监控循环 ─────────────────────────────────────────────────

async fn run_monitor_cycle(state: &mut State) {
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
    for ((name, _, unhealthy_restart), healthy) in to_check.iter().zip(results) {
        if let Some(proc) = state.processes.get_mut(name) {
            proc.set_healthy(Some(healthy));
        }
        if !healthy {
            tracing::warn!("[{}] 健康检查失败", name);
            if *unhealthy_restart {
                to_restart.push(name.clone());
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
