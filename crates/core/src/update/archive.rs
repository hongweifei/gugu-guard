//! 解包发布归档：只取出可执行文件，归档内的目录结构与权限都不落地。
//!
//! 发布包是 `gugu-windows-x86_64.zip` 或 `gugu-linux-x86_64.tar.gz`，里面除可执行文件外
//! 还有 LICENSE、README 与示例配置。更新只关心二进制本身，因此按文件名白名单挑条目，
//! 并且写入固定目标名，条目里的 `../` 或绝对路径无从生效。

use std::fs::File;
use std::io::{copy, BufReader, Read};
use std::path::{Path, PathBuf};

/// 归档内可执行文件的候选名。
const BINARY_NAMES: &[&str] = &["gugu", "gugu.exe", "gugu-guard", "gugu-guard.exe"];

#[cfg(windows)]
const STAGED_NAME: &str = "gugu.exe";
#[cfg(unix)]
const STAGED_NAME: &str = "gugu";

/// 归档格式，按文件名后缀判定。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ArchiveKind {
    Zip,
    TarGz,
}

impl ArchiveKind {
    /// 从路径推断格式；无法识别时返回 None。
    #[must_use]
    pub fn of(path: &Path) -> Option<Self> {
        let name = path
            .file_name()
            .map(|name| name.to_string_lossy().to_ascii_lowercase())?;
        if name.ends_with(".zip") {
            return Some(Self::Zip);
        }
        if name.ends_with(".tar.gz") || name.ends_with(".tgz") {
            return Some(Self::TarGz);
        }
        None
    }
}

/// 把归档里的可执行文件解到 `dest` 目录，返回它的绝对路径。
///
/// # Errors
/// 归档打不开、条目读不出或里面没有可执行文件时返回原因。
pub fn unpack_binary(archive: &Path, dest: &Path) -> Result<PathBuf, String> {
    let kind = ArchiveKind::of(archive)
        .ok_or_else(|| format!("无法识别的归档格式: {}", archive.display()))?;
    std::fs::create_dir_all(dest)
        .map_err(|e| format!("创建暂存目录 {} 失败: {e}", dest.display()))?;
    let target = dest.join(STAGED_NAME);
    if target.exists() {
        std::fs::remove_file(&target)
            .map_err(|e| format!("清理旧暂存文件 {} 失败: {e}", target.display()))?;
    }

    match kind {
        ArchiveKind::Zip => unpack_zip(archive, &target)?,
        ArchiveKind::TarGz => unpack_tar_gz(archive, &target)?,
    }
    grant_execute(&target)?;
    Ok(target)
}

fn unpack_zip(archive: &Path, target: &Path) -> Result<(), String> {
    let file = File::open(archive).map_err(|e| format!("打开 {} 失败: {e}", archive.display()))?;
    let mut zip = zip::ZipArchive::new(file)
        .map_err(|e| format!("读取 zip {} 失败: {e}", archive.display()))?;
    for index in 0..zip.len() {
        let mut entry = zip
            .by_index(index)
            .map_err(|e| format!("读取 zip 第 {} 个条目失败: {e}", index + 1))?;
        if !is_binary_entry(entry.name()) {
            continue;
        }
        let mut out =
            File::create(target).map_err(|e| format!("写入 {} 失败: {e}", target.display()))?;
        copy(&mut entry, &mut out).map_err(|e| format!("解出 {} 失败: {e}", target.display()))?;
        return Ok(());
    }
    Err(format!("{} 里没有找到 gugu 可执行文件", archive.display()))
}

fn unpack_tar_gz(archive: &Path, target: &Path) -> Result<(), String> {
    let file = File::open(archive).map_err(|e| format!("打开 {} 失败: {e}", archive.display()))?;
    let decoder = flate2::read::GzDecoder::new(BufReader::new(file));
    let mut tar = tar::Archive::new(decoder);
    let entries = tar
        .entries()
        .map_err(|e| format!("读取 tar {} 失败: {e}", archive.display()))?;
    for entry in entries {
        let mut entry = entry.map_err(|e| format!("读取 tar 条目失败: {e}"))?;
        let path = entry
            .path()
            .map_err(|e| format!("读取 tar 条目名失败: {e}"))?;
        if !is_binary_entry(&path.to_string_lossy()) {
            continue;
        }
        let mut out =
            File::create(target).map_err(|e| format!("写入 {} 失败: {e}", target.display()))?;
        copy(&mut entry, &mut out).map_err(|e| format!("解出 {} 失败: {e}", target.display()))?;
        return Ok(());
    }
    Err(format!("{} 里没有找到 gugu 可执行文件", archive.display()))
}

