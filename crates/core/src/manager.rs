use crate::config::{AppConfig, DaemonConfig, HealthCheckConfig, ProcessConfig};
use crate::error::{GuguError, Result};
use crate::health;
use crate::process::{ManagedProcess, ProcessInfo, LogEntry, ProcessStatus};
use std::collections::{HashMap, VecDeque};
use std::path::PathBuf;
use std::sync::Arc;
use tokio::sync::RwLock;

pub type SharedManager = Arc<RwLock<ProcessManager>>;

pub struct ProcessManager {
    processes: HashMap<String, ManagedProcess>,
    daemon_config: DaemonConfig,
    config_path: Option<PathBuf>,
}

pub(crate) fn topological_sort(processes: &HashMap<String, ProcessConfig>) -> Result<Vec<String>> {
    let mut in_degree: HashMap<&str, u32> = HashMap::new();
    let mut dependents: HashMap<&str, Vec<&str>> = HashMap::new();

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
            *in_degree.entry(name.as_str()).or_insert(0) += 1;
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
        let remaining: Vec<&str> = processes
            .keys()
            .filter(|k| !result.contains(&k.to_string()))
            .map(|k| k.as_str())
            .collect();
        return Err(GuguError::CyclicDependency(
            format!("检测到循环依赖: {}", remaining.join(", "))
        ));
    }

    Ok(result)
}

impl ProcessManager {
    pub fn new(config: &AppConfig, config_path: Option<PathBuf>) -> Self {
        let config_dir = config_path.as_deref()
            .and_then(|p| p.parent())
            .unwrap_or(std::path::Path::new("."));

        let log_base = config.daemon.log_dir.as_ref()
            .map(|d| crate::config::resolve_relative_path(d, config_dir))
            .unwrap_or_else(|| config_dir.to_path_buf());

        let processes = config
            .processes
            .iter()
            .map(|(name, proc_config)| {
                let resolved = Self::resolve_process_paths(proc_config, config_dir, &log_base);
                (name.clone(), ManagedProcess::new(name.clone(), resolved))
            })
            .collect();

        Self {
            processes,
            daemon_config: config.daemon.clone(),
            config_path,
        }
    }

    fn resolve_process_paths(
        config: &crate::config::ProcessConfig,
        config_dir: &std::path::Path,
        log_base: &std::path::Path,
    ) -> crate::config::ProcessConfig {
        let mut c = config.clone();
        if let Some(ref dir) = c.working_dir {
            c.working_dir = Some(crate::config::resolve_relative_path(dir, config_dir));
        }
        if let Some(ref p) = c.stdout_log {
            c.stdout_log = Some(crate::config::resolve_relative_path(p, log_base));
        }
        if let Some(ref p) = c.stderr_log {
            c.stderr_log = Some(crate::config::resolve_relative_path(p, log_base));
        }
        c
    }

    pub fn shared(self) -> SharedManager {
        Arc::new(RwLock::new(self))
    }

