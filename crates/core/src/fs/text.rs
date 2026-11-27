//! 文本文件读取与整体保存。
//!
//! 读取带上限并识别编码：UTF-8 严格解码失败时按 GBK 解，二进制文件只返回标记不返回正文。
//! 保存走「同目录临时文件 + 改名」，写入中途失败不会留下半个文件。

use serde::{Deserialize, Serialize};
use std::io::{Read, Write};

use crate::error::{GuguError, Result};
use crate::logs::LogEncoding;

use super::{entry, Scope};

/// 单次预览的读取上限。
pub const MAX_PREVIEW_BYTES: u64 = 1024 * 1024;
/// 单次保存的字节上限。
pub const MAX_WRITE_BYTES: usize = 8 * 1024 * 1024;
/// 二进制探测窗口：前 8 KB 里出现 0 字节即视为二进制。
const SNIFF_BYTES: usize = 8192;

/// 文本预览的结果。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TextFile {
    pub path: String,
    pub size: u64,
    /// 实际使用的解码：`utf-8` / `gbk` / `binary`。
    pub encoding: String,
    pub binary: bool,
    /// 文件比读取上限大，`content` 只是开头一段。
    pub truncated: bool,
    pub content: String,
}

/// 读取文本内容。
///
/// `limit` 为 0 时取默认上限；超过 [`MAX_PREVIEW_BYTES`] 的一律按上限读。
///
/// # Errors
/// 路径越界、不存在、是目录或读取失败时返回错误。
pub fn read(scope: &Scope, raw: &str, limit: u64) -> Result<TextFile> {
    let path = scope.resolve(raw)?;
    let meta = std::fs::metadata(&path)?;
    if meta.is_dir() {
        return Err(GuguError::InvalidPath(format!(
            "路径是目录: {}",
            path.display()
        )));
    }

    let limit = match limit {
        0 => MAX_PREVIEW_BYTES,
        value => value.min(MAX_PREVIEW_BYTES),
    };
    let mut bytes = Vec::with_capacity(limit.min(64 * 1024) as usize);
    std::fs::File::open(&path)?
        .take(limit + 1)
        .read_to_end(&mut bytes)?;
    let truncated = bytes.len() as u64 > limit;
    if truncated {
        bytes.truncate(limit as usize);
    }

    let sniffed = &bytes[..bytes.len().min(SNIFF_BYTES)];
    let binary = sniffed.contains(&0);
    let encoding = if binary {
        "binary"
    } else if std::str::from_utf8(&bytes).is_ok() {
        "utf-8"
    } else {
        "gbk"
    };
    let mut content = if binary {
        String::new()
    } else {
        LogEncoding::Auto.decode(&bytes)
    };
    // 上限切在多字节字符中间时，解码尾部会留下替换字符；它是截断的产物，不是文件内容。
    if truncated && content.ends_with('\u{FFFD}') {
        content.pop();
    }

    Ok(TextFile {
        path: crate::config::path_to_forward_slashes(&path),
        size: meta.len(),
        encoding: encoding.to_string(),
        binary,
        truncated,
        content,
    })
}

