//! 目录打包下载：先在系统临时目录生成 zip，再分块发送。
//!
//! zip 写入器要回填中央目录，因而不能直接写进 HTTP 响应体；这里先生成临时文件，
//! 响应结束后由 [`TempGuard`] 删掉。符号链接按链接自身跳过，链接指向哪里都不会被打进去。
//!
//! 目录默认套一层同名目录，`flat=true` 时内容直接位于 zip 根；单个文件打成一个条目。
//! 跨节点搬运复用同一套打包，见 [`bundle`]。

use std::io::{Seek, Write};
use std::path::{Path, PathBuf};
use std::sync::Arc;

use axum::body::{Body, Bytes};
use axum::extract::{Query, State};
use axum::http::{header, StatusCode};
use axum::response::{IntoResponse, Response};
use futures::Stream;
use gugu_core::error::GuguError;
use gugu_core::fs::TEMP_PREFIX;
use tokio::io::AsyncReadExt;
use zip::write::SimpleFileOptions;

use super::types::ArchiveQuery;
use super::{disposition, failure};
use crate::state::AppState;

/// 单个归档的原始字节上限：临时文件与响应各要走一遍完整数据，超过这个体量请分目录下载。
const MAX_ARCHIVE_BYTES: u64 = 2 * 1024 * 1024 * 1024;
/// 每次从临时文件读出的字节数。
const CHUNK: usize = 64 * 1024;

pub(super) async fn archive_fs(
    State(state): State<AppState>,
    Query(query): Query<ArchiveQuery>,
) -> Response {
    let Some(target) = query.path.filter(|path| !path.trim().is_empty()) else {
        return failure(GuguError::InvalidPath("缺少 path 参数".to_string()));
    };
    let source = match state.fs.resolve(&target) {
        Ok(path) => path,
        Err(error) => return failure(error),
    };
    let flat = query.flat;

    let built = tokio::task::spawn_blocking(move || build(&source, flat)).await;
    let Built { temp, size, name } = match built {
        Ok(Ok(built)) => built,
        Ok(Err(error)) => return failure(error),
        Err(error) => return failure(GuguError::InvalidPath(format!("打包被中断: {error}"))),
    };

    let file = match tokio::fs::File::open(&temp.path).await {
        Ok(file) => file,
        Err(error) => return failure(GuguError::Io(error)),
    };
    Response::builder()
        .header(header::CONTENT_TYPE, "application/zip")
        .header(header::CONTENT_LENGTH, size)
        .header(
            header::CONTENT_DISPOSITION,
            disposition(&format!("{name}.zip")),
        )
        .body(Body::from_stream(stream(file, temp)))
        .unwrap_or_else(|_| (StatusCode::INTERNAL_SERVER_ERROR, "构造打包响应失败").into_response())
}

/// 临时文件在最后一个数据块被读完之后删除。
struct TempGuard {
    path: PathBuf,
}

impl Drop for TempGuard {
    fn drop(&mut self) {
        let _ = std::fs::remove_file(&self.path);
    }
}

struct Built {
    temp: Arc<TempGuard>,
    size: u64,
    name: String,
}

/// 按块读出临时文件；`guard` 跟随流一起释放。
fn stream(
    file: tokio::fs::File,
    guard: Arc<TempGuard>,
) -> impl Stream<Item = std::io::Result<Bytes>> + Send + 'static {
    futures::stream::unfold((file, guard), |(mut file, guard)| async move {
        let mut buffer = vec![0_u8; CHUNK];
        match file.read(&mut buffer).await {
            Ok(0) => None,
            Ok(read) => {
                buffer.truncate(read);
                Some((Ok(Bytes::from(buffer)), (file, guard)))
            }
            Err(error) => Some((Err(error), (file, guard))),
        }
    })
}

fn build(source: &Path, flat: bool) -> gugu_core::Result<Built> {
    let name = source_name(source);
    let path = temp_path();
    match bundle(source, flat, &path) {
        Ok(size) => Ok(Built {
            temp: Arc::new(TempGuard { path }),
            size,
            name,
        }),
        Err(error) => {
            let _ = std::fs::remove_file(&path);
            Err(error)
        }
    }
}