    pub async fn start_all(&mut self) {
        let configs: HashMap<String, ProcessConfig> = self
            .processes
            .iter()
            .map(|(n, p)| (n.clone(), p.config().clone()))
            .collect();

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

    pub async fn stop_all(&mut self) {
        let names: Vec<String> = self.processes.keys().cloned().collect();
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

    pub async fn start_process(&mut self, name: &str) -> Result<()> {
        let proc = self
            .processes
            .get_mut(name)
            .ok_or_else(|| GuguError::ProcessNotFound(name.to_string()))?;
        proc.reset_crash_restart_count();
        proc.start().await
    }

    pub async fn stop_process(&mut self, name: &str) -> Result<()> {
        let proc = self
            .processes
            .get_mut(name)
            .ok_or_else(|| GuguError::ProcessNotFound(name.to_string()))?;
        proc.stop().await
    }

    pub async fn restart_process(&mut self, name: &str) -> Result<()> {
        let proc = self
            .processes
            .get_mut(name)
            .ok_or_else(|| GuguError::ProcessNotFound(name.to_string()))?;
        proc.restart().await
    }

    pub async fn check_process_health(&mut self, name: &str) -> Result<bool> {
        let hc = {
            let proc = self
                .processes
                .get(name)
                .ok_or_else(|| GuguError::ProcessNotFound(name.to_string()))?;
            proc.config()
                .health_check
                .clone()
                .ok_or_else(|| GuguError::ConfigError("该进程未配置健康检查".into()))?
        };
        let healthy = health::check_health(&hc).await;
        if let Some(proc) = self.processes.get_mut(name) {
            proc.set_healthy(Some(healthy));
        }
        Ok(healthy)
    }

    pub async fn add_process(&mut self, name: String, config: ProcessConfig, start_now: bool) -> Result<()> {
        if self.processes.contains_key(&name) {
            return Err(GuguError::ConfigError(format!("进程 '{name}' 已存在")));
        }
        config.validate()?;
        let mut proc = ManagedProcess::new(name.clone(), config.clone());
        if start_now {
            proc.start().await?;
        }
        self.processes.insert(name.clone(), proc);
        tracing::info!("[{}] 进程配置已添加", name);
        self.save_config()?;
        Ok(())
    }

    pub async fn update_process(&mut self, name: &str, config: ProcessConfig, new_name: Option<String>, force_restart: bool) -> Result<()> {
        if !self.processes.contains_key(name) {
            return Err(GuguError::ProcessNotFound(name.to_string()));
        }

        config.validate()?;

        // 先读取需要的信息，避免多次查找
        let was_running = self.processes.get(name).expect("checked above").is_running();
        let needs_restart =
            force_restart || !self.processes.get(name).expect("checked above").config().runtime_fields_eq(&config);

        if needs_restart && was_running {
            self.processes.get_mut(name).expect("checked above").stop().await?;
        }

        *self.processes.get_mut(name).expect("checked above").config_mut() = config;

        if let Some(ref new) = new_name {
            if new != name {
                if self.processes.contains_key(new) {
                    return Err(GuguError::ConfigError(format!("进程 '{new}' 已存在")));
                }
                let mut proc = self.processes.remove(name).expect("checked above");
                proc.rename(new.clone());
                self.processes.insert(new.clone(), proc);
                tracing::info!("[{}] 进程已改名为 [{}]", name, new);
            }
        }

        let target = new_name.as_deref().unwrap_or(name);
        if needs_restart && was_running {
            if let Some(proc) = self.processes.get_mut(target) {
                let _ = proc.start().await;
            }
        }

        tracing::info!("[{}] 进程配置已更新", target);
        self.save_config()?;
        Ok(())
    }

    pub async fn remove_process(&mut self, name: &str) -> Result<()> {
        if let Some(proc) = self.processes.get_mut(name) {
            if proc.is_running() {
                proc.stop().await?;
            }
        } else {
            return Err(GuguError::ProcessNotFound(name.to_string()));
        }
        self.processes.remove(name);
        tracing::info!("[{}] 进程已移除", name);
        self.save_config()?;
        Ok(())
    }

    pub fn list_processes(&self) -> Vec<ProcessInfo> {
        self.processes.values().map(|p| p.info()).collect()
    }

    pub fn get_process_info(&self, name: &str) -> Option<ProcessInfo> {
        self.processes.get(name).map(|p| p.info())
    }

    pub fn get_process_config(&self, name: &str) -> Option<&ProcessConfig> {
        self.processes.get(name).map(|p| p.config())
    }

    pub async fn get_process_logs(&self, name: &str, lines: usize) -> Result<Vec<LogEntry>> {
        let proc = self
            .processes
            .get(name)
            .ok_or_else(|| GuguError::ProcessNotFound(name.to_string()))?;
        Ok(proc.logs(lines).await)
    }

    pub async fn clear_process_logs(&self, name: &str) -> Result<()> {
        let proc = self
            .processes
            .get(name)
            .ok_or_else(|| GuguError::ProcessNotFound(name.to_string()))?;
        proc.clear_logs().await;
        Ok(())
    }

    pub fn subscribe_process_logs(&self, name: &str) -> Result<tokio::sync::broadcast::Receiver<LogEntry>> {
        let proc = self
            .processes
            .get(name)
            .ok_or_else(|| GuguError::ProcessNotFound(name.to_string()))?;
        Ok(proc.subscribe_logs())
    }

    pub fn all_process_names(&self) -> Vec<String> {
        self.processes.keys().cloned().collect()
    }

    pub fn find_dead_processes(&mut self) -> Vec<(String, std::time::Duration)> {
        let mut dead = Vec::new();
        for (name, proc) in &mut self.processes {
            if proc.is_running() && !proc.check_alive()
                && proc.should_auto_restart()
            {
                proc.set_status(ProcessStatus::Restarting);
                proc.mark_crash_restart();
                dead.push((name.clone(), proc.restart_delay()));
            }
        }
        dead
    }

    pub async fn run_due_health_checks(&mut self) {
        let now = std::time::Instant::now();

        let to_check: Vec<(String, HealthCheckConfig, bool)> = {
            let mut checks = Vec::new();
            for (name, proc) in &mut self.processes {
                if !proc.is_running() {
                    continue;
                }
                let (hc, should_check, ur) = {
                    let cfg = proc.config();
                    match &cfg.health_check {
                        Some(hc) => {
                            let interval =
                                std::time::Duration::from_secs(hc.interval_secs);
                            let should = proc
                                .last_health_check()
                                .is_none_or(|last| now.duration_since(last) >= interval);
                            (hc.clone(), should, cfg.unhealthy_restart)
                        }
                        None => continue,
                    }
                };
                if should_check {
                    proc.set_last_health_check(Some(now));
                    checks.push((name.clone(), hc, ur));
                }
            }
            checks
        };

        let mut to_restart: Vec<String> = Vec::new();
        // 并发执行所有健康检查
        let check_futures: Vec<_> = to_check
            .iter()
            .map(|(_, hc, _)| health::check_health(hc))
            .collect();
        let health_results = futures::future::join_all(check_futures).await;

        for ((name, _, unhealthy_restart), healthy) in to_check.iter().zip(health_results) {
            if let Some(proc) = self.processes.get_mut(name) {
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
            if let Some(proc) = self.processes.get_mut(&name) {
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

    pub async fn reload_config(&mut self, new_config: &AppConfig) -> Result<()> {
        self.daemon_config = new_config.daemon.clone();

        let to_remove: Vec<String> = self
            .processes
            .keys()
            .filter(|k| !new_config.processes.contains_key(*k))
            .cloned()
            .collect();
        for name in &to_remove {
            if let Some(proc) = self.processes.get_mut(name) {
                if proc.is_running() {
                    let _ = proc.stop().await;
                }
            }
            self.processes.remove(name);
            tracing::info!("[{}] 进程已移除 (配置重载)", name);
        }

        for (name, config) in &new_config.processes {
            if let Err(e) = config.validate() {
                tracing::warn!("[{}] 配置验证失败，跳过: {}", name, e);
                continue;
            }
            if let Some(proc) = self.processes.get_mut(name) {
                let was_running = proc.is_running();
                let needs_restart = was_running && !proc.config().runtime_fields_eq(config);
                *proc.config_mut() = config.clone();
                if needs_restart {
                    let _ = proc.stop().await;
                    let _ = proc.start().await;
                    tracing::info!("[{}] 配置已更新并重启 (配置重载)", name);
                } else {
                    tracing::info!("[{}] 配置已更新 (配置重载)", name);
                }
            } else {
                let mut proc = ManagedProcess::new(name.clone(), config.clone());
                if config.auto_start {
                    let _ = proc.start().await;
                }
                self.processes.insert(name.clone(), proc);
                tracing::info!("[{}] 新进程已添加 (配置重载)", name);
            }
        }

        Ok(())
    }

    pub async fn reload_from_file(&mut self) -> Result<()> {
        let path = match &self.config_path {
            Some(p) => p.clone(),
            None => return Err(GuguError::ConfigError("未配置配置文件路径".into())),
        };
        let config = AppConfig::load(&path)?;
        self.reload_config(&config).await
    }

    fn save_config(&self) -> Result<()> {
        if let Some(ref path) = self.config_path {
            let config = AppConfig {
                daemon: self.daemon_config.clone(),
                processes: self
                    .processes
                    .iter()
                    .map(|(name, proc)| (name.clone(), proc.config().clone()))
                    .collect(),
            };
            config.save(path)?;
            tracing::debug!("配置已保存到 {}", path.display());
        }
        Ok(())
    }
}

pub async fn start_monitor(manager: SharedManager) {
    let mut interval = tokio::time::interval(std::time::Duration::from_secs(2));
    loop {
        interval.tick().await;

        // 检测已退出进程 — 持锁时间短，只修改状态
        let dead = {
            let mut mgr = manager.write().await;
            mgr.find_dead_processes()
        };

        // 每个进程独立获取写锁来重启，不阻塞其他操作
        for (name, delay) in dead {
            let mgr_clone = manager.clone();
            tokio::spawn(async move {
                tracing::info!("[{}] 准备自动重启，等待 {:?}", name, delay);
                tokio::time::sleep(delay).await;
                let mut mgr = mgr_clone.write().await;
                if let Some(proc) = mgr.processes.get_mut(&name) {
                    if !proc.is_running() {
                        if let Err(e) = proc.start().await {
                            tracing::error!("[{}] 自动重启失败: {}", name, e);
                        }
                    }
                }
            });
        }

        // 健康检查 — 尽量缩短持锁时间
        {
            let mut mgr = manager.write().await;
            mgr.run_due_health_checks().await;
        }
    }
}
