//! 路径作用域：先解析成真实路径，再判断是否落在允许的根目录内。
//!
//! 判定发生在 `canonicalize` 之后，`..` 与符号链接都已被解析；
//! 准备新建的路径按父目录判定，父目录之外的目标写不进去。

use std::path::{Path, PathBuf};

use crate::config::{path_to_forward_slashes, strip_unc_prefix};
use crate::error::{GuguError, Result};

/// 文件操作的作用域：允许的根目录与写权限。
///
/// 根目录为空表示不限制范围；`writable` 由 `daemon.web.fs_write` 决定。
#[derive(Debug, Clone, Default)]
pub struct Scope {
    roots: Vec<PathBuf>,
    writable: bool,
}

impl Scope {
    /// 根目录统一成绝对、去 UNC 前缀的形态；相对路径以当前工作目录为基准。
    #[must_use]
    pub fn new(roots: Vec<PathBuf>, writable: bool) -> Self {
        let roots = roots
            .into_iter()
            .map(|root| normalize_root(&root))
            .collect();
        Self { roots, writable }
    }

    #[must_use]
    pub fn writable(&self) -> bool {
        self.writable
    }

    /// 允许的根目录；空列表表示不限制范围。
    #[must_use]
    pub fn roots(&self) -> &[PathBuf] {
        &self.roots
    }

    /// 写操作的前置检查：只读模式下直接拒绝，不去碰磁盘。
    ///
    /// # Errors
    /// `daemon.web.fs_write` 未开启时返回 [`GuguError::ReadOnly`]。
    pub fn ensure_writable(&self) -> Result<()> {
        if self.writable {
            Ok(())
        } else {
            Err(GuguError::ReadOnly)
        }
    }

    /// 解析一个必须存在的路径。
    ///
    /// # Errors
    /// 路径为空、不存在或落在根目录之外时返回错误。
    pub fn resolve(&self, raw: &str) -> Result<PathBuf> {
        let raw = checked(raw)?;
        let candidate = absolute(raw);
        let resolved = std::fs::canonicalize(&candidate).map_err(|error| {
            if error.kind() == std::io::ErrorKind::NotFound {
                GuguError::NotFound(format!("路径不存在: {}", candidate.display()))
            } else {
                GuguError::InvalidPath(format!("路径不可用: {}（{error}）", candidate.display()))
            }
        })?;
        let resolved = strip_unc_prefix(&resolved);
        self.admit(&resolved)?;
        Ok(resolved)
    }

    /// 缺省目录：第一个根目录；没有根时用当前工作目录。
    #[must_use]
    pub fn default_dir(&self) -> PathBuf {
        self.roots
            .first()
            .cloned()
            .or_else(|| std::env::current_dir().ok())
            .unwrap_or_else(|| PathBuf::from("."))
    }

    /// 解析一个准备新建或改名到的路径：自身可以不存在，父目录必须存在且在作用域内。
    ///
    /// # Errors
    /// 路径为空、缺少文件名、父目录不可用或越界时返回错误。
    pub fn resolve_new(&self, raw: &str) -> Result<PathBuf> {
        let raw = checked(raw)?;
        let candidate = absolute(raw);
        let Some(name) = candidate.file_name().map(std::ffi::OsStr::to_owned) else {
            return Err(GuguError::InvalidPath(format!(
                "路径缺少文件名: {}",
                candidate.display()
            )));
        };
        if name == "." || name == ".." {
            return Err(GuguError::InvalidPath(format!(
                "路径缺少可用的文件名: {}",
                candidate.display()
            )));
        }
        let parent = candidate.parent().ok_or_else(|| {
            GuguError::InvalidPath(format!("路径没有父目录: {}", candidate.display()))
        })?;
        let parent = std::fs::canonicalize(parent).map_err(|error| {
            GuguError::InvalidPath(format!("父目录不可用: {}（{error}）", parent.display()))
        })?;
        let parent = strip_unc_prefix(&parent);
        self.admit(&parent)?;

        let target = parent.join(name);
        // 目标已存在时按真实路径复核：指向作用域之外的符号链接不能借改名写进去。
        if let Ok(real) = std::fs::canonicalize(&target) {
            self.admit(&strip_unc_prefix(&real))?;
        }
        Ok(target)
    }

