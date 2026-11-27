//! 日志轮转的文件命名：写盘侧（`process::capture`）与检索侧（`logs::scan`）共用一套规则。
//!
//! `app.log` 的备份依次是 `app.1.log` … `app.6.log`，数字越大越旧：
//! 轮转时 `.5` 让位给 `.6`、`.1` 让位给 `.2`，当前文件改名成 `.1`；
//! 只有 [`MAX_BACKUPS`] 份，更旧的一份每次被覆盖。

use std::path::{Path, PathBuf};

/// 保留的备份份数（`.1` 到 `.6`），与轮转循环的上界一致。
pub const MAX_BACKUPS: u32 = 6;

/// 第 `n` 份备份的路径；`n` 从 1 开始，1 是最新的一份。
#[must_use]
pub fn rotated_path(path: &Path, n: u32) -> PathBuf {
    let stem = path
        .file_stem()
        .map(|s| s.to_string_lossy().to_string())
        .unwrap_or_default();
    let parent = path.parent().unwrap_or(Path::new("."));
    match path.extension().map(|e| e.to_string_lossy().to_string()) {
        Some(ext) => parent.join(format!("{stem}.{n}.{ext}")),
        None => parent.join(format!("{stem}.{n}")),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn keeps_the_extension() {
        assert_eq!(
            rotated_path(Path::new("out.log"), 1),
            Path::new("out.1.log")
        );
        assert_eq!(
            rotated_path(Path::new("/var/log/app/out.log"), 6),
            Path::new("/var/log/app/out.6.log")
        );
    }

    #[test]
    fn handles_paths_without_extension_and_multiple_dots() {
        assert_eq!(rotated_path(Path::new("out"), 2), Path::new("out.2"));
        assert_eq!(
            rotated_path(Path::new("access.2026-11-27.log"), 3),
            Path::new("access.2026-11-27.3.log")
        );
    }
}
