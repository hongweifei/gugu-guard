use std::path::PathBuf;
use std::sync::Arc;

use gugu_core::audit::AuditStore;
use gugu_core::manager::SharedManager;
use tokio::sync::mpsc;

use crate::metrics::Metrics;

#[derive(Clone)]
pub struct AppState {
    pub manager: SharedManager,
    pub metrics: Arc<Metrics>,
    pub audit: Arc<AuditStore>,
    pub api_key: Option<String>,
    pub cors_origins: Vec<String>,
    /// 是否开放 WebUI 内置终端。
    pub terminal_enabled: bool,
    /// 终端会话的工作目录（通常为配置文件所在目录）。
    pub terminal_cwd: Option<PathBuf>,
    /// `POST /api/v1/shutdown` 用以请求守护进程优雅停止；
    /// 由 CLI 的 run_core 注入，测试或不接 API 的形态下为 None。
    pub shutdown_tx: Option<mpsc::Sender<()>>,
}

impl AppState {
    pub fn new(manager: SharedManager, api_key: Option<String>, cors_origins: Vec<String>) -> Self {
        Self {
            manager,
            metrics: Arc::new(Metrics::new()),
            audit: Arc::new(AuditStore::disabled()),
            api_key,
            cors_origins,
            terminal_enabled: false,
            terminal_cwd: None,
            shutdown_tx: None,
        }
    }

    /// 注入审计仓库；不接时审计只留内存。
    #[must_use]
    pub fn with_audit(mut self, audit: Arc<AuditStore>) -> Self {
        self.audit = audit;
        self
    }

    /// 注入终端开关与工作目录；不调用时终端关闭。
    #[must_use]
    pub fn with_terminal(mut self, enabled: bool, cwd: Option<PathBuf>) -> Self {
        self.terminal_enabled = enabled;
        self.terminal_cwd = cwd;
        self
    }

    /// 注入守护进程停止信号的发送端。
    #[must_use]
    pub fn with_shutdown(mut self, shutdown_tx: Option<mpsc::Sender<()>>) -> Self {
        self.shutdown_tx = shutdown_tx;
        self
    }
}