    /// 解析一个可以有多级不存在的路径：向上找到最近的已存在祖先，校验后拼回缺失的分量。
    ///
    /// 逐级创建目录时用这一支：`a/b/c` 里的 `a`、`b` 都可以还没建，
    /// 但最近的祖先必须落在作用域内，`..` 在词法归约阶段就被折叠掉。
    ///
    /// # Errors
    /// 路径为空、没有可用的祖先目录或越界时返回错误。
    pub fn resolve_tree(&self, raw: &str) -> Result<PathBuf> {
        let raw = checked(raw)?;
        let mut candidate = lexical(absolute(raw));
        let mut missing: Vec<std::ffi::OsString> = Vec::new();
        while !candidate.exists() {
            let Some(name) = candidate.file_name().map(std::ffi::OsStr::to_owned) else {
                return Err(GuguError::InvalidPath(format!(
                    "路径没有可用的祖先目录: {}",
                    candidate.display()
                )));
            };
            missing.push(name);
            let Some(parent) = candidate.parent() else {
                return Err(GuguError::InvalidPath(format!(
                    "路径没有可用的祖先目录: {}",
                    candidate.display()
                )));
            };
            candidate = parent.to_path_buf();
        }

        let mut path = strip_unc_prefix(&std::fs::canonicalize(&candidate).map_err(|error| {
            GuguError::InvalidPath(format!(
                "祖先目录不可用: {}（{error}）",
                candidate.display()
            ))
        })?);
        self.admit(&path)?;
        for name in missing.iter().rev() {
            path.push(name);
        }
        Ok(path)
    }

    /// 路径是否落在某个根目录内（根目录自身也算）。
    #[must_use]
    pub fn contains(&self, path: &Path) -> bool {
        self.roots.is_empty() || self.roots.iter().any(|root| is_under(path, root))
    }

    /// 路径是否就是某个根目录自身；删除与改名用它挡住「把整个根端掉」。
    #[must_use]
    pub fn is_root(&self, path: &Path) -> bool {
        self.roots.iter().any(|root| same_path(path, root))
    }

    fn admit(&self, path: &Path) -> Result<()> {
        if self.contains(path) {
            return Ok(());
        }
        Err(GuguError::PathDenied(path_to_forward_slashes(path)))
    }
}

fn checked(raw: &str) -> Result<&str> {
    let raw = raw.trim();
    if raw.is_empty() {
        return Err(GuguError::InvalidPath("路径为空".to_string()));
    }
    Ok(raw)
}

fn absolute(raw: &str) -> PathBuf {
    let path = PathBuf::from(raw);
    if path.is_absolute() {
        return path;
    }
    std::env::current_dir()
        .map(|cwd| cwd.join(&path))
        .unwrap_or(path)
}

/// 词法归约：折叠 `.` 与 `..`，不访问磁盘。
///
/// 逐级创建时目标还不存在，`canonicalize` 用不上；这里先把路径整理成
/// 「不会再逃出上级」的形态，再交给 [`Scope::resolve_tree`] 找祖先。
fn lexical(path: PathBuf) -> PathBuf {
    use std::path::Component;

    let mut out = PathBuf::new();
    for component in path.components() {
        match component {
            Component::CurDir => {}
            Component::ParentDir => {
                out.pop();
            }
            other => out.push(other.as_os_str()),
        }
    }
    out
}

fn normalize_root(root: &Path) -> PathBuf {
    let absolute = if root.is_absolute() {
        root.to_path_buf()
    } else {
        std::env::current_dir()
            .map(|cwd| cwd.join(root))
            .unwrap_or_else(|_| root.to_path_buf())
    };
    strip_unc_prefix(&std::fs::canonicalize(&absolute).unwrap_or(absolute))
}

/// 按路径分量比较，`/data` 不会把 `/database` 算作子目录。
fn is_under(path: &Path, root: &Path) -> bool {
    if path.starts_with(root) {
        return true;
    }
    // Windows 的文件系统不区分大小写，`D:\Data` 与 `d:\data` 是同一个目录。
    #[cfg(windows)]
    {
        let lower = |p: &Path| p.to_string_lossy().to_lowercase().replace('/', "\\");
        let path = lower(path);
        let root = lower(root);
        let root = root.trim_end_matches('\\');
        path == root
            || path
                .strip_prefix(root)
                .is_some_and(|rest| rest.starts_with('\\'))
    }
    #[cfg(not(windows))]
    {
        false
    }
}

