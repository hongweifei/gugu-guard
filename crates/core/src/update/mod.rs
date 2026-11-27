//! 自更新：检查发布、下载校验、换入与重启。
//!
//! 分工：`source` 只负责拿到一个归一化的发布版本，`archive` 只负责从发布包里取出
//! 可执行文件，`installer` 只负责校验与换入，本文件把这几步串成一条可观察的流程，
//! 对外提供 [`UpdateService`]：一个查询快照、一个检查方法、一个启动安装的方法。
//!
//! 安装的最后一步必然由替身进程完成：守护进程无法覆盖自己的可执行文件，
//! 因此在替换之前先把新二进制作为替身启动，再由替身等旧进程退出、换入、重启。

pub mod archive;
pub mod installer;
pub mod release;
pub mod source;
pub mod version;

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, RwLock};
use std::time::{Duration, Instant};

use futures::StreamExt;
use serde::Serialize;
use sha2::{Digest, Sha256};
use tokio::io::AsyncWriteExt;
use tokio::sync::mpsc;

use crate::config::{AutoUpdate, UpdateConfig};

use release::{Asset, Release};
use version::Version;

/// 更新流程所处的阶段，界面据此显示进度与提示。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum Phase {
    /// 空闲：已有检查结果，或从未检查过
    Idle,
    Checking,
    Downloading,
    Verifying,
    Applying,
    Restarting,
    Failed,
}

impl Phase {
    /// 是否处在一次安装的中途；此时不允许再次触发。
    #[must_use]
    pub fn is_busy(self) -> bool {
        matches!(
            self,
            Self::Checking
                | Self::Downloading
                | Self::Verifying
                | Self::Applying
                | Self::Restarting
        )
    }
}

/// 一次检查与安装的可观察状态。
#[derive(Debug, Clone, Default)]
struct Status {
    phase: Option<Phase>,
    message: Option<String>,
    progress: f64,
    downloaded: u64,
    total: Option<u64>,
    latest: Option<Release>,
    error: Option<String>,
    checked_at: Option<String>,
}

/// 给界面与 CLI 的完整快照。
#[derive(Debug, Clone, Serialize)]
pub struct Snapshot {
    /// 当前二进制版本。
    pub current: String,
    /// 当前平台标识，如 `windows-x86_64`。
    pub platform: String,
    /// 生效中的更新配置。
    pub config: UpdateConfig,
    pub phase: Phase,
    pub message: Option<String>,
    /// 下载进度 0..=1。
    pub progress: f64,
    pub downloaded: u64,
    pub total: Option<u64>,
    /// 最近一次检查到的发布版本。
    pub latest: Option<Release>,
    /// 最近一次检查到的版本是否比当前版本新。
    pub available: bool,
    pub checked_at: Option<String>,
    pub error: Option<String>,
    /// 当前环境是否具备一键更新的条件（有本平台安装包、可写目录、有停止通道）。
    pub can_apply: bool,
    /// 正在运行的可执行文件路径。
    pub exe: String,
}

/// 更新服务：持有配置、当前版本与流程状态，供 HTTP 与 CLI 共用。
#[derive(Clone)]
pub struct UpdateService {
    inner: Arc<Inner>,
}

struct Inner {
    client: reqwest::Client,
    current: Version,
    platform: String,
    exe: PathBuf,
    config_path: Option<PathBuf>,
    shutdown: Option<mpsc::Sender<()>>,
    config: RwLock<UpdateConfig>,
    status: RwLock<Status>,
    applying: AtomicBool,
    auto_started: AtomicBool,
}

impl UpdateService {
    /// 构造服务；`exe` 缺省取当前进程的可执行文件。
    #[must_use]
    pub fn new(
        config: UpdateConfig,
        config_path: Option<PathBuf>,
        shutdown: Option<mpsc::Sender<()>>,
    ) -> Self {
        let exe = std::env::current_exe().unwrap_or_else(|_| PathBuf::from("gugu"));
        Self {
            inner: Arc::new(Inner {
                client: reqwest::Client::new(),
                current: Version::current(),
                platform: release::current_platform(),
                exe,
                config_path,
                shutdown,
                config: RwLock::new(config),
                status: RwLock::new(Status::default()),
                applying: AtomicBool::new(false),
                auto_started: AtomicBool::new(false),
            }),
        }
    }

    /// 当前版本。
    #[must_use]
    pub fn current(&self) -> Version {
        self.inner.current.clone()
    }

    /// 生效中的配置副本。
    #[must_use]
    pub fn config(&self) -> UpdateConfig {
        self.inner
            .config
            .read()
            .map(|config| config.clone())
            .unwrap_or_default()
    }

