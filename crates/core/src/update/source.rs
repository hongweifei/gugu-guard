//! 更新来源：按配置拼出清单地址、拉取响应并归一化出最新正式版。
//!
//! 三种来源的请求地址在这里收口：GitHub 与 Gitee 走各自的 release 列表接口，
//! 自建清单直接读一个 JSON。可选的镜像前缀只作用于下载地址，不影响清单地址。

use std::time::Duration;

use serde_json::Value;

use crate::config::{UpdateConfig, UpdateProvider};

use super::release::{parse_manifest, parse_release, pick_newest, Release};

const GITHUB_API: &str = "https://api.github.com";
const GITEE_API: &str = "https://gitee.com/api/v5";
/// 列表接口一次取的条数：足够覆盖若干个待选版本，又不至于拖慢响应。
const PER_PAGE: u32 = 20;

/// 一次检查需要的来源参数，从 [`UpdateConfig`] 派生。
#[derive(Debug, Clone)]
pub struct Source {
    pub provider: UpdateProvider,
    pub repo: String,
    pub manifest_url: Option<String>,
    pub mirror: Option<String>,
    pub timeout: Duration,
}

impl Source {
    #[must_use]
    pub fn from_config(config: &UpdateConfig) -> Self {
        Self {
            provider: config.provider,
            repo: config.repo.trim().to_string(),
            manifest_url: config
                .manifest_url
                .as_deref()
                .map(str::trim)
                .filter(|url| !url.is_empty())
                .map(str::to_string),
            mirror: config
                .mirror
                .as_deref()
                .map(str::trim)
                .filter(|url| !url.is_empty())
                .map(str::to_string),
            timeout: Duration::from_secs(config.timeout_secs),
        }
    }

    /// 清单地址：`manifest` 来源必须有 `manifest_url`。
    ///
    /// # Errors
    /// 自建清单缺地址时返回原因。
    pub fn index_url(&self) -> Result<String, String> {
        match self.provider {
            UpdateProvider::Manifest => self.manifest_url.clone().ok_or_else(|| {
                "来源设为 manifest 时必须配置 daemon.update.manifest_url".to_string()
            }),
            UpdateProvider::Github => Ok(format!(
                "{GITHUB_API}/repos/{}/releases?per_page={PER_PAGE}",
                self.repo
            )),
            UpdateProvider::Gitee => Ok(format!(
                "{GITEE_API}/repos/{}/releases?per_page={PER_PAGE}",
                self.repo
            )),
        }
    }
}

/// 拉取最新正式版。预发布与草稿跳过；一个都没有时报错。
///
/// # Errors
/// 网络失败、响应非 2xx、内容不是预期 JSON 或没有正式版时返回原因。
pub async fn fetch_latest(client: &reqwest::Client, source: &Source) -> Result<Release, String> {
    let url = source.index_url()?;
    let response = client
        .get(&url)
        .header(
            reqwest::header::USER_AGENT,
            format!("gugu-guard/{}", super::version::Version::current()),
        )
        .header(reqwest::header::ACCEPT, "application/json")
        .timeout(source.timeout)
        .send()
        .await
        .map_err(|e| format!("请求 {url} 失败: {e}"))?;

    let status = response.status();
    let body = response
        .text()
        .await
        .map_err(|e| format!("读取 {url} 的响应失败: {e}"))?;
    if !status.is_success() {
        return Err(format!(
            "{url} 返回 HTTP {}: {}",
            status.as_u16(),
            excerpt(&body)
        ));
    }

    match source.provider {
        UpdateProvider::Manifest => parse_manifest(&body),
        UpdateProvider::Github | UpdateProvider::Gitee => {
            let value: Value = serde_json::from_str(&body)
                .map_err(|e| format!("{url} 的响应不是合法 JSON: {e}"))?;
            let items = value
                .as_array()
                .ok_or_else(|| format!("{url} 的响应不是发布列表"))?;
            let releases = items
                .iter()
                .filter(|item| !is_skipped(item))
                .filter_map(parse_release)
                .collect::<Vec<_>>();
            pick_newest(releases).ok_or_else(|| "没有找到可用的正式发布版".to_string())
        }
    }
}

fn is_skipped(item: &Value) -> bool {
    item.get("draft").and_then(Value::as_bool).unwrap_or(false)
        || item
            .get("prerelease")
            .and_then(Value::as_bool)
            .unwrap_or(false)
}

/// 把下载地址套上镜像前缀：`{mirror}` 作占位符时直接替换，否则拼在原始地址前面。
#[must_use]
pub fn mirror_url(url: &str, mirror: Option<&str>) -> String {
    let Some(mirror) = mirror.map(str::trim).filter(|mirror| !mirror.is_empty()) else {
        return url.to_string();
    };
    if mirror.contains("{}") {
        return mirror.replace("{}", url);
    }
    format!("{}/{}", mirror.trim_end_matches('/'), url)
}

fn excerpt(body: &str) -> String {
    let text = body.trim();
    let taken: String = text.chars().take(160).collect();
    if taken.is_empty() {
        "(空响应)".to_string()
    } else {
        taken
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn source(provider: UpdateProvider) -> Source {
        Source {
            provider,
            repo: "hongweifei/gugu-guard".to_string(),
            manifest_url: None,
            mirror: None,
            timeout: Duration::from_secs(5),
        }
    }

    #[test]
    fn builds_index_urls_per_provider() {
        assert_eq!(
            source(UpdateProvider::Github).index_url().expect("github"),
            "https://api.github.com/repos/hongweifei/gugu-guard/releases?per_page=20"
        );
        assert_eq!(
            source(UpdateProvider::Gitee).index_url().expect("gitee"),
            "https://gitee.com/api/v5/repos/hongweifei/gugu-guard/releases?per_page=20"
        );
        let err = source(UpdateProvider::Manifest)
            .index_url()
            .expect_err("缺地址应报错");
        assert!(err.contains("manifest_url"), "{err}");
    }

    #[test]
    fn applies_mirror_prefix() {
        assert_eq!(mirror_url("https://e/a.zip", None), "https://e/a.zip");
        assert_eq!(
            mirror_url("https://e/a.zip", Some("https://m/")),
            "https://m/https://e/a.zip"
        );
        assert_eq!(
            mirror_url("https://e/a.zip", Some("https://m/{}")),
            "https://m/https://e/a.zip"
        );
    }
}