/// 整体保存文本：先写同目录临时文件，再改名覆盖目标。
///
/// `create` 为 false 时要求目标已存在，防止拼错路径时凭空造出文件。
///
/// # Errors
/// 只读模式、路径越界、目标不存在（`create = false`）、目标是目录或写入失败时返回错误。
pub fn write(scope: &Scope, raw: &str, content: &str, create: bool) -> Result<entry::Entry> {
    scope.ensure_writable()?;
    if content.len() > MAX_WRITE_BYTES {
        return Err(GuguError::InvalidPath(format!(
            "内容超过 {} MB，请改用上传",
            MAX_WRITE_BYTES / 1024 / 1024
        )));
    }

    let path = scope.resolve_new(raw)?;
    if path.is_dir() {
        return Err(GuguError::InvalidPath(format!(
            "路径是目录: {}",
            path.display()
        )));
    }
    if !create && !path.exists() {
        return Err(GuguError::NotFound(format!(
            "文件不存在: {}",
            path.display()
        )));
    }

    let parent = path
        .parent()
        .ok_or_else(|| GuguError::InvalidPath(format!("路径没有父目录: {}", path.display())))?;
    let temp = parent.join(format!(
        ".gugu-write-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_nanos())
            .unwrap_or(0)
    ));
    let written = (|| -> std::io::Result<()> {
        let mut file = std::fs::File::create(&temp)?;
        file.write_all(content.as_bytes())?;
        file.flush()?;
        file.sync_all()?;
        drop(file);
        std::fs::rename(&temp, &path)
    })();
    if let Err(error) = written {
        let _ = std::fs::remove_file(&temp);
        return Err(GuguError::Io(error));
    }

    entry::stat(scope, &crate::config::path_to_forward_slashes(&path))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::path_to_forward_slashes;
    use crate::tests::support::tempdir;

    #[test]
    fn round_trips_utf8_text_and_creates_a_missing_file() {
        let dir = tempdir();
        let scope = Scope::new(vec![dir.path().to_path_buf()], true);
        let target = path_to_forward_slashes(&dir.path().join("notes.md"));

        let saved = write(&scope, &target, "# 标题\n正文\n", true).expect("新建并写入");
        assert_eq!(saved.size, "# 标题\n正文\n".len() as u64);

        let read_back = read(&scope, &target, 0).expect("读回");
        assert_eq!(read_back.content, "# 标题\n正文\n");
        assert_eq!(read_back.encoding, "utf-8");
        assert!(!read_back.truncated);
        assert!(!read_back.binary);
    }

    #[test]
    fn decodes_gbk_and_truncates_on_a_char_boundary() {
        let dir = tempdir();
        let gbk = [0xC4, 0xE3, 0xBA, 0xC3]; // “你好”的 GBK 字节
        std::fs::write(dir.path().join("gbk.txt"), gbk).expect("写文件");
        let scope = Scope::new(vec![dir.path().to_path_buf()], true);
        let target = path_to_forward_slashes(&dir.path().join("gbk.txt"));

        let file = read(&scope, &target, 0).expect("读 GBK");
        assert_eq!(file.content, "你好");
        assert_eq!(file.encoding, "gbk");

        // 上限 3 字节切在第二个字中间：末尾的替换字符要被去掉。
        let cut = read(&scope, &target, 3).expect("截断读");
        assert!(cut.truncated);
        assert_eq!(cut.content, "你");
    }

    #[test]
    fn binary_files_are_flagged_without_body() {
        let dir = tempdir();
        std::fs::write(dir.path().join("blob.bin"), [0x00, 0x01, 0x02, 0x41]).expect("写文件");
        let scope = Scope::new(vec![dir.path().to_path_buf()], true);
        let file = read(
            &scope,
            &path_to_forward_slashes(&dir.path().join("blob.bin")),
            0,
        )
        .expect("读二进制");
        assert!(file.binary);
        assert_eq!(file.encoding, "binary");
        assert!(file.content.is_empty());
    }

    #[test]
    fn writing_needs_an_existing_file_unless_create_is_set() {
        let dir = tempdir();
        let scope = Scope::new(vec![dir.path().to_path_buf()], true);
        let target = path_to_forward_slashes(&dir.path().join("new.txt"));
        let error = write(&scope, &target, "x", false).expect_err("不新建应报错");
        assert!(error.to_string().contains("文件不存在"), "{error}");
        assert!(write(&scope, &target, "x", true).is_ok());
        assert!(write(&scope, &target, "y", false).is_ok(), "覆盖已存在文件");
    }

    #[test]
    fn readonly_scope_refuses_to_save() {
        let dir = tempdir();
        let scope = Scope::new(vec![dir.path().to_path_buf()], false);
        let target = path_to_forward_slashes(&dir.path().join("new.txt"));
        assert!(matches!(
            write(&scope, &target, "x", true),
            Err(GuguError::ReadOnly)
        ));
        assert!(!dir.path().join("new.txt").exists());
    }

    #[test]
    fn oversized_content_is_rejected_before_writing() {
        let dir = tempdir();
        let scope = Scope::new(vec![dir.path().to_path_buf()], true);
        let target = path_to_forward_slashes(&dir.path().join("big.txt"));
        let content = "a".repeat(MAX_WRITE_BYTES + 1);
        let error = write(&scope, &target, &content, true).expect_err("超限应拒绝");
        assert!(error.to_string().contains("超过"), "{error}");
    }
}
