use crate::config::{AppConfig, DaemonConfig, ProcessConfig};
use crate::error::{GuguError, Result};
use crate::health;
use crate::process::{ManagedProcess, ProcessInfo, LogEntry};
use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Arc;
use tokio::sync::RwLock;

pub type SharedManager = Arc<RwLock<ProcessManager>>;

pub struct ProcessManager {
    processes: HashMap<String, ManagedProcess>,
    daemon_config: DaemonConfig,
    config_path: Option<PathBuf>,
}

impl ProcessManager {
    pub fn new(config: &AppConfig, config_path: Option<PathBuf>) -> Self {
        let processes = config
            .processes
            .iter()
            .map(|(name, proc_config)| {
                (name.clone(), ManagedProcess::new(name.clone(), proc_config.clone()))
            })
            .collect();

        Self {
            processes,
            daemon_config: config.daemon.clone(),
            config_path,
        }
    }

    pub fn shared(self) -> SharedManager {
        Arc::new(RwLock::new(self))
    }

    pub async fn start_all(&mut self) {
        let mut names = Vec::new();
        for (name, proc) in &self.processes {
            if proc.config().auto_start {
                names.push(name.clone());
            }
        }
        for name in names {
            if let Some(proc) = self.processes.get_mut(&name) {
                if let Err(e) = proc.start().await {
                    tracing::error!("自动启动进程 '{}' 失败: {}", name, e);
                }
            }
        }
    }

    pub async fn stop_all(&mut self) {
        let names: Vec<String> = self.processes.keys().cloned().collect();
        for name in names {
            if let Some(proc) = self.processes.get_mut(&name) {
                if proc.is_running() {
                    if let Err(e) = proc.stop().await {
                        tracing::error!("停止进程 '{}' 失败: {}", name, e);
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

    pub async fn add_process(&mut self, name: String, config: ProcessConfig, start_now: bool) -> Result<()> {
        if self.processes.contains_key(&name) {
            return Err(GuguError::ConfigError(format!("进程 '{name}' 已存在")));
        }
        let mut proc = ManagedProcess::new(name.clone(), config.clone());
        if start_now {
            proc.start().await?;
        }
        self.processes.insert(name.clone(), proc);
        tracing::info!("[{}] 进程配置已添加", name);
        self.save_config()?;
        Ok(())
    }

    pub async fn update_process(&mut self, name: &str, config: ProcessConfig, new_name: Option<String>, restart: bool) -> Result<()> {
        let was_running = self.processes.get(name).map(|p| p.is_running()).unwrap_or(false);

        if let Some(proc) = self.processes.get_mut(name) {
            if proc.is_running() {
                proc.stop().await?;
            }
        } else {
            return Err(GuguError::ProcessNotFound(name.to_string()));
        }

        if let Some(ref new) = new_name {
            if new != name {
                if self.processes.contains_key(new) {
                    return Err(GuguError::ConfigError(format!("进程 '{new}' 已存在")));
                }
                let mut proc = self.processes.remove(name).unwrap();
                proc.rename(new.clone());
                *proc.config_mut() = config;
                self.processes.insert(new.clone(), proc);
                tracing::info!("[{}] 进程已改名为 [{}]", name, new);
            } else {
                *self.processes.get_mut(name).unwrap().config_mut() = config;
            }
        } else {
            *self.processes.get_mut(name).unwrap().config_mut() = config;
        }

        let target = new_name.as_deref().unwrap_or(name);
        if restart || was_running {
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

    pub fn find_dead_processes(&mut self) -> Vec<(String, std::time::Duration)> {
        let mut dead = Vec::new();
        for (name, proc) in &mut self.processes {
            if proc.is_running() && !proc.check_alive() {
                if proc.should_auto_restart() {
                    dead.push((name.clone(), proc.restart_delay()));
                }
            }
        }
        dead
    }

    pub async fn run_health_checks(&self) {
        for (name, proc) in &self.processes {
            if !proc.is_running() {
                continue;
            }
            if let Some(ref hc) = proc.config().health_check {
                let healthy = health::check_health(hc).await;
                if !healthy {
                    tracing::warn!("[{}] 健康检查失败", name);
                }
            }
        }
    }

    fn save_config(&self) -> Result<()> {
        if let Some(ref path) = self.config_path {
            let config = AppConfig {
                daemon: self.daemon_config.clone(),
                processes: self.processes
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

        let dead = {
            let mut mgr = manager.write().await;
            mgr.find_dead_processes()
        };

        for (name, delay) in dead {
            tracing::info!("[{}] 准备自动重启，等待 {:?}", name, delay);
            tokio::time::sleep(delay).await;

            let mut mgr = manager.write().await;
            if let Some(proc) = mgr.processes.get_mut(&name) {
                proc.set_status(crate::process::ProcessStatus::Restarting);
                if let Err(e) = proc.start().await {
                    tracing::error!("[{}] 自动重启失败: {}", name, e);
                }
            }
        }

        {
            let mgr = manager.read().await;
            mgr.run_health_checks().await;
        }
    }
}
