//! 自更新配置：来源、频率与是否自动安装。
//!
//! 更新只作用于本机正在运行的这个二进制；`auto` 决定发现新版本之后做什么，
//! `restart` 决定替换完成后是否由替身进程重新拉起守护进程。

use serde::{Deserialize, Serialize};

use super::process::default_true;

/// 发布信息的来源。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "lowercase")]
pub enum UpdateProvider {
    /// GitHub Releases（默认，发布资产齐全）
    #[default]
    Github,
    /// Gitee Releases
    Gitee,
    /// 自建清单：直接给出一个 JSON 的地址
    Manifest,
}

/// 发现新版本后的动作。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "lowercase")]
pub enum AutoUpdate {
    /// 只保留手动检查
    #[default]
    Off,
    /// 按间隔自动检查，发现新版本只提示
    Check,
    /// 按间隔自动检查并直接安装（会重启守护进程）
    Install,
}

impl AutoUpdate {
    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Off => "off",
            Self::Check => "check",
            Self::Install => "install",
        }
    }

    #[must_use]
    pub fn parse(text: &str) -> Option<Self> {
        match text.trim().to_ascii_lowercase().as_str() {
            "off" => Some(Self::Off),
            "check" => Some(Self::Check),
            "install" => Some(Self::Install),
            _ => None,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct UpdateConfig {
    /// 是否允许检查与安装；关闭后接口直接拒绝，不发起任何外部请求。
    #[serde(default = "default_true")]
    pub enabled: bool,
    #[serde(default)]
    pub provider: UpdateProvider,
    /// 仓库标识 `owner/name`。
    #[serde(default = "default_repo")]
    pub repo: String,
    /// `provider = "manifest"` 时的清单地址。
    #[serde(default)]
    pub manifest_url: Option<String>,
    /// 下载镜像前缀：`https://mirror/` 或含 `{}` 的模板，只作用于资产地址。
    #[serde(default)]
    pub mirror: Option<String>,
    #[serde(default)]
    pub auto: AutoUpdate,
    /// 自动检查的间隔小时数。
    #[serde(default = "default_check_hours")]
    pub check_hours: u64,
    /// 替换完成后是否自动拉起守护进程。
    #[serde(default = "default_true")]
    pub restart: bool,
    /// 是否保留被替换下来的旧二进制（`gugu.exe.bak`）。
    #[serde(default = "default_true")]
    pub keep_backup: bool,
    /// 清单与下载请求的超时秒数。
    #[serde(default = "default_timeout_secs")]
    pub timeout_secs: u64,
}

fn default_repo() -> String {
    "hongweifei/gugu-guard".to_string()
}

fn default_check_hours() -> u64 {
    24
}

fn default_timeout_secs() -> u64 {
    30
}

impl Default for UpdateConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            provider: UpdateProvider::Github,
            repo: default_repo(),
            manifest_url: None,
            mirror: None,
            auto: AutoUpdate::Off,
            check_hours: default_check_hours(),
            restart: true,
            keep_backup: true,
            timeout_secs: default_timeout_secs(),
        }
    }
}

impl UpdateConfig {
    /// 校验来源与频率；配置有误时更新接口在启动阶段就明确失败。
    ///
    /// # Errors
    /// 仓库标识、清单地址、镜像前缀、间隔或超时越界时返回原因。
    pub fn validate(&self) -> crate::error::Result<()> {
        let repo = self.repo.trim();
        let mut parts = repo.split('/');
        let owner = parts.next().unwrap_or_default();
        let name = parts.next().unwrap_or_default();
        if owner.is_empty() || name.is_empty() || parts.next().is_some() {
            return Err(crate::error::GuguError::ConfigError(format!(
                "daemon.update.repo 应写成 owner/name，当前是 {repo:?}"
            )));
        }
        if self.provider == UpdateProvider::Manifest {
            let url = self.manifest_url.as_deref().unwrap_or_default().trim();
            if !is_http(url) {
                return Err(crate::error::GuguError::ConfigError(
                    "daemon.update.manifest_url 必须是以 http:// 或 https:// 开头的地址"
                        .to_string(),
                ));
            }
        }
        if let Some(mirror) = self.mirror.as_deref().map(str::trim) {
            if !mirror.is_empty() && !is_http(mirror) {
                return Err(crate::error::GuguError::ConfigError(
                    "daemon.update.mirror 必须是以 http:// 或 https:// 开头的前缀".to_string(),
                ));
            }
        }
        if self.check_hours == 0 {
            return Err(crate::error::GuguError::ConfigError(
                "daemon.update.check_hours 至少为 1".to_string(),
            ));
        }
        if !(5..=600).contains(&self.timeout_secs) {
            return Err(crate::error::GuguError::ConfigError(
                "daemon.update.timeout_secs 应在 5 到 600 之间".to_string(),
            ));
        }
        Ok(())
    }
}

