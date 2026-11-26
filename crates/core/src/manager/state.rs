//! Actor 内部状态：进程表、配置基路径与「声明式 ↔ 解析式」路径换算、历史仓库。

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Arc;

use crate::config::{AppConfig, DaemonConfig, ProcessConfig};
use crate::error::Result;
use crate::history::History;
use crate::process::ManagedProcess;

use super::{topological_sort, ProcessSnapshot};

pub(super) struct State {
    pub(super) processes: HashMap<String, ManagedProcess>,
    pub(super) daemon_config: DaemonConfig,
    pub(super) config_path: Option<PathBuf>,
    pub(super) config_dir: PathBuf,
    pub(super) log_base: PathBuf,
    /// cgroup v2 的落地目录（`daemon.cgroup_root` 或系统默认）。
    pub(super) cgroup_base: PathBuf,
    /// 指标与退出日志；与 `SharedManager` 共享同一份。
    pub(super) history: Arc<History>,
}

impl State {
    pub(super) fn new(config: &AppConfig, config_path: Option<PathBuf>) -> Self {
        let (config_dir, log_base) = Self::bases(config, config_path.as_deref());
        let cgroup_base = Self::cgroup_base(config, &config_dir);
        let history = Arc::new(History::new(log_base.join("history")));

        let processes = config
            .processes
            .iter()
            .map(|(name, pc)| {
                let resolved = Self::resolve_paths(pc, &config_dir, &log_base);
                (
                    name.clone(),
                    ManagedProcess::new(name.clone(), resolved)
                        .with_history(Arc::clone(&history))
                        .with_cgroup_base(cgroup_base.clone()),
                )
            })
            .collect();

        Self {
            processes,
            daemon_config: config.daemon.clone(),
            config_path,
            config_dir,
            log_base,
            cgroup_base,
            history,
        }
    }

    /// 硬限的落地目录：`daemon.cgroup_root`（相对路径按配置目录解析）或系统默认。
    pub(super) fn cgroup_base(config: &AppConfig, config_dir: &std::path::Path) -> PathBuf {
        config
            .daemon
            .cgroup_root
            .as_ref()
            .map(|p| crate::config::resolve_relative_path(p, config_dir))
            .unwrap_or_else(|| PathBuf::from(crate::cgroup::DEFAULT_BASE))
    }

    pub(super) fn bases(
        config: &AppConfig,
        config_path: Option<&std::path::Path>,
    ) -> (PathBuf, PathBuf) {
        let config_dir = config_path
            .and_then(|p| p.parent())
            .map(std::path::Path::to_path_buf)
            .unwrap_or_else(|| PathBuf::from("."));
        let log_base = config
            .daemon
            .log_dir
            .as_ref()
            .map(|d| crate::config::resolve_relative_path(d, &config_dir))
            .unwrap_or_else(|| config_dir.clone());
        (config_dir, log_base)
    }

    /// 回写前把内存中已解析的绝对路径还原为配置相对形式 (绿色可搬移)。
    pub(super) fn declared_paths(&self, c: &ProcessConfig) -> ProcessConfig {
        let mut r = c.clone();
        if let Some(d) = &r.working_dir {
            r.working_dir = Some(crate::config::relativize_path(d, &self.config_dir));
        }
        if let Some(l) = &r.stdout_log {
            r.stdout_log = Some(crate::config::relativize_path(l, &self.log_base));
        }
        if let Some(l) = &r.stderr_log {
            r.stderr_log = Some(crate::config::relativize_path(l, &self.log_base));
        }
        r
    }

    pub(super) fn resolve_paths(
        c: &ProcessConfig,
        config_dir: &std::path::Path,
        log_base: &std::path::Path,
    ) -> ProcessConfig {
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

    pub(super) fn snapshot(&self) -> ProcessSnapshot {
        ProcessSnapshot {
            processes: self.processes.values().map(ManagedProcess::info).collect(),
        }
    }

    pub(super) fn save_config(&self) -> Result<()> {
        let Some(ref path) = self.config_path else {
            return Ok(());
        };
        let config = AppConfig {
            daemon: self.daemon_config.clone(),
            processes: self
                .processes
                .iter()
                .map(|(n, p)| (n.clone(), self.declared_paths(p.config())))
                .collect(),
        };
        config.save(path)?;
        tracing::debug!("配置已保存到 {}", path.display());
        Ok(())
    }

    pub(super) async fn start_all(&mut self) {
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
            let (auto_start, depends_on, ready_timeout) = match self.processes.get(&name) {
                Some(proc) => (
                    proc.config().auto_start,
                    proc.config().depends_on.clone(),
                    std::time::Duration::from_secs(proc.config().ready_timeout_secs),
                ),
                None => continue,
            };
            if !auto_start {
                continue;
            }

            // 依赖必须真正就绪才拉起本进程，否则失败的下游会让上游"看起来启动成功"
            let mut blocked_by = None;
            for dep in depends_on {
                if !self.processes.contains_key(&dep) {
                    continue;
                }
                if !super::ready::wait_dependency(self, &dep, ready_timeout).await {
                    blocked_by = Some(dep);
                    break;
                }
            }
            if let Some(dep) = blocked_by {
                tracing::error!(
                    "[{name}] 依赖 [{dep}] 在 {ready_timeout:?} 内未就绪，本进程不启动"
                );
                super::monitor::notify_dependency_not_ready(self, &name, &dep);
                continue;
            }

            if let Some(proc) = self.processes.get_mut(&name) {
                if let Err(e) = proc.start().await {
                    tracing::error!("自动启动进程 '{}' 失败: {}", name, e);
                }
            }
        }
    }

    pub(super) async fn stop_all(&mut self) {
        let configs: HashMap<String, ProcessConfig> = self
            .processes
            .iter()
            .map(|(n, p)| (n.clone(), p.config().clone()))
            .collect();

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
