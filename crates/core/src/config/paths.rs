//! 配置文件与工作路径的查找、规范化与相对化。

use std::path::PathBuf;

/// 默认配置文件名。
pub const CONFIG_FILE_NAME: &str = "gugu.toml";

/// 按绿色软件的查找顺序选择默认配置文件位置（纯函数，便于测试）：
/// 1. 程序同目录下的 gugu.toml —— 配置跟着程序走；
/// 2. 当前工作目录下的 gugu.toml —— 兼容在项目目录里直接运行；
/// 3. 两者都不存在时，返回程序同目录路径作为“应当放置/生成配置”的位置；
///    若拿不到程序目录，退回当前工作目录。
#[must_use]
pub fn select_config_path(
    exe_dir: Option<&std::path::Path>,
    cwd: &std::path::Path,
    file_exists: &dyn Fn(&std::path::Path) -> bool,
) -> PathBuf {
    if let Some(dir) = exe_dir {
        let candidate = dir.join(CONFIG_FILE_NAME);
        if file_exists(&candidate) {
            return candidate;
        }
    }
    let cwd_candidate = cwd.join(CONFIG_FILE_NAME);
    if file_exists(&cwd_candidate) {
        return cwd_candidate;
    }
    exe_dir.unwrap_or(cwd).join(CONFIG_FILE_NAME)
}

/// 解析默认配置文件路径（绝对路径）。
#[must_use]
pub fn default_config_path() -> PathBuf {
    let exe_dir = std::env::current_exe()
        .ok()
        .and_then(|p| p.parent().map(std::path::Path::to_path_buf));
    let cwd = std::env::current_dir().unwrap_or_else(|_| PathBuf::from("."));
    select_config_path(exe_dir.as_deref(), &cwd, &|p| p.is_file())
}

#[must_use]
pub fn canonicalize_clean(path: &std::path::Path) -> PathBuf {
    std::fs::canonicalize(path)
        .map(|p| strip_unc_prefix(&p))
        .unwrap_or_else(|_| path.to_path_buf())
}

#[must_use]
pub fn strip_unc_prefix(path: &std::path::Path) -> PathBuf {
    let s = path.to_string_lossy();
    match s.strip_prefix(r"\\?\UNC\") {
        Some(rest) => PathBuf::from(format!("\\\\{rest}")),
        None => match s.strip_prefix(r"\\?\") {
            Some(rest) => PathBuf::from(rest),
            None => path.to_path_buf(),
        },
    }
}

#[must_use]
pub fn resolve_relative_path(path: &std::path::Path, base: &std::path::Path) -> PathBuf {
    if path.is_absolute() {
        path.to_path_buf()
    } else {
        base.join(path)
    }
}

#[must_use]
pub fn path_to_forward_slashes(path: &std::path::Path) -> String {
    path.to_string_lossy().replace('\\', "/")
}

/// 若 `path` 位于 `base` 目录下，返回其相对路径 (正斜杠形式)；否则原样返回。
/// 用于把内存中已解析的绝对路径回写成配置里的相对路径 (绿色部署可搬移)。
#[must_use]
pub fn relativize_path(path: &std::path::Path, base: &std::path::Path) -> PathBuf {
    let ps = path_to_forward_slashes(path);
    let bs = path_to_forward_slashes(base);
    let bs = bs.trim_end_matches('/');
    if bs.is_empty() {
        return path.to_path_buf();
    }
    let prefix_len = bs.len() + 1; // base + '/'
    if ps.len() > prefix_len
        && ps.as_bytes()[prefix_len - 1] == b'/'
        && ps[..bs.len()].eq_ignore_ascii_case(bs)
    {
        PathBuf::from(&ps[prefix_len..])
    } else {
        path.to_path_buf()
    }
}