    /// 组装快照。
    #[must_use]
    pub fn snapshot(&self) -> Snapshot {
        let config = self.config();
        let status = self
            .inner
            .status
            .read()
            .map(|status| status.clone())
            .unwrap_or_default();
        let latest = status.latest.clone();
        let available = latest
            .as_ref()
            .is_some_and(|release| release.version.is_newer_than(&self.inner.current));
        Snapshot {
            current: self.inner.current.to_string(),
            platform: self.inner.platform.clone(),
            phase: status.phase.unwrap_or(Phase::Idle),
            message: status.message,
            progress: status.progress,
            downloaded: status.downloaded,
            total: status.total,
            latest: latest.clone(),
            available,
            checked_at: status.checked_at,
            error: status.error,
            can_apply: config.enabled
                && available
                && latest
                    .as_ref()
                    .is_some_and(|release| release.supports(&self.inner.platform))
                && self.inner.shutdown.is_some()
                && !self.inner.applying.load(Ordering::Relaxed),
            exe: self.inner.exe.to_string_lossy().to_string(),
            config,
        }
    }

    /// 拉取最新发布并与当前版本比较。
    ///
    /// # Errors
    /// 更新被关闭、网络失败或来源内容不可解析时返回原因。
    pub async fn check(&self) -> Result<Release, String> {
        if !self.config().enabled {
            return Err("更新已关闭（daemon.update.enabled = false）".to_string());
        }
        self.set_phase(Phase::Checking, Some("正在检查更新".to_string()));
        let source = source::Source::from_config(&self.config());
        match source::fetch_latest(&self.inner.client, &source).await {
            Ok(release) => {
                let available = release.version.is_newer_than(&self.inner.current);
                if let Ok(mut status) = self.inner.status.write() {
                    status.latest = Some(release.clone());
                    status.checked_at = Some(chrono::Utc::now().to_rfc3339());
                    status.error = None;
                }
                let message = if available {
                    format!("发现新版本 {}", release.version)
                } else {
                    format!("已是最新版本 {}", self.inner.current)
                };
                self.set_phase(Phase::Idle, Some(message));
                Ok(release)
            }
            Err(error) => {
                if let Ok(mut status) = self.inner.status.write() {
                    status.error = Some(error.clone());
                }
                self.set_phase(Phase::Failed, Some(error.clone()));
                Err(error)
            }
        }
    }

    /// 改自动更新策略：先写回配置文件，再更新内存中的配置。
    ///
    /// # Errors
    /// 没有配置文件路径或写入失败时返回原因。
    pub fn set_auto(&self, auto: AutoUpdate) -> Result<(), String> {
        let path = self
            .inner
            .config_path
            .as_deref()
            .ok_or_else(|| "当前进程没有配置文件路径，无法保存自动更新设置".to_string())?;
        crate::config::save_auto(path, auto).map_err(|e| e.to_string())?;
        if let Ok(mut config) = self.inner.config.write() {
            config.auto = auto;
        }
        Ok(())
    }

    /// 启动一次安装：校验条件后交给后台任务，立刻返回。
    ///
    /// # Errors
    /// 更新被关闭、已有安装在进行、没有本平台安装包或缺少停止通道时返回原因。
    pub fn start_apply(self: &Arc<Self>, expected: Option<String>) -> Result<(), String> {
        let config = self.config();
        if !config.enabled {
            return Err("更新已关闭（daemon.update.enabled = false）".to_string());
        }
        if self.inner.shutdown.is_none() {
            return Err("当前进程没有停止通道，替换后无法自动重启".to_string());
        }
        if self.inner.applying.swap(true, Ordering::SeqCst) {
            return Err("已有一次更新正在进行".to_string());
        }
        let service = Arc::clone(self);
        tokio::spawn(async move {
            if let Err(error) = service.run_apply(expected).await {
                tracing::warn!("更新失败: {error}");
                if let Ok(mut status) = service.inner.status.write() {
                    status.error = Some(error.clone());
                }
                service.inner.applying.store(false, Ordering::SeqCst);
                service.set_phase(Phase::Failed, Some(error));
            }
        });
        Ok(())
    }

