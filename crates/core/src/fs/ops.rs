//! 结构性操作：新建目录与文件、改名/移动、复制、删除。
//!
//! 每个入口先过 [`Scope`] 的越界与写权限检查，再动磁盘；
//! 目标已存在时默认拒绝，覆盖必须由调用方显式声明。

use std::path::Path;

use crate::error::{GuguError, Result};

use super::Scope;

/// 新建目录。
///
/// `parents` 为 true 时逐级创建，已存在不报错；为 false 时只建一层，已存在即失败。
///
/// # Errors
/// 只读模式、路径越界、父目录缺失或目标已存在时返回错误。
pub fn create_dir(scope: &Scope, raw: &str, parents: bool) -> Result<()> {
    scope.ensure_writable()?;
    let path = if parents {
        scope.resolve_tree(raw)?
    } else {
        scope.resolve_new(raw)?
    };
    if path.exists() {
        if parents && path.is_dir() {
            return Ok(());
        }
        return Err(GuguError::InvalidPath(format!(
            "目标已存在: {}",
            path.display()
        )));
    }
    let result = if parents {
        std::fs::create_dir_all(&path)
    } else {
        std::fs::create_dir(&path)
    };
    result.map_err(GuguError::Io)
}

/// 新建空文件；目标已存在时失败，不覆盖既有内容。
///
/// # Errors
/// 只读模式、路径越界或目标已存在时返回错误。
pub fn create_file(scope: &Scope, raw: &str) -> Result<()> {
    scope.ensure_writable()?;
    let path = scope.resolve_new(raw)?;
    if path.exists() {
        return Err(GuguError::InvalidPath(format!(
            "目标已存在: {}",
            path.display()
        )));
    }
    std::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(&path)
        .map(|_| ())
        .map_err(GuguError::Io)
}

/// 改名或移动；`overwrite` 为 false 时目标存在即失败。
///
/// 目标在同一个磁盘上走一次改名；跨磁盘时改名会失败，此时回退为「复制 + 删除源」。
///
/// # Errors
/// 只读模式、路径越界、源不存在、目标已存在（未声明覆盖）或目标位于源目录之内时返回错误。
pub fn relocate(scope: &Scope, from: &str, to: &str, overwrite: bool) -> Result<()> {
    scope.ensure_writable()?;
    let source = scope.resolve(from)?;
    let target = scope.resolve_new(to)?;
    check_pair(scope, &source, &target)?;
    clear_target(&target, overwrite)?;

    match std::fs::rename(&source, &target) {
        Ok(()) => Ok(()),
        Err(error) if is_cross_device(&error) => {
            copy_tree(&source, &target)?;
            remove_path(&source, true)
        }
        Err(error) => Err(GuguError::Io(error)),
    }
}

/// 复制文件或目录；目录递归复制。
///
/// # Errors
/// 只读模式、路径越界、源不存在、目标已存在（未声明覆盖）或目标位于源目录之内时返回错误。
pub fn copy(scope: &Scope, from: &str, to: &str, overwrite: bool) -> Result<()> {
    scope.ensure_writable()?;
    let source = scope.resolve(from)?;
    let target = scope.resolve_new(to)?;
    check_pair(scope, &source, &target)?;
    clear_target(&target, overwrite)?;
    copy_tree(&source, &target)
}

/// 删除文件或目录；目录非空时必须声明递归。
///
/// # Errors
/// 只读模式、路径越界、路径不存在、目录非空（未声明递归）或目标是作用域根目录时返回错误。
pub fn remove(scope: &Scope, raw: &str, recursive: bool) -> Result<()> {
    scope.ensure_writable()?;
    let path = scope.resolve(raw)?;
    if scope.is_root(&path) {
        return Err(GuguError::InvalidPath(format!(
            "不能删除作用域根目录: {}",
            path.display()
        )));
    }
    remove_path(&path, recursive)
}

/// 源与目标的共同前置检查。
fn check_pair(scope: &Scope, source: &Path, target: &Path) -> Result<()> {
    if source == target {
        return Err(GuguError::InvalidPath("源与目标是同一个路径".to_string()));
    }
    if scope.is_root(source) {
        return Err(GuguError::InvalidPath(format!(
            "不能移动或复制作用域根目录: {}",
            source.display()
        )));
    }
    if source.is_dir() && target.starts_with(source) {
        return Err(GuguError::InvalidPath(
            "目标位于源目录之内，这样会无限递归".to_string(),
        ));
    }
    Ok(())
}

/// 目标已存在时按调用方的覆盖意图处理。
fn clear_target(target: &Path, overwrite: bool) -> Result<()> {
    if !target.exists() {
        return Ok(());
    }
    if !overwrite {
        return Err(GuguError::InvalidPath(format!(
            "目标已存在: {}（需要覆盖请显式声明）",
            target.display()
        )));
    }
    remove_path(target, true)
}

fn remove_path(path: &Path, recursive: bool) -> Result<()> {
    let meta = std::fs::symlink_metadata(path)?;
    if meta.file_type().is_symlink() {
        // 删除链接本身，不去跟随到目标；Windows 的目录链接要用 remove_dir。
        return std::fs::remove_file(path)
            .or_else(|_| std::fs::remove_dir(path))
            .map_err(GuguError::Io);
    }
    if meta.is_dir() {
        if recursive {
            return std::fs::remove_dir_all(path).map_err(GuguError::Io);
        }
        return std::fs::remove_dir(path).map_err(|error| {
            if error.to_string().contains("not empty")
                || error.raw_os_error() == Some(145)
                || error.raw_os_error() == Some(39)
            {
                GuguError::InvalidPath(format!(
                    "目录非空: {}（需要递归删除请显式声明）",
                    path.display()
                ))
            } else {
                GuguError::Io(error)
            }
        });
    }
    std::fs::remove_file(path).map_err(GuguError::Io)
}

