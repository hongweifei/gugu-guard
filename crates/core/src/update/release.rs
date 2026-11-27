//! 发布清单：把 GitHub、Gitee 的 release JSON 与自建清单归一成同一个模型。
//!
//! 三种来源的差异只在字段名与资产命名上：GitHub / Gitee 用 `tag_name` + `assets[].name`，
//! 自建清单允许直接给出 `assets` 映射。归一之后，选择资产、下载与展示都只看
//! [`Release`] 与 [`Asset`]。

use serde::Serialize;
use serde_json::Value;

use super::version::Version;

/// 发布包里的一个下载资产；`key` 是平台标识（如 `windows-x86_64`）。
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Asset {
    pub key: String,
    pub url: String,
    pub size: Option<u64>,
    pub sha256: Option<String>,
}

/// 一个已归一化的发布版本。
#[derive(Debug, Clone, Serialize)]
pub struct Release {
    pub version: Version,
    pub tag: String,
    pub name: Option<String>,
    pub notes: Option<String>,
    pub page_url: Option<String>,
    pub published_at: Option<String>,
    pub assets: Vec<Asset>,
}

impl Release {
    /// 当前平台对应的下载资产。
    ///
    /// 只做平台键的精确匹配：同一版本里可能同时有 x86_64 与 aarch64 的包，
    /// 前缀匹配会把另一个架构的包当成自己的。
    #[must_use]
    pub fn asset_for(&self, platform: &str) -> Option<&Asset> {
        self.assets.iter().find(|asset| asset.key == platform)
    }

    /// 这个版本是否提供了当前平台的包；缺失时界面要说明「该版本暂无本平台安装包」。
    #[must_use]
    pub fn supports(&self, platform: &str) -> bool {
        self.asset_for(platform).is_some()
    }
}

/// 当前平台的资产键，形如 `windows-x86_64`。
#[must_use]
pub fn current_platform() -> String {
    format!("{}-{}", std::env::consts::OS, std::env::consts::ARCH)
}

/// 从资产文件名推出平台键：`gugu-windows-x86_64.zip` → `windows-x86_64`。
#[must_use]
pub fn asset_key(name: &str) -> Option<String> {
    let stem = name.trim().strip_prefix("gugu-")?;
    let stem = stem
        .strip_suffix(".tar.gz")
        .or_else(|| stem.strip_suffix(".zip"))
        .or_else(|| stem.strip_suffix(".exe"))
        .unwrap_or(stem);
    if stem.is_empty() || stem.contains('/') || stem.contains('\\') {
        return None;
    }
    Some(stem.to_string())
}

/// 解析 GitHub / Gitee 的单条 release 对象。
#[must_use]
pub fn parse_release(value: &Value) -> Option<Release> {
    let tag = value.get("tag_name")?.as_str()?.trim().to_string();
    let version = Version::parse(&tag)?;
    let assets = value
        .get("assets")
        .and_then(Value::as_array)
        .map(|items| items.iter().filter_map(parse_asset).collect())
        .unwrap_or_default();
    Some(Release {
        version,
        tag,
        name: text(value, "name"),
        notes: text(value, "body"),
        page_url: text(value, "html_url"),
        published_at: text(value, "published_at").or_else(|| text(value, "created_at")),
        assets,
    })
}

fn parse_asset(value: &Value) -> Option<Asset> {
    let name = value.get("name")?.as_str()?;
    let url = text(value, "browser_download_url")
        .or_else(|| text(value, "download_url"))
        .or_else(|| text(value, "url"))?;
    Some(Asset {
        key: asset_key(name)?,
        url,
        size: value.get("size").and_then(Value::as_u64),
        sha256: sha256_of(value),
    })
}

/// 解析自建清单：既接受 `assets` 映射，也接受 GitHub 形状的 `assets` 数组。
///
/// # Errors
/// 缺少版本或资产不可读时返回原因，调用方据此报「清单格式不对」。
pub fn parse_manifest(body: &str) -> Result<Release, String> {
    let value: Value = serde_json::from_str(body).map_err(|e| format!("清单不是合法 JSON: {e}"))?;
    let tag = value
        .get("tag")
        .or_else(|| value.get("version"))
        .and_then(Value::as_str)
        .ok_or_else(|| "清单缺少 tag 或 version".to_string())?
        .trim()
        .to_string();
    let version = Version::parse(&tag).ok_or_else(|| format!("清单版本号不合法: {tag}"))?;
    let assets = match value.get("assets") {
        Some(Value::Array(items)) => items.iter().filter_map(parse_asset).collect(),
        Some(Value::Object(map)) => map
            .iter()
            .filter_map(|(key, item)| match item {
                Value::String(url) => Some(Asset {
                    key: key.clone(),
                    url: url.clone(),
                    size: None,
                    sha256: None,
                }),
                Value::Object(_) => Some(Asset {
                    key: key.clone(),
                    url: text(item, "url")?,
                    size: item.get("size").and_then(Value::as_u64),
                    sha256: sha256_of(item),
                }),
                _ => None,
            })
            .collect(),
        _ => Vec::new(),
    };
    Ok(Release {
        version,
        tag,
        name: text(&value, "name"),
        notes: text(&value, "notes").or_else(|| text(&value, "body")),
        page_url: text(&value, "page_url").or_else(|| text(&value, "html_url")),
        published_at: text(&value, "published_at"),
        assets,
    })
}