    /// 发现新版本时按策略自动安装；`auto = off` 时什么都不做。
    pub fn spawn_auto(self: &Arc<Self>) {
        let config = self.config();
        if !config.enabled || config.auto == AutoUpdate::Off {
            return;
        }
        if self.inner.auto_started.swap(true, Ordering::SeqCst) {
            return;
        }
        let interval = Duration::from_secs(config.check_hours.max(1) * 3600);
        let service = Arc::clone(self);
        tokio::spawn(async move {
            // 启动时先让进程与服务稳定，再发第一次请求。
            tokio::time::sleep(Duration::from_secs(30)).await;
            loop {
                match service.check().await {
                    Ok(release) => {
                        let auto = service.config().auto;
                        if auto == AutoUpdate::Install
                            && release.version.is_newer_than(&service.inner.current)
                        {
                            tracing::info!("自动更新：安装 {}", release.version);
                            if let Err(error) = service.start_apply(None) {
                                tracing::warn!("自动更新未能启动: {error}");
                            }
                        }
                    }
                    Err(error) => tracing::warn!("自动检查更新失败: {error}"),
                }
                tokio::time::sleep(interval).await;
            }
        });
    }

    async fn run_apply(&self, expected: Option<String>) -> Result<(), String> {
        let mut latest = self
            .inner
            .status
            .read()
            .ok()
            .and_then(|status| status.latest.clone());
        if latest.is_none() {
            latest = Some(self.check().await?);
        }
        let release = latest.expect("上面已保证有值");
        if let Some(expected) = expected.as_deref() {
            if release.version.to_string() != expected.trim() {
                let refreshed = self.check().await?;
                if refreshed.version.to_string() != expected.trim() {
                    return Err(format!(
                        "最新版本是 {}，与请求的 {expected} 不一致",
                        refreshed.version
                    ));
                }
            }
        }
        if !release.version.is_newer_than(&self.inner.current) {
            return Err(format!(
                "当前版本 {} 已经不早于 {}",
                self.inner.current, release.version
            ));
        }
        let asset = release
            .asset_for(&self.inner.platform)
            .ok_or_else(|| {
                format!(
                    "版本 {} 没有 {} 的安装包",
                    release.version, self.inner.platform
                )
            })?
            .clone();

        let dir = installer::staging_dir(&self.inner.exe, &release.version);
        tokio::fs::create_dir_all(&dir)
            .await
            .map_err(|e| format!("创建暂存目录 {} 失败: {e}", dir.display()))?;
        let archive_path = dir.join(archive_name(&asset));
        let url = source::mirror_url(&asset.url, self.config().mirror.as_deref());

        self.set_phase(
            Phase::Downloading,
            Some(format!("正在下载 {}", release.version)),
        );
        let written = self
            .download(&url, &archive_path, asset.size, asset.sha256.as_deref())
            .await?;
        tracing::debug!("更新包已下载：{}（{written} 字节）", archive_path.display());

        self.set_phase(Phase::Verifying, Some("正在校验安装包".to_string()));
        let staged = {
            let archive_path = archive_path.clone();
            let dir = dir.clone();
            tokio::task::spawn_blocking(move || archive::unpack_binary(&archive_path, &dir))
                .await
                .map_err(|e| format!("解包任务失败: {e}"))??
        };
        let staged_for_check = staged.clone();
        let expected_version = release.version.clone();
        tokio::task::spawn_blocking(move || {
            installer::verify_binary(&staged_for_check, &expected_version)
        })
        .await
        .map_err(|e| format!("校验任务失败: {e}"))??;

        let config = self.config();
        let plan =
            installer::InstallPlan::new(&self.inner.exe, &release.version, config.keep_backup);
        let cwd = std::env::current_dir().unwrap_or_else(|_| PathBuf::from("."));
        let shim = installer::spawn_shim(&plan, std::process::id(), config.restart, &cwd)?;

        self.set_phase(
            Phase::Applying,
            Some(format!("已启动替换进程（PID {shim}），准备重启")),
        );
        // 留出把响应与状态送出的时间，再请求本进程停止。
        tokio::time::sleep(Duration::from_millis(600)).await;
        self.set_phase(Phase::Restarting, Some("守护进程正在重启".to_string()));
        match &self.inner.shutdown {
            Some(tx) => {
                tx.try_send(())
                    .map_err(|e| format!("停止请求未能送达: {e}"))?;
                Ok(())
            }
            None => Err("当前进程没有停止通道，替换后无法自动重启".to_string()),
        }
    }

