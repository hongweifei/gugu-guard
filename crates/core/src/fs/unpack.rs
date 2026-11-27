//! 解压归档：zip 与 tar（含 gzip）。
//!
//! 目标目录由调用方给出，缺省是归档旁的同名目录（`foo.zip` → `foo/`）。
//! 解压先落到目标旁的临时目录，全部条目成功后再整体改名就位：
//! 中途失败不会留下半个目录，覆盖已有目录时也不会先删掉旧内容。
//!
//! 条目名一律按相对路径重新拼装，`..`、绝对路径与盘符条目直接跳过并计数；
//! 符号链接、设备文件等特殊条目同样跳过，不让归档借链接指向白名单之外。

use std::fs::{self, File, OpenOptions};
use std::io::{self, BufReader, Read, Write};
use std::path::{Path, PathBuf};

use crate::config::path_to_forward_slashes;
use crate::error::{GuguError, Result};

use super::{Scope, TEMP_PREFIX};

/// 单个归档解压后的总字节上限。压缩率不设限，靠这个总量兜住压缩炸弹。
pub const MAX_EXTRACT_BYTES: u64 = 2 * 1024 * 1024 * 1024;
/// 条目数上限：大量空目录同样会拖垮文件系统。
const MAX_ENTRIES: usize = 20_000;
/// 条目名上限，超长的名字多为恶意构造。
const MAX_NAME_BYTES: usize = 1024;

/// 归档格式，按文件名后缀判定。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ArchiveKind {
    Zip,
    Tar,
    TarGz,
}

impl ArchiveKind {
    fn of(path: &Path) -> Option<Self> {
        let name = path.file_name()?.to_string_lossy().to_ascii_lowercase();
        if name.ends_with(".zip") {
            return Some(Self::Zip);
        }
        if name.ends_with(".tar.gz") || name.ends_with(".tgz") {
            return Some(Self::TarGz);
        }
        if name.ends_with(".tar") {
            return Some(Self::Tar);
        }
        None
    }

    /// 去掉本格式的后缀，得到缺省目标目录名。
    fn stem(self, name: &str) -> String {
        let lower = name.to_ascii_lowercase();
        let suffixes: &[&str] = match self {
            Self::Zip => &[".zip"],
            Self::Tar => &[".tar"],
            Self::TarGz => &[".tar.gz", ".tgz"],
        };
        for suffix in suffixes {
            if lower.ends_with(suffix) {
                return name[..name.len() - suffix.len()].to_string();
            }
        }
        name.to_string()
    }
}

/// 解压结果：目标目录、落地条目数与跳过的条目数。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Extracted {
    pub path: String,
    pub files: usize,
    pub dirs: usize,
    pub bytes: u64,
    pub skipped: usize,
}

impl Extracted {
    /// 一句话结果，接口与 CLI 共用。
    #[must_use]
    pub fn summary(&self) -> String {
        let mut message = format!(
            "已解压 {} 个文件、{} 个目录（{} 字节）→ {}",
            self.files, self.dirs, self.bytes, self.path
        );
        if self.skipped > 0 {
            message.push_str(&format!("，跳过 {} 个特殊条目", self.skipped));
        }
        message
    }
}

/// 解压归档到目标目录。
///
/// `dest` 为空时解到归档旁的同名目录；目标已存在时必须显式声明 `overwrite`，
/// 覆盖是整目录替换，不与旧内容合并。
///
/// # Errors
/// 只读模式、路径越界、格式不支持、目标已存在、归档损坏或解压后超过上限时返回错误。
pub fn extract(
    scope: &Scope,
    archive: &str,
    dest: Option<&str>,
    overwrite: bool,
) -> Result<Extracted> {
    scope.ensure_writable()?;
    let source = scope.resolve(archive)?;
    if !fs::metadata(&source)?.is_file() {
        return Err(GuguError::InvalidPath(format!(
            "只有文件可以解压: {}",
            source.display()
        )));
    }
    let kind = ArchiveKind::of(&source).ok_or_else(|| {
        GuguError::InvalidPath(format!(
            "不支持的压缩格式: {}（支持 zip、tar、tar.gz / tgz）",
            source.display()
        ))
    })?;
    let target = match dest {
        Some(raw) => scope.resolve_new(raw)?,
        None => auto_target(scope, &source, kind)?,
    };
    if target.exists() {
        if !target.is_dir() {
            return Err(GuguError::InvalidPath(format!(
                "解压目标不是目录: {}",
                target.display()
            )));
        }
        if !overwrite {
            return Err(GuguError::InvalidPath(format!(
                "目标已存在: {}（需要覆盖请显式声明）",
                target.display()
            )));
        }
    }

    let staging = staging_path(&target);
    let mut stats = Stats::default();
    let unpacked = unpack_into(&source, kind, &staging, &mut stats);
    if let Err(error) = unpacked {
        let _ = fs::remove_dir_all(&staging);
        return Err(error);
    }
    if let Err(error) = collapse(&staging, &target) {
        let _ = fs::remove_dir_all(&staging);
        return Err(error);
    }
    if let Err(error) = place(&staging, &target) {
        let _ = fs::remove_dir_all(&staging);
        return Err(error);
    }
    Ok(Extracted {
        path: path_to_forward_slashes(&target),
        files: stats.files,
        dirs: stats.dirs,
        bytes: stats.bytes,
        skipped: stats.skipped,
    })
}