/// 从 release 列表里挑出最新的正式版；草稿与预发布由调用方先过滤。
#[must_use]
pub fn pick_newest(releases: impl IntoIterator<Item = Release>) -> Option<Release> {
    releases
        .into_iter()
        .max_by(|a, b| a.version.cmp(&b.version))
}

fn text(value: &Value, key: &str) -> Option<String> {
    value
        .get(key)
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|text| !text.is_empty())
        .map(str::to_string)
}

/// GitHub 自 2025 起在资产上给 `digest: "sha256:..."`；自建清单可直接写 `sha256`。
fn sha256_of(value: &Value) -> Option<String> {
    if let Some(digest) = text(value, "sha256") {
        return Some(digest);
    }
    text(value, "digest").and_then(|digest| {
        digest
            .strip_prefix("sha256:")
            .map(str::to_string)
            .or(Some(digest))
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn maps_asset_names_to_platform_keys() {
        assert_eq!(
            asset_key("gugu-windows-x86_64.zip").as_deref(),
            Some("windows-x86_64")
        );
        assert_eq!(
            asset_key("gugu-linux-x86_64.tar.gz").as_deref(),
            Some("linux-x86_64")
        );
        assert_eq!(
            asset_key("gugu-linux-aarch64.tar.gz").as_deref(),
            Some("linux-aarch64")
        );
        assert_eq!(asset_key("gugu"), None);
        assert_eq!(asset_key("other-thing.zip"), None);
        assert_eq!(asset_key("gugu-a/b.zip"), None);
    }

    #[test]
    fn parses_github_release_shape() {
        let text = r#"{
            "tag_name": "v0.2.0",
            "name": "0.2.0",
            "body": "修了几处",
            "html_url": "https://github.com/o/r/releases/tag/v0.2.0",
            "published_at": "2026-11-27T10:00:00Z",
            "assets": [
                {"name":"gugu-windows-x86_64.zip","browser_download_url":"https://e/a.zip","size":1234,"digest":"sha256:abc"},
                {"name":"gugu-linux-x86_64.tar.gz","browser_download_url":"https://e/a.tgz","size":5678}
            ]
        }"#;
        let release =
            parse_release(&serde_json::from_str::<Value>(text).expect("JSON")).expect("可解析");
        assert_eq!(release.version.to_string(), "0.2.0");
        assert_eq!(release.notes.as_deref(), Some("修了几处"));
        let asset = release.asset_for("windows-x86_64").expect("有 Windows 包");
        assert_eq!(asset.url, "https://e/a.zip");
        assert_eq!(asset.sha256.as_deref(), Some("abc"));
        assert!(release.supports("linux-x86_64"));
        assert!(!release.supports("macos-aarch64"));
    }

    #[test]
    fn parses_manifest_with_mapping_assets() {
        let text = r#"{
            "version": "0.3.0",
            "notes": "自建镜像",
            "assets": {
                "linux-x86_64": "https://mirror/gugu-linux-x86_64.tar.gz",
                "windows-x86_64": {"url": "https://mirror/gugu.zip", "size": 99}
            }
        }"#;
        let release = parse_manifest(text).expect("可解析");
        assert_eq!(release.version.to_string(), "0.3.0");
        assert_eq!(release.assets.len(), 2);
        assert_eq!(
            release.asset_for("windows-x86_64").map(|a| a.size),
            Some(Some(99))
        );
    }

    #[test]
    fn rejects_manifest_without_version() {
        let err = parse_manifest(r#"{"notes":"x"}"#).expect_err("缺版本应失败");
        assert!(err.contains("tag 或 version"), "{err}");
    }

    #[test]
    fn picks_the_newest_release() {
        let make = |tag: &str| Release {
            version: Version::parse(tag).expect("合法版本"),
            tag: tag.to_string(),
            name: None,
            notes: None,
            page_url: None,
            published_at: None,
            assets: Vec::new(),
        };
        let picked =
            pick_newest(vec![make("0.1.0"), make("0.10.0"), make("0.9.3")]).expect("有结果");
        assert_eq!(picked.version.to_string(), "0.10.0");
    }
}
