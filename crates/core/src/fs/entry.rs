//! 目录列表与条目元信息。
//!
//! 类型与 `readonly` 取 `symlink_metadata`：符号链接按链接自身报告，不跟随到目标，
//! 面板因此能如实显示「这是一个链接」，而不是把链接目标的大小算进来。

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use std::path::Path;

use crate::config::path_to_forward_slashes;
use crate::error::{GuguError, Result};

use super::{Scope, TEMP_PREFIX};

/// 条目类型。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum EntryKind {
    Dir,
    File,
    Link,
}

/// 一个目录项。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Entry {
    pub name: String,
    /// 正斜杠形式的绝对路径，前端与 CLI 都直接回传这一份。
    pub path: String,
    pub is_dir: bool,
    pub kind: EntryKind,
    /// 文件字节数；目录固定为 0。
    pub size: u64,
    pub modified: Option<DateTime<Utc>>,
    pub readonly: bool,
}

/// 一次目录读取的结果。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Listing {
    pub path: String,
    /// 上一级目录；已到作用域顶端时为 None。
    pub parent: Option<String>,
    /// 当前作用域是否允许写，前端据此开关写操作按钮。
    pub writable: bool,
    pub entries: Vec<Entry>,
}

/// 读取目录内容：目录优先、名称不区分大小写排序。
///
/// # Errors
/// 路径越界、不存在或不是目录时返回错误。
pub fn list(scope: &Scope, raw: &str) -> Result<Listing> {
    let dir = scope.resolve(raw)?;
    if !dir.is_dir() {
        return Err(GuguError::InvalidPath(format!(
            "路径不是目录: {}",
            dir.display()
        )));
    }

    let read_dir = std::fs::read_dir(&dir)?;
    let mut entries = Vec::new();
    for item in read_dir.flatten() {
        let name = item.file_name().to_string_lossy().to_string();
        // 传输中的半成品不出现在面板里。
        if name.starts_with(TEMP_PREFIX) {
            continue;
        }
        let link_meta = item.metadata();
        let file_type = item.file_type();
        let (is_dir, kind, size, readonly) = match (&file_type, &link_meta) {
            (Ok(file_type), Ok(meta)) => {
                let kind = if file_type.is_symlink() {
                    EntryKind::Link
                } else if file_type.is_dir() {
                    EntryKind::Dir
                } else {
                    EntryKind::File
                };
                let size = if kind == EntryKind::Dir {
                    0
                } else {
                    meta.len()
                };
                (
                    file_type.is_dir(),
                    kind,
                    size,
                    meta.permissions().readonly(),
                )
            }
            // 读不到元信息（权限、竞态删除）时仍列出名字，不让整个目录失败。
            (_, _) => (false, EntryKind::File, 0, false),
        };
        let modified = link_meta
            .ok()
            .and_then(|meta| meta.modified().ok())
            .map(DateTime::<Utc>::from);
        entries.push(Entry {
            name,
            path: path_to_forward_slashes(&dir.join(item.file_name())),
            is_dir,
            kind,
            size,
            modified,
            readonly,
        });
    }

    entries.sort_by(|left, right| {
        right
            .is_dir
            .cmp(&left.is_dir)
            .then_with(|| left.name.to_lowercase().cmp(&right.name.to_lowercase()))
    });

    let parent = dir
        .parent()
        .filter(|parent| scope.contains(parent))
        .map(path_to_forward_slashes);

    Ok(Listing {
        path: path_to_forward_slashes(&dir),
        parent,
        writable: scope.writable(),
        entries,
    })
}

/// 单个路径的元信息；不存在或越界时返回错误。
///
/// # Errors
/// 路径越界或不存在时返回错误。
pub fn stat(scope: &Scope, raw: &str) -> Result<Entry> {
    let path: &Path = &scope.resolve(raw)?;
    let meta = std::fs::symlink_metadata(path)?;
    let kind = if meta.file_type().is_symlink() {
        EntryKind::Link
    } else if meta.is_dir() {
        EntryKind::Dir
    } else {
        EntryKind::File
    };
    Ok(Entry {
        name: path
            .file_name()
            .map(|name| name.to_string_lossy().to_string())
            .unwrap_or_else(|| path_to_forward_slashes(path)),
        path: path_to_forward_slashes(path),
        is_dir: meta.is_dir(),
        kind,
        size: if kind == EntryKind::Dir {
            0
        } else {
            meta.len()
        },
        modified: meta.modified().ok().map(DateTime::<Utc>::from),
        readonly: meta.permissions().readonly(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tests::support::tempdir;

    #[test]
    fn lists_directories_first_and_reports_metadata() {
        let dir = tempdir();
        let root = dir.path().join("root");
        std::fs::create_dir_all(root.join("zeta")).expect("建目录");
        std::fs::write(root.join("Alpha.txt"), b"12345").expect("写文件");
        std::fs::write(root.join("beta.log"), b"x").expect("写文件");

        let scope = Scope::new(vec![root.clone()], true);
        let listing = list(&scope, &path_to_forward_slashes(&root)).expect("读目录");

        assert!(listing.writable);
        let names: Vec<&str> = listing.entries.iter().map(|e| e.name.as_str()).collect();
        assert_eq!(
            names,
            ["zeta", "Alpha.txt", "beta.log"],
            "目录优先，名称不区分大小写"
        );

        let alpha = listing
            .entries
            .iter()
            .find(|entry| entry.name == "Alpha.txt")
            .expect("应有 Alpha.txt");
        assert_eq!(alpha.size, 5);
        assert_eq!(alpha.kind, EntryKind::File);
        assert!(alpha.modified.is_some());
    }

    #[test]
    fn parent_is_none_at_the_top_of_the_scope() {
        let dir = tempdir();
        let root = dir.path().join("root");
        std::fs::create_dir_all(root.join("child")).expect("建目录");
        let scope = Scope::new(vec![root.clone()], false);

        let at_root = list(&scope, &path_to_forward_slashes(&root)).expect("读根目录");
        assert!(at_root.parent.is_none(), "根目录之外不再放行上级");
        assert!(!at_root.writable);

        let child = list(&scope, &path_to_forward_slashes(&root.join("child"))).expect("读子目录");
        assert!(child.parent.is_some());
    }

    #[test]
    fn a_file_is_not_a_listable_directory() {
        let dir = tempdir();
        let file = dir.path().join("a.txt");
        std::fs::write(&file, b"a").expect("写文件");
        let scope = Scope::new(vec![dir.path().to_path_buf()], true);
        let error = list(&scope, &path_to_forward_slashes(&file)).expect_err("文件不能当目录读");
        assert!(error.to_string().contains("不是目录"), "{error}");
    }

    #[test]
    fn stat_returns_the_entry_itself() {
        let dir = tempdir();
        let file = dir.path().join("a.txt");
        std::fs::write(&file, b"abc").expect("写文件");
        let scope = Scope::new(vec![dir.path().to_path_buf()], true);
        let entry = stat(&scope, &path_to_forward_slashes(&file)).expect("取元信息");
        assert_eq!(entry.name, "a.txt");
        assert_eq!(entry.size, 3);
        assert_eq!(entry.kind, EntryKind::File);
    }
}
