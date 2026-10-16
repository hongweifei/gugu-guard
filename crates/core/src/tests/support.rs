//! 测试用临时目录。
//!
//! 不引入 `tempfile` 依赖：它会把 getrandom 等一串传递依赖带进依赖树，
//! 而测试只需要「建唯一目录 + Drop 时删掉」这点能力。

use std::path::{Path, PathBuf};

pub(crate) struct TempDir {
    path: PathBuf,
}

impl TempDir {
    pub(crate) fn path(&self) -> &Path {
        &self.path
    }
}

impl Drop for TempDir {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.path);
    }
}

/// 在系统临时目录下创建唯一目录（进程号 + 纳秒 + 序号），Drop 时递归删除。
pub(crate) fn tempdir() -> TempDir {
    use std::sync::atomic::{AtomicU64, Ordering};
    static SEQ: AtomicU64 = AtomicU64::new(0);

    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_nanos())
        .unwrap_or(0);
    let seq = SEQ.fetch_add(1, Ordering::Relaxed);
    let path = std::env::temp_dir()
        .join(format!("gugu-test-{}-{nanos}-{seq}", std::process::id()));
    std::fs::create_dir_all(&path).expect("创建测试临时目录失败");
    TempDir { path }
}