/// 缺省目标：归档所在目录下的同名目录。
fn auto_target(scope: &Scope, source: &Path, kind: ArchiveKind) -> Result<PathBuf> {
    let parent = source
        .parent()
        .ok_or_else(|| GuguError::InvalidPath(format!("归档没有父目录: {}", source.display())))?;
    let name = source
        .file_name()
        .map(|name| name.to_string_lossy().to_string())
        .unwrap_or_default();
    let target = parent.join(kind.stem(&name));
    scope.resolve_new(&path_to_forward_slashes(&target))
}

/// 临时解压目录与目标同盘同父，改名才是原子操作。
fn staging_path(target: &Path) -> PathBuf {
    let parent = target.parent().unwrap_or_else(|| Path::new("."));
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|duration| duration.as_nanos())
        .unwrap_or(0);
    let mut path = parent.join(format!(
        "{TEMP_PREFIX}unpack-{}-{nanos}",
        std::process::id()
    ));
    let mut bump = 0_u32;
    while path.exists() {
        bump += 1;
        path = parent.join(format!(
            "{TEMP_PREFIX}unpack-{}-{nanos}-{bump}",
            std::process::id()
        ));
    }
    path
}

/// 归档里只有一个与目标同名的顶层目录时把它提上来，避免 `pkg/pkg/...` 的双层嵌套。
///
/// 顶层目录名与目标名不同就不动：那多半是作者有意保留的一层，比如 `data.zip` 里的 `data-2026/`。
fn collapse(staging: &Path, target: &Path) -> Result<()> {
    let Some(wanted) = target.file_name() else {
        return Ok(());
    };
    let mut entries = fs::read_dir(staging)?;
    let Some(first) = entries.next() else {
        return Ok(());
    };
    let first = first?;
    if entries.next().is_some() {
        return Ok(());
    }
    if !first.file_type()?.is_dir() || first.file_name().as_os_str() != wanted {
        return Ok(());
    }

    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|duration| duration.as_nanos())
        .unwrap_or(0);
    let parked = staging.with_file_name(format!(
        "{TEMP_PREFIX}collapse-{}-{nanos}",
        std::process::id()
    ));
    fs::rename(staging, &parked).map_err(GuguError::Io)?;
    match fs::rename(parked.join(wanted), staging) {
        Ok(()) => {
            let _ = fs::remove_dir_all(&parked);
            Ok(())
        }
        Err(error) => {
            let _ = fs::rename(&parked, staging);
            Err(GuguError::Io(error))
        }
    }
}

/// 就位：目标不存在时直接改名；已存在时先把旧目录挪开，失败则挪回去。
fn place(staging: &Path, target: &Path) -> Result<()> {
    if !target.exists() {
        return fs::rename(staging, target).map_err(GuguError::Io);
    }
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|duration| duration.as_nanos())
        .unwrap_or(0);
    let backup = staging.with_file_name(format!("{TEMP_PREFIX}old-{}-{nanos}", std::process::id()));
    fs::rename(target, &backup).map_err(GuguError::Io)?;
    match fs::rename(staging, target) {
        Ok(()) => {
            let _ = fs::remove_dir_all(&backup);
            Ok(())
        }
        Err(error) => {
            let _ = fs::rename(&backup, target);
            Err(GuguError::Io(error))
        }
    }
}

#[derive(Default)]
struct Stats {
    files: usize,
    dirs: usize,
    bytes: u64,
    skipped: usize,
}

fn unpack_into(source: &Path, kind: ArchiveKind, root: &Path, stats: &mut Stats) -> Result<()> {
    fs::create_dir_all(root)?;
    match kind {
        ArchiveKind::Zip => unpack_zip(source, root, stats),
        ArchiveKind::Tar | ArchiveKind::TarGz => unpack_tar(source, kind, root, stats),
    }
}