fn copy_tree(source: &Path, target: &Path) -> Result<()> {
    let meta = std::fs::symlink_metadata(source)?;
    if !meta.is_dir() {
        // 符号链接与普通文件都按内容复制，目标得到一份独立副本。
        std::fs::copy(source, target)?;
        return Ok(());
    }
    std::fs::create_dir_all(target)?;
    for item in std::fs::read_dir(source)? {
        let item = item?;
        let child = target.join(item.file_name());
        copy_tree(&item.path(), &child)?;
    }
    Ok(())
}

/// 跨磁盘改名的错误码：Unix 是 EXDEV(18)，Windows 是 ERROR_NOT_SAME_DEVICE(17)。
fn is_cross_device(error: &std::io::Error) -> bool {
    matches!(error.raw_os_error(), Some(18) | Some(17))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::path_to_forward_slashes;
    use crate::tests::support::tempdir;

    fn slash(path: &Path) -> String {
        path_to_forward_slashes(path)
    }

    #[test]
    fn creates_directories_and_refuses_to_clobber() {
        let dir = tempdir();
        let scope = Scope::new(vec![dir.path().to_path_buf()], true);
        let nested = dir.path().join("a/b");
        create_dir(&scope, &slash(&nested), true).expect("逐级建目录");
        assert!(nested.is_dir());

        let error = create_dir(&scope, &slash(&nested), false).expect_err("已存在应失败");
        assert!(error.to_string().contains("已存在"), "{error}");
        create_dir(&scope, &slash(&nested), true).expect("parents 模式下已存在不算错");
    }

    #[test]
    fn creates_files_but_never_overwrites() {
        let dir = tempdir();
        let scope = Scope::new(vec![dir.path().to_path_buf()], true);
        let file = dir.path().join("new.txt");
        create_file(&scope, &slash(&file)).expect("新建");
        assert!(file.is_file());
        let error = create_file(&scope, &slash(&file)).expect_err("已存在应失败");
        assert!(error.to_string().contains("已存在"), "{error}");
    }

    #[test]
    fn renames_and_moves_with_an_explicit_overwrite() {
        let dir = tempdir();
        let scope = Scope::new(vec![dir.path().to_path_buf()], true);
        let from = dir.path().join("a.txt");
        let to = dir.path().join("b.txt");
        std::fs::write(&from, b"a").expect("写文件");

        relocate(&scope, &slash(&from), &slash(&to), false).expect("改名");
        assert!(!from.exists() && to.is_file());

        std::fs::write(&from, b"a2").expect("写文件");
        let error = relocate(&scope, &slash(&from), &slash(&to), false).expect_err("目标已存在");
        assert!(error.to_string().contains("目标已存在"), "{error}");
        relocate(&scope, &slash(&from), &slash(&to), true).expect("显式覆盖");
        assert_eq!(std::fs::read(&to).expect("读回"), b"a2");
    }

    #[test]
    fn copies_a_directory_tree_and_blocks_recursive_targets() {
        let dir = tempdir();
        let scope = Scope::new(vec![dir.path().to_path_buf()], true);
        let source = dir.path().join("src");
        std::fs::create_dir_all(source.join("sub")).expect("建目录");
        std::fs::write(source.join("sub/file.txt"), b"x").expect("写文件");

        let target = dir.path().join("dst");
        copy(&scope, &slash(&source), &slash(&target), false).expect("复制目录");
        assert!(target.join("sub/file.txt").is_file());

        let inside = source.join("inner");
        let error = copy(&scope, &slash(&source), &slash(&inside), false).expect_err("禁止自嵌套");
        assert!(error.to_string().contains("之内"), "{error}");
    }

    #[test]
    fn removing_a_non_empty_directory_needs_recursion() {
        let dir = tempdir();
        let scope = Scope::new(vec![dir.path().to_path_buf()], true);
        let folder = dir.path().join("folder");
        std::fs::create_dir_all(&folder).expect("建目录");
        std::fs::write(folder.join("a.txt"), b"a").expect("写文件");

        let error = remove(&scope, &slash(&folder), false).expect_err("非空目录应拒绝");
        assert!(error.to_string().contains("非空"), "{error}");
        remove(&scope, &slash(&folder), true).expect("递归删除");
        assert!(!folder.exists());
    }

    #[test]
    fn the_scope_root_itself_cannot_be_removed_or_moved() {
        let dir = tempdir();
        let root = dir.path().join("root");
        std::fs::create_dir_all(&root).expect("建目录");
        let scope = Scope::new(vec![root.clone()], true);

        let error = remove(&scope, &slash(&root), true).expect_err("根目录不可删");
        assert!(error.to_string().contains("根目录"), "{error}");
        let moved = root.join("moved");
        let error =
            relocate(&scope, &slash(&root), &slash(&moved), false).expect_err("根目录不可移动");
        assert!(error.to_string().contains("根目录"), "{error}");
    }

    #[test]
    fn readonly_scope_blocks_every_mutation() {
        let dir = tempdir();
        let scope = Scope::new(vec![dir.path().to_path_buf()], false);
        let target = slash(&dir.path().join("x"));
        assert!(matches!(
            create_dir(&scope, &target, false),
            Err(GuguError::ReadOnly)
        ));
        assert!(matches!(
            create_file(&scope, &target),
            Err(GuguError::ReadOnly)
        ));
    }
}
