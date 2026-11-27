use std::path::PathBuf;
use std::sync::Arc;

use gugu_core::audit::AuditStore;
use gugu_core::config::NodeConfig;
use gugu_core::manager::SharedManager;
use gugu_core::update::UpdateService;
use tokio::sync::mpsc;

use crate::metrics::Metrics;
use crate::nodes::NodeRegistry;

/// Web 服务的全部运行时状态：启动参数在这里一次成型，处理函数只读。
#[derive(Clone)]
pub struct AppState {
    pub manager: SharedManager,
    pub metrics: Arc<Metrics>,
    pub audit: Arc<AuditStore>,
    /// 节点表：本机身份 + 从节点与探测结果。
    pub nodes: Arc<NodeRegistry>,
    pub api_key: Option<String>,
    pub cors_origins: Vec<String>,
    /// 是否开放 WebUI 内置终端。
    pub terminal_enabled: bool,
    /// 终端会话的工作目录（通常为配置文件所在目录）。
    pub terminal_cwd: Option<PathBuf>,
    /// 文件浏览允许的根目录；空表示不限制。
    pub fs_roots: Vec<PathBuf>,
    /// `POST /api/v1/shutdown` 用以请求守护进程优雅停止；
    /// 由 CLI 的 run_core 注入，测试或不接 API 的形态下为 None。
    pub shutdown_tx: Option<mpsc::Sender<()>>,
    /// 自更新服务；不接时更新接口返回未启用。
    pub updates: Option<Arc<UpdateService>>,
}

impl AppState {
    /// 最小可用状态：只有本机节点表、内存审计、无认证、无跨域限制。
    pub fn new(manager: SharedManager) -> Self {
        Self {
            manager,
            metrics: Arc::new(Metrics::new()),
            audit: Arc::new(AuditStore::disabled()),
            nodes: Arc::new(NodeRegistry::local_only()),
            api_key: None,
            cors_origins: Vec::new(),
            terminal_enabled: false,
            terminal_cwd: None,
            fs_roots: Vec::new(),
            shutdown_tx: None,
            updates: None,
        }
    }

    /// 设置 API Key；None 表示不认证。
    #[must_use]
    pub fn with_api_key(mut self, api_key: Option<String>) -> Self {
        self.api_key = api_key;
        self
    }

    /// 设置跨域白名单；空表示允许任意来源。
    #[must_use]
    pub fn with_cors_origins(mut self, origins: Vec<String>) -> Self {
        self.cors_origins = origins;
        self
    }

    /// 按配置构建节点表；配置有误时返回拼好的问题清单，不进入运行态。
    ///
    /// # Errors
    /// 节点重名、地址不合法、对外地址缺 Key 时返回原因。
    pub fn with_nodes(
        mut self,
        configs: &[NodeConfig],
        node_name: Option<String>,
    ) -> Result<Self, String> {
        let problems = gugu_core::config::validate_nodes(configs);
        if !problems.is_empty() {
            return Err(problems.join("；"));
        }
        self.nodes = Arc::new(NodeRegistry::new(configs, node_name));
        Ok(self)
    }

    /// 直接注入已构建的节点表，用于测试与自定义探测配置。
    #[must_use]
    pub fn with_registry(mut self, nodes: Arc<NodeRegistry>) -> Self {
        self.nodes = nodes;
        self
    }

    /// 注入审计仓库；不接时审计只留内存。
    #[must_use]
    pub fn with_audit(mut self, audit: Arc<AuditStore>) -> Self {
        self.audit = audit;
        self
    }

    /// 按目录构建落盘审计；None 时审计只留内存。
    #[must_use]
    pub fn with_audit_dir(mut self, dir: Option<PathBuf>) -> Self {
        self.audit = Arc::new(dir.map_or_else(AuditStore::disabled, AuditStore::new));
        self
    }

    /// 注入终端开关与工作目录；不调用时终端关闭。
    #[must_use]
    pub fn with_terminal(mut self, enabled: bool, cwd: Option<PathBuf>) -> Self {
        self.terminal_enabled = enabled;
        self.terminal_cwd = cwd;
        self
    }

    /// 注入文件浏览的根目录白名单；不调用时不限制浏览范围。
    #[must_use]
    pub fn with_fs_roots(mut self, roots: Vec<PathBuf>) -> Self {
        self.fs_roots = roots;
        self
    }

    /// 注入守护进程停止信号的发送端。
    #[must_use]
    pub fn with_shutdown(mut self, shutdown_tx: Option<mpsc::Sender<()>>) -> Self {
        self.shutdown_tx = shutdown_tx;
        self
    }

    /// 注入自更新服务；不调用时更新接口不可用。
    #[must_use]
    pub fn with_updates(mut self, updates: Option<Arc<UpdateService>>) -> Self {
        self.updates = updates;
        self
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use gugu_core::config::AppConfig;

    fn node(name: &str, url: &str, key: Option<&str>) -> NodeConfig {
        NodeConfig {
            name: name.to_string(),
            label: None,
            url: url.to_string(),
            api_key: key.map(str::to_string),
        }
    }

    #[tokio::test]
    async fn with_nodes_rejects_a_duplicated_name() {
        let configs = [
            node("web1", "http://127.0.0.1:9091", None),
            node("web1", "http://127.0.0.1:9092", None),
        ];
        let err = AppState::new(gugu_core::manager::start(&AppConfig::default(), None))
            .with_nodes(&configs, None)
            .err()
            .expect("重名应被拒绝");
        assert!(err.contains("web1"), "问题清单应点名冲突的节点: {err}");
    }

    #[tokio::test]
    async fn with_nodes_builds_a_normalized_registry() {
        let mut config = node("web1", "  http://127.0.0.1:9091/  ", Some("  "));
        config.label = Some("   ".to_string());
        let state = AppState::new(gugu_core::manager::start(&AppConfig::default(), None))
            .with_nodes(&[config], Some("中心机".to_string()))
            .expect("配置有效");
        assert_eq!(state.nodes.local_name(), "中心机");
        let remote = state.nodes.remote("web1").expect("web1 应在表里");
        assert_eq!(remote.base(), "http://127.0.0.1:9091");
        // 空白标签与空白 Key 在构造时丢弃
        assert_eq!(remote.display_name(), "web1");
        assert_eq!(remote.api_key(), None);
    }
}