fn unpack_zip(source: &Path, root: &Path, stats: &mut Stats) -> Result<()> {
    let file = File::open(source)?;
    let mut archive = zip::ZipArchive::new(file).map_err(|error| {
        GuguError::InvalidPath(format!("读取 zip 失败: {}（{error}）", source.display()))
    })?;
    for index in 0..archive.len() {
        let mut entry = archive.by_index(index).map_err(|error| {
            GuguError::InvalidPath(format!("读取 zip 第 {} 个条目失败: {error}", index + 1))
        })?;
        let Some(rel) = rel_path(entry.name()) else {
            stats.skipped += 1;
            continue;
        };
        if entry.is_dir() {
            fs::create_dir_all(root.join(rel))?;
            stats.dirs += 1;
            continue;
        }
        if !entry.is_file() {
            // 符号链接与设备文件：链接可能在解压后才生效，一律不落地。
            stats.skipped += 1;
            continue;
        }
        ensure_room(stats)?;
        let mode = entry.unix_mode();
        write_entry(&mut entry, root, &rel, mode, stats)?;
    }
    Ok(())
}

fn unpack_tar(source: &Path, kind: ArchiveKind, root: &Path, stats: &mut Stats) -> Result<()> {
    let file = File::open(source)?;
    let reader: Box<dyn Read> = match kind {
        ArchiveKind::TarGz => Box::new(flate2::read::GzDecoder::new(BufReader::new(file))),
        _ => Box::new(BufReader::new(file)),
    };
    let mut archive = tar::Archive::new(reader);
    let entries = archive.entries().map_err(|error| {
        GuguError::InvalidPath(format!("读取 tar 失败: {}（{error}）", source.display()))
    })?;
    for entry in entries {
        let mut entry =
            entry.map_err(|error| GuguError::InvalidPath(format!("读取 tar 条目失败: {error}")))?;
        let header_type = entry.header().entry_type();
        let raw = entry
            .path()
            .map(|path| path.to_string_lossy().to_string())
            .unwrap_or_default();
        let Some(rel) = rel_path(&raw) else {
            stats.skipped += 1;
            continue;
        };
        if header_type.is_dir() {
            fs::create_dir_all(root.join(rel))?;
            stats.dirs += 1;
            continue;
        }
        if !header_type.is_file() {
            stats.skipped += 1;
            continue;
        }
        ensure_room(stats)?;
        let mode = entry.header().mode().ok();
        write_entry(&mut entry, root, &rel, mode, stats)?;
    }
    Ok(())
}

fn ensure_room(stats: &Stats) -> Result<()> {
    if stats.files + stats.dirs >= MAX_ENTRIES {
        return Err(GuguError::InvalidPath(format!(
            "归档条目超过 {MAX_ENTRIES} 个，已停止解压"
        )));
    }
    Ok(())
}

/// 写入单个条目；按实际写出的字节计数，不信任头部声明的大小。
fn write_entry<R: Read>(
    reader: &mut R,
    root: &Path,
    rel: &Path,
    mode: Option<u32>,
    stats: &mut Stats,
) -> Result<()> {
    let out_path = root.join(rel);
    if let Some(parent) = out_path.parent() {
        fs::create_dir_all(parent)?;
    }
    let mut out = OpenOptions::new()
        .write(true)
        .create(true)
        .truncate(true)
        .open(&out_path)?;
    let room = MAX_EXTRACT_BYTES.saturating_sub(stats.bytes);
    let written = io::copy(&mut reader.take(room + 1), &mut out)?;
    if written > room {
        return Err(GuguError::InvalidPath(format!(
            "解压后超过 {} GB 上限，已停止",
            MAX_EXTRACT_BYTES / 1024 / 1024 / 1024
        )));
    }
    out.flush()?;
    drop(out);
    stats.bytes = stats.bytes.saturating_add(written);
    stats.files += 1;
    set_mode(&out_path, mode)?;
    Ok(())
}

/// 保留归档里的权限位（tar 的可执行位会用到）；其余平台忽略。
fn set_mode(path: &Path, mode: Option<u32>) -> Result<()> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        if let Some(mode) = mode {
            let mode = mode & 0o777;
            if mode != 0 {
                fs::set_permissions(path, fs::Permissions::from_mode(mode))?;
            }
        }
    }
    #[cfg(not(unix))]
    let _ = (path, mode);
    Ok(())
}