/// 条目名只看最后一段：归档里的目录层级不参与判断，也不参与落盘。
fn is_binary_entry(name: &str) -> bool {
    let base = name.rsplit(['/', '\\']).next().unwrap_or_default();
    BINARY_NAMES.contains(&base)
}

#[cfg(unix)]
fn grant_execute(path: &Path) -> Result<(), String> {
    use std::os::unix::fs::PermissionsExt;
    let mut permissions = std::fs::metadata(path)
        .map_err(|e| format!("读取 {} 的权限失败: {e}", path.display()))?
        .permissions();
    permissions.set_mode(0o755);
    std::fs::set_permissions(path, permissions)
        .map_err(|e| format!("设置 {} 的可执行权限失败: {e}", path.display()))
}

#[cfg(windows)]
fn grant_execute(_path: &Path) -> Result<(), String> {
    Ok(())
}

/// 读一遍归档开头的魔数，用于日志里核对下载内容确实是压缩包。
#[must_use]
pub fn looks_like_archive(path: &Path) -> bool {
    let Ok(mut file) = File::open(path) else {
        return false;
    };
    let mut head = [0u8; 4];
    if file.read_exact(&mut head).is_err() {
        return false;
    }
    head.starts_with(b"PK\x03\x04") || head.starts_with(&[0x1f, 0x8b])
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;

    fn temp_dir(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("gugu-archive-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).expect("建临时目录");
        dir
    }

    fn write_zip(path: &Path, entries: &[(&str, &[u8])]) {
        let file = File::create(path).expect("建 zip");
        let mut writer = zip::ZipWriter::new(file);
        let options = zip::write::SimpleFileOptions::default()
            .compression_method(zip::CompressionMethod::Deflated);
        for (name, data) in entries {
            writer.start_file(*name, options).expect("写入条目");
            writer.write_all(data).expect("写数据");
        }
        writer.finish().expect("收尾");
    }

    #[test]
    fn extracts_the_binary_from_nested_paths() {
        let dir = temp_dir("zip");
        let archive = dir.join("gugu-windows-x86_64.zip");
        write_zip(
            &archive,
            &[
                ("gugu-windows-x86_64/README.md", b"readme"),
                ("gugu-windows-x86_64/gugu.exe", b"binary"),
                ("../../evil.txt", b"evil"),
            ],
        );
        let staged = unpack_binary(&archive, &dir.join("out")).expect("解包成功");
        assert!(staged.ends_with(STAGED_NAME));
        assert_eq!(std::fs::read(&staged).expect("读出"), b"binary");
        assert!(!dir.join("evil.txt").exists(), "条目路径不应落地");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn rejects_archives_without_the_binary() {
        let dir = temp_dir("missing");
        let archive = dir.join("gugu-linux-x86_64.zip");
        write_zip(&archive, &[("README.md", b"readme")]);
        let err = unpack_binary(&archive, &dir.join("out")).expect_err("应报缺可执行文件");
        assert!(err.contains("没有找到"), "{err}");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn reads_tar_gz() {
        let dir = temp_dir("targz");
        let archive = dir.join("gugu-linux-x86_64.tar.gz");
        let file = File::create(&archive).expect("建 tar.gz");
        let encoder = flate2::write::GzEncoder::new(file, flate2::Compression::fast());
        let mut builder = tar::Builder::new(encoder);
        let data = b"elf-binary";
        let mut header = tar::Header::new_gnu();
        header.set_size(data.len() as u64);
        header.set_mode(0o644);
        header.set_cksum();
        builder
            .append_data(&mut header, "gugu-linux-x86_64/gugu", &data[..])
            .expect("写入 tar 条目");
        builder
            .into_inner()
            .expect("收尾")
            .finish()
            .expect("压缩收尾");

        let staged = unpack_binary(&archive, &dir.join("out")).expect("解包成功");
        assert_eq!(std::fs::read(&staged).expect("读出"), data);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn detects_archive_by_extension() {
        assert_eq!(ArchiveKind::of(Path::new("a.zip")), Some(ArchiveKind::Zip));
        assert_eq!(
            ArchiveKind::of(Path::new("a.tar.gz")),
            Some(ArchiveKind::TarGz)
        );
        assert_eq!(ArchiveKind::of(Path::new("a.exe")), None);
    }
}
