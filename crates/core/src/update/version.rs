//! 版本号：解析、比较与展示。
//!
//! 只覆盖发布标签用到的 `x.y.z` 与 `x.y.z-pre` 两种形态：`v` 前缀与 `+build`
//! 元数据会被剥掉，预发布段按 `.` 分段比较（数字段按数值，其余按字典序），
//! 因此 `1.0.0-rc.10` 高于 `1.0.0-rc.2`，而无预发布段的 `1.0.0` 高于任何 `1.0.0-*`。

use std::cmp::Ordering;
use std::fmt;

/// 一个已解析的版本号。
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct Version {
    major: u64,
    minor: u64,
    patch: u64,
    pre: Option<String>,
}

impl Version {
    /// 解析 `v1.2.3`、`1.2.3-rc.1` 这类写法；段数不为三段或含非数字段时返回 None。
    #[must_use]
    pub fn parse(input: &str) -> Option<Self> {
        let text = input.trim();
        let text = text.strip_prefix('v').unwrap_or(text);
        let text = text.split('+').next().unwrap_or(text);
        let (core, pre) = match text.split_once('-') {
            Some((core, pre)) => (core, Some(pre)),
            None => (text, None),
        };
        let mut parts = core.split('.');
        let major = parts.next()?.parse().ok()?;
        let minor = parts.next()?.parse().ok()?;
        let patch = parts.next()?.parse().ok()?;
        if parts.next().is_some() {
            return None;
        }
        let pre = pre
            .map(str::trim)
            .filter(|pre| !pre.is_empty())
            .map(str::to_string);
        Some(Self {
            major,
            minor,
            patch,
            pre,
        })
    }

    /// 当前二进制的编译期版本。
    #[must_use]
    pub fn current() -> Self {
        Self::parse(env!("CARGO_PKG_VERSION")).expect("CARGO_PKG_VERSION 始终是三段版本号")
    }

    /// `self` 是否比 `other` 新。
    #[must_use]
    pub fn is_newer_than(&self, other: &Self) -> bool {
        self.cmp(other) == Ordering::Greater
    }

    #[must_use]
    pub fn without_pre(&self) -> Self {
        Self {
            major: self.major,
            minor: self.minor,
            patch: self.patch,
            pre: None,
        }
    }
}

impl fmt::Display for Version {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}.{}.{}", self.major, self.minor, self.patch)?;
        if let Some(pre) = &self.pre {
            write!(f, "-{pre}")?;
        }
        Ok(())
    }
}

/// 对外一律用字符串表示版本，避免前端与 CLI 各自拼 `major.minor.patch`。
impl serde::Serialize for Version {
    fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.serialize_str(&self.to_string())
    }
}

impl PartialOrd for Version {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}

impl Ord for Version {
    fn cmp(&self, other: &Self) -> Ordering {
        (self.major, self.minor, self.patch)
            .cmp(&(other.major, other.minor, other.patch))
            .then_with(|| cmp_pre(self.pre.as_deref(), other.pre.as_deref()))
    }
}

/// 预发布段比较：没有预发布段的一方更新。
fn cmp_pre(left: Option<&str>, right: Option<&str>) -> Ordering {
    match (left, right) {
        (None, None) => Ordering::Equal,
        (None, Some(_)) => Ordering::Greater,
        (Some(_), None) => Ordering::Less,
        (Some(left), Some(right)) => {
            let mut left_parts = left.split('.');
            let mut right_parts = right.split('.');
            loop {
                match (left_parts.next(), right_parts.next()) {
                    (None, None) => return Ordering::Equal,
                    (None, Some(_)) => return Ordering::Less,
                    (Some(_), None) => return Ordering::Greater,
                    (Some(a), Some(b)) => {
                        let ordering = match (a.parse::<u64>(), b.parse::<u64>()) {
                            (Ok(a), Ok(b)) => a.cmp(&b),
                            (Ok(_), Err(_)) => Ordering::Less,
                            (Err(_), Ok(_)) => Ordering::Greater,
                            (Err(_), Err(_)) => a.cmp(b),
                        };
                        if ordering != Ordering::Equal {
                            return ordering;
                        }
                    }
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn v(text: &str) -> Version {
        Version::parse(text).unwrap_or_else(|| panic!("{text} 应能解析"))
    }

    #[test]
    fn accepts_tag_forms() {
        assert_eq!(v("1.2.3").to_string(), "1.2.3");
        assert_eq!(v("v0.1.0").to_string(), "0.1.0");
        assert_eq!(v(" v1.2.3-rc.1 ").to_string(), "1.2.3-rc.1");
        assert_eq!(v("1.2.3+build.7").to_string(), "1.2.3");
    }

    #[test]
    fn rejects_malformed_input() {
        for text in ["", "1.2", "1.2.3.4", "a.b.c", "1.2.x", "-1.0.0"] {
            assert!(Version::parse(text).is_none(), "{text} 不应被接受");
        }
    }

    #[test]
    fn orders_numeric_segments() {
        assert!(v("0.2.0").is_newer_than(&v("0.1.9")));
        assert!(v("0.10.0").is_newer_than(&v("0.9.9")));
        assert!(!v("1.0.0").is_newer_than(&v("1.0.0")));
    }

    #[test]
    fn prerelease_is_older_than_release() {
        assert!(v("1.0.0").is_newer_than(&v("1.0.0-rc.1")));
        assert!(v("1.0.0-rc.10").is_newer_than(&v("1.0.0-rc.2")));
        assert!(v("1.0.0-rc.1").is_newer_than(&v("1.0.0-beta")));
    }

    #[test]
    fn current_matches_manifest_version() {
        assert_eq!(Version::current().to_string(), env!("CARGO_PKG_VERSION"));
    }
}