    async fn download(
        &self,
        url: &str,
        dest: &Path,
        expected_size: Option<u64>,
        expected_sha256: Option<&str>,
    ) -> Result<u64, String> {
        let response = self
            .inner
            .client
            .get(url)
            .header(
                reqwest::header::USER_AGENT,
                format!("gugu-guard/{}", self.inner.current),
            )
            .timeout(Duration::from_secs(self.config().timeout_secs))
            .send()
            .await
            .map_err(|e| format!("下载 {url} 失败: {e}"))?;
        let status = response.status();
        if !status.is_success() {
            return Err(format!("下载 {url} 返回 HTTP {}", status.as_u16()));
        }
        let total = response.content_length().or(expected_size);
        if let Ok(mut status) = self.inner.status.write() {
            status.total = total;
            status.downloaded = 0;
            status.progress = 0.0;
        }

        let mut file = tokio::fs::File::create(dest)
            .await
            .map_err(|e| format!("创建 {} 失败: {e}", dest.display()))?;
        let mut hasher = Sha256::new();
        let mut written = 0u64;
        let mut last_report = Instant::now();
        let mut stream = response.bytes_stream();
        while let Some(chunk) = stream.next().await {
            let chunk = chunk.map_err(|e| format!("读取下载内容失败: {e}"))?;
            file.write_all(&chunk)
                .await
                .map_err(|e| format!("写入 {} 失败: {e}", dest.display()))?;
            hasher.update(&chunk);
            written += chunk.len() as u64;
            if last_report.elapsed() >= Duration::from_millis(200) {
                self.report_progress(written, total);
                last_report = Instant::now();
            }
        }
        file.flush()
            .await
            .map_err(|e| format!("写入 {} 失败: {e}", dest.display()))?;
        self.report_progress(written, total);

        if let Some(expected) = expected_size {
            if written != expected {
                return Err(format!(
                    "安装包大小不符：期望 {expected} 字节，实际 {written} 字节"
                ));
            }
        }
        if let Some(expected) = expected_sha256 {
            let digest = format!("{:x}", hasher.finalize());
            if !digest.eq_ignore_ascii_case(expected.trim()) {
                return Err(format!("安装包校验和不符：期望 {expected}，实际 {digest}"));
            }
        }
        Ok(written)
    }

    fn report_progress(&self, downloaded: u64, total: Option<u64>) {
        if let Ok(mut status) = self.inner.status.write() {
            status.downloaded = downloaded;
            status.total = total;
            status.progress = match total {
                Some(total) if total > 0 => (downloaded as f64 / total as f64).min(1.0),
                _ => 0.0,
            };
        }
    }

    fn set_phase(&self, phase: Phase, message: Option<String>) {
        if let Ok(mut status) = self.inner.status.write() {
            status.phase = Some(phase);
            status.message = message;
            if phase == Phase::Failed {
                status.progress = 0.0;
            }
        }
    }
}

/// 归档文件名：优先沿用下载地址的最后一段，取不到时按平台键拼一个。
fn archive_name(asset: &Asset) -> String {
    let from_url = reqwest::Url::parse(&asset.url)
        .ok()
        .and_then(|url| {
            url.path_segments()
                .and_then(|mut parts| parts.next_back().map(str::to_string))
        })
        .filter(|name| !name.is_empty());
    match from_url {
        Some(name) => name,
        None => format!("gugu-{}.archive", asset.key),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn service() -> Arc<UpdateService> {
        let (tx, _rx) = mpsc::channel(1);
        Arc::new(UpdateService::new(UpdateConfig::default(), None, Some(tx)))
    }

    #[test]
    fn snapshot_reports_current_version_without_a_check() {
        let snapshot = service().snapshot();
        assert_eq!(snapshot.current, env!("CARGO_PKG_VERSION"));
        assert!(!snapshot.available);
        assert!(!snapshot.can_apply);
        assert_eq!(snapshot.phase, Phase::Idle);
    }

    #[test]
    fn busy_phases_block_a_second_install() {
        assert!(Phase::Downloading.is_busy());
        assert!(Phase::Restarting.is_busy());
        assert!(!Phase::Idle.is_busy());
        assert!(!Phase::Failed.is_busy());
    }

    #[test]
    fn archive_name_falls_back_to_platform_key() {
        let asset = Asset {
            key: "windows-x86_64".to_string(),
            url: "https://example.com/download".to_string(),
            size: None,
            sha256: None,
        };
        assert_eq!(archive_name(&asset), "download");
        let asset = Asset {
            url: "not a url".to_string(),
            ..asset
        };
        assert_eq!(archive_name(&asset), "gugu-windows-x86_64.archive");
    }

    #[tokio::test]
    async fn apply_is_refused_when_updates_are_disabled() {
        let config = UpdateConfig {
            enabled: false,
            ..Default::default()
        };
        let (tx, _rx) = mpsc::channel(1);
        let service = Arc::new(UpdateService::new(config, None, Some(tx)));
        let err = service.start_apply(None).expect_err("关闭后应拒绝");
        assert!(err.contains("更新已关闭"), "{err}");
    }

    #[tokio::test]
    async fn apply_is_refused_without_a_shutdown_channel() {
        let service = Arc::new(UpdateService::new(UpdateConfig::default(), None, None));
        let err = service.start_apply(None).expect_err("没有停止通道应拒绝");
        assert!(err.contains("停止通道"), "{err}");
    }
}