/// 把来源（目录或单个文件）打包到指定文件，返回归档字节数。
///
/// 目录在 `flat` 时把内容直接放在 zip 根，否则套一层同名目录；单个文件打成一个条目。
pub(super) fn bundle(source: &Path, flat: bool, path: &Path) -> gugu_core::Result<u64> {
    measure(source, 0)?;
    write_zip(source, &source_name(source), flat, path)
}

fn source_name(source: &Path) -> String {
    source
        .file_name()
        .map(|name| name.to_string_lossy().to_string())
        .unwrap_or_else(|| "archive".to_string())
}

/// 统计待打包的原始字节：先量一遍，超限时不必写半个归档再删。
fn measure(source: &Path, total: u64) -> gugu_core::Result<u64> {
    let meta = std::fs::symlink_metadata(source)?;
    if !meta.is_dir() {
        return checked(total.saturating_add(meta.len()));
    }
    let mut total = total;
    for item in std::fs::read_dir(source)? {
        let item = item?;
        let file_type = item.file_type()?;
        if file_type.is_symlink() || skipped(&item.file_name()) {
            continue;
        }
        total = measure(&item.path(), total)?;
    }
    Ok(total)
}

fn checked(total: u64) -> gugu_core::Result<u64> {
    if total > MAX_ARCHIVE_BYTES {
        return Err(GuguError::InvalidPath(format!(
            "内容超过 {} GB，请分目录下载",
            MAX_ARCHIVE_BYTES / 1024 / 1024 / 1024
        )));
    }
    Ok(total)
}

fn write_zip(source: &Path, top: &str, flat: bool, path: &Path) -> gugu_core::Result<u64> {
    let file = std::fs::File::create(path)?;
    let mut zip = zip::ZipWriter::new(file);
    let options = SimpleFileOptions::default().compression_method(zip::CompressionMethod::Deflated);
    if std::fs::symlink_metadata(source)?.is_dir() {
        let prefix = if flat {
            String::new()
        } else {
            add_dir(&mut zip, top, options)?;
            top.to_string()
        };
        walk(&mut zip, source, &prefix, options)?;
    } else {
        zip.start_file(top, options).map_err(pack)?;
        let mut reader = std::fs::File::open(source)?;
        std::io::copy(&mut reader, &mut zip)?;
    }

    let mut file = zip.finish().map_err(pack)?;
    file.flush()?;
    Ok(file.metadata()?.len())
}

fn walk<W: Write + Seek>(
    zip: &mut zip::ZipWriter<W>,
    root: &Path,
    prefix: &str,
    options: SimpleFileOptions,
) -> gugu_core::Result<()> {
    let mut stack = vec![root.to_path_buf()];
    while let Some(dir) = stack.pop() {
        for item in std::fs::read_dir(&dir)? {
            let item = item?;
            let file_type = item.file_type()?;
            if file_type.is_symlink() || skipped(&item.file_name()) {
                continue;
            }
            let path = item.path();
            let rel = path.strip_prefix(root).map_err(|_| {
                GuguError::InvalidPath(format!("路径不在打包目录内: {}", path.display()))
            })?;
            let rel = rel.to_string_lossy().replace('\\', "/");
            let name = if prefix.is_empty() {
                rel
            } else {
                format!("{prefix}/{rel}")
            };
            if file_type.is_dir() {
                add_dir(zip, &name, options)?;
                stack.push(path);
            } else if file_type.is_file() {
                zip.start_file(name, options).map_err(pack)?;
                let mut source = std::fs::File::open(&path)?;
                std::io::copy(&mut source, zip)?;
            }
        }
    }
    Ok(())
}

fn add_dir<W: Write + Seek>(
    zip: &mut zip::ZipWriter<W>,
    name: &str,
    options: SimpleFileOptions,
) -> gugu_core::Result<()> {
    zip.add_directory(format!("{name}/"), options).map_err(pack)
}

/// 传输中转文件不进归档。
fn skipped(name: &std::ffi::OsStr) -> bool {
    name.to_string_lossy().starts_with(TEMP_PREFIX)
}

fn pack(error: zip::result::ZipError) -> GuguError {
    GuguError::InvalidPath(format!("打包失败: {error}"))
}