fn same_path(left: &Path, right: &Path) -> bool {
    if left == right {
        return true;
    }
    #[cfg(windows)]
    {
        let normalize = |p: &Path| {
            p.to_string_lossy()
                .to_lowercase()
                .replace('/', "\\")
                .trim_end_matches('\\')
                .to_string()
        };
        normalize(left) == normalize(right)
    }
    #[cfg(not(windows))]
    {
        false
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::path_to_forward_slashes;
    use crate::tests::support::tempdir;

    #[test]
    fn component_wise_prefix_does_not_leak_into_siblings() {
        assert!(is_under(Path::new("/data/app"), Path::new("/data")));
        assert!(!is_under(Path::new("/database"), Path::new("/data")));
        assert!(is_under(Path::new("/data"), Path::new("/data")));
    }

    #[cfg(windows)]
    #[test]
    fn windows_paths_compare_case_insensitively() {
        assert!(is_under(Path::new(r"D:\Data\App"), Path::new(r"d:\data")));
        assert!(!is_under(Path::new(r"D:\Data2"), Path::new(r"d:\data")));
        assert!(same_path(Path::new(r"D:\Data\"), Path::new(r"d:\data")));
    }

    #[test]
    fn empty_root_list_allows_everywhere() {
        let scope = Scope::new(Vec::new(), false);
        assert!(scope.contains(Path::new("/anywhere")));
        assert!(
            scope.resolve_new("/anywhere/file.txt").is_err(),
            "父目录不存在时仍要报错"
        );
    }

    #[test]
    fn resolves_inside_the_root_and_rejects_siblings() {
        let dir = tempdir();
        let root = dir.path().join("root");
        std::fs::create_dir_all(root.join("inner")).expect("建目录");
        std::fs::write(root.join("inner/a.txt"), b"a").expect("写文件");
        let outside = dir.path().join("outside.txt");
        std::fs::write(&outside, b"b").expect("写文件");

        let scope = Scope::new(vec![root.clone()], true);
        let inside = scope
            .resolve(&path_to_forward_slashes(&root.join("inner/a.txt")))
            .expect("根目录内的文件应放行");
        assert!(inside.ends_with("a.txt"));

        let denied = scope
            .resolve(&path_to_forward_slashes(&outside))
            .expect_err("根目录之外的文件应拒绝");
        assert!(matches!(denied, GuguError::PathDenied(_)), "{denied}");

        let escape = root.join("..");
        assert!(
            scope.resolve(&path_to_forward_slashes(&escape)).is_err(),
            "`..` 解析成父目录后落在根之外，必须拒绝"
        );
        let nesting = path_to_forward_slashes(&root.join("x/y/z"));
        assert!(
            scope.resolve_tree(&nesting).is_ok(),
            "多级不存在的目录按最近的已存在祖先判定"
        );
        assert!(
            scope
                .resolve_tree(&path_to_forward_slashes(&dir.path().join("outside/x")))
                .is_err(),
            "祖先在作用域之外时同样拒绝"
        );
        let child = root.join("inner");
        assert!(scope
            .resolve_new(&path_to_forward_slashes(&child.join("new.txt")))
            .is_ok());
    }

    #[test]
    fn new_path_requires_an_existing_parent() {
        let dir = tempdir();
        let scope = Scope::new(vec![dir.path().to_path_buf()], true);
        let missing = dir.path().join("no-such-dir/file.txt");
        let error = scope
            .resolve_new(&path_to_forward_slashes(&missing))
            .expect_err("父目录不存在");
        assert!(error.to_string().contains("父目录不可用"), "{error}");

        assert!(scope.resolve_new("   ").is_err());
        let trailing = format!("{}/", path_to_forward_slashes(dir.path()));
        assert!(
            scope.resolve_new(&trailing).is_err(),
            "只有目录部分时没有文件名"
        );
    }

    #[test]
    fn readonly_scope_refuses_writes_before_touching_disk() {
        let scope = Scope::new(Vec::new(), false);
        assert!(matches!(scope.ensure_writable(), Err(GuguError::ReadOnly)));
        assert!(Scope::new(Vec::new(), true).ensure_writable().is_ok());
    }

    #[test]
    fn root_detection_marks_the_root_itself() {
        let dir = tempdir();
        let root = dir.path().join("root");
        std::fs::create_dir_all(root.join("child")).expect("建目录");
        let scope = Scope::new(vec![root.clone()], true);
        assert!(scope.is_root(&root));
        assert!(!scope.is_root(&root.join("child")));
    }
}