/// 条目名归一化成相对路径；绝对路径、盘符与 `..` 一律拒绝。
fn rel_path(raw: &str) -> Option<PathBuf> {
    if raw.is_empty() || raw.len() > MAX_NAME_BYTES {
        return None;
    }
    let normalized = raw.replace('\\', "/");
    if normalized.starts_with('/') {
        return None;
    }
    if normalized.len() > 1 && normalized.as_bytes()[1] == b':' {
        return None;
    }
    let mut out = PathBuf::new();
    for part in normalized.split('/') {
        if part == ".." {
            return None;
        }
        if !part.is_empty() && part != "." {
            out.push(part);
        }
    }
    if out.as_os_str().is_empty() {
        None
    } else {
        Some(out)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tests::support::tempdir;
    use std::io::Write;

    fn slash(path: &Path) -> String {
        path_to_forward_slashes(path)
    }

    fn write_zip(path: &Path, entries: &[(&str, &[u8])]) {
        let file = File::create(path).expect("建归档");
        let mut zip = zip::ZipWriter::new(file);
        let options = zip::write::SimpleFileOptions::default()
            .compression_method(zip::CompressionMethod::Deflated);
        for (name, body) in entries {
            zip.start_file(*name, options).expect("写条目");
            zip.write_all(body).expect("写内容");
        }
        zip.finish().expect("收尾");
    }

    fn write_tar_gz(path: &Path, entries: &[(&str, &[u8])], with_link: bool) {
        let file = File::create(path).expect("建归档");
        let encoder = flate2::write::GzEncoder::new(file, flate2::Compression::fast());
        let mut tar = tar::Builder::new(encoder);
        for (name, body) in entries {
            let mut header = tar::Header::new_gnu();
            header.set_size(body.len() as u64);
            header.set_mode(0o644);
            header.set_cksum();
            tar.append_data(&mut header, name, *body).expect("写条目");
        }
        if with_link {
            let mut header = tar::Header::new_gnu();
            header.set_entry_type(tar::EntryType::Symlink);
            header.set_size(0);
            header.set_mode(0o777);
            header.set_cksum();
            tar.append_link(&mut header, "link", "/etc/passwd")
                .expect("写链接");
        }
        tar.into_inner().expect("收尾").finish().expect("压缩收尾");
    }

    #[test]
    fn extracts_a_zip_beside_the_archive() {
        let dir = tempdir();
        let scope = Scope::new(vec![dir.path().to_path_buf()], true);
        let archive = dir.path().join("资料.zip");
        write_zip(&archive, &[("a.txt", b"alpha"), ("sub/中文.txt", b"beta")]);

        let done = extract(&scope, &slash(&archive), None, false).expect("解压");
        assert_eq!(done.files, 2);
        assert_eq!(done.bytes, 9);
        assert_eq!(done.skipped, 0);
        let target = dir.path().join("资料");
        assert_eq!(done.path, slash(&target));
        assert_eq!(fs::read(target.join("a.txt")).expect("读回"), b"alpha");
        assert_eq!(
            fs::read(target.join("sub/中文.txt")).expect("读回"),
            b"beta"
        );
        assert!(!dir.path().join(format!("{TEMP_PREFIX}unpack")).exists());
    }

    #[test]
    fn extracts_tar_gz_and_skips_links() {
        let dir = tempdir();
        let scope = Scope::new(vec![dir.path().to_path_buf()], true);
        let archive = dir.path().join("包.tar.gz");
        write_tar_gz(&archive, &[("x/y.txt", b"gamma")], true);

        let done = extract(&scope, &slash(&archive), None, false).expect("解压");
        assert_eq!(done.files, 1);
        assert_eq!(done.skipped, 1);
        let target = dir.path().join("包");
        assert_eq!(fs::read(target.join("x/y.txt")).expect("读回"), b"gamma");
        assert!(!target.join("link").exists(), "符号链接不该落地");
    }

    #[test]
    fn entry_names_cannot_escape_the_target() {
        let dir = tempdir();
        let scope = Scope::new(vec![dir.path().to_path_buf()], true);
        let archive = dir.path().join("evil.zip");
        write_zip(&archive, &[("../逃逸.txt", b"bad"), ("ok.txt", b"good")]);

        let done = extract(&scope, &slash(&archive), None, false).expect("解压");
        assert_eq!(done.skipped, 1);
        assert!(!dir.path().join("逃逸.txt").exists());
        assert!(dir.path().join("evil/ok.txt").is_file());

        assert_eq!(rel_path("../a"), None);
        assert_eq!(rel_path("/etc/passwd"), None);
        assert_eq!(rel_path("C:\\Windows\\win.ini"), None);
        assert_eq!(
            rel_path("a/./b.txt"),
            Some(PathBuf::from("a").join("b.txt"))
        );
    }

    #[test]
    fn refuses_an_existing_target_until_overwrite_is_declared() {
        let dir = tempdir();
        let scope = Scope::new(vec![dir.path().to_path_buf()], true);
        let archive = dir.path().join("data.zip");
        write_zip(&archive, &[("fresh.txt", b"new")]);
        let target = dir.path().join("data");
        fs::create_dir_all(&target).expect("建目录");
        fs::write(target.join("stale.txt"), b"old").expect("写旧文件");

        let error = extract(&scope, &slash(&archive), None, false).expect_err("应拒绝");
        assert!(error.to_string().contains("目标已存在"), "{error}");

        let done = extract(&scope, &slash(&archive), None, true).expect("覆盖");
        assert_eq!(done.files, 1);
        assert!(target.join("fresh.txt").is_file());
        assert!(!target.join("stale.txt").exists(), "覆盖是整目录替换");
        assert!(!dir
            .path()
            .read_dir()
            .expect("列目录")
            .filter_map(std::result::Result::ok)
            .any(|item| item.file_name().to_string_lossy().starts_with(TEMP_PREFIX)));
    }

    #[test]
    fn a_broken_archive_leaves_nothing_behind() {
        let dir = tempdir();
        let scope = Scope::new(vec![dir.path().to_path_buf()], true);
        let archive = dir.path().join("broken.zip");
        fs::write(&archive, b"not a zip").expect("写坏文件");

        let error = extract(&scope, &slash(&archive), None, false).expect_err("应失败");
        assert!(error.to_string().contains("zip"), "{error}");
        assert!(!dir.path().join("broken").exists());
        assert!(!dir
            .path()
            .read_dir()
            .expect("列目录")
            .filter_map(std::result::Result::ok)
            .any(|item| item.file_name().to_string_lossy().starts_with(TEMP_PREFIX)));
    }

    #[test]
    fn unknown_formats_and_read_only_scopes_are_refused() {
        let dir = tempdir();
        let writable = Scope::new(vec![dir.path().to_path_buf()], true);
        let archive = dir.path().join("a.7z");
        fs::write(&archive, b"7z").expect("写文件");
        let error = extract(&writable, &slash(&archive), None, false).expect_err("不支持的格式");
        assert!(error.to_string().contains("不支持"), "{error}");

        let readonly = Scope::new(vec![dir.path().to_path_buf()], false);
        let zip = dir.path().join("b.zip");
        write_zip(&zip, &[("a", b"a")]);
        assert!(matches!(
            extract(&readonly, &slash(&zip), None, false),
            Err(GuguError::ReadOnly)
        ));
    }

    #[test]
    fn collapses_a_top_level_directory_named_like_the_target() {
        let dir = tempdir();
        let scope = Scope::new(vec![dir.path().to_path_buf()], true);
        let archive = dir.path().join("pkg.zip");
        write_zip(
            &archive,
            &[("pkg/a.txt", b"alpha"), ("pkg/sub/b.txt", b"beta")],
        );

        let done = extract(&scope, &slash(&archive), None, false).expect("解压");
        assert_eq!(done.files, 2);
        let target = dir.path().join("pkg");
        assert!(target.join("a.txt").is_file(), "同名顶层目录应被提上来");
        assert!(target.join("sub/b.txt").is_file());
        assert!(!target.join("pkg").exists(), "不该出现 pkg/pkg");
    }

    #[test]
    fn keeps_a_top_level_directory_with_another_name() {
        let dir = tempdir();
        let scope = Scope::new(vec![dir.path().to_path_buf()], true);
        let archive = dir.path().join("data.zip");
        write_zip(&archive, &[("data-2026/a.txt", b"alpha")]);

        extract(&scope, &slash(&archive), None, false).expect("解压");
        let target = dir.path().join("data");
        assert!(
            target.join("data-2026/a.txt").is_file(),
            "名字不同就保留这一层"
        );
    }

    #[test]
    fn explicit_destination_is_honoured() {
        let dir = tempdir();
        let scope = Scope::new(vec![dir.path().to_path_buf()], true);
        let archive = dir.path().join("pkg.zip");
        write_zip(&archive, &[("only.txt", b"x")]);
        let dest = dir.path().join("落点");
        let done = extract(&scope, &slash(&archive), Some(&slash(&dest)), false).expect("解压");
        assert_eq!(done.path, slash(&dest));
        assert!(dest.join("only.txt").is_file());
    }
}