fn is_http(url: &str) -> bool {
    url.starts_with("https://") || url.starts_with("http://")
}

/// 只把 `auto` 一项写回配置文件：保留注释、顺序与其余条目。
///
/// # Errors
/// 配置文件无法解析或写入时返回原因。
pub fn save_auto(path: &std::path::Path, auto: AutoUpdate) -> crate::error::Result<()> {
    use toml_edit::{value, DocumentMut, Item, Table};

    let text = std::fs::read_to_string(path).unwrap_or_default();
    let mut doc: DocumentMut = if text.trim().is_empty() {
        DocumentMut::new()
    } else {
        text.parse::<DocumentMut>()
            .map_err(|e| crate::error::GuguError::ConfigError(format!("解析配置文件失败: {e}")))?
    };

    if doc.get("daemon").is_none() {
        doc.insert("daemon", Item::Table(Table::new()));
    }
    let daemon = doc["daemon"].as_table_mut().ok_or_else(|| {
        crate::error::GuguError::ConfigError("daemon 不是表，无法写入更新设置".to_string())
    })?;
    if daemon.get("update").is_none() {
        daemon.insert("update", Item::Table(Table::new()));
    }
    let update = daemon["update"].as_table_mut().ok_or_else(|| {
        crate::error::GuguError::ConfigError("daemon.update 不是表，无法写入更新设置".to_string())
    })?;
    update.insert("auto", value(auto.as_str()));

    std::fs::write(path, doc.to_string())
        .map_err(|e| crate::error::GuguError::ConfigError(format!("写入配置文件失败: {e}")))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn defaults_are_usable() {
        let config = UpdateConfig::default();
        assert!(config.enabled);
        assert_eq!(config.auto, AutoUpdate::Off);
        assert_eq!(config.check_hours, 24);
        assert!(config.restart);
        config.validate().expect("默认配置有效");
    }

    #[test]
    fn rejects_bad_repo_and_manifest() {
        let config = UpdateConfig {
            repo: "no-slash".to_string(),
            ..Default::default()
        };
        assert!(config.validate().is_err());

        let mut config = UpdateConfig {
            provider: UpdateProvider::Manifest,
            ..Default::default()
        };
        assert!(config.validate().is_err(), "manifest 缺地址应失败");
        config.manifest_url = Some("ftp://example/x.json".to_string());
        assert!(config.validate().is_err(), "非 http 地址应失败");
        config.manifest_url = Some("https://example/x.json".to_string());
        config.validate().expect("地址补齐后有效");
    }

    #[test]
    fn rejects_out_of_range_intervals() {
        let mut config = UpdateConfig {
            check_hours: 0,
            ..Default::default()
        };
        assert!(config.validate().is_err());
        config.check_hours = 1;
        config.timeout_secs = 1;
        assert!(config.validate().is_err());
        config.timeout_secs = 600;
        config.validate().expect("边界值有效");
    }

    #[test]
    fn writes_only_the_auto_key() {
        let dir = std::env::temp_dir().join(format!("gugu-update-config-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).expect("建目录");
        let path = dir.join("gugu.toml");
        std::fs::write(
            &path,
            "# 顶部说明\n[daemon]\nnode_name = \"中心机\"\n\n[processes.web]\ncommand = \"nginx\"\n",
        )
        .expect("写配置");

        save_auto(&path, AutoUpdate::Install).expect("写回成功");
        let text = std::fs::read_to_string(&path).expect("读回");
        assert!(text.contains("# 顶部说明"), "注释应保留: {text}");
        assert!(
            text.contains("node_name = \"中心机\""),
            "其它设置应保留: {text}"
        );
        assert!(text.contains("auto = \"install\""), "新设置应写入: {text}");
        assert!(text.contains("[processes.web]"), "进程段应保留: {text}");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn parses_auto_names() {
        assert_eq!(AutoUpdate::parse("check"), Some(AutoUpdate::Check));
        assert_eq!(AutoUpdate::parse(" INSTALL "), Some(AutoUpdate::Install));
        assert_eq!(AutoUpdate::parse("sometimes"), None);
    }
}