fn temp_path() -> PathBuf {
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|duration| duration.as_nanos())
        .unwrap_or(0);
    std::env::temp_dir().join(format!(
        "{TEMP_PREFIX}archive-{}-{nanos}",
        std::process::id()
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 测试用临时目录：离开作用域时整棵删掉。
    struct Sandbox(PathBuf);

    impl Sandbox {
        fn new() -> Self {
            let path = temp_path();
            std::fs::create_dir_all(&path).expect("建临时目录");
            Self(path)
        }

        fn path(&self) -> &Path {
            &self.0
        }
    }

    impl Drop for Sandbox {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    #[test]
    fn packs_a_tree_and_skips_transfer_leftovers() {
        let sandbox = Sandbox::new();
        let root = sandbox.path().join("打包 目录");
        std::fs::create_dir_all(root.join("子目录")).expect("建目录");
        std::fs::write(root.join("a.txt"), b"alpha").expect("写文件");
        std::fs::write(root.join("子目录/b.txt"), b"beta").expect("写文件");
        std::fs::write(root.join(format!("{TEMP_PREFIX}part-1")), b"half").expect("写临时文件");

        let path = sandbox.path().join("out.zip");
        let size = write_zip(&root, "打包 目录", false, &path).expect("打包成功");
        assert!(size > 0);

        let file = std::fs::File::open(&path).expect("打开归档");
        let mut archive = zip::ZipArchive::new(file).expect("读取归档");
        let names: Vec<String> = (0..archive.len())
            .map(|index| {
                archive
                    .by_index(index)
                    .expect("条目可读")
                    .name()
                    .to_string()
            })
            .collect();
        assert!(names.contains(&"打包 目录/".to_string()), "{names:?}");
        assert!(names.contains(&"打包 目录/a.txt".to_string()), "{names:?}");
        assert!(
            names.contains(&"打包 目录/子目录/b.txt".to_string()),
            "{names:?}"
        );
        assert!(
            !names.iter().any(|name| name.contains(TEMP_PREFIX)),
            "中转文件不该进归档: {names:?}"
        );
    }

    #[test]
    fn flat_bundles_lay_content_at_the_root() {
        let sandbox = Sandbox::new();
        let root = sandbox.path().join("src");
        std::fs::create_dir_all(root.join("sub")).expect("建目录");
        std::fs::write(root.join("a.txt"), b"alpha").expect("写文件");
        std::fs::write(root.join("sub/b.txt"), b"beta").expect("写文件");

        let path = sandbox.path().join("flat.zip");
        bundle(&root, true, &path).expect("平铺打包");
        let names = entry_names(&path);
        assert!(names.contains(&"a.txt".to_string()), "{names:?}");
        assert!(names.contains(&"sub/b.txt".to_string()), "{names:?}");
        assert!(
            !names.iter().any(|name| name.starts_with("src/")),
            "平铺打包不该带顶层目录: {names:?}"
        );
    }

    #[test]
    fn single_files_become_one_entry() {
        let sandbox = Sandbox::new();
        let file = sandbox.path().join("清单.txt");
        std::fs::write(&file, b"rows").expect("写文件");

        let path = sandbox.path().join("one.zip");
        bundle(&file, true, &path).expect("打包单文件");
        assert_eq!(entry_names(&path), vec!["清单.txt".to_string()]);
    }

    fn entry_names(path: &Path) -> Vec<String> {
        let file = std::fs::File::open(path).expect("打开归档");
        let mut archive = zip::ZipArchive::new(file).expect("读取归档");
        (0..archive.len())
            .map(|index| {
                archive
                    .by_index(index)
                    .expect("条目可读")
                    .name()
                    .to_string()
            })
            .collect()
    }

    #[test]
    fn oversize_trees_are_refused_before_writing() {
        let sandbox = Sandbox::new();
        std::fs::write(sandbox.path().join("a.txt"), b"tiny").expect("写文件");
        let error = measure(sandbox.path(), MAX_ARCHIVE_BYTES).expect_err("已超限");
        assert!(error.to_string().contains("分目录下载"), "{error}");
    }

    #[test]
    fn temp_names_live_outside_the_managed_tree() {
        let path = temp_path();
        assert_eq!(path.parent(), Some(std::env::temp_dir().as_path()));
        assert!(path
            .file_name()
            .expect("有文件名")
            .to_string_lossy()
            .starts_with(TEMP_PREFIX));
        assert_ne!(path, temp_path());
    }
}
